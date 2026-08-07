## Scenario: Tool Set Extension (4 new tools + ReadGuard + Bash spillover)

###1. Scope / Trigger

- Trigger: Step2 had only `read_file` / `write_file` / `shell` (3 tools). The coding
 agent UX was thin — every edit forced LLM to read whole file then `write_file` the
 whole thing back; every code search forced `shell` + `grep` shell-out wasting tokens.
- Round1 (2026-06-07) extended the toolset to7: adds `edit_file` /
 `grep` / `glob` / `list_dir`, plus a `ReadGuard` Tauri State that enforces
 read-before-edit + on-disk freshness before any edit is allowed to land.
- Why code-spec depth: mandatory — `edit_file` errors must be plain-English for LLM
 self-correction; `ReadGuard` is a cross-cutting concern (every edit goes through it);
 the toolset is now visible in the request body's `tools: Vec<ToolDef>` array and the
 schema must stay aligned with Anthropic's tool-use contract.

###2. Signatures

#### Tool declarations (added to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/edit_file.rs
ToolDef {
 name: "edit_file",
 description: "Apply a surgical edit to an existing file. The file must have been \
 read with read_file in the current session, and must not have been modified on disk \
 since. Replaces `old_string` with `new_string`.\n\n`old_string` must match exactly \
 (whitespace-sensitive). If it appears zero times the tool returns an error with hint \
 lines; if it appears more than once and `replace_all` is not set, the tool returns \
 the line numbers of all matches and asks for more context.\n\nPaths may be relative \
 (resolved against the session cwd) or absolute; the resolved path must be inside \
 the active project root.",
 input_schema: json!({
 "type": "object",
 "properties": {
 "path": { "type": "string" },
 "old_string": { "type": "string" },
 "new_string": { "type": "string" },
 "replace_all": { "type": "boolean" }
 },
 "required": ["path", "old_string", "new_string"]
 })
}

// app/src-tauri/src/tools/grep.rs
ToolDef { name: "grep", input_schema: { pattern, path?, glob?, output_mode?,
 case_insensitive?, show_line_numbers?, context?, head_limit? } }
//3 output_modes map to: "files_with_matches" (rg -l) |
// "content" (rg -n) |
// "count" (rg -c)
// Default: "files_with_matches". .gitignore is honored by default.

// app/src-tauri/src/tools/glob.rs
ToolDef { name: "glob", input_schema: { pattern, path? } }
// Cap100 results, sorted by mtime desc. .gitignore NOT honored (intentional).

// app/src-tauri/src/tools/list_dir.rs
ToolDef { name: "list_dir", input_schema: { path, show_hidden?, limit? } }
// Non-recursive. Default limit500. Directories suffixed with "/". Hidden files
// hidden by default; use show_hidden: true to opt in.

// app/src-tauri/src/tools/read_guard.rs
pub struct Fingerprint {
 // None when the OS/filesystem does not support mtime (e.g. some FAT
 // filesystems); in that case freshness falls back to size + head_hash
 // alone. Code: `app/src-tauri/src/tools/read_guard.rs:43`.
 pub mtime: Option<SystemTime>,
 pub size: u64,
 pub content_hash_head: u64, // xxh64 of first8 KiB
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
 line,1-based). The numbering survives the50 KiB head+tail truncation.
- `shell` output >30 KiB is **spilled to disk** at
 `<session_cwd>/.everlasting/outputs/<uuid>.txt` and the tool result returns
 `<path>` + a1 KiB head+tail preview. The spill directory is **pruned on
 `delete_session`** (best-effort, never blocks the delete).

#### P0 enhancement (2026-06-12): read_file offset/limit + shell timeout

- `read_file` gains `offset` (int, 1-indexed, default 1) and `limit` (int, default 2000)
  parameters. When provided, only lines `[offset, offset+limit)` are returned. Line
  numbers in the output start from `offset` (not 1), so the LLM can reference real file
  line numbers in `edit_file`. The ReadGuard fingerprint still covers the full file
  (offset/limit only affect the output slice). `offset=0` is treated as 1 (defensive).
  `offset` beyond EOF returns empty output (is_error: false).
