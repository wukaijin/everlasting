# Tool Contract —工具定义 + ReadGuard + Bash Spillover

> **基线**:2026-06-13(PR1 + PR3 of `06-12-a2-b7-permission-and-mode`)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + Extended Thinking + 反模式汇总 + ⑨ 关 IPC 协议
> - [tool-contract.md](./tool-contract.md) (本文) —工具定义 + ReadGuard + shell spillover
> - [permission-layer.md](./permission-layer.md) —⑨ 关 Permission Layer 设计合约(A2 + B7 canonical,2026-06-13)
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `builtin_tools()` / `edit_file` / `ReadGuard` / `shell` spillover / `grep` / `glob` / `list_dir` 时。
>
> **⑨ 关 Permission Layer 设计合约**:[permission-layer.md](./permission-layer.md)(A2 + B7, 2026-06-13,2026-06-21 移入)。

---

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

## Scenario: web_fetch tool (P1, 2026-06-12)

###1. Scope / Trigger

- Trigger: step 7 of the tool-extension roadmap — agent 自主抓取外部文档/API
参考/错误信息。With 7 built-in tools the LLM can only act on local files and
ripgrep; every external reference had to be copy-pasted by the user. `web_fetch`
adds a 8th tool that fetches arbitrary HTTP/HTTPS URLs and returns the content
as markdown (default) for the LLM to consume.
- Why code-spec depth: mandatory — `web_fetch` is a network egress tool (the
first one in this project) and the SSRF threat model is materially different
from a pure-filesystem tool. The IP-block list, attribution prefix, and
body-size cap are all security/longevity decisions, not nice-to-haves.
- Subagent availability (2026-06-25, task `06-25-subagent-web-access`):
  `web_fetch` is in both the `researcher` `SubagentDef.tools` allowlist
  and the concurrent-worker `READONLY_TOOL_ALLOWLIST`, so `researcher`
  workers AND concurrent `general-purpose` workers can fetch. A worker's
  `web_fetch` hits the same Tier 4 check (`check.rs` `WebFetch` branch):
  the worker's `PermissionContext.session_id` is the **parent** session
  id, so `check_tool_grant` inherits the parent session's `web_fetch`
  grant → auto-Allow (zero banner); no grant → surfaces a
  `WorkerAskBanner` (worker `AllowAlways` does NOT persist — see
  permission-layer.md §5b).

###2. Signatures

#### Tool declaration (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/web_fetch.rs
ToolDef {
 name: "web_fetch",
 description: "Fetches content from a URL and returns it as markdown (default), plain \
 text, or raw HTML. Use this to read external documentation, API references, error \
 messages, or any web page. Read-only; does not modify files. Supports HTTP and HTTPS \
 only.\n\nSecurity: by design, this tool refuses to fetch private, loopback, or \
 link-local addresses (e.g. 127.0.0.1, 192.168.x.x, 169.254.169.254) to prevent the \
 agent from being used as an SSRF proxy.\n\nResults may be truncated if very large; \
 use `format: \"text\"` for a smaller payload, or `format: \"html\"` to get the raw \
 response.",
 input_schema: json!({
   "type": "object",
   "properties": {
     "url":      { "type": "string" },
     "format":   { "type": "string", "enum": ["markdown", "text", "html"],
                    "default": "markdown" },
     "timeout":  { "type": "integer" }
   },
   "required": ["url"]
 })
}
```

The tool runs in the Rust backend and is **provider-agnostic** — it does not
go through `wire.rs` or the Anthropic / OpenAI adapter, so it works for every
LLM provider unchanged.

#### Execution pipeline

1. Parse URL with `reqwest::Url`; reject anything that isn't `http` / `https`
   → `InvalidUrl`.
2. DNS-resolve host via `tokio::net::lookup_host`; check every resolved IP
   against the hard-coded blocklist (RFC 1918 / loopback / link-local /
   CGNAT / multicast / reserved + 169.254.169.254 short-circuit). If every
   IP is blocked → `BlockedAddress(<first_ip>)`.
3. Pin the validated public IP on the reqwest `ClientBuilder::resolve()`
   (closes the small DNS-rebinding window between our check and reqwest's
   connect).
4. Build a reqwest client: `timeout = <user_timeout, default 30s, max 120s>`,
   `connect_timeout = 10s`, `redirect = Policy::limited(5)`, strict TLS
   (rustls default), `User-Agent: Everlasting/<version>`, `Accept:
   text/markdown;q=1,text/html;q=0.9,...`.
5. `tokio::time::timeout(timeout + 5s grace, request.send())` — outer wrapper
   so a body read landing right at the reqwest limit still surfaces as
   `Timeout`, not generic `Network`.
6. Non-2xx → `HttpStatus(<code>)`. 2xx → continue.
7. Read body (`.bytes()`); if > 5 MiB → `TooLarge`.
8. Convert per `format`:
   - `markdown` (default) on `text/html` → `htmd::HtmlToMarkdown` (skips
     script/style/noscript/nav/footer/header/aside).
   - `text` on `text/html` → single-pass tag strip + entity decode.
   - `markdown` / `text` on `application/json` → `serde_json::to_string_pretty`.
   - `html` on anything → raw body.
9. Head/tail truncation at 50 KB + 50 KB (with `<truncated: omitted N bytes>`
   marker) when the converted content exceeds 100 KB.
10. Prepend attribution prefix:
    `<!-- fetched: <url> at <RFC3339> · status <code> · <bytes> bytes ·
    content-type <ct> -->\n\n` — cheap T1a prompt-injection mitigation
    (lets the LLM attribute the content to a specific fetch).

#### 7 error variants (mapped to `is_error: true` strings)

| Variant | When | LLM-facing string |
|---------|------|-------------------|
| `InvalidUrl(scheme)` | non-http(s) scheme or unparseable URL | `URL must be http or https (got: <scheme>)` |
| `BlockedAddress(ip)` | all resolved IPs are private | `refusing to fetch private/loopback/link-local address (URL resolves to <ip>)` |
| `TooLarge` | body > 5 MiB | `response body exceeds 5 MiB cap` |
| `HttpStatus(code)` | non-2xx | `HTTP <code>` |
| `Timeout(secs)` | reqwest timeout or outer wrapper fired | `request timed out after <secs>s` |
| `Tls(msg)` | TLS handshake error | `TLS error: <msg>` |
| `Network(msg)` | DNS, TCP, generic reqwest errors | `network error: <msg>` |

#### Out of scope (MVP)

- POST / PUT / DELETE (GET only)
- `web_search` (separate P2 task)
- `prompt` extraction param (no second small model)
- JavaScript rendering (Playwright/CDP)
- Image / PDF / binary content
- Caching
- Cookies / session management
- Domain permissions gate (Claude Code-style first-time per host)
- DNS rebinding defense beyond a single-shot resolve + IP pin
- Configurable IP blocklist (hard-coded for MVP)

###3. Cancellation

The outer `execute_tool` wrapper in `tools/mod.rs` already wraps this future
in `tokio::select! { biased; cancel | future }`, so a user Stop aborts an
in-flight request by dropping the future. The inner `tokio::time::timeout`
is a separate time-based kill, distinct from the user-triggered C1 cancel.

###4. Tests

25 tests in `tools/web_fetch::tests`:

- 8 IP-block unit tests (loopback / RFC 1918 / link-local incl. 169.254.169.254
  / CGNAT / multicast / public allow / v6 loopback + link-local / v4-mapped
  v6 unwrap)
- 1 test-bypass test (allow_private)
- 2 format-parse tests
- 2 HTML-helper tests (text strip + entity decode, whitespace collapse)
- 2 truncate tests (passthrough under 100 KB, head/tail cap over 200 KB)
- 2 schema tests (name + required, SSRF described in description)
- 1 missing-param test
- 7 integration tests via `httpmock`:
  - HTML→markdown happy path (verifies attribution prefix is prepended)
  - text format (tag strip + entity decode)
  - html format (raw body after prefix)
  - 404 → `HttpStatus(404)`
  - 500 → `HttpStatus(500)`
  - `file://` scheme → `InvalidUrl`
  - unparseable URL → `InvalidUrl`
- 1 production-entry safety test (`execute` still blocks 127.0.0.1)

Test isolation: the `#[cfg(test)] execute_for_test` entry takes a private
`allow_private: bool` so httpmock (bound to 127.0.0.1) can be used without
polluting the production SSRF block via a global flag (which would race with
parallel tests). Production `execute` always passes `false`.

###5. Security notes (see `research/web-fetch-security.md`)

- **T2a-c (SSRF)** — High severity, MVP MUST block. Implementation: hard-coded
  IP blocklist in [`tools/web_fetch.rs`](../../../app/src-tauri/src/tools/web_fetch.rs)
  §`is_blocked`. Cloud-metadata short-circuit for 169.254.169.254.
- **T2e (DNS rebinding)** — Med severity, MVP accepted risk. Single-shot
  `lookup_host` + `ClientBuilder::resolve(domain, ip)` closes most of the
  window. Full socket-level re-validate is a follow-up.
- **T2f (redirect to private IP)** — Med severity, **IMPLEMENTED** via
  [`build_redirect_policy()`](../../../app/src-tauri/src/tools/web_fetch.rs)
  (RULE-E-003, 2026-06-14). A custom `redirect::Policy::custom` callback
  re-runs `resolve_and_check_sync` + `is_blocked` on every redirect hop and
  returns `Action::Stop` if the target IP is in the blocklist. The
  `allow_private` bypass is hard-coded to `false` in the redirect callback
  (it exists only for the initial URL path, to let integration tests talk
  to a `httpmock` server on 127.0.0.1). Distinct [`WebFetchError::RedirectBlocked`]
  variant + dedicated tests (`redirect_to_rfc1918_is_refused`,
  `redirect_to_cloud_metadata_is_refused`). Without this guard, an
  LLM-driven local agent would be an effective network scanner
  (`attacker.com → 169.254.169.254` leaks cloud metadata).
- **T3a (huge body)** — Hard 5 MiB cap → `TooLarge`.
- **T3b (slow-loris)** — `connect_timeout(10s)` + `timeout(30s, max 120s)` +
  `tokio::time::timeout(secs+5, ...)` + outer `CancellationToken`.
- **T4 (TLS)** — `reqwest` strict TLS (rustls default), no override toggle.
- **T1a (prompt injection)** — Attribution prefix lets the LLM attribute
  fetched content; the prefix is HTML-comment-shaped so the markdown converter
  downstream would strip it.
- **T6 (audit)** — `tracing::info!(url, final_url, status, bytes, duration_ms)`,
  no body in logs.

---

## Scenario: update_checklist tool (B12, 2026-06-19)

### 1. Scope / Trigger

- Trigger: B12 Checklist — the agent's self-tracking progress list. The model calls
  `update_checklist(items)` to atomically replace a per-request checklist held in the
  agent loop's run scope; the loop re-injects the current list into each turn so the
  model doesn't lose its plan. Aligns with Claude Code `TaskCreate/TaskList` / opencode
  `todowrite`. Paves the "inject dynamic agent-state each turn" mechanism that B6
  Subagent will reuse.
- Why code-spec depth: mandatory — this is the first tool whose state is **agent-managed
  and re-injected each turn** (not a plain request/response tool), and the injection
  placement carries a load-bearing prompt-cache invariant. The tool signature, the coerce
  contract, and the append-not-prepend injection rule are all executable contracts.

### 2. Signatures

#### Tool declaration (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/update_checklist.rs
ToolDef {
  name: "update_checklist",
  description: "Update your running progress checklist ... Pass the FULL list every
                call — the new list replaces the old one atomically (not append) ...
                At most one item should be in_progress at a time; if you pass multiple,
                only the last is kept as in_progress and the rest are demoted to pending ...",
  input_schema: json!({
    "type": "object",
    "properties": {
      "items": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "content": { "type": "string" },
            "status":  { "type": "string", "enum": ["pending", "in_progress", "done"] }
          },
          "required": ["content", "status"]
        }
      }
    },
    "required": ["items"]
  })
}
```

#### Types + handle

```rust
#[serde(rename_all = "snake_case")]
pub enum ChecklistStatus { Pending, InProgress, Done }   // wire: "pending"/"in_progress"/"done"
pub struct ChecklistItem { pub content: String, pub status: ChecklistStatus }

/// Per-request handle. NOT a run_chat_loop parameter — it lives inside ToolContext
/// (built once per run_chat_loop call ~chat_loop.rs:216), so run_chat_loop's 23-param
/// signature is UNCHANGED by the checklist addition (no agent_loop_* test call-site
/// sync needed; the B6 cluster + the 2026-06-21 B6 review defect A fix raised it from
/// 21→23 for `app_handle` + `system_prompt_override` respectively).
pub type ChecklistHandle = Arc<Mutex<Vec<ChecklistItem>>>;
pub fn new_handle() -> ChecklistHandle;                  // fresh empty, once per run
```

#### Execute (mutates the handle, returns tool_result)

```rust
pub async fn execute(input: &serde_json::Value, handle: &ChecklistHandle)
    -> (String, /* is_error */ bool);   // always (summary + rendered list, false)
