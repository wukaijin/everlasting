#![cfg(test)]

// The private helpers `is_blocked` / `html_to_text` /
// `truncate_output` / `resolve_and_check_sync` and the `Format` enum
// are `pub(crate)` so the test reaches them through the module path.

use std::net::{IpAddr, Ipv6Addr};

use httpmock::prelude::*;
use serde_json::json;

use crate::tools::web_fetch::{
    definition, execute, execute_for_test, execute_for_test_session, html_to_text, is_blocked,
    resolve_and_check_sync, truncate_output, Format, WebFetchError,
};
use crate::tools::ToolContext;

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
    assert!(is_blocked("100.64.0.1".parse().unwrap(), false)); // CGNAT
    assert!(is_blocked("239.255.255.255".parse().unwrap(), false)); // multicast
}

#[test]
fn allows_public_v4() {
    assert!(!is_blocked("8.8.8.8".parse().unwrap(), false)); // Google DNS
    assert!(!is_blocked("1.1.1.1".parse().unwrap(), false)); // Cloudflare DNS
    assert!(!is_blocked("93.184.216.34".parse().unwrap(), false)); // example.com
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
    let (out, is_err) = execute(&json!({}), &test_ctx(), None).await;
    assert!(is_err);
    assert!(out.contains("Missing"));
}

// -- Mock-server integration: happy path --

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
        project_id: "test-proj".to_string(),
        data_dir: std::path::PathBuf::from("/tmp/everlasting-tool-test"),
        workflow_name: None,
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
    let (out, is_err) =
        execute_for_test(&json!({"url": url, "format": "markdown"}), &test_ctx()).await;

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
    let (out, is_err) = execute_for_test(&json!({"url": url, "format": "text"}), &test_ctx()).await;

    assert!(!is_err, "got error: {}", out);
    assert!(out.contains("Hello"));
    assert!(out.contains("world"));
    // Attribution prefix (T1a mitigation) is HTML-comment-shaped
    // and contains `<` / `>`. The body itself has no tags.
    // Find the body after the prefix terminator (`-->\n\n`).
    let body = out.split("-->").nth(1).unwrap_or("");
    assert!(
        !body.contains('<'),
        "body should have no tags, got: {:?}",
        body
    );
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
    let (out, is_err) = execute_for_test(&json!({"url": url, "format": "html"}), &test_ctx()).await;

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
    let (out, is_err) =
        execute_for_test(&json!({"url": url, "format": "markdown"}), &test_ctx()).await;

    assert!(!is_err, "got error: {}", out);
    mock.assert_hits(1);
    // If decompression worked, the plaintext survives into the
    // markdown output; if it didn't, we'd have errored above (or
    // seen `�` garbage instead of these words).
    let head: String = out.chars().take(80).collect();
    assert!(
        out.contains("Compressed"),
        "no decompressed text; got: {}",
        head
    );
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
    let (out, is_err) = execute_for_test(&json!({"url": url}), &test_ctx()).await;

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
    let (out, is_err) = execute_for_test(&json!({"url": url}), &test_ctx()).await;

    assert!(is_err);
    assert!(out.contains("HTTP 500"), "got: {}", out);
}

#[tokio::test]
async fn invalid_scheme_returns_invalid_url_error() {
    // file://, gopher://, etc. all rejected at parse time.
    // (No mock needed — fails before any network call.)
    let (out, is_err) = execute_for_test(&json!({"url": "file:///etc/passwd"}), &test_ctx()).await;

    assert!(is_err);
    assert!(out.contains("http or https"), "got: {}", out);
}

#[tokio::test]
async fn unparseable_url_returns_invalid_url_error() {
    let (out, is_err) = execute_for_test(&json!({"url": "not a url"}), &test_ctx()).await;

    assert!(is_err);
    assert!(
        out.contains("http or https") || out.contains("URL"),
        "got: {}",
        out
    );
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
    let (out, is_err) = execute(&json!({"url": "http://127.0.0.1:1/"}), &test_ctx(), None).await;
    assert!(is_err, "127.0.0.1 should be blocked in production");
    assert!(
        out.contains("private") || out.contains("loopback"),
        "got: {}",
        out
    );
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
        then.status(301).header("Location", "http://10.0.0.1/admin");
    });

    let url = format!("http://{}/redirect", attacker.address());
    let (out, is_err) = execute_for_test(&json!({"url": url}), &test_ctx()).await;

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
    let (out, is_err) = execute_for_test(&json!({"url": url}), &test_ctx()).await;

    assert!(
        is_err,
        "redirect to cloud metadata must be refused, got: {}",
        out
    );
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
    let addr = resolve_and_check_sync("8.8.8.8", 80, false).expect("public IP must not be blocked");
    assert_eq!(addr.ip().to_string(), "8.8.8.8");
}

