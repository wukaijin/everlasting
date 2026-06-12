# Research: web_fetch tool — API design (param shapes, output, HTTP, security, errors)

- **Query**: Design a `web_fetch` tool for Everlasting (Tauri 2 + Rust + Vue 3) matching the conventions of mainstream AI coding agents
- **Scope**: external (Claude Code / OpenCode / Cline / Continue.dev / Anthropic API / OpenAI Responses), with internal cross-references
- **Date**: 2026-06-12
- **PRD**: `.trellis/tasks/06-12-feat-tools-web-fetch-agent-api-p1/prd.md`
- **Sister research**:
  - `html-to-markdown-rust.md` (chosen: `htmd`)
  - `web-fetch-security.md` (SSRF + prompt-injection threat model)

## 1. Per-tool comparison

| Aspect | Claude Code `WebFetch` | OpenCode `webfetch` | Cline `web_fetch` | Continue.dev `fetchUrlContent` | Anthropic API `web_fetch_20250929` (server-side) | OpenAI `web_search` (server-side) |
|---|---|---|---|---|---|---|
| **Primary param** | `url` (req) | `url` (req) | `url` (req) | `url` (req) | `url` (req) | (no url; takes a query) |
| **Second param** | `prompt` (req) — extraction prompt fed to a fast model | `format` (opt: `markdown` / `text` / `html`, default `markdown`) | `prompt` (req) — analysis prompt | — | `max_tokens` (opt, default 100000) | `search_context_size` (low/med/high) + `filters.allowed_domains` + `user_location` |
| **Optional params** | — | `timeout` (sec, max 120) | `task_progress` | — | — | — |
| **Output to LLM** | **Model-extracted answer** (not raw page) — Claude runs a small fast model on the fetched content with the prompt and returns that answer | Raw content in requested format (markdown by default via Turndown, text via tag-strip, html passthrough) | Model-extracted answer (the Cline backend calls `POST /api/v1/search/webfetch` with `{Url, Prompt}` and returns the server's `result` string) | Markdown of the article (Mozilla Readability → JSDOM → node-html-markdown), truncated to 20 000 chars with a notice if exceeded | Server-side: returns a synthetic assistant turn with `citations` blocks pointing back to the source URL; raw page is converted to markdown before the model sees it | Server-side search result list (titles + URLs + snippets), not a fetched page |
| **Default timeout** | not disclosed (server-side) | 30 s (max 120 s) | 15 s (axios client) | none explicit (uses provider `fetch` with no documented timeout) | server-side | server-side |
| **Max response size** | "fixed character limit" (undisclosed) | 5 MB (`MAX_RESPONSE_SIZE`) — checked twice (Content-Length header + actual byte count) | 15 s — no explicit byte cap, relies on 15 s axios timeout | 20 000 chars post-conversion (truncated, with explicit "truncation warning" message injected) | server-side (default `max_tokens=100000`) | n/a (search results) |
| **User-Agent** | `Claude-User/...` (Anthropic-branded, no browser impersonation) | `Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ... Chrome/143.0.0.0 Safari/537.36` (full Chrome impersonation); on 403 + `cf-mitigated: challenge`, retries with `User-Agent: opencode` | Cline cloud backend, not local | Whatever the IDE / extension's `fetch` polyfill uses (Node fetch) | server-side | server-side |
| **Accept header** | prefers `text/markdown` over `text/html` (content-negotiation) | q-weighted per `format` (e.g. markdown ⇒ `text/markdown;q=1.0,text/x-markdown;q=0.9,text/plain;q=0.8,text/html;q=0.7,*/*;q=0.1`) | not set explicitly | not set explicitly | server-side | server-side |
| **HTTP→HTTPS** | auto-upgrade | enforced (throws if not http(s) at entry) | not enforced client-side; server-side | n/a (uses URL constructor) | n/a | n/a |
| **Redirects** | **Refused cross-host**: "When a URL redirects to a different host, WebFetch returns a text result that names the original URL and the redirect target instead of following it. Claude then fetches the new URL with a second WebFetch call." | follows (reqwest default 10) | follows (axios default) | follows | server-side | n/a |
| **HTML→MD converter** | server-side (Anthropic) | `TurndownService` (atx headings, `---` hr, `-` bullets, fenced code, removes script/style/meta/link) | server-side | `Readability` (Mozilla) → `node-html-markdown` (`NodeHtmlMarkdown.translate`) | server-side | n/a |
| **text/html** | converted to MD, then LLM-extracted | Turndown (markdown) or text-strip (text) | server-side | Readability + MD | server-side | n/a |
| **text/plain / text/markdown** | passed through | passed through | server-side | passed through | server-side | n/a |
| **application/json** | not specified | passed through raw | server-side | not specifically handled | server-side | n/a |
| **image/png, image/jpeg** | not exposed (read-only docs tool) | returned as `attachments: [{type: "file", mime, url: "data:..."}]` (base64 inline) | server-side | not exposed (returns text only) | server-side | n/a |
| **image/svg+xml** | not exposed | kept as text/html passthrough (per tests) | server-side | not exposed | server-side | n/a |
| **application/xml / RSS** | not specified | passed through raw | server-side | not specifically handled | server-side | n/a |
| **Binary (PDF, video)** | not supported | not supported (5 MB cap; base64 only for images) | server-side | not supported | server-side | n/a |
| **Caching** | **15 minutes** per URL | none explicit | none explicit | none explicit | server-side | n/a |
| **Permissions / domain gate** | Yes — first-time per domain prompts user; preapproved doc domains skip the prompt; `WebFetch(domain:example.com)` rule for allow/deny/ask; auto / bypassPermissions skip the prompt | Yes — `ctx.ask({permission: "webfetch", patterns: [url]})` | Yes — auto-approve flow checks `shouldAutoApproveTool` + manual fallback (`askApprovalAndPushFeedback`); also feature-flagged behind "Cline web tools enabled" | `defaultToolPolicy: "allowedWithPermission"` | server-side | n/a (server) |
| **TLS cert handling** | server-side | server-side (default rustls) | server-side (axios default) | uses `fetch` polyfill default (strict) | server-side | server-side |
| **Retry policy** | none (single attempt; redirect to different host instead) | **Cloudflare 403 retry**: if response has `cf-mitigated: challenge` header + status 403, retries once with `User-Agent: opencode` (honnest UA) to bypass bot detection | none | none | server-side | server-side |
| **Error model** | HTTP/4xx/5xx → text result; cross-host redirect → text result naming both URLs; large pages → truncation | Non-2xx → throws (the `filterStatusOk` wrapper); 5 MB exceeded → throws; timeout → `Effect.die("Request timed out")`; abort signal respected | `axios.post` throws on non-200 → caught and returned as `Error fetching web content: ${message}` (text result with `is_error: true`) | Non-2xx → `throw new Error("HTTP ${status} ${statusText}")` (becomes tool error) | server-side | server-side |

## 2. Conventions (where everyone agrees)

- **`url: string` (required)** is the only universally required param. No agent ships a multi-URL fetch.
- **GET only.** None of the surveyed tools implement POST/PUT/DELETE for `web_fetch`. POST/PUT-style operations are always routed through `web_search` (or `web_search_preview` server-side tool).
- **Markdown is the canonical output format.** Even when the user can ask for `text` or `html`, the default and the recommended format is markdown — it's the lowest-token representation that preserves structure (headings, lists, code, links).
- **HTML→Markdown conversion happens locally** for the client-side agents (OpenCode, Continue). For server-side tools (Claude Code, Cline cloud, Anthropic API), conversion happens on the server.
- **`text/html` and `text/plain` are the only content types guaranteed to be handled cleanly.** Everything else (JSON, XML, images) is either passed through, returned as base64 inline (images only — OpenCode), or just doesn't work.
- **A response-size cap is universal.** 5 MB raw / 20 000 chars markdown / 100 000 tokens (server). The cap is small enough that the LLM's context stays sane.
- **HTTP→HTTPS auto-upgrade** is universal for client-side tools (OpenCode throws on non-http(s); Claude Code docs claim upgrade).
- **Permissions gate is a first-class concept** (Claude Code domain rules, OpenCode `ctx.ask`, Cline approval flow, Continue `defaultToolPolicy`). The tool is read-only and "exits" the workspace, so the gate is usually at the domain level (first time per host) rather than the per-call level.
- **`is_error: true` / tool-error channel is the failure surface**, not HTTP status codes. The tool eats the HTTP status, converts it to a readable sentence ("HTTP 404 Not Found", "Request timed out", "Response too large"), and the LLM can self-correct.

## 3. Where they diverge (and the trade-offs)

| Decision point | Camp A (Claude Code / Cline / Anthropic API) | Camp B (OpenCode / Continue) | Trade-off |
|---|---|---|---|
| **Extraction model** | A second small LLM extracts an answer to the user's `prompt`; Claude never sees the raw page | Raw markdown returned to the LLM; the LLM extracts itself | A is more reliable (purpose-built extraction model, smaller context, fewer tokens used); B is simpler, fully local, no extra LLM call, but burns more of the main model's context |
| **`prompt` param** | Required (Anthropic-style: "tell me what's on this page about X") | Not present (model reads the page directly) | A is query-shaped, B is fetch-shaped. Camp A converges well for Q&A patterns; Camp B keeps the tool general-purpose |
| **User-Agent** | Honest branded UA (`Claude-User`, `opencode` after CF fail) | Full browser impersonation (Chrome 143 / Safari) | Honest UA avoids lying to servers but is rate-limited / 403'd by Cloudflare-protected sites. Browser UA "just works" but is mildly deceptive |
| **Redirects** | **Refused cross-host** (Claude Code returns a text result naming both URLs and expects the LLM to call again) | Followed transparently (up to 10) | Cross-host refusal prevents the LLM from being tricked into following a benign-looking link to a malicious site. Following is more convenient. Both defend against open-redirect chains |
| **Cloudflare 403 retry** | None (Claude Code) | OpenCode retries once with `User-Agent: opencode` if `cf-mitigated: challenge` header present | Pragmatic workaround for a common 403 cause; not strictly necessary if the first UA already works |
| **Caching** | 15 minutes (Claude Code) | None | Cache helps repeated fetches in long sessions (e.g. LLM reads same doc twice); costs freshness. Camp B leaves it to the LLM (caller can save to file via `write_file`) |
| **Image content** | Not supported | Returned as `attachments: [{type: "file", mime, url: "data:image/...;base64,..."}]` for the model that supports image inputs | Image support is a nice-to-have (screenshots of error pages, etc.); adds a content-length branch and a base64 branch |
| **Timeout** | 15 s (Cline) / 30 s default (OpenCode) / 120 s max (OpenCode) | none (Continue) / server-side | Shorter = better DoS protection; longer = handles slow sites. 30 s is the sweet spot for "normal" web pages |
| **Permissions** | Domain-level rule (first time per host, with preapproved doc domains) | Per-call approval | Domain-level is friendlier; per-call is more conservative |

## 4. Map to our Rust/Tauri context

Constraints recap (from PRD):
- Rust backend, `reqwest 0.13` already in `Cargo.toml` with `rustls` (strict TLS by default)
- Tool runs inside the agent loop, must respect `CancellationToken` (so the LLM can be interrupted)
- Minimize new deps (already adding `htmd` for HTML→MD per sister research)
- 7 existing tools follow `ToolDef { name, description, input_schema }` + `execute(input, ctx)` pattern, all `tokio::select!`-cancel-safe
- No shared `reqwest::Client` — each provider builds its own; web_fetch should follow the same per-tool-client pattern (or a small `http` helper module that returns a builder)

Design choices that follow from constraints and conventions:

1. **Local fetch + raw markdown to LLM (Camp B).** We're self-hosted; we don't have a second small LLM to do extraction. The LLM already sees the page content (this is the same model that does the agent loop). This is what OpenCode does and it's simpler.
2. **`url` required, `format` optional defaulting to `markdown`.** This matches OpenCode exactly. Two-param tool is friendly for the LLM. Skip `prompt` (no extraction model).
3. **No `max_tokens` (or `max_bytes`) param for the LLM-facing output.** The 5 MB cap is enforced internally; the LLM gets the result truncated. We can add an opt-in `max_output_tokens` later if it becomes a problem. Simpler MVP.
4. **30 s default timeout, 120 s max.** Matches OpenCode. Use `connect_timeout(10s) + timeout(30s) + tokio::time::timeout` outer wrapper + `tokio::select!` on `CancellationToken` for full cancel safety. The LLM can override `timeout` in seconds; cap at 120.
5. **Honest `User-Agent: Everlasting/<version>` by default.** Don't impersonate Chrome — we're a coding tool, not a browser. If we hit Cloudflare 403 in practice we can add the retry-with-bare-UA fallback later.
6. **Follow redirects (reqwest default 10).** Apply the SSRF check (sister research §T2f) to *every* redirect target, not just the initial URL. This is the only place to enforce it.
7. **Apply SSRF protections to MVP** (sister research `web-fetch-security.md` rec): block private IPv4 (10/8, 172.16/12, 192.168/16, 127/8, 169.254/16), private IPv6 (::1, fc00::/7, fe80::/10), non-HTTP(S) schemes. This is the *one* security mitigation that pays for itself immediately. Everything else (TLS strict, response cap, timeout) is already free with reqwest defaults.
8. **No image support in MVP.** Adding it would mean a base64 branch + a separate `attachments` field on `tool_result`, and our `wire.rs` doesn't model attachments. Defer.
9. **No caching.** The LLM can `write_file` the markdown and `read_file` it back. Caching adds state, expiry, invalidation. Defer.
10. **No domain permissions gate for MVP.** The user already has the session-level stop button; the SSRF blocklist prevents the worst abuse. The existing tool infra doesn't have a domain-allowlist concept, and adding one is a non-trivial UI change. PRD already marks this out-of-scope. If we need it later, mirror Claude Code's `WebFetch(domain:example.com)` rule.
11. **No 5 MB cap at the LLM-facing output level.** The response can be up to 5 MB raw, but markdown conversion typically shrinks HTML 5-10×. Use head+tail truncation (50 KB head + 50 KB tail, similar to our `read_file` 8-PR1 policy) when the *converted* markdown exceeds ~100 KB. This matches our existing pattern.
12. **Error model**: a non-2xx response becomes `is_error: true` with a human-readable sentence — same shape as our other tools. The exact error categories we should distinguish: `Network` (DNS/TCP/TLS), `HttpStatus(code, reason)`, `ContentTooLarge`, `Timeout`, `InvalidUrl`, `UnsupportedContentType` (only if we later reject things; for now we pass everything through).

## 5. Three candidate API designs

### Candidate A — "OpenCode-style, two params, no extraction"

```json
{
  "name": "web_fetch",
  "description": "Fetches content from a URL and returns it as markdown (default), plain text, or raw HTML. Use this to read external documentation, API references, error messages, or any web page. Read-only; does not modify files. Results may be truncated if very large.",
  "input_schema": {
    "type": "object",
    "properties": {
      "url": {
        "type": "string",
        "description": "Fully-formed URL (http:// or https://). HTTP URLs are auto-upgraded to HTTPS."
      },
      "format": {
        "type": "string",
        "enum": ["markdown", "text", "html"],
        "default": "markdown",
        "description": "Output format. markdown (default) is best for documentation; text strips all tags; html returns the raw response body."
      },
      "timeout": {
        "type": "integer",
        "description": "Timeout in seconds (default 30, max 120)."
      }
    },
    "required": ["url"]
  }
}
```

- **Pros**: simple, matches OpenCode exactly, easy to test, low param count → fewer invalid-arg errors. Single `url` aligns with how LLMs naturally want to use the tool. The `format` opt-out to `text`/`html` covers the rare case where the LLM wants the raw page.
- **Cons**: no way to ask "summarize this page" — the LLM has to fetch and read 50 KB of markdown. Burns context on long pages.

### Candidate B — "Claude-Code-style, url + extraction prompt"

```json
{
  "name": "web_fetch",
  "description": "Fetches content from a URL and answers a question about it. Use this for Q&A against external documentation, API references, or error pages. The page is converted to markdown before being analyzed.",
  "input_schema": {
    "type": "object",
    "properties": {
      "url": {
        "type": "string",
        "description": "Fully-formed URL."
      },
      "prompt": {
        "type": "string",
        "description": "The question to answer using the fetched page. Must be at least 2 characters. The page is treated as untrusted reference material."
      }
    },
    "required": ["url", "prompt"]
  }
}
```

- **Pros**: matches Anthropic's own `web_fetch_20250929` shape. Saves main-model context because the model only sees the answer. Natural pattern: "fetch the docs for X and tell me how to do Y".
- **Cons**: requires a second LLM call (or a smaller model) to do the extraction — but we're self-hosted and don't have a separate extraction model, so we'd either (a) call the same model again (expensive, slow), or (b) skip extraction and just give the model the prompt + raw markdown as a user message (a bit awkward, breaks the "tool result" abstraction). Net: this is the wrong shape for our local setup.

### Candidate C — "Hybrid: url + optional format + optional prompt for summary"

```json
{
  "name": "web_fetch",
  "description": "Fetches content from a URL. Returns the page content in the requested format (markdown by default). Optionally pass a 'prompt' to extract a specific answer (saves context for long pages).",
  "input_schema": {
    "type": "object",
    "properties": {
      "url": {
        "type": "string",
        "description": "Fully-formed URL."
      },
      "format": {
        "type": "string",
        "enum": ["markdown", "text", "html"],
        "default": "markdown"
      },
      "prompt": {
        "type": "string",
        "description": "Optional. If provided, the tool asks a small model to answer this question using the page and returns the answer instead of the raw content."
      },
      "timeout": {
        "type": "integer",
        "description": "Timeout in seconds (default 30, max 120)."
      }
    },
    "required": ["url"]
  }
}
```

- **Pros**: works without a second model (degrades to Candidate A when `prompt` is omitted). Forward-compatible: when we add a local small-model extraction later, we just fill in the `prompt` branch. Backward-compatible with the PRD's assumptions.
- **Cons**: more params, more doc surface, more invalid-arg combinations. The `prompt` branch is a no-op in MVP and could be confusing.

## 6. Recommendation

**Adopt Candidate A (OpenCode-style).** Reasons:

1. **Local-only architecture.** Candidate B's `prompt` param is only useful if we have a separate extraction model. We don't, and adding one is a separate, much larger workstream.
2. **Simpler to test, simpler to spec.** Two required-implied params (`url`, `format` default) plus one opt-in (`timeout`). Five-line description. The 5MB cap + head+tail truncation handles the "long page" case well enough for MVP.
3. **Matches the only fully-local open-source tool with a published source.** We can copy OpenCode's Turndown-based conversion as a reference and `htmd` as the Rust equivalent (per sister research `html-to-markdown-rust.md`).
4. **The agent loop already has context budget planning.** When a fetched page is genuinely long and the LLM only wants a fact, the LLM can re-fetch with `format=text` (strips HTML, ~30% smaller) and skip sections — or save the page to a file with `write_file` and `read_file` it back with `offset`/`limit`. We don't need a built-in extractor.
5. **The PRD's assumptions line up with Candidate A** ("HTML→MD: Rust 端做", "max ~100KB (head+tail)", "format (markdown | text)", "timeout").

OpenCode is the closest reference implementation. The file is `packages/opencode/src/tool/webfetch.ts` (anomalyco/opencode, branch `dev`) — 192 lines, self-contained, MIT-licensed, and the same conversion library (Turndown) is ported to Rust as `htmd`. The two implementations will be near-line-for-line translatable.
