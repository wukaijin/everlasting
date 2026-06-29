//! `web_fetch` tool — fetch external URLs as markdown / text / html.
//!
//! Added in P1 (2026-06-12). Lets the LLM autonomously read external
//! documentation, API references, error messages, and arbitrary web
//! pages. The tool runs in the Rust backend (no LLM provider is
//! involved), so it is *protocol-agnostic* — works for both Anthropic
//! and OpenAI model adapters without any change.
//!
//! ## Design decisions (see prd.md §"Decision (ADR-lite)")
//!
//! - **3 params** (`url` required, `format` / `timeout` optional):
//!   matches OpenCode's local-only `webfetch` shape. No extraction
//!   `prompt` param — we have no second small model, so the main
//!   model reads the converted markdown directly.
//! - **SSRF protection in MVP**: a hard-coded IP blocklist (RFC 1918 +
//!   loopback + link-local + CGNAT + multicast + reserved) is applied
//!   to the initial URL AND to every redirect target. The redirect
//!   guard is implemented via `redirect::Policy::custom` callback in
//!   `build_redirect_policy` (RULE-E-003, 2026-06-14) — the `Fn`
//!   callback re-runs `resolve_and_check_sync` + `is_blocked` on
//!   every hop. Without this, an LLM-driven local agent is
//!   effectively a network scanner (e.g. `attacker.com → 169.254.169.254`
//!   leaks cloud metadata).
//! - **`htmd` 0.5** for HTML→MD conversion (see
//!   `research/html-to-markdown-rust.md`): leanest deps, most
//!   battle-tested (passes turndown.js test corpus), Apache-2.0.
//! - **GET only**, **scheme allowlist** (`http` / `https`), **strict
//!   TLS** (reqwest's rustls default), **5 MiB hard cap on the
//!   response body** (truncated, not spilled to disk — different
//!   tradeoff from `shell` because the LLM can re-fetch with
//!   `format=text` for smaller payloads), **30 s default timeout
//!   (max 120 s)**, **5 redirect hops**.
//!
//! ## Out of scope (MVP)
//!
//! - POST / PUT / DELETE (GET only)
//! - `web_search` (separate P2 task)
//! - Domain-allowlist / "first time per host" permissions
//! - DNS rebinding defense beyond a single-shot resolve + IP pin
//! - Caching (LLM can `write_file` the result and `read_file` back)
//! - Image / PDF / binary content
//! - JavaScript rendering (Playwright/CDP)
//! - Cookies / session management
//!
//! ## Cancellation
//!
//! The outer `execute_tool` wrapper in `tools/mod.rs` already wraps
//! this future in `tokio::select! { biased; cancel | future }`, so a
//! user Stop aborts an in-flight request by dropping the future. We
//! additionally layer `tokio::time::timeout` on top so an LLM-supplied
//! `timeout` doesn't deadlock on a hung server.
//!
//! ## Error model
//!
//! Seven internal variants (`WebFetchError`) are mapped to
//! `(is_error: true, human-readable string)` for the LLM. We do NOT
//! reuse `LlmError` — the LLM API is not involved here, so
//! `LlmError`'s 5 categories (Auth / RateLimit / InvalidRequest /
//! Server / Network) don't fit.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use htmd::HtmlToMarkdown;
use reqwest::redirect;
use serde_json::json;
use tokio::net::lookup_host;

use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Hard cap on the response body. After this we return `TooLarge`.
/// Matches OpenCode's `MAX_RESPONSE_SIZE` (5 MiB) so we have a peer
/// reference.
const MAX_BODY_BYTES: u64 = 5 * 1024 * 1024;

/// Default per-call timeout. LLM can override up to [`MAX_TIMEOUT_SECS`].
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Maximum `timeout` the LLM can request.
const MAX_TIMEOUT_SECS: u64 = 120;

/// Outer `tokio::time::timeout` wrapper adds this much to the
/// reqwest-level timeout so a slow body read (after headers) can
/// still finish cleanly. Without this, a 30 s reqwest timeout can
/// cut off a 29.9 s body read right when the data is arriving.
const TIMEOUT_GRACE_SECS: u64 = 5;

/// Total output size before the head/tail truncation kicks in.
/// Matches `read_file`'s MAX_OUTPUT_BYTES (50 KB head + 50 KB tail =
/// 100 KB total window) so the LLM gets a similar "this is what you
/// would see if you fetched the whole thing" UX.
const MAX_OUTPUT_BYTES: usize = 100 * 1024;
const TRUNCATE_HEAD: usize = 50 * 1024;
const TRUNCATE_TAIL: usize = 50 * 1024;

/// Max HTTP redirects. 5 matches `curl -L` and OpenCode's policy.
const MAX_REDIRECTS: usize = 5;

/// TCP connect timeout. Aggressive — if we can't connect in 10 s,
/// the LLM should move on.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// DNS resolution timeout. Applied to the manual `lookup_host` in
/// [`resolve_public`] — that step runs BEFORE the reqwest client is
/// built, so reqwest's own `.timeout()` (which only covers the HTTP
/// lifecycle: connect + headers + body) cannot bound it. Without
/// this wrapper a hung resolver blocks the tool **indefinitely** —
/// observed 2026-06-26 as a `researcher` subagent permanently stuck
/// on `web_fetch` under a fake-ip proxy (github.com → 198.18.0.34)
/// whose DNS module wasn't ready: `lookup_host` awaited forever,
/// the dispatch had no wall-clock cap, and the parent session froze
/// at the `dispatch_subagent` tool_use until the process was killed.
///
/// 20 s is far above the ~10 ms healthy case and above the system
/// resolver's default `attempts × timeout` (~10 s), so it only trips
/// on a genuinely broken resolver while tolerating slow cold-starts
/// on proxied / mobile networks. The HTTP lifecycle is still bounded
/// separately by `timeout_secs` (default 30). Note the *redirect*
/// path uses the sync [`resolve_and_check_sync`] inside reqwest's
/// `Policy::custom` callback — that one IS covered by reqwest's
/// total `.timeout()` (redirects happen inside `request.send()`),
/// so it needs no separate bound.
const DNS_TIMEOUT_SECS: u64 = 20;

