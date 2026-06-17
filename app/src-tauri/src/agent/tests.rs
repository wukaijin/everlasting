//! Tests previously inlined in `lib.rs` (post-PR1 of the audit
//! task). All tests exercise behavior that lives in
//! [`crate::agent`] modules; the test bodies are unchanged
//! except for the absolute-type paths (e.g. `db::SessionRow` →
//! `crate::db::SessionRow`).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, oneshot};
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
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Register a fake request for session "s1". No exit receiver is
    // registered (simulates a chat that predates the RULE-E-005
    // signal wiring, or one whose spawn closure already drained it).
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
    let exit_rx =
        cancel_inflight_for_session(&cancellations, &session_active_request, &inflight_exits, "s1")
            .await;
    assert!(
        token.is_cancelled(),
        "matching request's token should be cancelled"
    );
    // No receiver was registered → helper returns None (caller's
    // `await_inflight_exit` becomes a no-op, but the token cancel
    // still took effect).
    assert!(
        exit_rx.is_none(),
        "no exit signal should be returned when none was registered"
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
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Nothing registered.
    let exit_rx = cancel_inflight_for_session(
        &cancellations,
        &session_active_request,
        &inflight_exits,
        "s-missing",
    )
    .await;
    // No panic, no state change. Returns None (no in-flight request).
    assert!(
        cancellations.lock().await.is_empty(),
        "nothing to cancel"
    );
    assert!(exit_rx.is_none(), "no exit signal for a missing session");
    assert!(session_active_request.lock().await.is_empty());
    assert!(inflight_exits.lock().await.is_empty());
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
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // session_active_request has the entry, but the
    // cancellations map doesn't (the request already
    // finished and the CancellationGuard cleaned up).
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("s1".to_string(), "rid-gone".to_string());
    }
    let exit_rx =
        cancel_inflight_for_session(&cancellations, &session_active_request, &inflight_exits, "s1")
            .await;
    // No panic; the function is best-effort. No token found → no
    // exit receiver either (returns None).
    assert!(exit_rx.is_none());
}

/// RULE-E-005 (2026-06-15): `cancel_inflight_for_session` returns a
/// "agent loop exited" signal (`oneshot::Receiver`) that the
/// destructive commands await before deleting. This test proves the
/// signal's core contract: it stays **pending** while the producer
/// (the `chat` spawn closure standing in here) is silent, and
/// **resolves** only once the producer fires (i.e. `run_chat_loop`
/// returned). Without this, `delete_worktree`'s `await` would be a
/// no-op and the race the fix targets would still be open.
///
/// Mirrors `select_loop_breaks_on_cancellation`'s spawn + flag +
/// sleep pattern (a receiver is single-consumer, so we can't poll it
/// twice — we race it in a task and assert on a shared flag).
#[tokio::test(flavor = "multi_thread")]
async fn cancel_inflight_returns_exit_signal_resolving_on_completion() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Stand up a full in-flight registration (token + exit receiver +
    // session→rid), matching what `chat` does on spawn.
    let token = CancellationToken::new();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    {
        let mut m = cancellations.lock().await;
        m.insert("rid-1".to_string(), token.clone());
    }
    {
        let mut m = inflight_exits.lock().await;
        m.insert("rid-1".to_string(), done_rx);
    }
    {
        let mut m = session_active_request.lock().await;
        m.insert("s1".to_string(), "rid-1".to_string());
    }

    // Cancel + take the exit signal.
    let exit_rx =
        cancel_inflight_for_session(&cancellations, &session_active_request, &inflight_exits, "s1")
            .await;
    assert!(token.is_cancelled(), "token should be cancelled");
    let exit_rx = exit_rx.expect("an in-flight request yields an exit signal");
    // The receiver was taken out of the map (single-consumer).
    assert!(
        inflight_exits.lock().await.is_empty(),
        "exit receiver should be drained from the map"
    );

    // Race the receiver in a task so we can assert on a shared flag
    // (a `oneshot::Receiver` can only be awaited once).
    let resolved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let resolved_c = resolved.clone();
    let handle = tokio::spawn(async move {
        let _ = exit_rx.await;
        resolved_c.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // The agent loop has NOT exited yet (producer silent) → the
    // awaiting task must stay pending.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !resolved.load(std::sync::atomic::Ordering::SeqCst),
        "exit signal must NOT resolve before the agent loop exits"
    );

    // Simulate the `chat` spawn closure finishing `run_chat_loop`.
    let _ = done_tx.send(());

    // Now the signal resolves promptly.
    tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("await task should complete after the producer signals")
        .expect("task should not panic");
    assert!(
        resolved.load(std::sync::atomic::Ordering::SeqCst),
        "exit signal should resolve once the producer signals completion"
    );
}
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
// scripted events.
//
// The integration tests below run the full `run_chat_loop`
// (see `agent::chat_loop`) with a `MockProvider` (scripted
// `Provider` impl) and a `MockEmitter` (records events into a
// Vec for assertion). The 9 tests cover the P1 debt items
// listed in `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`
// §2.1 + §3.5.
//
// Design notes:
// - The test AppState is the minimum needed: a DB pool (in-
//   memory SQLite with migrations), a MemoryCache (empty),
//   the cancellation + session_active maps, a read guard, and
//   the permission store. The catalog is bypassed — tests
//   pass a pre-built `Arc<MockProvider>` directly to
//   `run_chat_loop`.
// - Each test creates a fresh project + session in the test
//   DB so cross-test state is impossible.
// - The default model's `context_window` is set large enough
//   (200_000) that C3 never fires unintentionally; the C3
//   tests pass a smaller window to force compaction.

use std::sync::atomic::Ordering;
use std::sync::Mutex as StdMutex;

use futures_util::StreamExt;
use sqlx::SqlitePool;
use tokio::sync::Mutex as AsyncMutex;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::permissions::new_permission_store;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, TokenUsage};
use crate::llm::Provider;
use crate::memory::MemoryCache;
use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};
use crate::tools::read_guard::ReadGuard;

