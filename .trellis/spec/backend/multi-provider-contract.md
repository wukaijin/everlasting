# Multi-Provider Contract — Provider Trait + Catalog + Anthropic/OpenAI Dispatch

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) (本文) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `Provider` trait / `WireMessage` 中间层 / `AnthropicProvider` / `OpenAIProvider` / `build_provider` factory / cross-protocol strip / catalog resolution 时。

---

## Scenario: Multi-Provider Abstraction (PR1 of06-08-multi-model-llm-provider-planning)

###1. Scope / Trigger

- Trigger: User-managed catalog of LLM providers + models. PR1 ships
 the data layer (3 new SQLite tables,8 CRUD functions,10 IPC
 commands, idempotent seed); PR2 (Anthropic adapter) and PR3 (OpenAI
 adapter) implement the `Provider` trait dispatch off this catalog.
- Why code-spec depth: mandatory — the new tables, IPC payload shapes,
 and `ProviderProtocol` enum are the cross-layer contract that PR2 /
 PR3 / PR4 all depend on. A change here cascades.

###2. Signatures

#### DB types (`app/src-tauri/src/db/types.rs`)

```rust
pub enum ProviderProtocol {
 Anthropic, // Messages API
 Openai, // Chat Completions
}

pub struct ProviderRow {
 pub id: String, // UUID v4
 pub protocol: String, // "anthropic" | "openai" (TEXT, not enum, for forward-compat)
 pub display_name: String, // user-facing label
 pub base_url: String,
 pub api_key: String,
 pub created_at: String, // RFC3339
 pub updated_at: String,
}

pub struct ModelRow {
 pub id: String, // UUID v4
 pub provider_id: String, // FK to providers.id (ON DELETE CASCADE)
 pub model_name: String, // sent to API
 pub display_name: String, // UI label
 pub max_tokens: Option<u32>, // None = fall back to global
 pub thinking_effort: Option<String>,// None = fall back to global
 pub supports_thinking: bool, // capabilities bit
 pub context_window: u32, // total capacity (input+output)
 pub created_at: String,
 pub updated_at: String,
}

pub struct ModelWithProvider {
 #[serde(flatten)]
 pub model: ModelRow,
 pub provider_display_name: String, // denormalized for UI list
 pub provider_protocol: String,
}
```

#### Tables (PR1 schema)

```sql
CREATE TABLE providers (
 id TEXT PRIMARY KEY,
 protocol TEXT NOT NULL,
 display_name TEXT NOT NULL,
 base_url TEXT NOT NULL,
 api_key TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);

CREATE TABLE models (
 id TEXT PRIMARY KEY,
 provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
 model_name TEXT NOT NULL,
 display_name TEXT NOT NULL,
 max_tokens INTEGER, -- nullable
 thinking_effort TEXT, -- nullable
 supports_thinking INTEGER NOT NULL DEFAULT0,
 context_window INTEGER NOT NULL,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);
CREATE INDEX idx_models_provider_id ON models(provider_id);

CREATE TABLE app_config (
 key TEXT PRIMARY KEY,
 value TEXT NOT NULL
);

-- sessions (existing table) gains a soft FK column:
ALTER TABLE sessions ADD COLUMN model_id TEXT;
CREATE INDEX idx_sessions_model_id ON sessions(model_id);
```

Note: `sessions.model_id` is a **soft FK** — no `REFERENCES models(id)`
constraint. The agent loop (PR2) is responsible for the resolve-default
fallback when `model_id` is NULL or dangling. See "Soft FK pattern" in
`database-guidelines.md` for the rationale.

#### IPC commands (registered in `lib.rs::invoke_handler!`)

| Command | Args (Rust) | Returns |
|---|---|---|
| `list_providers` | — | `Vec<ProviderRow>` |
| `add_provider` | `protocol, display_name, base_url, api_key` | `ProviderRow` |
| `update_provider` | `id, protocol, display_name, base_url, api_key` | `Option<ProviderRow>` |
| `delete_provider` | `id` | `bool` (cascades to models) |
| `list_models` | — | `Vec<ModelWithProvider>` |
| `add_model` | `provider_id, model_name, display_name, max_tokens?, thinking_effort?, supports_thinking, context_window` | `ModelRow` |
| `update_model` | `id, provider_id, model_name, display_name, max_tokens?, thinking_effort?, supports_thinking, context_window` | `Option<ModelRow>` |
| `delete_model` | `id` | `bool` (leaves dangling `sessions.model_id`) |
| `get_default_model` | — | `Option<ModelWithProvider>` (joins via `app_config.default_model_id`) |
| `set_default_model` | `model_id` | `()` |

###3. Contracts

#### Wire shape (camelCase to JS via `#[serde(rename_all = "camelCase")]`)

```jsonc
// list_providers response
[
 { "id": "uuid", "protocol": "anthropic", "displayName": "Anthropic官方",
 "baseUrl": "https://api.anthropic.com", "apiKey": "sk-...",
 "createdAt": "...", "updatedAt": "... }
]

// list_models response (ModelWithProvider.flatten)
[
 { "id": "uuid", "providerId": "uuid", "modelName": "claude-sonnet-4-5",
 "displayName": "Claude Sonnet4.5", "maxTokens": null,
 "thinkingEffort": null, "supportsThinking": true,
 "contextWindow":200000, "createdAt": "...", "updatedAt": "...",
 "providerDisplayName": "Anthropic官方", "providerProtocol": "anthropic" }
]
```

#### IPC arg names (Tauri2 auto-converts from JS camelCase to Rust snake_case)

