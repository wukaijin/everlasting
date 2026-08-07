## Scenario: Provider trait + Anthropic dispatch (PR2 of06-08-multi-model-llm-provider-planning)

###1. Scope / Trigger

- Trigger: PR1 shipped the catalog (3 tables +8 CRUD +10 IPC +
 seed). PR2 introduces a `Provider` trait abstraction and a
 catalog-resolved dispatch in the `chat` command. All sessions
 still go through the Anthropic protocol (OpenAI is PR3) — the
 goal is the architectural refactor + the catalog path, with
 end-to-end behavior1:1 identical to pre-PR2.
- Why code-spec depth: mandatory — the trait surface is the
 cross-layer contract PR3 (OpenAI) and PR4 (UI) will build on;
 the catalog resolution +3-way pre-flight are the new failure
 modes the chat command can hit; the1:1 behavior contract
 protects the user from any wire-level regression.

###2. Signatures

#### Provider trait (`app/src-tauri/src/llm/provider/mod.rs`)

```rust
pub trait Provider: Send + Sync {
 fn send(
 &self,
 system: Option<String>,
 messages: Vec<ChatMessage>,
 tools: Vec<ToolDef>,
 ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>>;

 fn capabilities(&self) -> ProviderCapabilities;

 fn protocol(&self) -> ProviderProtocol;
}

pub struct ProviderCapabilities {
 pub supports_system_prompt: bool,
 pub supports_tools: bool,
 pub supports_streaming: bool,
}

pub enum ProviderBuildError {
 NotImplemented(&'static str), // e.g. "openai" in PR2
 UnknownProtocol(String), // forward-compat: a future binary wrote a value the current binary doesn't know
}

pub fn build_provider(
 provider_row: &db::ProviderRow,
 model_row: &db::ModelRow,
) -> Result<Box<dyn Provider>, ProviderBuildError>;
```

#### `AnthropicProvider` (`app/src-tauri/src/llm/provider/anthropic.rs`)

The Anthropic adapter — the `chat_stream_with_tools` body from
pre-PR2 `client.rs`, moved verbatim into
`AnthropicProvider::chat_stream_with_tools` and exposed via
`impl Provider for AnthropicProvider`. The `LlmConfig` struct
moves into this module (private to the adapter) and is
re-exported at the `llm` module level for `AppState::load`'s
env-fallback path.

```rust
pub struct AnthropicProvider { config: LlmConfig }
impl AnthropicProvider {
 pub fn new(config: LlmConfig) -> Self;
}
impl Provider for AnthropicProvider { ... }
```

#### `chat` command pre-flight (`app/src-tauri/src/agent/provider.rs`)

```rust
struct ResolvedChatProvider {
 provider: Box<dyn llm::Provider>,
 model_display_name: String,
 provider_display_name: String,
}

enum PreFlightError {
 NoModel, // PRD Q2 #2
 ProviderMissing, // PRD Q2 #3
 EmptyApiKey { provider_display_name: String }, // PRD Q2 #1
 BuildFailed(llm::ProviderBuildError), // generic dispatcher error
}

async fn resolve_chat_provider(
 db: &SqlitePool,
) -> Result<ResolvedChatProvider, PreFlightError>;
```

#### `get_llm_config` IPC (`app/src-tauri/src/commands/config.rs`)

The IPC is now `async` and reads the catalog. The wire shape is
unchanged (`{model, baseUrl, configured}`); the `model` field
now carries `ModelRow.display_name` (e.g. "Claude Sonnet4.5")
per the PR2 PRD D1 decision.

###3. Contracts

####1:1 wire behavior (the only hard constraint of PR2)

