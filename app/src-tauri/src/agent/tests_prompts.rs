#![cfg(test)]

use crate::agent::helpers::build_synthetic_tool_result_message;
use crate::agent::system_prompt::build_system_prompt;
use crate::db;
use crate::llm::{ContentBlock, MessageContent, Role};
use crate::projects;

// ---------------------------------------------------------------------------
// build_system_prompt (Step 4 follow-up Bug 3)
// ---------------------------------------------------------------------------

/// Helper to construct a [`db::SessionRow`] with overridable
/// worktree fields. The other fields are hard-coded test
/// fixtures; the production schema is exercised by the DB
/// integration tests in `db.rs`, this helper only needs a
/// well-formed value for the prompt tests below.
fn make_session_row(
    id: &str,
    worktree_state: db::WorktreeState,
    worktree_path: Option<&str>,
) -> db::SessionRow {
    db::SessionRow {
        id: id.to_string(),
        title: "Test Session".to_string(),
        created_at: "2026-06-08T00:00:00Z".to_string(),
        updated_at: "2026-06-08T00:00:00Z".to_string(),
        model: "MiniMax-M2.7".to_string(),
        project_id: "proj-1".to_string(),
        current_cwd: "/test/cwd".to_string(),
        worktree_path: worktree_path.map(str::to_string),
        worktree_state,
        last_worktree_path: None,
        model_id: None,
        input_tokens_total: None,
        output_tokens_total: None,
        cache_creation_total: None,
        cache_read_total: None,
        color_tag: None,
        mode: db::Mode::Edit,
    }
}

/// Helper to construct a [`projects::ProjectRow`] with overridable
/// `is_git_repo` flag.
fn make_project_row(is_git_repo: bool) -> projects::ProjectRow {
    projects::ProjectRow {
        id: "proj-1".to_string(),
        name: "everlasting".to_string(),
        path: "/home/carlos/code/everlasting".to_string(),
        is_git_repo,
        git_branch: if is_git_repo {
            Some("main".to_string())
        } else {
            None
        },
        is_legacy: false,
        created_at: "2026-06-01T00:00:00Z".to_string(),
        updated_at: "2026-06-08T00:00:00Z".to_string(),
        hidden: false,
        metadata: None,
    }
}

/// Step 4 follow-up Bug 3: with an active worktree the prompt
/// names the branch (`session/<id>`), the short HEAD SHA, and
/// the working directory (the worktree path).
/// RULE-E-013 regression guard: `build_system_prompt` must NOT
/// hard-code a tool-name list (the old 7-tool literal drifted and
/// missed 6 registered tools). Tool visibility is the exclusive job
/// of the `tools[]` array sent to the provider; the prompt only
/// states tools are available + the path convention.
#[test]
fn build_system_prompt_no_hardcoded_tool_list() {
    let session = make_session_row("reg-id", db::WorktreeState::None, None);
    let project = make_project_row(true);
    let prompt = build_system_prompt(
        &session,
        &project,
        std::path::Path::new("/home/carlos/code/everlasting"),
        "abc1234",
    );
    assert!(
        !prompt.contains("read_file, write_file"),
        "prompt must not hard-code a tool-name list (RULE-E-013); tool visibility is via tools[]"
    );
    assert!(
        !prompt.contains("(read_file"),
        "prompt must not open a parenthesized tool enumeration"
    );
    assert!(
        prompt.contains("tools defined in this request"),
        "prompt must use the generic capability statement instead of an inline list"
    );
    assert!(
        prompt.contains("relative to the session's working directory"),
        "prompt must keep the path-relativity note"
    );
}

/// PR2 behavior_prompt content: covers the 8 sections + language
/// constraint, recommends `update_checklist` (NOT TodoWrite — §7.2
/// regression guard: the real tool is `update_checklist`).
#[test]
fn behavior_prompt_content_basics() {
    let p = crate::agent::behavior_prompt::DEFAULT_BEHAVIOR_PROMPT;
    for section in [
        "# Tone and style",
        "# Professional objectivity",
        "# Task management",
        "# Tool usage",
        "# Code conventions",
        "# Finishing work",
        "# Git safety",
        "# Language",
    ] {
        assert!(
            p.contains(section),
            "behavior_prompt must contain section {}",
            section
        );
    }
    assert!(
        p.contains("update_checklist"),
        "must recommend update_checklist for task tracking"
    );
    assert!(
        !p.contains("TodoWrite"),
        "must NOT reference TodoWrite (§7.2: the real tool is update_checklist)"
    );
    assert!(
        p.contains("Reply in the user's language"),
        "must include the reply-language constraint (PRD D3)"
    );
}

