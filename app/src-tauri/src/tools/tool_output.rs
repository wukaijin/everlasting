//! Shared tool-output truncation contract (C6, 2026-08-30).
//!
//! Every tool that bounds its output MUST go through this module —
//! one char-boundary-safe head/tail implementation, one spill path,
//! one truncation-marker format. The contract (spec
//! `pattern-output-truncation`, PRD `08-30-c6-output-truncation`):
//!
//! 1. **Every truncation carries a recovery path** — one of three
//!    sanctioned modes:
//!    - **A spill + path**: full output lands in
//!      `<app_data_dir>/outputs/<session_id>/<uuid>.txt`; the marker
//!      gives the path and the LLM pages through it with
//!      `read_file` offset/limit. For non-replayable results
//!      (shell, web_fetch).
//!    - **B range params**: the tool re-runs with offset/limit/
//!      head_limit; the marker names the params. For replayable
//!      queries (read_file, grep).
//!    - **C narrow hint**: narrow the query (glob-style). Fallback
//!      only, when the result is fully replayable by a narrower
//!      query.
//! 2. **One marker format** (machine-parsable, identical structure
//!    across tools; see [`truncation_marker`]).
//! 3. **RULE-E-009**: head/tail slices must land on UTF-8 char
//!    boundaries — never mid multi-byte sequence. The pre-C6 shell
//!    implementation was the codebase's sole violator (CJK output
//!    > 30 KB panicked at the 1 KB preview slice); its repair is an
//!    intended fix, exempt from byte-for-byte "old behavior"
//!    equivalence (PRD C5).
//!
//! Token-governance layering: this module is the *per-tool*,
//! single-shot defense; the turn-level aggregate defense is the
//! unified context budget at gate ⑤ (see
//! `agent-loop-architecture/pattern-budget-gate`). They own
//! different scopes and never substitute for each other.

use std::io;
use std::path::{Path, PathBuf};

/// Inline cap for read_file / shell inline truncation (head+tail
/// total). Kept per-tool-semantics; centralised here so the value
/// and its rationale have one home (R7).
pub(crate) const INLINE_CAP_BYTES: usize = 50 * 1024;
/// Disk-spill threshold for shell-family outputs (claude-code style
/// "spill to disk" trigger; above this the full output lands in
/// `<data_dir>/outputs/<session_id>/` and the tool result carries a
/// path + preview).
pub(crate) const SPILL_THRESHOLD_BYTES: usize = 30 * 1024;
/// Head/tail preview size for the spilled shell-family result
/// (keeps the tool_result under ~2 KB).
pub(crate) const SPILL_PREVIEW_BYTES: usize = 1024;
/// Inline cap for web_fetch converted content (head+tail total;
/// historically "matches read_file's 50+50 layout" by hand-written
/// comment — now centralised).
pub(crate) const WEB_INLINE_CAP_BYTES: usize = 100 * 1024;

/// Fallback spill directory when a caller has no session id
/// (tests, edge paths). A constant name keeps sweep-by-directory
/// semantics intact.
pub(crate) const NO_SESSION_DIR: &str = "_no_session";

/// Directory holding one session's spilled outputs.
pub fn session_outputs_dir(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir.join("outputs").join(session_id)
}

/// Recovery path attached to a truncation (contract mode A/B/C).
pub enum Recovery<'a> {
    /// Mode A: full output spilled to `path`; page through with
    /// `read_file` offset/limit.
    Spill { path: PathBuf },
    /// Mode B: re-run the tool with range params (hint names them).
    Range { hint: &'a str },
    /// Mode C: narrow the query. Policy knob (C6 R6): glob keeps
    /// its bespoke hint text as the mode-C reference implementation
    /// instead of consuming this variant — the variant stays so the
    /// contract is complete for future consumers.
    #[allow(dead_code)]
    Narrow,
    /// No recovery available — spill failed fallback (shell) or a
    /// tool whose recovery path lands in a later PR (web_fetch
    /// before mode-A spill). Renders the bare "omitted N of M"
    /// segment with no `recover:` tail.
    None,
}

