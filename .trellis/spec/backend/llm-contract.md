# LLM API Contract

> Anthropic Messages API contract enforced by the Rust agent core, with extended-thinking support.

---

## Overview

The LLM client (`app/src-tauri/src/llm/`) speaks the **Anthropic Messages API** schema
(`/v1/messages`, streaming via SSE) directly — not OpenAI, not a generic OpenAI-compat layer.
The `ChatRequest` / `ContentBlock` / `ChatEvent` types are aligned to the official schema
(serde tag `type` matches the wire string).

Two operator choices that look like "compat layer" are actually configuration knobs:

- **`ANTHROPIC_BASE_URL`** — a proxy or a self-hosted relay that follows the Anthropic
  schema. The current dev setup uses `wukaijin.com`'s Claude-compat endpoint; the
  payload is still Anthropic-shaped, not OpenAI-shaped.
- If at any point we switch to OpenAI-compat, the `reasoning_content` field replaces
  the `thinking` block entirely; that change would happen here, not in the UI.

For compatibility-layer caveats and what to test when the proxy changes, see
`docs/HACKING-llm.md`.

---

## Scenario: Extended Thinking Support (Step 6)

### 1. Scope / Trigger

- Trigger: Added `ContentBlock::Thinking` and `ContentBlock::RedactedThinking`
  to satisfy the cross-layer request/response contract for Anthropic extended thinking.
- Why code-spec depth: mandatory — the request body must include the right `thinking`
  shape, the response must be parsed without losing the `signature` blob, the signature
  must round-trip on subsequent turns or Anthropic returns `400`, and the SSE parser
  emits three new event variants that the frontend must handle in order.

### 2. Signatures

#### Backend types (`app/src-tauri/src/llm/types.rs`)

```rust
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
    pub thinking_effort: String,   // "low" | "medium" | "high" | "xhigh" | "max"
}

pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
    pub thinking: Option<ThinkingConfig>,  // always Some(Adaptive{..}) in practice
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Adaptive { display: String, effort: String },  // display always "summarized"
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Thinking { thinking: String, signature: String },
    RedactedThinking { data: String },
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    Start { request_id: String },
    Delta { text: String },
    ThinkingDelta { text: String },
    SignatureDelta { signature: String },
    RedactedThinkingDelta { data: String },
    ToolCall { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Done { stop_reason: String, usage: serde_json::Value },
    Error { message: String, kind: LlmErrorKind },
}
```

#### Frontend payload (`app/src/stores/chat.ts`)

```typescript
type ContentBlockPayload =
  | { type: "text"; text: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean }
  | { type: "thinking"; thinking: string; signature: string }
  | { type: "redacted_thinking"; data: string };

// ChatMessage.thinkingBlocks is in-memory only; persisted as
// ContentBlock::Thinking { thinking, signature } rows in the DB.
type ThinkingBlockInfo = { thinking: string; signature: string };
```

### 3. Contracts

#### Request (always sent)

```json
{
  "model": "<from env LLM_MODEL>",
  "max_tokens": 16384,
  "system": "<system prompt or omitted>",
  "messages": [ ... ],
  "tools": [ ... ],
  "thinking": { "type": "adaptive", "display": "summarized", "effort": "high" }
}
```

- `thinking` is **always present** in the request body. There is no kill switch.
  - If the upstream model does not support adaptive thinking, the call returns `400`.
    This is an accepted operational risk (see ADR D5 in the task PRD).
- `thinking.display` is **always `"summarized"`** — explicit, not omitted.
  - On Opus 4.7+ the default `display` is `"omitted"`, which suppresses `thinking_delta`
    SSE events and breaks the UI.
- `thinking.effort` is sourced from `LLM_THINKING_EFFORT` (default `"high"`).
  - Valid values: `low` / `medium` / `high` / `xhigh` / `max` (Anthropic schema).
  - Invalid values pass through unchanged; the upstream API will reject them.
- `max_tokens` default is `16384` (was `1024` in step 2; bumped in step 6 because
  thinking tokens count against the same budget as the actual answer).

#### Response (SSE event sequence)

```
content_block_start   { index: 0, content_block: { type: "thinking", thinking: "" } }
content_block_delta   { index: 0, delta: { type: "thinking_delta", thinking: "..." } }
content_block_delta   { index: 0, delta: { type: "signature_delta", signature: "..." } }
content_block_stop    { index: 0 }
content_block_start   { index: 1, content_block: { type: "text", text: "" } }
content_block_delta   { index: 1, delta: { type: "text_delta", text: "..." } }
content_block_stop    { index: 1 }
message_delta         { delta: { stop_reason: "end_turn" } }
message_stop
```

Block types observed in step 6: `text`, `tool_use`, `thinking`, `redacted_thinking`.
Delta types observed: `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`.

#### Environment keys

| Key | Required | Default | Notes |
|-----|----------|---------|-------|
| `ANTHROPIC_API_KEY` (or `ANTHROPIC_AUTH_TOKEN`) | yes | — | The dev setup uses `wukaijin.com` proxy tokens; `ANTHROPIC_AUTH_TOKEN` is the legacy alias. |
| `ANTHROPIC_BASE_URL` | no | `https://api.anthropic.com` | Trailing `/v1/messages` is appended by `LlmConfig::endpoint()`. |
| `LLM_MODEL` | no | `MiniMax-M2.7` | |
| `LLM_MAX_TOKENS` | no | `16384` | Was `1024` before step 6. |
| `LLM_THINKING_EFFORT` | no | `high` | Adaptive thinking effort. |

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `ANTHROPIC_API_KEY` missing at startup | `LlmConfig::unconfigured()` — `api_key: ""`, app still launches so UI shows a helpful error. |
| `LLM_MAX_TOKENS` is not a number | Falls back to default `16384`. |
| `LLM_THINKING_EFFORT` is unrecognized | Sent verbatim; upstream may 400. |
| Upstream rejects `thinking: { type: "adaptive" }` | Anthropic returns 400. Switch base_url or downgrade to manual mode (out of MVP scope). |
| `signature` is lost on round-trip (e.g. dropped during rehydrate) | Anthropic returns 400 on the next turn. **Hard rule: `signature` must round-trip verbatim.** |
| `redacted_thinking.data` is mutated or truncated | Anthropic returns 400 on the next turn. Opaque — store as-is. |
| `thinking` block appears after a `tool_use` block in history | Anthropic rejects the order. The rehydrate path emits thinking blocks FIRST. |
| `content_block_start` for `thinking` arrives with non-empty `thinking`/`signature` fields | Treated as the initial buffer content (defensive — Anthropic today sends empty). |

### 5. Good / Base / Bad Cases

#### Good: streaming + persistence + round-trip

1. Model emits `thinking_delta` × N, then `signature_delta` × 1, then `content_block_stop`.
2. Backend buffers the signature; emits `ThinkingDelta` per `thinking_delta` event;
   emits a single `SignatureDelta` on `content_block_stop`.
