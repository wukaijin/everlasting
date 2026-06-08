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

#### Synthetic `tool_result` on cancel (BUG FIX 2013 tool_use orphan)

- **Trigger**: the agent loop's cancel branch fires after one or more
  `tool_use` blocks have been streamed and accumulated but before any
  tool has executed (PR5 `Stop`, `attach_worktree`'s in-flight cancel
  hook, network drop, or any path that returns from the agent loop
  with `cancelled = true` and `tool_calls` non-empty).
- **Required behavior**: the cancel branch must persist a synthetic
  `user`-role `ChatMessage` carrying one `ContentBlock::ToolResult`
  per `(id, name, _input)` triple, then emit `done { stop_reason:
  "cancelled" }` and return. Pre-fix, the cancel branch returned
  immediately after persisting the assistant turn, leaving the DB
  with an orphan `tool_use` and no matching `tool_result` — the
  next `send()` then built a malformed history and the Anthropic API
  returned 2013 ("tool call result does not follow tool call").
- **Synthetic block shape** (must match the wire contract):
  - `type: "tool_result"`
  - `tool_use_id: <id>` (mirrors the corresponding `tool_use` block's id)
  - `content: "Tool execution was interrupted: the user stopped the
    request or the session was cancelled before the tool could run.
    The tool <name> did not run."` (English + tool name per the
    `HACKING-llm.md` "陷阱 3" decision)
  - `is_error: true` (the Anthropic schema's strong signal that the
    tool failed; combined with the content wording this usually
    causes the model to retry the tool_use on the next turn rather
    than reason on the empty result)
- **Helper**: `build_synthetic_tool_result_message(tool_calls: &[(String, String, serde_json::Value)]) -> ChatMessage` in `app/src-tauri/src/lib.rs`. Pure function over `tool_calls`; no DB / Tauri deps. Extracted as a free function (not inlined in the cancel branch) so the invariants are unit-testable in isolation.
- **Order invariants** (must hold after this fix lands):
  1. `assistant(tool_use)` is persisted FIRST, then the synthetic
     `user(tool_result)` is persisted. Persisting in the other order
     would still be malformed (Anthropic rejects `tool_result` blocks
     not preceded by a matching `tool_use` in the immediately-prior
     assistant message).
  2. `seq` is strictly increasing across the two rows.
  3. The synthetic message is NOT emitted as a `tool:result` Tauri
     event (the event is for UX feedback on actually-executed tools;
     no tool ran here, and emitting it would confuse the frontend's
     streaming pipeline).
  4. `stop_reason: "cancelled"` is still emitted on `done` — the
     frontend's cancel path doesn't change.

#### Orphan tool_use repair on rehydrate (BUG FIX 2013, frontend side)

- **Trigger**: any historical session row in the `messages` table
  where an `assistant` turn contains `tool_use` blocks with no
  matching `tool_result` in the immediately-following `user` turn.
  These can predate the synthetic-tool_result fix above (cancel /
  network drop in the old code) and can also come from a future
  bug that re-introduces the gap.
- **Required behavior**: the frontend's `rehydrateMessages` in
  `app/src/stores/streamController.ts` must splice in a synthetic
  `user`-role `ChatMessage` with one `tool_result` block per orphan
  `tool_use` id, immediately after the orphan assistant message.
  Without this, the next `send()` pushes a malformed history and
  the API returns 2013.
- **Detection rules** (applied to the post-merge-step message array):
  - An `assistant` message is considered to have an orphan `tool_use`
    when `toolCalls[i].id` is not in the union of:
    1. The assistant's own `toolResults[*].toolUseId` (set by the
       merge step from a later user message).
    2. The immediately-following `user` message's
       `toolResults[*].toolUseId`.
  - Loop direction: **reverse scan** (i = out.length-1 down to 0)
    so that `splice(i+1, 0, syntheticMsg)`'s index shift doesn't
    affect the next iteration.
- **Synthetic block shape on rehydrate** must match the backend's
  cancel-path synthetic exactly (same wording, same `is_error: true`,
  same tool name in content). The two repair paths must stay in lockstep;
  if they ever diverge, the LLM will see inconsistent recovery
  behavior depending on whether a session was repaired by the
  backend or the frontend.
- **Wire-effect**: the spliced synthetic message participates in
  `toPayloadContent` exactly like a real user-role `tool_result`
  message — `assistant(tool_use)` and `user(tool_result)` are
  emitted as two adjacent messages, satisfying the Anthropic
  contract.
- **Tests required** (locked in `app/src/stores/streamController.test.ts`):
  - Orphan `tool_use` with no following user → spliced synthetic
  - Multiple orphan `tool_use` in the same assistant → all repaired
  - Paired `tool_use` + `tool_result` (normal case) → NOT touched
  - Orphan at end-of-array (no following user at all) → spliced
  - Existing merge step (user.toolResults → preceding assistant) is
    preserved by the refactor

#### In-memory must mirror DB on send completion (BUG FIX 2026-06-08, 2013 reappears in normal-completion path)