- `shell` gains `timeout` (int, milliseconds, default 120000, max 600000). On timeout,
  the child process is killed, partial output is returned with a `[timeout after Nms,
  partial output]` marker, and is_error: true. `timeout <= 0` uses the default (120s).
  `timeout > 600000` is clamped to 600000. Timeout is distinct from C1 CancellationToken
  cancel: timeout is automatic (time-based), cancel is manual (user-triggered). Both
  kill the child and return partial output, but with different markers.

###3. Contracts

#### Request

The4 new tools are appended to `ChatRequest.tools`; nothing else changes in the
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
as the existing3 tools.

**P3 footnote injection (2026-06-29, 06-29-am-p3-tool-recall)**: the
`content` field of `ContentBlock::ToolResult` may be prepended with a plain-text
recall footnote (e.g. `"⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n• ..."`)
when a matching `active`-status pitfall is found in `autonomous_memories`. This
is **plain text inside `content`**, NOT a new content block. The `tool_use_id`
pairing, `is_error` semantics, and envelope wrap (`{result, cwd}`) are all
preserved. The frontend's lenient parser (`utils/messageFormat.ts
extractToolResultDisplay`) returns the `result` string verbatim, so the
footnote renders to the user as part of the tool output. See
[memory.md §Scenario 2 / Pre-tool pitfall recall contract](./memory.md#pre-tool-pitfall-recall-contract-p3-layer-2-of-2--2026-06-29-06-29-am-p3-tool-recall)
for the recall contract.

**P4 is_error consumption (2026-06-29, 06-29-am-p4-event-reflect)**: the
`is_error` field of `ToolResultPayload` is **read** by the P4
`FailureTracker` at the chat_loop post-execute seam to drive the
"consecutive ≥2 failures → success" event signal. P4 is a
**read-only consumer** of `ToolResultPayload` — it does not modify
`content` / `is_error` / `tool_use_id` / envelope shape. The P4
reflection (LLM call + `insert_memory` write) is fire-and-forget
via `tokio::spawn` and never bubbles back to the tool result. P4 is
the **write-side counterpart** of P3's read-side footnote injection:
P3 prepends a recall hint before tool execution; P4 records a new
pitfall after tool execution. They close the loop (P4 writes → P3
recalls → agent avoids the pitfall next time). See
[memory.md §Scenario 2 / Event-driven bypass reflection contract (P4)](./memory.md#event-driven-bypass-reflection-contract-p4-write-side-of-the-loop--2026-06-29-06-29-am-p4-event-reflect)
for the write contract.

#### Environment keys

No new env keys. The4 new tools have no configurable knobs (the cap100 /30 KiB
/500 limits are hard-coded constants in their respective modules, to be lifted
to env if personalization becomes a need).

#### Tauri State

`AppState.read_guard: ReadGuard` is managed by `app::setup` and held alongside
the existing `AppState.db`. `lib.rs::chat` injects `(state.read_guard,
session_id)` into every `execute_tool` call.

###4. Validation & Error Matrix

| Condition | Tool | Result |
|-----------|------|--------|
| File not read in this session | `edit_file` | `"You must read_file '<path>' first."` (is_error: true) |
| File read, but mtime/size changed on disk | `edit_file` | `"File <path> has changed on disk since you last read it. Re-read it first."` |
| `old_string` not found in file | `edit_file` | `"old_string not found in <path>. Closest match (line N): '<...>'."` (0-3 hints, Jaccard-sorted) |
| `old_string` matches N>1 times, `replace_all=false` | `edit_file` | `"old_string appears N times in <path>. Add more context or pass replace_all: true."` (lists all line numbers) |
| `old_string == new_string` | `edit_file` | `"old_string and new_string are identical. No-op edit rejected."` |
| `old_string == ""` | `edit_file` | `"old_string must not be empty."` |
| `replace_all: true` | `edit_file` | All occurrences replaced; tool_result includes the count. |
| `ripgrep` exit1 (no matches) | `grep` | `"No matches found for pattern <pattern> in <path>."` (is_error: false — not really an error) |
| `ripgrep` exit2 (rg error) | `grep` | stderr text passed to LLM (is_error: true) |
| `pattern` matches but output >30 KiB | `shell` | Spilled to `<cwd>/.everlasting/outputs/<uuid>.txt`; result = path +1 KiB preview |
| `delete_session` for a session whose cwd is gone | — | Cleanup is best-effort, no cascade. User's delete intent wins. |
| `ReadGuard.clear_session` on a missing session | — | Silent no-op (HashMap::remove returns None) |
| `read_file` of binary file | `read_file` | `read_to_string` fails → is_error: true, "Failed to read file" (existing behavior, unchanged) |
| `offset` beyond file length | `read_file` | Empty output (is_error: false) |
| `offset=0` | `read_file` | Treated as offset=1 (defensive, reads from start) |
| `limit` extends past EOF | `read_file` | Returns up to EOF, no error |
| `timeout` expires during execution | `shell` | Child killed, partial output + `[timeout after Nms, partial output]` (is_error: true) |
| `timeout <= 0` | `shell` | Uses default 120000ms |
| `timeout > 600000` | `shell` | Clamped to 600000ms |
| User clicks 跳过 (skip) | `ask_user_question` | `{"cancelled": true, "reason": "user_skipped", "hint": "..."}` (**is_error: false** — a deliberate user skip is not an error; the agent should continue / pick another approach). Mirrors the "ripgrep exit1 = not really an error" philosophy: is_error reflects whether the tool *failed*, not whether the user declined. |
| Session-level cancel (Stop / app shutdown) | `ask_user_question` | `{"cancelled_by_session": true}` (**is_error: true**) — distinct from user-skip. The `cancel.cancelled()` arm and the `RecvError` arm both emit this marker and keep is_error true. |
| Already a pending question for this session | `ask_user_question` | `"已有 pending question,等当前回答完成"` (**is_error: true**, short-circuit before the blocking wait) |
| Schema validation (empty/too-many questions, options out of 2..=4, header >12 chars) | `ask_user_question` | Structured error string (**is_error: true**, short-circuit; the LLM self-corrects next turn) |


#### P0 enhancement (2026-06-14): shell env_clear + safe allowlist (RULE-E-001)

- `shell` no longer inherits the agent's full environment. Before spawn the tool
  calls `apply_safe_env(cmd)` which does `cmd.env_clear()` and re-injects only
  a curated allowlist: `PATH` (from parent) + `HOME` / `USER` / `LOGNAME` /
  `LANG` / `LANGUAGE` / `LC_ALL` / `TERM` / `TZ` / `TMPDIR` (each re-injected
  only when present in the parent).
- The allowlist is hard-coded as `SAFE_ENV_VARS: &[&str]` in `tools/shell.rs`
  and is **forbidden** from containing any `*_API_KEY` / `*_TOKEN` /
  `*_SECRET` (structural guard test `apply_safe_env_clears_and_reinjects`
  asserts the negative set).
- The change closes the leak where an LLM `env` / `printenv` could read
  `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `*_TOKEN` from the parent. The
  Permission system (Tier 4 ask) gates whether `shell` should execute; this
  layer is the *execution-context* hardening that prevents the child from
  leaking credentials back to the LLM after execution is approved.