3. Agent loop finalizes a `ContentBlock::Thinking { thinking, signature }` at the
   turn boundary; persists to DB; emits to frontend.
4. Frontend rehydrates on next session load; `toPayloadContent` puts the thinking
   block first in the assistant message.
5. Next request to Anthropic carries the full signature; no 400.

#### Base: redacted_thinking

1. Safety filter triggers; Anthropic emits `content_block_start { type: "redacted_thinking", data: "..." }`
   followed immediately by `content_block_stop` (no streaming deltas).
2. Backend buffers `data`; emits a single `RedactedThinkingDelta` on stop.
3. Agent loop finalizes a `ContentBlock::RedactedThinking { data }`.
4. Frontend renders a "🔒 N redacted thinking block(s)" placeholder; the data is
   never displayed.

#### Bad: per-event signature emit

1. (The original step 6 implementation emitted `SignatureDelta` per `signature_delta`
   event instead of buffering until `content_block_stop`.)
2. If Anthropic ever splits the signature across N `signature_delta` events (defensive),
   the frontend opens N empty-text thinking blocks; the DB stores N partial-signature
   blocks; the next turn's history is malformed → Anthropic returns 400.
3. Fix: buffer in `BlockState::Thinking { signature_buf }`; emit once on stop.

#### Bad: thinking block re-emitted as second text block

1. UI's `MessageContent::to_text()` accidentally includes thinking text in the
   denormalized `text` column.
2. Rehydrate reads both the `text` block AND the thinking text into the bubble.
3. The user sees duplicated content; on the next turn the model is confused.

### 6. Tests Required

The step 6 PR added 15 unit tests; the following are mandatory for any future
change to this area.

#### Backend (`cargo test`)

| Test | Asserts |
|------|---------|
| `thinking_block_serializes_to_anthropic_schema` | `ContentBlock::Thinking` → `{"type":"thinking","thinking":"...","signature":"..."}`. |
| `thinking_block_deserializes_from_anthropic_schema` | The reverse round-trip. |
| `redacted_thinking_block_serializes_to_anthropic_schema` | `type: "redacted_thinking"` with only the `data` field. |
| `redacted_thinking_block_deserializes_from_anthropic_schema` | Reverse. |
| `chat_message_round_trip_with_thinking_blocks` | Thinking blocks survive `to_json` / `from_json` losslessly. |
| `chat_message_round_trip_with_redacted_thinking` | Same for redacted. |
| `message_content_to_text_excludes_thinking` | `MessageContent::to_text()` does NOT include thinking text in the denormalized string. |
| `chat_event_thinking_delta_serializes_with_snake_case_kind` | The wire `kind` is `"thinking_delta"`. |
| `chat_event_signature_delta_serializes_with_snake_case_kind` | `"signature_delta"`. |
| `chat_event_redacted_thinking_delta_serializes_with_snake_case_kind` | `"redacted_thinking_delta"`. |
| `chat_request_thinking_omitted_when_none` | `Option<ThinkingConfig>` uses `skip_serializing_if` (or equivalent) when `None`. |
| `chat_request_thinking_adaptive_serializes_correctly` | Output: `{"type":"adaptive","display":"summarized","effort":"high"}`. |
| `default_max_tokens_is_16384_not_1024` | Env-less default is `16384`. |
| `thinking_config_is_adaptive_summarized_with_configured_effort` | `LlmConfig::from_env()` honors `LLM_THINKING_EFFORT` and always sets `display: "summarized"`. |
| `unconfigured_has_empty_thinking_effort` | `LlmConfig::unconfigured()` has `thinking_effort: ""` (does not panic). |

Total backend suite: 57 tests pass as of step 6.

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. The thinking-related types live in
  `app/src/stores/chat.ts`; any field added there must be type-checked end-to-end.
- Manual smoke test (acceptance A9): `cd app && pnpm tauri dev`, observe
  thinking stream + `<details>` collapse + session switch round-trip.

### 7. Wrong vs Correct

#### Wrong: emit `SignatureDelta` per `signature_delta` SSE event

```rust
// BAD — emit immediately on each delta
"signature_delta" => {
    let sig = delta.get("signature").and_then(|s| s.as_str()).unwrap_or("").to_string();
    yield Ok(ChatEvent::SignatureDelta { signature: sig });
    // signature_buf is dead code
}
```

If Anthropic ever splits the signature across events, the frontend opens N
thinking blocks, the DB stores N partial signatures, and the next turn 400s.

#### Correct: buffer and emit on `content_block_stop`

```rust
// GOOD — buffer, then emit once on stop
"signature_delta" => {
    if let BlockState::Thinking { signature_buf, .. } = &mut block_state {
        signature_buf.push_str(delta.get("signature")...);
    }
    // no event emitted here
}
// ...
BlockState::Thinking { signature_buf, .. } if !signature_buf.is_empty() => {
    yield Ok(ChatEvent::SignatureDelta { signature: std::mem::take(signature_buf) });
}
```

The buffered signature is the full assembled blob, ready for the DB and the
next-turn payload.

#### Wrong: thinking block emitted after tool_use in history

```typescript
// BAD — toPayloadContent appends thinking at the tail
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
  return [
    ...m.toolUses.map(...),
    ...m.thinkingBlocks.map(t => ({ type: "thinking", ...t })),
  ];
}
```

Anthropic requires thinking blocks at the head of an assistant message; the
next turn 400s.

#### Correct: thinking blocks first

```typescript
// GOOD — thinking blocks first, then tool_use / text
function toPayloadContent(m: ChatMessage): ContentBlockPayload[] {
  return [
    ...m.thinkingBlocks.map(t => ({ type: "thinking", thinking: t.thinking, signature: t.signature })),
    ...m.text.map(t => ({ type: "text", text: t })),
    ...m.toolUses.map(...),
  ];
}
```

## Scenario: Tool Set Extension (4 new tools + ReadGuard + Bash spillover)

### 1. Scope / Trigger

- Trigger: Step 2 had only `read_file` / `write_file` / `shell` (3 tools). The coding
  agent UX was thin — every edit forced LLM to read whole file then `write_file` the
  whole thing back; every code search forced `shell` + `grep` shell-out wasting tokens.
- Round 1 (2026-06-07) extended the toolset to 7: adds `edit_file` /
  `grep` / `glob` / `list_dir`, plus a `ReadGuard` Tauri State that enforces
  read-before-edit + on-disk freshness before any edit is allowed to land.
- Why code-spec depth: mandatory — `edit_file` errors must be plain-English for LLM
  self-correction; `ReadGuard` is a cross-cutting concern (every edit goes through it);
  the toolset is now visible in the request body's `tools: Vec<ToolDef>` array and the
  schema must stay aligned with Anthropic's tool-use contract.

### 2. Signatures