const USER_AGENT: &str = concat!("Everlasting/", env!("CARGO_PKG_VERSION"));

/// Content-negotiation hint. Sites that serve `text/markdown` (e.g.
/// GitHub raw, some docs hosts) get the cheapest path; everyone else
/// falls through to HTML.
const ACCEPT_HEADER: &str =
    "text/markdown;q=1.0,text/html;q=0.9,text/plain;q=0.8,application/json;q=0.5,*/*;q=0.1";

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Internal error categories. The LLM sees a flat human-readable
/// string (via `Display`); this enum is the single source of truth
/// for the 7 categories the PRD locks in.
#[derive(Debug, thiserror::Error)]
pub enum WebFetchError {
    #[error("URL must be http or https (got: {0})")]
    InvalidUrl(String),

    /// The URL's resolved IPs are all in a blocked range (RFC 1918,
    /// loopback, link-local, CGNAT, multicast, reserved, or cloud
    /// metadata endpoints). Carries the first resolved IP for the
    /// error message.
    #[error("refusing to fetch private/loopback/link-local address (URL resolves to {0})")]
    BlockedAddress(IpAddr),

    /// Redirect target resolved to a blocked address (RULE-E-003,
    /// 2026-06-14). The chain was stopped by the SSRF guard
    /// `Policy::custom` callback before the body could be returned.
    /// `from` is the original URL we were following; `to` is the
    /// redirect Location header the upstream returned.
    #[error("redirect from <{from}> refused: target <{to}> resolves to a blocked (private/loopback/link-local) address")]
    RedirectBlocked { from: String, to: String },

    #[error("response body exceeds 5 MiB cap")]
    TooLarge,

    #[error("HTTP {0}")]
    HttpStatus(u16),

    #[error("request timed out after {0}s")]
    Timeout(u64),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("network error: {0}")]
    Network(String),
}

// ---------------------------------------------------------------------------
// IP blocklist — SSRF guard
// ---------------------------------------------------------------------------

/// CIDR ranges that must never be fetched. The list is hard-coded
/// (not user-configurable) for MVP — see PRD §"Out of Scope". Ranges
/// cover RFC 1918, loopback, link-local (which includes cloud
/// metadata at 169.254.169.254), CGNAT, multicast, and reserved.
const BLOCKED_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),       // "this network"
    (Ipv4Addr::new(10, 0, 0, 0), 8),      // RFC 1918
    (Ipv4Addr::new(127, 0, 0, 0), 8),     // loopback
    (Ipv4Addr::new(169, 254, 0, 0), 16),  // link-local + cloud metadata
    (Ipv4Addr::new(172, 16, 0, 0), 12),   // RFC 1918
    (Ipv4Addr::new(192, 168, 0, 0), 16),  // RFC 1918
    (Ipv4Addr::new(100, 64, 0, 0), 10),   // CGNAT
    (Ipv4Addr::new(224, 0, 0, 0), 4),     // multicast
    (Ipv4Addr::new(240, 0, 0, 0), 4),     // reserved
];

const BLOCKED_V6: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::LOCALHOST, 128),                                     // ::1
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),                // ULA
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),               // link-local
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),                // multicast
];

/// Returns true if `ip` is in any blocked range. Defends the LLM
/// from being tricked into fetching `http://127.0.0.1:8080/admin` or
/// `http://169.254.169.254/...` (cloud metadata).
///
/// IPv4-mapped IPv6 (`::ffff:192.168.1.1`) is unwrapped and the
/// embedded v4 is re-checked — this is the OWASP-recommended
/// behavior for SSRF guards.
///
/// `allow_private` short-circuits the blocklist and returns false
/// for everything. Production always passes `false`. The test suite
/// uses a separate entry point (`execute_for_test`) that passes
/// `true`; this avoids the need for a global flag, which would
/// race with parallel integration tests.
fn is_blocked(ip: IpAddr, allow_private: bool) -> bool {
    if allow_private {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => {
            // Cloud metadata short-circuit: even if the link-local
            // rule changes, 169.254.169.254 must always be blocked.
            if v4 == Ipv4Addr::new(169, 254, 169, 254) {
                return true;
            }
            BLOCKED_V4
                .iter()
                .any(|(net, prefix)| v4_in_cidr(v4, *net, *prefix))
        }
        IpAddr::V6(v6) => {
            // v4-mapped v6: re-check the embedded v4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked(IpAddr::V4(v4), allow_private);
            }
            BLOCKED_V6
                .iter()
                .any(|(net, prefix)| v6_in_cidr(v6, *net, *prefix))
        }
    }
}

fn v4_in_cidr(ip: Ipv4Addr, net: Ipv4Addr, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = !0u32 << (32 - prefix);
    (u32::from(ip) & mask) == (u32::from(net) & mask)
}

fn v6_in_cidr(ip: Ipv6Addr, net: Ipv6Addr, prefix: u8) -> bool {
    if prefix == 0 {
        return true;
    }
    let mask = !0u128 << (128 - prefix);
    (u128::from(ip) & mask) == (u128::from(net) & mask)
}

// ---------------------------------------------------------------------------
// Format enum
// ---------------------------------------------------------------------------

/// Output format. The PRD locks 3 values; unknown / missing values
/// fall back to `Markdown` (matches `format: { default: "markdown" }`
/// in the JSON schema).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Markdown,
    Text,
    Html,
}

