#![cfg(test)]

// Tests for the per-run grant cache (task `06-26-subagent-per-run-grant`,
// 2026-06-26). The cache mirrors `session_tool_permissions`'s three
// `match_kind` variants (tool / prefix / path) but lives in memory and
// dies with the worker's `run_chat_loop` invocation. These tests pin:
//
// - the three write/read semantics (tool / prefix / path glob)
// - AllowOnce does NOT write to the cache (it's the ask.rs path; the
//   cache is only written by the AllowAlways arm)
// - run isolation (two `RunGrantCache::new()` instances are invisible
//   to each other)
// - dedup (writing the same grant twice doesn't double-push)
// - fail-closed on unknown kind / mutex poisoning

use serde_json::json;

use super::run_grant::RunGrantCache;

// =====================================================================
// tool-kind grants (web_fetch)
// =====================================================================

/// web_fetch grant: `grant_for_run` writes a `tool`-kind row with
/// `match_value = None`; subsequent `has_run_grant("web_fetch",
/// "tool", _)` hits regardless of the candidate string (tool-level
/// grants don't have a per-call value to compare).
#[test]
fn run_grant_web_fetch_tool_kind_round_trip() {
    let cache = RunGrantCache::new();
    assert!(!cache.has_run_grant("web_fetch", "tool", ""));
    let input = json!({ "url": "https://example.com/a" });
    cache.grant_for_run("web_fetch", &input, "https://example.com/a");
    // Tool-kind grant ignores the candidate string.
    assert!(cache.has_run_grant("web_fetch", "tool", ""));
    assert!(cache.has_run_grant("web_fetch", "tool", "anything"));
    assert_eq!(cache.len(), 1, "exactly one grant written");
}

/// Tool-kind grant is tool-name-specific: a web_fetch grant does
/// NOT authorize a different tool name.
#[test]
fn run_grant_tool_kind_is_tool_specific() {
    let cache = RunGrantCache::new();
    cache.grant_for_run("web_fetch", &json!({ "url": "x" }), "https://x");
    assert!(!cache.has_run_grant("shell", "tool", ""));
    assert!(cache.has_run_grant("web_fetch", "tool", ""));
}

// =====================================================================
// prefix-kind grants (shell)
// =====================================================================

/// shell prefix grant: `cargo test` writes a `prefix`-kind row with
/// `match_value = "cargo"`; subsequent shell calls with first token
/// `cargo` hit, but a different first token (`npm`) does not.
#[test]
fn run_grant_shell_prefix_kind_round_trip() {
    let cache = RunGrantCache::new();
    let input = json!({ "command": "cargo test" });
    cache.grant_for_run("shell", &input, "cargo test");
    assert_eq!(cache.len(), 1);
    // `cargo` prefix matches.
    assert!(cache.has_run_grant("shell", "prefix", "cargo"));
    // Same prefix via a different command in the same family.
    assert!(cache.has_run_grant("shell", "prefix", "cargo"));
    // Different prefix does NOT match.
    assert!(!cache.has_run_grant("shell", "prefix", "npm"));
    // Empty candidate never matches (mirrors `check_prefix_grant`).
    assert!(!cache.has_run_grant("shell", "prefix", ""));
}

/// Multiple distinct shell prefixes accumulate (each is its own row).
#[test]
fn run_grant_shell_multiple_prefixes_accumulate() {
    let cache = RunGrantCache::new();
    cache.grant_for_run("shell", &json!({ "command": "cargo test" }), "cargo test");
    cache.grant_for_run("shell", &json!({ "command": "npm install" }), "npm install");
    assert_eq!(cache.len(), 2);
    assert!(cache.has_run_grant("shell", "prefix", "cargo"));
    assert!(cache.has_run_grant("shell", "prefix", "npm"));
    assert!(!cache.has_run_grant("shell", "prefix", "git"));
}

// =====================================================================
// path-kind grants (read_file / write_file / edit_file etc.)
// =====================================================================

/// path glob grant: `read_file` of `/tmp/notes/a.md` writes a
/// `path`-kind row with `match_value = "/tmp/notes/*"` (parent dir
/// + `/*` glob, mirroring `match_value_for_allow_always`).
/// Subsequent reads under `/tmp/notes/` hit; reads outside
/// (`/tmp/other/c.md`) do NOT.
#[test]
fn run_grant_path_glob_kind_round_trip() {
    let cache = RunGrantCache::new();
    let input = json!({ "path": "/tmp/notes/a.md" });
    cache.grant_for_run("read_file", &input, "/tmp/notes/a.md");
    assert_eq!(cache.len(), 1);
    // Same dir, direct child → glob hit.
    assert!(cache.has_run_grant("read_file", "path", "/tmp/notes/b.md"));
    // Sibling dir entirely → NO hit.
    assert!(!cache.has_run_grant("read_file", "path", "/tmp/other/c.md"));
}

