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
    crate::db::migrations::run_migrations(&pool).await.unwrap();
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

/// 08-29-schedule-task-tool (AC6/R6): the scheduling family has NO
/// classify arm by design — `ToolKind::Other` fallthrough = Tier 5
/// silent Allow (compensating controls: per-project cap, worker /
/// group-chat isolation, kill-switch create gate). This test pins
/// the absence of an arm: if someone later routes the family to a
/// Tier 4 kind, the PRD Q2 定案 is being changed and must be
/// re-reviewed.
#[test]
fn classify_tool_keeps_schedule_family_silent_allow() {
    assert_eq!(classify_tool("schedule_task"), ToolKind::Other);
    assert_eq!(classify_tool("schedule_status"), ToolKind::Other);
    assert_eq!(classify_tool("schedule_cancel"), ToolKind::Other);
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
    assert_eq!(classify_tool("run_background_shell"), ToolKind::Shell);
}

/// extract_path_arg reads the `path` key (with `cwd` /
/// `working_directory` fallbacks).
#[test]
fn extract_path_arg_reads_path_key() {
    let v = serde_json::json!({"path": "/abs/path.txt"});
    assert_eq!(
        extract_path_arg("read_file", &v),
        Some("/abs/path.txt".to_string())
    );
}

#[test]
fn extract_path_arg_falls_back_to_cwd() {
    let v = serde_json::json!({"cwd": "/fallback"});
    assert_eq!(
        extract_path_arg("read_file", &v),
        Some("/fallback".to_string())
    );
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
    let (kind, val) = match_value_for_allow_always("read_file", &v, "/Users/me/Documents/notes.md");
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
    let (kind, val) = match_value_for_allow_always("read_file", &v, "notes.md");
    assert_eq!(kind, "path");
    assert_eq!(val, Some("notes.md/*".to_string()));
}

/// match_value_for_allow_always: shell uses first token (Q7).
#[test]
fn match_value_for_allow_always_shell_uses_first_token() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always("shell", &v, "cargo test --release");
    assert_eq!(kind, "prefix");
    assert_eq!(val, Some("cargo".to_string()));
}

