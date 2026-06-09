# Test Model Contract — Per-Model Connectivity Probe

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) (本文) — `test_model` IPC
>
> **何时读本文**:涉及 `test_model` IPC / ModelsTab "测试"按钮 / per-model round-trip探测时。

---

## Scenario: `test_model` IPC — Per-Model Connectivity Probe (PR5 follow-up,2026-06-09)

> User-perceived "Test" flow for the multi-model catalog. Replaces
> the old "Test" button in `ProvidersTab` (which tested provider
> protocol reachability via `test_provider`). The new flow tests
> **a specific catalog model end-to-end** — the user wants to
> know "will this model name work when I actually chat with it?".

###1. Scope / Trigger

Trigger any of:
- Adding / changing the `test_model` IPC handler in `app/src-tauri/src/commands/providers.rs`
- Switching the per-protocol minimal-message strategy (Anthropic `/v1/messages`, OpenAI `/chat/completions`)
- Restoring or removing the legacy `test_provider` IPC
- Changing the response shape `{ success, latencyMs, error }`

###2. Signatures

```rust
// app/src-tauri/src/commands/providers.rs (added2026-06-09 in PR5 follow-up)
#[tauri::command]
async fn test_model(
 state: State<'_, Arc<AppState>>,
 model_id: String,
) -> Result<serde_json::Value, String>;
```

Registered in `tauri::generate_handler!` alongside `test_provider`.

```rust
// app/src-tauri/src/db/{models,providers}.rs (added2026-06-09)
// Returns the catalog row for a model id. None when id is unknown.
pub async fn get_model(
 db: &SqlitePool,
 id: &str,
) -> Result<Option<ModelRow>, sqlx::Error>;

// Returns the catalog row for a provider id. None when id is unknown.
pub async fn get_provider(
 db: &SqlitePool,
 id: &str,
) -> Result<Option<ProviderRow>, sqlx::Error>;
```

`test_provider` (the old IPC) is preserved in `commands/providers.rs` with
`#[allow(dead_code)]` and a "DEPRECATED — use `test_model`" doc
comment, for future catalog-resolution use. It is NOT registered
in `tauri::generate_handler!`'s new entries.

###3. Contracts

**Request to `test_model`**:
- `model_id: String` — the `models.id` (UUID-ish primary key) from
 the catalog. NOT `model_name` (the API-facing name).

**Response from `test_model`** — same shape as `test_provider`:

```json
{
 "success": boolean,
 "latencyMs": number, // u64 milliseconds
 "error": string | null // null when success=true; non-null when success=false
}
```

All four `failure` paths (model not found, provider missing,
HTTP client build, request error, HTTP non-2xx) return the same
shape with `success: false`. The handler NEVER propagates a Rust
`Err` to the IPC layer for catalog / network issues — it always
returns `Ok(json!(...))` so the frontend can render a uniform
result row.

**Anthropic branch** (protocol = `"anthropic"`):
- URL: `POST {provider.base_url}/v1/messages`
- Headers: `x-api-key: {provider.api_key}`,
 `anthropic-version:2023-06-01`, `content-type: application/json`
- Body:
```json
{
 "model": "<model.model_name from catalog>",
 "max_tokens":1,
 "messages": [{"role": "user", "content": "hi"}]
}
```
- Success: HTTP2xx. Failure: HTTP non-2xx → first200 chars of
 response body included in `error`.

**OpenAI branch** (protocol = `"openai"`):
- URL: `POST {provider.base_url}/chat/completions`
- Headers: `authorization: Bearer {provider.api_key}`,
 `content-type: application/json`
- Body:
```json
{
 "model": "<model.model_name from catalog>",
 "max_tokens":1,
 "messages": [{"role": "user", "content": "hi"}]
}
```
- Success: HTTP2xx. Failure: HTTP non-2xx → first200 chars of
 response body in `error`.

**Both branches**:
- `body.model` MUST be the real `model.model_name` from the catalog
 (e.g. `GLM-4.7` for a GLM proxy, or `gpt-4o-2024-08-06` for
 OpenAI). It MUST NOT be hardcoded.
- `latencyMs` is measured from before the protocol branch to the
 end of the branch, including JSON serialization.

###4. Validation & Error Matrix