/// Test ChatEventSink that records every emitted event into
/// a `Vec` for assertion. Mirrors the production
/// `AppHandleSink` (which forwards to `tauri::AppHandle::emit`)
/// but is in-process and inspectable.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) for the
/// internal storage: the sink is only ever called from the agent
/// loop's emit sites, which never hold the lock across an `.await`.
/// `std::sync::Mutex` lets the test code call `.lock().unwrap()`
/// synchronously without pulling in `.await` plumbing.
#[derive(Default)]
struct MockEmitter {
    chat_events: Arc<StdMutex<Vec<ChatEventPayload>>>,
    tool_calls: Arc<StdMutex<Vec<ToolCallPayload>>>,
    tool_results: Arc<StdMutex<Vec<ToolResultPayload>>>,
    permission_asks: Arc<StdMutex<Vec<crate::agent::permissions::PermissionAskPayload>>>,
}

impl MockEmitter {
    fn new() -> Self {
        Self::default()
    }

    /// Snapshot all chat-event payloads recorded so far.
    fn chat_events(&self) -> Vec<ChatEventPayload> {
        self.chat_events.lock().unwrap().clone()
    }

    /// Count of `Done` events with `stop_reason = Some("cancelled")`
    /// — the contract the cancel path uses to signal end-of-stream.
    fn cancel_done_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                matches!(&p.event, ChatEvent::Done { stop_reason, .. }
                    if stop_reason.as_deref() == Some("cancelled"))
            })
            .count()
    }

    /// Count of `Done` events with `stop_reason = Some("max_turns")`.
    fn max_turns_done_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                matches!(&p.event, ChatEvent::Done { stop_reason, .. }
                    if stop_reason.as_deref() == Some("max_turns"))
            })
            .count()
    }

    /// Count of `Error` chat-events.
    fn error_event_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| matches!(&p.event, ChatEvent::Error { .. }))
            .count()
    }

    /// Number of `tool:call` events recorded.
    fn tool_call_count(&self) -> usize {
        self.tool_calls.lock().unwrap().len()
    }

    /// Number of `tool:result` events recorded.
    fn tool_result_count(&self) -> usize {
        self.tool_results.lock().unwrap().len()
    }
}