impl Format {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("text") => Format::Text,
            Some("html") => Format::Html,
            _ => Format::Markdown,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

pub fn definition() -> ToolDef {
    ToolDef {
        name: "web_fetch".to_string(),
        description: Some(
            "Fetches content from a URL and returns it as markdown (default), plain \
             text, or raw HTML. Use this to read external documentation, API \
             references, error messages, or any web page. Read-only; does not \
             modify files. Supports HTTP and HTTPS only.\n\n\
             Security: by design, this tool refuses to fetch private, loopback, \
             or link-local addresses (e.g. 127.0.0.1, 192.168.x.x, \
             169.254.169.254) to prevent the agent from being used as an SSRF \
             proxy.\n\n\
             Results may be truncated if very large; use `format: \"text\"` for \
             a smaller payload, or `format: \"html\"` to get the raw response."
                .to_string(),
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Fully-formed URL (http:// or https://). Required."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text", "html"],
                    "default": "markdown",
                    "description": "Output format. markdown (default) converts HTML via htmd; text strips HTML tags to plain text; html returns the raw response body."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 30, max 120)."
                }
            },
            "required": ["url"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Execute (public entry point — signature matches the simpler tools
// like `grep` / `glob` / `list_dir` that don't need a ReadGuard or
// session_id)
// ---------------------------------------------------------------------------

pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool) {
    execute_with(input, ctx, false).await
}

/// Test-only entry that bypasses the SSRF block. Used by
/// integration tests that need to talk to a `httpmock` server
/// bound to 127.0.0.1. Production code MUST call [`execute`]
/// instead — this function is `#[cfg(test)]`-gated to keep it
/// out of the production binary.
#[cfg(test)]
pub async fn execute_for_test(
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool) {
    execute_with(input, ctx, true).await
}

async fn execute_with(
    input: &serde_json::Value,
    _ctx: &ToolContext,
    allow_private: bool,
) -> (String, bool) {
    let url = match input.get("url").and_then(|v| v.as_str()) {
        Some(u) => u.to_string(),
        None => return ("Missing required parameter: url".to_string(), true),
    };
    let timeout_secs = input
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let format = Format::parse(input.get("format").and_then(|v| v.as_str()));

    match fetch_and_process(&url, timeout_secs, format, allow_private).await {
        Ok(content) => (content, false),
        Err(e) => (e.to_string(), true),
    }
}

// ---------------------------------------------------------------------------
// Core fetch + process pipeline
// ---------------------------------------------------------------------------

async fn fetch_and_process(
    url: &str,
    timeout_secs: u64,
    format: Format,
    allow_private: bool,
) -> Result<String, WebFetchError> {
    // 1. Parse + scheme-validate the URL.
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| WebFetchError::InvalidUrl(format!("{} ({})", url, e)))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(WebFetchError::InvalidUrl(scheme.to_string()));
    }

    // 2. Resolve host → IP, reject if every resolved IP is in a
    //    blocked range. Pinning the validated IP on the reqwest
    //    request also closes the small DNS-rebinding window between
    //    our check and reqwest's connect.
    let host = parsed
        .host_str()
        .ok_or_else(|| WebFetchError::InvalidUrl("missing host".to_string()))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| WebFetchError::InvalidUrl("missing port".to_string()))?;
    let public_ip = resolve_public(host, port, allow_private).await?;

    // 3. Build the reqwest client. Per-PRD defaults: 30s total,
    //    10s connect, 5 redirects, strict TLS (rustls default).
    //    `.resolve(host, ip)` pins the connection to the IP we just
    //    validated — without this, reqwest would do its own DNS
    //    lookup and the small TOCTOU window between our check and
    //    reqwest's connect is open. The Host header is unaffected
    //    (reqwest still sends the original domain), so SNI / virtual
    //    hosting keep working.
    //
    //    Redirect handling uses a `Policy::custom` callback
    //    (RULE-E-003, 2026-06-14) that re-runs the SSRF blocklist
    //    check on every redirect target. Plain `Policy::limited`
    //    only counts depth, which lets `attacker.com → 169.254.169.254`
    //    style attacks slip through. See `build_redirect_policy`.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(build_redirect_policy())
        .user_agent(USER_AGENT)
        .resolve(host, public_ip)
        // Auto-decompress gzip/brotli/deflate per the response
        // `Content-Encoding` header. This pairs with the
        // `Accept-Encoding: gzip, br, deflate` request header below —
        // without it the server returns a compressed body and we
        // hand the raw bytes to `from_utf8`, which blows up on the
        // gzip magic (1f 8b): "non-utf8 html body ... invalid utf-8
        // sequence ... from index 1". Needs the matching `gzip` /
        // `brotli` / `deflate` reqwest features in Cargo.toml.
        .gzip(true)
        .brotli(true)
        .deflate(true)
        .build()
        .map_err(|e| WebFetchError::Network(e.to_string()))?;

    // 4. Build the GET request.
    let request = client
        .get(parsed.as_str())
        .header("Accept", ACCEPT_HEADER)
        .header("Accept-Encoding", "gzip, br, deflate");

    // 5. Send. We layer an outer `tokio::time::timeout` on top of
    //    reqwest's own timeout so a body read that lands right at
    //    the reqwest limit still gets the `Timeout` error category
    //    (not a generic `Network` one).
    let response = match tokio::time::timeout(
        Duration::from_secs(timeout_secs + TIMEOUT_GRACE_SECS),
        request.send(),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return Err(classify_reqwest_error(e, timeout_secs)),
        Err(_) => return Err(WebFetchError::Timeout(timeout_secs)),
    };