/// match_value_for_allow_always: web_fetch always grants
/// the whole tool (per-domain is OOS).
#[test]
fn match_value_for_allow_always_web_fetch_uses_tool() {
    let v = serde_json::json!({});
    let (kind, val) = match_value_for_allow_always("web_fetch", &v, "https://example.com");
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "shell", &input)
        .await
        .expect("recall must succeed on a healthy pool");
    let text = footnote.expect("active hit must produce a footnote");
    assert!(
        text.contains("Memory:"),
        "footnote carries the warning header"
    );
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "grep", &input)
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "shell", &input)
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "shell", &input)
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "shell", &input)
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
    let footnote = recall_pitfall_footnote(&pool, "proj-test", "shell", &input)
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
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope};
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
    let outcome = recall_pitfall(&pool, "proj-test", "edit_file", &input, &blocked).await;
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
    let outcome = recall_pitfall(&pool, "proj-test", "edit_file", &input, &blocked).await;
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
    let outcome = recall_pitfall(&pool, "proj-test", "shell", &input, &blocked).await;
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
    let outcome = recall_pitfall(&pool, "proj-test", "edit_file", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => {
            assert!(
                text.contains("v-second title"),
                "second hit surfaces as footnote"
            );
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
    let outcome = recall_pitfall(&pool, "proj-test", "shell", &input, &blocked).await;
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
    let outcome = recall_pitfall(&pool, "proj-test", "shell", &input, &blocked).await;
    match outcome {
        PitfallRecall::Footnote(text) => {
            assert!(text.contains("v-agnostic title"));
        }
        other => panic!("path/cmd-agnostic verified must be Footnote, got {other:?}"),
    }
}

/// Scope/project isolation for the recall seam (2026-09-02): a
/// project-scope pitfall from proj-a does NOT footnote/soft-block a
/// proj-b session's matching tool call; the same pitfall DOES fire
/// for its own project. Before the fix the trigger SQL had no scope
/// filter at all — cross-project hits also bumped `hit_count`,
/// polluting the P5 promotion input.
#[tokio::test]
async fn recall_pitfall_project_isolation() {
    use crate::db::memories::{test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus};
    use std::collections::HashSet;
    let pool = make_pool().await;
    // Active project-scope pitfall bound to proj-a (the P4 write shape).
    insert_raw(
        &pool,
        "pit-isolation",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "proj-a bound pitfall",
        "isolated pitfall content",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
         WHERE memory_id='pit-isolation'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let input = serde_json::json!({"command": "cargo test --lib"});

    // proj-b session → no recall.
    let footnote = recall_pitfall_footnote(&pool, "proj-b", "shell", &input)
        .await
        .expect("recall must succeed");
    assert!(
        footnote.is_none(),
        "proj-a pitfall must not fire for a proj-b session"
    );

    // proj-a session → recall fires.
    let footnote = recall_pitfall_footnote(&pool, "proj-a", "shell", &input)
        .await
        .expect("recall must succeed");
    let text = footnote.expect("own project's pitfall must fire");
    assert!(text.contains("proj-a bound pitfall"));

    // P5 tiered path shares the isolation (probe as proj-b → None).
    let blocked: HashSet<String> = HashSet::new();
    let outcome = recall_pitfall(&pool, "proj-b", "shell", &input, &blocked).await;
    assert_eq!(outcome, PitfallRecall::None);
}

// =====================================================================
// read-side boundary decouple (2026-07-01): Tier 2.5 deny-list +
// Tier 4 allow-list integration via `check()`.
// =====================================================================

/// Tier 2.5 is a hard wall — it must block sensitive project-outside
/// paths EVEN in Yolo (which otherwise bypasses Tier 4). `~/.ssh/id_rsa`
/// is outside the tempdir project root + matches the deny-list.
#[tokio::test]
async fn tier25_denies_sensitive_outside_even_in_yolo() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let store = new_permission_store();
    let sink: std::sync::Arc<dyn crate::state::ChatEventSink> =
        std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Yolo,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let ssh_key = dirs::home_dir().unwrap().join(".ssh/id_rsa");
    let decision = crate::agent::permissions::check::check(
        &ctx,
        &store,
        &pool,
        &sink,
        "read_file",
        &serde_json::json!({"path": ssh_key}),
        "tu-tier25",
        &token,
    )
    .await;
    assert!(
        decision.is_deny(),
        "yolo must still deny sensitive outside path (Tier 2.5 hard wall)"
    );
}

/// The trusted allow-list (`~/.config/everlasting/**`) must Allow without
/// emitting `permission:ask`. Wrapped in a 5s timeout so a regression
/// (allow-list miss → `ask_path` waits oneshot 120s) fails fast instead
/// of hanging the suite.
#[tokio::test]
async fn tier4_allow_trusted_external_skips_ask() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let trusted = dirs::home_dir()
        .unwrap()
        .join(".config/everlasting/commands/test-b3.md");
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "read_file",
            &serde_json::json!({"path": trusted}),
            "tu-tier4",
            &token,
        ),
    )
    .await
    .expect("regression: allow-list miss would hang on ask_path oneshot");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "trusted external (~/.config/everlasting/**) must Allow without ask"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "trusted external must NOT emit permission:ask"
    );
}

/// Symlink escape: a project-internal symlink pointing at a project-
/// external sensitive file must still be denied. Restores the
/// canonicalize protection the old tool-layer `assert_within_root` had
/// (the lexical deny-list alone can't catch it — `./link → ~/.ssh/id_rsa`
/// reads as project-internal lexically).
#[cfg(unix)]
#[tokio::test]
async fn tier25_denies_symlink_escape_to_sensitive_outside() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let store = new_permission_store();
    let sink: std::sync::Arc<dyn crate::state::ChatEventSink> =
        std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let token = tokio_util::sync::CancellationToken::new();
    let project = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap(); // sibling of project → project-external
    let sensitive = outside_dir.path().join("escape.pem");
    std::fs::write(&sensitive, "FAKE-PRIVATE-KEY").unwrap();
    let link = project.path().join("link.pem");
    std::os::unix::fs::symlink(&sensitive, &link).unwrap();
    let root = project.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Yolo,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = crate::agent::permissions::check::check(
        &ctx,
        &store,
        &pool,
        &sink,
        "read_file",
        &serde_json::json!({"path": "link.pem"}),
        "tu-symlink",
        &token,
    )
    .await;
    assert!(
        decision.is_deny(),
        "symlink → project-outside sensitive must be denied even in yolo"
    );
}