#### Tool declarations (added to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/edit_file.rs
ToolDef {
    name: "edit_file",
    description: "Replace exact text in a file. old_string must match byte-for-byte
        including whitespace. If not unique, pass replace_all: true or include more
        surrounding context. The file must have been read via read_file in this
        session — ReadGuard enforces read-before-edit and on-disk freshness.",
    input_schema: json!({
        "type": "object",
        "properties": {
            "path":         { "type": "string" },
            "old_string":   { "type": "string" },
            "new_string":   { "type": "string" },
            "replace_all":  { "type": "boolean" }
        },
        "required": ["path", "old_string", "new_string"]
    })
}

// app/src-tauri/src/tools/grep.rs
ToolDef { name: "grep", input_schema: { pattern, path?, glob?, output_mode?,
    case_insensitive?, show_line_numbers?, context?, head_limit? } }
// 3 output_modes map to: "files_with_matches" (rg -l) |
//                       "content"          (rg -n) |
//                       "count"            (rg -c)
// Default: "files_with_matches". .gitignore is honored by default.

// app/src-tauri/src/tools/glob.rs
ToolDef { name: "glob", input_schema: { pattern, path? } }
// Cap 100 results, sorted by mtime desc. .gitignore NOT honored (intentional).

// app/src-tauri/src/tools/list_dir.rs
ToolDef { name: "list_dir", input_schema: { path, show_hidden?, limit? } }
// Non-recursive. Default limit 500. Directories suffixed with "/". Hidden files
// hidden by default; use show_hidden: true to opt in.

// app/src-tauri/src/tools/read_guard.rs
pub struct Fingerprint {
    pub mtime: SystemTime,
    pub size: u64,
    pub content_hash_head: u64,   // xxh64 of first 8 KiB
}

pub struct ReadGuard(Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>);

impl ReadGuard {
    pub async fn record_read(&self, sid: &str, path: &Path);
    pub async fn verify_read(&self, sid: &str, path: &Path) -> Result<(), String>;
    pub async fn verify_fresh(&self, sid: &str, path: &Path) -> Result<(), String>;
    pub async fn invalidate(&self, sid: &str, path: &Path);
    pub async fn clear_session(&self, sid: &str);
}
```

#### Adjacent changes (still in the same task)

- `read_file` output is now prefixed with `cat -n` line numbers (`\t<line_num>\t` per
  line, 1-based). The numbering survives the 50 KiB head+tail truncation.
- `shell` output > 30 KiB is **spilled to disk** at
  `<session_cwd>/.everlasting/outputs/<uuid>.txt` and the tool result returns
  `<path>` + a 1 KiB head+tail preview. The spill directory is **pruned on
  `delete_session`** (best-effort, never blocks the delete).

### 3. Contracts

#### Request

The 4 new tools are appended to `ChatRequest.tools`; nothing else changes in the
request body. The agent does **not** opt in or out per turn — they are always
available.

#### Response (tool_use wire shape)

```json
{
  "type": "tool_use",
  "id": "toolu_<id>",
  "name": "edit_file",
  "input": { "path": "src/foo.rs", "old_string": "foo()", "new_string": "bar()", "replace_all": false }
}
```

After execution, the agent loop builds a `ContentBlock::ToolResult` and appends
it to the conversation in the same `tool_use_id` / `content` / `is_error` shape
as the existing 3 tools.

#### Environment keys

No new env keys. The 4 new tools have no configurable knobs (the cap 100 / 30 KiB
/ 500 limits are hard-coded constants in their respective modules, to be lifted
to env if personalization becomes a need).

#### Tauri State

`AppState.read_guard: ReadGuard` is managed by `app::setup` and held alongside
the existing `AppState.db`. `lib.rs::chat` injects `(state.read_guard,
session_id)` into every `execute_tool` call.

### 4. Validation & Error Matrix

| Condition | Tool | Result |
|-----------|------|--------|
| File not read in this session | `edit_file` | `"You must read_file <path> first."` (is_error: true) |
| File read, but mtime/size changed on disk | `edit_file` | `"File <path> has changed on disk since you last read it. Re-read it first."` |
| `old_string` not found in file | `edit_file` | `"old_string not found in <path>. Closest match (line N): '<...>'."` (0-3 hints, Jaccard-sorted) |
| `old_string` matches N>1 times, `replace_all=false` | `edit_file` | `"old_string appears N times in <path>. Add more context or pass replace_all: true."` (lists all line numbers) |
| `old_string == new_string` | `edit_file` | `"old_string and new_string are identical. No-op edit rejected."` |
| `old_string == ""` | `edit_file` | `"old_string must not be empty."` |
| `replace_all: true` | `edit_file` | All occurrences replaced; tool_result includes the count. |
| `ripgrep` exit 1 (no matches) | `grep` | `"No matches found for pattern <pattern> in <path>."` (is_error: false — not really an error) |
| `ripgrep` exit 2 (rg error) | `grep` | stderr text passed to LLM (is_error: true) |
| `pattern` matches but output > 30 KiB | `shell` | Spilled to `<cwd>/.everlasting/outputs/<uuid>.txt`; result = path + 1 KiB preview |
| `delete_session` for a session whose cwd is gone | — | Cleanup is best-effort, no cascade. User's delete intent wins. |
| `ReadGuard.clear_session` on a missing session | — | Silent no-op (HashMap::remove returns None) |
| `read_file` of binary file | `read_file` | `read_to_string` fails → is_error: true, "Failed to read file" (existing behavior, unchanged) |

### 5. Good / Base / Bad Cases

#### Good: read → edit → done

1. LLM calls `read_file` on `src/foo.rs`. `read_file` records the fingerprint in
   `ReadGuard` keyed by the active `session_id`.
2. LLM calls `edit_file` with `old_string` that matches exactly once.
3. `edit_file::execute` calls `ReadGuard::verify_read` (passes — just read) →
   `verify_fresh` (passes — mtime/size unchanged) → `match` (passes — exact 1
   occurrence, no need to check uniqueness separately when 1) → write to disk.
4. On success, `edit_file::execute` calls `ReadGuard::invalidate(sid, path)` so
   the next edit forces a re-read.
5. Tool result: `"Successfully edited <path>: 1 occurrence replaced."`.

#### Base: 0 matches with hint

1. `old_string` is slightly wrong (extra trailing whitespace, one char off).
2. `edit_file` returns the failure message + the 0-3 most similar lines (Jaccard
   similarity on 4-gram token sets). LLM reads the hint, adjusts `old_string`,
   retries on the next turn.

#### Bad: edit without read

1. LLM is given a fresh session and immediately calls `edit_file` without first
   calling `read_file`.
2. `verify_read` returns `Err`. The error message names the tool (`read_file`) the
   LLM must call first. The agent loop does NOT add this to a "previous error"
   list — the LLM can re-attempt with a corrected approach.
3. If LLM persists in calling `edit_file` without `read_file`, the same error
   message is returned each turn; the agent loop hits `max_turns = 20` and gives
   up. (No silent retry, no state corruption — the file is never touched.)

#### Bad: bash output lost when > 30 KiB

1. (Pre-extension behavior.) `shell` runs `cargo build`, output is 80 KiB of
   warnings. `shell` truncates head+tail to 50 KiB. The middle 30 KiB is
   silently dropped. LLM sees a 50 KiB blob and has to guess what was in the middle.
2. (Post-extension.) `shell` writes the 80 KiB to
   `<cwd>/.everlasting/outputs/<uuid>.txt` and returns `"Output saved to <path>
   (80000 bytes). First 1KB preview:\n<preview>"`. LLM can call `read_file` on
   the path to inspect any range with line numbers.

