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

Total backend suite:166+ tests pass as of round1 (was103 pre-toolset; +63 net
new from this round, including3 cleanup helpers).

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
  "toolName": "shell",
  "toolInput": { "command": "ls -la" },
  "risk": "high",
  "reason": "The tool shell requires your confirmation (risk: 高)."
}
```

**IPC command** (client → server):
`invoke("permission_response", { rid, decision })` where
`decision` is one of `"allow_once"` / `"allow_always"` /
`"deny"`. The Tauri command lives in
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