/// The trusted allow-list must also fire when the LLM emits the path in
/// `~/...` form (resolve_path expands before the allow check). Without
/// `~` expansion this would miss the allow-list and hang on ask_path.
#[tokio::test]
async fn tier4_allow_trusted_external_with_tilde_form() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "read_file",
            &serde_json::json!({"path": "~/.config/everlasting/commands/test-b3.md"}),
            "tu-tilde",
            &token,
        ),
    )
    .await
    .expect("regression: ~ not expanded → allow-list miss → ask_path hang");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "~/... form must expand + hit the trusted allow-list"
    );
}

// =====================================================================
// A2+ P1 (2026-07-04): grant short-circuit gate for compound commands.
//
// `ls; rm -rf ~/notes` should NOT enjoy a user's `ls` prefix-grant —
// the structural classifier must run, otherwise the trailing `rm`
// hides behind the benign first token. The gate is the deliberately
// NOT-quote-aware `has_structural_metachar` (a false positive on
// `echo "a;b"` is safe: grant skipped → classify_prefix re-splits
// accurately → correct tier returned, just without the short-circuit
// speed-up).
// =====================================================================

/// Helper: seed a `prefix`-kind grant row for `shell` on the given
/// first-token (`ls`, `cargo`, …). Mirrors what
/// `match_value_for_allow_always` would write on a user's
/// "始终允许" click.
async fn seed_shell_prefix_grant(pool: &sqlx::SqlitePool, session_id: &str, first_token: &str) {
    sqlx::query(
        r#"
        INSERT INTO session_tool_permissions (session_id, tool_name, match_kind, match_value)
        VALUES (?, 'shell', 'prefix', ?)
        "#,
    )
    .bind(session_id)
    .bind(first_token)
    .execute(pool)
    .await
    .expect("seed prefix grant");
}

/// Single-segment `ls` with a prefix grant → Allow (short-circuit
/// fires; the gate does NOT block non-compound commands).
#[tokio::test]
async fn tier4_shell_prefix_grant_short_circuits_for_single_segment() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    seed_shell_prefix_grant(&pool, "parent-sess", "ls").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Plan,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "shell",
            &serde_json::json!({"command": "ls -la"}),
            "tu-grant-single",
            &token,
        ),
    )
    .await
    .expect("regression: grant short-circuit would hang on ask_path oneshot");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "single-segment `ls` with prefix grant must short-circuit to Allow"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "grant short-circuit must NOT emit permission:ask"
    );
}

/// RULE-PERM-002 同族收口(2026-08-27,08-27-rule-smoke-perm-cleanup):
/// AllowAlways 在 `run_background_shell` 上写的是
/// `(run_background_shell, prefix, <token>)` 行(ask.rs 直写原始
/// tool_name),而 `check_prefix_grant` 旧查询硬编码
/// `tool_name='shell'` → 该行永不命中,"始终允许"不粘轮。读侧
/// 放宽为 `IN ('shell','run_background_shell')` 后,单段命令经
/// run_background_shell 也命中 prefix 行短路 Allow。
#[tokio::test]
async fn tier4_prefix_grant_under_run_background_shell_short_circuits() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    // Seed 镜像 ask.rs 的 AllowAlways 写入形状:原始 tool_name 直写。
    sqlx::query(
        r#"
        INSERT INTO session_tool_permissions (session_id, tool_name, match_kind, match_value)
        VALUES (?, 'run_background_shell', 'prefix', 'ls')
        "#,
    )
    .bind("parent-sess")
    .execute(&pool)
    .await
    .expect("seed run_background_shell prefix grant");
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "run_background_shell",
            &serde_json::json!({"command": "ls -la"}),
            "tu-grant-bg-shell",
            &token,
        ),
    )
    .await
    .expect("regression: run_background_shell prefix grant miss would hang on ask_path oneshot");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "single-segment `ls` via run_background_shell with a prefix grant row \
         under the raw tool_name must short-circuit to Allow"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "run_background_shell prefix grant hit must NOT emit permission:ask"
    );
}