### 6. Tests Required

The round 1 PR added 77 tool tests (14 + 10 + 8 + 8 + 8 + 14 + 15) + 3 for
`cleanup_outputs_dir` = 80 new tests; the following are mandatory for any future
change touching these areas.

#### Backend (`cargo test`)

| Test | Asserts |
|------|---------|
| `edit_file::edit_happy_path` | Read + edit + write back, success result, fingerprint invalidated. |
| `edit_file::edit_before_read` | No prior read → `verify_read` fails, no file written. |
| `edit_file::edit_after_external_modify` | Read, file changed externally, edit → `verify_fresh` fails. |
| `edit_file::edit_old_string_not_found` | 0 matches → error includes `"Closest match"`. |
| `edit_file::edit_old_string_ambiguous` | 2 matches, `replace_all=false` → error lists both line numbers. |
| `edit_file::edit_no_op` | `old_string == new_string` → rejected. |
| `edit_file::edit_with_replace_all` | 3 matches + `replace_all: true` → 3 occurrences replaced. |
| `read_guard::sessions_are_isolated` | s1's reads do not unlock s2's edits. |
| `read_guard::clear_session` | Removes all entries for a session; missing session is no-op. |
| `grep::output_mode_files_with_matches` | Maps to `rg -l` flag. |
| `grep::output_mode_content` | Maps to `rg -n` flag. |
| `grep::output_mode_count` | Maps to `rg -c` flag. |
| `grep::no_matches` | Exit 1 from rg → "No matches found" message (not is_error). |
| `grep::line_cap_500` | Lines longer than 500 chars are truncated with marker. |
| `glob::cap_100` | More than 100 results → truncation hint in result. |
| `glob::mtime_desc` | Results sorted by mtime descending. |
| `list_dir::basic` | Returns entries alphabetically with `/` suffix on directories. |
| `list_dir::hidden_default_off` | `.git/` is hidden unless `show_hidden: true`. |
| `read_file::cat_n_simple` | Single-line file → `1\tcontent`. |
| `read_file::cat_n_truncation_preserves_line_numbers` | 100 KiB file → first 25 lines + `<truncated>` + last 25 lines, both with correct line numbers. |
| `read_file::cat_n_empty_lines_numbered` | File with blank lines → numbering is continuous (1, 2, 3...) not skipped. |
| `shell::large_output_spills_to_disk` | > 30 KiB → file exists in `<cwd>/.everlasting/outputs/`, tool result contains path. |
| `shell::small_output_unchanged` | < 30 KiB → no spill file, tool result is the raw output. |
| `shell::cleanup_outputs_dir` | After `cleanup_outputs_dir(cwd)`, the spill dir is gone; nested files are also removed; missing dir is no-op. |

Total backend suite: 166+ tests pass as of round 1 (was 103 pre-toolset; +63 net
new from this round, including 3 cleanup helpers).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. Frontend sees the 4 new tool
  descriptions via the existing `chat-event` / `tool-call` protocol; **no
  frontend code changes** are required for the new tools because the existing
  `SessionList` tool card is generic.
- Manual smoke test: `cd app && pnpm tauri dev`, call `read_file` on a 100 KiB
  file, observe `cat -n` line numbers. Call `shell` with a long-output
  command, observe spill path in the result. Call `edit_file` after `read_file`,
  observe success; without `read_file`, observe the plain-English error.

### 7. Wrong vs Correct

#### Wrong: allow edit without read

```rust
// BAD — no guard, just match the string
pub async fn execute(input: &Value, ctx: &ToolContext) -> (String, bool) {
    let validated = assert_within_root(...)?;
    let content = tokio::fs::read_to_string(&validated).await?;
    let new_content = content.replace(&input["old_string"], &input["new_string"]);
    tokio::fs::write(&validated, &new_content).await?;
    (format!("Wrote {}", validated.display()), false)
}
```

