# Tool Contract —工具定义 + ReadGuard + Bash Spillover + ⑨ 关

> **基线**:2026-06-13(PR1 + PR3 of `06-12-a2-b7-permission-and-mode`)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + Extended Thinking + 反模式汇总 + ⑨ 关 IPC 协议
> - [tool-contract.md](./tool-contract.md) (本文) —工具定义 + ReadGuard + shell spillover + ⑨ 关 5-tier 决策合约
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `builtin_tools()` / `edit_file` / `ReadGuard` / `shell` spillover / `grep` / `glob` / `list_dir` 时。

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
 description: "Replace exact text in a file. old_string must match byte-for-byte
 including whitespace. If not unique, pass replace_all: true or include more
 surrounding context. The file must have been read via read_file in this
 session — ReadGuard enforces read-before-edit and on-disk freshness.",
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
 pub mtime: SystemTime,
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
| File not read in this session | `edit_file` | `"You must read_file <path> first."` (is_error: true) |
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
5. Tool result: `"Successfully edited <path>:1 occurrence replaced."`.

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

###2. Signatures

#### Tool declaration (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/web_fetch.rs
ToolDef {
 name: "web_fetch",
 description: "Fetches content from a URL and returns it as markdown \
                (default), plain text, or raw HTML. Supports HTTP and \
                HTTPS only. Refuses private/loopback/link-local addresses \
                to prevent SSRF.",
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
- **T2f (redirect to private IP)** — Med severity, MVP accepted risk. We use
  reqwest's default redirect limit (5) and let reqwest follow; the per-redirect
  IP check is not implemented. The cost of implementing it is a custom redirect
  policy; the benefit is a tighter SSRF defense on the LLM.
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
/// (built once per run_chat_loop call ~chat_loop.rs:216), so run_chat_loop's 14-param
/// signature is UNCHANGED (no 9+ agent_loop_* test call-site sync needed).
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

#### Tool declaration(`app/src-tauri/src/agent/subagent.rs::definition()`,注册进 `builtin_tools()`)

```rust
ToolDef {
    name: "dispatch_subagent",
    description: "Dispatch a worker subagent to run a sub-task in its own isolated context \
                  (independent messages, independent turn budget). The worker runs to \
                  completion (synchronous — the parent chat blocks until the worker returns). \
                  When the worker finishes, its final summary is injected as the tool_result. \
                  Two built-in subagents: `researcher` (read-only: read_file / grep / glob / \
                  list_dir) and `general-purpose` (full toolset minus dispatch_subagent / \
                  update_checklist / background-shell tools). Worker inherits parent's \
                  permission Mode (Yolo → all-allow; Edit/Plan → writes / shells auto-denied \
                  because the worker has no UI to surface a permission modal).",
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

#### `run_chat_loop` 嵌套调用(worker 路径,`chat_loop.rs:1940`)

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
    Some(SUBAGENT_MAX_TURNS),                // 18: 20
    true,                                    // 19: skip_session_active(REVIEW-SUBAGENT-PRD #2)
    true,                                    // 20: skip_persist(worker 中间过程不进 DB)
)).await;
```

#### `run_chat_loop` 3 个新参数(PR1a + PR1b)

| # | 参数 | PR | 用途 |
|---|---|---|---|
| 18 | `max_turns: Option<usize>` | PR1a | worker turn 上限(None = 50 默认)。`turn_limit = max_turns.unwrap_or(MAX_TURNS)` |
| 19 | `skip_session_active: bool` | PR1b | `CancellationGuard::drop` 时跳过 `session_active_request.remove(session_id)`。worker 传 `true` 避免误删父映射(REVIEW-SUBAGENT-PRD #2 / RULE-E-005 不破坏) |
| 20 | `skip_persist: bool` | PR1b | run_chat_loop 函数体内 18 处 `if !skip_persist { ... }` gate 守住所有 persist 站点(persist_turn / update_message_metadata / touch_session / add_token_usage / record_*_audit / persist_turn_cwd)。worker 传 `true` 避免与父 `messages` 表 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程由 `SubagentBufferSink` transcript 捕获(PR2 落 `subagent_runs.transcript_json`) |

### 3. Contracts

#### Built-in `SubagentDef` registry(`subagent.rs::builtin_subagents`,OnceLock 缓存)

| name | `tools` allowlist | system_prompt |
|---|---|---|
| `researcher` | `[read_file, grep, glob, list_dir]` | "你是只读研究子代理...Cannot edit/write/shell,不能嵌套 dispatch..." |
| `general-purpose` | `[]`(全集减结构性禁项) | "你是通用子代理...minus dispatch_subagent / update_checklist / background-shell..." |

#### `filter_tools_for_subagent(builtin_tools, def)`(`subagent.rs`)

1. `def.tools.is_empty()` → 起点全集(general-purpose 模式);否则 → 起点 allowlist
2. **`STRUCTURALLY_DISABLED` 永远 strip**(无论 allowlist 怎么写):
   - `update_checklist`(main 进度表,worker 写会污染)
   - `dispatch_subagent`(禁嵌套,对标 Cline)
   - `run_background_shell` + `shell_status` + `shell_kill`(L1a session 级通知注入,worker 无 sink)
3. 测试 `filter_strips_structurally_disabled_even_if_allowlist_lists_them` 锁定(防御未来 frontmatter 定义误开禁项)

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

#### `format_dispatch_result(status, worker_text)`(`subagent.rs`)

| status | content | is_error |
|---|---|---|
| `Completed` | `[status: completed]\n<summary>`(空文本回退 `(worker produced no final text)`) | false |
| `Cancelled` | `[status: cancelled]\n<text>\n\n[CANCELLED_MARKER]`(空文本退化为 `marker alone`) | true |
| `Error` | `[status: error]\n<error text>` | true |

terminal `Done{cancelled}` 事件**不**守 `skip_persist` —— worker `SubagentBufferSink.was_cancelled` 仍能正确捕捉,只 DB writes(cwd / touch_session)守门。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `subagent` 不在 enum | LLM 错(input schema 校验拦在前面) |
| `lookup_subagent(name)` 返 None | 拦截点合成 `tool_result` `[status: error]\nunknown subagent`,`is_error: true`,**tool_use/tool_result 配对保持**(同 RULE-A-007) |
| worker turn 超 `SUBAGENT_MAX_TURNS=20` | `Done{stop_reason: max_turns}` → status=Completed(soft,"ran out of budget"),summary 仍带 worker 产出 |
| 用户 Stop 传播到 `worker_token`(child of `parent_token`) | `Done{stop_reason: cancelled}` → status=Cancelled + `CANCELLED_MARKER` |
| worker LLM stream error | `ChatEvent::Error` → SubagentBufferSink.had_error → status=Error |
| parent 复用 `session_id` + guard `skip_session_active=true` | worker Drop **不** evict 父 `session_active_request[parent_session_id]`(回归测试 `dispatch_subagent_guard_does_not_evict_parent_session_active`) |
| worker 内写 messages 表(`skip_persist=true`) | 18 处 gate 全部拦下,worker 中间过程不进父 DB |

### 5. Good / Base / Bad Cases

**Good**:parent turn 1 LLM 派 `researcher`("找出所有引用 `dispatch_subagent` 的文件")→ researcher 跑 `read_file`/`grep`/`list_dir`(4 路径全 silent allow)→ final text "found 3 files: ..." → `format_dispatch_result(Completed, ...)` → parent 构造 `ContentBlock::ToolResult`,tool_use/tool_result 配对,parent turn 2 继续。

**Base**:parent turn 1 LLM 派 `general-purpose` 改文件 + main=yolo(继承 yolo,写/shell Tier 4 bypass 早返回 Allow)→ worker 跑 `write_file` + `shell`(无 ask modal 阻塞)→ final text "已修改 3 个文件: ..." → Completed。

**Bad**:parent turn 1 LLM 派 `general-purpose` + main=Edit + `write_file`(触发 Tier 4 ask)→ **当前实现 (RULE-A-014 偏离)**:`PermissionContext.is_worker` 未 thread 到嵌套 run_chat_loop → `ask_path` 顶部 `if ctx.is_worker { Deny }` 在嵌套路径不可达 → emit `permission:ask` 等 oneshot(永远等不到,worker 无 UI sink)→ **挂起直到 user Stop**。修复见 `RULE-A-014`(PR2+ follow-up,~10 行 + 1 端到端测试)。

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

**Integration**(`agent/tests.rs::agent_loop_dispatch_subagent_*`,4 个):

| Test | 断言 |
|---|---|
| `dispatch_subagent_completes_and_returns_summary` | parent turn 1 dispatch_subagent tool_use → worker 跑 → summary tool_result `[status: completed]` + worker text;主对话 `phantom_worker_text == 0`(worker 中间过程**不**进父 messages) |
| `dispatch_subagent_cancel_propagates_to_worker` | parent_token cancel → worker_token child 触发 → status=cancelled + `CANCELLED_MARKER`;tool_use/tool_result 配对保持 |
| `dispatch_subagent_error_returns_status_error` | MockProvider stream error → status=error;tool_use/tool_result 配对保持 |
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

#### Decision: `skip_persist` 守 18 处 persist 站点(worker 中间过程不进 DB)

- **Context**: worker 复用 parent_session_id,直接调 `persist_turn` 会与父的 `(session_id, seq)` UNIQUE 约束冲突;worker 中间过程对父 messages 透明是核心约定。
- **Decision**: 加第 20 参 `skip_persist: bool`,run_chat_loop 函数体内 18 处 persist 调用全部包 `if !skip_persist { ... }`(initial user / resend audit / metadata / token usage / assistant turn / cancel-synthetic / parallel+serial tool_executed_audit / tool_result / max_turns / cwd / touch_session / etc.)。
- **Why 不在拦截点单独守门**:worker 调用 `run_chat_loop` 嵌套后,函数体本身不知道自己是 worker;把守门推到函数体内**单一权威**(对齐 RULE-A-006 单一权威),每个 persist site 一目了然。

---

## Scenario: ⑨ 关 Permission Decision Layer (A2 + B7 PR1, 2026-06-13)

> **Source of truth**: the 5-tier evaluation order lives in
> `app/src-tauri/src/agent/permissions.rs` (PR1 implementation) +
> `app/src-tauri/src/agent/permissions/dangerous.rs` (Tier 2 hard
> kill list). The IPC surface is `app/src-tauri/src/commands/permissions.rs`.
> PR3 (2026-06-13) wired the frontend `usePermissionsStore` +
> `<PermissionModal>` to consume the IPC; the spec cross-references
> `.trellis/spec/frontend/state-management.md §"Permissions store
> + PermissionModal IPC bridge"` for the TS side.

### 1. Scope / Trigger

The ⑨ 关 is the unified decision point between the agent
loop's `provider.send()` stream and `tools::execute_tool`. On
every `tool_use` block, the agent loop calls
`permissions::check()` and uses the returned `Decision` to
either execute the tool, skip it (with an `is_error: true`
tool_result for the LLM to self-correct), or await user
input. This is the only place tool execution is gated; the
agent loop MUST NOT call `execute_tool` without first calling
`permissions::check` for every tool_use.

The full 5-tier order + the ⑧a system-prompt / tool-list
defenses are documented in
`llm-contract.md §"Per-Session Mode + ⑨ 关 Permission Layer"`.
This file's section is the **tool-side** contract: the
`execute_tool` signature, the hard kill list contents, the
`session_tool_permissions` schema, and the audit log payload.

### 2. Signatures

#### `execute_tool` (gated wrapper, PR1 改造后)

```rust
// app/src-tauri/src/agent/chat.rs: ⑨ 关 dispatch 入口
let decision = permissions::check(
    &ctx,                  // PermissionContext { session_id, mode }
    &state.permission_asks, // PermissionStore
    &state.db,
    &app_handle,
    tool_name,
    tool_input,
    &cancel_token,         // C1 cancellation
).await;

match decision {
    Decision::Allow => execute_tool(name, input, &ctx).await,
    Decision::Deny { reason, critical: _ } => {
        // 构造 is_error: true tool_result 回 LLM
        // 不触发 CancellationToken
        return ToolResult { is_error: true, content: reason };
    }
    Decision::Ask { reason: _, risk: _ } => unreachable!(),
    // ⑨ 关 在 check() 内部 collapse Ask → Allow / Deny
}
```

The actual `tools::execute_tool(name, input, ctx)` (no
permission args) is unchanged from PR1. The `permissions::check`
wrapper is the new layer.

#### `Risk` enum

```rust
// app/src-tauri/src/agent/permissions.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk { Low, Medium, High, Critical }
```

`Critical` is reserved for future use (PR3's PermissionModal
already reads it from the wire payload, but the per-tool
static map only returns Low / Medium / High in the MVP).
Serializes lowercase to match the frontend TS type
(`"low" | "medium" | "high" | "critical"`).

#### Per-tool risk map

```rust
// app/src-tauri/src/agent/permissions.rs
pub fn risk_for_tool(tool_name: &str) -> Risk {
    match tool_name {
        "shell" => Risk::High,
        "write_file" | "edit_file" => Risk::Medium,
        _ => Risk::Low,  // read_file / grep / glob / list_dir / web_fetch
    }
}
```

#### `session_tool_permissions` schema (PR1 新增)

```sql
CREATE TABLE session_tool_permissions (
    session_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    match_kind TEXT NOT NULL CHECK (match_kind IN ('tool', 'prefix', 'path')),
    match_value TEXT,           -- NULL for 'tool', command-prefix or glob otherwise
    granted_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (session_id, tool_name, match_kind, match_value),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
```

MVP only writes `match_kind = 'tool'`, `match_value = NULL`. The
`prefix` / `path` variants are reserved in the schema (per
`research/permission-modal-ux.md §Q4 "持久化方案"` 决策) but
the decision-matching code in `permissions::check` only checks
the `tool` variant today. `ON DELETE CASCADE` requires
`PRAGMA foreign_keys = ON` at connection init (verified in
`db/mod.rs`).

#### `session_audit_events` schema (PR1 新增)

```sql
CREATE TABLE session_audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    ts TEXT NOT NULL DEFAULT (datetime('now')),
    kind TEXT NOT NULL,           -- AuditKind as_str() output
    payload_json TEXT,            -- JSON object (see §6 audit payload)
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX idx_session_audit_events_session_ts
    ON session_audit_events(session_id, ts DESC);
```

### 3. Hard kill list (Tier 2)

The kill list lives in
`app/src-tauri/src/agent/permissions/dangerous.rs`. Static
match — pure function `is_kill_listed(tool_name, input) ->
Option<String>` returns the trigger reason when the tool
input matches a hard-coded dangerous pattern. MVP triggers:

| Command | Pattern |
|---|---|
| `rm -rf /` | `^rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)*/\s*$` |
| `rm -rf /*` | `^rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)*/\*` |
| `mkfs` | `\bmkfs\b` (any arg) |
| `dd if=` | `\bdd\b.*\bif\s*=` |
| fork bomb | `:\(\)\s*\{.*:\|:&.*\};:` |
| `> /dev/sda` | `>\s*/dev/(sda\|nvme\d)` |
| `chmod -R 777 /` | `\bchmod\s+(-[a-zA-Z]*R[a-zA-Z]*\s+)*777\s+/` |
| `git push --force` / `git push -f` 主分支 | `\bgit\b.*\bpush\b.*(\-\-force\|-f\b)` |
| `curl ... \| bash` | `\bcurl\b.*\|\s*(ba)?sh\b` |
| `wget ... \| bash` | `\bwget\b.*\|\s*(ba)?sh\b` |

All shell patterns apply **only** to `tool_name == "shell"`.
`write_file` / `edit_file` are NOT subject to the command
patterns (they have their own path-boundary enforcement via
`projects::boundary::assert_within_root`, already in place
from step 1).

### 4. `permission:ask` IPC protocol (cross-reference llm-contract.md)

**Wire shape** (server → client, camelCase per Rust
`#[serde(rename_all = "camelCase")]` on
`PermissionAskPayload`):

```jsonc
{
  "rid": "550e8400-e29b-41d4-a716-446655440000",
  "sessionId": "sess-abc",
  "toolUseId": "toolu_01ABC",
  "toolName": "shell",
  "toolInput": { "command": "ls -la" },
  "risk": "high",
  "reason": "The tool shell requires your confirmation (risk: 高)."
}
```

`sessionId` (2026-06-16, inline approval card) routes the ask to the
right session in the frontend's per-session `pendingBySession` map —
this fixes the multi-session concurrency bug where asks from different
sessions collided on a single global slot. `toolUseId` lets the inline
`<ToolCallCard>` match `call.id === toolUseId` to render the approval
state on the exact card that triggered the ask. `path` is also present
for path tools (omitted for shell / web_fetch — see
`PermissionAskPayload`'s `#[serde(skip_serializing_if)]`).

**IPC command** (client → server):
`invoke("permission_response", { rid, decision, reason })` where
`decision` is one of `"allow_once"` / `"allow_always"` /
`"deny"`. `reason` (2026-06-16) is the user's optional "拒绝并说明"
feedback — only meaningful for `"deny"` (pass `undefined` otherwise);
the agent loop surfaces it as the `tool_result(is_error)` content so
the LLM learns *why* it was denied. The Tauri command lives in
`commands::permissions::permission_response` and looks up
the `HashMap<rid, oneshot::Sender>` keyed by `rid`. Unknown
`decision` returns `Err`; unknown `rid` returns `Ok(false)`
(best-effort no-op, NOT an error).

### 5. Boundary check integration (⑨ 关 Tier 0 / pre-check)

The `projects::boundary::assert_within_root` check (documented
in `project-cwd-boundary.md`) is the project's Tier 1 hard
guard. PR1 ⑨ 关 integrates with it as the **first** check
even before Tier 1 Hooks:

```rust
// In agent/chat.rs: L70-71, before permission::check
// (this is the existing step-1 boundary check, unchanged
// from before PR1)
let effective_cwd = input.get("working_directory")
    .and_then(Value::as_str)
    .map(Path::new)
    .unwrap_or(&ctx.cwd);
let validated_cwd = boundary::assert_within_root(&ctx.project_root, effective_cwd)?;
```

A boundary violation is a Tier 0 hard error (not a ⑨ 关
Decision) — the agent loop bails out before any tool
execution. This preserves the 7-edge-case contract in
`project-cwd-boundary.md` §2.

### 6. Audit payload shape (10 类 AuditKind)

`permissions::record_audit` writes the audit row with a
uniform payload JSON shape:

```json
{
  "tool_name": "shell",
  "tool_input": { "command": "ls -la" },
  "reason": "matches denylist: rm -rf /",
  "mode": "edit",
  "critical": true
}
```

`critical: true` is set ONLY for Tier 2 hard-kill denials
(where the kill list is intrinsically catastrophic). Tier 3
user-deny / timeout / cancel paths are `critical: false`
(the user opted out, nothing catastrophic; the LLM needs
to self-correct, not be told "the world is ending").

The 10 `AuditKind` variants are listed in
`llm-contract.md §6`. Audit write failures are best-effort
(`tracing::warn!` + continue) — they MUST NOT break the
agent loop.

### 7. Tests Required (PR1 已有)

| Test | Asserts |
|---|---|
| `permissions::tests::risk_for_tool_categorization` | per-tool static map correct |
| `permissions::tests::risk_label_cn_is_full_text` | 中文 label 完整 |
| `permissions::tests::audit_kind_round_trip` | 10 类 AuditKind 都 serializable |
| `permissions::tests::filter_tools_for_mode_drops_writes_in_plan_review` | ⑧a tool filter |
| `permissions::dangerous::tests::kill_list_blocks_rm_rf_root` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_fork_bomb` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_mkfs` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_dev_sd_write` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_normal_dev_commands_pass` | Tier 2 不误杀 |
| `permissions::dangerous::tests::kill_list_does_not_block_normal_rm` | Tier 2 不误杀正常 rm |
| `permissions::dangerous::tests::kill_list_blocks_chmod_777_root` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_dd` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_git_push_force_protected` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_blocks_curl_pipe_shell` | Tier 2 命中 |
| `permissions::dangerous::tests::kill_list_empty_command_passes` | 空 input 不误杀 |
| `permissions::dangerous::tests::kill_list_only_checks_shell` | kill list 仅作用于 shell,不影响 edit_file |

(总计 20 个 PR1 落地的 permission 测试,见
`cargo test --lib permissions` 输出。)

PR3 不新增 backend 测试 — ⑨ 关 backend 行为已在 PR1 锁定;
PR3 全部新增是 frontend store + modal 测试,见
`llm-contract.md §8 "Tests Required" / Frontend`。

---

## Scenario: Path-based Permission Layer (A2+B7 re-grill, 2026-06-13)

> **Source of truth**: this scenario supersedes the Tier 3 / Tier
> 4 ordering in the previous "⑨ 关 Permission Decision Layer"
> section above. The hard-kill-list (Tier 2) is INVARIANT —
> `dangerous::is_kill_listed` is untouched. The re-grill only
> restructures Tier 3-5 + adds path-based dispatch in Tier 4.
> The new file `app/src-tauri/src/agent/permissions/shell_trust.rs`
> is a Tier 4 helper. The IPC wire shape is **backward-
> compatible** — `PermissionAskPayload` gained a `path` field
> with `skip_serializing_if = "Option::is_none"`.
>
> **Update 2026-06-14 (shell 三档分类)**: `ShellTrust` 从 2 档
> (Allow/Ask) 拆成 3 档 (ReadOnly/SideEffect/Ask),shell 的 Mode
> 感知从 Tier 3 下沉到 Tier 4 (见下 §1/§2)。Tier 3 现在只拦
> write_file/edit_file。IPC wire 形状不变。

### 1. New 5-Tier Order (re-grill SOT)

```
Tier 1. Hooks           (MVP no-op)
Tier 2. Deny rules      (硬 kill list,shell 9 个 regex,Yolo 走)
Tier 3. Mode check      (Plan 拦截 write_file/edit_file,text 错,不发 modal;shell 不在此层)
Tier 4. Path / Prefix / External policy
       ├─ Path 工具:is_within_root → 查 session_tool_permissions
       │   (match_kind='path') → hit Allow / miss silent(in) / miss ask(out)
       ├─ Shell (三档 2026-06-14):先查 prefix grant → Allow;否则 classify_prefix →
       │     ReadOnly(纯读,含 git diff/log/status 等只读子命令) → Allow(Plan 也静默)
       │     SideEffect(可恢复副作用:mkdir/git push/cargo) → Plan:ask / Edit:Allow
       │     Ask(高危/未知/含 |,&&,;) → ask(Plan & Edit)
       └─ Web Fetch:查 match_kind='tool' for 'web_fetch' → hit Allow / miss ask
       (Yolo:整段 bypass,直接 Allow;Tier 2 仍 hard wall)
Tier 5. Allow rules     (default allow-all)
Tier 6. Audit           (写 session_audit_events)
```

### 2. New files

| File | Lines | Purpose |
|---|---|---|
| `app/src-tauri/src/agent/permissions/shell_trust.rs` | ~250 + tests | 三档分类(2026-06-14):`READ_ONLY_WHITELIST`(~40) + `SIDE_EFFECT_WHITELIST`(~30) + `SHELL_ASKLIST`(~30 参考) + `GIT_READONLY_SUBCOMMANDS`(~23) const tables + `classify_prefix`(结构降级:含 `\|`/`&&`/`;` → Ask + git 子命令细化 + 通用表;asklist 仍为参考表,未命中前两表即 Ask) |
| `app/src-tauri/src/projects/boundary.rs` (extension) | +20 | `is_within_root` non-failing boolean (no canonicalize) |
| `app/src-tauri/src/agent/permissions/mod.rs` (rewrite) | +200 -120 | 5-tier reorder + path dispatch + Tier 4 helpers |

### 3. Wire compat

- `PermissionAskPayload`: added `path: Option<String>`, `#[serde(skip_serializing_if = "Option::is_none")]` — old PermissionModal code that doesn't read `path` still works.
- `commands::permissions::grant_tool_permission` Tauri command: added optional `match_kind` + `match_value` args; defaults to old behavior (`tool` / `None`) when omitted.

### 4. Match-kind table (session_tool_permissions)

| match_kind | match_value | Use case | Persistence trigger |
|---|---|---|---|
| `tool` | NULL | `web_fetch`, or any future tool with no path/prefix | Tier 4 "始终允许" on web_fetch / future tools |
| `prefix` | first whitespace token | Shell commands (`cargo`, `git`, …) | Tier 4 "始终允许" on shell |
| `path` | parent + `/*` glob | Path tools (`/Users/me/Documents/*`) | Tier 4 "始终允许" on read/write/edit/list_dir/grep/glob |

### 5. Out of scope (re-grill ⑨ 关)

- shell 白名单/asklist UI 自定义:用户增删 — 留 PR3+
- 跨 session 信任同步 — 留 future
- path-glob `**` 递归 — 留 PR3+ 考虑自己写 matcher
- prefix 通配符(`cargo *` glob)— 留 PR3+
- "始终允许" 撤销 UI — 留 PR3+
- web_fetch per-domain — 留 PR3+ 增 `match_kind='domain'`

### 6. Tests added (re-grill)

39 net new tests across `agent::permissions::shell_trust` (14
new), `agent::permissions::tests` (17 new: 8 dispatch/extract/
glob/payload tests + 9 carried-over mode/audit/risk/filter
tests that survived the rewrite), and
`projects::boundary::tests` (8 new — for `is_within_root`).
Total backend suite: 437 tests (was 398 pre-PR1; +39 net new).
See `cargo test --lib agent::permissions` and
`cargo test --lib projects::boundary` output.

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