impl ChatEventSink for MockEmitter {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        self.chat_events.lock().unwrap().push(payload.clone());
    }
    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        self.tool_calls.lock().unwrap().push(payload.clone());
    }
    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        self.tool_results.lock().unwrap().push(payload.clone());
    }
    fn emit_permission_ask(
        &self,
        payload: crate::agent::permissions::PermissionAskPayload,
    ) {
        self.permission_asks.lock().unwrap().push(payload);
    }
}

async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

/// Build a fresh AppState-equivalent for a test: in-memory DB +
/// empty cache + cancel maps. The test passes a pre-built
/// `Arc<MockProvider>` to `run_chat_loop` directly, bypassing
/// the catalog.
///
/// `project_id` / `project_path` are kept on the harness for
/// readability (callers can see what session they're talking to
/// via the named fields) even though no test reads them back —
/// the values are also stored in the DB row the harness inserts.
///
/// **Lifetime invariant**: the harness owns the `tempfile::TempDir`
/// guard (`_tempdir`) for the entire test. Without it, `make_harness`
/// returning would drop the guard and delete the on-disk directory
/// before `run_chat_loop`'s pre-flight `assert_within_root` could
/// `canonicalize()` it — that path (chat_loop.rs:173) returns Err
/// on a missing directory, the agent loop short-circuits with an
/// Error emit, `provider.send` is never called, and `call_count`
/// stays 0. The 6 FAILED + 1 hung test symptom in the first run
/// was exactly this regression. The leading underscore on
/// `_tempdir` is intentional — the value is never read, only
/// kept alive by being a struct field.
#[allow(dead_code)]
struct TestHarness {
    db: SqlitePool,
    project_id: String,
    project_path: std::path::PathBuf,
    session_id: String,
    cancellations: Arc<AsyncMutex<HashMap<String, CancellationToken>>>,
    session_active_request: Arc<AsyncMutex<HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    /// TempDir guard — kept alive for the duration of the test so
    /// the project_path directory remains on disk while the agent
    /// loop's pre-flight canonicalizes it. See struct docstring.
    _tempdir: tempfile::TempDir,
}

async fn make_harness() -> TestHarness {
    let pool = test_pool().await;
    // Create a project in the default "Legacy" bucket (the
    // migration's seed). We use a fresh path in the tempdir
    // so the worktree assertion (assert_within_root) succeeds
    // even though the path doesn't exist on disk for the
    // text-only / tool-execution-skipping tests.
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().to_path_buf();
    db::create_project(
        &pool,
        "test-project",
        project_path.to_str().unwrap(),
        false,
        None,
    )
    .await
    .expect("create_project");
    // The project id is generated server-side; re-fetch.
    let projects = db::list_projects(&pool, false).await.expect("list_projects");
    let project_id = projects
        .iter()
        .find(|p| p.path == project_path.to_string_lossy().to_string())
        .map(|p| p.id.clone())
        .expect("project should be present after create");

    let session_id = uuid::Uuid::new_v4().to_string();
    db::create_session(
        &pool,
        &session_id,
        &project_id,
        project_path.to_str().unwrap(),
        "mock-model",
        None,
    )
    .await
    .expect("create_session");

    TestHarness {
        db: pool,
        project_id,
        project_path,
        session_id,
        cancellations: Arc::new(AsyncMutex::new(HashMap::new())),
        session_active_request: Arc::new(AsyncMutex::new(HashMap::new())),
        read_guard: ReadGuard::new(),
        memory_cache: MemoryCache::arc(),
        permission_asks: new_permission_store(),
        // Move the TempDir guard INTO the harness so it lives as
        // long as the harness (i.e. the whole test). Without this
        // move, `dir` drops at the end of `make_harness` and the
        // temp directory is deleted before `run_chat_loop` can
        // canonicalize it.
        _tempdir: dir,
    }
}

fn test_messages() -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("hello".to_string()),
    }]
}

// ---------------------------------------------------------------------------
// 1) Basic text-only response
// ---------------------------------------------------------------------------