LLM edits a file it has never seen — frequently edits the wrong occurrence, or
edits an out-of-date in-memory model of the file (e.g. another process changed
it between LLM's read and edit). 1 in 5 edits silently corrupts the file.

#### Correct: ReadGuard enforces read-before-edit + on-disk freshness

```rust
// GOOD — three mandatory checks
pub async fn execute(
    input: &Value, ctx: &ToolContext,
    guard: &ReadGuard, session_id: &str,
) -> (String, bool) {
    let validated = assert_within_root(...)?;

    // 1. Read-before-edit
    if let Err(e) = guard.verify_read(session_id, &validated).await {
        return (e, true);
    }
    // 2. On-disk freshness
    if let Err(e) = guard.verify_fresh(session_id, &validated).await {
        return (e, true);
    }

    let content = tokio::fs::read_to_string(&validated).await?;
    // 3. Match + uniqueness
    let occurrences = content.matches(&old).count();
    if occurrences == 0 {
        let hint = find_similar_lines(&content, &old);
        return (format!("old_string not found in <{}>. Closest match: {}", path, hint), true);
    }
    if occurrences > 1 && !replace_all {
        return (format!("old_string appears {} times in <{}>. Add context or pass replace_all: true.", occurrences, path), true);
    }

    let new_content = if replace_all {
        content.replace(&old, &new)
    } else {
        content.replacen(&old, &new, 1)
    };
    tokio::fs::write(&validated, &new_content).await?;
    guard.invalidate(session_id, &validated).await;
    (format!("Successfully edited <{}>: {} occurrence(s) replaced.", path, occurrences), false)
}
```

#### Wrong: shell silently truncates > 30 KiB output

```rust
// BAD — head+tail truncation, middle silently dropped
fn truncate(s: String) -> String {
    if s.len() <= 30_000 { return s; }
    let head = &s[..25_000];
    let tail = &s[s.len()-5_000..];
    format!("{}\n<truncated {} bytes>\n{}", head, s.len()-30_000, tail)
}
```

LLM misses errors / warnings in the middle 30 KiB. Long `cargo build` outputs
are exactly this case.

#### Correct: shell spills to disk, LLM can re-read with line numbers

```rust
// GOOD — claude-code style spillover
const DISK_SPILL_THRESHOLD: usize = 30 * 1024;
const PREVIEW_BYTES: usize = 1024;

async fn execute(input: &Value, ctx: &ToolContext) -> (String, bool, ToolContextUpdate) {
    let output = run_command(...).await?;
    if output.len() <= DISK_SPILL_THRESHOLD {
        return (output, false, ToolContextUpdate::default());
    }
    let spill_dir = ctx.cwd.join(".everlasting/outputs");
    tokio::fs::create_dir_all(&spill_dir).await?;
    let spill_path = spill_dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
    tokio::fs::write(&spill_path, &output).await?;
    let preview = head_tail_preview(&output, PREVIEW_BYTES);
    (format!("Output saved to {} ({} bytes). Preview:\n{}", spill_path.display(), output.len(), preview), false, ToolContextUpdate::default())
}
```

LLM gets the path and a preview; if it needs the middle, it calls
`read_file` on the path (now with `cat -n` line numbers, thanks to the adjacent
change in R7) to inspect any range.

---

## Scenario: Worktree State Transparency + LLM Cancel (Step 4 Follow-up, 2026-06-08)

### 1. Scope / Trigger

- Trigger: Decoupling worktree from session lifecycle. After this change the
  user can `attach_worktree` / `detach_worktree` / `delete_worktree` mid-session
  (previously worktree was auto-created at session create and never changed).
  Three risks emerged that need code-spec depth:
  1. **LLM confusion across worktree views** — the same session's tool results
     can come from two different on-disk roots (worktree vs project root) within
     one chat. Without explicit signalling the LLM will see "I read S1" then
     "I read S2" with no reason for the change.
  2. **Stale conversation history** — if the frontend keeps its cached messages
     after a worktree transition, the LLM's next turn won't see the new
     `[worktree event]` system event and will reason on outdated context.
  3. **Destructive path racing an in-flight chat** — `delete_session` /
     `detach_worktree` / `delete_worktree` can run while the agent loop is
     still streaming. Without a cancel hook the in-flight turn's `INSERT`
     fails on a deleted row, or a tool call writes to a worktree that no
     longer exists.
- Why code-spec depth: mandatory — new Tauri command surface, new message
  shape on the wire (envelope), new row kind in the `messages` table
  (system event), and a new cross-layer ordering constraint
  (cancel → destructive → system event → next LLM turn).

### 2. Signatures

#### New Tauri commands (`app/src-tauri/src/lib.rs`)

```rust
#[tauri::command]
async fn attach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn detach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn delete_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String>;
```

Each is registered in `invoke_handler!` and exposed to the frontend.

#### New DB schema (`app/src-tauri/src/db.rs`)

```sql
ALTER TABLE sessions ADD COLUMN worktree_state TEXT NOT NULL DEFAULT 'none';
ALTER TABLE sessions ADD COLUMN last_worktree_path TEXT;
-- One-shot backfill on startup (idempotent, see migration helper):
UPDATE sessions
SET worktree_state = 'active'
WHERE worktree_path IS NOT NULL
  AND worktree_state = 'none';
```

Valid `worktree_state` values (snake_case strings; serialized as
`#[serde(rename_all = "snake_case")]`):

| Value | Meaning | `worktree_path` | `last_worktree_path` |
|-------|---------|-----------------|----------------------|
| `"none"` | Never had a worktree, or worktree was deleted. | `NULL` | `NULL` (never used) or last value preserved |
| `"active"` | A worktree is currently bound to this session. | `Some(<path>)` | `NULL` (or previous value preserved) |
| `"detached"` | Had a worktree, now unbound. Directory may still exist on disk. | `NULL` | `Some(<previous path>)` |

#### New helpers

```rust
// app/src-tauri/src/git/worktree.rs
pub fn check_clean(repo_path: &Path) -> Result<(), GitError>;
//   Uses libgit2 `Repository::open(repo_path)?.status()?` to detect modified
//   tracked files + untracked files. Ignores .gitignore'd files.
//   Rejects when status is non-empty.

// app/src-tauri/src/db.rs
pub async fn set_worktree_state(
    pool: &SqlitePool, session_id: &str, state: WorktreeState,
    last_worktree_path: Option<&str>,
) -> Result<(), sqlx::Error>;

pub async fn insert_system_event(
    pool: &SqlitePool, session_id: &str, text: &str,
) -> Result<(), sqlx::Error>;
//   Inserts a row into `messages` with role='user' and content=text.
//   seq = max(seq)+1 for the session.

// app/src-tauri/src/lib.rs
pub fn tool_result_envelope(content: String, worktree_path: &Path) -> String;
//   Returns: {"result": "<content>", "cwd": "<worktree_path>"}
//   Lives in lib.rs (NOT in the tool modules) so the existing 60+ tool
//   unit tests are unchanged.

pub async fn cancel_inflight_for_session(
    cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
    session_active_request: &Arc<Mutex<HashMap<String, String>>>,
    session_id: &str,
) -> Option<String>;
//   Returns the cancelled request_id, or None if no in-flight request.
```

#### New AppState field

```rust
struct AppState {
    // ... existing fields ...
    session_active_request: Arc<Mutex<HashMap<String, String>>>,
    //   Maps session_id -> currently active request_id.
    //   Inserted by `chat` on spawn; cleared by CancellationGuard on Drop.
    //   Read by the 3 destructive paths to find the request_id to cancel.
}
```

### 3. Contracts

#### Tool result envelope (LLM boundary)

- The tool internals (`app/src-tauri/src/tools/*.rs`) return `String`
  unchanged. None of the existing 60+ tool unit tests are modified.
- At the LLM-facing boundary in `lib.rs::chat`, the `ToolResult` event
  payload AND the `ContentBlock::ToolResult.content` stored in the DB
  are wrapped via `tool_result_envelope(content, ctx.worktree_path)`.
- Wire / DB shape: `{"result": "<original content>", "cwd": "<worktree_path>"}`.
- `cwd` is the canonical on-disk path the tool actually ran against
  (`ctx.worktree_path` if the session has a worktree, else
  `project.path`).
- Frontend display: `extractToolResultDisplay(content)` leniently parses
  the envelope and returns the unwrapped `result` field. Falls back to
  the raw string for pre-follow-up data, non-JSON, missing `result`
  key, or parse errors. The wire format to the LLM is preserved.

#### System event injection

- After successful attach/detach/delete, the backend calls
  `db::insert_system_event` with the text format:
  - attach: `[worktree event] attached: <path> on branch session/<id>`
  - detach: `[worktree event] detached from <path> (changes preserved on branch session/<id>)`
  - delete: `[worktree event] deleted: branch session/<id> and dir <path> removed`
- The row is stored with `role='user'` and a structured `metadata`
  marker (`{kind: "worktree_event", event: "attached" | "detached" | "deleted"}`).
- The LLM's next chat loads messages and sees the event in history.
- Frontend `controller.refresh(sessionId)` evicts the cached messages
  and re-loads from DB so the UI bubble list also shows the event
  (rendered as a regular user-role message with the `[worktree event]`
  prefix). A future PR may add a dedicated info-badge rendering.

#### System prompt (Step 4 follow-up Bug 3)

- The agent loop builds a **session-grounding system prompt** in
  `lib.rs::build_system_prompt(session, project, ctx_root, head_sha)`
  and passes it via `chat_stream_with_tools(config, Some(prompt), ...)`.
  Pre-fix, the request body's `system` field was hard-coded to `None`;
  the LLM honestly answered "no" when asked "does your system prompt
  mention you're in a worktree" because the field was empty. The
  `[worktree event]` user-role messages live in the conversation
  history (see "System event injection" above) and describe
  *transitions*, but the system prompt is what tells the LLM the
  *current* state.
- The prompt is constructed **once per `chat` invocation**, before
  the `for turn in 1..=MAX_TURNS` loop. The worktree state can't
  change between turns of the same agent loop (destructive worktree
  commands have a cancel hook that aborts in-flight chats), so
  rebuilding per-turn would be wasteful.
- The prompt shape (always exactly these lines):

  ```
  You are a coding agent. You have access to tools (read_file, write_file,
  edit_file, shell, grep, glob, list_dir). All file paths in tool inputs
  are relative to the session's working directory.

  Session context:
  - Session ID: <session.id>
  - Project: <project.name> (<project.path>)
  - Working directory: <ctx_root>
  - Worktree: <state phrase>
  - Available tool result envelope: {"result": "<content>", "cwd": "<worktree_path>"}
    — `cwd` tells you which root the tool ran against when worktree transitions
    happen mid-session.
  ```

- `<state phrase>` is one of:
  - `Active`: `ACTIVE on branch 'session/<session.id>' (HEAD <short_sha>)`
  - `Detached`: `DETACHED — was on branch 'session/<session.id>' (HEAD <short_sha>), currently in project root`
  - `None`: `NONE — running in project root`
  - Non-git project (regardless of `worktree_state`): `N/A — non-git project`
- `<short_sha>` = first 7 chars of HEAD commit SHA, looked up via
  `lookup_head_sha(ctx_root)`. Best-effort: a non-git path or empty
  repo returns a placeholder (`"not a git repo"` / `"no commits yet"`).
- **Tool result envelope vs system prompt — division of labor**:
  - System prompt is the **persistent declaration** of the session's
    grounding (built once per chat invocation, repeated each request).
  - Tool result envelope's `cwd` field (see "Tool result envelope"
    above) is the **runtime data point** confirming what cwd a
    specific tool actually used. Both are needed: the prompt sets
    the model's mental model; the per-tool `cwd` confirms it after
    each call so the model can detect drift.