/// Counting unit for the marker's "omitted N of M" segment.
/// `Matches` is exercised by the marker golden tests; non-test
/// consumers currently use the dedicated `hit_limit_marker`.
pub enum Unit {
    Bytes,
    #[allow(dead_code)]
    Matches,
}

/// The single truncation-marker format (R2):
///
/// ```text
/// <truncated: omitted N of M bytes | full output: <path> | recover: read_file with offset/limit>
/// ```
///
/// The `full output:` segment appears only in mode A (Spill); mode
/// B carries a param hint; mode C says "narrow the pattern". A
/// truncation with no recovery available (spill failed fallback)
/// omits both segments.
pub fn truncation_marker(
    omitted: usize,
    total: usize,
    unit: Unit,
    recovery: &Recovery<'_>,
) -> String {
    let unit_s = match unit {
        Unit::Bytes => "bytes",
        Unit::Matches => "matches",
    };
    let head = format!("<truncated: omitted {} of {} {}", omitted, total, unit_s);
    match recovery {
        Recovery::Spill { path } => format!(
            "{} | full output: {} | recover: read_file with offset/limit>",
            head,
            path.display()
        ),
        Recovery::Range { hint } => format!("{} | recover: {}>", head, hint),
        Recovery::Narrow => format!("{} | recover: narrow the pattern>", head),
        Recovery::None => format!("{}>", head),
    }
}

/// Marker variant for a cut whose total is unknown post-cut (e.g.
/// grep `head_limit`): the "omitted N of M" segment degrades to the
/// honest "hit limit of N" form, keeping the `recover:` tail.
pub fn hit_limit_marker(limit: usize, hint: &str) -> String {
    format!(
        "<truncated: hit head_limit of {} matches | recover: {}>",
        limit, hint
    )
}

/// Head+tail truncation with the marker as the middle segment.
/// Slices land on UTF-8 char boundaries (RULE-E-009). Returns `s`
/// unchanged when it fits `head + tail`.
pub fn head_tail_truncate(s: &str, head: usize, tail: usize, marker: &str) -> String {
    let len = s.len();
    if len <= head + tail {
        return s.to_string();
    }
    let head_end = s.floor_char_boundary(head);
    let tail_start = s.ceil_char_boundary(len.saturating_sub(tail));
    if head_end >= tail_start {
        // Degenerate (tiny head/tail vs boundary shifts) — no room
        // for a middle segment; return the head slice alone.
        return s[..head_end].to_string();
    }
    format!("{}\n{}\n{}", &s[..head_end], marker, &s[tail_start..])
}

/// Write the full output to
/// `<data_dir>/outputs/<session_id>/<uuid>.txt` and return the
/// absolute path. Byte input — the most faithful form (background
/// shell stdout/stderr can be non-UTF-8; no lossy conversion on the
/// way to disk). `&str` callers pass `.as_bytes()`.
pub async fn spill(
    data_dir: &Path,
    session_id: Option<&str>,
    contents: &[u8],
) -> io::Result<PathBuf> {
    let dir = session_outputs_dir(data_dir, session_id.unwrap_or(NO_SESSION_DIR));
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, contents).await?;
    Ok(path)
}