- Behavior tests: `execute_env_does_not_leak_api_key` (Anthropic key),
  `execute_env_does_not_leak_openai_key` (OpenAI key), `execute_preserves_path`
  (PATH inherited), `execute_optional_env_vars_do_not_error` (allowlist
  var count ≤ 20 to catch leaks). All 31 `tools::shell` tests pass.
- `shell` tool description string now ends with: "Environment is restricted
  to a safe allowlist (PATH/HOME/USER/LOGNAME/LANG/LANGUAGE/LC_ALL/TERM/TZ/
  TMPDIR). API keys and tokens from the agent process are NOT inherited."
- Out of scope (MVP): user-configurable allowlist, Windows-specific
  `env_clear` quirks, leaking the project root / session id as
  `EVERLASTING_*` env vars.

###5. Good / Base / Bad Cases

#### Good: read → edit → done

1. LLM calls `read_file` on `src/foo.rs`. `read_file` records the fingerprint in
 `ReadGuard` keyed by the active `session_id`.
2. LLM calls `edit_file` with `old_string` that matches exactly once.
3. `edit_file::execute` calls `ReadGuard::verify_read` (passes — just read) →
 `verify_fresh` (passes — mtime/size unchanged) → `match` (passes — exact1
 occurrence, no need to check uniqueness separately when1) → write to disk.