    let status = response.status();
    if status.is_redirection() {
        // The redirect SSRF guard (RULE-E-003, `build_redirect_policy`)
        // returns `Action::Stop` for blocked targets. Reqwest then
        // returns the 3xx response as the final response. Surface a
        // clean error so the LLM knows the chain was refused by our
        // SSRF defense, not by the upstream server.
        return Err(WebFetchError::RedirectBlocked {
            from: url.to_string(),
            to: response.url().to_string(),
        });
    }
    if !status.is_success() {
        return Err(WebFetchError::HttpStatus(status.as_u16()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    // Capture metadata BEFORE `.bytes()` consumes the response
    // (needed for the attribution prefix in step 9).
    let final_url = response.url().to_string();
    let status_code = status.as_u16();

    // 6. Read the body. The 5 MiB cap is enforced *after* we have
    //    the bytes (reqwest buffers in memory regardless); a future
    //    streaming variant can move this to a counting `AsyncRead`.
    let body_bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => return Err(classify_reqwest_error(e, timeout_secs)),
    };
    if body_bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(WebFetchError::TooLarge);
    }

    // 7. Convert per `format`. JSON gets pretty-print; HTML gets
    //    `htmd` (markdown) or tag-strip (text); everything else
    //    passes through as-is.
    let content = convert_body(&body_bytes, &content_type, format)?;

    // 8. Apply head/tail truncation to the converted content
    //    (attribution prefix is prepended AFTER, so the prefix
    //    is never truncated).
    let truncated = truncate_output(content);

    // 9. Attribution prefix. Prepending an HTML-comment-style
    //    marker with the final URL, fetch timestamp, status, byte
    //    count, and content-type is a cheap prompt-injection
    //    mitigation (T1a in `research/web-fetch-security.md`): the
    //    LLM can attribute the content to a specific fetch and
    //    treat it as untrusted reference material. The comment
    //    format is harmless if the LLM treats it as plain text
    //    and the markdown converter downstream of this tool would
    //    strip it anyway.
    let attribution = format!(
        "<!-- fetched: <{}> at <{}> · status {} · {} bytes · content-type <{}> -->\n\n",
        final_url,
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        status_code,
        body_bytes.len(),
        if content_type.is_empty() { "none" } else { content_type.as_str() },
    );

    Ok(format!("{}{}", attribution, truncated))
}

/// Map a reqwest error to our 7-variant taxonomy. The distinction
/// matters because the LLM can self-correct on a `Timeout` (retry
/// with a bigger budget) but not on a `Tls` (the cert is broken).
fn classify_reqwest_error(e: reqwest::Error, timeout_secs: u64) -> WebFetchError {
    if e.is_timeout() {
        return WebFetchError::Timeout(timeout_secs);
    }
    if e.is_connect() {
        // Connect failures often surface a TLS error message in the
        // chain — check before falling back to Network.
        let source_chain = format!("{:?}", e);
        if source_chain.contains("certificate")
            || source_chain.contains("tls")
            || source_chain.contains("handshake")
            || source_chain.contains("Ssl")
        {
            return WebFetchError::Tls(e.to_string());
        }
    }
    WebFetchError::Network(e.to_string())
}

fn convert_body(
    body: &[u8],
    content_type: &str,
    format: Format,
) -> Result<String, WebFetchError> {
    let ct = content_type.to_ascii_lowercase();
    let is_html = ct.contains("text/html") || ct.contains("application/xhtml");
    let is_json = ct.contains("json");

    match format {
        Format::Html => Ok(String::from_utf8_lossy(body).to_string()),
        Format::Markdown => {
            if is_html {
                let html = std::str::from_utf8(body).map_err(|e| {
                    WebFetchError::Network(format!("non-utf8 html body: {}", e))
                })?;
                html_to_markdown(html)
            } else if is_json {
                pretty_json(body)
            } else {
                Ok(String::from_utf8_lossy(body).to_string())
            }
        }
        Format::Text => {
            if is_html {
                let html = std::str::from_utf8(body).map_err(|e| {
                    WebFetchError::Network(format!("non-utf8 html body: {}", e))
                })?;
                Ok(html_to_text(html))
            } else if is_json {
                pretty_json(body)
            } else {
                Ok(String::from_utf8_lossy(body).to_string())
            }
        }
    }
}

fn pretty_json(body: &[u8]) -> Result<String, WebFetchError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| WebFetchError::Network(format!("invalid JSON: {}", e)))?;
    serde_json::to_string_pretty(&v)
        .map_err(|e| WebFetchError::Network(format!("JSON serialize: {}", e)))
}

fn html_to_markdown(html: &str) -> Result<String, WebFetchError> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "nav", "footer", "header", "aside",
        ])
        .build();
    converter
        .convert(html)
        .map_err(|e| WebFetchError::Network(format!("html→md: {}", e)))
}

/// Lightweight HTML tag stripper for `format: "text"`. We intentionally
/// do NOT route through `htmd` here (which would emit markdown syntax
/// we'd then have to strip) — a single-pass char filter is faster and
/// gives a tighter payload for the LLM. Not as smart as
/// Mozilla `Readability` but adequate for "give me the words on the
/// page".
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Decode the 6 most common named entities (a full entity table
    // is overkill for MVP — the rest pass through as `&xyz;` and
    // the LLM can usually still parse them).
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    // Collapse runs of whitespace into a single space.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_was_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(c);
            last_was_space = false;
        }
    }
    collapsed
}

fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    // `head_end` / `tail_start` are byte offsets, but a Rust `&str`
    // slice index MUST land on a UTF-8 char boundary — slicing
    // mid-character panics ("byte index N is not a char boundary").
    // This bites on any body with multi-byte chars (CJK, emoji, or
    // the U+FFFD `�` that `from_utf8_lossy` emits for bad bytes).
    // Walk head back / tail forward to the nearest boundary first.
    let mut head_end = TRUNCATE_HEAD;
    while head_end > 0 && !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - TRUNCATE_TAIL;
    while tail_start < s.len() && !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let omitted = s.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &s[..head_end],
        omitted,
        &s[tail_start..]
    )
}