/// The simplest turn-orchestration invariant: a single-turn
/// text-only response results in exactly 1 `send` call and
/// one terminal `Done { stop_reason: Some("end_turn") }`
/// event. Covers the regression where pre-fix the agent loop
/// called `send` twice for a single-turn response (the
/// "thinking_ms only on last turn" bug class — same family
/// of off-by-one).
#[tokio::test]
async fn agent_loop_basic_text_only_completes() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "hi".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-basic".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    assert_eq!(mock.call_count(), 1, "expected exactly 1 send call");
    let done = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect::<Vec<_>>();
    // `filter_map` flattens one layer of `Option`, so `done` is
    // `Vec<String>` — the extracted `stop_reason` values that were
    // `Some(...)`. The `Some("end_turn")` case means we see
    // exactly one entry here.
    assert_eq!(done, vec!["end_turn".to_string()]);
}

// ---------------------------------------------------------------------------
// 2) Tool use → tool result loop
// ---------------------------------------------------------------------------

/// Turn 1: model emits `tool_use` (the agent loop's `stop_reason`
/// becomes "tool_use"). The agent loop MUST execute the tool
/// (default-allow for read tools) and call `send` a SECOND time.
/// Turn 2: model emits a final text response. The loop MUST
/// terminate with `Done { stop_reason: Some("end_turn") }`.
///
/// This is the "tool_use triggers another turn" invariant — if
/// the agent loop's tool execution path is broken (e.g. the
/// `should_continue` branch fails to re-enter the outer loop),
/// this test fails with `mock.call_count() == 1`.
#[tokio::test]
async fn agent_loop_tool_use_triggers_tool_result_turn() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. The MockProvider script
        // auto-exhausts on this call index.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: text response (after the agent loop
        // built the tool_result message).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-tool".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "tool_use must trigger exactly one more turn (2 sends total)"
    );
    assert_eq!(emitter.tool_call_count(), 1);
    // list_dir is a read-only tool that goes through Tier 5
    // default-allow; the agent loop emits one `tool:result`
    // (success path, is_error=false) before re-entering the
    // outer loop.
    assert_eq!(emitter.tool_result_count(), 1);
}

// ---------------------------------------------------------------------------
// 3) Cancel in turn 2 kills the loop
// ---------------------------------------------------------------------------

/// Spawn `run_chat_loop` and cancel its token after turn 1 has
/// cleanly completed and turn 2's `send` has been observed. The
/// agent loop MUST:
/// - emit `Done { stop_reason: Some("cancelled") }` (exactly
///   one, not zero, not two)
/// - call `send` exactly twice (turn 1 tool_use + the cancelled
///   turn 2)
/// - NOT emit `tool:result` for a turn 2 tool (turn 2 is a
///   HangingThenCancel so no tool_use arrives)
///
/// The semantics here match PRD R3 "2 turn cancel":
/// - Turn 1 emits `tool_use` (`list_dir` with `path: "."`). The
///   agent loop runs the read tool through Tier 5 default-allow,
///   persists the tool_result, and re-enters the outer loop for
///   turn 2. Critically, `run_chat_loop` does NOT exit on
///   `tool_use` (only `end_turn` / non-`tool_use` exits) — see
///   `chat_loop.rs`'s `should_continue` branch.
/// - Turn 2 is `HangingThenCancel`: the stream is forever
///   pending. The cancel side-channel polls `call_count` and
///   fires the cancel token once `call_count >= 2` (turn 2's
///   `send` has been called). The agent loop's `select!` cancel
///   arm (`biased;` first) wins over the pending stream and
///   emits exactly one `Done("cancelled")`.
///
/// We gate the cancel on `call_count >= 2` (not 1) so that turn
/// 1 completes normally — earlier versions gated on 1, which
/// races with the tool-execution path and can flip the
/// `cancelled` flag mid-tool.
#[tokio::test]
async fn agent_loop_cancel_in_turn_2_kills_loop() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. `list_dir` is a read tool → Tier 5
        // default-allow (no permission ask), the agent loop
        // executes it, persists the tool_result, and re-enters
        // the outer loop for turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: this script entry is consumed (call_count → 2)
        // but the agent loop is cancelled mid-stream (the
        // `HangingThenCancel` arm keeps the stream pending
        // until the cancel arm wins the `select!`).
        MockResponse::HangingThenCancel,
    ]));
    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Poll until call_count >= 2 (turn 2's send has been
        // observed by the agent loop), then cancel. Gating on 2
        // lets turn 1's tool_use + tool execution + tool_result
        // persist complete cleanly before the cancel fires —
        // the cancel races only against turn 2's pending stream.
        loop {
            if call_handle.load(Ordering::SeqCst) >= 2 {
                cancel_for_task.cancel();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        cancel_token,
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    assert_eq!(
        mock.call_count(),
        2,
        "agent loop should call send twice (turn 1 tool_use + the cancelled turn 2)"
    );
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "exactly one Done(cancelled) event expected"
    );
    assert_eq!(emitter.max_turns_done_count(), 0);
}

// ---------------------------------------------------------------------------
// 4) MAX_TURNS fallback
// ---------------------------------------------------------------------------

/// Script the mock to always emit `tool_use` (no end_turn),
/// forcing the agent loop to hit MAX_TURNS. The agent loop
/// MUST emit `Done { stop_reason: Some("max_turns") }` and
/// must call `send` exactly MAX_TURNS times.
///
/// This covers the "infinite tool loop" pathological case
/// (C3 + MAX_TURNS safety net, see context.rs for the C3
/// half; this test is the MAX_TURNS half).
#[tokio::test]
async fn agent_loop_max_turns_emits_done_marker() {
    use crate::agent::MAX_TURNS;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());

    // Build a script with MAX_TURNS tool_use responses.
    // The agent loop will keep emitting tool_use, executing
    // the tool (Tier 5 default-allow for list_dir), and
    // calling send again. After MAX_TURNS iterations, the
    // outer loop bails.
    let mut script = Vec::with_capacity(MAX_TURNS);
    for i in 0..MAX_TURNS {
        script.push(MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: format!("toolu_max_{}", i),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]));
    }
    let mock = Arc::new(MockProvider::new(script));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-maxturns".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    assert_eq!(
        mock.call_count(),
        MAX_TURNS,
        "agent loop should call send MAX_TURNS times"
    );
    assert_eq!(
        emitter.max_turns_done_count(),
        1,
        "exactly one Done(max_turns) event expected"
    );
}

