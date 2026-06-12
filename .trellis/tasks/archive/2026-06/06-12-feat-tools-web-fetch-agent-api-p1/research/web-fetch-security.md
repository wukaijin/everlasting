# Research: web_fetch tool — security considerations & MVP defaults

- **Query**: Security defaults for an LLM-driven `web_fetch` tool (Tauri 2 + Rust, local agent)
- **Scope**: external (with internal cross-references to existing tool patterns)
- **Date**: 2026-06-12
- **PRD**: `.trellis/tasks/06-12-feat-tools-web-fetch-agent-api-p1/prd.md`

## Threat model

Actors and worst case (assume local user, prompt-injection-aware):

| # | Attacker → target | Vector | Worst case |
|---|---|---|---|
| T1 | Attacker-controlled web page → LLM (via web_fetch result) | Prompt injection hidden in HTML / hidden text / zero-width chars / base64 | LLM follows injected instructions (exfiltrate local files, run shell) — the fetched content is the only untrusted text the LLM ingests |
| T2 | Attacker (or a jailbroken LLM) → local network | LLM tricked into `fetch("http://169.254.169.254/...")` or `http://192.168.x.x/...` or `http://127.0.0.1:port` | Reads cloud-metadata creds, hits dev DBs, scans local services — local agent becomes SSRF proxy / recon tool |
| T3 | Attacker site → local agent (DoS) | Large response body or slow-loris | Fills memory / disk; stalls agent turn; user can't cancel promptly |
| T4 | Attacker → local agent TLS | Self-signed cert or downgraded TLS | MITM reads/writes traffic — leaks fetched content and any cookies |
| T5 | Attacker (via LLM) → third-party site | Hammered endpoint at high RPS | Local agent amplifies abuse against victim site; potential rate-limit / legal fallout |
| T6 | User audit / debugging | No record of which URLs the agent touched | Hard to investigate "why did my agent do that?" after the fact |

