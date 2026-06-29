#![cfg(test)]

// The Tier 4 helpers (`classify_tool` / `extract_path_arg` /
// `sqlite_glob_match` / `match_value_for_allow_always`) are
// `pub(crate)` so the test reaches them through the module path.

use sqlx::SqlitePool;

use crate::agent::permissions::check::{
    classify_tool, extract_path_arg, match_value_for_allow_always, recall_pitfall,
    recall_pitfall_footnote, sqlite_glob_match, PitfallRecall, ToolKind,
};
use crate::agent::permissions::risk_for_tool;
use crate::agent::permissions::Risk;

/// P3 (2026-06-29, 06-29-am-p3-tool-recall): in-memory pool with
/// migrations + FK pragma. Local helper so this test file stays
/// independent from the `db/*_tests.rs` family (project
/// convention: each domain owns its pool setup).
async fn make_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool)
        .await
        .unwrap();
    pool
}

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
    // L3b PR3 (2026-06-27): merge_worker / discard_worker route to
    // GitMutation (tool-level grant + ask), NOT Shell.
    assert_eq!(classify_tool("merge_worker"), ToolKind::GitMutation);
    assert_eq!(classify_tool("discard_worker"), ToolKind::GitMutation);
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

/// L3b PR3 (2026-06-27): merge_worker / discard_worker rewrite the
/// parent session's git branch — High risk (same tier as shell).
#[test]
fn risk_for_tool_includes_merge_discard_high() {
    assert_eq!(risk_for_tool("merge_worker"), Risk::High);
    assert_eq!(risk_for_tool("discard_worker"), Risk::High);
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

// =====================================================================
// P3 (2026-06-29, 06-29-am-p3-tool-recall): Tier 1 Hooks —
// pre-tool pitfall recall. These tests cover the `recall_pitfall_footnote`
// helper that hooks Tier 1 (currently no-op) with a
// `find_pitfalls_by_trigger` probe + a footnote string builder.
// =====================================================================

/// P3 AC: an active pitfall whose `tool_name` matches the LLM's
/// tool_use produces a non-empty footnote string. Mirrors the
/// "手写/P4 产出一条 pitfall → agent 跑同名命令 → 工具执行前命中
/// → tool_result 注脚回填可见" acceptance flow from prd.md.
#[tokio::test]
async fn recall_pitfall_footnote_active_hit_returns_text() {
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus, test_helpers::insert_raw};
    let pool = make_pool().await;
    // Path-agnostic active pitfall for the `shell` tool with
    // `command_pattern="cargo test"` — the canonical example
    // from prd.md.
    insert_raw(
        &pool,
        "pit-cargo",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "WSL cargo test needs PKG_CONFIG_PATH",
        "run with PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig cargo test",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
         WHERE memory_id='pit-cargo'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let input = serde_json::json!({"command": "cargo test --lib"});
    let footnote = recall_pitfall_footnote(&pool, "shell", &input)
        .await
        .expect("recall must succeed on a healthy pool");
    let text = footnote.expect("active hit must produce a footnote");
    assert!(text.contains("Memory:"), "footnote carries the warning header");
    assert!(
        text.contains("WSL cargo test needs PKG_CONFIG_PATH"),
        "footnote carries the pitfall title"
    );
    assert!(
        text.contains("PKG_CONFIG_PATH=/usr/lib"),
        "footnote carries the pitfall content"
    );
}

/// P3 AC: an unrelated tool invocation does NOT produce a
/// footnote. The recall probes by `tool_name` exact-match first,
/// so a `shell` pitfall does NOT fire for a `grep` tool_use
/// (irrelevant to the agent's actual action).
#[tokio::test]
async fn recall_pitfall_footnote_unrelated_tool_returns_none() {
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus, test_helpers::insert_raw};
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-shell",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "shell pitfall",
        "shell content",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-shell'")
        .execute(&pool)
        .await
        .unwrap();

    // A `grep` tool_use with no matching pitfall — must return None.
    let input = serde_json::json!({"path": "src/", "pattern": "foo"});
    let footnote = recall_pitfall_footnote(&pool, "grep", &input)
        .await
        .expect("recall must succeed");
    assert!(footnote.is_none());
}

/// P3 AC: a verified-status pitfall is OUT OF SCOPE for P3
/// (verified soft-intercept is P5 scope per spike-007 §4 + P3
/// PRD). The recall helper currently filters `active` only;
/// verified rows must NOT produce a footnote here (the P5 task
/// will extend the helper to handle verified with a separate
/// soft-intercept path).
#[tokio::test]
async fn recall_pitfall_footnote_verified_hit_returns_none_for_p3() {
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus, test_helpers::insert_raw};
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-verified",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Verified,
        "verified pitfall",
        "verified content",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-verified'")
        .execute(&pool)
        .await
        .unwrap();

    let input = serde_json::json!({"command": "anything"});
    let footnote = recall_pitfall_footnote(&pool, "shell", &input)
        .await
        .expect("recall must succeed");
    assert!(
        footnote.is_none(),
        "verified-status rows must NOT produce an active footnote (P5 scope)"
    );
}