| Concern | Pre-PR2 (env path) | Post-PR2 (catalog path) |
|---------|-------------------|------------------------|
| Request URL | `ANTHROPIC_BASE_URL + "/v1/messages"` | `provider_row.base_url + "/v1/messages"` (same shape; base_url is now a catalog value, not an env value) |
| Headers | `x-api-key: <ANTHROPIC_API_KEY>`, `anthropic-version:2023-06-01`, `content-type: application/json` | Identical; `api_key` is `provider_row.api_key` |
| `thinking` field | `{type: "adaptive", display: "summarized", effort: LLM_THINKING_EFFORT \|\| "high"}` | `{type: "adaptive", display: "summarized", effort: model.thinking_effort \|\| "high"}` |
| `max_tokens` | `LLM_MAX_TOKENS` \|\|16384 | `model.max_tokens` \|\|16384 |
| `model` field | `LLM_MODEL` | `model.model_name` |
| Tool definitions | `builtin_tools()` | Identical (catalog dispatch doesn't touch tools) |
| SSE event sequence | text / tool_use / thinking / redacted_thinking + signature_delta | Identical (the `BlockState` state machine is unchanged) |

#### Provider dispatch timing

The provider is constructed **once per `chat` invocation**,
before the `for turn in1..=MAX_TURNS` loop, and the same
`Box<dyn Provider>` is used for all20 turns. The user's
protocol choice is stable within a chat — switching protocol
requires starting a new chat (the same invariant as the
pre-PR2 env path, which was loaded once at startup).

#### Catalog resolution (read order)

1. `app_config["default_model_id"]` → `model_id`
2. `list_models()` → find row with `id == model_id`
3. `list_providers()` → find row with `id == model_row.provider_id`
4. Pre-flight: `provider.api_key.is_empty()`?
5. `llm::build_provider(provider_row, model_row)` → `Box<dyn Provider>`

If any step fails, the chat command emits a `ChatEvent::Error`
with the locked-in PRD §Q2 message and returns Ok (the error
travels over the `chat-event` Tauri channel, not the IPC return
value, so the frontend's existing error rendering path applies).

#### Pre-flight error messages (PRD §Q2, locked)

| Failure | `message` | `category` |
|---------|-----------|-----------|
| `default_model_id` unset or model row missing | `"没有可用 model,请到 Settings选 default model"` | `InvalidRequest` |
| `model_row.provider_id` points to a deleted provider | `"default model指向的 provider已被删除,请到 Settings 重选"` | `InvalidRequest` |
| `provider_row.api_key` empty | `"请到 Settings填 {provider_display_name} 的 api_key"` | `Auth` |
| `build_provider` returns `NotImplemented("openai")` / `UnknownProtocol(...)` | `"无法构造 LLM provider: {error}"` | `InvalidRequest` |

The `category` drives the PR4 Settings modal's auto-jump logic
(modal opens the right tab based on the failure kind).

#### `get_llm_config` catalog path

Resolution: `default_model_id` → `models` → `providers` → reads
`provider.base_url` and `provider.api_key` (the latter for the
`configured` flag only). If any step is missing (no default, no
model row, no provider row), returns:

```json
{ "model": "", "baseUrl": "", "configured": false }
```

— the frontend's existing "no model configured" warning renders
as before. This IPC and the `chat` command both go through the
catalog exclusively; there is no env fallback path (the legacy
`state.config` / `LlmConfig::from_env` cold-start fallback was
removed once the catalog became the sole source of truth).

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `app_config["default_model_id"]` is `NULL` | `PreFlightError::NoModel` → "没有可用 model..." |
| `default_model_id` points to a deleted `models` row | `PreFlightError::NoModel` → "没有可用 model..." |
| `default_model_id` points to a row whose `provider_id` is dangling | `PreFlightError::ProviderMissing` → "default model指向的 provider..." |
| `provider.api_key` is empty string | `PreFlightError::EmptyApiKey { provider_display_name }` → "请到 Settings填 {display_name} 的 api_key" |
| `provider.protocol` is `"openai"` (PR2: not yet implemented) | `PreFlightError::BuildFailed(NotImplemented("openai"))` → "无法构造 LLM provider: provider protocol 'openai' is not implemented yet" |
| `provider.protocol` is an unknown string (forward-compat) | `PreFlightError::BuildFailed(UnknownProtocol(s))` → "无法构造 LLM provider: unknown provider protocol: 's'" |
| `provider.api_key` is set, model row has `max_tokens=Some(8192)` | `AnthropicProvider::new(LlmConfig { max_tokens:8192, ... })` → request body has `"max_tokens":8192` |
| `provider.api_key` is set, model row has `max_tokens=None` | Factory falls back to `16384` (the Anthropic default) |
| `provider.api_key` is set, model row has `thinking_effort=Some("xhigh")` | Request body has `"thinking": {"type": "adaptive", "display": "summarized", "effort": "xhigh"}` |
| `provider.api_key` is set, model row has `thinking_effort=None` | Falls back to `"high"` (matches the pre-PR2 env default) |
| `provider_row.base_url` has trailing `/` | `endpoint()` strips it (matches pre-PR2 `LlmConfig::endpoint()` behavior) |
| `get_llm_config` called before any default is set | Returns `Ok(PublicLlmConfig { model: "", base_url: "", configured: false })` |

###5. Good / Base / Bad Cases

#### Good: full happy path

1. User opens the app for the first time. Seed runs:
 `Anthropic官方` (empty api_key) + `claude-sonnet-4-5`
 bound to it + `default_model_id` = the sonnet row.
2. User goes to Settings, pastes their `sk-ant-...` into
 `Anthropic官方.api_key`.
3. User opens a session, types a question, clicks Send.
4. Frontend calls `invoke("chat", { requestId, sessionId, messages })`.
5. Backend `chat`:
 - `resolve_chat_provider(db)`:
 - reads `app_config["default_model_id"]` → sonnet UUID
 - finds the sonnet `ModelWithProvider` row
 - finds the `Anthropic官方` provider row
 - `api_key` is non-empty → pre-flight OK
 - `llm::build_provider(provider_row, sonnet)` returns
 `Box<dyn Provider>` (an `AnthropicProvider`)
 - `provider.send(system, messages, tools)` per turn
 - request URL = `https://api.anthropic.com/v1/messages`
 - headers / `thinking` field / SSE event handling: identical
 to pre-PR2
6. User sees the same response stream as before PR2.

#### Base: missing default model

1. User deletes all providers / models in Settings.
2. `app_config["default_model_id"]` still references the (now
 deleted) sonnet UUID.
3. User sends a message.
4. `resolve_chat_provider`:
 - reads the sonnet UUID
 - `list_models` returns `[]` (cascade-deleted)
 - returns `PreFlightError::NoModel`
5. Chat command emits `ChatEvent::Error { message: "没有可用
 model,请到 Settings选 default model", category: InvalidRequest }`
6. Frontend shows the error in the chat panel + (post-PR4)
 "跳到 Settings" button.

#### Bad: per-event `signature_delta` emit (regression check)

PR2 must not regress the step6 signature-buffer fix (see the
earlier "Scenario: Extended Thinking Support" section). The
`AnthropicProvider::chat_stream_with_tools` body is a verbatim
move of the pre-PR2 implementation, so the
`BlockState::Thinking { signature_buf }` buffering is
preserved. The4 client.rs tests in
`provider/anthropic::tests::*` (`default_max_tokens`,
`thinking_config_is_adaptive_summarized_with_configured_effort`,
`unconfigured_has_empty_thinking_effort`,
`chat_request_system_field_serializes_when_some`) are the
regression net.

#### Bad: tool envelope lost

PR2 must not regress the step4 follow-up tool envelope
(`{"result": "<content>", "cwd": "<worktree_path>"}`). The
envelope is applied in `agent::chat::chat` at the agent-loop
boundary, NOT inside the provider. The tool returns the raw
string; the chat command wraps it via `tool_result_envelope(...)` before
emitting on the `tool:result` IPC channel (via `ChatEventSink::emit_tool_result`)
and persisting the `ContentBlock::ToolResult`. There is no
`ChatEvent::ToolResult` variant — that was a never-implemented
stale design that was removed 2026-06-24 (see DEBT RULE-A-009 修 2c).
The pre-existing `tool_result_envelope_round_trip` test in
`agent::helpers::tests` continues to lock this.

###6. Tests Required

The4 pre-existing client.rs tests moved into
`provider/anthropic::tests` (unchanged).7 new tests added in
PR2:

| Test | Asserts |
|------|---------|
| `llm::provider::tests::build_provider_anthropic_returns_anthropic_provider` | `protocol() == Anthropic`, all3 capabilities true |
| `llm::provider::tests::build_provider_openai_returns_not_implemented` | `ProviderBuildError::NotImplemented("openai")` |
| `llm::provider::tests::build_provider_unknown_protocol_returns_error` | `ProviderBuildError::UnknownProtocol(s)` for unknown strings |
| `llm::provider::tests::factory_passes_model_max_tokens` | Factory threads `model.max_tokens` into `LlmConfig` (verified via successful construction) |
| `llm::provider::tests::factory_falls_back_to_default_max_tokens_and_effort` | `None` model overrides → `max_tokens=16384`, `thinking_effort="high"` |
| `llm::provider::tests::provider_build_error_displays_human_readable` | `ProviderBuildError` impls `Display` (used in `tracing::warn!` / IPC error path) |
| `llm::provider::tests::provider_protocol_reexport_matches_db` | `llm::ProviderProtocol` re-export is the same enum as `db::ProviderProtocol` |
| `llm::provider::anthropic::tests::anthropic_provider_reports_capabilities_and_protocol` | `AnthropicProvider::protocol() == Anthropic`, capabilities all true |
| `llm::provider::anthropic::tests::anthropic_provider_is_send_sync` | `AnthropicProvider: Send + Sync` (compile-time assertion, `Box<dyn Provider>` is movable) |
| `llm::provider::anthropic::tests::factory_built_provider_reports_anthropic_capabilities` | End-to-end: `build_provider` → `protocol()` + `capabilities()` |

Pre-existing test count:208 (pre-PR2). PR2 net new:10 tests
(7 in `llm::provider::tests` +3 in
`llm::provider::anthropic::tests`). Total:218 (verified via
`cargo test --lib` —0 warnings,0 failures).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. PR2 does NOT change
 the frontend; the `get_llm_config` IPC's wire shape
 (`{model, baseUrl, configured}`) is preserved, the
 `useConfigStore.load()` code is untouched.
- Manual smoke test (acceptance A2 from the parent PRD):
1. `cd app && pnpm tauri dev`
2. Open Settings, see the2 seeded providers +4 seeded
 models. Default is `claude-sonnet-4-5`.
3. Open a session, type a question, click Send.
4. Observe the LLM responds — same wire behavior as pre-PR2.
5. In Settings, delete the `claude-sonnet-4-5` model (or
 blank its `provider.api_key`).
6. Try to send a message; observe the locked-in PRD §Q2
 error message ("请到 Settings填 Anthropic官方的 api_key"
 or "没有可用 model...").

###7. Wrong vs Correct

#### Wrong: per-turn provider construction

```rust
// BAD — constructs a new provider per turn; user-visible
// protocol drift if a different default is selected mid-loop
for turn in1..=MAX_TURNS {
 let provider = build_provider(&provider_row, &model_row)?;
 let mut stream = provider.send(...);
 // ...
}
```

If `attach_worktree` / `set_default_model` runs between turns
(via a second IPC in another window), the per-turn
construction would silently switch providers — a wire-protocol
inconsistency Anthropic would400 on.

#### Correct: once-per-chat construction

```rust
// GOOD — resolve once, reuse for all20 turns
let resolved = resolve_chat_provider(&db).await?;
let provider = resolved.provider;
for turn in1..=MAX_TURNS {
 let mut stream = provider.send(...);
 // ...
}
```

The provider is stable for the lifetime of the chat
invocation. The agent loop's `tokio::select!` already
listens for the cancellation token between turns, so a
destructive `set_default_model` cannot race the agent loop.

#### Wrong: chat dispatch bypasses the catalog

```rust
// BAD — bypasses the catalog; `default_model_id` is ignored;
// the user's model choice in Settings is decorative. (Pre-PR2
// this was the env path; post-cleanup there is no `state.config`
// to clone either, so this no longer type-checks — which is the
// point: the type system now enforces catalog-only dispatch.)
let config = state.config.clone();
let mut stream = chat_stream_with_tools(config, ...);
```

This was the pre-PR2 behavior (env path) — exactly what PR2
removes. The catalog is the source of truth, and after the env
fallback cleanup `AppState` no longer carries a `config` field,
so this bypass is no longer expressible.

#### Correct: resolve the provider from the catalog per chat

```rust
// GOOD — the `chat` command resolves provider + model from the
// DB catalog via `resolve_chat_provider` on every invocation.
let resolved = resolve_chat_provider(&db).await?;
let mut stream = resolved.provider.send(...);
```

There is no `state.config` and no env fallback; the catalog is
the single source of truth for provider/model/api_key/base_url.

#### Wrong: pre-flight messages not in the locked-in PRD §Q2 copy

```rust
// BAD — ad-hoc message; PR4's Settings modal auto-jump
// logic can't read it
PreFlightError::EmptyApiKey => ChatEvent::Error {
 message: "API key missing".to_string(),
 category: LlmErrorCategory::Auth,
},
```

The PR4 Settings modal's auto-jump logic needs the PRD §Q2
copy verbatim so it can decide which tab to open
(`api_key` tab vs. `default model` picker vs.
`re-select model` recovery).

#### Correct: PRD §Q2 copy

```rust
// GOOD — matches PRD §Q2 table verbatim
PreFlightError::EmptyApiKey { provider_display_name } => {
 (format!("请到 Settings填 {} 的 api_key", provider_display_name),
 LlmErrorCategory::Auth)
}
```

PR4's modal: the `category` is `Auth` → jump to the
provider's `api_key` field. `InvalidRequest` + "没有可用
model" → jump to the default-model picker. `InvalidRequest`
+ "default model指向的 provider..." → jump to the
re-select flow.

### Design Decisions

#### Decision: `ProviderProtocol` lives in `db`, re-exported by `llm`

**Context**: PR1 already added `db::ProviderProtocol` (the
Anthropic / Openai enum) with `as_str` / `from_str_opt`. PR2
needs the same enum on the `Provider` trait's
`protocol() -> ProviderProtocol` method. Putting it in two
places is a maintenance hazard; putting it in `llm` instead
of `db` would require a PR1 schema docstring rewrite.

**Decision**: The enum lives in `db` (where PR1 put it).
`llm::provider` re-exports it as `llm::ProviderProtocol`.
The trait's `protocol()` method returns the same enum.

**Consequences**:
- ✅ PR1's docstrings / DB tests are unchanged.
- ✅ `llm::ProviderProtocol` is a thin re-export — downstream
 code doesn't need to know `db` exists.
- ⚠️ Future protocol additions land in `db` first, then
 propagate to `llm` (the existing `from_str_opt` lenient
 parse covers forward-compat).

#### Decision: `LlmConfig` is private to `provider/anthropic`, re-exported

**Context**: `LlmConfig` was a public type in pre-PR2
`llm::client` — the chat command built one and the
`AppState::load` constructor read env. PR2's `chat` command
no longer builds `LlmConfig` (the factory does); only
`AppState::load` still reads env.

**Decision**: `LlmConfig` is now a module-private type
inside `provider::anthropic`. The `llm` module re-exports
it for `AppState::load`'s `LlmConfig::from_env` import
path.

**Consequences**:
- ✅ The factory is the only builder of `LlmConfig` in
 the **chat command path** (which is the only path that
 talks to the LLM); invariant that "every `AnthropicProvider`
 has its config sourced from a catalog row" is enforced by
 the type being private to that path. `AppState::load` still
 calls `LlmConfig::from_env()` directly for the cold-start
 env-fallback `state.config` field — that value is not used
 by chat or `get_llm_config` after PR2.
- ✅ `AppState::load` keeps its `llm::LlmConfig` import (no
 churn outside the LLM module).
- ⚠️ PR3 (OpenAI) may want a separate `OpenAIConfig` struct;
 the re-export is `provider::anthropic::LlmConfig`-specific
 and the OpenAI provider will have its own.

> **Update (2026-07, env fallback removal):** the env path
> described above is now gone. `LlmConfig::from_env` /
> `unconfigured` / `is_unconfigured` were deleted, `AppState`
> no longer has a `config` field, and `llm::LlmConfig` is no
> longer re-exported (no external consumers remain). The
> factory in `build_provider` is the sole constructor. The
> core invariant — "every `AnthropicProvider` has its config
> sourced from a catalog row" — is now enforced unconditionally
> rather than just on the chat path.

#### Decision:7 new tests, not5

**Context**: The PR2 PRD said "4-5 new build_provider +
factory tests". The implementation added10 new tests
(7 in `llm::provider::tests` +3 in
`llm::provider::anthropic::tests`) to cover the catalog
wiring, the Send + Sync invariant, and the Anthropic
provider's re-export of `db::ProviderProtocol`.

**Decision**:10 new tests. The Send + Sync assertion is
the load-bearing one — it locks the `Box<dyn Provider>`
move-into-spawn pattern that the chat command relies on.
The `provider_protocol_reexport_matches_db` test guards
against accidental enum duplication.

**Consequences**: Test count is higher than the PRD's
estimate, but each test guards a real invariant; none
are "for coverage" filler.

---