/// Compound `ls; rm x` with a prefix grant on `ls` → does NOT
/// short-circuit. Falls through to classify_prefix, which produces
/// Ask (max(ReadOnly, Ask)), so Plan mode surfaces a modal. This is
/// the R1 security fix.
#[tokio::test]
async fn tier4_shell_prefix_grant_does_not_short_circuit_for_compound() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    seed_shell_prefix_grant(&pool, "parent-sess", "ls").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        // Edit mode: SideEffect/Ask would normally modal; we want to
        // confirm the grant short-circuit is BYPASSED, so we expect
        // an ask emission (the modal path).
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    // Fire the check; expect it to reach `ask_path` (which waits on
    // a oneshot). Wrap in a 2s timeout so the test fails fast on
    // regression (grant short-circuit firing → Allow, no ask emitted).
    let compound_input = serde_json::json!({
        "command": "ls; rm /tmp/nonexistent-everlasting-test"
    });
    let check_fut = crate::agent::permissions::check::check(
        &ctx,
        &store,
        &pool,
        &sink_arc,
        "shell",
        &compound_input,
        "tu-grant-compound",
        &token,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), check_fut).await;
    // The check is still pending on the oneshot (timeout elapsed).
    // Assert the ask was emitted (proof the grant did NOT short-circuit).
    let asks = sink.asks.lock().unwrap();
    assert!(
        !asks.is_empty(),
        "compound `ls; rm` must NOT short-circuit the prefix grant — \
         expected a permission:ask emission (R1 security fix)"
    );
}

/// Worker run-grant path: same gate applies. A compound command
/// with a worker prefix run-grant on `ls` must NOT short-circuit.
/// This pins R1 coverage on the worker path (the worker run-grant
/// is the in-memory sibling of the DB prefix grant).
#[tokio::test]
async fn tier4_worker_run_grant_does_not_short_circuit_for_compound() {
    use crate::agent::permissions::run_grant::RunGrantCache;
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let cache = std::sync::Arc::new(RunGrantCache::new());
    // Seed a worker run-grant on `ls` (prefix kind).
    cache.grant_for_run("shell", &serde_json::json!({"command": "ls"}), "ls");
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: true,
        worker_run_id: Some("worker-run-grant-test".to_string()),
        run_grants: Some(cache),
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let compound_input = serde_json::json!({
        "command": "ls; rm /tmp/nonexistent-everlasting-test"
    });
    let check_fut = crate::agent::permissions::check::check(
        &ctx,
        &store,
        &pool,
        &sink_arc,
        "shell",
        &compound_input,
        "tu-worker-grant-compound",
        &token,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), check_fut).await;
    let asks = sink.asks.lock().unwrap();
    assert!(
        !asks.is_empty(),
        "compound `ls; rm` on worker path must NOT short-circuit the \
         run-grant — expected a permission:ask emission (R1 worker coverage)"
    );
}

// =====================================================================
// Isolated-worker inside-anchor regression (2026-07-09 bug fix)
// =====================================================================