| Condition | `test_model` response | Notes |
|---|---|---|
| `model_id` not in `models` table | `success: false, error: "model '<id>' not found"` | `latencyMs:0` |
| Catalog read error (db down) | `success: false, error: "failed to load model: <e>"` | `latencyMs:0` |
| `model.provider_id` not in `providers` table | `success: false, error: "provider for model '<display_name>' is missing"` | `latencyMs:0` |
| `reqwest::Client::build()` fails (TLS init, etc.) | `Err("failed to build HTTP client: <e>")` from IPC | Rust `Err` is allowed here — unrecoverable startup |
| HTTP request fails (DNS, connection refused, timeout) | `success: false, error: "request failed: <e>"` | `latencyMs` still measured |
| Anthropic, HTTP non-2xx | `success: false, error: "HTTP <code>: <body[:200]>"` | |
| Anthropic, HTTP2xx | `success: true, error: null` | |
| OpenAI, HTTP non-2xx (e.g.400 `model_not_found`) | `success: false, error: "HTTP <code>: <body[:200]>"` | The user-visible failure point for a wrong `model_name` |
| OpenAI, HTTP2xx | `success: true, error: null` | |
| `protocol` is neither `anthropic` nor `openai` | `success: false, error: "unsupported protocol: <p>"` | Matches `test_provider` behaviour |
| `provider.api_key` empty | The request is still sent; provider returns401, surfaces as HTTP401 in `error` | Pre-flight check belongs to `chat` command, not `test_model` |

The15-second timeout on `reqwest::Client` (built once per call)
is the only timeout — no per-protocol override. GLM proxies
typically respond in <2s; a15s timeout is generous for the
`max_tokens:1` minimal request.

###5. Good / Base / Bad Cases

**Good** — the canonical2026-06-09 implementation:
```rust
let model = db::get_model(&state.db, &model_id).await
 .map_err(|e| format!("failed to load model: {}", e))?;
// ... resolve provider ...
let body = serde_json::json!({
 "model": model.model_name,
 "max_tokens":1,
 "messages": [{"role": "user", "content": "hi"}]
});
let resp = client.post(&url).json(&body).send().await
 .map_err(|e| format!("request failed: {}", e))?;
// map status to { success, latencyMs, error }
```

**Base** — model found, provider found, network works, but the
provider returns400 because the GLM proxy has a typo in
`model.model_name`. The user sees:
```
success: false
latencyMs:412
error: "HTTP400: {\"error\":{\"code\":\"model_not_found\",..."
```
This is the **intended** failure mode — the user learns the
specific model name is not supported, not just "the protocol
doesn't work".

**Bad** — using `GET /models` for the OpenAI branch (the original
2026-06-09 sub-agent implementation, which the user overrode to
`POST /chat/completions` round-trip):
```rust
// WRONG: GET /models doesn't validate the model name on the wire.
// A200 here only proves the base URL + auth work, not that the
// specific model name is supported. The user is misled.
let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
let resp = client.get(&url).header("authorization", ...).send().await?;
```
This was the default in the initial PR5 implementation. It was
rejected by the user because the per-model test should reveal a
bad `model_name`, not a generic "provider reachable" success.

**Bad** — hardcoding `body.model`:
```rust
// WRONG: this is what `test_provider` (the deprecated IPC) did.
// The whole point of `test_model` is to test the specific
// catalog-configured model name, not a known-good probe model.
let body = json!({
 "model": "claude-sonnet-4-5", // ← catalog says GLM-4.7
 "max_tokens":1,
 "messages": [{"role": "user", "content": "hi"}]
});
```

###6. Tests Required

| Test | What it pins |
|---|---|
| Manual (no automated test) — Anthropic2xx path | User-facing flow: ModelsTab "测试" → row shows green ✓ + latency in ms |
| Manual — Anthropic401 (bad API key) | Row shows red ✗ + error includes "401" |
| Manual — OpenAI200 (real chat-completions with valid model_name) | Row shows green ✓ |
| Manual — OpenAI400 with `code: model_not_found` (typo in catalog model_name) | Row shows red ✗ + error includes `model_not_found` — proves POST round-trip is in effect |
| Manual — Unknown `model_id` (deleted catalog row) | Row shows red ✗ + error includes "not found" |
| Manual — Unknown `model.provider_id` (orphaned model) | Row shows red ✗ + error includes "missing" |

**There are no automated `cargo test` cases for `test_model`** —
it makes real HTTP calls to user-configured LLM endpoints, and
unit tests would need a mock HTTP server. The262-test lib suite
covers the unit-level pieces (`db::get_model`, `db::get_provider`,
the request body construction) implicitly through the
`test_provider` precedent and the JSON serialization in
`serde_json::json!`. Manual smoke test is the contract.

###7. Wrong vs Correct

#### Wrong — `GET /models` for OpenAI

```rust
"openai" => {
 // ❌ The OpenAI GET /models endpoint lists available models;
 // it does NOT validate the configured `model.model_name` on
 // the wire. A200 here is misleading — the real test should
 // be a chat-completions round-trip with the actual model name.
 let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
 let resp = client.get(&url)
 .header("authorization", format!("Bearer {}", provider.api_key))
 .send().await?;
 // ...
}
```