// ---------------------------------------------------------------------------
// 5) MockProvider script exhaustion
// ---------------------------------------------------------------------------

/// When the agent loop asks for more turns than the test
/// scripted, MockProvider surfaces a typed
/// `LlmError::InvalidRequest { "exhausted" }` and the agent
/// loop bails with `had_error = true`. The test asserts:
/// - the typed error message made it to the agent loop
///   (proves the exhaustion contract is observable)
/// - exactly one `send` was attempted (the second was never
///   reached because the first hit the error path)
///
/// This guards against silent script-overflow regressions.
#[tokio::test]
async fn agent_loop_mock_provider_exhaustion_surfaces_error() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Script has 0 entries — the very first send hits
    // exhaustion. The agent loop's `if had_error` branch
    // returns before persisting any assistant turn.
    let mock = Arc::new(MockProvider::new(vec![]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-exhaust".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    // The agent loop's error path emits one `ChatEvent::Error`
    // and returns; we expect at least one error event in the
    // recorded events.
    assert_eq!(emitter.error_event_count(), 1, "one error event");
    assert!(emitter
        .chat_events()
        .iter()
        .any(|p| matches!(&p.event,
            ChatEvent::Error { message, .. } if message.contains("exhausted"))));
    assert_eq!(mock.call_count(), 1);
}

// ---------------------------------------------------------------------------
// 6) C3 compaction preserves the agent loop (no panic / no error)
// ---------------------------------------------------------------------------

/// Force C3 compaction by setting a tiny context_window (10
/// tokens). The agent loop MUST:
/// - NOT panic (C3 returns whatever it can trim; with an
///   empty messages vec after compaction, the turn body
///   short-circuits and the model just sees the system
///   prompt + nothing)
/// - emit `Done` (some stop_reason) — the loop must
///   terminate, not hang
///
/// This is the safety-net test for C3 (the bigger
/// pair-atomicity invariant — RULE-A-001 — is covered by
/// the upstream `agent::context::tests`; this integration
/// test just asserts "the agent loop survives a C3 run").
#[tokio::test]
async fn agent_loop_c3_compaction_does_not_panic() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "after-c3".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    // context_window = 10 forces aggressive trimming. The
    // estimator (tiktoken cl100k_base) on a tiny
    // `["hello"]` message is already > 0 tokens; the
    // agent loop's pre-compact check (80% of window)
    // triggers and trims. With a 10-token window, the
    // agent loop may end up with 0 middle messages to
    // drop; the B5 synthetic user/assistant head pair
    // + the current user message are protected, so the
    // loop survives.
    run_chat_loop(
        vec![],
        mock.clone(),
        10,
        "rid-c3".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    // The loop should have completed (one Done event) and
    // not emitted any error events.
    let events = emitter.chat_events();
    assert!(
        events
            .iter()
            .any(|p| matches!(&p.event, ChatEvent::Done { .. })),
        "agent loop must terminate with a Done event after C3 compaction"
    );
    assert_eq!(
        emitter.error_event_count(),
        0,
        "no error events expected (C3 is best-effort, not fatal)"
    );
}

// ---------------------------------------------------------------------------
// 7) Provider protocol is `Mock`
// ---------------------------------------------------------------------------

/// The `MockProvider::protocol()` returns
/// `ProviderProtocol::Mock`. This is the catalog dispatch
/// contract — the chat command's pre-flight could reject
/// unknown protocols, so we test that the protocol wire
/// format is well-formed end-to-end.
#[test]
fn mock_provider_reports_mock_protocol() {
    let mock = MockProvider::new(vec![]);
    assert_eq!(mock.protocol(), db::ProviderProtocol::Mock);
    let caps = mock.capabilities();
    assert!(caps.supports_system_prompt);
    assert!(caps.supports_tools);
    assert!(caps.supports_streaming);
}

// ---------------------------------------------------------------------------
// 8) MockProvider call count tracking
// ---------------------------------------------------------------------------

/// `call_count()` is the primary assertion surface for "did
/// the agent loop dispatch the expected number of turns?".
/// This unit test guards the counter itself (the agent-loop
/// integration tests above rely on it being accurate).
#[tokio::test]
async fn mock_provider_call_count_tracks_send_calls() {
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![Ok(ChatEvent::Start), Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        })]),
        MockResponse::Events(vec![Ok(ChatEvent::Start), Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        })]),
        MockResponse::Events(vec![Ok(ChatEvent::Start), Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        })]),
    ]));
    assert_eq!(mock.call_count(), 0);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 1);
    let _ = mock.send(None, vec![], vec![]).collect::<Vec<_>>().await.len();
    assert_eq!(mock.call_count(), 2);
    let _ = mock.send(None, vec![], vec![]).collect::<Vec<_>>().await.len();
    assert_eq!(mock.call_count(), 3);
}