- **Privacy / surface area**: only `session.id`, `project.name`,
  `project.path`, `ctx_root`, and the short HEAD SHA are emitted. No
  user messages, tool inputs, or DB rows are echoed into the prompt.
  Future additions (e.g. "the project's main branch is `main`") go in
  `build_system_prompt`; do not scatter prompt-building across the
  agent loop.

#### In-flight cancel hook

- `chat` spawn fills `session_active_request: session_id -> request_id`.
- `CancellationGuard` (Drop on every agent-loop exit path) removes the entry.
- The 3 destructive paths (`delete_session` / `detach_worktree` /
  `delete_worktree`) call `cancel_inflight_for_session` at entry:
  1. Look up the active `request_id` for `session_id`.
  2. If found, fetch the matching `CancellationToken` from
     `cancellations` and `.cancel()`.
  3. Return; the agent loop's `tokio::select!` notices and exits
     cleanly, which clears the map entry via the `CancellationGuard`.
- **Ordering invariant**: cancel → destructive execute → system event
  injection (if any). The system event must land AFTER the cancel
  completes (so the event reflects the post-cancel state) and BEFORE
  the LLM's next turn (so the LLM sees it).
- Frontend guard: `detach` / `delete worktree` menu items are
  `:disabled="chatStore.isStreaming"`. This is a UX guard, not the
  safety net — the backend cancel hook covers the in-flight IPC case.

#### Rehydrate + outbound payload

- The system event is a `role='user'` message, so `toPayloadContent`
  passes it through as a text block. No new block type needed.
- Order: system events appear at the position of their original
  insertion (typically after the latest assistant turn). Anthropic
  accepts interleaved user messages without re-ordering.

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `attach_worktree` on non-git project | Reject: `"project <name> is not a git repository"` |
| `attach_worktree` while project root has uncommitted changes | Reject: `"project root has uncommitted changes; commit or stash before attaching"` |
| `attach_worktree` while worktree_path already exists | Reject: libgit2 error (mirrors PR1 behavior) |
| `detach_worktree` while `worktree_state != 'active'` | Reject: `"no active worktree to detach"` |
| `detach_worktree` while worktree has uncommitted changes | Reject: `"worktree <path> has uncommitted changes; commit/stash before detach"` |
| `delete_worktree` while `worktree_state != 'active'` | Reject: same as detach |
| Destructive path with active in-flight chat | Cancel in-flight first, then proceed |
| `extractToolResultDisplay` receives non-JSON string | Fallback: return raw string |
| `extractToolResultDisplay` receives JSON without `result` key | Fallback: return raw string |
| `insert_system_event` fails (e.g. DB locked) | `tracing::warn!`; the destructive operation has already succeeded — the system event is best-effort |
| `cancel_inflight_for_session` finds no active request | Returns `None`; destructive proceeds |
| `cancel_inflight_for_session` finds request_id but no token (rare race) | `tracing::warn!`; destructive proceeds |
| `lookup_head_sha` on a non-git `ctx_root` | Returns `"not a git repo"`; prompt embeds the placeholder |
| `lookup_head_sha` on an empty (no-commits) repo | Returns `"no commits yet"`; prompt embeds the placeholder |
| `build_system_prompt` for a non-git project | Worktree line is `N/A — non-git project` regardless of `worktree_state` |
| `chat_stream_with_tools` called with `system: None` | Request omits the `system` field (skip_serializing_if=None); backward compat with the pre-fix call sites |

### 5. Good / Base / Bad Cases

#### Good: attach → LLM sees event + envelope

1. User clicks "attach worktree" in the chat panel.
2. Backend `attach_worktree` runs:
   - `cancel_inflight_for_session` (no-op, not streaming).
   - `check_clean(project_root)` passes.
   - `git::worktree::create` builds the worktree.
   - `set_worktree_state(sid, Active, None)` updates the row.
   - `insert_system_event` writes the `[worktree event] attached: ...` row.
   - Returns the new `SessionRow`.
3. Frontend `attachWorktree` action calls `controller.refresh(sid)` to
   evict + reload the cached messages (so the bubble list shows the event).
4. User sends a new message.
5. LLM loads messages, sees the event in history, then calls `read_file`.
6. Backend emits `ToolResult` with envelope `{"result": "...", "cwd": "<worktree_path>"}`.
7. LLM sees `cwd` and knows which root it just operated on.

#### Good: LLM aware of worktree (Step 4 follow-up Bug 3)

1. User has an active worktree on session `abc-123`. User sends:
   "where am I right now? am I in a worktree?".
2. Frontend `send()` posts the message; backend `chat` spawns the
   agent loop. Before the `for turn in 1..=MAX_TURNS`:
   - `lookup_head_sha(worktree_path)` returns `"e3f4567"`.
   - `build_system_prompt(session, project, worktree_path, "e3f4567")`
     produces a prompt that includes:
     ```
     - Working directory: /home/carlos/.local/share/everlasting/worktrees/<pid>/abc-123
     - Worktree: ACTIVE on branch 'session/abc-123' (HEAD e3f4567)
     ```