#### Correct — `POST /chat/completions` with real `model.model_name`

```rust
"openai" => {
 // ✅ Round-trip a real chat/completions request with the
 // catalog's model.model_name on the wire. A400 with
 // code: "model_not_found" surfaces the real failure for a
 // typo in the model name — which is the point of the
 // per-model test (vs. the per-provider probe).
 let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));
 let body = serde_json::json!({
 "model": model.model_name,
 "messages": [{"role": "user", "content": "hi"}],
 "max_tokens":1
 });
 let resp = client.post(&url)
 .header("authorization", format!("Bearer {}", provider.api_key))
 .header("content-type", "application/json")
 .json(&body)
 .send().await?;
 // ...
}
```

#### Wrong — hardcoded `body.model`

```rust
// ❌ Catalog says `model.model_name = "GLM-4.7"` but
// body sends "claude-sonnet-4-5". The test passes for
// claude-sonnet-4-5 (which is a valid Anthropic model) but
// tells the user nothing about whether their configured
// GLM-4.7 will work.
let body = json!({ "model": "claude-sonnet-4-5", "max_tokens":1, ... });
```

#### Correct — `body.model = model.model_name`

```rust
// ✅ The catalog row's model_name is what we put on the wire.
// If the user typo'd it, the test surfaces the typo via the
// provider's4xx response.
let body = json!({ "model": model.model_name, "max_tokens":1, ... });
```

### Decision: `test_model` is per-model round-trip, not per-provider probe (2026-06-09)

**Context**: PR4 shipped a `test_provider` IPC that ran a
minimal message with a hardcoded `claude-sonnet-4-5` model
name. The PR4 UI surfaced this as a "Test" button on the
`ProvidersTab` Add/Edit form, with the model field disabled
until the test passed. The user found this confusing: testing
the provider doesn't tell the user whether their actual
configured model will work (the GLM proxy might be set up
correctly for one model name but not another, and OpenAI's
`GET /models` doesn't validate the model name on the wire).

**Decision**: Replace `test_provider`'s UI usage with a new
`test_model(model_id)` IPC. The new IPC:

1. Looks up the catalog model + provider rows by id
 (the credentials come from the provider row).
2. Puts the catalog's `model.model_name` on the wire for
 both Anthropic and OpenAI branches.
3. For OpenAI, uses `POST /chat/completions` (NOT
 `GET /models`) so the test surfaces a real
 model-not-found failure.
4. Returns the same `{ success, latencyMs, error }` shape
 so the frontend ModelsTab row can render a uniform
 result.

The old `test_provider` is preserved (with `#[allow(dead_code)]`
and a deprecation doc) for future catalog-resolution use
(catalog resolution could in principle pre-check the
provider's protocol reachability separately from the
per-model test).

**Consequences**:
- ✅ User sees the real failure mode for a bad model name
 (OpenAI400 `model_not_found`) instead of a confusing
 generic success.
- ✅ Both Anthropic and OpenAI tests follow the same shape
 — the cross-protocol contract is consistent.
- ✅ ModelsTab's "测试" button is the single source of truth
 for "will this model work?" — no need to test the
 provider first, then the model.
- ⚠️ OpenAI tests cost a (tiny) chat-completions API call
 per click. The `max_tokens:1` payload makes this a
 one-token billable request, but it is real. A future
 PR could add a "skip OpenAI round-trip" toggle for users
 who are rate-limited; left OOS for now.
- ⚠️ No automated tests cover the real HTTP path. The
262-test lib suite covers `db::get_model` and
 `db::get_provider` but the wire-level behaviour is
 manual. A mock HTTP server is the natural next step but
 is out of scope for this PR.

### Related

- Provider trait + Anthropic / OpenAI dispatch:
 [Scenario: Multi-Provider Abstraction](./multi-provider-contract.md)
 in `multi-provider-contract.md`.
- `test_provider` (deprecated): the source of the round-trip
 pattern; the anthropic branch of `test_provider` is
 almost identical to the anthropic branch of `test_model`.
 Diff is the `body.model` value and the lookup
 (test_provider takes baseUrl/apiKey/protocol as IPC
 args; test_model looks them up by `model_id`).
- `.trellis/tasks/06-09-06-09-06-08-multi-model-pr5-ux-followup-settingsbar-chatinput-model-popover/prd.md`
 (the PR5 follow-up that introduced this IPC; the
 R2 + D2 sections in that PRD document the user
 decision that led to this scenario).
