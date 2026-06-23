#![cfg(test)]

// The Tier 4 helpers (`classify_tool` / `extract_path_arg` /
// `sqlite_glob_match` / `match_value_for_allow_always`) are
// `pub(crate)` so the test reaches them through the module path.

use crate::agent::permissions::check::{
    classify_tool, extract_path_arg, match_value_for_allow_always, sqlite_glob_match, ToolKind,
};
use crate::agent::permissions::risk_for_tool;
use crate::agent::permissions::Risk;

// =====================================================================
// Re-grill 2026-06-13: path-based / prefix / Yolo bypass / Plan
// early-block / match_kind wiring tests.
// =====================================================================

/// classify_tool returns the right variant for every built-in
/// tool. Locked list — a future tool addition must add a
/// classify match arm + a test here.
#[test]
fn classify_tool_dispatch() {
    assert_eq!(classify_tool("read_file"), ToolKind::Path);
    assert_eq!(classify_tool("write_file"), ToolKind::Path);
    assert_eq!(classify_tool("edit_file"), ToolKind::Path);
    assert_eq!(classify_tool("list_dir"), ToolKind::Path);
    assert_eq!(classify_tool("grep"), ToolKind::Path);
    assert_eq!(classify_tool("glob"), ToolKind::Path);
    assert_eq!(classify_tool("shell"), ToolKind::Shell);
    assert_eq!(classify_tool("run_background_shell"), ToolKind::Shell);
    assert_eq!(classify_tool("web_fetch"), ToolKind::WebFetch);
    assert_eq!(classify_tool("unknown_future_tool"), ToolKind::Other);
}

/// L1a (2026-06-19): `run_background_shell` is High risk (same
/// as `shell`). `shell_status` / `shell_kill` are Low (read-only
/// inspection / kill of an already-existing process; no new
/// code is executed).
#[test]
fn risk_for_tool_includes_background_shell_high() {
    assert_eq!(risk_for_tool("run_background_shell"), Risk::High);
    assert_eq!(risk_for_tool("shell_status"), Risk::Low);
    assert_eq!(risk_for_tool("shell_kill"), Risk::Low);
}

/// `run_background_shell` routes through the Tier 4 Shell branch
/// (kill-list + classify_prefix + prefix grants), so a user's
/// "始终允许" grant on `cargo` works for BOTH sync `shell` and
/// async `run_background_shell`. This test guards the routing.
#[test]
fn classify_tool_routes_background_shell_to_shell_kind() {
    assert_eq!(
        classify_tool("run_background_shell"),
        ToolKind::Shell
    );
}

/// extract_path_arg reads the `path` key (with `cwd` /
/// `working_directory` fallbacks).
#[test]
fn extract_path_arg_reads_path_key() {
    let v = serde_json::json!({"path": "/abs/path.txt"});
    assert_eq!(extract_path_arg("read_file", &v), Some("/abs/path.txt".to_string()));
}

#[test]
fn extract_path_arg_falls_back_to_cwd() {
    let v = serde_json::json!({"cwd": "/fallback"});
    assert_eq!(extract_path_arg("read_file", &v), Some("/fallback".to_string()));
}

#[test]
fn extract_path_arg_returns_none_when_missing() {
    let v = serde_json::json!({});
    assert_eq!(extract_path_arg("read_file", &v), None);
}

/// sqlite_glob_match: the *doesn't cross /* rule. This is
/// the core invariant of Tier 4 path-grant matching — a
/// glob `/foo/*` must NOT match `/foo/bar/baz`.
#[test]
fn sqlite_glob_match_star_does_not_cross_slash() {
    assert!(sqlite_glob_match("/foo/*", "/foo/notes.md"));
    assert!(sqlite_glob_match("/foo/*", "/foo/a"));
    // Negative: a nested dir is NOT matched by the parent's
    // single-asterisk glob (sqlite GLOB semantics).
    assert!(!sqlite_glob_match("/foo/*", "/foo/bar/notes.md"));
    assert!(!sqlite_glob_match("/foo/*", "/bar/notes.md"));
}

/// sqlite_glob_match: `?` matches exactly one char.
#[test]
fn sqlite_glob_match_question_mark() {
    assert!(sqlite_glob_match("/foo/?.txt", "/foo/a.txt"));
    assert!(!sqlite_glob_match("/foo/?.txt", "/foo/ab.txt"));
}

/// sqlite_glob_match: empty pattern matches only empty
/// text.
#[test]
fn sqlite_glob_match_empty() {
    assert!(sqlite_glob_match("", ""));
    assert!(!sqlite_glob_match("", "x"));
}

/// sqlite_glob_match: literal pattern (no metachars).
#[test]
fn sqlite_glob_match_literal() {
    assert!(sqlite_glob_match("/foo/bar", "/foo/bar"));
    assert!(!sqlite_glob_match("/foo/bar", "/foo/baz"));
}

/// match_value_for_allow_always: path tools use parent + /*
/// glob. (Q8)
#[test]
fn match_value_for_allow_always_path_uses_parent_glob() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always(
        "read_file", &v, "/Users/me/Documents/notes.md",
    );
    assert_eq!(kind, "path");
    assert_eq!(val, Some("/Users/me/Documents/*".to_string()));
}

/// match_value_for_allow_always: path tools with a relative
/// input still produce a sensible parent glob. (The caller
/// would normally pass an absolute path because the
/// permission layer resolves relative → cwd.join, but the
/// function is robust to either.)
#[test]
fn match_value_for_allow_always_path_basename_only() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always(
        "read_file", &v, "notes.md",
    );
    assert_eq!(kind, "path");
    assert_eq!(val, Some("notes.md/*".to_string()));
}

/// match_value_for_allow_always: shell uses first token (Q7).
#[test]
fn match_value_for_allow_always_shell_uses_first_token() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always(
        "shell", &v, "cargo test --release",
    );
    assert_eq!(kind, "prefix");
    assert_eq!(val, Some("cargo".to_string()));
}

/// match_value_for_allow_always: web_fetch always grants
/// the whole tool (per-domain is OOS).
#[test]
fn match_value_for_allow_always_web_fetch_uses_tool() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always(
        "web_fetch", &v, "https://example.com",
    );
    assert_eq!(kind, "tool");
    assert_eq!(val, None);
}