3. `chat_stream_with_tools(config, Some(prompt), messages, tools)`
   sends the request body with the `system` field populated.
4. The LLM's reply correctly states "you are in a worktree at
   `/home/carlos/.local/share/everlasting/worktrees/.../abc-123`
   on branch `session/abc-123` (HEAD `e3f4567`)" — quoting the
   prompt verbatim if asked.
5. Pre-fix counterpart: with `system: None`, the LLM would answer
   "I don't see any worktree in my system prompt" — which is
   honest but useless. The `[worktree event]` row in history says
   "user told me X happened", which the LLM treats as user speech,
   not authoritative grounding.

#### Base: detach clean

1. `worktree_state = 'active'`, worktree is clean.
2. User clicks "detach worktree".
3. `detach_worktree` runs: `check_clean(worktree_path)` passes, then
   `set_worktree_state(sid, Detached, Some(previous_path))`.
4. `insert_system_event` writes the detach event.
5. Next tool call: `ctx.worktree_path` is now `project.path` (the
   fallback), so the envelope's `cwd` reflects project root.
6. LLM sees the event, knows the worktree is gone, and continues
   operating in project root.

#### Bad: ToolCallCard displays the envelope literally

1. Backend correctly emits envelope `{"result": "hello", "cwd": "/worktree"}`.
2. Frontend `ToolCallCard.vue` does `{{ result.content }}` directly.
3. User sees a JSON blob in the tool card instead of `hello`.
4. Fix: `ToolCallCard.vue` uses `extractToolResultDisplay(result.content)`
   for both `outputSize` and the rendered `<pre>`.

#### Bad: missing controller.refresh

1. `attachWorktree` action mutates the row but does NOT evict the
   cached messages.
2. User clicks send. `controller.startRequest` reads from
   `messagesBySession.get(sid)` (the cache, not the DB).
3. LLM's payload does not include the new system event.
4. LLM reasons on stale context, may try to `cd` into a worktree path
   it doesn't know exists, etc.
5. Fix: `controller.refresh(sid)` at the end of `attachWorktree` /
   `detachWorktree` / `deleteWorktree` actions in `chat.ts`.

#### Bad: missing cancel hook in delete_session

1. User clicks delete session.
2. `lib.rs::delete_session` immediately starts cleanup of
   shell outputs dir + worktree + DB row.
3. LLM is mid-stream; the agent loop is about to `INSERT` a
   `ContentBlock::Text` row referencing the about-to-be-deleted
   session_id.
4. INSERT fails with FK violation; the message is lost.
5. Fix: `cancel_inflight_for_session(sid)` at the top of
   `delete_session` (and the other two destructive paths).

### 6. Tests Required

| Test | Asserts |
|------|---------|
| `tool_result_envelope_round_trip` | Output has exactly 2 keys: `result` + `cwd` |
| `tool_result_envelope_handles_special_chars` | Newline / quote / backslash correctly JSON-escaped |
| `git::worktree::check_clean::rejects_modified` | Modified tracked file → `Err(GitError::Dirty)` |
| `git::worktree::check_clean::rejects_untracked` | Untracked file → `Err` |
| `git::worktree::check_clean::ignores_gitignore` | `.gitignore`d file does not change verdict |
| `db::set_worktree_state::active` | Row updated; `last_worktree_path` cleared or preserved per call |
| `db::set_worktree_state::detached` | Row updated; `last_worktree_path` set |
| `db::insert_system_event::role_user` | New row in `messages`; `role='user'`; content contains `[worktree event]` |
| `attach_worktree::uncommitted_project_root` | Returns `Err` with "uncommitted changes" message |
| `detach_worktree::uncommitted_worktree` | Returns `Err` with same shape |
| `lib::cancel_inflight_for_session::finds_active` | In-flight token cancelled; entry removed after `CancellationGuard` drop |
| `lib::cancel_inflight_for_session::no_active` | Returns `None`; proceeds silently |
| `vitest extractToolResultDisplay::unwraps_envelope` | `{"result":"X","cwd":"Y"}` → `X` |
| `vitest extractToolResultDisplay::falls_back_raw` | Plain string → same string |
| `vitest extractToolResultDisplay::handles_empty` | `""` → `""` |
| `vitest extractToolResultDisplay::handles_non_json` | `"not json"` → `"not json"` |
| `db::migration::backfill_legacy_active` | Pre-follow-up sessions with `worktree_path IS NOT NULL` get `state='active'` |
| `lib::build_system_prompt_active_worktree` | Active state → prompt contains `ACTIVE on branch 'session/<id>'`, HEAD SHA, and worktree path as cwd |
| `lib::build_system_prompt_detached_worktree` | Detached → `DETACHED — was on branch ... currently in project root`; cwd = project root |
| `lib::build_system_prompt_no_worktree` | None → `NONE — running in project root`; no branch / SHA leakage |
| `lib::build_system_prompt_non_git_project` | Non-git project → `Worktree: N/A — non-git project`; no `session/<id>` reference |
| `llm::client::chat_request_system_field_serializes_when_some` | `ChatRequest { system: Some(s), .. }` serializes with the `system` field present at the top level |

Total: ~17 new tests added in this round; backend suite now 180+ tests,
frontend vitest 44+. As of step 4 follow-up: **182 backend tests + 44 frontend
vitest = 226 tests pass**. Bug 3 (system prompt) adds **+5 backend tests**
(`193 total` with the Bug 1/2 self-heal counts not included here — see
the round-2 task PRD for those).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. The 4 worktree actions
  (`attachWorktree` / `detachWorktree` / `deleteWorktree` /
  `controller.refresh`) live in `app/src/stores/chat.ts` and
  `app/src/stores/streamController.ts`; any new field on `SessionSummary`
  must round-trip end-to-end.
- Manual smoke test (acceptance A29):
  1. `cd app && pnpm tauri dev`.
  2. Open a session, attach worktree, observe the chip flips to
     "diff (0) ▼" and the dropdown shows copy + detach + delete.
  3. Send a chat, observe the LLM's next response references the
     worktree path (the `cwd` it sees in the tool result envelope).
  4. Detach, send another chat, observe the response references the
     project root and the bubble list shows the
     `[worktree event] detached from ...` message.
  5. While streaming, click "detach worktree" — observe the disabled
     state, and verify that a quick race (IPC mid-flight) still
     results in a clean cancel via the backend hook.

### 7. Wrong vs Correct

#### Wrong: envelope added to a tool's return type

```rust
// BAD — modifies the tool's signature, breaks 60+ tests
pub async fn execute(...) -> Result<serde_json::Value, GitError> {
    Ok(json!({ "result": output, "cwd": ctx.worktree_path }))
}
```

LLM transparency logic is now baked into the business logic of every
tool. Future tools must remember to do the same. The 60+ existing
unit tests have to be rewritten.