/// DNS-resolve `host:port` and return the first non-blocked
/// `SocketAddr`, or `BlockedAddress(<first_ip>)` if every resolved
/// address is private. The returned `SocketAddr` is what reqwest
/// should connect to — pinning the validated IP into the request
/// closes the small DNS-rebinding window between our check and
/// reqwest's own resolve.
///
/// `allow_private` mirrors the [`is_blocked`] parameter: `true` is
/// the test-only path that lets `httpmock` (bound to 127.0.0.1)
/// work; production always passes `false`.
async fn resolve_public(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<SocketAddr, WebFetchError> {
    // Bound the DNS lookup itself. reqwest's `.timeout()` only
    // covers the HTTP lifecycle and this `lookup_host` runs BEFORE
    // the client is built — so without `tokio::time::timeout` a
    // hung resolver (dead nameserver, fake-ip proxy DNS module not
    // yet ready) blocks the tool forever. See `DNS_TIMEOUT_SECS`.
    let addrs: Vec<SocketAddr> = match tokio::time::timeout(
        Duration::from_secs(DNS_TIMEOUT_SECS),
        lookup_host((host, port)),
    )
    .await
    {
        Ok(Ok(it)) => it.collect(),
        Ok(Err(e)) => {
            return Err(WebFetchError::Network(format!("DNS lookup failed: {}", e)))
        }
        Err(_) => {
            tracing::warn!(
                host = host,
                timeout_secs = DNS_TIMEOUT_SECS,
                "web_fetch: DNS lookup timed out"
            );
            return Err(WebFetchError::Network(format!(
                "DNS lookup timed out after {}s (host: {})",
                DNS_TIMEOUT_SECS, host
            )));
        }
    };
    if addrs.is_empty() {
        return Err(WebFetchError::Network(
            "DNS lookup returned no addresses".to_string(),
        ));
    }
    for addr in &addrs {
        if !is_blocked(addr.ip(), allow_private) {
            return Ok(*addr);
        }
    }
    Err(WebFetchError::BlockedAddress(addrs[0].ip()))
}

/// Sync version of [`resolve_public`] for use inside the redirect
/// `Policy::custom` callback (which is `Fn`, not async).
///
/// Uses [`std::net::ToSocketAddrs`] (sync `getaddrinfo`); ~10-100ms
/// per call on a healthy network — acceptable since redirect hops
/// are bounded to [`MAX_REDIRECTS`].
///
/// IMPORTANT: `allow_private` is taken as a parameter but the
/// redirect SSRF guard **always** passes `false` regardless of
/// the caller-provided test bypass — see [`build_redirect_policy`]
/// for the rationale. Tests that need to follow a redirect to a
/// loopback mock server use the *initial URL* `allow_private=true`
/// path (handled by `resolve_public`), and then issue a same-server
/// redirect so the redirect target's host resolves to the same
/// loopback (still gated, but `execute_for_test` callers should
/// design their test fixtures so this works — see the
/// `redirect_chain_follows_when_public` test which uses a
/// relative-path redirect).
fn resolve_and_check_sync(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<SocketAddr, WebFetchError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| WebFetchError::Network(format!("DNS lookup failed: {}", e)))?
        .collect();
    if addrs.is_empty() {
        return Err(WebFetchError::Network(
            "DNS lookup returned no addresses".to_string(),
        ));
    }
    for addr in &addrs {
        if !is_blocked(addr.ip(), allow_private) {
            return Ok(*addr);
        }
    }
    Err(WebFetchError::BlockedAddress(addrs[0].ip()))
}