- **Trigger**: any `chat` IPC that **completes** (not just cancels)
  while the agent loop ran at least one tool. The pre-fix behavior
  kept the in-memory `streamController.messagesBySession` cache
  alive after `done` so the user could keep viewing the session;
  the in-memory shape is the *streaming-accumulation* shape
  (single `assistantMsg` placeholder that absorbed every `delta`
  / `tool_call` / `tool_result` / `thinking_delta` event across
  all turns), while the DB shape is one assistant message per
  agent-loop turn (per `lib.rs:chat`).
- **Failure mode**: a subsequent `send()` for the same session
  hits `ensureLoaded`'s in-memory fast path, the cache is the
  accumulation shape, `toPayloadContent` for `assistant` role
  emits `tool_use` (per the Anthropic contract: `tool_result`
  blocks only go on user-role messages), and the next wire
  message after the assistant turn is a user-text prompt with
  no `tool_result` in between. Anthropic Messages API returns
  2013 ("tool call result does not follow tool call").
- **Required behavior**: `streamController.finalizeRequest` (the
  function the `done` / `error` / catch-error paths all route
  through) must:
  1. `evict(sessionId)` — clears `messagesBySession`,
     `loadedFromDb`, and `pinnedSessions` for the session, so
     the next `ensureLoaded` takes the re-load-from-DB path and
     gets the per-turn split shape.
  2. `useChatStore().invalidateDiff(sessionId)` — clears the
     worktree diff cache for the same session, so the worktree
     chip's "diff (N)" counter re-fetches on the next read
     (after a `git commit` ran inside the worktree, etc.).
- **Why both, paired**: the in-memory shape and the diff cache
  are owned by different stores (`streamController` vs `chat`),
  but they're both stale on send completion for the same root
  reason. A refactor that only calls one of the two would
  silently re-introduce one of the two bugs above. The
  `streamController.test.ts` `finalizeRequest` describe block
  has a `both actions fire on the same finalizeRequest call
  (paired invariant)` test that locks this.
- **Relation to the cancel-path fix** (`Synthetic tool_result
  on cancel` above, c35c384): the two fixes address
  *different* 2013 paths. The cancel-path fix prevents the DB
  from developing an orphan `tool_use` when the user stops
  mid-stream. The in-memory-mirror fix prevents the wire-format
  history from having an apparent orphan `tool_use` even when
  the DB is fully self-consistent. Both must stay in place —
  removing either re-opens 2013 under a different repro path.