/// Regression: an isolated worker's `cwd` is its worktree path
/// (`<app_data_dir>/worktrees/.../worker/<run_id>`), NOT the project
/// root. But the worker reads files by the project root's absolute
/// path (e.g. `/usr/local/code/.../src/main.rs`). Pre-fix the Tier 4
/// Path branch anchored the `is_within_root` inside check on
/// `ctx.cwd` (the worktree) → `inside = false` → every read-only
/// tool went to `ask_path` → the user saw a permission modal for
/// each `read_file` / `grep` / `list_dir` call inside the project.
///
/// Fix: the inside anchor is `ctx.worktree_path` (project root),
/// matching the Tier 2.5 sensitive-path check. This test pins that
/// behavior: a PermissionContext with `cwd ≠ worktree_path` (the
/// isolated-worker shape) reading a project-root file must return
/// `Decision::Allow` WITHOUT emitting `permission:ask`. Wrapped in a
/// 5s timeout so a regression (inside=false → ask_path waits oneshot
/// 120s) fails fast instead of hanging the suite.
#[tokio::test]
async fn isolated_worker_read_project_root_skips_ask() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();

    // project root (the original repo the user opened)
    let project = tempfile::tempdir().unwrap();
    let project_root = project.path().canonicalize().unwrap();
    // a file inside the project root the worker wants to read
    let src_file = project_root.join("src/main.rs");
    std::fs::create_dir_all(src_file.parent().unwrap()).unwrap();
    std::fs::write(&src_file, "fn main() {}").unwrap();

    // the worker's worktree — a SIBLING directory tree, simulating
    // `<app_data_dir>/worktrees/.../worker/<run_id>`. The key property
    // under test: `cwd` is NOT under `project_root` lexically.
    let worktree = tempfile::tempdir().unwrap();
    let worker_cwd = worktree.path().canonicalize().unwrap();

    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: worker_cwd.clone(),
        is_worker: true,
        worker_run_id: Some("worker-iso-regression".to_string()),
        run_grants: None,
        // PRODUCTION WIRING for an isolated worker: `worktree_path` is
        // the worker's own checkout subtree (the sibling `worker_cwd`
        // tempdir above, standing in for `<app_data_dir>/worktrees/...
        // /worker/<run_id>`), NOT the project root. The original test
        // (commit 71976ea, 2026-07-09) hand-filled `worktree_path` with
        // `project_root` — that does NOT match production and masked the
        // anchor bug: check.rs anchored the inside-check on `worktree_path`,
        // so a fake project-root worktree_path made every read pass
        // regardless of the real bug. We now model the real shape and
        // rely on `project_main_path` (below) as the project-root anchor.
        worktree_path: worker_cwd.clone(),
        // project_main_path = the ORIGINAL project repo path. This is the
        // anchor check.rs uses for `is_within_root`; the worker reads the
        // project's source files by their original absolute paths, so this
        // must be the project root, not the worker's checkout subtree.
        project_main_path: project_root.clone(),
        turn_seq: None,
    };

    // The worker reads the project-root file by absolute path (the
    // real-world shape: plugin researcher.md prompts the LLM to read
    // the project's source files). Pre-fix this returned Ask/Deny.
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "read_file",
            // absolute path inside project_root — the LLM-emitted form
            &serde_json::json!({"path": src_file}),
            "tu-iso-worker",
            &token,
        ),
    )
    .await
    .expect("regression: inside=false would hang on ask_path oneshot 120s");

    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "isolated worker reading a project-root file must Allow \
         (inside-anchor = worktree_path = project root)"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "isolated worker reading a project-root file must NOT emit \
         permission:ask"
    );

    // Same invariant for the other read-only path tools (grep / glob /
    // list_dir). One representative assertion per tool is enough — they
    // all share the Tier 4 Path branch.
    for (tool, input) in [
        (
            "grep",
            serde_json::json!({"path": project_root.join("src")}),
        ),
        ("glob", serde_json::json!({"path": project_root})),
        ("list_dir", serde_json::json!({"path": project_root})),
    ] {
        let sink2 = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
        let sink2_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink2.clone();
        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::agent::permissions::check::check(
                &ctx,
                &store,
                &pool,
                &sink2_arc,
                tool,
                &input,
                &format!("tu-iso-worker-{tool}"),
                &token,
            ),
        )
        .await
        .expect("regression: inside=false would hang on ask_path oneshot 120s");
        assert!(
            matches!(decision, crate::agent::permissions::Decision::Allow),
            "isolated worker {tool} on project-root path must Allow"
        );
        assert!(
            sink2.asks.lock().unwrap().is_empty(),
            "isolated worker {tool} on project-root path must NOT emit permission:ask"
        );
    }
}

