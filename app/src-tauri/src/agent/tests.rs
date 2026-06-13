//! Tests previously inlined in `lib.rs` (post-PR1 of the audit
//! task). All tests exercise behavior that lives in
//! [`crate::agent`] modules; the test bodies are unchanged
//! except for the absolute-type paths (e.g. `db::SessionRow` →
//! `crate::db::SessionRow`).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{
    build_synthetic_tool_result_message, cancel_inflight_for_session, tool_result_envelope,
};
use crate::agent::system_prompt::build_system_prompt;
use crate::agent::thinking::flush_pending_thinking;
use crate::agent::thinking::PendingThinking;
use crate::db;
use crate::llm::{ContentBlock, MessageContent, Role};
use crate::projects;
use crate::state::CancellationGuard;

/// Race a slow fake stream against a cancellation token. Mirrors
/// the per-event select! loop in `chat` (minus the SSE plumbing).
/// Asserts cancel wins when fired mid-stream.
#[tokio::test]
async fn select_loop_breaks_on_cancellation() {
    let token = CancellationToken::new();
    let cancelled_flag = Arc::new(std::sync::Mutex::new(false));
    let cancelled_flag_clone = cancelled_flag.clone();
    let token_clone = token.clone();

    // Simulate the per-event select! pattern. Each "event" is
    // tokio::time::sleep; the cancel arm races them.
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = token_clone.cancelled() => {
                    *cancelled_flag_clone.lock().unwrap() = true;
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    // Stream "produced an event" — loop again.
                }
            }
        }
    });

    // Give the loop a tick to start, then cancel.
    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();

    // The select! arm should win within a few ms.
    let joined = tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("select loop should have broken within 500ms")
        .expect("task should not have panicked");
    assert!(
        *cancelled_flag.lock().unwrap(),
        "cancelled flag should be set when select! breaks on cancel"
    );
    // Silence the "joined result unused" warning — the function
    // already returns ().
    let _ = joined;
}

#[tokio::test]
async fn cancellation_token_idempotent() {
    let token = CancellationToken::new();
    token.cancel();
    token.cancel();
    // Second cancel is a no-op; is_cancelled stays true; no panic.
    assert!(token.is_cancelled());
}

/// Mirrors the `cancel_chat` command's lookup logic, isolated
/// from the Tauri State wrapper. Tests that a missing
/// `request_id` is a silent Ok (idempotent) and a present one
/// actually flips the token.
#[tokio::test]
async fn cancel_chat_idempotent_for_missing_and_present() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Missing request_id → no-op, returns Ok.
    let missing = {
        let map = cancellations.lock().await;
        map.get("does-not-exist").cloned()
    };
    assert!(missing.is_none(), "unknown id should not be in map");

    // Present request_id → token fetched, is_cancelled flips.
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-1".to_string(), token.clone());
    }
    let fetched = {
        let map = cancellations.lock().await;
        map.get("rid-1").cloned()
    };
    assert!(fetched.is_some());
    let t = fetched.unwrap();
    assert!(!t.is_cancelled());
    t.cancel();
    assert!(t.is_cancelled());
}

/// Concurrent requests: two `request_id`s are independent. Cancel
/// one; the other should not be affected.
#[tokio::test]
async fn two_concurrent_requests_are_independent() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let a = CancellationToken::new();
    let b = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("a".to_string(), a.clone());
        map.insert("b".to_string(), b.clone());
    }
    // Cancel A.
    {
        let map = cancellations.lock().await;
        let t = map.get("a").cloned();
        if let Some(t) = t {
            t.cancel();
        }
    }
    assert!(a.is_cancelled());
    assert!(!b.is_cancelled(), "B should not be affected by A's cancel");
}

/// CancellationGuard removes the entry on Drop. We construct a
/// guard, drop it, and verify the map is empty. The Drop runs
/// `tauri::async_runtime::spawn`, so the test is wrapped in
/// `#[tokio::test]` to provide a runtime (the guard's spawn
/// borrows the current Tokio runtime via the Tauri shim; in
/// unit tests we route through the global runtime).
#[tokio::test(flavor = "multi_thread")]
async fn cancellation_guard_removes_entry_on_drop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-g".to_string(), CancellationToken::new());
    }
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("sid-g".to_string(), "rid-g".to_string());
    }
    assert_eq!(cancellations.lock().await.len(), 1);
    {
        let _guard = CancellationGuard {
            cancellations: cancellations.clone(),
            session_active_request: session_active_request.clone(),
            request_id: "rid-g".to_string(),
            session_id: "sid-g".to_string(),
        };
        // _guard drops at end of block.
    }
    // Give the spawned cleanup task a moment to run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        cancellations.lock().await.is_empty(),
        "guard's Drop should have removed the cancellations entry"
    );
    assert!(
        session_active_request.lock().await.is_empty(),
        "guard's Drop should have removed the session_active_request entry"
    );
}