// ---------------------------------------------------------------------------
// 9) Error path emits ChatEvent::Error
// ---------------------------------------------------------------------------

/// The `ErrThenEnd` script entry must surface to the
/// frontend as a `ChatEvent::Error` event, NOT a silent
/// loop. This is the canonical error-path contract.
#[tokio::test]
async fn agent_loop_error_path_emits_chat_event_error() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::ErrThenEnd(
        LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
        },
    )]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(error_events.len(), 1, "one error event expected");
    let (msg, _cat) = &error_events[0];
    assert!(msg.contains("服务") || msg.contains("服务器"));
    // Server category for HTTP 5xx.
    assert_eq!(error_events[0].1, crate::llm::LlmErrorCategory::Server);
}

// ---------------------------------------------------------------------------
// 10) C3 degradation — `StillOver` aborts the turn with an Error event
//     (RULE-A-002, 2026-06-14)
// ---------------------------------------------------------------------------

/// When `compact_messages` runs out of safe droppable candidates but
/// the budget is still over the target, the agent loop MUST:
///
/// 1. Emit exactly one `ChatEvent::Error` with
///    `LlmErrorCategory::InvalidRequest`.
/// 2. NOT call `provider.send` (the over-budget request would 400
///    on `prompt is too long`).
/// 3. NOT emit a terminal `Done` event (the chat is aborted, not
///    completed). The frontend treats `Error` as terminal.
///
/// This is the integration-test guard for RULE-A-002. The unit
/// level is covered by `agent::context::tests::compact_emits_still_over_degradation`;
/// this test verifies the agent loop body (in `chat_loop.rs`)
/// translates the signal into the Error event correctly. It MUST
/// mirror the production `chat.rs` C3 block (see module docstring
/// "Drift hazard" — the two implementations share the same wire
/// contract here).
#[tokio::test]
async fn agent_loop_c3_still_over_emits_error_and_skips_provider() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // The provider script has ONE turn's worth of events. If the
    // agent loop's C3 guard works, `send` is never called and the
    // script is left unconsumed. If the guard is broken, the
    // provider WILL be called and we'll see call_count == 1.
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "should never reach".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    // Construct messages that force `DegradationKind::StillOver`:
    // head[2 small] + middle[1 small droppable] + tail[1 HUGE].
    // context_window = 1000 → trigger = 800, target = 500.
    // After dropping the middle, head(2 tiny) + tail(1 huge > 500)
    // is still over target → StillOver.
    //
    // big_pad(8_000) ≈ 8KB ≈ ~2000 tokens (well over target 500).
    let huge = {
        // Mirror the helper used by context.rs tests — repeated
        // ASCII filler that cl100k_base encodes at ~4 chars/token.
        "the quick brown fox jumps over the lazy dog. "
            .repeat(8_000 / 45 + 1)
            .chars()
            .take(8_000)
            .collect::<String>()
    };
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("tiny head 1".into()),
        },
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text("tiny head 2".into()),
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text("droppable middle".into()),
        },
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(huge),
        },
    ];

    run_chat_loop(
        vec![],
        mock.clone(),
        // Force tiny context_window so compaction triggers and
        // StillOver fires.
        1000,
        "rid-c3-still-over".into(),
        h.session_id.clone(),
        messages,
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    // (1) `provider.send` was NEVER called — the C3 guard
    //     short-circuited before dispatch.
    assert_eq!(
        mock.call_count(),
        0,
        "provider.send MUST NOT be called when C3 degradation is StillOver"
    );

    // (2) Exactly one Error event with the InvalidRequest category.
    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(
        error_events.len(),
        1,
        "exactly one Error event expected on StillOver (got {})",
        error_events.len()
    );
    let (err_msg, err_cat) = &error_events[0];
    assert!(
        err_msg.contains("Context window exceeded after compaction"),
        "Error message should describe the over-budget state, got: {}",
        err_msg
    );
    assert_eq!(
        *err_cat,
        crate::llm::LlmErrorCategory::InvalidRequest,
        "category should be InvalidRequest (mirrors prompt-too-long 400)"
    );

    // (3) No terminal Done event — the chat is aborted via Error,
    //     not completed via Done.
    let done_count = emitter
        .chat_events()
        .iter()
        .filter(|p| matches!(&p.event, ChatEvent::Done { .. }))
        .count();
    assert_eq!(
        done_count, 0,
        "no Done event expected — the turn was aborted, not completed"
    );
}