// =====================================================================
// P3c (09-01-a2-p3c-sandbox-ux): sandbox face short-circuit — when the
// resolved project policy puts the session inside the sandbox, the
// Tier 4 shell approval layers (prefix-grant / classify / ask) are
// replaced by "Allow now, escalate at execution time". Tier 1–3 above
// (kill list / sensitive paths / Plan write block) must NOT be
// replaced. The shared pool pins the backstop project to `off`
// (classic path); these tests re-seed the tier they exercise.
// =====================================================================

/// Seed the backstop project (the `parent-sess` session's default
/// project) to the given tier.
async fn set_project_tier(pool: &sqlx::SqlitePool, tier: &str) {
    sqlx::query("UPDATE projects SET sandbox_policy = ? WHERE id = ?")
        .bind(tier)
        .bind(crate::projects::DEFAULT_PROJECT_ID)
        .execute(pool)
        .await
        .expect("set project sandbox tier");
}

/// AC1: Edit + readwrite tier — an Ask-tier command (`rm x`, would
/// pre-emit a modal in the classic path) short-circuits to silent
/// Allow. The escalation happens later, at execution time, only when
/// the command fails out-of-face.
#[tokio::test]
async fn tier4_shell_short_circuits_to_allow_when_sandbox_face_active() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    set_project_tier(&pool, "readwrite").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "shell",
            &serde_json::json!({"command": "rm x"}),
            "tu-sbx-ask-tier",
            &token,
        ),
    )
    .await
    .expect("short-circuit must bypass ask_path (would hang on oneshot)");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "sandbox face active → shell short-circuits to Allow"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "sandbox face active → NO permission:ask emission"
    );
    // AC8: the Allow decision is audited (ToolAllowed), so the audit
    // trail keeps its "who decided" row even without an ask.
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT kind FROM session_audit_events WHERE session_id = 'parent-sess'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        rows.iter().any(|(k,)| k == "tool_allowed"),
        "short-circuit must record a ToolAllowed audit row, got: {rows:?}"
    );
}

/// D2 invariant: Tier 2 (kill list) runs BEFORE the short-circuit —
/// a fork bomb is silently denied even with the sandbox face active.
#[tokio::test]
async fn tier2_kill_list_still_precedes_sandbox_short_circuit() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    set_project_tier(&pool, "readwrite").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = crate::agent::permissions::check::check(
        &ctx,
        &store,
        &pool,
        &sink_arc,
        "shell",
        &serde_json::json!({"command": ":(){ :|:& };:"}),
        "tu-sbx-forkbomb",
        &token,
    )
    .await;
    match decision {
        crate::agent::permissions::Decision::Deny { critical: true, .. } => {}
        other => panic!("fork bomb must be Tier 2 denied (critical), got: {other:?}"),
    }
}

/// Project `off` tier → classic path byte-identical: an Ask-tier
/// command reaches the ask round-trip (no short-circuit). The ask
/// emission on the captured sink is the signal; the round-trip is
/// completed via `resolve_ask` (mirrors the production permission
/// response path) so the check returns a real Decision.
#[tokio::test]
async fn tier4_shell_classic_ask_path_survives_under_off_tier() {
    use crate::agent::permissions::{
        new_permission_store, resolve_ask, PermissionContext, PermissionResponse,
    };
    // worker_test_pool already pins the project to `off`; make it
    // explicit so the test survives a default flip.
    let pool = super::tests_common::worker_test_pool().await;
    set_project_tier(&pool, "off").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    // `rm x` is Ask-tier → ask_path emits + blocks on the oneshot.
    // Resolve from a sibling task once the emission lands (mirrors
    // the frontend `permission_response` IPC handler).
    let resolver_store = store.clone();
    let resolver_sink = sink.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            if !resolver_sink.asks.lock().unwrap().is_empty() {
                let rid = resolver_sink.asks.lock().unwrap()[0].rid.clone();
                resolve_ask(
                    &resolver_store,
                    &rid,
                    PermissionResponse::Deny {
                        reason: "test".into(),
                    },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "shell",
            &serde_json::json!({"command": "rm x"}),
            "tu-sbx-off-ask",
            &token,
        ),
    )
    .await
    .expect("off tier: classic ask round-trip must complete");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Deny { .. }),
        "off tier: classic Deny after user denial, got: {decision:?}"
    );
    assert!(
        !sink.asks.lock().unwrap().is_empty(),
        "off tier must keep the classic ask emission (no short-circuit)"
    );
}

