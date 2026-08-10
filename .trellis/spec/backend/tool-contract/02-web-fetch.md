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
  IP blocklist in [`tools/web_fetch.rs`](../../../../app/src-tauri/src/tools/web_fetch.rs)
  §`is_blocked`. Cloud-metadata short-circuit for 169.254.169.254.
- **T2e (DNS rebinding)** — Med severity, MVP accepted risk. Single-shot
  `lookup_host` + `ClientBuilder::resolve(domain, ip)` closes most of the
  window. Full socket-level re-validate is a follow-up.
- **T2f (redirect to private IP)** — Med severity, **IMPLEMENTED** via
  [`build_redirect_policy()`](../../../../app/src-tauri/src/tools/web_fetch.rs)
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