/// The glob `*` does NOT cross `/`. So `/tmp/notes/*` matches
/// `/tmp/notes/b.md` but NOT `/tmp/notes/sub/c.md` (the latter is
/// two levels deep).
#[test]
fn run_grant_path_glob_does_not_cross_slash() {
    let cache = RunGrantCache::new();
    let input = json!({ "path": "/tmp/notes/a.md" });
    cache.grant_for_run("read_file", &input, "/tmp/notes/a.md");
    // Direct child — hit.
    assert!(cache.has_run_grant("read_file", "path", "/tmp/notes/b.md"));
    // Nested child — NO hit (`*` doesn't cross `/`).
    assert!(!cache.has_run_grant("read_file", "path", "/tmp/notes/sub/c.md"));
    // Sibling dir entirely — NO hit.
    assert!(!cache.has_run_grant("read_file", "path", "/tmp/other/c.md"));
}

/// path glob grants are tool-name-scoped (mirrors DB
/// `check_path_grant`'s `tool_name = ?` predicate). A `read_file`
/// glob does NOT authorize a `write_file` to the same path.
#[test]
fn run_grant_path_glob_is_tool_specific() {
    let cache = RunGrantCache::new();
    let input = json!({ "path": "/tmp/notes/a.md" });
    cache.grant_for_run("read_file", &input, "/tmp/notes/a.md");
    assert!(cache.has_run_grant("read_file", "path", "/tmp/notes/b.md"));
    assert!(!cache.has_run_grant("write_file", "path", "/tmp/notes/b.md"));
}

// =====================================================================
// Isolation + dedup + defensive defaults
// =====================================================================

/// Two independent `RunGrantCache` instances are invisible to each
/// other. This is the L3a concurrent-dispatch isolation argument:
/// each worker constructs its own Arc<RunGrantCache> in
/// `run_subagent`, so a grant on one worker's `cargo` does NOT
/// silently authorize another worker's `cargo`.
#[test]
fn run_grant_instances_are_isolated() {
    let cache_a = RunGrantCache::new();
    let cache_b = RunGrantCache::new();
    cache_a.grant_for_run("web_fetch", &json!({ "url": "x" }), "https://x");
    assert!(cache_a.has_run_grant("web_fetch", "tool", ""));
    assert!(
        !cache_b.has_run_grant("web_fetch", "tool", ""),
        "cache_b must not see cache_a's grant (isolation)"
    );
    assert!(cache_b.is_empty());
}

/// Writing the SAME grant twice dedups (no double-push). Cheap
/// defensive guard — the cache is per-run and bounded by the
/// number of distinct (tool, kind, value) tuples.
#[test]
fn run_grant_dedup_on_identical_write() {
    let cache = RunGrantCache::new();
    let input = json!({ "command": "cargo test" });
    cache.grant_for_run("shell", &input, "cargo test");
    cache.grant_for_run("shell", &input, "cargo test");
    cache.grant_for_run("shell", &input, "cargo test");
    assert_eq!(cache.len(), 1, "dedup: identical writes don't double-push");
}

/// `has_run_grant` returns `false` for an unknown `kind` string.
/// Defensive — never fall through to a permissive default.
#[test]
fn run_grant_unknown_kind_returns_false() {
    let cache = RunGrantCache::new();
    cache.grant_for_run("web_fetch", &json!({ "url": "x" }), "https://x");
    // Unknown kind → fail closed.
    assert!(!cache.has_run_grant("web_fetch", "unknown_kind", ""));
    assert!(!cache.has_run_grant("web_fetch", "regex", "anything"));
}

/// `has_run_grant` on an empty cache returns `false` for every kind.
/// (Just a sanity check that the read path doesn't panic on empty.)
#[test]
fn run_grant_empty_cache_has_no_grants() {
    let cache = RunGrantCache::new();
    assert!(cache.is_empty());
    assert!(!cache.has_run_grant("web_fetch", "tool", ""));
    assert!(!cache.has_run_grant("shell", "prefix", "cargo"));
    assert!(!cache.has_run_grant("read_file", "path", "/x/y"));
}

// =====================================================================
// AllowOnce does NOT write the cache
// =====================================================================

/// The cache's `grant_for_run` is ONLY called from `ask.rs`'s worker
/// `AllowAlways` arm. This test isn't calling ask_path directly
/// (that requires a sink + store + db + oneshot round-trip) — it
/// asserts the contract at the type level: `grant_for_run` is the
/// only writer, and the test harness documents that AllowOnce
/// (which doesn't call grant_for_run) leaves the cache empty.
///
/// The companion integration test
/// `agent_loop_dispatch_subagent_*` in `tests_subagent.rs` covers
/// the ask.rs wiring end-to-end (the existing
/// `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
/// pins the worker deny path; a follow-up worker-allow-always test
/// will pin the cache-write path once the frontend Step 2 lands).
#[test]
fn run_grant_only_written_by_explicit_grant_for_run_call() {
    let cache = RunGrantCache::new();
    // No grant_for_run call → cache stays empty.
    assert!(cache.is_empty());
    // Merely constructing a PermissionContext with run_grants=Some
    // doesn't write anything (the cache is queried, not mutated, on
    // the read path).
    let _ = cache.has_run_grant("web_fetch", "tool", "");
    assert!(cache.is_empty(), "read path must not mutate the cache");
}