/// Step 4 follow-up: `cancel_inflight_for_session` cancels the
/// matching request token when the session has an in-flight
/// request, and is a silent no-op otherwise.
#[tokio::test]
async fn cancel_inflight_for_session_cancels_token() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Register a fake request for session "s1".
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-1".to_string(), token.clone());
    }
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("s1".to_string(), "rid-1".to_string());
    }
    assert!(!token.is_cancelled());
    cancel_inflight_for_session(&cancellations, &session_active_request, "s1").await;
    assert!(
        token.is_cancelled(),
        "matching request's token should be cancelled"
    );
}

/// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
/// when the session has no in-flight request.
#[tokio::test]
async fn cancel_inflight_for_session_missing_session_is_noop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Nothing registered.
    cancel_inflight_for_session(&cancellations, &session_active_request, "s-missing").await;
    // No panic, no state change. (Asserts on the maps being
    // empty would be a tautology given the setup, but the
    // function returning is the actual contract.)
    assert!(cancellations.lock().await.is_empty());
    assert!(session_active_request.lock().await.is_empty());
}

/// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
/// when the session has a request_id but the matching
/// cancellation token is already gone (rare race: the
/// request finished between the map reads).
#[tokio::test]
async fn cancel_inflight_for_session_token_gone_is_noop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // session_active_request has the entry, but the
    // cancellations map doesn't (the request already
    // finished and the CancellationGuard cleaned up).
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("s1".to_string(), "rid-gone".to_string());
    }
    cancel_inflight_for_session(&cancellations, &session_active_request, "s1").await;
    // No panic; the function is best-effort.
}

/// Step 4 follow-up (REQ-16): the tool result envelope has
/// exactly the shape `{"result": <content>, "cwd": <path>}`.
/// This is the LLM-facing contract — the LLM gets the cwd so
/// it can correlate tool results with the worktree state.
/// The frontend's `extractToolResultDisplay` parses this same
/// shape; a regression here would leak the raw JSON into the
/// UI.
#[test]
fn tool_result_envelope_round_trip() {
    let path = std::path::Path::new("/data/worktrees/p1/s1");
    let env = tool_result_envelope("hello world", path);
    let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
    assert_eq!(parsed["result"], "hello world");
    assert_eq!(parsed["cwd"], "/data/worktrees/p1/s1");
    // No extra top-level keys — schema discipline matters
    // because the LLM is reading this.
    assert_eq!(
        parsed.as_object().unwrap().len(),
        2,
        "envelope must have exactly 2 keys: result, cwd"
    );
}

/// Step 4 follow-up: empty / unicode / special-char content
/// all round-trip cleanly through the envelope. (Sanity — the
/// envelope is built with `serde_json::json!` which handles
/// escaping, but a hand-written string would not.)
#[test]
fn tool_result_envelope_handles_special_chars() {
    let path = std::path::Path::new("/data/wt");
    // Newline, quote, and backslash in the content.
    let content = "line 1\nline 2 with \"quote\" and \\ slash";
    let env = tool_result_envelope(content, path);
    let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
    assert_eq!(parsed["result"], content);
    assert_eq!(parsed["cwd"], "/data/wt");
}

// ---------------------------------------------------------------------------
// flush_pending_thinking
// ---------------------------------------------------------------------------

/// A pending thinking block with both text and signature is
/// moved into the finalized vec on flush.
#[test]
fn flush_pending_thinking_moves_into_finalized() {
    let mut pending = Some(PendingThinking {
        text: "reasoning text".to_string(),
        signature: "sig-blob".to_string(),
    });
    let mut finalized: Vec<(String, String)> = Vec::new();
    flush_pending_thinking(&mut pending, &mut finalized);
    assert!(pending.is_none(), "pending should be cleared after flush");
    assert_eq!(finalized.len(), 1);
    assert_eq!(finalized[0].0, "reasoning text");
    assert_eq!(finalized[0].1, "sig-blob");
}

/// A no-op when pending is None (already flushed).
#[test]
fn flush_pending_thinking_noop_when_already_flushed() {
    let mut pending: Option<PendingThinking> = None;
    let mut finalized: Vec<(String, String)> = Vec::new();
    flush_pending_thinking(&mut pending, &mut finalized);
    assert!(pending.is_none());
    assert!(finalized.is_empty());
}

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
    assert_eq!(blocks.len(), 1, "one tool_call must produce one ToolResult block");
    match &blocks[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "toolu_abc", "tool_use_id must match");
            assert!(is_error, "synthetic tool_result must be flagged is_error=true");
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
        ("id_1".to_string(), "read_file".to_string(), serde_json::json!({})),
        ("id_2".to_string(), "edit_file".to_string(), serde_json::json!({})),
        ("id_3".to_string(), "shell".to_string(), serde_json::json!({})),
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
        block.get("content").and_then(|s| s.as_str()).unwrap().contains("shell"),
        "wire content must mention the tool name"
    );
}

// (A4 follow-up hotfix tests removed in the revert — see
// `06-10-a4-token-per-session-chatinput-hint` task PRD for
// the rationale. The per-turn `db::add_token_usage` call in
// `agent::chat::chat` is unchanged; the cumulative tracker
// + 3 exit-path emits reverted to `usage: None`.)