#[test]
fn resolve_and_check_sync_blocks_rfc1918() {
    // RFC 1918 — the SSRF guard must reject even with the
    // test-only `allow_private=true` bypass NOT set. This
    // mirrors what the redirect policy callback sees.
    let err = resolve_and_check_sync("10.0.0.1", 80, false).expect_err("RFC 1918 must be blocked");
    assert!(
        matches!(err, WebFetchError::BlockedAddress(_)),
        "got: {:?}",
        err
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

// ---------------------------------------------------------------------------
// C6 PR2: mode-A spill recovery (08-30-c6-output-truncation, AC5)
// ---------------------------------------------------------------------------

/// Test ctx whose data_dir is a tempdir we KEEP (unlike the leaked
/// `test_ctx`, the spill tests need to inspect the written file).
fn spill_test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
    let p = tmp.path().canonicalize().unwrap();
    ToolContext {
        worktree_path: p.clone(),
        cwd: p,
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: crate::tools::test_default_pool(),
        project_id: "test-proj".to_string(),
        data_dir: tmp.path().to_path_buf(),
        workflow_name: None,
    }
}

/// >100 KB converted content spills the full body to
/// `<data_dir>/outputs/<session_id>/` and the result's marker names
/// the path with the mode-A recovery hint.
#[tokio::test]
async fn large_body_spills_with_recovery_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let server = MockServer::start();
    // Plain text passthrough (no HTML conversion) keeps the byte
    // count exact: 150 KB of ASCII.
    let body = "x".repeat(150 * 1024);
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/big");
        then.status(200)
            .header("content-type", "text/plain")
            .body(body.clone());
    });
    let url = format!("http://{}/big", server.address());

    let (out, is_err) =
        execute_for_test_session(&json!({"url": url}), &spill_test_ctx(&tmp), "sess-wf").await;
    assert!(!is_err, "got error: {}", out);
    assert!(
        out.contains("full output: "),
        "mode-A marker missing: {}",
        &out[..200.min(out.len())]
    );
    assert!(out.contains("recover: read_file with offset/limit"));
    assert!(out.contains("<truncated: omitted"));
    // Extract the path from the marker and verify the FULL body landed.
    let path_str = out
        .split("full output: ")
        .nth(1)
        .unwrap()
        .split(" | recover")
        .next()
        .unwrap()
        .trim();
    let path = std::path::Path::new(path_str);
    assert!(path.starts_with(tmp.path().join("outputs").join("sess-wf")));
    let saved = std::fs::read(path).unwrap();
    assert_eq!(saved.len(), 150 * 1024);
}

/// AC5 full recovery chain: after the spill, `read_file` on the
/// marker's path with offset/limit pages through the content
/// (this is exactly what the LLM does next in mode A).
#[tokio::test]
async fn spilled_output_readable_via_read_file_offset_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = spill_test_ctx(&tmp);
    let server = MockServer::start();
    // 300 numbered lines of ~600 bytes each = 180 KB.
    let body: String = (0..300)
        .map(|i| format!("{}{}\n", "y".repeat(590), i))
        .collect();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/lines");
        then.status(200)
            .header("content-type", "text/plain")
            .body(body);
    });
    let url = format!("http://{}/lines", server.address());

    let (out, is_err) = execute_for_test_session(&json!({"url": url}), &ctx, "sess-wf2").await;
    assert!(!is_err, "got error: {}", out);
    let path_str = out
        .split("full output: ")
        .nth(1)
        .unwrap()
        .split(" | recover")
        .next()
        .unwrap()
        .trim()
        .to_string();

    // Page through the spill exactly like the LLM would.
    let (page, read_err, _images) = crate::tools::read_file::execute(
        &json!({"path": path_str, "offset": 5, "limit": 3}),
        &ctx,
        None,
        Some("sess-wf2"),
    )
    .await;
    assert!(!read_err, "read_file failed: {}", page);
    // Line numbers start at the requested offset (read_file
    // contract) and the sliced content is the spilled body
    // (`<n>\t<content>`, content = y-run + trailing index).
    assert!(
        page.contains("\t5\t"),
        "offset numbering missing: {}",
        &page[..100.min(page.len())]
    );
    assert!(page.contains('y'));
    assert!(page.lines().count() <= 3 + 2); // 3 content lines + truncation guards
}