/// Remove the session's spill directory on session delete. Missing
/// directory is a no-op (the session never spilled).
pub async fn sweep_session_outputs(data_dir: &Path, session_id: &str) {
    let dir = session_outputs_dir(data_dir, session_id);
    // A missing directory is a no-op (the session never spilled) —
    // same contract as the legacy `cleanup_outputs_dir`; do NOT
    // warn on it or every delete of a non-spilling session logs
    // noise.
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        if e.kind() == std::io::ErrorKind::NotFound {
            return;
        }
        tracing::warn!(
            error = %e,
            spill_dir = %dir.display(),
            "tool_output: failed to sweep session outputs on session delete"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE-E-009 regression, first case: the pre-C6 shell
    /// `head_tail_preview` sliced `&s[..1024]` raw — a >30 KB pure
    /// CJK buffer panics there (3-byte chars never align 1024).
    /// The unified implementation must return normally.
    #[test]
    fn cjk_head_slice_no_panic() {
        let cjk = "汉".repeat(40 * 1024); // ~120 KB, every char 3 bytes
        let marker = truncation_marker(1, cjk.len(), Unit::Bytes, &Recovery::Range { hint: "x" });
        let out = head_tail_truncate(&cjk, 1024, 1024, &marker);
        assert!(out.starts_with('汉'));
        assert!(out.ends_with('汉'));
        assert!(out.contains("<truncated:"));
    }

    /// Property-ish: multibyte soup at every alignment must not
    /// panic and must keep head/tail content.
    #[test]
    fn multibyte_soup_any_alignment_no_panic() {
        // 1-4 byte chars incl. U+FFFD (lossy marker) and emoji.
        let soup: String = ("a汉é😀\u{FFFD}b").repeat(9000);
        for cap in [1usize, 7, 1023, 1024, 4096] {
            let marker = "m";
            let out = head_tail_truncate(&soup, cap, cap, marker);
            assert!(out.contains('\n'), "cap {cap}: marker segment missing");
            // Surviving slices are valid str by construction; spot
            // check first/last payload chars around the markers.
            let mut lines = out.splitn(2, '\n');
            let first = lines.next().unwrap();
            assert!(!first.is_empty() || cap == 0);
        }
    }

    /// Short input passes through untouched (parity with the old
    /// `head_tail_preview` early-return).
    #[test]
    fn short_input_passthrough() {
        let s = "short".to_string();
        assert_eq!(head_tail_truncate(&s, 1024, 1024, "m"), s);
    }

    #[test]
    fn marker_golden_spill_mode() {
        let p = PathBuf::from("/data/outputs/s1/abc.txt");
        let m = truncation_marker(118, 120_000, Unit::Bytes, &Recovery::Spill { path: p });
        assert_eq!(
            m,
            "<truncated: omitted 118 of 120000 bytes | full output: /data/outputs/s1/abc.txt | recover: read_file with offset/limit>"
        );
    }

    #[test]
    fn marker_golden_range_mode() {
        let m = truncation_marker(
            9,
            99,
            Unit::Matches,
            &Recovery::Range {
                hint: "raise head_limit or narrow the pattern",
            },
        );
        assert_eq!(
            m,
            "<truncated: omitted 9 of 99 matches | recover: raise head_limit or narrow the pattern>"
        );
    }

    #[test]
    fn marker_golden_narrow_mode() {
        let m = truncation_marker(42, 142, Unit::Matches, &Recovery::Narrow);
        assert_eq!(
            m,
            "<truncated: omitted 42 of 142 matches | recover: narrow the pattern>"
        );
    }

    #[test]
    fn marker_golden_no_recovery() {
        let m = truncation_marker(8, 58_000, Unit::Bytes, &Recovery::None);
        assert_eq!(m, "<truncated: omitted 8 of 58000 bytes>");
    }

    #[tokio::test]
    async fn spill_writes_session_keyed_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = spill(tmp.path(), Some("sess-42"), b"full output")
            .await
            .unwrap();
        assert!(path.starts_with(session_outputs_dir(tmp.path(), "sess-42")));
        assert!(path.to_string_lossy().ends_with(".txt"));
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"full output");
    }

    #[tokio::test]
    async fn spill_without_session_uses_fallback_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = spill(tmp.path(), None, b"x").await.unwrap();
        assert!(path.starts_with(session_outputs_dir(tmp.path(), NO_SESSION_DIR)));
    }

    #[tokio::test]
    async fn sweep_removes_session_dir_and_tolerates_missing() {
        let tmp = tempfile::tempdir().unwrap();
        spill(tmp.path(), Some("sess-42"), b"x").await.unwrap();
        sweep_session_outputs(tmp.path(), "sess-42").await;
        assert!(!session_outputs_dir(tmp.path(), "sess-42").exists());
        // Missing dir is a no-op, not an error.
        sweep_session_outputs(tmp.path(), "never-spilled").await;
    }
}