- **Tests required** (locked in
  `app/src/stores/streamController.test.ts`,
  `finalizeRequest` describe block):
  - `evicts the in-memory message buffer and unloads from DB
    cache` (after `done` / after `error`)
  - `invalidates the chat store's diff cache for the same
    session` (after `done` / after `error`)
  - `both actions fire on the same finalizeRequest call (paired
    invariant)` — paired test, not two independent tests

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
| `chat` cancel branch with empty `tool_calls` | Returns immediately (no synthetic message persisted; `seq` not incremented) |
| `chat` cancel branch with non-empty `tool_calls` | Persists `assistant(Blocks{tool_use...})` THEN synthetic `user(Blocks{tool_result...})`; both rows must have strictly-increasing `seq` |
| Synthetic `tool_result` content / `is_error` field | `serde_json::Value` round-trip preserves `type: "tool_result"`, `tool_use_id`, `content` (with tool name), `is_error: true` (the `is_false` skip filter only drops `false`) |
| Rehydrate orphan `tool_use` (frontend) | Spliced synthetic message has `role: "user"`, `content: ""` (no text), `toolResults: [{toolUseId, content, isError: true}]`; message's `id` is `<assistant.id>-orphan-repair` |
| Rehydrate paired `tool_use` (frontend) | Output array length unchanged (no synthetic inserted) |

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
| `lib::synthetic_tool_result_message_mirrors_tool_calls` | One `tool_call` → one `ToolResult` block with matching `tool_use_id`, `is_error: true`, tool name in content |
| `lib::synthetic_tool_result_message_preserves_order_for_multi_call` | 3 `tool_call`s → 3 `ToolResult` blocks in the same order, all `is_error: true` |
| `lib::synthetic_tool_result_message_empty_when_no_tool_calls` | Empty input → empty `Blocks` (no stray user message) |
| `lib::synthetic_tool_result_message_serializes_to_anthropic_wire_shape` | `serde_json::to_string(msg)` round-trip produces `{"role":"user","content":[{"type":"tool_result","tool_use_id":"X","content":"...","is_error":true}]}` |
| `vitest rehydrateMessages::splices_synthetic_user_tool_result_after_orphan_assistant` | Orphan `tool_use` → synthetic `user(tool_result)` spliced in at `i+1` |
| `vitest rehydrateMessages::does_not_splice_when_paired` | Normal `assistant(tool_use)` + `user(tool_result)` pair → no extra synthetic |
| `vitest rehydrateMessages::repairs_every_orphan_in_same_assistant` | Multi-call orphan → all `tool_use` ids covered by the spliced synthetic |
| `vitest rehydrateMessages::synthetic_id_is_unique` | Synthetic message's `id` ≠ the orphan assistant's `id` (won't collide with `send()` placeholder) |
| `vitest rehydrateMessages::orphan_at_end_of_array_repaired` | Last-message orphan → synthetic still spliced in (loop must not underflow) |
| `vitest rehydrateMessages::empty_messages_array_does_not_crash` | Defensive: `load_session` returning `[]` rehydrates to `[]` |
| `vitest rehydrateMessages::merge_step_preserved` | Pre-existing merge step (`user.toolResults` → preceding `assistant.toolResults`) is not regressed by the orphan-repair refactor |

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

## Scenario: Multi-Provider Abstraction (PR1 of 06-08-multi-model-llm-provider-planning)

### 1. Scope / Trigger

- Trigger: User-managed catalog of LLM providers + models. PR1 ships
  the data layer (3 new SQLite tables, 8 CRUD functions, 10 IPC
  commands, idempotent seed); PR2 (Anthropic adapter) and PR3 (OpenAI
  adapter) implement the `Provider` trait dispatch off this catalog.
- Why code-spec depth: mandatory — the new tables, IPC payload shapes,
  and `ProviderProtocol` enum are the cross-layer contract that PR2 /
  PR3 / PR4 all depend on. A change here cascades.

### 2. Signatures

#### DB types (`app/src-tauri/src/db.rs`)

```rust
pub enum ProviderProtocol {
    Anthropic,  // Messages API
    Openai,     // Chat Completions
}

pub struct ProviderRow {
    pub id: String,            // UUID v4
    pub protocol: String,      // "anthropic" | "openai" (TEXT, not enum, for forward-compat)
    pub display_name: String,  // user-facing label
    pub base_url: String,
    pub api_key: String,
    pub created_at: String,    // RFC 3339
    pub updated_at: String,
}

pub struct ModelRow {
    pub id: String,            // UUID v4
    pub provider_id: String,   // FK to providers.id (ON DELETE CASCADE)
    pub model_name: String,    // sent to API
    pub display_name: String,  // UI label
    pub max_tokens: Option<u32>,        // None = fall back to global
    pub thinking_effort: Option<String>,// None = fall back to global
    pub supports_thinking: bool,        // capabilities bit
    pub context_window: u32,            // total capacity (input+output)
    pub created_at: String,
    pub updated_at: String,
}

pub struct ModelWithProvider {
    #[serde(flatten)]
    pub model: ModelRow,
    pub provider_display_name: String,  // denormalized for UI list
    pub provider_protocol: String,
}
```

#### Tables (PR1 schema)

```sql
CREATE TABLE providers (
    id           TEXT PRIMARY KEY,
    protocol     TEXT NOT NULL,
    display_name TEXT NOT NULL,
    base_url     TEXT NOT NULL,
    api_key      TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE models (
    id                TEXT PRIMARY KEY,
    provider_id       TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_name        TEXT NOT NULL,
    display_name      TEXT NOT NULL,
    max_tokens        INTEGER,           -- nullable
    thinking_effort   TEXT,              -- nullable
    supports_thinking INTEGER NOT NULL DEFAULT 0,
    context_window    INTEGER NOT NULL,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);
CREATE INDEX idx_models_provider_id ON models(provider_id);

CREATE TABLE app_config (
    key   TEXT PRIMARY KEY,
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

### 3. Contracts

#### Wire shape (camelCase to JS via `#[serde(rename_all = "camelCase")]`)

```jsonc
// list_providers response
[
  { "id": "uuid", "protocol": "anthropic", "displayName": "Anthropic 官方",
    "baseUrl": "https://api.anthropic.com", "apiKey": "sk-...",
    "createdAt": "...", "updatedAt": "..." }
]

// list_models response (ModelWithProvider.flatten)
[
  { "id": "uuid", "providerId": "uuid", "modelName": "claude-sonnet-4-5",
    "displayName": "Claude Sonnet 4.5", "maxTokens": null,
    "thinkingEffort": null, "supportsThinking": true,
    "contextWindow": 200000, "createdAt": "...", "updatedAt": "...",
    "providerDisplayName": "Anthropic 官方", "providerProtocol": "anthropic" }
]
```

#### IPC arg names (Tauri 2 auto-converts from JS camelCase to Rust snake_case)

```typescript
// JS — camelCase
await invoke("add_model", {
    providerId: "uuid",
    modelName: "claude-sonnet-4-5",
    displayName: "Claude Sonnet 4.5",
    maxTokens: 8192,         // number or omit for None
    thinkingEffort: "high",  // string or omit for None
    supportsThinking: true,
    contextWindow: 200000,
})
```

#### `Option<T>` args — HACKING-wsl FU-1 pattern

For `add_model` / `update_model` `Option<u32>` and `Option<String>` args
(`max_tokens`, `thinking_effort`):

- **JS omit the field for `None`** — do NOT pass `null`. Tauri 2 IPC
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

### 4. Validation & Error Matrix

| Condition | Error |
|---|---|
| `add_model` with `provider_id` not in `providers` | FK violation (SQLITE_CONSTRAINT) — wrapped as `String` at IPC |
| `add_model` with `context_window: 0` | Accepted (no min validation); UI should prevent in form |
| `update_provider` / `update_model` on missing `id` | `None` returned (NOT an error) |
| `delete_provider` on missing `id` | `false` returned |
| `get_default_model` when `default_model_id` is unset | `None` returned |
| `get_default_model` when `default_model_id` points to a deleted model | `None` returned (the `list_models` filter finds no match) |
| `set_default_model` with non-existent `model_id` | Accepted (no FK validation); the next `get_default_model` returns `None`. PR4 will add the pre-flight check. |

### 5. Good / Base / Bad Cases

- **Good**: User opens Settings → adds "Anthropic 官方" provider with
  their `sk-ant-...` key → adds "claude-sonnet-4-5" model under it →
  sets it as default. `get_default_model` returns the model. New
  sessions auto-pick it. UI shows `(no model)` warning until key is
  filled.
- **Base**: Default seed runs on first install; user never opens
  Settings. The 2 seeded providers + 4 models + 1 default give the
  app enough to function; the user just needs to fill `api_key` for
  the provider they want to use.
- **Bad**: User adds a "claude-sonnet-4-5" model to a provider with
  no `api_key` and tries to send a message. PR2's pre-flight check
  should reject this with a "请先到 Settings 填 api_key" toast.
  PR1's `set_default_model` accepts it (the value is stored); PR2
  enforces.

### 6. Tests Required

- [ ] `cargo test --lib db::` — 11 new tests (covered PR1, see
      `db.rs` `#[cfg(test)] mod tests` PR1 section)
- [ ] Each CRUD function: 1 happy + 1 error path
- [ ] Cascade: `delete_provider` removes its models
- [ ] Cascade: `delete_provider` does NOT touch other providers' models
- [ ] Seed: idempotent (running twice doesn't double the catalog)
- [ ] Seed: sets `default_model_id` to a real model
- [ ] Seed: backfills `sessions.model_id` for legacy rows
- [ ] `app_config` round-trip: set + get + overwrite

### 7. Wrong vs Correct

#### Wrong

```typescript
// JS — passing `null` for an Option<String> arg
await invoke("add_model", {
    providerId: "uuid",
    modelName: "claude-sonnet-4-5",
    displayName: "...",
    maxTokens: null,           // ❌ Tauri 2 treats null as missing
    thinkingEffort: null,      // ❌ same — error message hides the field name
    supportsThinking: true,
    contextWindow: 200000,
})
```

```rust
// Rust — declaring sessions.model_id as a hard FK
ALTER TABLE sessions ADD COLUMN model_id TEXT
    REFERENCES models(id);  // ❌ breaks legacy rows + INSERTs
```

#### Correct

```typescript
// JS — omit the field for None
await invoke("add_model", {
    providerId: "uuid",
    modelName: "claude-sonnet-4-5",
    displayName: "...",
    // maxTokens omitted → None
    // thinkingEffort omitted → None
    supportsThinking: true,
    contextWindow: 200000,
})
```

```rust
// Rust — soft FK + backfill in run_migrations
add_session_column_if_missing(pool, "model_id", "TEXT").await?;
// Backfill at the end of run_migrations:
seed_default_providers_and_models(pool).await?;  // also UPDATEs NULL model_id
```

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

#### Decision: 10 IPC commands, not the prd's 7-8

**Context**: The prd's PR1 estimate was 7-8 CRUD IPC. PR1 ships 10
because `get_default_model` and `set_default_model` are exposed as
typed IPC commands rather than a generic `get_app_config(key)` /
`set_app_config(key, value)`.

**Decision**: 10 commands. The typed shape is self-documenting in the
`invoke_handler!` macro and gives the frontend a clear contract.

**Consequences**: Slightly more boilerplate at the IPC layer, but the
catalog API is explicit. If a future `app_config` key needs a typed
IPC, add another pair rather than a generic accessor.

#### Decision: idempotent seed on first run (no migration version)

**Context**: The seed inserts 2 providers + 4 models + a default
model id. A user who deletes everything shouldn't get the seed
re-run; a user who never opened the app should get it on first
launch.

**Decision**: Gate the seed on `SELECT COUNT(*) FROM providers = 0`.
This is a one-time, irreversible trigger.

**Consequences**: If we ever change the default catalog (e.g. add
"claude-opus-4-8" to the seed list), existing installs won't pick it
up. The recovery path is "manually add the new model in Settings".

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

---

## Scenario: Provider trait + Anthropic dispatch (PR2 of 06-08-multi-model-llm-provider-planning)

### 1. Scope / Trigger

- Trigger: PR1 shipped the catalog (3 tables + 8 CRUD + 10 IPC +
  seed). PR2 introduces a `Provider` trait abstraction and a
  catalog-resolved dispatch in the `chat` command. All sessions
  still go through the Anthropic protocol (OpenAI is PR3) — the
  goal is the architectural refactor + the catalog path, with
  end-to-end behavior 1:1 identical to pre-PR2.
- Why code-spec depth: mandatory — the trait surface is the
  cross-layer contract PR3 (OpenAI) and PR4 (UI) will build on;
  the catalog resolution + 3-way pre-flight are the new failure
  modes the chat command can hit; the 1:1 behavior contract
  protects the user from any wire-level regression.

### 2. Signatures

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
    NotImplemented(&'static str),   // e.g. "openai" in PR2
    UnknownProtocol(String),         // forward-compat: a future binary wrote a value the current binary doesn't know
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

#### `chat` command pre-flight (`app/src-tauri/src/lib.rs`)

```rust
struct ResolvedChatProvider {
    provider: Box<dyn llm::Provider>,
    model_display_name: String,
    provider_display_name: String,
}

enum PreFlightError {
    NoModel,                                                  // PRD Q2 #2
    ProviderMissing,                                          // PRD Q2 #3
    EmptyApiKey { provider_display_name: String },            // PRD Q2 #1
    BuildFailed(llm::ProviderBuildError),                     // generic dispatcher error
}

async fn resolve_chat_provider(
    db: &SqlitePool,
) -> Result<ResolvedChatProvider, PreFlightError>;
```

#### `get_llm_config` IPC (`app/src-tauri/src/lib.rs`)

The IPC is now `async` and reads the catalog. The wire shape is
unchanged (`{model, baseUrl, configured}`); the `model` field
now carries `ModelRow.display_name` (e.g. "Claude Sonnet 4.5")
per the PR2 PRD D1 decision.

### 3. Contracts

#### 1:1 wire behavior (the only hard constraint of PR2)

| Concern | Pre-PR2 (env path) | Post-PR2 (catalog path) |
|---------|-------------------|------------------------|
| Request URL | `ANTHROPIC_BASE_URL + "/v1/messages"` | `provider_row.base_url + "/v1/messages"` (same shape; base_url is now a catalog value, not an env value) |
| Headers | `x-api-key: <ANTHROPIC_API_KEY>`, `anthropic-version: 2023-06-01`, `content-type: application/json` | Identical; `api_key` is `provider_row.api_key` |
| `thinking` field | `{type: "adaptive", display: "summarized", effort: LLM_THINKING_EFFORT \|\| "high"}` | `{type: "adaptive", display: "summarized", effort: model.thinking_effort \|\| "high"}` |
| `max_tokens` | `LLM_MAX_TOKENS` \|\| 16384 | `model.max_tokens` \|\| 16384 |
| `model` field | `LLM_MODEL` | `model.model_name` |
| Tool definitions | `builtin_tools()` | Identical (catalog dispatch doesn't touch tools) |
| SSE event sequence | text / tool_use / thinking / redacted_thinking + signature_delta | Identical (the `BlockState` state machine is unchanged) |

#### Provider dispatch timing

The provider is constructed **once per `chat` invocation**,
before the `for turn in 1..=MAX_TURNS` loop, and the same
`Box<dyn Provider>` is used for all 20 turns. The user's
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
| `default_model_id` unset or model row missing | `"没有可用 model,请到 Settings 选 default model"` | `InvalidRequest` |
| `model_row.provider_id` points to a deleted provider | `"default model 指向的 provider 已被删除,请到 Settings 重选"` | `InvalidRequest` |
| `provider_row.api_key` empty | `"请到 Settings 填 {provider_display_name} 的 api_key"` | `Auth` |
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

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `app_config["default_model_id"]` is `NULL` | `PreFlightError::NoModel` → "没有可用 model..." |
| `default_model_id` points to a deleted `models` row | `PreFlightError::NoModel` → "没有可用 model..." |
| `default_model_id` points to a row whose `provider_id` is dangling | `PreFlightError::ProviderMissing` → "default model 指向的 provider..." |
| `provider.api_key` is empty string | `PreFlightError::EmptyApiKey { provider_display_name }` → "请到 Settings 填 {display_name} 的 api_key" |
| `provider.protocol` is `"openai"` (PR2: not yet implemented) | `PreFlightError::BuildFailed(NotImplemented("openai"))` → "无法构造 LLM provider: provider protocol 'openai' is not implemented yet" |
| `provider.protocol` is an unknown string (forward-compat) | `PreFlightError::BuildFailed(UnknownProtocol(s))` → "无法构造 LLM provider: unknown provider protocol: 's'" |
| `provider.api_key` is set, model row has `max_tokens=Some(8192)` | `AnthropicProvider::new(LlmConfig { max_tokens: 8192, ... })` → request body has `"max_tokens": 8192` |
| `provider.api_key` is set, model row has `max_tokens=None` | Factory falls back to `16384` (the Anthropic default) |
| `provider.api_key` is set, model row has `thinking_effort=Some("xhigh")` | Request body has `"thinking": {"type": "adaptive", "display": "summarized", "effort": "xhigh"}` |
| `provider.api_key` is set, model row has `thinking_effort=None` | Falls back to `"high"` (matches the pre-PR2 env default) |
| `provider_row.base_url` has trailing `/` | `endpoint()` strips it (matches pre-PR2 `LlmConfig::endpoint()` behavior) |
| `get_llm_config` called before any default is set | Returns `Ok(PublicLlmConfig { model: "", base_url: "", configured: false })` |

### 5. Good / Base / Bad Cases

#### Good: full happy path

1. User opens the app for the first time. Seed runs:
   `Anthropic 官方` (empty api_key) + `claude-sonnet-4-5`
   bound to it + `default_model_id` = the sonnet row.
2. User goes to Settings, pastes their `sk-ant-...` into
   `Anthropic 官方.api_key`.
3. User opens a session, types a question, clicks Send.
4. Frontend calls `invoke("chat", { requestId, sessionId, messages })`.
5. Backend `chat`:
   - `resolve_chat_provider(db)`:
     - reads `app_config["default_model_id"]` → sonnet UUID
     - finds the sonnet `ModelWithProvider` row
     - finds the `Anthropic 官方` provider row
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
   model,请到 Settings 选 default model", category: InvalidRequest }`
6. Frontend shows the error in the chat panel + (post-PR4)
   "跳到 Settings" button.

#### Bad: per-event `signature_delta` emit (regression check)

PR2 must not regress the step 6 signature-buffer fix (see the
earlier "Scenario: Extended Thinking Support" section). The
`AnthropicProvider::chat_stream_with_tools` body is a verbatim
move of the pre-PR2 implementation, so the
`BlockState::Thinking { signature_buf }` buffering is
preserved. The 4 client.rs tests in
`provider/anthropic::tests::*` (`default_max_tokens`,
`thinking_config_is_adaptive_summarized_with_configured_effort`,
`unconfigured_has_empty_thinking_effort`,
`chat_request_system_field_serializes_when_some`) are the
regression net.

#### Bad: tool envelope lost

PR2 must not regress the step 4 follow-up tool envelope
(`{"result": "<content>", "cwd": "<worktree_path>"}`). The
envelope is applied in `lib.rs::chat` at the agent-loop
boundary, NOT inside the provider. The provider returns
`ChatEvent::ToolResult { content: <raw string>, ... }` and the
chat command wraps it via `tool_result_envelope(...)` before
emitting `tool:result` and persisting the `ContentBlock::ToolResult`.
The pre-existing `tool_result_envelope_round_trip` test in
`lib.rs::tests` continues to lock this.

### 6. Tests Required

The 4 pre-existing client.rs tests moved into
`provider/anthropic::tests` (unchanged). 7 new tests added in
PR2:

| Test | Asserts |
|------|---------|
| `llm::provider::tests::build_provider_anthropic_returns_anthropic_provider` | `protocol() == Anthropic`, all 3 capabilities true |
| `llm::provider::tests::build_provider_openai_returns_not_implemented` | `ProviderBuildError::NotImplemented("openai")` |
| `llm::provider::tests::build_provider_unknown_protocol_returns_error` | `ProviderBuildError::UnknownProtocol(s)` for unknown strings |
| `llm::provider::tests::factory_passes_model_max_tokens` | Factory threads `model.max_tokens` into `LlmConfig` (verified via successful construction) |
| `llm::provider::tests::factory_falls_back_to_default_max_tokens_and_effort` | `None` model overrides → `max_tokens=16384`, `thinking_effort="high"` |
| `llm::provider::tests::provider_build_error_displays_human_readable` | `ProviderBuildError` impls `Display` (used in `tracing::warn!` / IPC error path) |
| `llm::provider::tests::provider_protocol_reexport_matches_db` | `llm::ProviderProtocol` re-export is the same enum as `db::ProviderProtocol` |
| `llm::provider::anthropic::tests::anthropic_provider_reports_capabilities_and_protocol` | `AnthropicProvider::protocol() == Anthropic`, capabilities all true |
| `llm::provider::anthropic::tests::anthropic_provider_is_send_sync` | `AnthropicProvider: Send + Sync` (compile-time assertion, `Box<dyn Provider>` is movable) |
| `llm::provider::anthropic::tests::factory_built_provider_reports_anthropic_capabilities` | End-to-end: `build_provider` → `protocol()` + `capabilities()` |

Pre-existing test count: 208 (pre-PR2). PR2 net new: 10 tests
(7 in `llm::provider::tests` + 3 in
`llm::provider::anthropic::tests`). Total: 218 (verified via
`cargo test --lib` — 0 warnings, 0 failures).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. PR2 does NOT change
  the frontend; the `get_llm_config` IPC's wire shape
  (`{model, baseUrl, configured}`) is preserved, the
  `useConfigStore.load()` code is untouched.
- Manual smoke test (acceptance A2 from the parent PRD):
  1. `cd app && pnpm tauri dev`
  2. Open Settings, see the 2 seeded providers + 4 seeded
     models. Default is `claude-sonnet-4-5`.
  3. Open a session, type a question, click Send.
  4. Observe the LLM responds — same wire behavior as pre-PR2.
  5. In Settings, delete the `claude-sonnet-4-5` model (or
     blank its `provider.api_key`).
  6. Try to send a message; observe the locked-in PRD §Q2
     error message ("请到 Settings 填 Anthropic 官方的 api_key"
     or "没有可用 model...").

### 7. Wrong vs Correct

#### Wrong: per-turn provider construction

```rust
// BAD — constructs a new provider per turn; user-visible
// protocol drift if a different default is selected mid-loop
for turn in 1..=MAX_TURNS {
    let provider = build_provider(&provider_row, &model_row)?;
    let mut stream = provider.send(...);
    // ...
}
```

If `attach_worktree` / `set_default_model` runs between turns
(via a second IPC in another window), the per-turn
construction would silently switch providers — a wire-protocol
inconsistency Anthropic would 400 on.

#### Correct: once-per-chat construction

```rust
// GOOD — resolve once, reuse for all 20 turns
let resolved = resolve_chat_provider(&db).await?;
let provider = resolved.provider;
for turn in 1..=MAX_TURNS {
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
    (format!("请到 Settings 填 {} 的 api_key", provider_display_name),
     LlmErrorCategory::Auth)
}
```

PR4's modal: the `category` is `Auth` → jump to the
provider's `api_key` field. `InvalidRequest` + "没有可用
model" → jump to the default-model picker. `InvalidRequest`
+ "default model 指向的 provider..." → jump to the
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

#### Decision: 7 new tests, not 5

**Context**: The PR2 PRD said "4-5 new build_provider +
factory tests". The implementation added 10 new tests
(7 in `llm::provider::tests` + 3 in
`llm::provider::anthropic::tests`) to cover the catalog
wiring, the Send + Sync invariant, and the Anthropic
provider's re-export of `db::ProviderProtocol`.

**Decision**: 10 new tests. The Send + Sync assertion is
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

## Scenario: OpenAI Chat Completions adapter + cross-protocol WireMessage (PR3 of 06-08-multi-model-llm-provider-planning)

### 1. Scope / Trigger

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

### 2. Signatures

#### Wire layer (`app/src-tauri/src/llm/provider/wire.rs` — new)

```rust
pub struct WireRequest {
    pub model: String,
    pub max_tokens: Option<u32>,
    pub system: Option<String>,
    pub messages: Vec<WireMessage>,
    pub tools: Vec<WireTool>,
    pub reasoning_effort: Option<String>,  // OpenAI o1/o3
}

pub enum WireMessage {
    User { content: String },
    Assistant { blocks: Vec<WireBlock> },
    Tool { tool_call_id: String, content: String },
}

pub enum WireBlock {
    Text { text: String },
    Reasoning { text: String },             // Anthropic thinking / OpenAI reasoning_content
    Signature { data: String },             // Anthropic-only
    RedactedThinking { data: String },      // Anthropic-only
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
    pub reasoning_effort: Option<String>,  // from ModelRow.thinking_effort
}

pub struct OpenAIProvider { config: OpenAIConfig }
impl OpenAIProvider { pub fn new(config: OpenAIConfig) -> Self; }
impl Provider for OpenAIProvider { ... }
```

### 3. Contracts

#### Protocol differences (the only spec table PR3 needs)

| Concern | Anthropic (PR2) | OpenAI (PR3) |
|---------|-----------------|---------------|
| URL | `provider.base_url + "/v1/messages"` | `provider.base_url + "/v1/chat/completions"` |
| Auth | `x-api-key: <key>` + `anthropic-version: 2023-06-01` | `Authorization: Bearer <key>` |
| System prompt | top-level `system` field | first `role: "system"` message |
| Tools | `[ToolDef]` (Anthropic) | `[{type: "function", function: {name, description, parameters}}]` |
| Tool call | `tool_use` block in `content[]` | `tool_calls[]` array of `{index, id, function: {name, arguments: "<json-string>"}}` |
| Tool result | `role: "user"` + `tool_result` block | independent `role: "tool"` message + `tool_call_id` |
| Text delta | `content_block_delta.text_delta` | `choices[0].delta.content` |
| Reasoning | `thinking_delta` block (Anthropic SSE) | `choices[0].delta.reasoning_content` (OpenAI o1/o3) |
| Finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
| Error body | `{"error": {"type": "<class>", "message": "..."}}` | `{"error": {"code": "<class>", "message": "..."}}` |
| Stream format | `event: ...\ndata: {...}\n\n` (typed) | `data: {...}\n\n` (data-only) |

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
ChatRequest  --(chat_request_to_wire)-->  WireRequest
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
            POST {base}/v1/chat/completions
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
ChatRequest  --(chat_request_to_wire)-->  WireRequest
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
          chat_stream_with_tools(req)  // unchanged SSE parser
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
(5xx → Server, 4xx → InvalidRequest). Net effect: same 5
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

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `OpenAIProvider::send` on an empty `messages` | Body has `messages: []` (valid) |
| `messages` contains a `Thinking` block, target is OpenAI | Strip keeps as `[reasoning] <text>` in content (cross-protocol history marker) |
| `messages` contains a `Signature` block, target is OpenAI | Strip drops the block entirely |
| `messages` contains a `RedactedThinking` block, target is OpenAI | Strip drops the block |
| OpenAI 401 with `error.code = "invalid_api_key"` | `LlmError::Auth` |
| OpenAI 429 with `error.code = "rate_limit_exceeded"` | `LlmError::RateLimit` |
| OpenAI 400 with `error.code = "invalid_request_error"` | `LlmError::InvalidRequest` |
| OpenAI 5xx | `LlmError::Server` |
| `delta.tool_calls[i]` with no `function.name` (defensive) | Buffer accumulates; emit-time check returns `None` and skips the broken tool call with a `tracing::warn!` |
| `delta.tool_calls[i].function.arguments` is partial JSON | Buffer accumulates; emit-time parse falls back to `{}` on parse error |
| `data: [DONE]` arrives with in-flight tool_call buffers | Defensive: emit any unfinished tool calls before terminating the stream |
| `AnthropicProvider::send` with `signature` block in history | Strip keeps it; the inverse `wire_messages_to_chat_messages` reconstitutes the `Thinking` block with the signature intact (no Anthropic round-trip regression) |

### 5. Good / Base / Bad Cases

#### Good: OpenAI gpt-4o happy path

1. User opens Settings, adds an OpenAI provider with their
   `sk-...` key, adds a `gpt-4o` model under it, sets it as
   the default.
2. User opens a session, types a question, clicks Send.
3. `resolve_chat_provider` resolves the gpt-4o
   `ModelWithProvider` and the OpenAI provider row.
4. `build_provider` constructs an `OpenAIProvider` with
   `max_tokens = 16384`, `reasoning_effort = None`.
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

#### Base: OpenAI 401 with new-style error body

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

### 6. Tests Required

#### Wire layer (`wire::tests`)

| Test | Asserts |
|------|---------|
| `caps_anthropic_with_thinking_signatures_supported` | All 3 caps true for Anthropic + thinking-effort-set model |
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
| `endpoint_trims_trailing_slash` | `base_url = "https://x.com/"` → endpoint has no double slash |
| `endpoint_uses_provided_base_url` | Custom proxy base URL works |
| `openai_provider_reports_openai_capabilities_and_protocol` | `protocol() == Openai`, all 3 caps true |
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
| `openai_500_classified_as_server` | OpenAI 5xx → `LlmError::Server` |
| `build_tool_call_event_parses_accumulated_arguments_json` | Tool-call buffer with complete JSON → `ChatEvent::ToolCall` with parsed `input` |
| `build_tool_call_event_handles_partial_arguments` | Concatenated fragments → valid JSON parsed at emit time |
| `build_tool_call_event_returns_none_without_name` | Defensive: missing `function.name` → drop |
| `build_tool_call_event_empty_args_buf_yields_empty_object` | Defensive: no arguments → `{}` |
| `wire_block_to_chat_event_text_path` | (sanity, same as wire test) |
| `wire_block_to_chat_event_reasoning_path` | (sanity, same as wire test) |

#### Factory / dispatch (`provider::tests` — 1 changed, 1 new)

- `build_provider_openai_returns_openai_provider` (CHANGED from
  `build_provider_openai_returns_not_implemented`): PR2's
  stub is now a real dispatch. Same test name with new
  assertion shape.

PR3 net new tests: 16 wire + 14 OpenAI = **30+ new tests**.
Pre-PR3 baseline: 218. Post-PR3 target: **248+ tests** (the
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
  2. Open Settings, see the 2 seeded providers + 4
     seeded models (2 Anthropic, 2 OpenAI per the PR1
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

### 7. Wrong vs Correct

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

OpenAI 401 returns
`{"error": {"code": "invalid_api_key", "message": "..."}}`.
`error.type` is a literal `"error"`. The keyword match
fails. Status is 401 (4xx) → falls through to
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
db::update_messages(session_id, &stripped).await?;  // ❌
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

#### Decision: Anthropic also goes through the wire layer (D1, locked 2026-06-09)

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
- ⚠️ The 4 PR2 `anthropic::tests::*` tests still pass
  verbatim because the wire round-trip preserves the
  same field set on the Anthropic→Anthropic path.

#### Decision: In-memory strip, no persistence (D2, locked 2026-06-09)

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
| `ProviderCapabilities`-gated dispatch | All 3 protocols (Anthropic, OpenAI, future Gemini) support system + tools + streaming. Capability gating is a no-op until a protocol diverges. |
| Provider-level API key redaction (so `api_key` is never logged even in `tracing::debug!`) | The `info!` logs in `chat_stream_with_tools` and `OpenAIProvider::send` don't include the key today; explicit redaction is a defensive layer PR4+ should add. |
| OpenAI `max_completion_tokens` (o1+ specific) | The OpenAI API uses `max_tokens` for non-o1 models and `max_completion_tokens` for o1+. PR3 uses `max_tokens`; future PR may branch on `model_name` to pick the right field. |
| OpenAI `parallel_tool_calls: true` | PR3 emits multiple `tool_calls` from a single assistant turn (matches the OpenAI streaming semantics), but the request body doesn't set `parallel_tool_calls: true` explicitly. The default is `true` on most models, so this is a no-op today. |
| Gemini / Ollama adapters | Each will plug in via the wire layer — write a new `provider::gemini` module, implement `Provider`, register in `build_provider`. The wire layer is the cross-protocol contract that doesn't need to change. |
