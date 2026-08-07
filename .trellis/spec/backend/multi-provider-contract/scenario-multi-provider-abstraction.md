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
 // RULE-D-001 (2026-06-24): 解密后的明文, 内部消费 (build_provider / catalog).
 // `#[serde(skip)]` 切断 IPC 明文回传 —— 前端只见 hasKey.
 #[serde(skip)]
 pub api_key: String,
 pub has_key: bool, // api_key_enc 非空, 与能否解密无关
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
 api_key TEXT NOT NULL DEFAULT '', -- RULE-D-001: 旧明文列, 迁移后置空保留 (SQLite < 3.35 无 DROP COLUMN)
 api_key_enc TEXT NOT NULL DEFAULT '', -- RULE-D-001: base64(VERSION||nonce||ct||tag), AES-256-GCM + HKDF(machine-id), AAD = provider id
 key_migrated_at TEXT, -- RULE-D-001: 迁移哨兵 (RFC3339), NULL = 未迁移
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
| `list_providers` | — | `Vec<ProviderRow>` (RULE-D-001: 不含明文 apiKey, 只 hasKey) |
| `add_provider` | `protocol, display_name, base_url, api_key` (明文入参, 后端加密存 api_key_enc) | `ProviderRow` |
| `update_provider` | `id, protocol, display_name, base_url, api_key: Option<String>` (None=保持原 key 留空覆盖, Some=加密覆盖) | `Option<ProviderRow>` |
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
 "baseUrl": "https://api.anthropic.com", "hasKey": true, // RULE-D-001: 不回传明文 apiKey (字段 serde(skip))
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

#### Env-keys (已移除 env 兜底路径)

项目**不读任何 LLM 相关 env 变量**。`ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL` / `LLM_MODEL` / `LLM_MAX_TOKENS` / `LLM_THINKING_EFFORT` 这些历史上的 env 变量已随 `LlmConfig::from_env` 的删除而移除(2026-07)。provider/model/api_key/base_url/max_tokens/thinking_effort 的唯一来源是 DB catalog。

早期 PR1/PR2 阶段曾保留 env 作为冷启动兜底(`state.config`),catalog 优先;在 catalog 架构稳定、chat 与 `get_llm_config` 都走 catalog 后,env 路径成为死代码,遂整体删除(`AppState.config` 字段、`LlmConfig::from_env` / `unconfigured` / `is_unconfigured`、相关测试与日志)。

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