/// P3 AC: a candidate pitfall is NOT recalled (the status machine
/// says `candidate` hasn't earned recall surface yet). Only
/// `active` and (P5) `verified` surface.
#[tokio::test]
async fn recall_pitfall_footnote_candidate_hit_returns_none() {
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus, test_helpers::insert_raw};
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-cand",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Candidate,
        "candidate pitfall",
        "candidate content",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-cand'")
        .execute(&pool)
        .await
        .unwrap();

    let input = serde_json::json!({"command": "anything"});
    let footnote = recall_pitfall_footnote(&pool, "shell", &input)
        .await
        .expect("recall must succeed");
    assert!(
        footnote.is_none(),
        "candidate rows must NOT produce a footnote (P5 state machine scope)"
    );
}

/// P3 AC: command_pattern substring mismatch does NOT fire. The
/// pitfall's `command_pattern` is a distinctive substring; the
/// caller's command must contain it for the recall to match.
#[tokio::test]
async fn recall_pitfall_footnote_command_pattern_mismatch_returns_none() {
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus, test_helpers::insert_raw};
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-cargo",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "cargo test pitfall",
        "cargo test content",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
         WHERE memory_id='pit-cargo'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Caller runs `npm install`, NOT `cargo test` → no substring
    // match → no recall.
    let input = serde_json::json!({"command": "npm install"});
    let footnote = recall_pitfall_footnote(&pool, "shell", &input)
        .await
        .expect("recall must succeed");
    assert!(
        footnote.is_none(),
        "command_pattern mismatch must not produce a footnote (precision-first)"
    );
}

/// P3 AC: the recall helper MUST NOT error on a candidate row
/// without `tool_name` set (the SQL filter `WHERE tool_name = ?`
/// will simply return no rows — the recall is graceful, not
/// fallible). The "recall failure MUST NOT block tool execution"
/// rule is exercised more strongly in the live chat_loop.rs path
/// (tracing::warn + continue), but the recall fn itself never
/// returns Err on this case either.
#[tokio::test]
async fn recall_pitfall_footnote_empty_db_returns_none() {
    let pool = make_pool().await;
    let input = serde_json::json!({"command": "cargo test"});
    let footnote = recall_pitfall_footnote(&pool, "shell", &input)
        .await
        .expect("recall must succeed on empty DB");
    assert!(footnote.is_none());
}

// =====================================================================
// P5 (2026-06-29, 06-29-am-p5-quality): tiered pre-tool pitfall recall.
// `recall_pitfall` replaces `recall_pitfall_footnote` at the chat_loop
// call sites. Tiering (design §4 + D1):
//   verified + full trigger-key match + not-yet-blocked → SoftBlock
//   active / candidate / partial / second-hit-on-same-pitfall → Footnote
//   miss → None
// =====================================================================

/// Helper: insert a pitfall row with explicit trigger-key fields.
async fn seed_pitfall(
    pool: &SqlitePool,
    memory_id: &str,
    status: crate::db::memories::MemoryStatus,
    tool: &str,
    command_pattern: Option<&str>,
    path_globs: Option<&str>,
) {
    use crate::db::memories::{MemoryKind, MemoryScope, test_helpers::insert_raw};
    insert_raw(
        pool,
        memory_id,
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        status,
        &format!("{memory_id} title"),
        &format!("{memory_id} content"),
    )
    .await
    .unwrap();
    let cmd_clause = match command_pattern {
        Some(c) => format!(", command_pattern='{c}'"),
        None => String::new(),
    };
    let path_clause = match path_globs {
        Some(p) => format!(", path_globs='{p}'"),
        None => String::new(),
    };
    let sql = format!(
        "UPDATE autonomous_memories SET tool_name='{tool}'{cmd_clause}{path_clause} \
         WHERE memory_id='{memory_id}'"
    );
    sqlx::query(&sql).execute(pool).await.unwrap();
}