/// PR2 system-prompt assembly order (cache-stability): behavior
/// guidance, then mode prefix, then per-turn base prompt — stablest
/// layer first so the upstream prompt-cache prefix stays warm.
#[test]
fn assemble_system_prompt_orders_layers_behavior_mode_base() {
    let prompt = crate::agent::system_prompt::assemble_system_prompt("MODE_MARKER", "BASE_MARKER");
    let behavior_pos = prompt
        .find("# Tone and style")
        .expect("behavior section present");
    let mode_pos = prompt.find("MODE_MARKER").expect("mode marker present");
    let base_pos = prompt.find("BASE_MARKER").expect("base marker present");
    assert!(
        behavior_pos < mode_pos,
        "behavior guidance must precede the mode prefix"
    );
    assert!(
        mode_pos < base_pos,
        "mode prefix must precede the per-turn base prompt"
    );
    assert!(
        prompt.starts_with("# Tone and style"),
        "behavior guidance must be the very first thing in the system prompt"
    );
}

#[test]
fn build_system_prompt_active_worktree() {
    let session = make_session_row(
        "test-id",
        db::WorktreeState::Active,
        Some("/data/worktrees/p1/test-id"),
    );
    let project = make_project_row(true);
    let prompt = build_system_prompt(
        &session,
        &project,
        std::path::Path::new("/data/worktrees/p1/test-id"),
        "abc1234",
    );
    assert!(
        prompt.contains("Session ID: test-id"),
        "prompt must name the session"
    );
    assert!(
        prompt.contains("ACTIVE on branch 'session/test-id'"),
        "prompt must label state ACTIVE and include branch name"
    );
    assert!(
        prompt.contains("HEAD abc1234"),
        "prompt must include the short HEAD SHA"
    );
    assert!(
        prompt.contains("Working directory: /data/worktrees/p1/test-id"),
        "prompt must list the worktree path as the working directory"
    );
    assert!(
        prompt.contains("Available tool result envelope"),
        "prompt must describe the tool result envelope"
    );
}

/// Step 4 follow-up Bug 3: with no worktree the prompt labels
/// the state as NONE and uses the project root as the
/// working directory. Does NOT mention "session/<id>" since
/// no branch is active.
#[test]
fn build_system_prompt_no_worktree() {
    let session = make_session_row("test-id", db::WorktreeState::None, None);
    let project = make_project_row(true);
    let prompt = build_system_prompt(
        &session,
        &project,
        std::path::Path::new("/home/carlos/code/everlasting"),
        "abc1234",
    );
    assert!(
        prompt.contains("NONE — running in project root"),
        "prompt must label state NONE"
    );
    assert!(
        prompt.contains("Working directory: /home/carlos/code/everlasting"),
        "working directory must be project root"
    );
    assert!(
        !prompt.contains("ACTIVE"),
        "prompt must not say ACTIVE when state is None"
    );
    assert!(
        !prompt.contains("DETACHED"),
        "prompt must not say DETACHED when state is None"
    );
}

/// Step 4 follow-up Bug 3: a detached worktree retains the
/// branch name + HEAD SHA so the LLM can reason about the
/// previous worktree, but the working directory is the project
/// root since the worktree is unbound.
#[test]
fn build_system_prompt_detached_worktree() {
    let session = make_session_row("det-id", db::WorktreeState::Detached, None);
    let project = make_project_row(true);
    let prompt = build_system_prompt(
        &session,
        &project,
        std::path::Path::new("/home/carlos/code/everlasting"),
        "deadbee",
    );
    assert!(
        prompt.contains("DETACHED — was on branch 'session/det-id'"),
        "prompt must label state DETACHED and reference the old branch"
    );
    assert!(
        prompt.contains("HEAD deadbee"),
        "prompt must include the HEAD short SHA"
    );
    assert!(
        prompt.contains("currently in project root"),
        "prompt must clarify the detached fallback"
    );
    assert!(
        prompt.contains("Working directory: /home/carlos/code/everlasting"),
        "detached's working directory is the project root"
    );
}

/// Step 4 follow-up Bug 3: a non-git project never gets a
/// branch / SHA — the worktree line should say "N/A — non-git
/// project" regardless of the session's `worktree_state`
/// column.
#[test]
fn build_system_prompt_non_git_project() {
    let session = make_session_row("ng-id", db::WorktreeState::None, None);
    let project = make_project_row(false);
    let prompt = build_system_prompt(
        &session,
        &project,
        std::path::Path::new("/some/non/git/dir"),
        "not a git repo",
    );
    assert!(
        prompt.contains("Worktree: N/A — non-git project"),
        "non-git project must show the N/A worktree line"
    );
    assert!(
        !prompt.contains("session/ng-id"),
        "non-git project must not reference a session branch"
    );
}