4. On success, `edit_file::execute` calls `ReadGuard::invalidate(sid, path)` so
 the next edit forces a re-read.
5. Tool result: `Successfully edited '<path>'.` (single occurrence) or
   `Successfully edited '<path>': replaced N occurrences.` (when
   `replace_all: true`, N ≥ 1). The single-occurrence form deliberately
   omits the count — frontends and tests should match the literal prefix
   `Successfully edited` and inspect the trailing colon + `replaced N`
   only when present. Code: `app/src-tauri/src/tools/edit_file.rs:224-232`.

#### Base:0 matches with hint

1. `old_string` is slightly wrong (extra trailing whitespace, one char off).
2. `edit_file` returns the failure message + the0-3 most similar lines (Jaccard
 similarity on4-gram token sets). LLM reads the hint, adjusts `old_string`,
 retries on the next turn.

#### Bad: edit without read

1. LLM is given a fresh session and immediately calls `edit_file` without first
 calling `read_file`.
2. `verify_read` returns `Err`. The error message names the tool (`read_file`) the
 LLM must call first. The agent loop does NOT add this to a "previous error"
 list — the LLM can re-attempt with a corrected approach.
3. If LLM persists in calling `edit_file` without `read_file`, the same error
 message is returned each turn; the agent loop hits `max_turns =20` and gives
 up. (No silent retry, no state corruption — the file is never touched.)

#### Bad: bash output lost when >30 KiB

1. (Pre-extension behavior.) `shell` runs `cargo build`, output is80 KiB of
 warnings. `shell` truncates head+tail to50 KiB. The middle30 KiB is
 silently dropped. LLM sees a50 KiB blob and has to guess what was in the middle.
2. (Post-extension.) `shell` writes the80 KiB to
 `<cwd>/.everlasting/outputs/<uuid>.txt` and returns `"Output saved to <path>
 (80000 bytes). First1KB preview:\n<preview>"`. LLM can call `read_file` on
 the path to inspect any range with line numbers.

###6. Tests Required

The round1 PR added77 tool tests (14 +10 +8 +8 +8 +14 +15) +3 for
`cleanup_outputs_dir` =80 new tests; the following are mandatory for any future
change touching these areas.

#### Backend (`cargo test`)

| Test | Asserts |
|------|---------|
| `edit_file::edit_happy_path` | Read + edit + write back, success result, fingerprint invalidated. |
| `edit_file::edit_before_read` | No prior read → `verify_read` fails, no file written. |
| `edit_file::edit_after_external_modify` | Read, file changed externally, edit → `verify_fresh` fails. |
| `edit_file::edit_old_string_not_found` |0 matches → error includes `"Closest match"`. |
| `edit_file::edit_old_string_ambiguous` |2 matches, `replace_all=false` → error lists both line numbers. |
| `edit_file::edit_no_op` | `old_string == new_string` → rejected. |
| `edit_file::edit_with_replace_all` |3 matches + `replace_all: true` →3 occurrences replaced. |
| `read_guard::sessions_are_isolated` | s1's reads do not unlock s2's edits. |
| `read_guard::clear_session` | Removes all entries for a session; missing session is no-op. |
| `grep::output_mode_files_with_matches` | Maps to `rg -l` flag. |
| `grep::output_mode_content` | Maps to `rg -n` flag. |
| `grep::output_mode_count` | Maps to `rg -c` flag. |
| `grep::no_matches` | Exit1 from rg → "No matches found" message (not is_error). |
| `grep::line_cap_500` | Lines longer than500 chars are truncated with marker. |
| `glob::cap_100` | More than100 results → truncation hint in result. |
| `glob::mtime_desc` | Results sorted by mtime descending. |
| `list_dir::basic` | Returns entries alphabetically with `/` suffix on directories. |
| `list_dir::hidden_default_off` | `.git/` is hidden unless `show_hidden: true`. |
| `read_file::cat_n_simple` | Single-line file → `1\tcontent`. |
| `read_file::cat_n_truncation_preserves_line_numbers` |100 KiB file → first25 lines + `<truncated>` + last25 lines, both with correct line numbers. |
| `read_file::cat_n_empty_lines_numbered` | File with blank lines → numbering is continuous (1,2,3...) not skipped. |
| `shell::large_output_spills_to_disk` | >30 KiB → file exists in `<cwd>/.everlasting/outputs/`, tool result contains path. |
| `shell::small_output_unchanged` | <30 KiB → no spill file, tool result is the raw output. |
| `shell::cleanup_outputs_dir` | After `cleanup_outputs_dir(cwd)`, the spill dir is gone; nested files are also removed; missing dir is no-op. |
| `shell::execute_env_does_not_leak_api_key` | (RULE-E-001) `ANTHROPIC_API_KEY` set in parent is NOT visible to the child via `printenv`. |
| `shell::execute_env_does_not_leak_openai_key` | (RULE-E-001) `OPENAI_API_KEY` set in parent is NOT visible to the child via `printenv`. |
| `shell::execute_preserves_path` | (RULE-E-001) `PATH` is inherited from parent so commands like `sh` resolve. |
| `shell::execute_optional_env_vars_do_not_error` | (RULE-E-001) Child env has ≤ 20 vars; an unexpected count means a leak. |
| `shell::apply_safe_env_clears_and_reinjects` | (RULE-E-001) `SAFE_ENV_VARS` does not contain any `*_API_KEY` / `*_TOKEN` / `*_SECRET` (structural guard). |