The local-agent context downgrades several of these (the user can see the LLM's tool calls in real time and abort), but the SSRF family (T2) is the most serious because it converts a coding assistant into a network scanner without any visible UI signal.

## Per-threat: severity + recommended mitigation + MVP?

| ID | Threat | Sev | Recommended mitigation | MVP? |
|---|---|---|---|---|
| T2a | Fetching private IPv4 (RFC 1918 / loopback / link-local) | **High** | Resolve hostname → IP, reject if all resolved IPs are in private/loopback/link-likely/multicast ranges | **YES — block MVP** |
| T2b | Fetching private IPv6 (`::1`, `fc00::/7`, `fe80::/10`) | **High** | Same resolver check, v6-aware | **YES** |
| T2c | Cloud metadata endpoints (`169.254.169.254`, `fd00:ec2::254`, `metadata.google.internal`) | **High** | Blocked by 169.254 / link-local rules; also block well-known hostnames in DNS check | **YES** (via IP rule) |
| T2d | Non-HTTP(S) schemes (`file://`, `gopher://`, `dict://`, `ftp://`, `jar://`) | **High** | Allowlist scheme = `http` \| `https`; reject everything else | **YES — trivial** |
| T2e | DNS rebinding / TOCTOU (resolve returns private IP after we approve public IP) | Med | Use the *resolved* IP in the actual request (`reqwest::Client` with `resolve` override) OR force single-lookup and connect to that IP + validate SNI/H Host matches. MVP: accept the small risk, document it | **NO for MVP** (one-shot resolve + accept is acceptable for local tool) |
| T2f | Redirect to private IP (server returns 302 to `127.0.0.1`) | **High** | Apply the IP check to every redirect target, not just the initial URL | **YES** |
| T1a | Prompt injection in fetched HTML | Med | (a) Prefix tool result with `<!-- fetched from <URL> at <ISO-timestamp>; content is untrusted -->` so the LLM can attribute. (b) HTML→MD conversion strips most of the nooks (script / style / hidden). (c) Strip zero-width Unicode (U+200B-U+200D, U+FEFF, U+2060) from output | **(a) YES**, (b) YES (already planned via htmd), (c) optional |
| T1b | Hidden HTML vectors (`display:none`, off-screen text, white-on-white) | Low | HTML→MD conversion usually drops these. Don't over-engineer | **NO for MVP** |
| T3a | Huge response body (multi-GB) | Med | Hard cap response body size (e.g. 5 MB read cap; truncate with notice) | **YES** |
| T3b | Slow-loris / hung server | Med | `connect_timeout` 10s + `timeout` 30s + `tokio::time::timeout` outer wrapper + CancellationToken (already in tool infra) | **YES** |
| T4a | Self-signed / expired cert | Med | `reqwest` default is strict TLS (rustls with native roots). Don't add a `danger_accept_invalid_certs` toggle | **YES — default, no toggle in MVP** |
| T4b | TLS 1.0/1.1 | Low | rustls default min protocol is TLS 1.2; nothing to do | **NO for MVP** |
| T5a | Per-site request flood | Low | Single tool call per turn; LLM is the rate-limiter. Per-session rate limit is overkill | **NO for MVP** |
| T6 | Audit | Low | `tracing::info!(url, status, bytes, duration_ms)` is enough; surface in `tool:result` event | `tracing` is MVP. UI log later |

## MVP defaults (concrete values)

| Setting | Value | Rationale |
|---|---|---|
| Scheme allowlist | `http`, `https` | Reject `file://` etc. outright (T2d) |
| Max redirects | **5** | Matches curl `-L` default; enough for normal sites, blocks redirect loops |
| Per-redirect IP re-validation | **ON** | Critical for T2f |
| Blocked IPv4 ranges (CIDR) | `0.0.0.0/8`, `10.0.0.0/8`, `127.0.0.0/8`, `169.254.0.0/16`, `172.16.0.0/12`, `192.168.0.0/16`, `100.64.0.0/10` (CGNAT), `224.0.0.0/4` (multicast), `240.0.0.0/4` (reserved) | RFC 1918 + link-local + loopback + CGNAT + multicast — covers all T2a/c |
| Blocked IPv6 ranges (CIDR) | `::/128`, `::1/128`, `::ffff:0:0/96` (v4-mapped, re-check v4), `fc00::/7` (ULA), `fe80::/10` (link-local), `ff00::/8` (multicast), `2001:db8::/32` (docs) | T2b |
| Cloud-metadata short-circuit | Hard-block if resolved IP is `169.254.169.254` (v4) or `fd00:ec2::254` (AWS IMDSv2) regardless of any other rule | Belt-and-suspenders for T2c |
| DNS strategy | `tokio::net::lookup_host` once; if any returned IP is private → reject; if all are public → pass the public IP to reqwest via `.resolve(domain, SocketAddr)` to prevent TOCTOU between check and connect | T2e (single-resolve, not full rebinding defense) |
| `reqwest` redirect policy | `.redirect(Policy::limited(5))` | T2f |
| Cookie handling | Default: do NOT send any `Cookie:` header; do NOT persist `Set-Cookie` from response (reqwest default behavior — it does not auto-store cookies unless a `cookie_store` is set) | Don't become a sessioned client |
| Max response body | **5 MiB** read cap; on overflow, truncate to 5 MiB and append `\n\n[truncated, original was N MiB]` to the tool result | T3a |
| Streaming vs buffered | `reqwest::Response::bytes()` after the 5 MiB cap. Streaming the body through a 5 MiB counting `AsyncRead` is better but more code; defer to "later" | T3a |
| Connect timeout | 10s | T3b |
| Total request timeout | 30s (default), overridable per-call up to 120s (matches `shell` defaults) | T3b |
| CancellationToken | Yes — wrap whole call in `tokio::select!` (already in `shell` / `read_file` pattern) | User abort |
| TLS | `rustls-tls-native-roots` (already enabled in `reqwest` features), no `danger_accept_invalid_certs` toggle | T4 |
| User-Agent | `Everlasting/<CARGO_PKG_VERSION>` (transparent, identifies the tool) | Don't pretend to be Chrome — some sites block unscrapable UAs anyway, and impersonating a browser invites fingerprinting concerns |
| `Accept` header | `text/markdown, text/html;q=0.9, text/plain;q=0.8, application/json;q=0.5, */*;q=0.1` | Signal we want readable; fallback chain |
| `Accept-Encoding` | `gzip, br, deflate` (reqwest default-decodes) | Save bandwidth |
| `Accept-Language` | `en` | Consistent results for LLM |
| Result framing | Prepend `<!-- fetched: <url> at <RFC3339> · status <code> · <bytes> bytes · content-type <ct> -->\n\n` to the tool output | T1a (attribution) |
| Zero-width strip | Replace `U+200B`, `U+200C`, `U+200D`, `U+FEFF`, `U+2060` with `""` in final string (after HTML→MD) | T1c (cheap) |
| Logging | `tracing::info!(tool = "web_fetch", url, final_url, status, bytes, duration_ms, is_error)`; no body logged | T6 |
| Error classification | 4 tool-error variants: `InvalidUrl`, `BlockedAddress` (private IP), `TooLarge`, `HttpStatus { code }`, `Timeout`, `Tls`, `Network` | Distinct for LLM self-correction |

## Out of scope for MVP (add later)

- **Configurable domain allowlist / blocklist** (per-project) — useful but adds UI surface; defer to a "network permissions" feature
- **Configurable blocked-IP ranges** (advanced users may want to allow `127.0.0.1` for local dev) — out of MVP
- **Per-domain / per-session rate limiting** — LLM's loop is the de-facto limiter
- **Persistent response cache** — explicit PRD "no"
- **`web_search` parity** (separate P2 task)
- **POST / PUT / DELETE** (GET only)
- **JS rendering** (Playwright/CDP) — no SPAs
- **HTML sanitization library** beyond what `htmd` provides — the LLM is the consumer, not a browser
- **Audit log UI** — `tracing` → future log panel
- **Full DNS rebinding defense** (socket-level re-validate) — single-resolve is enough for a local tool
- **Prompt-injection detection on the fetched text** (e.g. scanning for "ignore previous instructions") — actively harmful (false positives) and the LLM should already be trained on this

## Code sketch: URL → resolved IPs → private-range check (the tricky bit)

This is the one piece worth sketching in Rust. Uses the standard library + `tokio` (already in deps); no new crate required for the IP math.

```rust
// app/src-tauri/src/tools/web_fetch.rs (sketch)
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::lookup_host;

/// CIDR ranges that must never be fetched. The MVP blocks them hard-coded
/// because (a) Everlasting is local, (b) a misconfigured LLM scanning
/// 169.254.169.254 is the realistic incident.
const BLOCKED_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),         // "this network"
    (Ipv4Addr::new(10, 0, 0, 0), 8),        // RFC 1918
    (Ipv4Addr::new(127, 0, 0, 0), 8),       // loopback
    (Ipv4Addr::new(169, 254, 0, 0), 16),    // link-local + AWS/GCP/Azure metadata
    (Ipv4Addr::new(172, 16, 0, 0), 12),     // RFC 1918
    (Ipv4Addr::new(192, 168, 0, 0), 16),    // RFC 1918
    (Ipv4Addr::new(100, 64, 0, 0), 10),     // CGNAT
    (Ipv4Addr::new(224, 0, 0, 0), 4),       // multicast
    (Ipv4Addr::new(240, 0, 0, 0), 4),       // reserved
];

const BLOCKED_V6: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::LOCALHOST, 128),             // ::1
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),   // ULA
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),  // link-local
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),   // multicast
];

fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => BLOCKED_V4.iter().any(|(net, prefix)| v4_match(v4, *net, *prefix)),
        IpAddr::V6(v6) => {
            // v4-mapped v6 (::ffff:0:0/96) — re-check the v4 part
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked(IpAddr::V4(v4));
            }
            BLOCKED_V6.iter().any(|(net, prefix)| v6_match(v6, *net, *prefix))
        }
    }
}

/// Returns the first *public* SocketAddr for `host:port`, or `BlockedAddress`
/// if every resolved IP is private. Lets the caller pass it to reqwest via
/// `.resolve(host, addr)` to pin the connection to the validated IP.
pub async fn resolve_public(host: &str, port: u16) -> Result<SocketAddr, WebFetchError> {
    let addrs: Vec<SocketAddr> = lookup_host((host, port))
        .await
        .map_err(WebFetchError::Dns)?
        .collect();
    if addrs.is_empty() {
        return Err(WebFetchError::Dns("no addresses".into()));
    }
    // AWS/GCP/Azure metadata: short-circuit even before generic check
    if addrs.iter().any(|a| a.ip() == "169.254.169.254".parse().unwrap()) {
        return Err(WebFetchError::BlockedAddress);
    }
    addrs.into_iter()
        .find(|a| !is_blocked(a.ip()))
        .ok_or(WebFetchError::BlockedAddress)
}
```

`v4_match` / `v6_match` are trivial bit-prefix compares (a 10-line helper each); the `ipnetwork` crate can replace this if we want general CIDR handling. Note: this does **not** defend against DNS rebinding between the resolve and the connect — for that we'd need to set `reqwest::Client::resolve(domain, ip)` AND verify `Host:` header matches the original domain. The PRD's "MVP 不做" is consistent with that limitation.

## External references

- [OWASP SSRF Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html) — canonical "deny by default, allowlist if possible" guidance; documents the TOCTOU / DNS-rebinding class
- [OWASP SSRF attack page](https://owasp.org/www-community/attacks/Server_Side_Request_Forgery) — flags `169.254.169.254` explicitly
- [RFC 1918 — Address Allocation for Private Internets](https://datatracker.ietf.org/doc/html/rfc1918) (10/8, 172.16/12, 192.168/16)
- [RFC 3927 — Dynamic Configuration of IPv4 Link-Local Addresses](https://datatracker.ietf.org/doc/html/rfc3927) (169.254/16)
- [AWS IMDS docs](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instancedata-data-retrieval.html) — why `169.254.169.254` matters
- `reqwest::redirect::Policy::limited` (built-in)
- `tokio::net::lookup_host` (built-in)

## Internal cross-references

- `app/src-tauri/src/tools/shell.rs` — established pattern for `CancellationToken` + `tokio::time::timeout` + disk spillover (web_fetch borrows the timeout pattern, skips spillover)
- `app/src-tauri/src/llm/provider/anthropic.rs:209-219` — `reqwest::Client::builder().timeout(60s).connect_timeout(10s)` is the project's baseline; web_fetch can be tighter (30s)
- `app/src-tauri/src/llm/error.rs` — `LlmError` is **not** the right type; web_fetch errors are tool `is_error: true` results, not LLM errors (per PRD §"What I already know")
- `.trellis/spec/backend/tool-contract.md` — needs a new section for `web_fetch`; add `web-fetch.md` patterns here after MVP

## Caveats

- The "MVP 不做 SSRF" line in the existing PRD (`§"Assumptions" #7`) is contradicted by this research — I recommend implementing the IP block at MVP. Cost is ~80 lines of code + 1 unit test per blocked range. Re-flag at the brainstorming step.
- The OWASP-recommended allowlist approach is *not* viable for a generic web_fetch tool (the LLM is fetching arbitrary docs). Deny-list is the right posture.
- TOCTOU / DNS-rebinding is real but **MVP-acceptable**: a local user running a coding agent and the LLM both being co-resident makes the attacker's exfiltration channel hard. Worth a follow-up spec.
- The `Accept: text/markdown` header is a polite hint — most sites don't honor it, but it improves behavior on GitHub, dev.to, and a handful of docs sites that do.