// ---------------------------------------------------------------------------
// P2 RULE-A-005 (2026-06-24): head_sha refresh after commit
// ---------------------------------------------------------------------------
//
// Pre-fix: `lookup_head_sha` was a one-shot at chat_loop.rs:492
// (pre-fix line), so the 50-turn loop sent a stale SHA on turn 2+
// even after a tool call committed. Post-fix: `head_sha` is refreshed
// at the start of every turn (see chat_loop.rs:732-744 in
// `for turn in 1..=turn_limit`). This test pins the contract
// `lookup_head_sha` + `build_system_prompt` together so the
// refresh-pipeline is exercised end-to-end:
//
//   1. Spin up a temp git repo with commit A → record short SHA-1.
//   2. Commit B on top → record short SHA-2.
//   3. Assert the post-fix refresh path produces SHA-2 in a freshly
//      computed `build_system_prompt` output (i.e. a turn 4 prompt
//      built AFTER the second commit sees SHA-2, not SHA-1).
//
// We don't run `run_chat_loop` here — the `lookup_head_sha` ↔
// `build_system_prompt` glue is the testable surface, and the
// `for turn` loop in `run_chat_loop` is the (uncovered-by-this-test)
// caller. The integration-level rule is covered by
// `tests_subagent::system_prompt_override_*` (which exercises the
// `None` path that drives the per-turn refresh).
#[test]
fn head_sha_refresh_after_commit_updates_system_prompt() {
    use crate::agent::system_prompt::{build_system_prompt, lookup_head_sha};
    use git2::Repository;

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = Repository::init(tmp.path()).expect("init repo");

    // Commit A: write a file + commit on `main`.
    let sig = git2::Signature::now("test", "test@example.com").unwrap();
    {
        let mut index = repo.index().expect("index");
        std::fs::write(tmp.path().join("README.md"), "first commit\n").unwrap();
        index
            .add_path(std::path::Path::new("README.md"))
            .expect("add README");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        // First commit has no parents.
        let _ = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "initial commit",
                &tree,
                &[],
            )
            .expect("commit A");
    }
    let sha_1 = lookup_head_sha(tmp.path());
    assert_ne!(
        sha_1, "not a git repo",
        "first commit must produce a real SHA, not the placeholder"
    );
    assert_ne!(
        sha_1, "no commits yet",
        "first commit must produce a real SHA, not the placeholder"
    );

    // Commit B: amend the file + commit on top.
    {
        let head = repo.head().expect("head").peel_to_commit().expect("peel");
        let mut index = repo.index().expect("index");
        std::fs::write(tmp.path().join("README.md"), "second commit\n").unwrap();
        index
            .add_path(std::path::Path::new("README.md"))
            .expect("add README");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let _ = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "second commit",
                &tree,
                &[&head],
            )
            .expect("commit B");
    }
    let sha_2 = lookup_head_sha(tmp.path());
    assert_ne!(
        sha_1, sha_2,
        "second commit must produce a different SHA; otherwise the refresh is a no-op"
    );

    // Build a system_prompt the way chat_loop.rs would AFTER the
    // per-turn refresh. It must reflect SHA-2 (the new HEAD), not
    // SHA-1 (the pre-refresh stale value).
    let session = make_session_row(
        "rule-a005",
        db::WorktreeState::Active,
        Some(tmp.path().to_str().unwrap()),
    );
    let mut project = make_project_row(true);
    project.path = tmp.path().to_string_lossy().to_string();
    let prompt_after_refresh =
        build_system_prompt(&session, &project, tmp.path(), &sha_2);
    assert!(
        prompt_after_refresh.contains(&format!("HEAD {}", sha_2)),
        "post-refresh system_prompt must carry SHA-2 ({}) — the per-turn \
         refresh path is what makes the LLM see the latest commit",
        sha_2
    );
    assert!(
        !prompt_after_refresh.contains(&format!("HEAD {}", sha_1)),
        "post-refresh system_prompt must NOT carry the stale SHA-1 ({})",
        sha_1
    );
}

// ---------------------------------------------------------------------------
// build_synthetic_tool_result_message (BUG FIX: 2013 tool_use orphan)
// ---------------------------------------------------------------------------