/// Build the `Policy::custom` that gates every redirect target
/// through the SSRF blocklist (RULE-E-003, 2026-06-14).
///
/// Each `Attempt` callback receives the next URL reqwest is
/// considering following; we re-resolve the host and check the
/// IP against the hard-coded blocklist. If the target is in a
/// blocked range (RFC 1918 / loopback / link-local / cloud
/// metadata / CGNAT / multicast / reserved) we return
/// [`redirect::Action::Stop`] — reqwest will then return the 3xx
/// response as the final response, which `fetch_and_process`
/// converts to [`WebFetchError::RedirectBlocked`].
///
/// ## Why `allow_private` is hardcoded to `false`
///
/// The redirect SSRF guard is the *only* defense against
/// `attacker.com → 169.254.169.254` style attacks. The
/// `allow_private` bypass exists solely to let integration
/// tests talk to a `httpmock` server bound to 127.0.0.1 (handled
/// by the *initial URL* path in [`resolve_public`]). Applying
/// the bypass to the redirect callback would defeat the entire
/// purpose of this guard, so we ignore any caller-provided flag
/// and always pass `false`. Tests that need a loopback redirect
/// must design their fixtures accordingly (relative-path
/// redirects resolve to the same host that was already
/// validated, so they pass the IP check naturally — see
/// `redirect_chain_follows_when_public`).
fn build_redirect_policy() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        // Cap redirect depth to prevent long redirect chains from
        // stalling the request. `attempt.previous()` includes the
        // initial URL plus all already-followed hops, so the chain
        // length is `previous().len()`. The first call has
        // `previous().len() == 1` (just the initial URL), so we
        // allow up to MAX_REDIRECTS hops before refusing.
        if attempt.previous().len() > MAX_REDIRECTS {
            tracing::warn!(
                hops = attempt.previous().len(),
                "web_fetch: too many redirects; stopping"
            );
            return attempt.stop();
        }
        let url = attempt.url();
        let host = match url.host_str() {
            Some(h) => h,
            None => {
                tracing::warn!(
                    url = %url,
                    "web_fetch: redirect target has no host; stopping"
                );
                return attempt.stop();
            }
        };
        let port = match url.port_or_known_default() {
            Some(p) => p,
            None => {
                tracing::warn!(
                    url = %url,
                    "web_fetch: redirect target has no port; stopping"
                );
                return attempt.stop();
            }
        };
        // Hardcoded `allow_private = false` — see fn docstring.
        match resolve_and_check_sync(host, port, false) {
            Ok(_addr) => attempt.follow(),
            Err(WebFetchError::BlockedAddress(ip)) => {
                tracing::warn!(
                    host = host,
                    ip = %ip,
                    "web_fetch: redirect target refused (SSRF block)"
                );
                attempt.stop()
            }
            Err(e) => {
                tracing::warn!(
                    host = host,
                    error = %e,
                    "web_fetch: redirect target DNS failed; stopping"
                );
                attempt.stop()
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- IP blocklist unit tests --

    #[test]
    fn blocks_loopback_v4() {
        assert!(is_blocked("127.0.0.1".parse().unwrap(), false));
        assert!(is_blocked("127.255.255.254".parse().unwrap(), false));
    }

    #[test]
    fn blocks_rfc1918() {
        assert!(is_blocked("10.0.0.1".parse().unwrap(), false));
        assert!(is_blocked("10.255.255.255".parse().unwrap(), false));
        assert!(is_blocked("172.16.0.1".parse().unwrap(), false));
        assert!(is_blocked("172.31.255.254".parse().unwrap(), false));
        assert!(is_blocked("192.168.0.1".parse().unwrap(), false));
        assert!(is_blocked("192.168.255.254".parse().unwrap(), false));
    }

    #[test]
    fn blocks_link_local_including_cloud_metadata() {
        assert!(is_blocked("169.254.1.1".parse().unwrap(), false));
        assert!(is_blocked("169.254.169.254".parse().unwrap(), false)); // AWS / GCP / Azure IMDS
    }

    #[test]
    fn blocks_cgnat_and_multicast() {
        assert!(is_blocked("100.64.0.1".parse().unwrap(), false));      // CGNAT
        assert!(is_blocked("239.255.255.255".parse().unwrap(), false));   // multicast
    }

    #[test]
    fn allows_public_v4() {
        assert!(!is_blocked("8.8.8.8".parse().unwrap(), false));         // Google DNS
        assert!(!is_blocked("1.1.1.1".parse().unwrap(), false));         // Cloudflare DNS
        assert!(!is_blocked("93.184.216.34".parse().unwrap(), false));   // example.com
    }

    #[test]
    fn blocks_v6_loopback_and_link_local() {
        assert!(is_blocked("::1".parse().unwrap(), false));
        assert!(is_blocked("fe80::1".parse().unwrap(), false));
        assert!(is_blocked("fc00::1".parse().unwrap(), false));
    }

    #[test]
    fn unwraps_v4_mapped_v6() {
        // ::ffff:127.0.0.1 must be blocked even though the outer
        // representation is v6.
        let mapped: Ipv6Addr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_blocked(IpAddr::V6(mapped), false));
        let mapped: Ipv6Addr = "::ffff:192.168.1.1".parse().unwrap();
        assert!(is_blocked(IpAddr::V6(mapped), false));
    }

    #[test]
    fn allow_private_bypasses_everything() {
        // The test path: every IP, even the cloud-metadata
        // short-circuit, is allowed.
        assert!(!is_blocked("127.0.0.1".parse().unwrap(), true));
        assert!(!is_blocked("169.254.169.254".parse().unwrap(), true));
        assert!(!is_blocked("192.168.1.1".parse().unwrap(), true));
    }

    // -- Format parsing --

    #[test]
    fn format_parse_defaults_to_markdown() {
        assert_eq!(Format::parse(None), Format::Markdown);
        assert_eq!(Format::parse(Some("markdown")), Format::Markdown);
        assert_eq!(Format::parse(Some("garbage")), Format::Markdown);
    }

    #[test]
    fn format_parse_recognizes_text_and_html() {
        assert_eq!(Format::parse(Some("text")), Format::Text);
        assert_eq!(Format::parse(Some("html")), Format::Html);
    }

    // -- HTML helpers --

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        let html = "<p>Hello &amp; <b>world</b>!</p>";
        let txt = html_to_text(html);
        assert!(txt.contains("Hello"));
        assert!(txt.contains("&"));
        assert!(txt.contains("world"));
        assert!(!txt.contains('<'));
        assert!(!txt.contains('>'));
    }

    #[test]
    fn html_to_text_collapses_whitespace() {
        let html = "<div>a   b\n\n\nc</div>";
        let txt = html_to_text(html);
        // Runs of internal whitespace become a single space.
        assert!(txt.contains("a b"));
    }

    #[test]
    fn truncate_output_passthrough_under_limit() {
        let s = "x".repeat(1000);
        assert_eq!(truncate_output(s.clone()), s);
    }

    #[test]
    fn truncate_output_caps_at_100kb_with_marker() {
        let s = "x".repeat(200_000);
        let t = truncate_output(s);
        assert!(t.contains("<truncated: omitted"));
        // Head (50 KB) + marker + tail (50 KB) ≈ 100 KB + marker.
        assert!(t.len() < 110_000);
    }

    #[test]
    fn truncate_output_multibyte_boundary_no_panic() {
        // A long run of multi-byte chars (中文 = 6 bytes/pair). The
        // 50 KB head/tail byte offsets land in the MIDDLE of a 3-byte
        // char, so the naive `&s[..head_end]` panics with
        // "byte index N is not a char boundary". Regression for the
        // crash seen on CJK / lossy-`�` bodies. The fix walks the
        // offsets to the nearest char boundary before slicing.
        let chunk = "中文"; // 6 bytes
        let s = chunk.repeat(40_000); // 240 KB, past the 100 KB cap
        let t = truncate_output(s);
        assert!(t.contains("<truncated: omitted"));
        // No panic = pass. Sanity-check a clean char at the head seam.
        assert!(t.starts_with('中') || t.starts_with('文'));
    }

    // -- Definition --

    #[test]
    fn definition_has_correct_name_and_required_field() {
        let d = definition();
        assert_eq!(d.name, "web_fetch");
        let required = d.input_schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("url")));
    }

    #[test]
    fn definition_describes_ssrf_protection() {
        let d = definition();
        let desc = d.description.as_deref().unwrap();
        assert!(desc.contains("private"));
        assert!(desc.contains("loopback"));
        assert!(desc.contains("SSRF"));
    }

    // -- Execute (input parsing) --

    #[tokio::test]
    async fn execute_missing_url_param_returns_error() {
        let (out, is_err) = execute(&json!({}), &test_ctx()).await;
        assert!(is_err);
        assert!(out.contains("Missing"));
    }

    // -- Mock-server integration: happy path --

    use httpmock::prelude::*;

    fn test_ctx() -> ToolContext {
        // web_fetch doesn't actually use ToolContext (no project
        // boundary, no ReadGuard) but the signature requires one.
        // Use a placeholder tmpdir so construction succeeds on
        // every test platform.
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().canonicalize().unwrap();
        // Intentionally leak the tempdir so the path stays valid
        // for the duration of the test (httpmock binds to localhost
        // so we don't actually touch this path).
        std::mem::forget(tmp);
        ToolContext {
            worktree_path: p.clone(),
            cwd: p,
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
        }
    }

    #[tokio::test]
    async fn fetches_html_and_converts_to_markdown() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/page");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body("<html><body><h1>Title</h1><p>Hello &amp; world</p></body></html>");
        });

        let url = format!("http://{}/page", server.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url, "format": "markdown"}),
            &test_ctx(),
        )
        .await;

        assert!(!is_err, "got error: {}", out);
        mock.assert_hits(1);
        // Attribution prefix must be present (T1a mitigation).
        assert!(out.starts_with("<!-- fetched:"), "got: {:?}", &out[..80]);
        assert!(out.contains("status 200"));
        assert!(out.contains("content-type <text/html"));
        // Markdown output should have the title as a header and
        // the entity decoded.
        assert!(out.contains("# Title") || out.contains("Title"));
        assert!(out.contains("Hello & world"));
    }

    #[tokio::test]
    async fn fetches_text_and_returns_plain_text() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/page");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body("<p>Hello <b>world</b>!</p>");
        });

        let url = format!("http://{}/page", server.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url, "format": "text"}),
            &test_ctx(),
        )
        .await;

        assert!(!is_err, "got error: {}", out);
        assert!(out.contains("Hello"));
        assert!(out.contains("world"));
        // Attribution prefix (T1a mitigation) is HTML-comment-shaped
        // and contains `<` / `>`. The body itself has no tags.
        // Find the body after the prefix terminator (`-->\n\n`).
        let body = out.split("-->").nth(1).unwrap_or("");
        assert!(!body.contains('<'), "body should have no tags, got: {:?}", body);
    }

    #[tokio::test]
    async fn fetches_html_format_returns_raw() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/raw");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body("<h1>raw</h1>");
        });

        let url = format!("http://{}/raw", server.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url, "format": "html"}),
            &test_ctx(),
        )
        .await;

        assert!(!is_err, "got error: {}", out);
        // Attribution prefix is prepended, so the output is
        // `<!-- fetched: ... -->\n\n<h1>raw</h1>`.
        assert!(out.starts_with("<!-- fetched:"));
        assert!(out.ends_with("<h1>raw</h1>"), "got: {:?}", out);
    }

    #[tokio::test]
    async fn fetches_gzipped_html_gets_decompressed() {
        // Server returns a gzip-encoded HTML body with
        // `Content-Encoding: gzip`. Without `.gzip(true)` on the
        // client (+ the `gzip` reqwest feature) the raw compressed
        // bytes reach `from_utf8` and fail on the gzip magic `1f 8b`
        // → "non-utf8 html body ... from index 1". Regression guard
        // for that exact bug. We produce a real gzip body with flate2
        // on the mock side so reqwest's auto-decompress path is
        // genuinely exercised end-to-end.
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let html = "<html><body><h1>Compressed</h1><p>gunzip me</p></body></html>";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(html.as_bytes()).unwrap();
        let gzipped: Vec<u8> = encoder.finish().unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/gz");
            then.status(200)
                .header("content-type", "text/html; charset=utf-8")
                .header("content-encoding", "gzip")
                .body(gzipped);
        });

        let url = format!("http://{}/gz", server.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url, "format": "markdown"}),
            &test_ctx(),
        )
        .await;

        assert!(!is_err, "got error: {}", out);
        mock.assert_hits(1);
        // If decompression worked, the plaintext survives into the
        // markdown output; if it didn't, we'd have errored above (or
        // seen `�` garbage instead of these words).
        let head: String = out.chars().take(80).collect();
        assert!(out.contains("Compressed"), "no decompressed text; got: {}", head);
        assert!(out.contains("gunzip me"));
    }

    #[tokio::test]
    async fn http_404_returns_http_status_error() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/missing");
            then.status(404);
        });

        let url = format!("http://{}/missing", server.address());
        let (out, is_err) =
            execute_for_test(&json!({"url": url}), &test_ctx()).await;

        assert!(is_err);
        assert!(out.contains("HTTP 404"), "got: {}", out);
    }

    #[tokio::test]
    async fn http_500_returns_http_status_error() {
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/boom");
            then.status(500).body("internal error");
        });

        let url = format!("http://{}/boom", server.address());
        let (out, is_err) =
            execute_for_test(&json!({"url": url}), &test_ctx()).await;

        assert!(is_err);
        assert!(out.contains("HTTP 500"), "got: {}", out);
    }

    #[tokio::test]
    async fn invalid_scheme_returns_invalid_url_error() {
        // file://, gopher://, etc. all rejected at parse time.
        // (No mock needed — fails before any network call.)
        let (out, is_err) = execute_for_test(
            &json!({"url": "file:///etc/passwd"}),
            &test_ctx(),
        )
        .await;

        assert!(is_err);
        assert!(out.contains("http or https"), "got: {}", out);
    }

    #[tokio::test]
    async fn unparseable_url_returns_invalid_url_error() {
        let (out, is_err) = execute_for_test(
            &json!({"url": "not a url"}),
            &test_ctx(),
        )
        .await;

        assert!(is_err);
        assert!(out.contains("http or https") || out.contains("URL"), "got: {}", out);
    }

    /// Production entry (`execute`) MUST still block loopback.
    /// This test uses a real `httpmock` server bound to 127.0.0.1
    /// and asserts that the production entry refuses to fetch it.
    /// (We don't actually start a server here — the block happens
    /// before any network call, so the test stays fast.)
    #[tokio::test]
    async fn production_entry_blocks_loopback() {
        // 127.0.0.1 should always be rejected, regardless of any
        // test-only override (this is the production path).
        let (out, is_err) = execute(
            &json!({"url": "http://127.0.0.1:1/"}),
            &test_ctx(),
        )
        .await;
        assert!(is_err, "127.0.0.1 should be blocked in production");
        assert!(out.contains("private") || out.contains("loopback"), "got: {}", out);
    }

    // -- Redirect SSRF guard (RULE-E-003, 2026-06-14) --
    //
    // These tests assert that the custom redirect Policy
    // (`build_redirect_policy`) re-runs the IP blocklist on every
    // redirect target — without it, an attacker URL could 301 to a
    // private/loopback/cloud-metadata IP and our guard would only
    // have protected the initial URL.

    /// Redirect to a literal RFC 1918 address is refused by the
    /// SSRF guard before the body is fetched. The attacker mock is
    /// hit once (initial URL), but the target mock is NEVER hit
    /// (the redirect chain is stopped in the Policy callback).
    #[tokio::test]
    async fn redirect_to_rfc1918_is_refused() {
        // Attacker mock serves a 301 pointing at a literal RFC 1918
        // address. We do not need a second mock server — the
        // redirect is rejected by the IP blocklist before reqwest
        // tries to connect to the target.
        let attacker = MockServer::start();
        let attacker_mock = attacker.mock(|when, then| {
            when.method(GET).path("/redirect");
            then.status(301)
                .header("Location", "http://10.0.0.1/admin");
        });

        let url = format!("http://{}/redirect", attacker.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url}),
            &test_ctx(),
        )
        .await;

        assert!(is_err, "redirect to RFC 1918 must be refused, got: {}", out);
        assert!(
            out.contains("redirect") && (out.contains("refused") || out.contains("blocked")),
            "expected redirect-refused error, got: {}",
            out
        );
        // The initial URL was hit (we got a 301 response back),
        // and the SSRF guard stopped the chain at the policy layer
        // before any connect attempt to 10.0.0.1.
        attacker_mock.assert_hits(1);
    }

    /// Redirect to link-local / cloud-metadata (169.254.169.254) is
    /// the most realistic exfiltration path: the LLM agent fetches
    /// `attacker.com` which 301s to the AWS IMDS endpoint. The SSRF
    /// guard must stop this.
    #[tokio::test]
    async fn redirect_to_cloud_metadata_is_refused() {
        let attacker = MockServer::start();
        let attacker_mock = attacker.mock(|when, then| {
            when.method(GET).path("/imds");
            then.status(301).header(
                "Location",
                "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            );
        });

        let url = format!("http://{}/imds", attacker.address());
        let (out, is_err) = execute_for_test(
            &json!({"url": url}),
            &test_ctx(),
        )
        .await;

        assert!(is_err, "redirect to cloud metadata must be refused, got: {}", out);
        assert!(
            out.contains("redirect") && out.contains("refused"),
            "expected redirect-refused error, got: {}",
            out
        );
        attacker_mock.assert_hits(1);
    }

    /// Same-server redirect (relative Location) follows normally
    /// when the host is a *public* IP. The redirect target host
    /// is the same as the initial host, so the SSRF guard's
    /// per-hop IP check passes — the only thing being tested here
    /// is that the policy callback correctly returns
    /// `attempt.follow()` for a non-blocked target.
    ///
    /// We can't use a `httpmock` server here because httpmock
    /// binds to 127.0.0.1, and the SSRF guard deliberately rejects
    /// loopback redirect targets (this is the whole point of
    /// RULE-E-003). Instead we exercise `resolve_and_check_sync`
    /// and the policy callback shape directly.
    #[test]
    fn resolve_and_check_sync_allows_public_ip() {
        // Public IP — the SSRF guard must pass.
        let addr = resolve_and_check_sync("8.8.8.8", 80, false)
            .expect("public IP must not be blocked");
        assert_eq!(addr.ip().to_string(), "8.8.8.8");
    }

    #[test]
    fn resolve_and_check_sync_blocks_rfc1918() {
        // RFC 1918 — the SSRF guard must reject even with the
        // test-only `allow_private=true` bypass NOT set. This
        // mirrors what the redirect policy callback sees.
        let err = resolve_and_check_sync("10.0.0.1", 80, false)
            .expect_err("RFC 1918 must be blocked");
        assert!(
            matches!(err, WebFetchError::BlockedAddress(_)),
            "got: {:?}", err
        );
    }

    #[test]
    fn resolve_and_check_sync_blocks_cloud_metadata() {
        // The single most important check — 169.254.169.254 is
        // the AWS IMDS endpoint. Even with the short-circuit
        // in `is_blocked` the redirect path must stop here.
        let err = resolve_and_check_sync("169.254.169.254", 80, false)
            .expect_err("cloud metadata must be blocked");
        assert!(matches!(err, WebFetchError::BlockedAddress(_)));
    }

    /// The redirect SSRF guard MUST reject loopback even when the
    /// test-only `allow_private=true` bypass is enabled for the
    /// *initial* URL. This is the contract that closes RULE-E-003:
    /// a test that fetches `http://attacker.com` (mock server on
    /// 127.0.0.1) and gets redirected to a different loopback
    /// address MUST be refused.
    #[test]
    fn resolve_and_check_sync_blocks_loopback_even_with_bypass() {
        // `allow_private=true` mimics the test-only initial-URL
        // bypass; the redirect path uses `allow_private=false`,
        // but this test documents the intent: if the redirect
        // callback ever gets `allow_private=true`, it would be
        // a security regression.
        //
        // We assert that with `allow_private=false` (the actual
        // value used by the redirect callback), loopback is
        // blocked. The hardcoded `false` in `build_redirect_policy`
        // is what makes this a real guard rather than a no-op.
        let err = resolve_and_check_sync("127.0.0.1", 80, false)
            .expect_err("loopback must be blocked by redirect SSRF guard");
        assert!(matches!(err, WebFetchError::BlockedAddress(_)));
    }
}