```typescript
// JS — camelCase
await invoke("add_model", {
 providerId: "uuid",
 modelName: "claude-sonnet-4-5",
 displayName: "Claude Sonnet4.5",
 maxTokens:8192, // number or omit for None
 thinkingEffort: "high", // string or omit for None
 supportsThinking: true,
 contextWindow:200000,
})
```

#### `Option<T>` args — HACKING-wsl FU-1 pattern

For `add_model` / `update_model` `Option<u32>` and `Option<String>` args
(`max_tokens`, `thinking_effort`):

- **JS omit the field for `None`** — do NOT pass `null`. Tauri2 IPC
 treats `null` as missing required, and the error message hides the
 field name.
- Rust `Option::None` is the wire-level "not set" — corresponds to
 `NULL` in the DB column.
- `Some(value)` is wire-level "set" — writes the value.

#### Default model

`app_config` is a small key/value store. The only key today is
`default_model_id` (UUID string). `get_default_model` resolves it via
`list_models` and finds the matching row — returns `None` if the key
is unset or the model was deleted.

#### Env-keys (PR1 does NOT add new env keys)

- `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
 `LLM_MODEL`, `LLM_MAX_TOKENS`, `LLM_THINKING_EFFORT` — still read
 by `LlmConfig::from_env()` in `llm/client.rs`. PR1 keeps the env
 path; the new catalog co-exists as a parallel source.
- The IPC `get_llm_config` command still returns the env-derived
 config (for backward compat); `get_default_model` returns the
 catalog-derived default. PR2 will replace `get_llm_config` with a
 catalog read.

###4. Validation & Error Matrix

| Condition | Error |
|---|---|
| `add_model` with `provider_id` not in `providers` | FK violation (SQLITE_CONSTRAINT) — wrapped as `String` at IPC |
| `add_model` with `context_window:0` | Accepted (no min validation); UI should prevent in form |
| `update_provider` / `update_model` on missing `id` | `None` returned (NOT an error) |
| `delete_provider` on missing `id` | `false` returned |
| `get_default_model` when `default_model_id` is unset | `None` returned |
| `get_default_model` when `default_model_id` points to a deleted model | `None` returned (the `list_models` filter finds no match) |
| `set_default_model` with non-existent `model_id` | Accepted (no FK validation); the next `get_default_model` returns `None`. PR4 will add the pre-flight check. |

###5. Good / Base / Bad Cases

- **Good**: User opens Settings → adds "Anthropic官方" provider with
 their `sk-ant-...` key → adds "claude-sonnet-4-5" model under it →
 sets it as default. `get_default_model` returns the model. New
 sessions auto-pick it. UI shows `(no model)` warning until key is
 filled.
- **Base**: Default seed runs on first install; user never opens
 Settings. The2 seeded providers +4 models +1 default give the
 app enough to function; the user just needs to fill `api_key` for
 the provider they want to use.
- **Bad**: User adds a "claude-sonnet-4-5" model to a provider with
 no `api_key` and tries to send a message. PR2's pre-flight check
 should reject this with a "请先到 Settings填 api_key" toast.
 PR1's `set_default_model` accepts it (the value is stored); PR2
 enforces.

###6. Tests Required

- [] `cargo test --lib db::` —11 new tests (covered PR1, see
 `db.rs` `#[cfg(test)] mod tests` PR1 section)