// ---------------------------------------------------------------------------
// 10) RULE-A-003: persist_turn failure surfaces a typed Error
// ---------------------------------------------------------------------------

/// RULE-A-003 (2026-06-15): when `persist_turn` fails (disk full /
/// DB-lock contention) on a NORMAL persist site (initial user
/// message / assistant turn / tool_result turn), the agent loop
/// must NOT stay silent — it emits a `ChatEvent::Error { Server }`
/// and aborts. Previously the failure was `tracing::error!`-only,
/// so the message was rendered to the user but never reached the
/// DB; the next session reload was blank, and the in-memory seq
/// drifted out of sync with the DB. The cancel-path persist sites
/// intentionally stay log-only (no Error) to avoid emitting two
/// terminal events — that's a code-review invariant, not a
/// runtime path this test exercises.
///
/// We force the failure with a `BEFORE INSERT ON messages` trigger
/// that always ABORTs. This blocks only INSERT (what `persist_turn`
/// does); SELECT (what `load_session` / `get_project` do) is
/// unaffected, so the loop reaches the persist site cleanly. The
/// initial user-message persist runs before the `for turn` loop, so
/// `provider.send` is never called (`call_count == 0`).
#[tokio::test]
async fn agent_loop_persist_failure_emits_error() {
    let h = make_harness().await;
    // Poison INSERTs into `messages`: persist_turn's INSERT will
    // RAISE, but load_session's SELECT on `messages` still works.
    sqlx::query(
        r#"CREATE TRIGGER messages_no_insert BEFORE INSERT ON messages
           BEGIN
               SELECT RAISE(ABORT, 'simulated persist failure');
           END"#,
    )
    .execute(&h.db)
    .await
    .expect("install fail-insert trigger");

    let emitter = Arc::new(MockEmitter::new());
    // The provider script is never consumed (call_count stays 0).
    // Provided as a sentinel so a broken fix that skipped the
    // abort would surface as call_count == 1.
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "should never reach".into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-persist-fail".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
    )
    .await;

    // (1) provider.send was never called — the initial user-message
    //     persist (before the `for turn` loop) failed and aborted.
    assert_eq!(
        mock.call_count(),
        0,
        "persist failure must abort before provider.send is called"
    );

    // (2) Exactly one Error event, category Server, persist-failure
    //     copy. Mirrors the StillOver test's assertion shape.
    let error_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Error { message, category } => Some((message, category)),
            _ => None,
        })
        .collect();
    assert_eq!(
        error_events.len(),
        1,
        "exactly one Error event expected on persist failure (got {})",
        error_events.len()
    );
    let (err_msg, err_cat) = &error_events[0];
    assert!(
        err_msg.contains("保存对话记录失败"),
        "Error message should be the persist-failure copy, got: {}",
        err_msg
    );
    assert_eq!(
        *err_cat,
        crate::llm::LlmErrorCategory::Server,
        "category should be Server (system-side, not a bad request)"
    );
}