#### Correct: envelope applied at the LLM-facing boundary only

```rust
// GOOD — tool internals unchanged
pub async fn execute(...) -> Result<String, GitError> { ... }

// lib.rs::chat at the agent-loop boundary
let wire = tool_result_envelope(output, &ctx.worktree_path);
emit(ChatEvent::ToolResult { content: wire, ... });
persist(ContentBlock::ToolResult { content: wire, ... });
```

The `tool_result_envelope` function lives in `lib.rs` and is the
single place that knows about the envelope shape. Tools remain pure
data producers; the agent loop is the formatter.

#### Wrong: destructive path without cancel

```rust
// BAD — race against an in-flight chat
async fn delete_session(state, sid) -> Result<()> {
    cleanup_outputs_dir(...).await;     // shells out
    destroy_worktree(...).await;        // libgit2
    db::delete_session(...).await;      // FK cascades
}
```

If the agent loop is mid-turn, its `db::insert_message(...)` or
`db::update_session(...)` may fire after the row is gone, hitting FK
violations or silently dropping the message. The LLM's next-turn
history is also missing the messages that didn't land.

#### Correct: cancel, then proceed

```rust
// GOOD — cancel hook at the top
async fn delete_session(state, sid) -> Result<()> {
    cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        sid,
    ).await;
    // CancellationGuard's Drop will remove the map entry once the
    // agent loop exits; we don't wait for it explicitly — the next
    // operation in this function does not need the entry removed.
    cleanup_outputs_dir(...).await;
    destroy_worktree(...).await;
    db::delete_session(...).await;
}
```

The destructive path is bounded by the cancel; the agent loop's
`tokio::select!` will exit on the next event boundary, and the guard
cleans the map.

#### Wrong: signal worktree state only via user-role event messages

```rust
// BAD — system field hard-coded to None; rely on [worktree event]
// rows in history as the only signal
chat_stream_with_tools(config.clone(), messages.clone(), tools.clone());
// (system: None inside the function)
```

The model honestly answers "no" when asked "does your system prompt
mention you're in a worktree" — because the `system` field IS empty.
The `[worktree event]` user-role messages tell the model "the user
told me X happened", but the model treats them as user assertions,
not authoritative grounding. Worse, the events describe
*transitions*; if the user attaches a worktree and then sends 5
unrelated messages, the relevant `[worktree event]` is buried in
history and the model has no compact, always-present statement of
its current state.

#### Correct: build system prompt once per chat, pass via `system:`

```rust
// GOOD — build_system_prompt fills the Anthropic `system` field
let head_sha = lookup_head_sha(&worktree_path);
let system_prompt = build_system_prompt(
    &loaded_session.session,
    &project,
    &worktree_path,
    &head_sha,
);
for turn in 1..=MAX_TURNS {
    let stream = chat_stream_with_tools(
        config.clone(),
        Some(system_prompt.clone()),
        messages.clone(),
        tool_defs.clone(),
    );
    // ...
}
```

The system prompt is the model's *current state*; the `[worktree
event]` messages remain in history so the model can recall the
*transition* (useful if the model wants to explain "I just attached
the worktree"). Both work together:

- **System prompt** = persistent declaration. Built once per chat
  invocation. Lives in `lib.rs::build_system_prompt`, single source
  of truth.
- **`[worktree event]`** = transition log. Injected at attach /
  detach / delete time as a user-role message, persisted in the
  `messages` table.
- **Tool result envelope `cwd`** = runtime data point per tool
  call. Confirms what cwd the specific tool actually ran against.

---



### Decision: Always send `thinking`, no per-session / per-request toggle

**Context**: MVP UX. Adding a toggle would expand the settings surface and the
DB schema.

**Decision**: `thinking` is always in the request body. The only knob is
`LLM_THINKING_EFFORT` env, applied globally.

**Consequences**: Simple. If the upstream model does not support adaptive
thinking the call 400s — accepted as an operational risk.

### Decision: `display: "summarized"` is explicit, never omitted

**Context**: Opus 4.7+ defaults to `display: "omitted"`, which suppresses
`thinking_delta` SSE events and breaks the UI's streaming label.

**Decision**: `ThinkingConfig::Adaptive { display: "summarized", effort }` is
hard-coded in `LlmConfig::thinking_config()`.

**Consequences**: Streamed thinking is always visible. (Trade ~1-2 ms per
response for guaranteed streaming.)

### Decision: `max_tokens` default 1024 → 16384

**Context**: Thinking tokens count against the same budget as the actual answer.
1024 was too low — non-trivial turns would hit `stop_reason: "max_tokens"`.

**Decision**: `DEFAULT_MAX_TOKENS = 16384`.

**Consequences**: Cheap requests waste ~8k of budget, but no truncation on
real workloads. Env override available.

---

## Common Mistakes

### Mistake: Treating `redacted_thinking` as a partial `thinking`

The `data` field of `redacted_thinking` is opaque — Anthropic redacts it for
safety reasons. Don't try to "decode" it, don't append it to the visible
thinking text, don't mutate it. Store and forward verbatim. The UI shows a
"🔒 redacted" placeholder.

### Mistake: Dropping the signature to "save space"

The `signature` is a cryptographic anchor — Anthropic uses it to verify the
thinking block was generated by the same session. Drop it, and the next turn
400s. The DB stores it in full; the rehydrate path emits it in full.

### Mistake: Ordering thinking blocks after tool_use in history

Anthropic requires thinking blocks at the head of an assistant message. If
`toPayloadContent` appends thinking at the tail (or interleaves it with
tool_use), the next turn 400s. The fix lives in `toPayloadContent`; the
ordering is tested in the frontend type-check.

---

## Anti-Patterns

- **Don't** add a per-session or per-request thinking toggle. (MVP simplification;
  see PRD D1.)
- **Don't** add an `LLM_THINKING=off` env var. (PRD D5; if the upstream is broken,
  fix the upstream or change code.)
- **Don't** parse `signature` client-side. It is opaque; the proxy might transform
  it, but the wire format is "string of unknown base64-like data."
- **Don't** coalesce multiple thinking blocks into one on rehydrate. The
  interleaving is meaningful when it happens (rare), and Anthropic expects
  the same order on round-trip.

---

## Future Work (Deferred from Step 6)

| Item | Why deferred |
|------|-------------|
| Parse `usage.output_tokens_details.thinking_tokens` from `message_delta` | UI uses `length / 4` estimate; real count would require a new `ChatEvent` variant + plumbing. |
| Preserve interleaved `thinking → redacted_thinking` order | Redacted is rare and interleaved is even rarer; current code appends redacted to the tail. |
| Coalesce `text → text` into one block | Multiple text blocks in one turn are coalesced; interleaved `thinking → text → thinking → text` loses the second text's position. |
| Fix pre-existing `tool_result` in `assistant` role | From step 3a; out of step 6 scope. Follow-up task. |
| OpenAI-compat `reasoning_content` fallback | Different wire format entirely; would require its own `ContentBlock` variant + parser. |