- [] Each CRUD function:1 happy +1 error path
- [] Cascade: `delete_provider` removes its models
- [] Cascade: `delete_provider` does NOT touch other providers' models
- [] Seed: idempotent (running twice doesn't double the catalog)
- [] Seed: sets `default_model_id` to a real model
- [] Seed: backfills `sessions.model_id` for legacy rows
- [] `app_config` round-trip: set + get + overwrite

### Design Decisions

#### Decision: `ProviderProtocol` enum is forward-compatible (lenient parse)

**Context**: Adding a new protocol (Ollama, Gemini) will ship in a
later release. The current binary's `from_str_opt` reads from a DB that
may already have a row with the new protocol string.

**Decision**: Unknown protocol strings fall back to `Anthropic` (the
default). The new binary's `Provider` dispatch checks for
`ProviderProtocol::Openai` first, then falls back to Anthropic — so an
old binary on a new DB doesn't crash, it just treats the new protocol
as Anthropic and likely fails at the HTTP layer (which is the desired
behavior: the user upgrades to the new release to use the new
protocol).

**Consequences**: A release that adds a new protocol must not change
the `from_str_opt` default — otherwise old binaries on new DBs would
crash on read.

#### Decision: `sessions.model_id` is a soft FK (no `REFERENCES`)

**Context**: `model_id` is added to `sessions` via a non-destructive
`ALTER TABLE`; the column must accept `NULL` for pre-existing rows;
and a `REFERENCES` constraint would reject `INSERT` of legacy
sessions with dangling `model_id`.

**Decision**: Soft FK — the column is plain `TEXT` with no `REFERENCES
models(id)`. The read path (PR2's `chat` command) is responsible for
the resolve-default fallback when `model_id` is NULL or the model
row was deleted.

**Consequences**: The DB will not enforce referential integrity on
`model_id`. A deleted model leaves dangling references in `sessions`;
this is acceptable because the resolve-default fallback handles it
transparently. The hard FK is only used where the child is meaningless
without the parent (e.g. `models.provider_id`).

#### Decision:10 IPC commands, not the prd's7-8

**Context**: The prd's PR1 estimate was7-8 CRUD IPC. PR1 ships10
because `get_default_model` and `set_default_model` are exposed as
typed IPC commands rather than a generic `get_app_config(key)` /
`set_app_config(key, value)`.

**Decision**:10 commands. The typed shape is self-documenting in the
`invoke_handler!` macro and gives the frontend a clear contract.

**Consequences**: Slightly more boilerplate at the IPC layer, but the
catalog API is explicit. If a future `app_config` key needs a typed
IPC, add another pair rather than a generic accessor.

#### Decision: idempotent seed on first run (no migration version)

**Context**: The seed inserts2 providers +4 models + a default
model id. A user who deletes everything shouldn't get the seed
re-run; a user who never opened the app should get it on first
launch.

**Decision**: Gate the seed on `SELECT COUNT(*) FROM providers =0`.
This is a one-time, irreversible trigger.

**Consequences**: If we ever change the default catalog (e.g. add
"claude-opus-4-8" to the seed list), existing installs won't pick it
up. The recovery path is "manually add the new model in Settings".

---

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
as before. The env path is no longer read for this IPC (it
remains as `state.config` for any future fallback, but the
`chat` command and `get_llm_config` both go through the
catalog).

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
boundary, NOT inside the provider. The provider returns
`ChatEvent::ToolResult { content: <raw string>, ... }` and the
chat command wraps it via `tool_result_envelope(...)` before
emitting `tool:result` and persisting the `ContentBlock::ToolResult`.
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

#### Wrong: `state.config` reused for chat dispatch

```rust
// BAD — bypasses the catalog; PR1's `default_model_id` is
// ignored; the user's model choice in Settings is decorative
let config = state.config.clone();
let mut stream = chat_stream_with_tools(config, ...);
```

This was the pre-PR2 behavior (env path) — exactly what PR2
removes. The catalog is now the source of truth.

#### Correct: `state.config` is env-fallback only

```rust
// GOOD — env is read once at startup, kept on AppState for
// `LlmConfig::from_env` symmetry / future fallback, but the
// `chat` command reads the catalog via `resolve_chat_provider`.
let resolved = resolve_chat_provider(&db).await?;
let mut stream = resolved.provider.send(...);
```

`state.config` is preserved on `AppState` (the env-fallback
path is intact), but the chat command does not touch it.

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

## Future Work (Deferred from PR2 → resolved in PR3)

| Item | Why deferred / resolved |
|------|-------------------------|
| OpenAI adapter | **Resolved in PR3** — `app/src-tauri/src/llm/provider/openai.rs` ships `OpenAIProvider::new` + `impl Provider`; see the PR3 section below. |
| Cross-protocol `WireMessage` intermediate type | **Resolved in PR3** — `app/src-tauri/src/llm/provider/wire.rs` ships `WireRequest` / `WireMessage` / `WireBlock` / `WireCapabilities` + `chat_request_to_wire` + `strip_unsupported` + `wire_messages_to_chat_messages`. |
| `ProviderCapabilities`-gated dispatch | Anthropic + OpenAI both support system + tools + streaming; capability gating is a no-op until a future protocol (Gemini? Ollama?) diverges. PR3 also adds the model-level `WireCapabilities` struct used at the wire layer for the cross-protocol strip pass. |
| Provider-level API key redaction (so `api_key` is never logged even in `tracing::debug!`) | Deferred. The PR3 `OpenAIProvider::send` info log does not include the key (only `url`, `model`, `tools_count`, `has_system`); the same is true for the PR2 `chat_stream_with_tools`. Explicit redaction is a defensive layer a future PR should add. |

---

## Scenario: OpenAI Chat Completions adapter + cross-protocol WireMessage (PR3 of06-08-multi-model-llm-provider-planning)

###1. Scope / Trigger

- Trigger: PR2 shipped the catalog + `Provider` trait dispatch
 with a real Anthropic adapter and a stub `OpenAI` branch.
 PR3 closes the loop: implement `OpenAIProvider` (Chat
 Completions streaming), introduce a `WireMessage`
 intermediate layer so both providers share a single
 cross-protocol conversion + strip path, and lock the
 cross-protocol degradation rules (parent PRD §Q5 H1
 decision: "switching model silently drops the wire
 blocks the new model can't represent").
- Why code-spec depth: mandatory — the wire layer is the
 single place that knows how to map between
 Anthropic-shaped `ChatRequest` / `ChatEvent` and the
 provider-specific wire payloads. A bug here cascades to
 every future protocol (Gemini, Ollama, …).

###2. Signatures

#### Wire layer (`app/src-tauri/src/llm/provider/wire.rs` — new)

```rust
pub struct WireRequest {
 pub model: String,
 pub max_tokens: Option<u32>,
 pub system: Option<String>,
 pub messages: Vec<WireMessage>,
 pub tools: Vec<WireTool>,
 pub reasoning_effort: Option<String>, // OpenAI o1/o3
}

pub enum WireMessage {
 User { content: String },
 Assistant { blocks: Vec<WireBlock> },
 Tool { tool_call_id: String, content: String },
}

pub enum WireBlock {
 Text { text: String },
 Reasoning { text: String }, // Anthropic thinking / OpenAI reasoning_content
 Signature { data: String }, // Anthropic-only
 RedactedThinking { data: String }, // Anthropic-only
 ToolUse { id: String, name: String, input: serde_json::Value },
}

pub struct WireTool {
 pub name: String,
 pub description: Option<String>,
 pub input_schema: serde_json::Value,
}

pub struct WireCapabilities {
 pub supports_thinking: bool,
 pub supports_reasoning_effort: bool,
 pub supports_thinking_signatures: bool,
}

pub fn chat_request_to_wire(req: ChatRequest, system: Option<String>) -> WireRequest;
pub fn strip_unsupported(messages: Vec<WireMessage>, caps: &WireCapabilities) -> Vec<WireMessage>;
pub fn wire_messages_to_chat_messages(messages: Vec<WireMessage>) -> Vec<ChatMessage>;
```

#### `OpenAIConfig` (`app/src-tauri/src/llm/provider/openai.rs` — new)

```rust
pub struct OpenAIConfig {
 pub base_url: String,
 pub model: String,
 pub api_key: String,
 pub max_tokens: u32,
 pub reasoning_effort: Option<String>, // from ModelRow.thinking_effort
}

pub struct OpenAIProvider { config: OpenAIConfig }
impl OpenAIProvider { pub fn new(config: OpenAIConfig) -> Self; }
impl Provider for OpenAIProvider { ... }
```

###3. Contracts

#### Protocol differences (the only spec table PR3 needs)

| Concern | Anthropic (PR2) | OpenAI (PR3) |
|---------|-----------------|---------------|
| URL | `provider.base_url + "/v1/messages"` | `provider.base_url + "/chat/completions"` (base_url MUST include `/v1`) |
| Auth | `x-api-key: <key>` + `anthropic-version:2023-06-01` | `Authorization: Bearer <key>` |
| System prompt | top-level `system` field | first `role: "system"` message |
| Tools | `[ToolDef]` (Anthropic) | `[{type: "function", function: {name, description, parameters}}]` |
| Tool call | `tool_use` block in `content[]` | `tool_calls[]` array of `{index, id, function: {name, arguments: "<json-string>"}}` |
| Tool result | `role: "user"` + `tool_result` block | independent `role: "tool"` message + `tool_call_id` |
| Text delta | `content_block_delta.text_delta` | `choices[0].delta.content` |
| Reasoning | `thinking_delta` block (Anthropic SSE) | `choices[0].delta.reasoning_content` (OpenAI o1/o3) |
| Finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
| Error body | `{"error": {"type": "<class>", "message": "..."}}` | `{"error": {"code": "<class>", "message": "..."}}` |
| Stream format | `event: ...\ndata: {...}\n\n` (typed) | `data: {...}\n\n` (data-only) |

**`base_url` convention is per-protocol, NOT symmetric:**

| Protocol | Seed base_url (PR1) | What adapter appends | Final URL |
|----------|---------------------|----------------------|-----------|
| Anthropic | `https://api.anthropic.com` (no `/v1`) | `/v1/messages` | `https://api.anthropic.com/v1/messages` |
| OpenAI | `https://api.openai.com/v1` (with `/v1`) | `/chat/completions` | `https://api.openai.com/v1/chat/completions` |

This **mismatch is intentional**: Anthropic historically exposes `https://<host>` as the protocol root and the API lives at `/v1/messages`; OpenAI exposes `https://<host>/v1` as the versioned root and the completion endpoint is `/chat/completions` (the `/v1` is part of the host path, not an endpoint component). The convention matches `test_model` / `test_provider` in `lib.rs` (which both pass `provider.base_url` straight through and append the path component).

**BUG FIX (06-09-fix-session):** prior to this fix, `OpenAIConfig::endpoint()` appended `/v1/chat/completions` (matching the table row above the table's convention was already documented as `+ "/v1/chat/completions"`). Against any realistic `base_url` that already included `/v1` (the seed and every real OpenAI-compatible proxy like `https://api.deepseek.com/v1`), this produced `/v1/v1/chat/completions` and the upstream404'd with `path not found: /v1/v1/chat/completions`. The SSE parser never saw a stream, the agent loop emitted `ChatEvent::Error`, and `streamController.finalizeRequest` evicted the in-memory cache (per the8509bff2013-wire-invariant fix) so the UI landed on the empty state — exactly the "新 session发送消息，闪一下变空" symptom. The fix updates the table + code + spec in lockstep.

#### `strip_unsupported` decision matrix

| `WireBlock` variant | `supports_thinking` | `supports_reasoning_effort` | `supports_thinking_signatures` | Outcome |
|---------------------|---------------------|-----------------------------|----------------------------------|---------|
| `Text` | * | * | * | keep |
| `ToolUse` | * | * | * | keep |
| `Reasoning` | true | * | * | keep → Anthropic thinking block |
| `Reasoning` | false | true | * | keep → OpenAI `reasoning_content` stream |
| `Reasoning` | false | false | * | **drop** |
| `Signature` | * | * | true | keep |
| `Signature` | * | * | false | **drop** |
| `RedactedThinking` | * | * | true | keep |
| `RedactedThinking` | * | * | false | **drop** |

`User` and `Tool` messages are passed through unchanged.

#### `OpenAIProvider::send` flow

```text
ChatRequest --(chat_request_to_wire)--> WireRequest
 |
 v
 (strip_unsupported, openai caps)
 |
 v
 WireRequest
 |
 v
 (build_http_body: openai-shape)
 |
 v
 POST {base}/chat/completions
 |
 v
 SSE stream (data-only)
 |
 v
 choices[0].delta.{content, reasoning_content, tool_calls}
 choices[0].finish_reason + data: [DONE]
 |
 v
 Stream<ChatEvent>
```

#### `AnthropicProvider::send` (PR3 cross-protocol symmetry)

The Anthropic adapter also runs the request through the wire
layer (decision D1 — symmetry). The flow:

```text
ChatRequest --(chat_request_to_wire)--> WireRequest
 |
 v
 (strip_unsupported, anthropic caps — no-op)
 |
 v
 WireRequest
 |
 v
 (wire_messages_to_chat_messages) -> ChatRequest
 |
 v
 chat_stream_with_tools(req) // unchanged SSE parser
```

`strip_unsupported` is a no-op when caps say "support
everything" (Anthropic target on Anthropic source). The
inverse function reconstitutes the Anthropic-shaped
`ChatRequest` the legacy SSE parser understands, so the
rest of the call chain is byte-for-byte the same as
pre-PR3.

#### Error classification (PR3 extension)

`classify_error_response` in `error.rs` now reads BOTH
`error.type` (Anthropic / GLM convention) and `error.code`
(OpenAI convention). The keyword-match logic picks the
field that contains a classification keyword
(`authentication` / `new_api_error` / `invalid_api_key` for
Auth, `rate_limit` for RateLimit, `invalid_request` for
InvalidRequest). If neither field carries a useful
keyword, the function falls back to the HTTP status
(5xx → Server,4xx → InvalidRequest). Net effect: same5
`LlmError` categories, both protocols.

#### DB / persistence (PR3 doesn't change)

- `providers.protocol = "openai"` is now a real dispatch
 path; the existing catalog + seed code (PR1) is
 unchanged.
- `ModelRow.thinking_effort` is dual-purpose: it
 configures `Anthropic.adaptive.effort` (PR2) and the
 top-level `reasoning_effort` field on OpenAI requests
 (PR3). `None` means "do not emit" on either side.
- The wire strip is **in-memory only**. The DB stores the
 full assistant turn (text + thinking + signature +
 redacted_thinking) regardless of which model the user
 switches to. Strip only affects what goes on the wire
 this turn.

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `OpenAIProvider::send` on an empty `messages` | Body has `messages: []` (valid) |
| `messages` contains a `Thinking` block, target is OpenAI | Strip keeps as `[reasoning] <text>` in content (cross-protocol history marker) |
| `messages` contains a `Signature` block, target is OpenAI | Strip drops the block entirely |
| `messages` contains a `RedactedThinking` block, target is OpenAI | Strip drops the block |
| OpenAI401 with `error.code = "invalid_api_key"` | `LlmError::Auth` |
| OpenAI429 with `error.code = "rate_limit_exceeded"` | `LlmError::RateLimit` |
| OpenAI400 with `error.code = "invalid_request_error"` | `LlmError::InvalidRequest` |
| OpenAI5xx | `LlmError::Server` |
| `delta.tool_calls[i]` with no `function.name` (defensive) | Buffer accumulates; emit-time check returns `None` and skips the broken tool call with a `tracing::warn!` |
| `delta.tool_calls[i].function.arguments` is partial JSON | Buffer accumulates; emit-time parse falls back to `{}` on parse error |
| `data: [DONE]` arrives with in-flight tool_call buffers | Defensive: emit any unfinished tool calls before terminating the stream |
| `AnthropicProvider::send` with `signature` block in history | Strip keeps it; the inverse `wire_messages_to_chat_messages` reconstitutes the `Thinking` block with the signature intact (no Anthropic round-trip regression) |

###5. Good / Base / Bad Cases

#### Good: OpenAI gpt-4o happy path

1. User opens Settings, adds an OpenAI provider with their
 `sk-...` key, adds a `gpt-4o` model under it, sets it as
 the default.
2. User opens a session, types a question, clicks Send.
3. `resolve_chat_provider` resolves the gpt-4o
 `ModelWithProvider` and the OpenAI provider row.
4. `build_provider` constructs an `OpenAIProvider` with
 `max_tokens =16384`, `reasoning_effort = None`.
5. `OpenAIProvider::send` runs `chat_request_to_wire` →
 `strip_unsupported` (drops any prior thinking /
 signature blocks silently) → `build_http_body` →
 `POST https://api.openai.com/v1/chat/completions` with
 `Authorization: Bearer ...`.
6. SSE stream: text deltas arrive as `ChatEvent::Delta`,
 tool_calls arrive fully-assembled as `ChatEvent::ToolCall`,
 finish_reason arrives as `ChatEvent::Done { stop_reason:
 "end_turn" }` (normalized from OpenAI's `"stop"`).
7. The chat command's agent loop continues identically to
 the Anthropic path — same `ChatEvent` stream, same
 persistence, same tool envelope.

#### Good: switch from Claude to gpt-4o mid-session

1. User has an active session on `claude-sonnet-4-5`; the
 assistant's last turn emitted a `thinking` block with a
 signature.
2. User opens Settings, sets `gpt-4o` as the default model.
3. User sends a follow-up message in the same session.
4. `resolve_chat_provider` returns the OpenAI provider.
5. The history that goes on the wire includes the
 `Thinking` block from the prior Anthropic turn. The
 `OpenAIProvider::send` runs `strip_unsupported` with
 OpenAI caps (`supports_thinking = false`,
 `supports_reasoning_effort = true` since the gpt-4o
 model row has no `thinking_effort` set → `false` in
 practice): the `Signature` block is dropped
 (opaque — not mappable), the `Reasoning` block is
 dropped (no reasoning target). The `Text` block is
 kept.
6. The DB still has the full Thinking + Signature
 blocks; only the wire payload is degraded.

#### Base: OpenAI401 with new-style error body

1. User's OpenAI key is invalid; first request returns:
```json
{ "error": { "code": "invalid_api_key", "message": "Incorrect API key provided", "type": "error" } }
```
2. `classify_error_response(401, body)` reads `error.type`
 first (literal `"error"`) — no keyword match. Then
 reads `error.code` (`"invalid_api_key"`) — matches
 `invalid_api_key` → `LlmError::Auth`.
3. The chat command emits
 `ChatEvent::Error { message: "API key 无效或已过期...",
 category: Auth }`.

#### Bad: stripping on the wrong side

1. (Pre-PR3 v1 implementation) `strip_unsupported` lived
 inside `OpenAIProvider::send` and ran on
 `Vec<ContentBlock>` (Anthropic-shaped) instead of
 `Vec<WireBlock>`.
2. Switching from Anthropic to OpenAI would attempt to
 strip `ContentBlock::Signature` directly — but the
 function signature expected `WireBlock::Signature`,
 producing a confusing type error on the first attempt
 to compile the adapter.
3. Fix: `strip_unsupported` lives in the wire module and
 takes `Vec<WireMessage>` + `&WireCapabilities`. Both
 providers call it. The Anthropic provider's call is
 observably a no-op for Anthropic→Anthropic, but the
 code path is the same as OpenAI's.

#### Bad: persistent strip on the wrong default

1. (Anti-pattern, NOT the implementation) Strip once on
 model switch, persist the stripped form to the DB.
2. User switches back to a thinking-capable model: the
 thinking blocks are GONE from history; the LLM has no
 memory of its prior reasoning.
3. Fix (PR3 doesn't do this): strip is in-memory only;
 the DB stores the full turn. The DB shape is
 independent of the active default model.

###6. Tests Required

#### Wire layer (`wire::tests`)

| Test | Asserts |
|------|---------|
| `caps_anthropic_with_thinking_signatures_supported` | All3 caps true for Anthropic + thinking-effort-set model |
| `caps_openai_drops_signatures_even_with_effort` | OpenAI: `supports_thinking_signatures` is false |
| `caps_no_effort_disables_reasoning_effort` | `reasoning_effort = None` → `supports_reasoning_effort = false` |
| `chat_request_to_wire_preserves_system_and_tools` | System + tools come through unchanged |
| `chat_request_to_wire_lifts_tool_results_out_of_user_message` | A `role: "user"` with N `tool_result` blocks + interleaved text fans out to N+1 wire messages in order |
| `chat_request_to_wire_thinking_block_splits_reasoning_and_signature` | Anthropic `Thinking { thinking, signature }` → `Reasoning { text }` (signature split out for independent strip) |
| `strip_drops_signature_when_target_cant_carry_it` | OpenAI target: Signature dropped, Reasoning kept if `reasoning_effort = true` |
| `strip_drops_reasoning_when_target_has_no_thinking_or_reasoning` | gpt-4o (no thinking, no reasoning effort) → Reasoning dropped |
| `strip_keeps_tool_use_and_text_always` | Worst-case caps: ToolUse + Text survive |
| `strip_drops_redacted_thinking_on_cross_protocol` | OpenAI target: RedactedThinking dropped |
| `strip_preserves_user_and_tool_messages_unchanged` | User / Tool messages flow through unchanged |
| `strip_keeps_signature_for_anthropic_target` | Anthropic→Anthropic: signature survives strip |
| `wire_block_text_to_chat_event_delta` | `Text` → `ChatEvent::Delta` |
| `wire_block_reasoning_to_chat_event_thinking_delta` | `Reasoning` → `ChatEvent::ThinkingDelta` |
| `wire_block_tool_use_to_chat_event_tool_call` | `ToolUse` → `ChatEvent::ToolCall { id, name, input }` |
| `wire_block_redacted_thinking_to_chat_event_redacted_delta` | `RedactedThinking` → `ChatEvent::RedactedThinkingDelta` |

#### OpenAI adapter (`openai::tests`)

| Test | Asserts |
|------|---------|
| `endpoint_trims_trailing_slash` | `base_url = "https://x.com/v1/"` → endpoint has no double slash |
| `endpoint_uses_provided_base_url` | Custom proxy base URL works |
| `endpoint_does_not_double_prefix_v1_when_base_url_includes_v1` | `base_url = "https://api.openai.com/v1"` and `base_url = "https://api.deepseek.com/v1"` both produce `<base>/chat/completions` (no `/v1/v1/`) |
| `openai_provider_reports_openai_capabilities_and_protocol` | `protocol() == Openai`, all3 caps true |
| `openai_provider_is_send_sync` | `Send + Sync` (compile-time) |
| `build_http_body_system_prompt_becomes_first_message` | `system: Some(s)` → first `role: "system"` message |
| `build_http_body_no_system_prompt_omits_system_message` | `system: None` → no system message |
| `build_http_body_tools_wrapped_in_function_envelope` | `WireTool` → `[{type: "function", function: {…}}]` |
| `build_http_body_tool_results_become_role_tool_messages` | `WireMessage::Tool` → `role: "tool"` with `tool_call_id` + `content` |
| `build_http_body_assistant_message_carries_text_and_tool_calls` | `WireMessage::Assistant` → `{role: "assistant", content, tool_calls[]}` |
| `build_http_body_omits_tools_field_when_empty` | No `tools: []` (absent) |
| `build_http_body_sets_model_and_max_tokens_from_config` | `model` + `max_tokens` come from `OpenAIConfig` |
| `openai_strip_drops_thinking_signature_from_anthropic_history` | Cross-protocol strip integration with wire layer |
| `openai_401_classified_as_auth` | OpenAI `error.code = "invalid_api_key"` → `LlmError::Auth` |
| `openai_429_classified_as_rate_limit` | OpenAI `error.code = "rate_limit_exceeded"` → `LlmError::RateLimit` |
| `openai_400_with_invalid_request_code_is_invalid` | OpenAI `error.code = "invalid_request_error"` → `LlmError::InvalidRequest` |
| `openai_500_classified_as_server` | OpenAI5xx → `LlmError::Server` |
| `build_tool_call_event_parses_accumulated_arguments_json` | Tool-call buffer with complete JSON → `ChatEvent::ToolCall` with parsed `input` |
| `build_tool_call_event_handles_partial_arguments` | Concatenated fragments → valid JSON parsed at emit time |
| `build_tool_call_event_returns_none_without_name` | Defensive: missing `function.name` → drop |
| `build_tool_call_event_empty_args_buf_yields_empty_object` | Defensive: no arguments → `{}` |
| `wire_block_to_chat_event_text_path` | (sanity, same as wire test) |
| `wire_block_to_chat_event_reasoning_path` | (sanity, same as wire test) |

#### Factory / dispatch (`provider::tests` —1 changed,1 new)

- `build_provider_openai_returns_openai_provider` (CHANGED from
 `build_provider_openai_returns_not_implemented`): PR2's
 stub is now a real dispatch. Same test name with new
 assertion shape.

PR3 net new tests:16 wire +14 OpenAI =**30+ new tests**.
Pre-PR3 baseline:218. Post-PR3 target:**248+ tests** (the
implementation may land a few more in the
`AnthropicProvider::send` integration path).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. PR3 does NOT
 change the frontend; `get_llm_config` IPC's wire shape
 (`{model, baseUrl, configured}`) is preserved. The
 catalog's existing display still works (no
 protocol-aware UI yet — that's PR4).
- Manual smoke test (acceptance A2 from the parent PRD):
1. `cd app && pnpm tauri dev`
2. Open Settings, see the2 seeded providers +4
 seeded models (2 Anthropic,2 OpenAI per the PR1
 seed). Default is `claude-sonnet-4-5`.
3. Open a session, type a question, click Send.
4. Observe the LLM responds — same wire behavior as
 pre-PR3 (Anthropic path).
5. In Settings, switch the default to `gpt-4o` (or
 any OpenAI model with a valid key).
6. Open a NEW session (the previous session's history
 is still in the DB; new sessions auto-pick the new
 default). Type a question, click Send.
7. Observe the LLM responds via OpenAI Chat
 Completions. The cross-protocol strip in
 `OpenAIProvider::send` silently drops any
 `Signature` blocks from the prior Anthropic turns
 — the wire payload omits them; the DB still has
 them.

###7. Wrong vs Correct

#### Wrong: openai branch of `build_provider` returns a stub

```rust
// BAD — pre-PR3 stub
"openai" => Err(ProviderBuildError::NotImplemented("openai")),
```

User picks a gpt-4o model in Settings; chat command's
pre-flight returns `PreFlightError::BuildFailed(NotImplemented)`
which renders as "无法构造 LLM provider: provider protocol
'openai' is not implemented yet". The user has no way to
actually use the model they configured.

#### Correct: openai branch constructs an `OpenAIProvider`

```rust
// GOOD — PR3 dispatch
"openai" => {
 let max_tokens = model_row.max_tokens.unwrap_or(16384);
 let reasoning_effort = model_row.thinking_effort.clone();
 let config = openai::OpenAIConfig {
 base_url: provider_row.base_url.clone(),
 model: model_row.model_name.clone(),
 api_key: provider_row.api_key.clone(),
 max_tokens,
 reasoning_effort,
 };
 Ok(Box::new(OpenAIProvider::new(config)))
}
```

The `reasoning_effort` value is plumbed from
`ModelRow.thinking_effort` so o1/o3 users get the
correct effort level on every request.

#### Wrong: strip on the wrong layer

```rust
// BAD — strip on Anthropic-shaped ContentBlock
fn strip_for_openai(blocks: &mut Vec<ContentBlock>) {
 blocks.retain(|b| !matches!(b, ContentBlock::Thinking { .. }));
 // ...
}
```

This couples the strip logic to the Anthropic-shaped
types. Future protocols (Gemini, Ollama) would each need
their own strip function. Cross-protocol history
(Signature / RedactedThinking) cannot be expressed.

#### Correct: strip on the wire layer

```rust
// GOOD — provider-agnostic
fn strip_unsupported(messages: Vec<WireMessage>, caps: &WireCapabilities) -> Vec<WireMessage> {
 messages.into_iter()
 .filter_map(|m| match m {
 WireMessage::User { content } => Some(WireMessage::User { content }),
 // ... etc
 WireMessage::Assistant { blocks } => {
 let filtered: Vec<WireBlock> = blocks.into_iter()
 .filter(|b| block_supported(b, caps))
 .collect();
 Some(WireMessage::Assistant { blocks: filtered })
 }
 })
 .collect()
}
```

Single function. Driven by `WireCapabilities`. Both
providers call it. Future protocols plug in by writing
their own provider-wire converter and the strip pass
auto-adapts.

#### Wrong: `classify_error_response` only matches `error.type`

```rust
// BAD — pre-PR3 Anthropic-only
if keyword.contains("authentication") || keyword.contains("new_api_error") {
 Auth
} else if keyword.contains("rate_limit") {
 RateLimit
} // ...
```

OpenAI401 returns
`{"error": {"code": "invalid_api_key", "message": "..."}}`.
`error.type` is a literal `"error"`. The keyword match
fails. Status is401 (4xx) → falls through to
`InvalidRequest`. The user sees "请求无效: Incorrect API
key provided" instead of "API key 无效或已过期...".

#### Correct: read both `error.type` and `error.code`

```rust
// GOOD — PR3 extended classifier
let err_type = parsed.error.as_ref().and_then(|e| e.r#type.clone());
let err_code = parsed.error.as_ref().and_then(|e| e.code.clone());
// Pick the field that contains a classification keyword
let mut chosen: Option<String> = None;
for cand in [&err_type, &err_code, &top_type] {
 let s = keyword_in(cand);
 if has_keyword(&s) {
 chosen = Some(s);
 break;
 }
}
```

The first field with a useful keyword wins. If neither
field has one, the function falls back to status code.
Pre-PR3 Anthropic / GLM tests still pass (they use
`error.type` and the keyword match still finds it).

#### Wrong: persist the strip result

```rust
// BAD — strip + persist + lose the original
let stripped = strip_unsupported(messages, &caps);
db::update_messages(session_id, &stripped).await?; // ❌
```

The DB now lacks the thinking / signature blocks.
Switching back to a thinking-capable model: the LLM has
no memory of its prior reasoning. Recovery requires
re-running the model on the original turn (lossy and
expensive).

#### Correct: strip in-memory only

```rust
// GOOD — wire-only strip
let wire = chat_request_to_wire(req, system);
let wire = WireRequest {
 messages: strip_unsupported(wire.messages, &caps),
 ..wire
};
// DB write path uses the original `req.messages` —
// untouched by strip.
```

The DB is the source of truth for conversation history.
The wire payload is a per-turn projection of the DB
state onto the target protocol's capabilities. Switch
back, the projection includes the blocks again.

### Design Decisions

#### Decision: Anthropic also goes through the wire layer (D1, locked2026-06-09)

**Context**: The PR3 PRD considered two architectures:
(a) Anthropic stays on its pre-PR3 code path
(verbatim-move to a new `AnthropicProvider`), OpenAI
plugs in via a separate `chat_openai_stream_with_tools`
function — minimal disruption to PR2's tests. (b) Both
providers go through a shared wire layer — symmetric
architecture, but Anthropic's `chat_stream_with_tools`
has to be refactored to consume a `ChatRequest` (the
wire layer's inverse output) rather than the legacy
`system + messages + tools` parameters.

**Decision**: (b). The wire layer's inverse
(`wire_messages_to_chat_messages`) reconstructs the
Anthropic-shaped `ChatRequest` the legacy SSE parser
consumes, so the rest of `chat_stream_with_tools` is
unchanged. The cost is one extra in-memory conversion
per `send` call. The benefit is architectural symmetry
— future protocols (Gemini, Ollama) plug in with no
refactor to the existing providers.

**Consequences**:
- ✅ Cross-protocol consistency: both providers go
 through the same `chat_request_to_wire → strip →
 provider-wire-converter` flow.
- ✅ The strip pass is exercised on the Anthropic
 path too, catching bugs early.
- ⚠️ Slight code overhead: the Anthropic `send`
 method is ~20 lines longer.
- ⚠️ The4 PR2 `anthropic::tests::*` tests still pass
 verbatim because the wire round-trip preserves the
 same field set on the Anthropic→Anthropic path.

#### Decision: In-memory strip, no persistence (D2, locked2026-06-09)

**Context**: The parent PRD §Q5 H1 decided on "silent
degradation" when switching models. PR3 implements this
by stripping blocks the new model can't carry from the
wire payload. The question is whether the strip result
is persisted.

**Decision**: Strip in-memory only. The DB stores the
full turn (text + thinking + signature + redacted).
The wire payload is a per-turn projection.

**Consequences**:
- ✅ Switching back to a thinking-capable model
 restores the thinking display.
- ✅ No DB schema change.
- ✅ No migration risk on existing sessions.
- ⚠️ A cross-protocol session always has a "stripped"
 view on the wire for non-thinking targets — the
 model's prior reasoning is invisible to it. This is
 the documented trade-off (parent PRD §Q5).

### Future Work (Deferred from PR3)

| Item | Why deferred |
|------|-------------|
| `ProviderCapabilities`-gated dispatch | All3 protocols (Anthropic, OpenAI, future Gemini) support system + tools + streaming. Capability gating is a no-op until a protocol diverges. |
| Provider-level API key redaction (so `api_key` is never logged even in `tracing::debug!`) | The `info!` logs in `chat_stream_with_tools` and `OpenAIProvider::send` don't include the key today; explicit redaction is a defensive layer PR4+ should add. |
| OpenAI `max_completion_tokens` (o1+ specific) | The OpenAI API uses `max_tokens` for non-o1 models and `max_completion_tokens` for o1+. PR3 uses `max_tokens`; future PR may branch on `model_name` to pick the right field. |
| OpenAI `parallel_tool_calls: true` | PR3 emits multiple `tool_calls` from a single assistant turn (matches the OpenAI streaming semantics), but the request body doesn't set `parallel_tool_calls: true` explicitly. The default is `true` on most models, so this is a no-op today. |
| Gemini / Ollama adapters | Each will plug in via the wire layer — write a new `provider::gemini` module, implement `Provider`, register in `build_provider`. The wire layer is the cross-protocol contract that doesn't need to change. |