/// One `tool_call` → exactly one matching `ToolResult` block,
/// with the same `id`, `name` echoed in the content, and
/// `is_error: true`. The role is `User` (Anthropic contract:
/// `tool_result` blocks only appear in user-role messages).
#[test]
fn synthetic_tool_result_message_mirrors_tool_calls() {
    let tool_calls = vec![(
        "toolu_abc".to_string(),
        "read_file".to_string(),
        serde_json::json!({"path": "/etc/hosts"}),
    )];
    let msg = build_synthetic_tool_result_message(&tool_calls);
    assert_eq!(msg.role, Role::User, "synthetic message must be User role");
    let blocks = match &msg.content {
        MessageContent::Blocks(b) => b,
        MessageContent::Text(_) => panic!("synthetic message must be Blocks, not Text"),
    };
    assert_eq!(
        blocks.len(),
        1,
        "one tool_call must produce one ToolResult block"
    );
    match &blocks[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "toolu_abc", "tool_use_id must match");
            assert!(
                is_error,
                "synthetic tool_result must be flagged is_error=true"
            );
            assert!(
                content.contains("read_file"),
                "content must name the tool that did not run: {:?}",
                content
            );
            assert!(
                content.contains("interrupted"),
                "content must say the tool was interrupted: {:?}",
                content
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// Three `tool_call`s in one turn → three matching `ToolResult`
/// blocks in order, all flagged is_error=true, all in the
/// same user-role message.
#[test]
fn synthetic_tool_result_message_preserves_order_for_multi_call() {
    let tool_calls = vec![
        (
            "id_1".to_string(),
            "read_file".to_string(),
            serde_json::json!({}),
        ),
        (
            "id_2".to_string(),
            "edit_file".to_string(),
            serde_json::json!({}),
        ),
        (
            "id_3".to_string(),
            "shell".to_string(),
            serde_json::json!({}),
        ),
    ];
    let msg = build_synthetic_tool_result_message(&tool_calls);
    let blocks = match &msg.content {
        MessageContent::Blocks(b) => b,
        _ => panic!("expected Blocks"),
    };
    assert_eq!(blocks.len(), 3);
    let names: Vec<&str> = blocks
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.as_str(),
            _ => panic!("expected ToolResult"),
        })
        .collect();
    assert!(names[0].contains("read_file"));
    assert!(names[1].contains("edit_file"));
    assert!(names[2].contains("shell"));
}

/// Empty `tool_calls` → empty `Blocks` array (and still a User
/// message).
#[test]
fn synthetic_tool_result_message_empty_when_no_tool_calls() {
    let msg = build_synthetic_tool_result_message(&[]);
    assert_eq!(msg.role, Role::User);
    let blocks = match &msg.content {
        MessageContent::Blocks(b) => b,
        _ => panic!("expected Blocks even when empty"),
    };
    assert!(blocks.is_empty());
}

/// Wire shape: the synthetic block must round-trip through
/// serde as a `tool_result` block with `is_error: true` and
/// the expected `tool_use_id`. This is the property the
/// Anthropic API actually validates.
#[test]
fn synthetic_tool_result_message_serializes_to_anthropic_wire_shape() {
    let tool_calls = vec![(
        "toolu_xyz".to_string(),
        "shell".to_string(),
        serde_json::json!({"command": "ls"}),
    )];
    let msg = build_synthetic_tool_result_message(&tool_calls);
    let json = serde_json::to_string(&msg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v.get("role").and_then(|s| s.as_str()), Some("user"));
    let content = v.get("content").and_then(|c| c.as_array()).expect(
        "synthetic message content must be an array of blocks (Anthropic tool_result contract)",
    );
    assert_eq!(content.len(), 1);
    let block = &content[0];
    assert_eq!(
        block.get("type").and_then(|s| s.as_str()),
        Some("tool_result"),
        "wire type must be exactly `tool_result`"
    );
    assert_eq!(
        block.get("tool_use_id").and_then(|s| s.as_str()),
        Some("toolu_xyz")
    );
    assert_eq!(
        block.get("is_error").and_then(|b| b.as_bool()),
        Some(true),
        "is_error: true must serialize (the is_false skip filter only drops false)"
    );
    assert!(
        block
            .get("content")
            .and_then(|s| s.as_str())
            .unwrap()
            .contains("shell"),
        "wire content must mention the tool name"
    );
}

// (A4 follow-up hotfix tests removed in the revert — see
// `06-10-a4-token-per-session-chatinput-hint` task PRD for
// the rationale. The per-turn `db::add_token_usage` call in
// `agent::chat::chat` is unchanged; the cumulative tracker
// + 3 exit-path emits reverted to `usage: None`.)

// ===========================================================================
// P1 RULE-A-006 (2026-06-14): Agent Loop integration tests
// ===========================================================================
//
// The single-function unit tests above exercise individual
// building blocks (cancel token mechanics, system prompt shape,
// synthetic tool_result). They DO NOT cover turn-orchestration
// invariants (cancel mid-turn, MAX_TURNS fallback, C3
// compaction under load, error path emit, tool_use → tool_result
// loop) — those need the full agent loop body running against