/// P5 AC: verified + full trigger-key match (command_pattern set AND
/// contained in the probe command) + NOT in `already_blocked` →
/// SoftBlock. The hint carries the title + content; the `memory_id`
/// is returned so the loop can record it.
#[tokio::test]
async fn p5_recall_verified_full_match_returns_soft_block() {
    use crate::db::memories::MemoryStatus;
    use std::collections::HashSet;
    let pool = make_pool().await;
    seed_pitfall(
        &pool,
        "v-full",
        MemoryStatus::Verified,
        "shell",
        Some("cargo test"),
        Some(r#"["app/*"]"#),
    )
    .await;
    // Probe: command contains "cargo test"; path matches "app/*".
    // (`extract_probe_args` for Shell pulls `command` → command_pattern;
    // path is None for Shell, so the glob check is skipped — but the
    // row's path_globs is Some, which makes is_full_match require
    // path Some. To exercise a true full match we use a Path-kind
    // tool instead — edit_file with a path.)
    // Re-seed as an edit_file pitfall for a clean full-match exercise.
    sqlx::query("DELETE FROM autonomous_memories WHERE memory_id='v-full'")
        .execute(&pool)
        .await
        .unwrap();
    seed_pitfall(
        &pool,
        "v-full",
        MemoryStatus::Verified,
        "edit_file",
        None,
        Some(r#"["app/src/foo.rs"]"#),
    )
    .await;
    let input = serde_json::json!({"path": "app/src/foo.rs", "old_string": "a", "new_string": "b"});
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "edit_file", &input, &blocked).await;
    match outcome {
        PitfallRecall::SoftBlock { hint, memory_id } => {
            assert_eq!(memory_id, "v-full");
            assert!(hint.contains("未实际执行"), "hint says it was NOT executed");
            assert!(hint.contains("v-full title"), "hint carries the title");
        }
        other => panic!("expected SoftBlock, got {other:?}"),
    }
}

/// P5 AC: an active pitfall (even with full trigger-key match) →
/// Footnote, NOT SoftBlock. Only verified soft-blocks.
#[tokio::test]
async fn p5_recall_active_full_match_returns_footnote() {
    use crate::db::memories::MemoryStatus;
    use std::collections::HashSet;
    let pool = make_pool().await;
    seed_pitfall(
        &pool,
        "a-full",
        MemoryStatus::Active,
        "edit_file",
        None,
        Some(r#"["app/src/foo.rs"]"#),
    )
    .await;
    let input = serde_json::json!({"path": "app/src/foo.rs", "old_string": "a", "new_string": "b"});
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "edit_file", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => {
            assert!(text.contains("Memory:"));
            assert!(text.contains("a-full title"));
        }
        other => panic!("expected Footnote for active, got {other:?}"),
    }
}

/// P5 AC: a candidate pitfall → Footnote (the promotion entry point;
/// candidate gets surfaced + bumped, may promote to active per D2).
#[tokio::test]
async fn p5_recall_candidate_returns_footnote() {
    use crate::db::memories::MemoryStatus;
    use std::collections::HashSet;
    let pool = make_pool().await;
    seed_pitfall(
        &pool,
        "c-1",
        MemoryStatus::Candidate,
        "shell",
        Some("cargo test"),
        None,
    )
    .await;
    let input = serde_json::json!({"command": "cargo test --lib"});
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "shell", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => assert!(text.contains("c-1 title")),
        other => panic!("expected Footnote for candidate, got {other:?}"),
    }
}

/// P5 AC + D1 (dead-loop guard): a verified pitfall already in
/// `already_blocked` (already soft-blocked once this session) →
/// degrades to Footnote. The chat loop then executes normally.
#[tokio::test]
async fn p5_recall_verified_second_hit_degrades_to_footnote() {
    use crate::db::memories::MemoryStatus;
    use std::collections::HashSet;
    let pool = make_pool().await;
    seed_pitfall(
        &pool,
        "v-second",
        MemoryStatus::Verified,
        "edit_file",
        None,
        Some(r#"["app/src/foo.rs"]"#),
    )
    .await;
    let input = serde_json::json!({"path": "app/src/foo.rs", "old_string": "a", "new_string": "b"});
    let mut blocked: HashSet<String> = HashSet::new();
    blocked.insert("v-second".to_string());
    let outcome = recall_pitfall(&pool, "edit_file", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => {
            assert!(text.contains("v-second title"), "second hit surfaces as footnote");
        }
        PitfallRecall::SoftBlock { .. } => panic!("second hit must NOT soft-block (D1)"),
        PitfallRecall::None => panic!("verified row should still surface as footnote"),
    }
}

/// P5 AC: no matching pitfall (empty DB or unrelated tool) → None.
#[tokio::test]
async fn p5_recall_no_match_returns_none() {
    use std::collections::HashSet;
    let pool = make_pool().await;
    let input = serde_json::json!({"command": "cargo test"});
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "shell", &input, &blocked).await;
    assert_eq!(outcome, PitfallRecall::None);
}

/// P5 AC: verified but path/command-agnostic (both `command_pattern`
/// AND `path_globs` are `None`) → Footnote, NOT SoftBlock. Such a
/// pitfall is too broad to soft-block (would fire on every
/// invocation of the tool regardless of args).
#[tokio::test]
async fn p5_recall_verified_path_command_agnostic_returns_footnote() {
    use crate::db::memories::MemoryStatus;
    use std::collections::HashSet;
    let pool = make_pool().await;
    // Both fields None — fully path/command-agnostic.
    seed_pitfall(
        &pool,
        "v-agnostic",
        MemoryStatus::Verified,
        "shell",
        None,
        None,
    )
    .await;
    let input = serde_json::json!({"command": "cargo test --lib"});
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "shell", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => {
            assert!(text.contains("v-agnostic title"));
        }
        other => panic!("path/cmd-agnostic verified must be Footnote, got {other:?}"),
    }
}