```

`execute` parses → coerces → atomically replaces the Vec (`clear()` + `extend()`, lock
held only for the swap, no I/O in the critical section) → returns
`"Checklist updated (N items, M done, K in_progress).\n\n<rendered list>"`.

### 3. Contracts

- **Full replace** (not append). Replay requires "last `update_checklist` tool_result ==
  current state", so the input array replaces the Vec in full.
- **At-most-one `in_progress` coerce** (`coerce_at_most_one_in_progress`, pure fn): keep
  the LAST `in_progress` by array order (`rposition`), demote any earlier to `pending`.
  Does NOT error, does NOT abort the loop.
- **`execute` never returns `is_error: true`** — pure in-memory state mutation, no failure mode.
- **Plan mode**: auto-allowed. `permissions::filter_tools_for_mode` drops only `write_file` /
  `edit_file` / `shell`; `update_checklist` passes. `risk_for_tool` → `Risk::Low` (`_` default).
- **Lifetime**: per-request. `new_handle()` is constructed inside each `run_chat_loop`
  invocation; new user message → new run → new empty checklist. **No DB table, no
  migration** — replay reconstructs from `update_checklist` tool_results already in history.

#### Ephemeral per-turn injection (the load-bearing part)

In `agent/chat_loop.rs`, each turn AFTER `compact_messages` and BEFORE `provider.send`,
if the Vec is non-empty:

```rust
let mut req = messages.clone();              // request COPY — persisted messages UNTOUCHED
req.push(ChatMessage {                       // APPEND, not insert(0, …)  — see §7
    role: Role::User,
    content: MessageContent::Blocks(vec![ /* checklist + in-progress focus marker */ ]),
});
provider.send(req).await
```

- The block is appended to a **clone**; the persisted `messages` Vec never carries it.
- **No `cache_control`** on the block (it changes every turn → cache would never hit).
- Empty Vec (turn 1, before any update) → skip injection, symmetric to memory/skill empty-skip.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| Missing `items` key | Empty list (`is_error: false`) — same as `{"items": []}` |
| Empty `items: []` | Clears the list (`is_error: false`) |
| 2+ items `status: "in_progress"` | Coerced to exactly 1 (last kept, rest → `pending`); `is_error: false` |
| Unrecognized `status` string | Coerced to `pending`; `is_error: false` |
| Item missing `content` | That item skipped (rest pass); `is_error: false` |
| Tool cancelled mid-execution (RULE-A-004) | No `tool_executed` audit row; cancel path emits a synthetic `is_error: true` tool_result to preserve the tool_use/tool_result pairing invariant |

### 5. Good / Base / Bad Cases

- **Good**: model calls with `[{a, done}, {b, in_progress}, {c, pending}]` → Vec replaced;
  next turn the ephemeral block is appended to the request; card renders.
- **Base**: model passes two `in_progress` → coerced to one; summary reads `1 in_progress`;
  ephemeral block marks only the last as `<- in progress`.
- **Bad (anti-pattern the tool corrects)**: model calls intending to "add one item" but
  passes only the new item → list full-replaced to just that one (prior items lost). The
  tool is full-replace by design (replay contract); the model must always pass the FULL list.

### 6. Tests Required

Backend (`cargo test --lib`):

| Test | Asserts |
|---|---|
| `update_checklist::execute_full_replace_not_append` | 2nd call's list fully replaces (2 items), not append (5) |
| `update_checklist::execute_two_in_progress_coerces_to_one` | 2 in_progress → 1 (last kept); summary `1 in_progress` |
| `update_checklist::execute_empty_items_clears_list` / `_missing_items_key_treated_as_empty` | empty/missing → cleared, is_error false |
| `update_checklist::execute_unknown_status_coerced_to_pending` | `"blocked"` → pending |
| `update_checklist::coerce_keeps_last_in_progress_demotes_earlier` | rposition last, demote earlier |
| `agent_loop_update_checklist_replaces_vec_and_injects_next_turn` | tool_use → Vec replaced → tool_result full list → turn-2 request carries the ephemeral block; turn-1 does NOT; persisted messages never carry it |
| `agent_loop_update_checklist_coerces_two_in_progress_to_one` | ephemeral block marks only the last in_progress |
| `agent_loop_cancelled_update_checklist_skips_audit_row` | RULE-A-004: cancelled update → 0 `tool_executed` rows |

Frontend: `pnpm build` (vue-tsc strict) + `pnpm vitest run` (31 checklist store tests). The
TS `coerceAtMostOneInProgress` must stay a line-by-line mirror of the Rust
`coerce_at_most_one_in_progress` — a drift is a cross-layer bug (see
`frontend/state-management.md §"B12 Checklist store"`).

### 7. Wrong vs Correct — injection placement (prompt-cache invariant)

#### Wrong: prepend the checklist block to position 0

```rust
// BAD — insert(0, …) puts the per-turn-mutating block IN FRONT of the memory
// breakpoint at messages[0].
let mut req = messages.clone();
req.insert(0, checklist_msg);   // ← busts the memory cache key
provider.send(req).await
```

The memory instructions block at `messages[0]` carries a `cache_control: Ephemeral`
breakpoint (`memory/loader.rs::build_instructions_blocks`). Anthropic's cache key is the
byte-prefix up to the breakpoint; everything before it must be byte-identical across turns.
A per-turn-changing checklist block at position 0 sits in front of the breakpoint and busts
the memory cache every turn (50 turns × ~100 KiB instruction files = the exact cost
explosion B5 was built to eliminate — 1.0× input billing instead of 0.1× cache-read).

#### Correct: append the checklist block (after the breakpoint)

```rust
// GOOD — append keeps the checklist AFTER the memory breakpoint, so the memory
// cache window stays intact (same reason the B4 skill block sits at position 2,
// after the memory pair).
let mut req = messages.clone();
req.push(checklist_msg);        // ← after the breakpoint, cache preserved
provider.send(req).await
```

Anthropic accepts consecutive user-role messages, so appending a user block after the
user's latest prompt is wire-legal; recency (checklist is the last thing in context) is a
bonus. (This was caught by `trellis-check` on PR1 — the original plan said "prepend".)

---

## Scenario: dispatch_subagent tool (B6 PR1, 2026-06-19)

### 1. Scope / Trigger

- Trigger: main agent 在 turn N 通过 LLM `tool_use` 派一个 worker subagent 跑**独立 context**(独立 messages + 独立 token 预算 + 独立 turn 上限),完成后 worker final summary 回填为 dispatch_subagent 的 `tool_result`。对标 Claude Code Task tool / OpenHands TaskToolSet。
- **不是普通 I/O tool** —— 是 agent 层控制流工具。注册为 `ToolDef` 供 LLM 发现 + 走 ⑨ 关权限 check,但**执行不走** `execute_tool_inner`(拿不到 `provider` / `db` / `cancellations` 等依赖),在 `chat_loop.rs` tool_use 处理循环**拦截** → 直接调 `run_subagent(deps..., input, ctx)`(REVIEW-SUBAGENT-PRD #3 核实的真实约束)。
- ROADMAP §4.1 "B6 = Subagent(harness 学习价值高)" + ROADMAP §1.2 计划项。

### 2. Signatures

#### Tool declaration(`app/src-tauri/src/agent/subagent/mod.rs::definition()`)

> **L3d PR3 (2026-06-26) 动态化**:dispatch_subagent **不再**注册进 `builtin_tools()` 启动快照(`state.tools`,由 `AppState::load()` 启动调一次固化)。production 用 `definition_with_cache(cache: &SubagentCache, project_path: &str) -> ToolDef`(async),每 turn 在 `chat_loop.rs` turn tool list 构造处(`filter_tools_for_mode` 之后)调用 —— `enum` 从 `cache.list(project_path).await` 取所有 subagent 名(builtin + user + project 按优先级合并),`description` 末尾追加 `Available subagents: <name> (source: <builtin|user|project>): <desc>; ...`。.md 改动经 mtime fence 下次 chat 自动生效(无 reload 命令)。下面的静态 `definition()` 保留供单元测试;`tools/mod.rs::builtin_tools()` 现只含其他 12 个工具。no-nesting 防护见 §3 callout。

```rust
ToolDef {
    name: "dispatch_subagent",
    description: "Dispatch a worker subagent to run a sub-task in its own isolated context \
                  (independent messages, independent turn budget). The worker runs to \
                  completion (synchronous — the parent chat blocks until the worker returns). \
                  When the worker finishes, its final summary is injected as the tool_result. \
                  Two built-in subagents: `researcher` (read-only: read_file / grep / glob / \
                  list_dir / web_fetch) and `general-purpose` (full toolset minus \
                  dispatch_subagent / update_checklist / background-shell tools). Worker \
                  inherits parent's permission Mode: Yolo → all-allow; Edit/Plan → a tool \
                  needing confirmation (writes, shells, web_fetch without a prior grant) \
                  surfaces a `WorkerAskBanner` in the parent's UI for allow/deny (120s \
                  timeout denies; since 2026-06-22 RULE-FrontSubagent-003).",
    input_schema: {
      "type": "object",
      "properties": {
        "subagent": {"type": "string", "enum": ["researcher", "general-purpose"]},
        "task":     {"type": "string"}
      },
      "required": ["subagent", "task"]
    },
}
```

#### `run_chat_loop` 嵌套调用(worker 路径,`chat_loop.rs:2155`)

```rust
Box::pin(run_chat_loop(
    worker_tool_defs,                        // 1
    provider.clone(),                        // 2
    context_window,                          // 3
    worker_rid,                              // 4: "{parent_rid}-sub-{seq}"
    parent_session_id.to_string(),           // 5: 复用父 session_id
    worker_messages,                         // 6: [memory_blocks, delegation_task]
    worker_sink_dyn,                        // 7: SubagentBufferSink
    db.clone(),                              // 8
    cancellations.clone(),                   // 9: worker rid 注册(不进 session_active_request)
    _session_active_request.clone(),         // 10: 复用父 map(不修改)
    read_guard.clone(),                      // 11
    memory_cache.clone(),                    // 12
    skill_cache.clone(),                     // 13
    permission_asks.clone(),                 // 14
    worker_token,                            // 15
    None,                                    // 16: resend_seq
    background_shells.clone(),               // 17
    Some(SUBAGENT_MAX_TURNS),                // 18: 200
    true,                                    // 19: skip_session_active(REVIEW-SUBAGENT-PRD #2)
    true,                                    // 20: skip_persist(worker 中间过程不进 DB)
    Some(true),                              // 21: is_worker(RULE-A-014; worker asks route via WorkerAskBanner since 2026-06-22)
    app_handle,                              // 22: 转发父 AppHandle,worker SubagentBufferSink 可走 subagent:event IPC(测试 None)
    Some(assemble_subagent_prompt(def, task)), // 23: worker 覆写父 system_prompt(B6 review defect A 修复)
)).await;
```

#### `run_chat_loop` 5 个新参数(PR1a + PR1b + PR2b + PR3 + 06-21 fix)

| # | 参数 | PR | 用途 |
|---|---|---|---|
| 18 | `max_turns: Option<usize>` | PR1a | worker turn 上限(None = 50 默认)。`turn_limit = max_turns.unwrap_or(MAX_TURNS)` |
| 19 | `skip_session_active: bool` | PR1b | `CancellationGuard::drop` 时跳过 `session_active_request.remove(session_id)`。worker 传 `true` 避免误删父映射(REVIEW-SUBAGENT-PRD #2 / RULE-E-005 不破坏) |
| 20 | `skip_persist: bool` | PR1b + PR2a fix | run_chat_loop 函数体内 **16 处**(PR1 spec 写 18,PR2a RULE-A-015 拆出 2 处:`add_token_usage` 应 streaming 累加进父 sessions 表,不在 messages 表 UNIQUE 范围内;terminal `Done` emit 必经 sink 才能让 `was_cancelled` 正确 catch) `if !skip_persist { ... }` gate 守住所有 persist 站点(persist_turn / update_message_metadata / touch_session / record_*_audit / persist_turn_cwd)。worker 传 `true` 避免与父 `messages` 表 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程由 `SubagentBufferSink` transcript 捕获(PR2 落 `subagent_runs.transcript_json`) |
| 21 | `is_worker: Option<bool>` | PR2b (RULE-A-014) | worker 路径传 `Some(true)`,`run_chat_loop` 内部构造 `PermissionContext { is_worker: true }`。Pre-2026-06-22 让 Tier 4 `ask_path`/`ask_shell` 顶部 `if ctx.is_worker { Decision::Deny }` 立刻拒绝(worker 无 UI sink,弹 modal 会挂起到 user Stop);**2026-06-22 RULE-FrontSubagent-003 后** worker ask 走 `WorkerAskBanner` round-trip(见 permission-layer.md §5b:biased select over parent cancel / 120s timeout / oneshot),`is_worker` 现主要用于 (a) ask 内部 session key `"worker:{run_id}"` 隔离 + (b) 阻止 worker `AllowAlways` 持久化进父 `session_tool_permissions`(跨权限边界)。production + 35 个 `agent_loop_*` 集成测试传 `Some(false)` 显式声明非 worker;None 默认 `false`(向后兼容) |
| 22 | `app_handle: Option<AppHandle>` | PR3 (PR2 hotfix) | 转发父 AppHandle,worker SubagentBufferSink 才能 emit `subagent:event` IPC channel(否则 PR3 drawer 看不到 worker transcript live streaming)。production 传 `Some(app.clone())`,tests 传 `None`(无 Tauri runtime,emit 路径变 no-op) |
| 23 | `system_prompt_override: Option<String>` | 06-21 fix (B6 review defect A) | worker 路径传 `Some(assemble_subagent_prompt(def, task))`,让 worker 真正使用 `SubagentDef.system_prompt` —— pre-fix `_worker_system_prompt = assemble_subagent_prompt(def, task)` 是 dead code(`chat_loop.rs:2052`),worker 实际拿到父的 `assemble_system_prompt(mode_prefix, base_prompt)` 输出,导致 prompt / permission 矛盾(pre-2026-06-22 Edit/Plan 模式下 worker prompt 写"可写"但 Tier 4 把写工具 collapse 到 `Deny`;2026-06-22 后写工具走 `WorkerAskBanner`)。fix 后 `run_chat_loop` 内部守卫:`Some(p)` → 直接用 `p`;`None` → 走原有 `assemble_system_prompt(mode_prefix, base_prompt)`(production + 36 tests 路径)。4 指令文件 prompt caching 不受影响(cache_control breakpoint 在 user role,跟 system 正交) |

### 3. Contracts

#### `SubagentDef` registry + 三层来源(`agent/subagent/mod.rs::builtin_subagents` builtin + `agent/subagent/loader.rs::SubagentCache` user/project)

> **L3d (2026-06-26)**:`SubagentDef` 字段已 owned 化(`name`/`description: String`,`tools: Vec<String>`,PR1)。builtin 之上新增 user 层(`~/.config/everlasting/agents/*.md`)+ project 层(`<project>/.everlasting/agents/*.md`),由 `SubagentCache`(read-through mtime fence,照搬 B3 `CommandCache`)合并,优先级 **project > user > builtin**(last-write-wins)。frontmatter schema(`name`/`description`/`tools`/`model`/body)+ 错误处理(per-file isolation,silent skip + warn,**无 fail-fast**)+ `tools` 继承语义(覆盖 builtin 同名且未声明 tools → 继承 builtin tools;全新 agent 未声明 → `vec![]` 全工具集)详见 [ROADMAP §1.2 L3d 已实施条目](../../../docs/ROADMAP.md#12-路线图外完成)(**原设计 PRD `docs/subagent-loader.md` 已删除**,实施后归档,2026-06-26)。`model` 字段 v1 解析但 warn-ignored(`Provider` trait 单实例模型,不切换)。builtin 两个定义见下表。

| name | `tools` allowlist | system_prompt |
|---|---|---|
| `researcher` | `[read_file, grep, glob, list_dir, web_fetch]` | "你是只读研究子代理...Cannot edit/write/shell,不能嵌套 dispatch...(06-25 起含 web_fetch)" |
| `general-purpose` | `[]`(全集减结构性禁项) | "你是通用子代理...minus dispatch_subagent / update_checklist / background-shell..." |

#### `filter_tools_for_subagent(builtin_tools, def)`(`subagent.rs`)

1. `def.tools.is_empty()` → 起点全集(general-purpose 模式);否则 → 起点 allowlist
2. **`STRUCTURALLY_DISABLED` 永远 strip**(无论 allowlist 怎么写):
   - `update_checklist`(main 进度表,worker 写会污染)
   - `dispatch_subagent`(禁嵌套,对标 Cline)
   - `run_background_shell` + `shell_status` + `shell_kill`(L1a session 级通知注入,worker 无 sink)
3. 测试 `filter_strips_structurally_disabled_even_if_allowlist_lists_them` 锁定(防御未来 frontmatter 定义误开禁项)

> **⚠️ no-nesting 真实机制(L3d PR3, 2026-06-26 修正)**:`dispatch_subagent` 在 `STRUCTURALLY_DISABLED` 里是 **defense-in-depth,不是主机制**。PR3 起 dispatch_subagent 不再注册进 `builtin_tools()` 启动快照(见 §2),而是**每 turn 由 `definition_with_cache(&SubagentCache, project_path)` 动态 append** 到 turn tool list —— 此 append 在 parent/worker 共享的 `run_chat_loop` body 内,若不区分,worker nested 调用会同样 append → worker LLM 看得到 dispatch_subagent → **可嵌套**(PR3 check 发现的 BLOCKING 安全回归,单测全绿是因为没人断言 worker turn 的 tools 内容)。真正防嵌套的是 `chat_loop.rs` 的 `if !effective_is_worker { push definition_with_cache }` gate(worker 跳过 append);`filter_tools_for_subagent` 只作用于 seed list(`builtin_tools()`),而 dispatch_subagent 已不在 seed list,故 filter 对它是冗余兜底。
>
> **Forbidden Pattern**:在 parent/worker 共享的 `run_chat_loop` body 内 append 动态 tool(或任何结构性禁项 tool),**必须**用 `effective_is_worker` gate 区分;只靠下游 `filter_tools_for_subagent` 不够 —— filter 只过滤 seed list,不过滤 per-turn append。回归测试 `agent_loop_dispatch_subagent_completes_and_returns_summary` 用 `MockProvider::sent_tools()` 断言 worker turn(slot 1)收到的 tools 不含 `dispatch_subagent`。

#### worker context(`subagent.rs::build_worker_messages`,`chat_loop.rs:1940` 传入)

- `messages[0]` = `build_instructions_blocks(memory_cache)` synthetic user message(4 文件:User/Project × CLAUDE.md/AGENTS.md,带 `cache_control: Ephemeral`,worker **自己** cache breakpoint,与父正交)
- (可选) `messages[1]` = synthetic assistant ack("Understood. I will follow these instructions...")—— 镜像 main loop 的 memory pair 保持 Anthropic wire user/assistant 交替
- 末尾 `messages.push` delegation task user message(**APPEND,不 prepend**)

**prompt cache 不变量**(B12 + L1a 两次踩过的坑锁死):worker `messages[0]` 与父 `messages[0]` 正交,不污染父 cache key。summary 注入主对话走 `ContentBlock::ToolResult`(天然末尾),绝不 `insert(0)`。

#### `SubagentBufferSink`(`subagent.rs`,实现 `ChatEventSink` trait)

- `transcript: Mutex<Vec<TranscriptEntry { kind, payload_json }>>` —— 累积 worker 的 chat-event/tool:call/tool:result,PR2 落 `subagent_runs.transcript_json`
- `text_parts: Mutex<Vec<String>>` —— Delta 事件累积,`final_text()` 拼成 summary
- `had_error: AtomicBool` —— `ChatEvent::Error` → true
- `was_cancelled: AtomicBool` —— `Done{stop_reason: cancelled}` → true(`max_turns` 不算 cancel,归 Completed)
- **不 forward 父 sink** —— 否则 main UI 被 worker 流刷屏(Claude Code 约定:中间过程对 main 隔离)

#### `format_dispatch_result(status, worker_text, partial_actions)`(`subagent.rs`)

| status | content | is_error |
|---|---|---|
| `Completed` | `[status: completed]\n<summary>`(空文本回退 `(worker produced no final text)`) | false |
| `Cancelled` | `[status: cancelled]\n<text>\n\n[CANCELLED_MARKER]`(空文本退化为 `marker alone`) | true |
| `Incomplete` | `[status: incomplete]\n<text>\n\n[INCOMPLETE_MARKER]`(`[未完成]`,空文本退化为 `marker alone`) | true |
| `Error` | `[status: error]\n<error text>` | true |

**RULE-BackSubagent-001 (2026-06-22)**:`partial_actions: Option<&str>` 对**非 completed 三态**(Cancelled / Incomplete / Error)在 body 之后 append `\n\nWorker partial actions:\n<summary>` 段,让 parent LLM 看到 worker 已执行的 tool_call 摘要(`- {name}({key_param}): ok|failed|?`),做补偿修复(跳过已落地 write/edit、重试 failed tool)。摘要由 `summarize_worker_tool_actions(transcript_snapshot)` 构建(tool_call/tool_result 按 `tool_use_id` 配对,orphan tool_call 标 `?`;`chat_event`/`permission_ask` 跳过;2 KiB head+tail cap,超限 `(N actions omitted)` 计数)。`Completed` 传 `None`;空摘要(worker 未执行任何 tool_call)也传 `None`,不产生空标题。摘要**只进 tool_result wire**(parent LLM 消费),**不进** `subagent_runs.final_text`(drawer 已有完整 Tools 段,避免冗余 —— `format_final_text` 不变)。

terminal `Done{cancelled}` 事件**不**守 `skip_persist` —— worker `SubagentBufferSink.was_cancelled` 仍能正确捕捉,只 DB writes(cwd / touch_session)守门。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `subagent` 不在 enum | LLM 错(input schema 校验拦在前面) |
| `subagent_cache.lookup(project_path, name)` 返 None(L3d PR3 起,替代 `lookup_subagent`) | 拦截点合成 `tool_result` `[status: error]\nunknown subagent: <name>. Available: <cache.list 全部名字>`,`is_error: true`,**tool_use/tool_result 配对保持**(同 RULE-A-007) |
| worker turn 超 `SUBAGENT_MAX_TURNS=200` | `Done{stop_reason: max_turns}` → status=**Incomplete**(soft,"ran out of budget"),summary 仍带 worker 产出 + `INCOMPLETE_MARKER` `[未完成]` 标记(R2: 06-21 task 把 max_turns 终止从 Completed 改为 Incomplete) |
| 用户 Stop 传播到 `worker_token`(child of `parent_token`) | `Done{stop_reason: cancelled}` → status=Cancelled + `CANCELLED_MARKER` |
| worker LLM stream error | `ChatEvent::Error` → SubagentBufferSink.had_error → status=Error |
| parent 复用 `session_id` + guard `skip_session_active=true` | worker Drop **不** evict 父 `session_active_request[parent_session_id]`(回归测试 `dispatch_subagent_guard_does_not_evict_parent_session_active`) |
| worker 内写 messages 表(`skip_persist=true`) | **16 处** gate 全部拦下(PR1 spec 18,PR2a RULE-A-015 拆出 2 处:terminal Done emit / add_token_usage streaming 累加父 sessions),worker 中间过程不进父 DB |

### 5. Good / Base / Bad Cases

**Good**:parent turn 1 LLM 派 `researcher`("找出所有引用 `dispatch_subagent` 的文件")→ researcher 跑 `read_file`/`grep`/`list_dir`(4 路径全 silent allow)→ final text "found 3 files: ..." → `format_dispatch_result(Completed, ...)` → parent 构造 `ContentBlock::ToolResult`,tool_use/tool_result 配对,parent turn 2 继续。

**Base**:parent turn 1 LLM 派 `general-purpose` 改文件 + main=yolo(继承 yolo,写/shell Tier 4 bypass 早返回 Allow)→ worker 跑 `write_file` + `shell`(无 ask modal 阻塞)→ final text "已修改 3 个文件: ..." → Completed。

**Bad**:parent turn 1 LLM 派 `general-purpose` + main=Edit + `write_file`(触发 Tier 4 ask)→ **RULE-A-014 修复前(B6 PR1b)**:worker 路径构造了 `_worker_permission_ctx { is_worker: true }` 但未 thread 进嵌套 `run_chat_loop`,run_chat_loop 内部从 session row 重建 `PermissionContext { is_worker: false }` → `ask_path` 顶部 `if ctx.is_worker { Deny }` 在嵌套路径不可达 → emit `permission:ask` 等 oneshot(永远等不到,worker 无 UI sink)→ **挂起直到 user Stop**。**RULE-A-014 修复后(B6 PR2b)**:worker 路径传 `is_worker=Some(true)` 给嵌套 `run_chat_loop`,loop 内部 `effective_is_worker = is_worker.unwrap_or(false) = true` → `PermissionContext { is_worker: true }` 构造成功 → Tier 4 `ask_path` 顶部立即 `Decision::Deny`,无 oneshot 等待,无挂起;tool_result `is_error=true` + deny 原因回 LLM 自我纠错。**RULE-A-016 修复后(B6 PR3a 2026-06-20)**:worker deny 不再写父 `session_audit_events`(改走 sink → transcript PermissionAsk entry,见 §3 "audit 不污染父的分工")。回归测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(`tokio::time::timeout(15s)` 包裹,若 PR2b 修复回退则卡 oneshot 触发 15s 超时 fail)。**B6 review defect A 修复后(2026-06-21)**:worker 路径额外传 `system_prompt_override=Some(assemble_subagent_prompt(def, task))`,让 worker 真正使用 `SubagentDef.system_prompt`(pre-fix `_worker_system_prompt` 是 dead code)。修后 worker prompt 写"可写"时即真正可写(yolo)、写"只读"时即 read-only(researcher),prompt / 权限行为一致。

### 6. Tests Required

**Unit**(`subagent.rs::tests`,~17 个):

| Test | 断言 |
|---|---|
| `definition_has_correct_name` | `ToolDef.name == DISPATCH_TOOL_NAME` |
| `definition_schema_requires_subagent_and_task` | `input.required` 含两字段 |
| `definition_schema_subagent_enum_covers_two` | enum == `["researcher", "general-purpose"]` |
| `builtin_subagents_has_two_entries` | registry 长度 2 |
| `builtin_subagents_researcher_tool_allowlist` | researcher.tools == 4 只读件 |
| `builtin_subagents_general_purpose_empty_allowlist` | general-purpose.tools.is_empty() |
| `lookup_subagent_unknown_returns_none` | unknown name → None |
| `filter_researcher_keeps_only_read_tools_and_strips_disabled` | researcher + 禁项全 strip |
| `filter_general_purpose_keeps_full_set_minus_disabled` | general-purpose 保留写/shell,strip 禁项 |
| `filter_strips_structurally_disabled_even_if_allowlist_lists_them` | 即便 allowlist 列了禁项也强制 strip |
| `buffer_sink_accumulates_text_deltas` | Delta 事件累积成 summary |
| `buffer_sink_tracks_cancelled_done` | `Done{cancelled}` → was_cancelled |
| `buffer_sink_tracks_error_event` | `Error` → had_error |
| `buffer_sink_records_transcript_entries` | 3 类 emit 进 transcript |
| `format_completed_with_summary` / `_empty_text_falls_back_to_note` | Completed 两种格式 |
| `format_cancelled_includes_marker` / `_empty_text_uses_marker_alone` | Cancelled 两种格式 |
| `format_error_includes_status_prefix` | Error 格式 |
| `format_dispatch_result_appends_partial_actions_when_some` / `_none_..._no_section` / `_empty_..._no_section` | RULE-BackSubagent-001:`Some`(非空) append `Worker partial actions:` 段;`None` / `Some("")` 不 append |
| `summarize_*`(pairs/key_param/unknown/skips/order/under_cap/head_tail/empty/no_tool_calls) + `key_param_truncates_long_values` | summarize 配对 ok/failed/? + per-tool key_param + head+tail cap + 跳过非 tool kind + 多字节截断 |

**Integration**(`agent/tests.rs::agent_loop_dispatch_subagent_*`,5 个):

| Test | 断言 |
|---|---|
| `dispatch_subagent_completes_and_returns_summary` | parent turn 1 dispatch_subagent tool_use → worker 跑 → summary tool_result `[status: completed]` + worker text;主对话 `phantom_worker_text == 0`(worker 中间过程**不**进父 messages) |
| `dispatch_subagent_cancel_propagates_to_worker` | parent_token cancel → worker_token child 触发 → status=cancelled + `CANCELLED_MARKER`;tool_use/tool_result 配对保持 |
| `dispatch_subagent_error_returns_status_error` | MockProvider stream error → status=error;tool_use/tool_result 配对保持 |
| `dispatch_subagent_error_includes_partial_transcript_summary` | RULE-BackSubagent-001:worker 先执行 `read_file` 再 stream error → status=error + tool_result content 含 `Worker partial actions:` 段 + `read_file(` 摘要行 |
| `dispatch_subagent_guard_does_not_evict_parent_session_active` | 用 `HangingThenCancel` worker(500ms 延迟 cancel)保住 worker 在飞,snapshot 验证父 `session_active_request[parent_session_id]` 仍正确(worker Drop 因 `skip_session_active=true` 不误删) |

### 7. Wrong vs Correct —— 拦截路径(execute_tool_inner vs chat_loop loop)

#### Wrong:`dispatch_subagent` 走 `execute_tool_inner`

```rust
// tools/mod.rs::execute_tool_inner
match name {
    "dispatch_subagent" => dispatch_subagent::execute(input, ctx, ...).await,
    // ...
}
```

**Why it's wrong**:`execute_tool_inner` 签名 `(name, input, ctx, guard, session_id, skill_cache, cancel)` 拿不到 `provider` / `db` / `cancellations` / `session_active_request` / `read_guard` / `memory_cache` / `permission_asks` / `background_shells`,而 `run_chat_loop` 嵌套调用需要全部(REVIEW-SUBAGENT-PRD #3 核实)。即使把它们塞进 `ToolContext`,会模糊工具层和 agent 层边界;`Box<dyn Any>` extension point hacky。

#### Correct:agent loop 层拦截

```rust
// chat_loop.rs tool_use 处理循环(约 :1380)
if tool_name == DISPATCH_TOOL_NAME {
    // 不走 execute_tool;直接调 run_subagent(拿到全部 run_chat_loop 闭包依赖)
    let (content, is_error, _cancel_parent, _exit_code) =
        run_subagent(/* 全部闭包依赖 */, tool_input, ctx).await;
    // 构造 ContentBlock::ToolResult 回填(配对)
    result_blocks.push(ContentBlock::ToolResult { tool_use_id, content, is_error });
    continue;
}
// 其他 tool 走原 execute_tool 路径
let (out, is_err, update, exit_code) = execute_tool(name, input, ...).await;
```

### 8. Design Decisions

#### Decision: 同步阻塞 MVP,异步 fan-out 留 v2 / L3

- **Context**: Claude Code `background: true` 字段区分前/后台;OpenHands `DelegateTool`(并行)vs `TaskToolSet`(同步)两个独立工具。
- **Decision**: MVP `dispatch_subagent` 同步阻塞(main 在 execute 里 await worker)。main UI 在 worker 跑期间不刷新,worker 完成后 summary 一次性回填。
- **Why**: 与本项目 L1a background shell 的"返回 handle + 下一轮 APPEND notification"模式正交;MVP 最小结构;异步 fan-out 是 v2 / L3 增强。
- **Future**: 加 `dispatch_subagents`(plural)并行 fan-out + 完成通知走 L1a `drain_notifications` 机制。

#### Decision: tool allowlist + 结构性禁项双层过滤

- **Context**: Claude Code `tools` allowlist + `disallowedTools` denylist;Cline 硬编码 6 只读件。
- **Decision**: allowlist + `STRUCTURALLY_DISABLED` 硬编码 5 项永远 strip(无论 allowlist 怎么写)。
- **Why**: allowlist 表达"worker 能用什么";`STRUCTURALLY_DISABLED` 表达"无论什么都不能用"(跨 subagent 类型的安全边界,防止未来 frontmatter 定义误开禁项)。`filter_strips_structurally_disabled_even_if_allowlist_lists_them` 测试锁定。

#### Decision: CancellationGuard 加 `skip_session_active` 字段(不复用等价证明)

- **Context**: 现有 `CancellationGuard::drop` 固定 `remove(rid) + remove(session_id)`。worker 复用 parent_session_id 时 Drop 会误删父 `session_active_request[parent_session_id]`,破坏 RULE-E-005 / `cancel_inflight_for_session`。
- **Decision**: 加 `pub skip_session_active: bool` 字段;Drop 包 `if !skip_session_active { ... }`。production chat 传 `false`(行为不变);worker 传 `true`。
- **Why(选 A 不选 B/C)**:A 干净,签名 `CancellationGuard { ..., skip_session_active: bool }`,B 让 worker 不创建 guard 手动管理清理脆弱(多个 cleanup site 易漏),C 用 dummy `session_active_request` map 浪费(且与真实 map 行为可能 drift)。

#### Decision: `skip_persist` 守 16 处 persist 站点(worker 中间过程不进 DB,PR1 18 → PR2a 16)

- **Context**: worker 复用 parent_session_id,直接调 `persist_turn` 会与父的 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程对父 messages 透明是核心约定。
- **Decision**: 加第 20 参 `skip_persist: bool`,run_chat_loop 函数体内 persist 调用全部包 `if !skip_persist { ... }`(initial user / resend audit / metadata / assistant turn / cancel-synthetic / parallel+serial tool_executed_audit / tool_result / max_turns / cwd / touch_session / etc.)。**PR1 写 18 处,PR2a 实测 16 处(RULE-A-015 拆出)**:(a) `add_token_usage` 不在 messages 表的 `(session_id, seq)` UNIQUE 范围,worker streaming 累加应进父 sessions 表,不归 skip_persist 守;(b) terminal `Done` emit 必经 `SubagentBufferSink` 才能让 `was_cancelled` 正确 catch,不是 DB write 不归 skip_persist 守。这 2 处修正同步进 `agent-loop-architecture.md` §"Pattern: PR2a corrected PR1 over-broad skip_persist gate (RULE-A-015)" pattern 段,作为未来 skip_* flag 的设计参考(不重蹈 PR1 "all persist = same gate" 过度宽泛的反模式)。
- **Why 不在拦截点单独守门**:worker 调用 `run_chat_loop` 嵌套后,函数体本身不知道自己是 worker;把守门推到函数体内**单一权威**(对齐 RULE-A-006 单一权威),每个 persist site 一目了然。

---

## Scenario: subagent_runs persistence (B6 PR2, 2026-06-20)

> **Source of truth**: the migration lives in
> `app/src-tauri/src/db/migrations.rs` (PR2 schema, 2026-06-20);
> the CRUD layer is `app/src-tauri/src/db/subagent_runs.rs`; the
> 4 MiB cap helper is `app/src-tauri/src/agent/subagent.rs::truncate_transcript_for_persistence`;
> the worker→persist bridge is `run_subagent` in
> `app/src-tauri/src/agent/chat_loop.rs` (~:1802-2200).
>
> **何时读本文**:涉及 `subagent_runs` 表 / 增删改查 helper / 4 MiB transcript cap / PR3 前端 ToolCallCard 展开 / C4 audit 查 worker 决策 / worker token usage 折算进父 session 等。
>
> 上一段 Scenario(PR1 的 `dispatch_subagent tool`)锁定 worker 注册 / 拦截路径 / SubagentBufferSink 行为 / `format_dispatch_result` 输出格式。本段锁定 worker 跑完后 transcript + token usage + summary 如何落 DB,以及**审计不污染父 session** 的分工。

### 1. Scope / Trigger

- Trigger: PR1 落地的 `SubagentBufferSink` transcript(worker chat-event / tool:call / tool:result)是**进程内 in-memory** —— 关掉 app 后 reload 主对话,worker 中间过程全丢。PR3 前端 `ToolCallCard` 展开 UI 需要持久化 transcript 才能渲染;reload-after-restart 必须仍能查;每条 worker 决策(Tier 2/3/4 collapse)需要可审计但不污染父 `session_audit_events`(审计完整性 RULE-A-016 closed B6 PR3a 2026-06-20)。
- Why code-spec depth: mandatory —— 涉及新表 migration / 5 个 CRUD helper / 4 MiB 截断 cap / streaming token_usage 累加(PR2a 顺手解 RULE-A-015 over-broad gate)/ parent-session CASCADE / audit 不污染父(transcript `PermissionAsk` vs `session_audit_events`)分工,每项都是可执行合约。
- ROADMAP §1.2 B6 Subagent PR2 计划项 + DEBT.md §"子 task 编排建议" PR1+2 拆分。

### 2. Signatures

#### `subagent_runs` schema(跟随 `session_audit_events` 模式,`db/migrations.rs` 2026-06-20 新增)

```sql
CREATE TABLE IF NOT EXISTS subagent_runs (
    id TEXT PRIMARY KEY,                                           -- UUID v4 nanoid
    parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_request_id TEXT NOT NULL,                               -- worker rid "{parent}-sub-{seq}"(非 FK,cancellations in-memory)
    subagent_name TEXT NOT NULL,                                   -- 'researcher' | 'general-purpose'
    status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error')),
    started_at TEXT NOT NULL,                                      -- ISO 8601 RFC 3339
    finished_at TEXT,                                              -- NULL = running
    token_usage_json TEXT,                                         -- JSON TokenUsage { input / output / cache_creation / cache_read }
    summary TEXT,                                                  -- final_text 纯文本(无 status 前缀;PR2 PRD 决策 #3)
    transcript_json TEXT,                                          -- JSON Vec<TranscriptEntry>(4 MiB cap 后)
    transcript_truncated INTEGER NOT NULL DEFAULT 0,               -- 1 = 超过 4 MiB cap
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
    ON subagent_runs(parent_session_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
    ON subagent_runs(parent_request_id);
```

CASCADE 要求 `PRAGMA foreign_keys = ON`(`init_pool` 在每个 connection 首次启用),与 `session_audit_events` 一致。

#### `db::subagent_runs` module API(`app/src-tauri/src/db/subagent_runs.rs`)

```rust
// DB-side status 枚举,字符串与 migration CHECK 约束一一对应
#[serde(rename_all = "lowercase")]
pub enum SubagentStatusDb { Running, Completed, Cancelled, Error }
impl SubagentStatusDb {
    pub fn as_str(&self) -> &'static str;  // wire form, lockstep with CHECK
    pub fn from_str_opt(s: &str) -> Self;  // lenient parse:unknown → Running
}

// Row 形状(IPC 边界 camelCase)
#[derive(FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunRow {
    pub id: String, pub parent_session_id: String, pub parent_request_id: String,
    pub subagent_name: String, pub status: String, pub started_at: String,
    pub finished_at: Option<String>, pub token_usage_json: Option<String>,
    pub summary: Option<String>, pub transcript_json: Option<String>,
    pub transcript_truncated: i64, pub created_at: String,
}

// 5 个公开 helper
pub async fn insert_run(pool, parent_session_id, parent_request_id, subagent_name)
    -> Result<String, sqlx::Error>;                            // → id; status='running' + 空 TokenUsage + '[]' transcript

pub async fn update_run_finished(
    pool, id, status: SubagentStatusDb, finished_at, summary,
    token_usage: &TokenUsage, transcript_json: &str, truncated: bool,
) -> Result<(), sqlx::Error>;

pub async fn get_run(pool, id) -> Result<Option<SubagentRunRow>, sqlx::Error>;
pub async fn list_runs_by_session(pool, parent_session_id) -> Result<Vec<SubagentRunRow>, sqlx::Error>;

pub async fn add_token_usage_streaming(
    pool, parent_session_id: &str, usage: &TokenUsage,
) -> Result<(), sqlx::Error>;                                // 复用 db::add_token_usage,但语义独立(worker streaming)
```

#### 4 MiB transcript cap(`agent/subagent.rs::truncate_transcript_for_persistence`)

```rust
pub const TRANSCRIPT_MAX_BYTES: usize = 4 * 1024 * 1024;  // 4 MiB

/// 纯函数。serialize `Vec<TranscriptEntry>` → JSON 字节数:
/// - ≤ cap → 原样返回 (transcript, truncated=false)
/// - > cap → 保留 head + tail 各 half 字节,重新 parse,标记 truncated=true
/// - 极端 case(head/tail 落在单元素 JSON 边界)→ 退化为保留 head 字节,
///   transcript_truncated=1 信号让 PR3 UI 显示 "transcript truncated" badge
pub fn truncate_transcript_for_persistence(
    transcript: Vec<TranscriptEntry>, max_bytes: usize,
) -> (Vec<TranscriptEntry>, bool);
```

### 3. Contracts

#### `insert_run` 行为(`run_subagent` 启动时立即调)

- 生成 UUID v4 nanoid 作为 `id`,`started_at = Utc::now().to_rfc3339()`。
- `status='running'`,`finished_at=NULL`,
  `token_usage_json = serde_json::to_string(&TokenUsage::default())`(全 0 占位,后续 UPDATE 覆盖),
  `summary=NULL`,`transcript_json='[]'`,`transcript_truncated=0`。
- 返回 `id` 给 `run_subagent` 作为 `worker_run_id`,PR3 展开 UI 用此 id 调 `get_run`。
- 失败:`Result::Err(sqlx::Error)`(FK 违反说明 parent_session_id 不存在,罕见;`run_subagent` 走 warn+continue 不 block dispatch)。

#### `update_run_finished` 行为(`run_subagent` 跑完时一次性调)

- `status` 由 caller 决定:`Completed` / `Cancelled` / `Error`(`Cancelled` 由 `was_cancelled()` 触发;`Error` 由 `had_error()` 触发;两者皆 false 走 `Completed`)。
- `finished_at = Utc::now().to_rfc3339()`(terminal 必填;DB 没 NOT NULL 但契约层强制)。
- `summary = worker_sink.final_text()`(PR2 PRD #3 决策:**纯文本,无 `[status: ...]` 前缀**;`status` 字段独立,前端重组展示时按 status 加前缀)。
- `token_usage_json = serde_json::to_string(&token_usage_sum)`,`token_usage_sum` 是 worker 全 turn 累加(`sink.cumulative_usage()` 返回的 4 字段 TokenUsage)。
- `transcript_json = serde_json::to_string(&truncate_transcript_for_persistence(worker_sink.transcript_snapshot(), TRANSCRIPT_MAX_BYTES).0)`,`transcript_truncated` 是 cap 函数的第二个返回(`bool` → i64)。
- 失败:best-effort —— `tracing::warn!` + 继续。dispatch_subagent 的 `tool_result` 才是用户可见信号;DB 持久化失败不应阻止 user 收到 worker 产出(RULE-A-003 风格)。

#### `add_token_usage_streaming` 行为(worker 每 turn 调,与 `update_run_finished` 互补)

- 走 `db::add_token_usage` 同源(对 `sessions` 表的 `input_tokens_total` / `output_tokens_total` 等列累加),语义独立为"worker streaming"版本,避免被 `skip_persist` gate 误拦(PR2a RULE-A-015 修复)。
- 在 `run_chat_loop` 的 turn 边界(已有 `add_token_usage(db, &session_id, &turn_usage)` 调用)拆出来 —— worker 路径用 `parent_session_id`(`&parent_session_id` 而非 `&session_id`)。父 UI 可在 worker 跑期间看到 token counter 实时上涨(不是 worker 跑完才一次性更新)。
- 失败:best-effort warn+continue(token_usage 是元数据,不污染 messages 表也不参与 messages.seq,失败不应中断 agent loop)。
- 测试 `subagent_runs_token_usage_streaming_missing_session_is_noop` 锁定:parent_session_id 不存在时返回 Ok(())(实际场景不会发生,但防御性一致 —— 与 `update_message_metadata` 的"未知 seq → noop"对称)。

#### `get_run` / `list_runs_by_session` 行为(PR3 API 表面)

- `get_run(id)` 按主键查,None 表示 id 不存在,Some 返回 `SubagentRunRow`。
- `list_runs_by_session(parent_session_id)` 按 `(parent_session_id, started_at DESC)` 索引扫描,返回该 session 所有 worker run(包含 `running` 中,前端可显示 "5 workers active" 状态徽章)。
- IPC surface:`#[serde(rename_all = "camelCase")]` 跨 Tauri 2 边界(与所有 `db::*Row` 一致),前端 TS 直接读 `row.id` / `row.parentSessionId` / `row.transcriptJson` 等。

#### `SubagentStatusDb::as_str` ↔ migration CHECK 约束的 lockstep

- `as_str()` 返回的字符串必须与 migration 的 `CHECK(status IN ('running','completed','cancelled','error'))` 精确一致;未来增加新状态(如 `'timed_out'`)必须**同时改** migration CHECK + 枚举 + 测试 `audit_kind_round_trip` 类。`from_str_opt` 对 unknown 字符串 lenient 解析为 `Running`(forward-compat 兜底,见 `database-guidelines.md` "Enum pattern: lenient parse for forward-compat")。

#### `transcript_json` cap 4 MiB 的取舍(PR2 PRD #2 决策)

- 4 MiB = `4 * 1024 * 1024 = 4_194_304 bytes`。SQLite TEXT 默认上限 1 GiB,4 MiB 远在安全阈值内;实际 20 turn worker transcript 在 busy tool-use 场景约 100 KiB(3 个数量级 below cap),4 MiB 是"防 worst case 单 worker 撑爆 DB"的兜底,不是"贴近真实使用"的量级。
- 超过 cap → `truncate_transcript_for_persistence` 头+尾各半 + `transcript_truncated=1` 标记。PR3 展开 UI 看到 `truncated=true` 时显示 "transcript truncated, show full via..." 提示(具体 UX 留 PR3 设计)。

#### token_usage 折算父 session 的 streaming 设计

- worker `run_chat_loop` 内部 turn 边界已有 `add_token_usage(db, &session_id, &turn_usage)` 调用。worker 复用 `parent_session_id`,且 PR2a 拆出 `add_token_usage_streaming` 版本(独立于 `skip_persist` gate,见 RULE-A-015)。Streaming 路径:worker 每 turn 调 `add_token_usage_streaming(pool, &parent_session_id, &turn_usage)` → 直接累加进父 `sessions.input_tokens_total` 等;**不**经过 `update_run_finished`(那里只算 worker's own `token_usage_json` 字段,作为 run-level snapshot,不是累加源)。
- 测试 `agent_loop_dispatch_subagent_token_usage_folds_into_parent` 锁定:worker 跑 2 turn,父 session `input_tokens_total` 等于 2 turn 累加值;`subagent_runs.token_usage_json` 解码后是同一 sum(两个来源一致)。

#### audit 不污染父的分工(RULE-A-016 closed B6 PR3a 2026-06-20)

- **当前行为(post-PR3a)**:worker 路径**不**调 `record_audit_event`(由 `run_chat_loop` 函数体内的 16 个 `if !skip_persist` gate 自动实现 —— `record_*_audit` 调用在 gate 内);worker 的 ⑨ 决策存于 transcript 的 `TranscriptKind::PermissionAsk` 事件。
- **Tier 4 `ask_path` worker 分支(post-PR3a)**:`ask_path` 顶部 `if ctx.is_worker { ... }` collapse 路径**不**调 `record_audit_event`;改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 记录 `TranscriptKind::PermissionAsk` entry 到 worker transcript(PR3 drawer 可见)。父 `session_audit_events` 不被 worker 决策污染。
- **Worker ask resolve outcome (2026-06-22, RULE-WorkerAsk-001)**:post-FRONT-001 fix,worker `ask_path` 的 `tokio::select!{cancel, timeout, oneshot}` 返回后,`SubagentBufferSink::emit_permission_ask_resolved(&rid, outcome)` 写 `TranscriptKind::PermissionAskResolved` transcript entry(payload `{rid, outcome}`,outcome ∈ `{"allow","deny","timeout","cancel"}`;worker `AllowAlways` 当 `AllowOnce` 统一记 `"allow"`;`OneshotDropped` 记 `"cancel"`)。trait `ChatEventSink::emit_permission_ask_resolved` **默认 no-op**,仅 `SubagentBufferSink` override(避免 `Arc<dyn>` downcast,`AppHandleSink` / 测试 sink 零改动继承默认)。**transcript-only** — 不双发 `permission:ask` IPC;live 期间交互卡从 interactive 翻 historical 由 permissions store rid removal 驱动(Session 62 `89e5ba1` 行为不变)。前端 `pairSections` 按 `rid` 预扫 `PermissionAskResolvedSection` → 给 `PermissionAskSection.outcome` 透传 → `<PermissionAskBody>` historical 分支显 ✓已允许 / ✗已拒绝 / ⏱已超时 / ⊘已取消 badge(色 token 复用 `--color-tool-write` / `--color-tool-error` / `--color-text-muted`,无 one-off 新 token)。pre-fix 老 transcript(无 `PermissionAskResolved` entry)降级显中性 "worker asked for X",不 crash(`pairSections` 配对找不到 resolved → `outcome` undefined → `<PermissionAskBody>` 走 neutral 分支)。
- **测试覆盖**:`audit_not_polluted_by_worker`(researcher silent allow / Tier 5)断言父 audit `delta == 2`;`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(general-purpose + Edit + write_file 触发 Tier 4 ask_path collapse)断言父 `tool_denied count == 0` + worker transcript `PermissionAsk count == 1` + audit delta ≤ 2。两个场景都覆盖。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `insert_run` parent_session_id 不存在 | FK 违反 → `Err(sqlx::Error::Database)`;`run_subagent` warn+continue 走 `format_dispatch_result(SubagentStatus::Error, "spawn failed")` |
| `update_run_finished` 重复调(同 id 调 2 次) | 第 2 次覆盖(running → terminal 是单向,terminal → terminal 是覆盖);不影响 DB 完整性 |
| `update_run_finished` 时 worker 0 turn(`transcript` 空) | `transcript_json = '[]'`,`transcript_truncated = 0`,`summary` 走 `final_text` 空文本退化(`(worker produced no final text)`) |
| `transcript` serialize 后 > 4 MiB | `truncate_transcript_for_persistence` 头+尾各半;`transcript_truncated=1`;DB 写 OK |
| `transcript` 极端 case:head 字节落在单元素 JSON 边界 | `truncate_transcript_for_persistence` 退化为保留 head 字节(single-element vector);`truncated=true`;JSON 可能不严格 valid 但 `transcript_truncated=1` 标记已警示 |
| `add_token_usage_streaming` parent_session_id 不存在 | `Ok(())` 静默 noop(与 `update_message_metadata_on_unknown_seq_is_noop` 防御模式一致) |
| `get_run(id)` 未知 id | `Ok(None)`(前端 PR3 展示 "run not found",不是 Err) |
| `list_runs_by_session` 空 session | `Ok(vec![])` |
| 删父 session | `ON DELETE CASCADE` 同步删所有 `subagent_runs` 行;测试 `subagent_runs_cascade_delete_with_parent_session` 锁定 |
| `status='running'` 行无 `finished_at` | 运行时(running)合法;terminal update 后 `finished_at` NOT NULL |
| RULE-A-016 已修复(post-PR3a) | Tier 4 collapse worker 分支 emit `PermissionAskPayload` via sink → worker transcript;**不**写父 `session_audit_events`(PR3a 修复,见 §3) |

### 5. Good / Base / Bad Cases

**Good**(happy path):parent turn 1 派 `general-purpose`("分析 /tmp/下的 .log 文件")→ worker 跑 3 turn(读 + grep + 总结)→ final_text "找到 3 个 .log,最大的 2GB..." → `run_subagent`:
1. INSERT subagent_runs row: status='running', summary=NULL, transcript_json='[]'
2. 3 turn 各自 streaming 累加进父 sessions.input_tokens_total
3. UPDATE subagent_runs row: status='completed', finished_at=NOW, summary="找到 3 个 .log...", token_usage_json=sum, transcript_json=[3 turn events] (JSON ~5 KiB), transcript_truncated=0
→ parent 构造 ContentBlock::ToolResult `[status: completed]\n找到 3 个 .log...`,tool_use/tool_result 配对,parent turn 2 继续。用户 reload 主对话,ToolCallCard 展开时调 `get_run(worker_run_id)` 取 transcript 渲染。

**Base**(cancel 中途):parent_token 在 worker turn 2 cancel → worker_token child 触发 → SubagentBufferSink.was_cancelled=true → status=Cancelled → `update_run_finished`: status='cancelled', finished_at=NOW, summary="partial result\n\n[CANCELLED_MARKER]", transcript_json 包含 cancel 之前 2 turn 的事件 + `Done{cancelled}` terminal 事件(PR2a 修复 RULE-A-015 后 terminal Done emit 必到 sink), transcript_truncated=0。父 UI reload 后 ToolCallCard 展开看到 "cancelled at turn 2" + 中间 2 turn 的 transcript(可读性 + 可恢复性的中间态)。

**Good**(RULE-A-016 fixed B6 PR3a):parent turn 1 派 `general-purpose` + main=Edit + `write_file` 触 Tier 4 ask → worker Tier 4 ask_path 立即 `Decision::Deny`(`is_worker=true` collapse),`format_dispatch_result(SubagentStatus::Error, "denied: ...")` 回 parent,tool_result `is_error=true`;PR3a 修复后,worker 分支**不**写 `record_audit_event(ToolDenied)`,改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 记录 `TranscriptKind::PermissionAsk` entry 到 worker transcript → 父 `session_audit_events` 不被污染,C4 audit log UI 只看父自己的 ⑨ 决策;PR3 drawer 可见 worker 的 deny 决策(transcript 中)。

**Bad**(RULE-A-015 已修但 spec 留档):如果 PR2a 没拆 `add_token_usage` 和 terminal `Done` emit 2 处 `skip_persist` gate,worker 跑完时 `subagent_runs.status` 永远='completed'(cancelled 路径也走 Completed 因为 `was_cancelled=false`,sink 没收到 terminal Done),`sessions.input_tokens_total` 永远是 parent dispatch 前的值(worker 期间的 token 没 streaming 进来)。回归测试 `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled` 锁定 fix。

### 6. Tests Required

**Unit**(`db/tests.rs::subagent_runs_*`,PR2a 7 个):

| Test | 断言 |
|---|---|
| `subagent_runs_insert_creates_running_row` | INSERT 后 row 存在 + status='running' + finished_at=NULL + transcript_json='[]' + transcript_truncated=0 + token_usage_json 解码为全 0 TokenUsage |
| `subagent_runs_update_finished_sets_status_and_fields` | UPDATE 后 status 切到 terminal + finished_at NOT NULL + summary = 输入 final_text + token_usage_json 解码 = 输入 TokenUsage + transcript_json 包含 emit 的事件 |
| `subagent_runs_update_finished_records_truncated_flag` | UPDATE 时 truncated=true → transcript_truncated=1;transcript_json 包含 head+tail 头尾(中间段已截断) |
| `subagent_runs_cascade_delete_with_parent_session` | 2 个 subagent_runs row 挂在 s1 → 删 s1 → 2 row 同步消失(RULE-CASCADE FK 生效) |
| `subagent_runs_list_by_session_orders_by_started_desc` | 2 row 不同 started_at → list 返回按 started_at DESC 排序 |
| `subagent_runs_token_usage_streaming_accumulates_in_parent` | 2 turn usage 调 streaming 累加 → parent session input_tokens_total = 2 turn sum(解 add_token_usage 路径) |
| `subagent_runs_token_usage_streaming_missing_session_is_noop` | parent_session_id 不存在 → Ok(())(防御性,无 panic / Err) |

**Unit**(`subagent.rs::tests`,PR2a truncate 函数 ~5 个):
- `truncate_transcript_under_cap_passes_through` (transcript 字节 < cap → 原样返 + truncated=false)
- `truncate_transcript_over_cap_keeps_head_and_tail` (transcript 字节 > cap → 头尾各半 + truncated=true)
- `truncate_transcript_empty_input` (空 transcript → 返回 ([], false))
- `truncate_transcript_respects_max_bytes_constant` (TRANSCRIPT_MAX_BYTES == 4*1024*1024 常量锁)
- `truncate_transcript_handles_extreme_head_fallback` (极端:head parse fail → 退化为 head 字节 single-element)

**Integration**(`agent/tests.rs::agent_loop_dispatch_subagent_*`,PR2a 4 + PR2b 1 = 5 个):

| Test | 断言 |
|---|---|
| `agent_loop_dispatch_subagent_persists_subagent_run` | 跑完 worker → subagent_runs row 存在 + status='completed' + summary=worker_text + transcript_json 非空 + transcript_truncated=0 + transcript_snapshot() 包含 emit 的 chat-event / tool:call / tool:result |
| `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled` | parent_token cancel 中途 → status='cancelled' + finished_at NOT NULL(RULE-A-015 回归:terminal Done emit 不在 skip_persist gate) |
| `agent_loop_dispatch_subagent_audit_not_polluted_by_worker` | researcher worker(silent allow)→ 父 session_audit_events 行数 == parent 自己的 ⑨ 决策行数(worker Tier 5 不写 audit) |
| `agent_loop_dispatch_subagent_token_usage_folds_into_parent` | worker 跑 2 turn → 父 sessions.input_tokens_total = 2 turn 累加 + subagent_runs.token_usage_json = 同一 sum |
| `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` | B6 PR2b + RULE-A-014 + RULE-A-016:parent Edit mode + general-purpose + write_file → `tokio::time::timeout(15s)` 包裹;worker Tier 4 ask_path 立即 Decision::Deny(无 oneshot 等待,无挂起)→ tool_result is_error=true + deny 原因;**post-PR3a** worker deny 不写父 audit,改走 sink → transcript PermissionAsk(1 entry);父 audit `tool_denied count == 0` + audit delta ≤ 2|

### 7. Wrong vs Correct

#### Wrong: `SubagentBufferSink` 直接调 `update_run_finished`(无 4 MiB cap)

```rust
// BAD — transcript 直接序列化,worker busy 时可能撑到数十 MB;
//      单 worker 撑爆 DB,慢 reload,UI 渲染卡顿
let transcript_json = serde_json::to_string(&worker_sink.transcript_snapshot()).unwrap();
db::subagent_runs::update_run_finished(
    &pool, &worker_run_id, status, finished_at, &summary, &token_usage,
    &transcript_json, false,  // ← truncated 永远 false,真实体积不可见
).await?;
```

**Why it's wrong**:无 cap → DB 体积不可控 → 慢 reload → 共享 DB 资源争抢。4 MiB 是 PR2 PRD #2 决策的硬上限,必须经过 `truncate_transcript_for_persistence` 后再 serialize。

#### Correct: `run_subagent` 调 `truncate_transcript_for_persistence` → `update_run_finished`

```rust
// GOOD — cap 在 persist 前必经
let (transcript_capped, truncated) =
    truncate_transcript_for_persistence(worker_sink.transcript_snapshot(), TRANSCRIPT_MAX_BYTES);
let transcript_json = serde_json::to_string(&transcript_capped).unwrap_or_else(|_| "[]".into());
db::subagent_runs::update_run_finished(
    &pool, &worker_run_id, status, finished_at, &summary, &token_usage_sum,
    &transcript_json, truncated,
).await?;
```

`truncated=true` 透传到 DB 的 `transcript_truncated=1`,PR3 展开 UI 检测到时显示"transcript truncated"提示(UX 留 PR3)。

#### Wrong: worker 决策也写 `session_audit_events`(污染父)

```rust
// BAD — 复用现有 record_audit_event 路径,不分 worker/parent
if let Decision::Deny { reason, critical: _ } = decision {
    record_audit_event(&db, &ctx.session_id, AuditKind::ToolDenied.as_str(), Some(...))
        .await;  // ← ctx.session_id 是 parent_session_id(worker 复用),污染父 audit
    return ToolResult { is_error: true, content: reason };
}
```

**Why it's wrong**:worker 复用 `parent_session_id`,`record_audit_event` 不区分 worker → worker 的 Tier 4 collapse 决策写进父的 `session_audit_events` → C4 audit log 看到 worker 决策,混淆责任归属(RULE-A-016)。

#### Correct: worker 决策走 transcript PermissionAsk,父 audit 仅 parent 自己决策

```rust
// GOOD — 入口先看 ctx.is_worker,worker 改写 transcript
if let Decision::Deny { reason, critical: _ } = decision {
    if ctx.is_worker {
        // worker 路径:append PermissionAsk 事件到 SubagentBufferSink transcript,
        //              transcript 已在 PR2 落 subagent_runs.transcript_json
        //              (PR3 展开 UI 看到 "permission: ask, denied: ..." 事件流)
    } else {
        record_audit_event(&db, &ctx.session_id, AuditKind::ToolDenied.as_str(), Some(...))
            .await;
    }
    return ToolResult { is_error: true, content: reason };
}
```

PR3a implementation matches this Correct pattern exactly (see `permissions/mod.rs::ask_path` worker branch post-PR3a): the `if ctx.is_worker { ... }` block emits a `PermissionAskPayload` via `sink.emit_permission_ask(...)` (which the `SubagentBufferSink` impl records as a `TranscriptKind::PermissionAsk` transcript entry) instead of calling `record_audit_event`. The `audit_not_polluted_by_worker` test (`delta == 2`) + the `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` test (`tool_denied count == 0` in parent audit + `permission_ask count == 1` in worker transcript + audit delta ≤ 2) lock the post-fix invariant.

### 8. Design Decisions

#### Decision: streaming 累加 token_usage(每 turn 调,父 UI 实时可见)

- **Context**:worker 复用 `parent_session_id`,且 `run_chat_loop` turn 边界已有 `add_token_usage(db, &session_id, &turn_usage)` 调用。被 16 处 `if !skip_persist` gate 守护(PR1 spec 写 18,PR2a RULE-A-015 拆出 2 处后),但 `add_token_usage` 本身不在 messages 表的 `(session_id, seq)` UNIQUE 范围内,理应不被 gate。
- **Decision**:worker 路径每 turn 调 `db::subagent_runs::add_token_usage_streaming(pool, &parent_session_id, &turn_usage)`,语义独立(不依赖 `skip_persist` gate 的当前值),直接累加进父 `sessions.input_tokens_total` / `output_tokens_total` / `cache_creation_input_tokens` / `cache_read_input_tokens`。父 UI 在 worker 跑期间看到 token counter 实时上涨,不是 worker 跑完才一次性跳。
- **Why 不一次性累加**(worker 跑完 `update_run_finished` 时 add 整个 sum):streaming 让用户感知 worker 进度(类比 Claude Code 跑任务时 token counter 滚动);一次性累加则 worker 跑 30 秒内父 UI 看到 counter 冻住,UX 差。无功能差异,纯 UX 决策。
- **Why 拆出独立 `add_token_usage_streaming` 函数**(而非改 `add_token_usage` + 检查 `is_worker`):语义清晰、worker 路径显式、PR2 单元测试可独立测(无 worker / run_chat_loop 上下文)。

#### Decision: audit 不污染父 — worker 决策走 transcript `PermissionAsk` 而非 `session_audit_events`

- **Context**:worker 复用 `parent_session_id` 是必须的(PR1 决策 5:`UNIQUE (session_id, seq)` 约束 + DB linkage 简化),但 `record_audit_event` 用 `ctx.session_id` 写入,**不分 worker / parent** → worker 的 ⑨ 决策会被审计行误写到父 `session_audit_events` 表。
- **Decision**:worker 路径的 ⑨ 决策(Deny / ToolExecuted / PermissionAsk)**不写**父 `session_audit_events`;由 `SubagentBufferSink` 的 `transcript` 累积 `TranscriptKind::PermissionAsk` 事件,PR2 落 `subagent_runs.transcript_json`。前端 C4 audit log 查父 session 时不显示 worker 决策行(`audit_not_polluted_by_worker` 测试锁定 —— researcher silent allow 场景);RULE-A-016(closed B6 PR3a 2026-06-20)已修 Tier 4 collapse 路径 —— `ask_path` worker 分支不调 `record_audit_event(ToolDenied)`,改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 写 transcript `PermissionAsk` entry。`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` 测试断言反转:`tool_denied count == 0` in parent audit + transcript `PermissionAsk count == 1` + audit delta ≤ 2。
- **Post-FRONT-001 (2026-06-22, RULE-WorkerAsk-001) extension**:worker ask resolve 后,`SubagentBufferSink::emit_permission_ask_resolved` 追加一个 `TranscriptKind::PermissionAskResolved` entry 携带 `{rid, outcome}`(outcome ∈ `{"allow","deny","timeout","cancel"}`)。这条 entry **不进** `permission:ask` IPC(避免与 live interaction 路径双发混淆)、**不进**父 `session_audit_events`(同 RULE-A-016 隔离原则)。**transcript-only** 是 outcome 的唯一归属;前端 `pairSections` 按 `rid` 配对 → `<PermissionAskBody>` historical badge。这是 transcript-as-audit 模式的自然延伸 —— worker ask 现在在 transcript 里有完整 `PermissionAsk` + `PermissionAskResolved` 两段,historical 回放可看到决策 + 结果。
- **Why 不直接给 `record_audit_event` 加 `is_worker` 参**:保持 record_audit_event 接口稳定(已 11+ 调用点),在 caller 层(if-else 写 audit / 写 transcript)显式区分;新增 `is_worker` 参会让所有 caller 多 1 个无关参数,影响面大。`if !ctx.is_worker` 入口守门是更小的 change。

#### Decision: 4 MiB transcript cap(防单 worker 撑爆 DB)

- **Context**:transcript 是 `Vec<TranscriptEntry>` 序列化,1 turn busy tool-use ≈ 2-5 KiB,20 turn worker 实际约 100 KiB;但恶意 LLM 或未来结构变化(thinking + extended thinking + parallel batch)可能把单 worker 推到数 MB 甚至 GB。SQLite TEXT 默认上限 1 GiB,worker 单条超 100 MB 已属异常。
- **Decision**:`TRANSCRIPT_MAX_BYTES = 4 * 1024 * 1024 = 4 MiB`,由 `truncate_transcript_for_persistence` 强制;超 cap 时头+尾各半(每半 2 MiB)保留 + `transcript_truncated=1` 标记 + DB 写仍 OK(降级,从不拒写)。
- **Why 4 MiB(不是 1 MiB / 16 MiB / 64 MiB)**:**远超真实使用**(20 turn worker ≈ 100 KiB,3 个数量级 below);**远在 SQLite TEXT 安全阈值内**(1 GiB,5 个数量级 below);**降级可读性**(head+tail 各 2 MiB 仍可读,不是粗暴截断末位)。比 1 MiB 宽(给 4-5 turn parallel batch + extended thinking 留足);比 16 MiB 严(防单 worker 占太大,多 worker 并发时不相互饿死)。
- **Why 降级不拒写**:worker 已经跑完 token 烧了,用户已经付出等待成本,即使 transcript 超 cap 也应落 DB(降级可读);拒写会让 worker 跑完但 user 看不到中间过程,UX 更差。`transcript_truncated=1` 给前端足够信号展示"transcript 截断"提示,用户知情。

---


## Scenario: L1a Background Shell Tools (`run_background_shell` / `shell_status` / `shell_kill`, 2026-06-19)

> **Source of truth**: implementation lives in
> `app/src-tauri/src/tools/{run_background_shell,shell_status,shell_kill}.rs`,
> backed by `app/src-tauri/src/background_shell/{mod.rs,in_memory.rs}`.
> Decisions Q1-Q7 live in `.trellis/tasks/06-19-l1-shell-pty/prd.md`.
> Follow-up: L1b (real PTY via `portable-pty`) and L3 (parallel
> subagent + worktree isolation) are separate tasks.

### 1. Scope / Trigger

The synchronous `shell` tool's 600-second cap is insufficient for
"fire-and-forget" workloads: full builds, package installs, large
test suites, dev servers. L1a adds a sibling toolset so the LLM
can start a long-running command, return a handle immediately,
and react to its completion when the next agent-loop turn begins.
Same `sh -c <command>` execution model, same env-allowlist
(RULE-E-001), same process-group SIGKILL (RULE-E-002), same
30 KB disk-spill threshold — but the lifetime crosses both the
`execute_tool` call AND the `invoke("chat")` call.

### 2. Signatures

#### Tool declarations (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/run_background_shell.rs
ToolDef {
  name: "run_background_shell",
  description: "Start a shell command in the background and return immediately \
                with a `shell_session_id` handle. Use this for long-running commands \
                (full builds, package installs, large test suites, dev servers) that \
                exceed the synchronous `shell` tool's 600-second timeout cap...",
  input_schema: json!({
    "type": "object",
    "properties": {
      "command": { "type": "string" },
      "working_directory": { "type": "string" },
      "max_runtime_ms": { "type": "integer" }
    },
    "required": ["command"]
  })
}

// app/src-tauri/src/tools/shell_status.rs
ToolDef { name: "shell_status", input_schema: { session_id: string } }

// app/src-tauri/src/tools/shell_kill.rs
ToolDef { name: "shell_kill", input_schema: { session_id: string } }
```

#### `BackgroundShellRegistry` trait + impl

```rust
// app/src-tauri/src/background_shell/mod.rs
#[async_trait-free — native async fn in trait]
pub trait BackgroundShellRegistry: Send + Sync {
    async fn start(&self, session_id, command, cwd, max_runtime_ms) -> Result<String, BackgroundShellError>;
    async fn status(&self, session_id, shell_session_id) -> Result<BackgroundShellStatus, BackgroundShellError>;
    async fn kill(&self, session_id, shell_session_id) -> Result<(), BackgroundShellError>;
    async fn kill_all_for_session(&self, session_id) -> Result<(), BackgroundShellError>;
    async fn drain_notifications(&self, session_id) -> Vec<BackgroundShellNotification>;
    async fn kill_all(&self) -> Result<(), BackgroundShellError>;
}

pub type DefaultRegistry = Arc<InMemoryBackgroundShellRegistry>;
pub fn default_registry() -> DefaultRegistry { ... }
```

#### `ToolContext` extension (PR2 additive change)

```rust
// app/src-tauri/src/tools/mod.rs
#[derive(Clone)]
pub struct ToolContext {
    pub worktree_path: PathBuf,
    pub cwd: PathBuf,
    pub checklist: ChecklistHandle,
    // NEW (L1a 2026-06-19):
    pub background_shells: DefaultRegistry,
}
```

The registry is a concrete `Arc<InMemoryBackgroundShellRegistry>`
(not `dyn BackgroundShellRegistry`) — matches the codebase's
pattern for the other cross-request handles (`MemoryCache`,
`SkillCache`, `ReadGuard`). Future daemon-ization swaps the
concrete type at the `AppState::load` site only.

### 3. Contracts

#### Background shell session_id format

- `shell_session_id` is `bsh_<uuid-simple>` (32 hex chars, no
  dashes). Stable shape — the LLM and the frontend regex-match on
  the `bsh_` prefix.
- The format is distinct from the chat session id (a plain UUID
  with dashes), so the two namespaces never collide.

#### Wire name collision: `session_id` in the LLM-facing schema

Per the L1 PRD, the `shell_status` / `shell_kill` input schemas
expose a field named `session_id`. Internally this is the
*background shell's* id, NOT the chat session id. The tool layer
translates using `ctx.background_shells` (chat-session-scoped via
the registry's `(session_id, shell_session_id)` key) + the chat
`session_id` argument always supplied by `execute_tool_inner`'s
dispatch. The rename is intentional: the LLM doesn't need to
track two distinct "session" concepts — the background shell is
just "the background shell you started".

#### `run_background_shell` execution flow

1. Parse `command` (required) + optional `working_directory` +
   optional `max_runtime_ms`.
2. Resolve the effective cwd via
   `projects::boundary::assert_within_root(&ctx.worktree_path, requested)`
   (mirrors `shell::execute`). Failure → `is_error: true` +
   plain-English rejection message so the LLM self-corrects.
3. Pull `DefaultRegistry` from `ctx.background_shells`.
4. `registry.start(chat_session_id, command, validated_cwd,
   max_runtime_ms)` returns the new `shell_session_id`.
5. Format confirmation string: `Started background shell
   bsh_xxx (cwd: <path>). Use shell_status to query progress, or
   shell_kill to terminate. When it finishes, you will see a
   \`[system] 后台 shell ... 已完成...\` message at the start of
   your next turn.`
6. `ToolContextUpdate::new_cwd = Some(validated_cwd)` so the
   agent loop persists the cwd at turn end (same UX as
   `shell::execute`).

#### Completion notification injection (the load-bearing part)

In `agent::chat_loop::run_chat_loop`, each turn AFTER C3
compaction and BEFORE `provider.send`:

```rust
let background_notifications =
    background_shells.drain_notifications(&session_id).await;
let turn_messages = {
    let checklist_snapshot = ...;
    let mut req = messages.clone();
    // ... checklist injection (unchanged) ...
    // APPEND each notification as ONE user message:
    for note in &background_notifications {
        let text = format!(
            "[system] 后台 shell {} 已完成,exit code {}。调 shell_status(session_id=\"{}\") 看输出。",
            note.shell_session_id,
            note.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".into()),
            note.shell_session_id,
        );
        req.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::Text { text, cache_control: None }]),
        });
    }
    req
};
```

- **APPEND, never prepend** — the memory breakpoint at
  `messages[0]` carries a `cache_control: Ephemeral` block; a
  per-turn-mutating notification at position 0 would bust the
  memory cache every turn (50 turns × ~100 KiB of instruction
  files = the exact cost explosion the B5 memory-caching work
  was built to eliminate). Same rule as the B12 checklist
  injection (see §"update_checklist" Scenario §7).
- **No `cache_control`** on the notification block — the block
  changes every turn, so a cache breakpoint would never hit.
- **One message per notification** — the LLM tracks multiple
  completions more reliably when they're separated.
- **Drain once per turn, not per tool_use** — the queue is
  consumed at the start of every turn; drained notifications are
  GONE from the registry.
- **Empty queue → skip injection** — fast path, no extra
  `.clone()`, no extra `push()`. The `notification count == 0`
  branch is the common case (no background shells active).

#### Permission routing (⑨ 关 Tier 4)

- `run_background_shell` → `ToolKind::Shell` (same routing as
  `shell`). The Tier 4 shell branch (kill-list + 3-tier
  `classify_prefix` + prefix grants) applies uniformly. A user's
  "始终允许" grant on `cargo` works for BOTH sync `shell` and
  async `run_background_shell`.
- `shell_status` / `shell_kill` → `ToolKind::Other` (Tier 5
  default allow). No `permission:ask` modal. Cross-session
  access still rejected by the registry's session-scoping
  (`(chat_session_id, shell_session_id)` key).
- `run_background_shell` is `Risk::High` (matches `shell`);
  `shell_status` / `shell_kill` are `Risk::Low` (read-only /
  kill-only, no new code executed).
- Plan mode filter drops `run_background_shell` along with
  `shell` / `write_file` / `edit_file` (read-only session).

#### Lifecycle hooks (3 trigger points)

| Hook | Where | Trigger |
|---|---|---|
| `kill_all_for_session(session_id)` | `commands/sessions.rs::delete_session` | User deletes the chat session; every bg shell under that session is SIGKILLed |
| `kill_all()` | `lib.rs::run`'s `RunEvent::Exit` closure | App shutdown; every bg shell across every session is SIGKILLed |
| `kill(session_id, shell_session_id)` | `shell_kill::execute` | LLM-driven kill; SIGKILLs one shell |

All three use the registry's process-group SIGKILL (RULE-E-002)
so descendants of `&` / `nohup` / pipelines are reaped along
with the direct child.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `command` missing | `is_error: true`, "Missing required parameter: command" |
| `working_directory` outside project root | `is_error: true`, "working_directory '<path>' rejected: <reason>" |
| `max_runtime_ms <= 0` | Default 86_400_000 (24h) |
| `max_runtime_ms > 600_000` | No upper clamp (PRD Q6) |
| `registry.start` spawn failure (ENOENT / EACCES) | `is_error: true`, "Failed to spawn background shell: <io error>"; `SpawnFailed` notification still queued |
| `shell_status` / `shell_kill` on unknown shell | `is_error: true`, "Background shell <id> not found" |
| `shell_status` / `shell_kill` cross-session access | `is_error: true`, "Background shell <id> is not owned by this chat session" |
| `shell_kill` on already-completed shell | `Ok(())` — idempotent no-op |
| `notification_queue` overflows past 100 entries | Oldest entry dropped + `tracing::warn!` (no panic) |
| `delete_session` with running bg shells | `kill_all_for_session` called; SIGKILLs sent synchronously, teardown async (does not block IPC) |
| App `RunEvent::Exit` with running bg shells | `kill_all` called; SIGKILLs sent before OS process reaping |
| `agent_loop` empty notification queue | Skip injection (no extra `.clone()`, no extra `push()`) |

### 5. Good / Base / Bad Cases

#### Good: build → status → fix

1. LLM calls `run_background_shell({"command": "pnpm install"})`.
2. Tool returns `Started background shell bsh_a (cwd: /repo)`.
3. LLM's turn ends (`Done{end_turn}`); agent loop persists the
   `tool_result`.
4. ~30s later, the bg shell exits. The spawned task writes
   `BackgroundShellNotification { outcome: Completed, exit_code: 0 }`
   and pushes to the per-session queue.
5. LLM sends a new message. Agent loop starts; C3 compaction
   (passthrough); `drain_notifications` returns the 1 entry.
6. Turn 1 request body: user message appended with
   `[system] 后台 shell bsh_a 已完成,exit code 0。调 shell_status(session_id="bsh_a") 看输出。`
7. LLM sees the notification, calls `shell_status`, reads the
   stdout/stderr preview + optional disk-spill path, and decides
   the next action.

#### Bad: completion arrives during turn 1

1. LLM calls `run_background_shell({"command": "echo fast"})`.
2. Tool returns immediately with `bsh_a` (echo completes in
   <50ms, but the spawned task hasn't pushed the notification
   yet).
3. Agent loop builds the `tool_result` user message and calls
   `provider.send` for turn 1.
4. The notification is queued AFTER turn 1's drain, so turn 2's
   drain picks it up. Correct behavior — turn 1's `provider.send`
   has already gone out by the time the notification lands.

#### Bad: prepend instead of append (cache invariant)

```rust
// BAD — would bust the memory cache breakpoint at messages[0]
let mut req = messages.clone();
req.insert(0, ChatMessage {
    role: Role::User,
    content: MessageContent::Blocks(vec![ContentBlock::Text { text: notification }]),
});
```

The memory breakpoint at `messages[0]` is part of Anthropic's
cache key (everything before it must be byte-identical across
turns). A per-turn-changing notification at position 0 sits
IN FRONT of the breakpoint and busts the memory cache every
turn. The fix lives in `chat_loop.rs` — the injection is
`req.push(notification_msg)` AFTER the checklist block, not
`req.insert(0, ...)`.

### 6. Tests Required

#### Backend (`cargo test --lib`)

| Test | Asserts |
|---|---|
| `tools::run_background_shell::tests::definition_has_correct_name` | Tool name |
| `tools::run_background_shell::tests::definition_documents_max_runtime` | `max_runtime_ms` field present + schema `required` is `["command"]` |
| `tools::run_background_shell::tests::execute_starts_a_background_shell` | Happy path; returns shell_id; carries cwd in `ToolContextUpdate` |
| `tools::run_background_shell::tests::execute_missing_command_returns_error` | `is_error: true` + "Missing required parameter" |
| `tools::run_background_shell::tests::execute_rejects_outside_root_cwd` | Boundary rejection |
| `tools::run_background_shell::tests::execute_without_session_id_returns_bug_marker` | Defensive no-session-id path |
| `tools::run_background_shell::tests::returned_shell_id_is_queryable_via_registry` | Round-trip: started shell is `status`-able via the same registry |
| `tools::shell_status::tests::definition_has_correct_name` / `definition_schema_requires_session_id` | Name + required |
| `tools::shell_status::tests::execute_missing_session_id_returns_error` / `execute_unknown_shell_returns_not_found_error` / `execute_without_chat_session_id_returns_bug_marker` | Error paths |
| `tools::shell_status::tests::execute_round_trip_returns_running_or_completed` | Round-trip |
| `tools::shell_status::tests::execute_cross_session_returns_error` | Q7 session-scoping |
| `tools::shell_status::tests::format_running_status_includes_id_and_elapsed` / `format_completed_status_includes_stdout_preview_and_path` / `format_killed_status_includes_exit_code` | Status string format |
| `tools::shell_kill::tests::definition_has_correct_name` / `definition_schema_requires_session_id` | Name + required |
| `tools::shell_kill::tests::execute_missing_session_id_returns_error` / `execute_unknown_shell_returns_not_found_error` / `execute_without_chat_session_id_returns_bug_marker` | Error paths |
| `tools::shell_kill::tests::execute_round_trip_kills_running_shell` | Round-trip + state transition to Killed |
| `tools::shell_kill::tests::execute_on_completed_shell_is_idempotent` | Idempotency |
| `tools::shell_kill::tests::execute_cross_session_returns_error_and_does_not_kill` | Q7 + side-effect (cross-session call leaves the shell running) |
| `permissions::tests::risk_for_tool_includes_background_shell_high` | `Risk::High` for `run_background_shell`; `Risk::Low` for `shell_status` / `shell_kill` |
| `permissions::tests::classify_tool_routes_background_shell_to_shell_kind` | Tier 4 shell branch applies uniformly |
| `agent::tests::agent_loop_drains_background_shell_notification_into_turn_2` | End-to-end: start → complete → drain → inject on next turn → provider.send carries the text |
| `agent::tests::agent_loop_no_pending_notifications_skips_injection` | Empty-queue fast path (no extra allocations, no extra push) |

Total L1a PR2 contribution: **+22 net new tests** (the 3 tool
modules' unit tests + 2 permission tests + 2 agent-loop
integration tests). Backend suite went from 651 → 680 = **+29
net new** when combined with the PR1 contributions that landed
in this PR (the additional test helpers / dispatch fixtures).

### 7. Wrong vs Correct — registry handle vs trait object

#### Wrong: `Arc<dyn BackgroundShellRegistry>` on `ToolContext`

```rust
// BAD — would force every tool to spell out the trait on the
// dispatch site and add a layer of dynamic dispatch the
// codebase doesn't use elsewhere.
pub struct ToolContext {
    pub background_shells: Arc<dyn BackgroundShellRegistry>,
}
```

The codebase's pattern for cross-request handles is concrete
`Arc<InMemory*>` (or `Arc<MemoryCache>`, `Arc<SkillCache>`,
`ReadGuard`). The future daemon-ization swap is a one-line
change at the `AppState::load` site (per L1 PRD Q1 decision C)
— no call-site churn, no `dyn`-dispatch overhead in the hot
path (every `execute_tool` call would pay for one vtable
indirection per method invocation).

#### Correct: concrete `Arc<InMemoryBackgroundShellRegistry>`

```rust
// GOOD — matches `MemoryCache::arc()` / `SkillCache::arc()` /
// `ReadGuard::new()` pattern. Daemon-ization PR swaps the
// concrete type at the `AppState::load` site only.
pub type DefaultRegistry = Arc<InMemoryBackgroundShellRegistry>;
pub fn default_registry() -> DefaultRegistry {
    Arc::new(InMemoryBackgroundShellRegistry::new())
}
```

The trait still exists as the type-erased interface the future
daemon impl will satisfy, but the production wiring holds the
concrete impl — exactly the pattern every other cross-request
handle in `AppState` uses.

## Scenario: Concurrent dispatch_subagent batch (L3a, 2026-06-24)

### 1. Scope / Trigger

- Trigger: L3a — the parent LLM emits **≥2 `dispatch_subagent` tool_use in one turn** (a "pure
  dispatch batch") and expects them to run concurrently, the parent turn blocking until all complete.
  Aligns with Hermes default-foreground `delegate_task` (ThreadPoolExecutor fan-out, parent blocks)
  and Claude Code `Agent` (multiple `Agent` tool_use run concurrently per turn). Replaces the B6 MVP
  serial "one worker at a time" path (chat_loop.rs ~:1697 self-ack "parallel fan-out is v2 / L3").
- Why code-spec depth: mandatory — first **multi-worker concurrent** control-flow path. The batch
  classifier, the read-only enforcement, the hard-reject limit, and the order-preservation contract
  are all executable. Critically, the **3 race conditions** (permission:ask / token usage /
  cancellations) are provably dissolved by the existing architecture in the read-only scope — that
  proof is itself a load-bearing contract future edits must not violate (full derivation in
  `agent-loop-architecture.md §"Pattern: Concurrent readonly dispatch"`).

### 2. Signatures

#### Batch classifier (chat_loop.rs)

```rust
enum DispatchBatch { Serial, OverLimit { count: usize, max_concurrent: usize }, Concurrent { count: usize } }
fn classify_dispatch_batch(tool_calls: &[(String, String, Value)]) -> DispatchBatch;
//   d = count(name == DISPATCH_TOOL_NAME); o = count(everything else)
//   d == 0                       → Serial   (not a dispatch batch; L2 read-only path handles it upstream)
//   d >= 2 && o == 0 && d <= MAX → Concurrent { d }
//   d >  MAX                     → OverLimit { d, MAX }
//   else (d == 1 || o > 0)       → Serial   (single dispatch, or mixed batch → existing serial loop)
```

#### Read-only enforcement (subagent/mod.rs)

```rust
// web_fetch is read-only (network fetch, no file mutation) so it is kept
// in the concurrent-branch allowlist; the 4 file-reading tools cover the
// on-disk read surface. See `READONLY_TOOL_ALLOWLIST` doc in
// `app/src-tauri/src/agent/subagent/mod.rs:546` for rationale.
const READONLY_TOOL_ALLOWLIST: &[&str] = &["read_file", "grep", "glob", "list_dir", "web_fetch"];
pub fn filter_tools_readonly(tools: Vec<ToolDef>) -> Vec<ToolDef>;   // mirrors STRUCTURALLY_DISABLED pattern

// run_subagent gained a trailing param:
pub(crate) async fn run_subagent(/* …existing params… */, force_readonly: bool)
    -> (String, /* is_error */ bool, /* cancel_parent */ bool, Option<i32>);
//   force_readonly == true  → apply filter_tools_readonly AFTER filter_tools_for_subagent
//   force_readonly == false → unchanged (serial path keeps B6 behavior)
```

#### Concurrency primitive

`FuturesUnordered` + `result_slots: Vec<Option<ContentBlock>>` (pre-allocated to N, each task writes
its own index) + `Arc<AtomicBool>` shared `cancelled_flag` — **structurally mirrored from the L2
read-only-batch parallel path** (chat_loop.rs ~:1439-1639); the only per-task difference is the body
calls `run_subagent(force_readonly=true)` instead of `execute_tool`.

### 3. Contracts

- **Pure-batch gate**: only `d >= 2 && o == 0` enters the concurrent branch. A mixed batch
  (`dispatch_subagent` + `read_file` in the same turn) falls through to the serial path unchanged.
- **Order preservation**: `tool_result` blocks re-collapse into `result_blocks` in **tool_use
  order** via `result_slots[i]`, NOT completion order. Streaming `emit_tool_result` fires in
  completion order (frontend UX); the LLM context sees tool_results in tool_use order (matches L2).
- **Read-only guarantee — 3 layers**: (1) `SubagentDef` allowlist (`researcher` = the 5 read-only
  tools `read_file, grep, glob, list_dir, web_fetch`; `general-purpose` = empty = full set);
  (2) **runtime strip** (L3a): concurrent branch passes
  `force_readonly=true` → keeps only `[read_file, grep, glob, list_dir, web_fetch]` — `researcher`
  no-op, `general-purpose` downgraded; (3) `is_worker: true` (B6 PR2b, pre-existing safety
  floor): even if a write tool slipped through, the worker's Tier 4 `ask` collapses to
  `Decision::Deny`.
- **Hard reject on over-limit** (Hermes alignment): `d > DELEGATION_MAX_CONCURRENT_CHILDREN` → every
  `dispatch_subagent` returns an `is_error: true` tool_result ("exceeded concurrent delegation
  limit"); **0 workers spawn** (no truncation, no queueing, no partial execution).
- **Cancel aggregation**: `cancelled_flag` set by any task whose worker returned `cancel_parent=true`
  (parent Stop reached it) OR detected `token.is_cancelled()`; after the join, the main loop flips
  its local `cancelled`.
- **`run_subagent` single source of truth**: L3a adds the `force_readonly` param (4-line filter)
  rather than duplicating the ~450-line function — duplication is the faithful-port drift hazard
  (`agent-loop-architecture.md §"Anti-pattern: faithful port as a drift hazard"`). Serial call site
  passes `false`; B6 single-dispatch behavior byte-for-byte unchanged.

#### Environment keys

| Key | Default | Purpose |
|---|---|---|
| `DELEGATION_MAX_CONCURRENT_CHILDREN` | `3` | max concurrent workers per dispatch batch; mirrors Hermes `_DEFAULT_MAX_CONCURRENT_CHILDREN`. Read per-call (no cache) so tests can override in-process. Non-integer / missing → falls back to 3. |

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| Pure batch, d=3 (≤ limit) | 3 workers spawn concurrently; 3 tool_results in tool_use order |
| Pure batch, d=4 (> limit 3) | 4 tool_results all `is_error: true`; 0 workers spawn |
| Mixed batch (dispatch + read_file) | Falls to serial path; serial loop runs dispatch (B6) + read_file in order |
| Single dispatch (d=1) | Serial path (B6 behavior, `force_readonly=false`) — unchanged |
| `general-purpose` in concurrent branch | worker toolset stripped to 5 read-only tools (`read_file, grep, glob, list_dir, web_fetch`); writes impossible |
| Parent Stop mid-batch | `parent_token` fires → all N `child_token()` fire → all workers cancelled; `cancel_parent` aggregated |
| One worker errors, others succeed | each returns its own `(content, is_error, …)`; tool_results carry per-worker `[status: …]` prefix independently |

### 5. Good / Base / Bad Cases

- **Good**: parent emits 3 `dispatch_subagent{researcher, …}` for 3 topics → 3 workers run
  concurrently (wall-clock ≈ max(single), not sum) → 3 tool_results in tool_use order.
- **Base**: parent emits 3 dispatch but one is `general-purpose` → all 3 still concurrent; the
  `general-purpose` worker is silently downgraded to read-only (its writes would be Deny'd anyway).
- **Bad (anti-pattern the gate prevents)**: parent emits 5 dispatch (> limit 3) hoping to fan out →
  hard-rejected, 0 spawn, parent told to reduce count or raise the env limit. No silent truncation
  (which would make the parent think 3 ran when it sent 5).

### 6. Tests Required

Backend (`cargo test --lib`, `agent/tests_subagent.rs`):

| Test | Asserts |
|---|---|
| `l3a_filter_tools_readonly_keeps_only_five_read_tools` | unit: allowlist keeps 5 read-only tools (`read_file, grep, glob, list_dir, web_fetch`), strips writes incl. `dispatch_subagent` (anti-nesting pin) |
| `l3a_classify_dispatch_batch_branches_correctly` | unit: all 3 branches (Serial/OverLimit/Concurrent) classified by (d, o) |
| `l3a_pure_batch_of_three_dispatches_runs_concurrently` | AC1/6: 3 workers, tool_use order preserved (asserted via persisted DB messages) |
| `l3a_pure_batch_over_limit_hard_rejects_all` | AC3: 4 dispatch → all tool_error, 0 workers (call_count, runs empty) |
| `l3a_concurrent_general_purpose_workers_complete_readonly` | AC2: general-purpose in concurrent branch stripped to read-only |
| `l3a_concurrent_cancel_propagates_to_all_workers` | AC4: parent cancel → 3 cancelled tool_results + 3 cancelled runs + parent Done{cancelled} |
| `l3a_concurrent_token_usage_folds_into_parent` | AC5: 3 workers' usage folds into parent `sessions.*_total` (atomic increment invariant) |
| `l3a_mixed_batch_falls_through_to_serial_path` | AC7: dispatch + read_file → serial path |
| `l3a_single_dispatch_runs_serial_path_unchanged` | regression: d=1 → B6 serial behavior, `force_readonly=false` |

### 7. Wrong vs Correct — concurrency race handling

#### Wrong: add explicit locks / channels for the 3 race points

```rust
// BAD — re-inventing concurrency control the existing architecture already provides.
let permit = Arc::new(Semaphore::new(MAX));   // ← the batch size IS the gate (hard-reject handles over-limit)
let ask_mutex = Arc::new(Mutex::new(()));     // ← worker is_worker=true collapses ask to Deny; no concurrent ask exists
let usage_mutex = Arc::new(Mutex::new(()));   // ← add_token_usage is col = COALESCE(col,0)+? atomic SQL; no read-modify-write
```

The 3 race points are **dissolved by scope**, not by new synchronization (full derivation in
`agent-loop-architecture.md §"Race dissolution by scope"`):
1. `permission:ask` — worker `is_worker=true` → Tier 4 `ask` → `Decision::Deny` (no oneshot wait);
   read tools are low-Tier silent-allow. **No concurrent interactive ask can occur.**
2. `token usage` — `add_token_usage` / `add_token_usage_streaming` are `col = COALESCE(col,0) + ?`
   atomic increment; SQLite's single-writer lock serializes. **No lost updates.**
3. `cancellations` — each worker registers a unique `worker_rid = "{parent_rid}-sub-{tool_use_id}"`;
   `worker_token = parent_token.child_token()` × N → parent cancel fires all children. **Free fan-out.**

#### Correct: reuse the existing architecture + the L2 parallel template

```rust
// GOOD — the concurrent branch is the L2 read-only-batch path with run_subagent(force_readonly=true)
// in place of execute_tool. No new locks; the read-only scope IS the safety argument.
let result_slots: Vec<Option<ContentBlock>> = (0..n).map(|_| None).collect();
let cancelled_flag = Arc::new(AtomicBool::new(false));
let mut fu: FuturesUnordered<_> = dispatches.enumerate().map(|(i, (id, input))| async move {
    let (content, is_error, cancel_parent, _) =
        run_subagent(/* …shared-ref deps… */, /*force_readonly=*/ true).await;
    if cancel_parent { cancelled_flag.store(true, Ordering::SeqCst); }
    Some((i, ContentBlock::ToolResult { tool_use_id: id, content, is_error }))
}).collect();
while let Some(Some((i, block))) = fu.next().await { result_slots[i] = Some(block); }
let result_blocks = result_slots.into_iter().flatten().collect();
```

> **Invariant to preserve on any future edit**: if the concurrent branch is ever widened to allow
> write-capable workers (L3b + worktree), the race-dissolution proof above **breaks** — at minimum,
> `permission:ask` (now a real concurrent interactive modal) and token-usage contention must be
> re-evaluated. Do NOT lift the read-only strip without worktree + re-deriving the safety argument.

### `dispatch_subagent` worktree-isolation input (L3b PR1, 2026-06-27)

L3b PR1 extends `dispatch_subagent` with a `isolation: Option<bool>` input parameter and a matching `SubagentDef.isolation: Option<bool>` frontmatter field. The merge semantics (`resolve_isolation(frontmatter_default, dispatch_input) -> bool` in `agent/subagent/dispatch.rs`):

| frontmatter `isolation` | dispatch `isolation` | result |
|---|---|---|
| `Some(true)` | not specified | isolated |
| `Some(true)` | `Some(false)` | shared (LLM opted out) |
| `Some(false)` or `None` | `Some(true)` | isolated (LLM opted in) |
| `Some(false)` or `None` | not specified | shared (legacy behavior) |
| `Some(false)` or `None` | `Some(false)` | shared |
| `Some(true)` | `Some(true)` | isolated |

Precedence: **dispatch input > frontmatter default > not isolated**.

#### Builtin defaults

- `general-purpose`: `isolation: Some(true)` — write-capable workers benefit most from isolation (concurrent dispatch conflict isolation is the core L3a → L3b motivation).
- `researcher`: `isolation: None` — read-only workers don't need a separate checkout; saves the per-dispatch checkout cost.

#### Tool schema addition

```json
{
  "isolation": {
    "type": "boolean",
    "description": "Override the subagent's worktree-isolation decision for THIS dispatch only. When `true`, the worker runs in its own git worktree on branch `worker/<run_id>`; when `false`, the worker reuses the parent session's checkout (legacy B6 behavior). Precedence: this input overrides the subagent's frontmatter default. See `agent-loop-architecture.md` §worktree_override + `worktree-contract.md` §Worker Worktree Variant for the runtime behavior."
  }
}
```

### L3b PR1 update on the concurrent dispatch warning above

The "concurrent branch write-capable" warning above is **partially addressed** by PR1 (worker worktree isolation exists; the runtime plumbing is in place) but **not fully closed** — the `chat_loop.rs` concurrent dispatch branch still strips `force_readonly=true` (L3a behavior unchanged). The full `force_readonly → 各 worker worktree` switch is **L3b PR2**, a follow-up task. The race-dissolution proof in this spec is therefore still valid only for the L3a read-only strip; the proof must be re-derived when L3b PR2 lands.