Total backend suite: 461+ tests pass as of 2026-06-14 (was 437 pre-RULE-E-001; +5 net
new from the shell env_clear work — 4 behavioral + 1 structural).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. Frontend sees the4 new tool
 descriptions via the existing `chat-event` / `tool-call` protocol; **no
 frontend code changes** are required for the new tools because the existing
 `SessionList` tool card is generic.
- Manual smoke test: `cd app && pnpm tauri dev`, call `read_file` on a100 KiB
 file, observe `cat -n` line numbers. Call `shell` with a long-output
 command, observe spill path in the result. Call `edit_file` after `read_file`,
 observe success; without `read_file`, observe the plain-English error.

###7. Wrong vs Correct

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
it between LLM's read and edit).1 in5 edits silently corrupts the file.

#### Correct: ReadGuard enforces read-before-edit + on-disk freshness

```rust
// GOOD — three mandatory checks
pub async fn execute(
 input: &Value, ctx: &ToolContext,
 guard: &ReadGuard, session_id: &str,
) -> (String, bool) {
 let validated = assert_within_root(...)?;

//1. Read-before-edit
 if let Err(e) = guard.verify_read(session_id, &validated).await {
 return (e, true);
 }
//2. On-disk freshness
 if let Err(e) = guard.verify_fresh(session_id, &validated).await {
 return (e, true);
 }

 let content = tokio::fs::read_to_string(&validated).await?;
 //3. Match + uniqueness
 let occurrences = content.matches(&old).count();
 if occurrences ==0 {
 let hint = find_similar_lines(&content, &old);
 return (format!("old_string not found in <{}>. Closest match: {}", path, hint), true);
 }
 if occurrences >1 && !replace_all {
 return (format!("old_string appears {} times in <{}>. Add context or pass replace_all: true.", occurrences, path), true);
 }

 let new_content = if replace_all {
 content.replace(&old, &new)
 } else {
 content.replacen(&old, &new,1)
 };
 tokio::fs::write(&validated, &new_content).await?;
 guard.invalidate(session_id, &validated).await;
 (format!("Successfully edited <{}>: {} occurrence(s) replaced.", path, occurrences), false)
}
```

#### Wrong: shell silently truncates >30 KiB output

```rust
// BAD — head+tail truncation, middle silently dropped
fn truncate(s: String) -> String {
 if s.len() <=30_000 { return s; }
 let head = &s[..25_000];
 let tail = &s[s.len()-5_000..];
 format!("{}\n<truncated {} bytes>\n{}", head, s.len()-30_000, tail)
}
```

LLM misses errors / warnings in the middle30 KiB. Long `cargo build` outputs
are exactly this case.

#### Correct: shell spills to disk, LLM can re-read with line numbers

```rust
// GOOD — claude-code style spillover
const DISK_SPILL_THRESHOLD: usize =30 *1024;
const PREVIEW_BYTES: usize =1024;

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