/// P3c AC4: Plan + sandbox face → shell (any tier, incl. SideEffect)
/// short-circuits to Allow — the Tier 4 Plan branch (SideEffect→Ask,
/// dormant since 2026-06-14) stays dormant. The read-only guarantee
/// comes from the FACE (worktree read-only spec), not from approvals.
#[tokio::test]
async fn tier4_plan_sideeffect_short_circuits_under_sandbox_face() {
    use crate::agent::permissions::{new_permission_store, PermissionContext};
    let pool = super::tests_common::worker_test_pool().await;
    set_project_tier(&pool, "readwrite").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Plan,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "shell",
            &serde_json::json!({"command": "mkdir x"}),
            "tu-sbx-plan-sideeffect",
            &token,
        ),
    )
    .await
    .expect("Plan + face must bypass ask_path");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Allow),
        "Plan + sandbox face → SideEffect shell Allows (sandboxed read-only face)"
    );
    assert!(
        sink.asks.lock().unwrap().is_empty(),
        "Plan + face must NOT emit the (previously active) SideEffect ask"
    );
}

/// P3c AC4 corollary: Plan + project `off` → no face → the tool never
/// reaches check() (filtered by `filter_tools_for_mode` with
/// `plan_shell_available = false`); but IF a tool_use somehow arrives,
/// the policy resolves Off and the shell branch behaves classically.
/// Here: project off + Plan + SideEffect → the dormant Plan branch
/// asks (byte-identical to the pre-P3c behavior).
#[tokio::test]
async fn tier4_plan_sideeffect_still_asks_under_off_tier() {
    use crate::agent::permissions::{
        new_permission_store, resolve_ask, PermissionContext, PermissionResponse,
    };
    let pool = super::tests_common::worker_test_pool().await;
    set_project_tier(&pool, "off").await;
    let store = new_permission_store();
    let sink = std::sync::Arc::new(super::tests_common::CaptureAskSink::default());
    let sink_arc: std::sync::Arc<dyn crate::state::ChatEventSink> = sink.clone();
    let token = tokio_util::sync::CancellationToken::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let ctx = PermissionContext {
        session_id: "parent-sess".to_string(),
        mode: crate::db::Mode::Plan,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let resolver_store = store.clone();
    let resolver_sink = sink.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            if !resolver_sink.asks.lock().unwrap().is_empty() {
                let rid = resolver_sink.asks.lock().unwrap()[0].rid.clone();
                resolve_ask(
                    &resolver_store,
                    &rid,
                    PermissionResponse::Deny {
                        reason: "test".into(),
                    },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
    let decision = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::agent::permissions::check::check(
            &ctx,
            &store,
            &pool,
            &sink_arc,
            "shell",
            &serde_json::json!({"command": "mkdir x"}),
            "tu-sbx-plan-off",
            &token,
        ),
    )
    .await
    .expect("Plan + off tier: classic ask round-trip must complete");
    assert!(
        matches!(decision, crate::agent::permissions::Decision::Deny { .. }),
        "Plan + off tier → dormant Plan branch fires (classic ask), got: {decision:?}"
    );
    assert!(
        !sink.asks.lock().unwrap().is_empty(),
        "Plan + off tier must keep the classic SideEffect ask emission"
    );
}