// ---------------------------------------------------------------------------
// 11) RULE-A-004: a cancelled tool is NOT recorded as tool_executed
// ---------------------------------------------------------------------------

/// RULE-A-004 (2026-06-15): `record_tool_executed_audit` must run
/// AFTER the `token.is_cancelled()` check. A tool whose execution
/// was interrupted by a cancel must NOT get a `tool_executed` audit
/// row — recording it would lie to the audit log (the user hit
/// Stop; the tool did not complete from their intent).
///
/// Turn 1 emits `tool_use` (`list_dir` — a read tool that does NOT
/// consult the cancel token, so execute_tool runs to completion
/// regardless). A side task cancels the token once `call_count >= 1`
/// (turn 1's `send` has been called). The cancel task `yield_now`s
/// (no sleep) so it re-checks at every agent-loop await point and
/// cancels as early as possible. Two landing spots, both correct:
/// - mid-stream → the `select!`'s biased cancel arm wins, the tool
///   never executes → no audit row (trivially correct).
/// - at/after execute_tool returns → `token.is_cancelled()` is true
///   at the audit check → audit skipped (the RULE-A-004 fix).
/// Either way `session_audit_events` has zero `tool_executed` rows.
///
/// Contrast `agent_loop_tool_use_triggers_tool_result_turn` (no
/// cancel): the same `list_dir` DOES write an audit row there — so
/// this is a real regression guard, not a tautology.
#[tokio::test]
async fn agent_loop_cancel_skips_audit_for_cancelled_tool() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use. `list_dir` is a read tool → Tier 5
        // default-allow, and it does NOT consult the cancel token,
        // so execute_tool runs to completion even after cancel.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_1".into(),
                name: "list_dir".into(),
                input: serde_json::json!({"path": "."}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2 sentinel — only consumed if the loop re-enters
        // (it shouldn't; cancel aborts before turn 2).
        MockResponse::HangingThenCancel,
    ]));
    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // yield_now (not sleep) so the cancel task re-runs at every
        // agent-loop await point and cancels as soon as turn 1's
        // send has been observed.
        loop {
            if call_handle.load(Ordering::SeqCst) >= 1 {
                cancel_for_task.cancel();
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-audit-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.permission_asks,
        cancel_token,
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    // No tool_executed audit row for this session — the cancelled
    // tool must not leave a "this tool ran" record behind it.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert_eq!(
        audit_count, 0,
        "a cancelled tool must NOT be recorded as tool_executed (RULE-A-004)"
    );
}
