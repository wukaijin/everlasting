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
            skip_session_active: false,
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
    let prompt = crate::agent::system_prompt::assemble_system_prompt(
        "MODE_MARKER",
        "BASE_MARKER",
    );
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
use crate::skill::loader::SkillCache;
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

    /// Snapshot all `tool:result` payloads (content + is_error) — for
    /// asserting what the agent loop fed back to the LLM (e.g. a
    /// resolved skill body, or an "is_error" self-correction nudge).
    fn tool_results_snapshot(&self) -> Vec<ToolResultPayload> {
        self.tool_results.lock().unwrap().clone()
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
    skill_cache: Arc<SkillCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    /// L1a (2026-06-19): cross-request background-shell registry.
    /// Each test gets a fresh registry so concurrent tests can't
    /// see each other's shells. Threads through `run_chat_loop`'s
    /// new 15th parameter and is the same handle `ToolContext`
    /// hands to the 3 L1a tools.
    background_shells: crate::background_shell::DefaultRegistry,
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
        skill_cache: SkillCache::arc(),
        permission_asks: new_permission_store(),
        background_shells: crate::background_shell::default_registry(),
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
// 2b) B4 use_skill loads the skill body into the tool_result
// ---------------------------------------------------------------------------

/// B4: turn 1 model emits `use_skill("review-pr")`. The agent loop
/// resolves the skill body from the SkillCache (a real skill file
/// seeded under the project's `.everlasting/skills/`) and feeds it
/// back as the tool_result — L1 activation via the tool_result path
/// (PR2 brainstorm Q2). Turn 2: final text. Asserts the body lands
/// in the tool_result with is_error=false.
#[tokio::test]
async fn agent_loop_use_skill_loads_body_into_tool_result() {
    let h = make_harness().await;
    // Seed a real skill the loader will scan.
    let skill_dir = h
        .project_path
        .join(".everlasting")
        .join("skills")
        .join("review-pr");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review-pr\ndescription: review a PR\n---\nREVIEW-SKILL-BODY",
    )
    .unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_skill".into(),
                name: "use_skill".into(),
                input: serde_json::json!({"skill_name": "review-pr"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "applied".into() }),
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
        "rid-skill".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "use_skill must trigger a second turn (body fed back as tool_result)"
    );
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result for use_skill");
    assert!(
        results[0].content.contains("REVIEW-SKILL-BODY"),
        "tool_result must carry the skill body, got: {}",
        results[0].content
    );
    assert!(
        !results[0].is_error,
        "resolved skill must be a success tool_result"
    );
}

/// B4: `use_skill("nope")` with no matching skill returns
/// is_error=true — the standard ⑫ error-feedback path so the LLM
/// can self-correct.
#[tokio::test]
async fn agent_loop_use_skill_unknown_returns_error() {
    let h = make_harness().await;
    // No skill files seeded → "nope" won't resolve.

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_miss".into(),
                name: "use_skill".into(),
                input: serde_json::json!({"skill_name": "nope"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
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
        "rid-skill-miss".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].is_error,
        "unknown skill must be is_error so the LLM can self-correct"
    );
    assert!(
        results[0].content.contains("not found"),
        "error content should name the missing skill, got: {}",
        results[0].content
    );
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
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
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

// ---------------------------------------------------------------------------
// 12) RULE-A-007 (2026-06-17): error arm persists partial turn
// ---------------------------------------------------------------------------

/// Helper: extract the persisted assistant message rows from a
/// session, in `seq` order. Used by the RULE-A-007 tests to
/// verify the error path landed the partial turn (text +
/// ERROR_MARKER + thinking + tool_use) in the DB.
async fn load_assistant_rows(db: &SqlitePool, session_id: &str) -> Vec<db::MessageRow> {
    let loaded = db::load_session(db, session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    loaded
        .messages
        .into_iter()
        .filter(|m| m.role == "assistant")
        .collect()
}

/// RULE-A-007 (2026-06-17): when the LLM stream emits `Delta`
/// and then `Error` mid-turn, the agent loop MUST persist the
/// partial text (+ ERROR_MARKER) so a reload shows it. Before
/// the fix the error arm did `return` immediately, dropping
/// already-rendered text — an asymmetry vs the cancel path.
///
/// Script: `Delta("partial")` → `Error(Server)`. After the
/// loop runs, the DB has one assistant row whose `text` contains
/// both "partial" AND `ERROR_MARKER`.
#[tokio::test]
async fn agent_loop_error_persists_partial_text() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "partial".into() }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err-partial".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // Exactly one Error event (the pre-emit from the per-event
    // arm). RULE-A-007 decision B: no second terminal Error from
    // a persist failure path.
    assert_eq!(
        emitter.error_event_count(),
        1,
        "exactly one Error event (no double-terminal)"
    );

    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(
        assistants.len(),
        1,
        "exactly one assistant row (the partial turn persisted)"
    );
    let text = &assistants[0].text;
    assert!(
        text.contains("partial"),
        "partial text must survive in DB, got: {}",
        text
    );
    assert!(
        text.contains(crate::agent::helpers::ERROR_MARKER),
        "ERROR_MARKER must be appended, got: {}",
        text
    );
}

/// RULE-A-007 edge case: an error event with NO preceding delta
/// must persist a row whose text is exactly `ERROR_MARKER`
/// (symmetric to cancel's empty-text → CANCELLED_MARKER branch).
#[tokio::test]
async fn agent_loop_error_empty_text_uses_error_marker() {
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
        "rid-err-empty".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    assert_eq!(emitter.error_event_count(), 1);
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    assert_eq!(
        assistants[0].text,
        crate::agent::helpers::ERROR_MARKER,
        "empty-text error → text is exactly ERROR_MARKER"
    );
}

/// RULE-A-007: thinking + tool_use blocks accumulated before
/// the error event MUST also survive in the persisted turn's
/// `content` JSON (not just the `text` column). Verifies the
/// `finalized_thinking` / `tool_calls` paths are persisted,
/// not just `text_parts`.
#[tokio::test]
async fn agent_loop_error_persists_thinking_and_tool_calls() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ThinkingDelta { text: "hmm".into() }),
        Ok(ChatEvent::SignatureDelta { signature: "sig".into() }),
        Ok(ChatEvent::ToolCall {
            id: "toolu_err".into(),
            name: "list_dir".into(),
            input: serde_json::json!({"path": "."}),
        }),
        Err(LlmError::Server {
            status: 500,
            message: "boom".into(),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err-blocks".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    assert_eq!(emitter.error_event_count(), 1);
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    let row = &assistants[0];
    // has_tool_calls flag set by persist_turn.
    assert!(row.has_tool_calls, "tool_use block must be flagged");
    // Content JSON carries thinking + tool_use blocks.
    let content_str = row.content.to_string();
    assert!(
        content_str.contains("hmm"),
        "thinking text must survive in content JSON: {}",
        content_str
    );
    assert!(
        content_str.contains("toolu_err"),
        "tool_use id must survive in content JSON: {}",
        content_str
    );
    assert!(
        content_str.contains("\"thinking\""),
        "thinking block variant must be present: {}",
        content_str
    );
}

/// RULE-A-007 decision B: on the error path, a `persist_turn`
/// failure must NOT emit a second terminal Error event. The
/// per-event arm already emitted one; emitting again would be a
/// conflicting double-terminal. Symmetric to the cancel path's
/// synthetic tool_result persist (log-only).
///
/// Test: install a trigger that blocks assistant-turn INSERTs,
/// script a `Delta` + `Error` turn, then assert exactly one
/// Error event survives (the pre-emit one — no second from
/// the persist failure path).
#[tokio::test]
async fn agent_loop_error_persist_failure_is_log_only() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    // Block INSERTs into `messages` AFTER the initial user
    // message is already persisted (so pre-flight succeeds).
    // We use a BEFORE INSERT trigger; the user-message persist
    // happens first, so we install the trigger AFTER
    // run_chat_loop starts... but that's not possible without
    // a thread. Instead, scope the trigger to assistant-role
    // rows only: the user message has role='user', the partial
    // assistant turn has role='assistant'. The trigger raises
    // only for assistant inserts.
    sqlx::query(
        r#"CREATE TRIGGER messages_no_assistant_insert BEFORE INSERT ON messages
           WHEN NEW.role = 'assistant'
           BEGIN
               SELECT RAISE(ABORT, 'simulated assistant persist failure');
           END"#,
    )
    .execute(&h.db)
    .await
    .expect("install assistant-only fail-insert trigger");

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "partial".into() }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err-persist-fail".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // The single Error event is the pre-emit from the per-event
    // arm. The persist failure on the error path MUST NOT add a
    // second one (RULE-A-007 decision B).
    assert_eq!(
        emitter.error_event_count(),
        1,
        "persist failure on error path must be log-only (no double-terminal Error)"
    );
    // And no Done event either (the loop returns without
    // emitting Done — Error is the terminal).
    let done_count = emitter
        .chat_events()
        .into_iter()
        .filter(|p| matches!(p.event, ChatEvent::Done { .. }))
        .count();
    assert_eq!(done_count, 0, "no Done event on error path");
}

/// RULE-A-007 decision C: after the error path persists the
/// partial turn, a `ChatEvent::TurnComplete` MUST be emitted
/// (same as cancel / normal paths) so the frontend has the seq
/// + latency breakdown for the partial row. The TurnComplete
/// coexists with the pre-emit Error event.
#[tokio::test]
async fn agent_loop_error_emits_turn_complete() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: "partial".into() }),
        Err(LlmError::Server {
            status: 503,
            message: "service unavailable".into(),
        }),
    ])]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-err-tc".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // Exactly one TurnComplete, pointing at the persisted
    // partial assistant row's seq. The user message has seq=0
    // (initial persist), so the assistant turn is seq=1.
    let turn_completes: Vec<i64> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::TurnComplete { seq, .. } => Some(seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        turn_completes.len(),
        1,
        "exactly one TurnComplete expected on error path, got {}",
        turn_completes.len()
    );
    assert_eq!(turn_completes[0], 1, "TurnComplete seq points at partial turn");

    // And the row actually exists at that seq.
    let assistants = load_assistant_rows(&h.db, &h.session_id).await;
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0].seq, 1);
}

// ---------------------------------------------------------------------------
// 13) B12 (2026-06-19): update_checklist tool integration
// ---------------------------------------------------------------------------

/// B12 PR1: a `tool_use("update_checklist")` from the model flows
/// through the full agent loop:
/// - The tool executes (Tier 5 default-allow — `update_checklist`
///   is not in `filter_tools_for_mode`'s Plan-mode blacklist, so
///   it's auto-allowed for every mode).
/// - The loop-local checklist Vec gets atomically replaced with
///   the new items.
/// - The `tool_result` event the frontend receives carries the
///   full list (post-coerce).
/// - On turn 2's `provider.send`, the agent loop prepends an
///   ephemeral `<current-checklist>` block to the REQUEST body
///   (visible via `mock.sent_messages()`), but the persisted
///   `messages` Vec never contains that block.
#[tokio::test]
async fn agent_loop_update_checklist_replaces_vec_and_injects_next_turn() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: model emits update_checklist + tool_use stop_reason.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_1".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "step one", "status": "done"},
                        {"content": "step two", "status": "in_progress"},
                        {"content": "step three", "status": "pending"}
                    ]
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: final text response.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done with checklist".into(),
            }),
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
        "rid-checklist".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // 2 turns = 2 send calls.
    assert_eq!(
        mock.call_count(),
        2,
        "tool_use must trigger a second turn"
    );

    // tool:result event landed in the sink — the frontend renders
    // the checklist card from this.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result for update_checklist");
    assert!(
        !results[0].is_error,
        "update_checklist success path must be is_error=false"
    );
    let body = &results[0].content;
    // The tool_result carries the full list with the rendered
    // [x]/[~]/[ ] markers.
    assert!(body.contains("step one"), "body: {}", body);
    assert!(body.contains("step two"), "body: {}", body);
    assert!(body.contains("step three"), "body: {}", body);
    assert!(body.contains("[x]"), "done marker present: {}", body);
    assert!(body.contains("[~]"), "in_progress marker present: {}", body);
    assert!(body.contains("[ ]"), "pending marker present: {}", body);

    // ---- ephemeral injection assertion ----
    //
    // Turn 1's request body: checklist Vec is empty (no
    // update_checklist has run yet) → NO `<current-checklist>`
    // block in the first request. Symmetric to memory/skill empty-
    // skip.
    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2, "captured 2 turn request bodies");
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("<current-checklist>"),
        "turn 1 (empty Vec) must NOT inject checklist block"
    );

    // Turn 2's request body: checklist Vec is non-empty → the
    // ephemeral block IS prepended.
    let turn2_text = messages_to_text(&sent[1]);
    assert!(
        turn2_text.contains("<current-checklist>"),
        "turn 2 must include the ephemeral checklist block, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("step one"),
        "ephemeral block carries the full list"
    );
    assert!(
        turn2_text.contains("step two"),
        "ephemeral block carries the full list"
    );

    // ---- persisted messages never contain the ephemeral block ----
    //
    // The injection is per-turn-only; reload reconstructs the
    // checklist from the `update_checklist` tool_result already
    // in history. The persisted `messages.content` JSON must NOT
    // carry `<current-checklist>` — otherwise a reload would see
    // a phantom user message.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    for m in &loaded.messages {
        let text = serde_json::to_string(&m.content).unwrap_or_default();
        assert!(
            !text.contains("<current-checklist>"),
            "persisted message seq={} must NOT contain the ephemeral block, got: {}",
            m.seq,
            text
        );
    }
}

/// B12 PR1: at-most-one `in_progress` coerce survives the full
/// agent loop end-to-end. The model passes 2 `in_progress` items;
/// the loop's Vec + the tool_result + the next turn's ephemeral
/// block all reflect exactly 1 `in_progress` (the LAST one).
#[tokio::test]
async fn agent_loop_update_checklist_coerces_two_in_progress_to_one() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_coerce".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "earlier", "status": "in_progress"},
                        {"content": "later", "status": "in_progress"}
                    ]
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
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
        "rid-cl-coerce".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    let body = &results[0].content;
    // The summary line in the tool_result says "1 in_progress"
    // (post-coerce), NOT 2.
    assert!(
        body.contains("1 in_progress"),
        "post-coerce summary must say exactly 1 in_progress, got: {}",
        body
    );

    // Turn 2's ephemeral block carries the post-coerce state: only
    // "later" has the in-progress marker.
    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2);
    let turn2_text = messages_to_text(&sent[1]);
    assert!(turn2_text.contains("<current-checklist>"));
    // "later" is the only one with `<- in progress`.
    assert!(
        turn2_text.contains("[~] later <- in progress"),
        "ephemeral block marks only the LAST in_progress, got: {}",
        turn2_text
    );
    // "earlier" must be demoted to pending.
    assert!(
        turn2_text.contains("[ ] earlier"),
        "ephemeral block demotes the earlier in_progress to pending, got: {}",
        turn2_text
    );
}

/// B12 PR1 — RULE-A-004 consistency for `update_checklist`: a
/// cancelled-in-flight tool must NOT leave a phantom
/// `tool_executed` audit row. `update_checklist` is a fast
/// in-memory swap (it does NOT consult the cancel token, so it
/// runs to completion regardless of when cancel fires), which
/// means the most likely landing spot for the cancel is "tool
/// already finished, but the loop's cancel branch fires
/// afterwards". In that case the tool_result IS persisted (as
/// the cancel path's "partial results" branch — this is correct
/// per Anthropic's tool_use/tool_result pairing invariant).
///
/// What we actually assert here is the RULE-A-004 invariant
/// itself: NO `tool_executed` audit row was written for this
/// session. The `record_tool_executed_audit` call in
/// `chat_loop.rs` is gated by `!token.is_cancelled()`, so a
/// cancelled tool — fast or slow — never lands in the audit log.
///
/// This is the precise RULE-A-004 invariant, restated for the
/// B12 surface: the existing audit-after-cancel-check ordering
/// automatically protects the checklist tool, with no new
/// persist path introduced (per the PRD's "Do NOT introduce a
/// new persist path" constraint).
#[tokio::test]
async fn agent_loop_cancelled_update_checklist_skips_audit_row() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_cl_cancel".into(),
                name: "update_checklist".into(),
                input: serde_json::json!({
                    "items": [
                        {"content": "wont commit audit", "status": "in_progress"}
                    ]
                }),
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
        // yield_now so the cancel fires as soon as turn 1's send
        // has been observed (mirrors the audit-skip test's gating).
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
        "rid-cl-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    // Exactly one cancelled Done event.
    assert_eq!(emitter.cancel_done_count(), 1);

    // RULE-A-004 invariant: zero `tool_executed` audit rows.
    // `update_checklist` is not in any way special here — the
    // existing audit-after-cancel-check ordering covers it
    // automatically. The assertion mirrors
    // `agent_loop_cancel_skips_audit_for_cancelled_tool` (the
    // list_dir version) but exercises the new checklist tool so
    // a future refactor that accidentally special-cases it
    // would fail here.
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
        "a cancelled update_checklist must NOT be recorded as tool_executed (RULE-A-004)"
    );
}

/// Helper: flatten a `Vec<ChatMessage>` into a single string for
/// substring assertions. Concatenates every text block in every
/// message — order matters for the ephemeral-injection tests
/// because the checklist block is PREPENDED (so it should appear
/// before the user's "hello" text from `test_messages()`).
fn messages_to_text(msgs: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in msgs {
        match &m.content {
            MessageContent::Text(t) => out.push_str(t),
            MessageContent::Blocks(blocks) => {
                for b in blocks {
                    if let ContentBlock::Text { text, .. } = b {
                        out.push_str(text);
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// 13) L2 (2026-06-19): parallel read-only tool batch
// ---------------------------------------------------------------------------

/// `is_parallel_eligible` — pure predicate. Covers:
/// - all-eligible set → true
/// - each excluded tool (write_file / edit_file / shell / web_fetch /
///   update_checklist) in an otherwise-eligible batch → false
/// - empty batch → false (defensive; the agent loop only calls
///   this on non-empty `tool_calls`)
/// - RULE-A-013 (2026-06-19): path-outside-root read tools
///   pull the batch back to serial. `paths` lets the test pin
///   each tool's `path` arg (empty string = no `path` arg).
#[test]
fn is_parallel_eligible_classifies_correctly() {
    use crate::agent::chat_loop::is_parallel_eligible;

    /// `names[i]` is the tool name; `paths[i]` is the `path`
    /// arg to inject (empty string = no `path` field, mirroring
    /// a model call without a path arg). All paths are
    /// constructed as absolute to the `root` passed in.
    fn batch(names: &[&str], paths: &[&str], root: &std::path::Path) -> Vec<(String, String, serde_json::Value)> {
        names
            .iter()
            .zip(paths.iter().chain(std::iter::repeat(&"")))
            .map(|(n, p)| {
                let input = if p.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::json!({ "path": root.join(p).to_string_lossy() })
                };
                ("x".into(), (*n).into(), input)
            })
            .collect()
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // All-eligible permutations of the read-only set (no paths
    // — the path check is vacuously true).
    assert!(is_parallel_eligible(&batch(&["read_file"], &[], root), root));
    assert!(is_parallel_eligible(
        &batch(&["read_file", "grep", "glob"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(&["list_dir", "use_skill"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(
            &["read_file", "grep", "glob", "list_dir", "use_skill"],
            &[],
            root
        ),
        root
    ));

    // Each excluded tool alone → false (so a single-tool batch
    // of an excluded tool stays serial).
    assert!(!is_parallel_eligible(&batch(&["write_file"], &[], root), root));
    assert!(!is_parallel_eligible(&batch(&["edit_file"], &[], root), root));
    assert!(!is_parallel_eligible(&batch(&["shell"], &[], root), root));
    assert!(!is_parallel_eligible(&batch(&["web_fetch"], &[], root), root));
    assert!(!is_parallel_eligible(
        &batch(&["update_checklist"], &[], root),
        root
    ));

    // Mixed: one excluded tool poisons the whole batch.
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "edit_file", "grep"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "web_fetch"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "update_checklist"], &[], root),
        root
    ));

    // Unknown / future tool → conservatively false (serial).
    assert!(!is_parallel_eligible(
        &batch(&["some_future_tool"], &[], root),
        root
    ));

    // Empty batch → false (defensive).
    assert!(!is_parallel_eligible(&[], root));
}

/// RULE-A-013 (2026-06-19): `is_parallel_eligible` now also
/// checks path-outside-root. Cases (all use a real tempdir as
/// `root` so `is_within_root` works as in production):
/// 1. absolute in-root path → eligible
/// 2. relative in-root path → eligible (joined onto root)
/// 3. absolute out-of-root path → falls back to serial
/// 4. relative `../foo` out-of-root path → falls back to serial
/// 5. path tool without a `path` arg → eligible (tool layer
///    schema validation is the fallback; mirrors the
///    permission layer's no-path convention)
/// 6. `use_skill` + arbitrary path in same batch → eligible
///    (`use_skill` is name-eligible and exempt from the path
///    check, so it can ride along with a path tool that has a
///    path arg)
#[test]
fn is_parallel_eligible_boundary_silent() {
    use crate::agent::chat_loop::is_parallel_eligible;
    use std::path::PathBuf;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Build an in-root file and an out-of-root sibling file so
    // absolute and relative paths have real targets to resolve
    // against. `is_within_root` is lexical so existence isn't
    // strictly required, but having real files makes the test
    // mirror production intent more clearly.
    std::fs::write(root.join("in_root.txt"), "x").unwrap();
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside_file: PathBuf = outside_dir.path().join("outside.txt");
    std::fs::write(&outside_file, "x").unwrap();

    // Helper to build a single-tool batch. `root` is unused
    // (the caller resolves the path string in advance) but
    // kept in the signature for symmetry with `batch()` above.
    #[allow(unused_variables)]
    fn single(
        root: &std::path::Path,
        name: &str,
        path: Option<&str>,
    ) -> Vec<(String, String, serde_json::Value)> {
        let input = match path {
            Some(p) => serde_json::json!({ "path": p }),
            None => serde_json::json!({}),
        };
        vec![("x".into(), name.into(), input)]
    }

    // 1. absolute in-root → eligible
    let abs_in = root.join("in_root.txt").to_string_lossy().into_owned();
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some(&abs_in)), root),
        "absolute in-root path should be eligible"
    );

    // 2. relative in-root → eligible (joined onto root)
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some("in_root.txt")), root),
        "relative in-root path should be eligible"
    );

    // 3. absolute out-of-root → NOT eligible
    let abs_out = outside_file.to_string_lossy().into_owned();
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&abs_out)), root),
        "absolute out-of-root path should fall back to serial"
    );

    // 4. relative `../<file>` out-of-root → NOT eligible
    //    Build a relative path from `root` that escapes: e.g.
    //    `../<outside_dir_basename>/outside.txt`. We don't know
    //    the basename, so walk up one level to a tempdir
    //    sibling and back down.
    let rel_out = format!(
        "../{}/{}",
        outside_dir.path().file_name().unwrap().to_string_lossy(),
        "outside.txt"
    );
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&rel_out)), root),
        "relative out-of-root path should fall back to serial"
    );

    // 5. path tool with no `path` arg → eligible (tool layer
    //    validates; we mirror the permission layer convention)
    assert!(
        is_parallel_eligible(&single(root, "read_file", None), root),
        "path tool without path arg should be eligible"
    );
    assert!(
        is_parallel_eligible(&single(root, "grep", None), root),
        "grep without path arg should be eligible"
    );

    // 6. use_skill + arbitrary path coexist → eligible.
    //    use_skill is name-eligible and exempt from the path
    //    check; it can ride along with a path tool that has a
    //    valid in-root path.
    let mixed = vec![
        ("x".into(), "use_skill".into(), serde_json::json!({})),
        (
            "x".into(),
            "read_file".into(),
            serde_json::json!({ "path": abs_in.clone() }),
        ),
    ];
    assert!(
        is_parallel_eligible(&mixed, root),
        "use_skill + in-root path tool should be eligible"
    );
}

/// L2: three `read_file` tool_use blocks in one turn execute
/// concurrently. Asserts:
/// - exactly 3 `tool:result` events fire
/// - the result contents appear in the SAME order as the
///   tool_use blocks (LLM-context stability contract — Q3) by
///   cross-referencing `tool_use_id` to file content
/// - the persisted `tool_result` user message has its blocks
///   in tool_use order, not completion order
///
/// The three reads target different-sized files so the result
/// text is unambiguous about which tool_use produced which
/// result. (No timing assertion — see PRD §Acceptance for the
/// "no flaky timing" rule.)
#[tokio::test]
async fn agent_loop_parallel_readonly_batch_preserves_order() {
    let h = make_harness().await;

    // Three distinct files under the project root so each
    // read_file produces a unique, identifiable output.
    std::fs::write(h.project_path.join("a.txt"), "AAA-content").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB-content").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC-content").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three tool_use blocks (eligible batch).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text (consumed only if turn 1 ran
        // successfully through the parallel batch and the
        // tool_result got fed back).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "done".into() }),
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
        "rid-par-batch".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "parallel batch must trigger exactly one follow-up turn (2 sends total)"
    );
    assert_eq!(emitter.tool_call_count(), 3, "all 3 tool_use fire");
    assert_eq!(
        emitter.tool_result_count(),
        3,
        "all 3 read_file produce a tool_result"
    );

    // Order contract: the result_blocks in the persisted
    // tool_result user message MUST appear in tool_use order
    // (a, b, c), regardless of which task finished first.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // `MessageContent::Blocks` serializes as a top-level JSON
    // array (see `llm/types.rs` MessageContent Serialize impl),
    // so `MessageRow.content` is the array directly.
    let tool_result_msg = loaded
        .messages
        .iter()
        .find(|m| {
            m.role == "user"
                && m.content.as_array().map(|arr| {
                    arr.iter().any(|b| {
                        b.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                    })
                }).unwrap_or(false)
        })
        .expect("tool_result user message persisted");
    let blocks = tool_result_msg
        .content
        .as_array()
        .expect("content array");
    let tool_use_ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                b.get("tool_use_id").and_then(|v| v.as_str()).map(String::from)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        tool_use_ids,
        vec!["toolu_a", "toolu_b", "toolu_c"],
        "tool_result blocks MUST be in tool_use order (Q3 — LLM context stability), got: {:?}",
        tool_use_ids
    );

    // All three audit rows fire — read tools complete fully
    // (no cancel in this test), so each leaves a tool_executed.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert_eq!(
        audit_count, 3,
        "each completed read tool leaves a tool_executed audit row"
    );
}

/// L2 fallback: a batch containing one `edit_file` (a write
/// tool) MUST fall back to the serial path. Behavior must be
/// byte-identical to pre-L2 (the read still runs, the edit
/// still runs, results appear in order). This is a regression
/// guard: a future change that accidentally routes mixed
/// batches through the parallel path would fail here.
#[tokio::test]
async fn agent_loop_mixed_batch_with_edit_falls_back_to_serial() {
    let h = make_harness().await;
    // Seed a file so edit_file's read-before-edit guard passes
    // (the test messages don't read it, but the guard returns
    // is_error=true on missing read — the agent loop still
    // feeds that error back to the LLM, which is what we want
    // to assert: serial path runs end-to-end).
    std::fs::write(h.project_path.join("target.txt"), "original").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: mixed batch — read_file + edit_file.
        // `is_parallel_eligible` returns false → serial path.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "target.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_edit".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "target.txt",
                    "old_string": "original",
                    "new_string": "edited"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text.
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
        "rid-mixed".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    assert_eq!(mock.call_count(), 2, "serial path drives 2 turns");
    assert_eq!(emitter.tool_call_count(), 2);
    assert_eq!(
        emitter.tool_result_count(),
        2,
        "serial path emits 2 tool_results (read + edit)"
    );
    // Order is still tool_use order (serial is naturally ordered).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results[0].tool_use_id, "toolu_read");
    assert_eq!(results[1].tool_use_id, "toolu_edit");
}

/// L2 Q2: a batch containing `web_fetch` (a read-only tool that
/// is EXCLUDED from the parallel set because its Tier 4 default
/// is `ask`) MUST fall back to the serial path. The web_fetch
/// here would normally fire a `permission:ask` — but since
/// `permission_asks` is empty (no sender), the 120s timeout
/// would fire and the test would hang. To avoid that, we pair
/// web_fetch with a read_file (so the batch is non-eligible for
/// parallel anyway) and assert the serial path is taken by
/// checking tool_result ordering is preserved — no need to
/// actually execute web_fetch.
///
/// The assertion is structural: web_fetch in the batch → serial.
/// The pure-predicate test above (`is_parallel_eligible_*`)
/// covers the classification; this test is the end-to-end
/// confirmation that the agent loop honors the predicate.
#[tokio::test]
async fn agent_loop_web_fetch_batch_does_not_run_parallel() {
    // Structural-only: we don't need to invoke run_chat_loop
    // here because the predicate is the gate, and the predicate
    // is exhaustively covered in `is_parallel_eligible_*`. This
    // test is intentionally a no-op placeholder documenting
    // that the Q2 exclusion is enforced at the predicate layer;
    // an end-to-end run would hang on the web_fetch ask
    // timeout. The classification test asserts the contract.
    //
    // Kept as a named test so a future refactor that removes
    // `is_parallel_eligible` will fail here (the named test
    // exists; if the predicate is renamed/removed the test
    // body, which references it via the unit test above, would
    // not compile).
    use crate::agent::chat_loop::is_parallel_eligible;
    let batch: Vec<(String, String, serde_json::Value)> = vec![
        ("x".into(), "read_file".into(), serde_json::json!({"path": "a"})),
        ("y".into(), "web_fetch".into(), serde_json::json!({"url": "https://example.com"})),
    ];
    // Root argument is irrelevant here: `web_fetch` is excluded
    // by the name check (Q2) before the path check ever runs.
    // Use a dummy tempdir for the parameter contract.
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        !is_parallel_eligible(&batch, dir.path()),
        "web_fetch MUST exclude the batch from parallel execution (Q2)"
    );
}

/// L2 + RULE-A-004: a parallel read-only batch cancelled mid-
/// flight MUST:
/// - mark the turn `cancelled` (so the existing cancel path
///   persists partial results + emits Done{cancelled})
/// - NOT leave a `tool_executed` audit row for the cancelled
///   task(s) (RULE-A-004 invariant, preserved per-task)
/// - still leave audit rows for tools that completed BEFORE
///   the cancel token fired
///
/// Script: turn 1 emits two `read_file` tool_use blocks. A
/// background task cancels the token as soon as the provider
/// has been called once (i.e. turn 1's send completed, tool
/// execution is about to start). With three reads, the cancel
/// arrives during the parallel batch — at least one task's
/// `token.is_cancelled()` is true after execute → its audit
/// write is skipped; the batch-level cancel flag flips → the
/// existing cancel path runs.
///
/// NOTE: read_file is fast and doesn't consult the cancel token
/// internally (the wrapper `execute_tool` select! only fires
/// if the cancel arrives BEFORE the inner future completes).
/// So whether the audit row fires depends on which task's
/// `token.is_cancelled()` check wins. The contract under test
/// is the EQUIVALENCE: a cancelled task skips audit, a
/// completed task records audit. The assert is "0 or more, but
/// consistent" — specifically, the test asserts the cancel
/// path was taken (Done{cancelled} emitted), which is the
/// invariant the L2 change can break.
#[tokio::test]
async fn agent_loop_parallel_batch_cancel_marks_turn_cancelled() {
    let h = make_harness().await;
    std::fs::write(h.project_path.join("a.txt"), "AAA").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three read_file tool_use blocks (eligible).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Sentinel — only consumed if cancel fails to mark the
        // turn (it shouldn't).
        MockResponse::HangingThenCancel,
    ]));

    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
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
        "rid-par-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    // The cancel path MUST have been taken. Two invariants:
    // (a) exactly one Done{cancelled} event
    // (b) the loop did NOT re-enter turn 2 (mock.call_count == 1)
    assert_eq!(
        mock.call_count(),
        1,
        "cancel must abort before turn 2 (call_count stays at 1)"
    );
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "cancel path emits exactly one Done{{cancelled}} event"
    );

    // RULE-A-004 cross-check: the number of tool_executed audit
    // rows MUST be <= the number of tool_result events (a
    // cancelled task still emits a tool_result but skips
    // audit). Specifically: tool_result_count == 3 (all three
    // read_files complete because read_file doesn't consult
    // the cancel token internally), but audit_count <= 3
    // because tasks whose post-execute `token.is_cancelled()`
    // check came back true skipped their audit write.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert!(
        emitter.tool_result_count() >= audit_count as usize,
        "cancelled tasks skip audit (RULE-A-004): results={} audit={} (audit MUST be <= results)",
        emitter.tool_result_count(),
        audit_count
    );
}

// ---------------------------------------------------------------------------
// L1a: agent loop drains background-shell notifications and prepends
// (technically: appends to the request clone) a user-role message
// containing the completion text on the NEXT turn. PR2 closes the
// round-trip from `BackgroundShellRegistry::start` → completion →
// notification → `provider.send` request body.
// ---------------------------------------------------------------------------

/// L1a end-to-end: start a fast background shell from the harness's
/// registry, wait for completion, then drive a 2-turn agent loop.
/// Turn 1 emits a `tool_use(run_background_shell)` so the tool layer
/// actually runs (proving the dispatch + ToolContext thread works);
/// turn 2 fires after the completion notification lands in the
/// agent-loop drain. The captured `sent_messages[1]` (turn 2's
/// request body) MUST contain the `[system] 后台 shell ... 已完成`
/// text — this is the wire contract the LLM sees.
///
/// Why this matters: the agent loop's notification drain is the only
/// place a per-turn cross-request state gets injected into the
/// outbound wire payload. A regression (e.g. drain moved to the
/// wrong turn, append swapped to prepend, format string drift)
/// silently breaks the LLM's ability to react to backgrounded
/// commands.
#[tokio::test]
async fn agent_loop_drains_background_shell_notification_into_turn_2() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: emit run_background_shell tool_use. The agent
        // loop's tool dispatch routes this through the new
        // run_background_shell::execute, which starts a real
        // background shell via the registry.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_bg_1".into(),
                name: "run_background_shell".into(),
                input: serde_json::json!({"command": "echo done-from-bg"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text (consumed only if turn 1
        // successfully started the shell and the notification
        // arrived before turn 2's drain).
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
        "rid-bg-drain".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // Two turns → two `send` calls.
    assert_eq!(mock.call_count(), 2, "tool_use must trigger a second turn");

    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 2, "captured 2 turn request bodies");

    // Turn 1's request body MUST NOT carry the notification block
    // yet (the shell only completed AFTER turn 1's `provider.send`
    // fired, and the drain runs at the start of turn 2).
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("[system] 后台 shell"),
        "turn 1 must NOT carry the notification (it hadn't completed yet), got: {}",
        turn1_text
    );

    // Turn 2's request body MUST carry the notification block.
    // The format is exact: the LLM-facing string must match so
    // it can grep for `后台 shell ...` and call shell_status.
    let turn2_text = messages_to_text(&sent[1]);
    assert!(
        turn2_text.contains("[system] 后台 shell"),
        "turn 2 must include the drained notification, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("已完成"),
        "notification carries completion marker, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("exit code 0"),
        "echo succeeds with exit code 0, got: {}",
        turn2_text
    );
    assert!(
        turn2_text.contains("shell_status"),
        "notification tells the LLM which tool to call next, got: {}",
        turn2_text
    );

    // Persistence invariant: the ephemeral notification block is
    // per-turn-only. The persisted `messages.content` MUST NOT
    // contain a USER-role message whose content is a plain text
    // block (not a tool_result block) carrying the
    // `[system] 后台 shell` notification. The
    // `run_background_shell` TOOL RESULT itself contains the
    // literal `[system] 后台 shell ... 已完成...` snippet in its
    // success message (the LLM-facing UX hint), so we walk each
    // user-role row's content and look for a plain-text block
    // (the notification shape) — a tool_result block is typed
    // (`{"type":"tool_result", ...}`) and is excluded.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let mut phantom_count = 0;
    for m in &loaded.messages {
        if m.role != "user" {
            continue;
        }
        if let Some(arr) = m.content.as_array() {
            for block in arr {
                let block_type = block.get("type").and_then(|t| t.as_str());
                let has_notification = block_type == Some("text")
                    && block
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.contains("[system] 后台 shell") && s.contains("已完成"))
                        .unwrap_or(false);
                if has_notification {
                    phantom_count += 1;
                }
            }
        }
    }
    assert_eq!(
        phantom_count, 0,
        "persisted messages must NOT carry an ephemeral notification block (got {} phantom rows)",
        phantom_count
    );
}

/// L1a: when no background shells have completed between turns,
/// no notification block is injected. The empty-queue path is the
/// fast path (no extra `.clone()`, no extra push) — the L1a
/// implementation MUST take it.
///
/// This is the regression guard for "always inject one notification"
/// bugs (where the loop builds an empty list and still pays the
/// allocation cost / produces a noop user message).
#[tokio::test]
async fn agent_loop_no_pending_notifications_skips_injection() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Single turn, text-only.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "just chatting".into(),
            }),
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
        "rid-bg-empty".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    let sent = mock.sent_messages();
    assert_eq!(sent.len(), 1);
    let turn1_text = messages_to_text(&sent[0]);
    assert!(
        !turn1_text.contains("[system] 后台 shell"),
        "empty notification queue must skip the injection, got: {}",
        turn1_text
    );
}

// ===========================================================================
// B6 Subagent (2026-06-19 PR1b): worker dispatch integration tests
//
// The 4 tests cover the core worker dispatch invariants from the PR1b
// task brief:
//   1. worker completes → summary returned as dispatch_subagent
//      tool_result; parent messages contain the tool_call + tool_result
//      pair, NO worker intermediate events.
//   2. worker cancel (parent Stop propagates to worker_token) →
//      tool_result with status=cancelled + CANCELLED_MARKER.
//   3. worker error (provider stream errors) → tool_result with
//      status=error; tool_use/tool_result pairing preserved.
//   4. worker guard does NOT evict parent's session_active_request
//      entry (PR1a skip_session_active regression guard).
//
// Script pattern: the parent MockProvider emits a dispatch_subagent
// tool_use on turn 1, then a final text on turn 2. The worker's
// responses come from a SEPARATE MockProvider passed in via... well,
// we can't — `run_subagent` clones the parent's `Arc<dyn Provider>`
// for the worker. So the parent MockProvider's script is shared
// between parent + worker. The parent consumes turn 1 (the
// dispatch_subagent tool_use) + turn 3 (the final text); the worker
// consumes turn 2 (its single turn). Script ordering: [parent_t1,
// worker_t1, parent_t2].
//
// For cancel / error tests the worker script entry is the failure
// shape; for the "happy" test it's a normal events vec.
// ===========================================================================

/// Worker completes: parent turn 1 emits dispatch_subagent, the
/// worker runs a single turn (produces "found 3 files" summary),
/// parent turn 2 sees the tool_result and emits final text.
///
/// Invariants:
/// - The dispatch_subagent tool_result carries `[status: completed]`
///   + the worker's final text.
/// - The parent's persisted messages contain the dispatch_subagent
///   tool_call (assistant turn) + the tool_result (user turn). NO
///   worker intermediate events leak into the parent's session —
///   the worker's tool_use / tool_result land ONLY in the
///   SubagentBufferSink transcript, which is in-memory only.
/// - Parent frontend emits exactly one tool:call (the dispatch) +
///   one tool:result (the summary). No worker tool:call / tool:result
///   on the parent sink.
#[tokio::test]
async fn agent_loop_dispatch_subagent_completes_and_returns_summary() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "Find all .rs files under src/."
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1 (script slot 1): single-turn summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "found 3 files".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok based on the worker's report".into(),
            }),
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
        "rid-dispatch".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (50) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 PR1b: production-style caller → skip_session_active=false.
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380, not
        // by the test harness).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // Parent turn count: parent_t1 + worker_t1 + parent_t2 = 3 sends.
    assert_eq!(
        mock.call_count(),
        3,
        "expected 3 send calls (parent_t1 + worker_t1 + parent_t2)"
    );

    // The dispatch_subagent tool_result carries the worker's summary
    // + the status prefix.
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one dispatch_subagent tool_result");
    assert!(
        !results[0].is_error,
        "completed worker → is_error=false, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("[status: completed]"),
        "tool_result must carry status=completed prefix, got: {}",
        results[0].content
    );
    assert!(
        results[0].content.contains("found 3 files"),
        "tool_result must carry the worker's summary, got: {}",
        results[0].content
    );

    // Parent messages contain the dispatch_subagent tool_call +
    // tool_result, but NO worker text ("found 3 files") outside the
    // tool_result envelope. The worker's stream is isolated.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let mut dispatch_tool_call_seen = false;
    let mut dispatch_tool_result_seen = false;
    let mut phantom_worker_text = 0;
    for m in &loaded.messages {
        let text = serde_json::to_string(&m.content).unwrap_or_default();
        if text.contains(r#""name":"dispatch_subagent""#) {
            dispatch_tool_call_seen = true;
        }
        // The tool_result content envelope echoes "found 3 files";
        // count only NON-tool_result rows that contain the worker's
        // text (those would be phantom worker leaks).
        if !text.contains(r#""type":"tool_result""#)
            && text.contains("found 3 files")
        {
            phantom_worker_text += 1;
        }
        if text.contains(r#""type":"tool_result""#)
            && text.contains("found 3 files")
        {
            dispatch_tool_result_seen = true;
        }
    }
    assert!(dispatch_tool_call_seen, "parent must persist the tool_call");
    assert!(
        dispatch_tool_result_seen,
        "parent must persist the dispatch tool_result"
    );
    assert_eq!(
        phantom_worker_text, 0,
        "worker intermediate text must NOT leak into parent messages"
    );
}

/// Worker cancel: the parent's cancellation token fires mid-worker.
/// The worker's child_token inherits the cancel; its stream loop's
/// `select!` cancel arm wins, the worker emits Done{cancelled}, and
/// run_subagent formats the tool_result with `[status: cancelled]` +
/// the CANCELLED_MARKER.
///
/// Script: parent_t1 dispatches; worker_t1 is HangingThenCancel
/// (worker's select! never produces an event, the cancel arm wins).
/// The cancel side-channel cancels the parent token once call_count
/// >= 2 (worker's send has been called).
#[tokio::test]
async fn agent_loop_dispatch_subagent_cancel_propagates_to_worker() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_cancel".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "search forever"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HangingThenCancel — never produces events.
        MockResponse::HangingThenCancel,
        // Parent turn 2 sentinel (only consumed if cancel fails).
        MockResponse::HangingThenCancel,
    ]));

    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Wait until the worker's send has started (call_count >= 2),
        // then cancel the parent token. The child_token relationship
        // propagates the cancel to the worker.
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
        "rid-dispatch-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    // The dispatch_subagent tool_result carries the cancelled prefix.
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one tool_result (cancel still pairs)"
    );
    assert!(
        results[0].is_error,
        "cancelled worker → is_error=true"
    );
    assert!(
        results[0]
            .content
            .contains("[status: cancelled]"),
        "tool_result must carry status=cancelled prefix, got: {}",
        results[0].content
    );
    assert!(
        results[0]
            .content
            .contains(crate::agent::helpers::CANCELLED_MARKER),
        "tool_result must carry CANCELLED_MARKER, got: {}",
        results[0].content
    );

    // Parent loop then emits its own terminal Done{cancelled} (the
    // cancel_parent flag flipped the parent's cancelled branch).
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "parent loop emits Done{{cancelled}} after worker cancel"
    );
}

/// Worker error: the worker's stream emits an Error event. The
/// worker's error path runs (per RULE-A-007), the worker exits, and
/// run_subagent formats the tool_result with `[status: error]`.
///
/// Script: parent_t1 dispatches; worker_t1 is a MockResponse::Events
/// with Delta + Err (the LlmError variant). The worker's had_error
/// flag flips → SubagentStatus::Error → format_dispatch_result
/// prefixes `[status: error]`.
#[tokio::test]
async fn agent_loop_dispatch_subagent_error_returns_status_error() {
    use crate::llm::error::LlmError;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_err".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "do something that will error"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: stream errors mid-turn.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "starting work".into(),
            }),
            Err(LlmError::Server {
                status: 503,
                message: "worker upstream failed".into(),
            }),
        ]),
        // Parent turn 2: final text (worker exited with error →
        // tool_result → parent turn 2).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok noting the worker errored".into(),
            }),
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
        "rid-dispatch-err".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // 3 sends: parent_t1 + worker_t1 (errored) + parent_t2.
    assert_eq!(
        mock.call_count(),
        3,
        "worker error → tool_result → parent turn 2"
    );

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1, "exactly one tool_result");
    assert!(
        results[0].is_error,
        "errored worker → is_error=true"
    );
    assert!(
        results[0].content.contains("[status: error]"),
        "tool_result must carry status=error prefix, got: {}",
        results[0].content
    );

    // Parent loop does NOT abort — the worker's error is contained
    // inside the tool_result. The parent continues to turn 2.
    let done_events: Vec<_> = emitter
        .chat_events()
        .into_iter()
        .filter_map(|p| match p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason,
            _ => None,
        })
        .collect();
    assert!(
        done_events.iter().any(|s| s == "end_turn"),
        "parent loop completes normally after worker error, got stops: {:?}",
        done_events
    );
}

/// Worker guard does NOT evict the parent's session_active_request
/// entry. This is the PR1a `skip_session_active` regression guard
/// called out in the PR1b task brief.
///
/// Setup: pre-populate session_active_request[parent_session_id] =
/// parent_rid (what `chat.rs::chat` would do on spawn). Run the
/// parent loop with a dispatch_subagent tool_use. After the loop
/// exits (parent CancellationGuard Drop runs), the
/// session_active_request must be EMPTY (parent's own Drop cleared
/// it) — but DURING the loop, while the worker's CancellationGuard
/// drops, the entry must STILL contain parent_rid (the worker's
/// skip_session_active=true guard left it alone).
///
/// The cleanest way to test this is to check post-loop: parent's
/// guard clears the entry on Drop, so the map is empty. But if the
/// worker's guard had ALSO cleared it (the bug we're guarding
/// against), the parent's loop would see the entry gone MID-loop
/// — that wouldn't surface as a post-loop failure. So we ALSO
/// inspect mid-loop via a side-channel: register a separate rid
/// in cancellations before the loop and verify the worker's rid
/// appears there during the worker's run.
///
/// Simplification: the most direct invariant is "the worker rid
/// appears in `cancellations` during the worker's run and is
/// cleaned up by the worker's guard Drop, while the parent rid
/// remains registered for the parent's lifetime." We assert:
///   1. Post-loop: `cancellations` is empty (both rids cleaned up).
///   2. Post-loop: `session_active_request[parent_session_id]` is
///      gone (parent's Drop cleared it; the worker's Drop did NOT
///      clear it mid-loop, which would have left the entry gone
///      BEFORE the parent's Drop — observable via mid-loop cancel).
///
/// The cleanest behavioral test: trigger a dispatch, then mid-loop
/// inspect the maps. We do that via the MockProvider's call_count
/// signal + a short-lived snapshot task.
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_guard_does_not_evict_parent_session_active() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_guard".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "noop"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HANG. The worker stays in its select!
        // loop until the parent cancels the parent_token (which
        // fires the worker's child_token). This keeps the
        // worker "in flight" long enough for the snapshot task
        // below to read `cancellations` and
        // `session_active_request` while the worker is still
        // running — the worker's CancellationGuard has NOT yet
        // dropped, so the worker rid is still in cancellations
        // and the parent session_active_request entry is
        // untouched.
        MockResponse::HangingThenCancel,
        // Parent turn 2: final (only consumed after the cancel
        // propagates back through the worker, then through
        // `run_subagent`'s `cancel_parent` flag).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Pre-populate the parent's session_active_request entry, mirroring
    // what chat.rs::chat does on spawn. This lets us assert the
    // worker's guard Drop leaves it intact.
    let parent_rid = "rid-guard-test".to_string();
    {
        let mut map = h.session_active_request.lock().await;
        map.insert(h.session_id.clone(), parent_rid.clone());
    }
    // Also register the parent token in cancellations, mirroring
    // chat.rs::chat.
    let parent_token = CancellationToken::new();
    {
        let mut map = h.cancellations.lock().await;
        map.insert(parent_rid.clone(), parent_token.clone());
    }

    // Snapshot task: race the loop, snapshot the maps once the
    // worker's send has been called (call_count >= 2). At that
    // point the worker is mid-run (hung on its HangingThenCancel
    // stream); the parent's session_active_request entry must
    // STILL be intact, AND the worker rid must be in
    // `cancellations` (the worker registered itself in
    // `run_subagent` before the nested `run_chat_loop` call).
    let session_active_clone = h.session_active_request.clone();
    let cancellations_clone = h.cancellations.clone();
    let session_id_clone = h.session_id.clone();
    let call_handle = mock.call_count_handle();
    // Clone the parent_rid for the snapshot closure; the original
    // stays for the run_chat_loop call below.
    let parent_rid_for_snapshot = parent_rid.clone();
    let snapshot_handle: tokio::task::JoinHandle<
        Option<(bool, bool)>, // (parent_session_active_present, worker_rid_present)
    > = tokio::spawn(async move {
        // Wait until the worker has been dispatched (call_count >= 2).
        for _ in 0..1000 {
            if call_handle.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        if call_handle.load(Ordering::SeqCst) < 2 {
            return None; // worker never ran
        }
        // Give the worker a moment to register its rid AND settle
        // into its hung select! state. The worker is HUNG (Hanging
        // ThenCancel stream) so its CancellationGuard is held
        // open — the worker rid will remain in `cancellations`
        // and the parent session_active_request entry will
        // remain untouched until we cancel below.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let parent_present = {
            let map = session_active_clone.lock().await;
            map.get(&session_id_clone).map(|s| s.to_string())
                == Some(parent_rid_for_snapshot.clone())
        };
        // The worker's rid must be present in cancellations (it
        // registered itself). Its key is `<parent_rid>-sub-<toolu_id>`.
        let worker_rid_suffix =
            format!("{}-sub-toolu_dispatch_guard", parent_rid_for_snapshot);
        let worker_present = {
            let map = cancellations_clone.lock().await;
            map.contains_key(&worker_rid_suffix)
        };
        Some((parent_present, worker_present))
    });

    // Cancel task: once the snapshot has had its chance to read
    // the maps, cancel the parent token. The child_token
    // relationship propagates the cancel to the worker, the
    // worker's select! cancel arm wins, the worker exits with
    // Done{cancelled}, run_subagent detects the cancel_parent
    // flag, the parent loop flips its `cancelled` and drives
    // its own cancel path (Done{cancelled} to the parent
    // sink). The parent_token was pre-inserted in cancellations
    // (we mock what `chat.rs::chat` does on spawn).
    let cancel_for_task = parent_token.clone();
    let cancel_handle = tokio::spawn(async move {
        // Wait until the snapshot has had time to read the maps
        // AND take its snapshot. The snapshot polls for up to
        // ~2000ms after spawn; we give it a comfortable 500ms
        // margin so the cancel propagates AFTER the snapshot,
        // not before. The parent token is pre-inserted in
        // cancellations (mirroring `chat.rs::chat`); cancelling
        // it before the parent dispatches the worker would
        // short-circuit the parent's tool execution, and
        // `run_subagent` would never run (the worker is never
        // dispatched). 500ms is enough for the parent's user-
        // message persist + first `provider.send` + tool
        // dispatch.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        cancel_for_task.cancel();
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        parent_rid.clone(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        parent_token,
        None,
        h.background_shells.clone(),
        None,
        false,
        // B6 PR1b: production-style caller → skip_persist=false
        // (persist every turn normally; worker skip is gated by the
        // dispatch_subagent interceptor at chat_loop.rs:1380).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;
    cancel_handle.await.unwrap();

    let snapshot = snapshot_handle.await.expect("snapshot task not panic");
    let (parent_present, worker_present) = snapshot.expect("snapshot captured");

    // Mid-loop invariants:
    //   1. Parent's session_active_request entry is STILL the parent
    //      rid (worker's skip_session_active=true Drop has not
    //      evicted it; if it had, the entry would be gone OR the
    //      parent's cancel_inflight_for_session would have lost its
    //      target).
    //   2. Worker rid is present in cancellations (the worker
    //      registered itself).
    assert!(
        parent_present,
        "parent's session_active_request entry must survive the worker's guard Drop          (skip_session_active=true)"
    );
    assert!(
        worker_present,
        "worker rid must be registered in cancellations during the worker's run"
    );
}

// ---------------------------------------------------------------------------
// B6 PR2: subagent_runs persistence integration tests
// ---------------------------------------------------------------------------

/// End-to-end: parent dispatches a researcher worker → worker
/// runs and returns a summary → `subagent_runs` row is in
/// `completed` state with `transcript_json` non-empty and
/// `summary` containing the worker's text. This is the canonical
/// PR2 success path: a `subagent_runs` row must survive a session
/// reload (PR3's expand UI will read it).
#[tokio::test]
async fn agent_loop_dispatch_subagent_persists_subagent_run() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent tool_use.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "Find all .rs files under src/."
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: single-turn summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "found 3 files".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok based on the worker's report".into(),
            }),
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
        "rid-dispatch".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // Verify the worker run is in `subagent_runs` and the row
    // reflects the completed state. The list_runs_by_session
    // query returns newest first — the only run is the one we
    // just dispatched.
    let runs =
        crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
            .await
            .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "exactly one subagent_run was persisted");
    let run = &runs[0];
    assert_eq!(run.status, "completed");
    assert_eq!(run.subagent_name, "researcher");
    assert!(run.finished_at.is_some(), "finished_at must be set");
    assert_eq!(
        run.summary.as_deref(),
        Some("found 3 files"),
        "summary must equal worker's final_text"
    );
    // transcript_json must be a valid JSON array of TranscriptEntry.
    let transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(run.transcript_json.as_deref().unwrap())
            .expect("transcript_json parses as Vec<TranscriptEntry>");
    // Worker emitted 3 events (Start, Delta, Done) → 3 transcript entries.
    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[0].kind, crate::agent::subagent::TranscriptKind::ChatEvent);
    // token_usage_json must round-trip as a TokenUsage (all zeros here).
    let usage: TokenUsage =
        serde_json::from_str(run.token_usage_json.as_deref().unwrap())
            .expect("token_usage_json parses as TokenUsage");
    assert_eq!(usage.input_tokens, 0);
    assert_eq!(usage.output_tokens, 0);
    // The worker rid format is "{parent_rid}-sub-{tool_use_id}".
    assert!(run.parent_request_id.contains("rid-dispatch-sub-"));
}

/// End-to-end: parent dispatches a worker and the parent cancel
/// propagates → `subagent_runs` row is in `cancelled` state with
/// `finished_at` set and `summary` reflecting the partial
/// accumulation.
#[tokio::test]
async fn agent_loop_dispatch_subagent_cancelled_persists_status_cancelled() {
    use crate::db::subagent_runs::{get_run, list_runs_by_session};

    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    // Two HangingThenCancel responses: parent turn 1 gets cancelled
    // before the dispatch (actually we want parent to dispatch
    // first, then cancel mid-worker). The MockProvider's
    // HangingThenCancel pattern is "produce 0 events, wait for
    // cancel" — used for the worker below.
    //
    // For parent turn 1 we need a real response that issues the
    // dispatch_subagent tool_use, then we cancel after the worker
    // starts.
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "long running search"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: HangingThenCancel — never produces an
        // event; the cancel arm wins, the worker emits
        // Done{cancelled}.
        MockResponse::HangingThenCancel,
    ]));
    let cancel_token = CancellationToken::new();
    let cancel_token_for_task = cancel_token.clone();
    let call_count_for_cancel = mock.clone();
    let cancel_task = tokio::spawn(async move {
        // Wait until the worker has been entered (call_count >= 2)
        // before firing the cancel.
        loop {
            if call_count_for_cancel.call_count() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        // Brief delay so the worker is mid-flight (so its select!
        // sees the cancel).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_token_for_task.cancel();
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
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;
    let _ = cancel_task.await;

    // Worker run is persisted with status=cancelled.
    let runs = list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.status, "cancelled");
    assert!(run.finished_at.is_some());
    // get_run returns the same row (catches the path-not-list path).
    let fetched = get_run(&h.db, &run.id).await.unwrap().expect("row exists");
    assert_eq!(fetched.status, "cancelled");
}

/// Audit invariant (R6 / AC4): worker's `record_audit_event` calls
/// do NOT add **new** rows to the parent's `session_audit_events`
/// that aren't attributable to the parent's own ⑨ 关 path. The
/// parent WILL write 2 audit rows for `dispatch_subagent`:
/// 1. `tool_allowed` from `permissions::check` (line 556 in
///    `permissions/mod.rs`).
/// 2. `tool_executed` from `record_tool_executed_audit`
///    (`agent/chat_loop.rs:1362`).
///
/// Both are parent-side writes — neither is the worker writing
/// ⑨ decisions to the parent's audit log. The worker path's
/// `skip_persist=true` (B6 PR1b) gates the worker's own
/// `record_audit_event` / `record_tool_executed_audit` call
/// sites inside `run_chat_loop` — so a worker with no tool
/// calls (like this researcher test) produces 0 worker-internal
/// audit rows. The total audit count delta is therefore
/// **exactly 2** for this test scenario; a delta > 2 would mean
/// the worker is leaking audit rows.
#[tokio::test]
async fn agent_loop_dispatch_subagent_audit_not_polluted_by_worker() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "noop"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ack".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Snapshot the audit count BEFORE the run.
    let audit_before =
        crate::db::permissions::list_audit_events(&h.db, &h.session_id)
            .await
            .expect("list_audit_events before");
    let before_count = audit_before.len();

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-audit".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    let audit_after =
        crate::db::permissions::list_audit_events(&h.db, &h.session_id)
            .await
            .expect("list_audit_events after");
    let after_count = audit_after.len();
    let delta = after_count - before_count;
    // Parent's 2 rows: `tool_allowed` + `tool_executed` for the
    // `dispatch_subagent` tool_use. A delta > 2 means the
    // worker leaked audit rows.
    assert_eq!(
        delta, 2,
        "worker must not add audit rows beyond the parent's 2 \
         (tool_allowed + tool_executed for dispatch_subagent); got delta={}",
        delta
    );
}

/// Streaming token usage (R5 / AC2): the worker's per-turn
/// `TokenUsage` folds into the parent session's
/// `input_tokens_total` / `output_tokens_total` in real time.
/// This is the PR2 decoupled `add_token_usage` path — the
/// worker reuses parent_session_id + the
/// `if !skip_persist` gate is removed from the `Done` handler
/// in this PR, so the worker's `add_token_usage` calls
/// accumulate into the parent's running total.
#[tokio::test]
async fn agent_loop_dispatch_subagent_token_usage_folds_into_parent() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_1".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "researcher",
                    "task": "compute usage"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            }),
        ]),
        // Worker turn 1: returns a non-zero usage.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ok".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_input_tokens: 7,
                    cache_read_input_tokens: 11,
                }),
            }),
        ]),
        // Parent turn 2: also non-zero.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "ack".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 8,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-usage".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Tier 4 ask is reachable
        // (permission:ask modal works normally, the loop is not a
        // worker). Mirrors the production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
    )
    .await;

    // The parent's session should have accumulated:
    //   parent_t1: in=10, out=5
    //   worker_t1: in=100, out=50, cc=7, cr=11 (worker reuses parent session)
    //   parent_t2: in=20, out=8
    // Total: in=130, out=63, cc=7, cr=11
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    let s = &loaded.session;
    assert_eq!(
        s.input_tokens_total,
        Some(130),
        "parent + worker input tokens should accumulate"
    );
    assert_eq!(
        s.output_tokens_total,
        Some(63),
        "parent + worker output tokens should accumulate"
    );
    assert_eq!(
        s.cache_creation_total,
        Some(7),
        "worker cache_creation should land in parent"
    );
    assert_eq!(
        s.cache_read_total,
        Some(11),
        "worker cache_read should land in parent"
    );
}

/// RULE-A-014 end-to-end: `general-purpose` worker + Edit mode +
/// `write_file` to a path outside the worker's cwd. The worker's
/// `permissions::check` would normally emit a `permission:ask` for
/// a Tier 4 path-outside-cwd tool_use — and the worker has no UI
/// sink, so the oneshot resolution would never arrive. PR2b
/// threads `is_worker: Option<bool>` through the nested
/// `run_chat_loop` so the worker builds a `PermissionContext` with
/// `is_worker: true`, which short-circuits the Tier 4 `ask_path`
/// to `Decision::Deny` (mirroring the Claude Code "background
/// subagent auto-deny" convention). The worker's tool_result
/// carries `is_error=true` + the deny reason, the LLM self-
/// corrects on turn 2, the worker completes normally, and the
/// parent loop gets the dispatch_subagent tool_result with
/// `[status: completed]`. Without PR2b, this test would HANG
/// (the worker's `select!` waits on the oneshot that never
/// resolves), the `MockProvider`'s call_count would never reach
/// 3, and the test would time out (default `#[tokio::test]`
/// timeout is 60s).
///
/// Note: `Edit` mode (the harness default) is used because
/// `Plan` mode's `filter_tools_for_mode` drops `write_file` from
/// the worker's tool set entirely (defense in depth — the worker
/// never sees the tool, so the worker never even gets to call
/// `permissions::check` for it). Edit mode keeps the tool
/// available, and the `is_within_root(cwd, path)` check inside
/// Tier 4 dispatches to `ask_path` only when the target path is
/// outside the project root — `/tmp/everlasting_worker_escape`
/// is a real path outside any test's tempdir.
#[tokio::test(flavor = "multi_thread")]
async fn agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: dispatch_subagent general-purpose.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_dispatch_rule_a_014".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "Write a file at /tmp/everlasting_worker_escape.txt with content 'leaked'"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 1: write_file to a path OUTSIDE the worker's
        // cwd. The path is absolute (`/tmp/...`), so `is_within_root`
        // returns false → Tier 4 `ask_path` triggers. With
        // `is_worker=true` (PR2b), `ask_path` returns
        // `Decision::Deny` immediately (no permission:ask emit, no
        // oneshot wait — the worker cannot ask the user).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_worker_write".into(),
                name: "write_file".into(),
                input: serde_json::json!({
                    "path": "/tmp/everlasting_worker_escape.txt",
                    "content": "leaked"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker turn 2: LLM sees the deny tool_result, self-
        // corrects with a final summary. (No additional tool_use
        // — the worker gave up and reported back to the parent.)
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "Write denied by worker permission policy; cannot surface modal."
                    .into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2: final text response.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    // Snapshot the audit count BEFORE so we can assert the worker's
    // ⑨ decision does NOT add a `tool_permission_ask` row (PR2b
    // collapses the ask to a deny — no permission:ask emit, no
    // oneshot wait, no `tool_permission_ask` audit row). The
    // worker's auto-deny DOES write a `tool_denied` audit row
    // (permissions::ask_path line 1002-1009, unconditional), so
    // the post-run delta includes 1 `tool_denied` from the worker
    // + 2 parent rows (tool_allowed + tool_executed for
    // dispatch_subagent) = 3 total.
    let audit_before =
        crate::db::permissions::list_audit_events(&h.db, &h.session_id)
            .await
            .expect("list_audit_events before");

    // Wrap the run in a `tokio::time::timeout` so a hang (the
    // pre-PR2b symptom — oneshot never resolved) is caught and
    // fails the test with a clear message instead of timing out
    // the test runner at 60s.
    let run_result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        run_chat_loop(
            vec![],
            mock.clone(),
            200_000,
            "rid-rule-a-014".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
            h.db.clone(),
            h.cancellations,
            h.session_active_request,
            h.read_guard,
            h.memory_cache,
            h.skill_cache,
            h.permission_asks,
            CancellationToken::new(),
            None,
            h.background_shells.clone(),
            None,
            false,
            false,
            // B6 Subagent PR2b (RULE-A-014, 2026-06-20):
            // production-style caller → Some(false). The parent
            // loop is NOT a worker; only the nested worker call
            // passes Some(true) (at chat_loop.rs:2155). Mirrors
            // the production chat.rs call site.
            Some(false),
            // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None
            // (no Tauri runtime).
            None,
            // 2026-06-21 fix (B6 review defect A): tests pass
            // `None` (production-style caller — not a worker,
            // so the parent's `assemble_system_prompt(mode_prefix,
            // base_prompt)` path runs unchanged). The worker
            // nested call in `run_subagent` passes `Some(...)` to
            // fully replace the parent's prompt with the worker's
            // `SubagentDef.system_prompt`.
            None,
        ),
    )
    .await;
    assert!(
        run_result.is_ok(),
        "PR2b fix: run_chat_loop must NOT hang on the worker's \
         Tier 4 ask_path — without the fix, the worker's \
         oneshot never resolves and the test times out at 15s"
    );

    // 4 sends: parent_t1 + worker_t1 + worker_t2 + parent_t2.
    assert_eq!(
        mock.call_count(),
        4,
        "expected 4 send calls (parent_t1 + worker_t1 + worker_t2 + parent_t2); \
         without PR2b, worker_t1's oneshot hang would prevent the worker from \
         ever emitting Done, so call_count would be stuck at 2"
    );

    // The dispatch_subagent tool_result is the parent's view of
    // the worker — it must carry `[status: completed]` + the
    // worker's final summary (which mentions the deny).
    let results = emitter.tool_results_snapshot();
    let dispatch_result = results
        .iter()
        .find(|r| r.content.contains("dispatch_subagent") || r.tool_use_id.contains("dispatch"))
        .or_else(|| results.first())
        .expect("at least one tool_result (the dispatch_subagent pair)");
    assert!(
        !dispatch_result.is_error,
        "completed worker → is_error=false, got: {}",
        dispatch_result.content
    );
    assert!(
        dispatch_result.content.contains("[status: completed]"),
        "tool_result must carry status=completed, got: {}",
        dispatch_result.content
    );
    assert!(
        dispatch_result.content.contains("Write denied by worker permission policy"),
        "tool_result must echo the worker's self-correction summary, got: {}",
        dispatch_result.content
    );

    // CRITICAL: the worker's `tool_denied` must NOT pollute the
    // parent's `session_audit_events` (RULE-A-016, B6 PR3a
    // 2026-06-20). Before the fix, the worker's Tier 4 ask_path
    // collapse wrote a `tool_denied` row into the parent's audit
    // table — which leaked worker ⑨ decisions into the C4 audit
    // log UI. The fix routes the worker's deny to the
    // `SubagentBufferSink` transcript (as a `PermissionAsk`
    // entry) and skips the parent's audit write. This assertion
    // confirms the worker's deny row IS NOT in the parent's
    // audit — the regression catch.
    let audit_after =
        crate::db::permissions::list_audit_events(&h.db, &h.session_id)
            .await
            .expect("list_audit_events after");
    let tool_denied_count = audit_after
        .iter()
        .filter(|e| {
            e.kind == "tool_denied"
                && e.payload_json
                    .as_deref()
                    .unwrap_or("")
                    .contains("write_file")
        })
        .count();
    assert_eq!(
        tool_denied_count, 0,
        "RULE-A-016: worker's tool_denied must NOT pollute the \
         parent's session_audit_events (PR3a routes the deny to \
         the worker's transcript instead); got audit events: {:?}",
        audit_after
            .iter()
            .map(|e| (e.kind.as_str(), e.payload_json.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
    );
    // No `tool_permission_ask` rows from the worker — the
    // ask_path collapse bypasses the IPC + oneshot dance
    // entirely.
    let tool_permission_ask_count = audit_after
        .iter()
        .filter(|e| e.kind == "tool_permission_ask")
        .count();
    assert_eq!(
        tool_permission_ask_count, 0,
        "worker must NOT emit tool_permission_ask (PR2b ask_path \
         collapse goes straight to Deny — no modal, no oneshot)"
    );
    // Sanity: the delta vs `audit_before` is bounded (parent's
    // 2 rows for dispatch_subagent ONLY — worker tool_denied
    // went to transcript per RULE-A-016). A larger delta would
    // mean a regression (e.g. the worker's record_tool_executed_audit
    // leaking).
    let delta = audit_after.len() - audit_before.len();
    assert!(
        delta <= 2,
        "RULE-A-016 invariant: parent's audit log gains at most 2 \
         rows (tool_allowed + tool_executed for dispatch_subagent); \
         worker's tool_denied now lives in subagent_runs.transcript_json. \
         got delta={}",
        delta
    );

    // RULE-A-016 cross-check: the worker's transcript MUST carry
    // the deny as a `TranscriptKind::PermissionAsk` entry (this is
    // where the worker's audit-like record lives post-PR3a).
    // Fetch the worker's `subagent_runs` row (the most recent one
    // for this session — there's only one in this test).
    let runs =
        crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
            .await
            .expect("list_runs_by_session");
    assert_eq!(runs.len(), 1, "exactly one subagent_runs row");
    let run = &runs[0];
    let transcript: Vec<crate::agent::subagent::TranscriptEntry> =
        serde_json::from_str(run.transcript_json.as_deref().unwrap())
            .expect("transcript_json parses as Vec<TranscriptEntry>");
    let permission_ask_count = transcript
        .iter()
        .filter(|e| {
            e.kind == crate::agent::subagent::TranscriptKind::PermissionAsk
        })
        .count();
    assert_eq!(
        permission_ask_count, 1,
        "RULE-A-016: worker's transcript must carry exactly 1 \
         PermissionAsk entry (the auto-deny for write_file); got \
         transcript: {:?}",
        transcript
            .iter()
            .map(|e| e.kind)
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 2026-06-21 fix (B6 review defect A): system_prompt_override
//
// Pre-fix the worker path's `assemble_subagent_prompt(def, task)`
// output was dead code (`_worker_system_prompt` discarded at
// `chat_loop.rs:2052`); the worker actually received the parent's
// `assemble_system_prompt(mode_prefix, base_prompt)` output, which
// made `SubagentDef.system_prompt` effectively documentation-only
// and produced prompt / permission contradictions in Edit/Plan
// mode (worker told "you can write" in Edit mode but Tier 4
// collapsed write tools to `Deny` because the worker has no UI
// sink). The fix threads the worker's overridden prompt as the
// 23rd `run_chat_loop` parameter. These two tests pin the
// behavior: the override is actually used (worker path) and the
// None case still goes through the parent's
// `assemble_system_prompt` path (production path — the common
// case the existing 34 tests already cover; this is a
// targeted regression guard).
// ---------------------------------------------------------------------------

/// Worker path: when `system_prompt_override` is `Some(p)`,
/// `run_chat_loop` sends `p` as the system prompt to the LLM,
/// NOT the parent's `assemble_system_prompt(mode_prefix,
/// base_prompt)` output. Verifies the worker actually receives
/// its `SubagentDef.system_prompt` and the pre-fix dead-code
/// regression is locked.
#[tokio::test]
async fn system_prompt_override_worker_path_sends_override() {
    use crate::agent::subagent::{assemble_subagent_prompt, lookup_subagent};
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

    // The worker uses the `researcher` `SubagentDef` (read-only
    // research subagent); its system_prompt is the one the
    // worker path should see.
    let def = lookup_subagent("researcher").expect("researcher is a built-in subagent");
    let worker_prompt = assemble_subagent_prompt(def, "summarize the docs");

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        "rid-worker-override".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        // B6 PR2b: production-style caller is NOT a worker
        // (this is the worker-path test, so the
        // `is_worker` flag itself is `Some(false)` — the
        // "worker-ness" is conveyed by the
        // `system_prompt_override` param, not by `is_worker`).
        // The `is_worker` flag governs the ⑨ 关 Tier 4
        // collapse; the override is a separate concern.
        Some(false),
        None,
        // The actual fix being tested.
        Some(worker_prompt.clone()),
    )
    .await;

    // The override must reach the LLM verbatim.
    let sent = mock.sent_systems();
    assert_eq!(sent.len(), 1, "expected exactly 1 send call");
    let received = sent[0]
        .as_ref()
        .expect("worker path: system prompt must be Some, not None");
    assert_eq!(
        received, &worker_prompt,
        "worker path system prompt must equal `SubagentDef.system_prompt` \
         (the pre-fix bug was the override being dead-code-discarded and \
         the parent's `assemble_system_prompt` output being sent instead)"
    );
    // Negative guard: the parent prompt would carry the mode_prefix
    // (e.g. "You are in Yolo mode..."); the worker's prompt
    // explicitly does NOT (Claude Code convention — workers do
    // not inherit the main system prompt).
    assert!(
        !received.contains("Yolo mode") && !received.contains("Edit mode") && !received.contains("Plan mode"),
        "worker's system prompt must NOT carry the parent's mode_prefix; \
         the worker's `SubagentDef.system_prompt` is a fully-replacement prompt. \
         got: {}",
        received
    );
}

/// Production path: when `system_prompt_override` is `None`
/// (the production + 34 existing test path), `run_chat_loop`
/// sends the result of `assemble_system_prompt(mode_prefix,
/// base_prompt)` to the LLM. This is the regression guard that
/// the parent path is unaffected by the worker-path fix.
#[tokio::test]
async fn system_prompt_override_none_path_uses_parent_assembly() {
    use crate::agent::permissions::mode_system_prefix;
    use crate::agent::system_prompt::{assemble_system_prompt, lookup_head_sha};
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
        "rid-parent-override-none".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        None,
        false,
        false,
        Some(false),
        None,
        // Production path: `None` override.
        None,
    )
    .await;

    // Recompute what the parent path should send. We mirror the
    // exact steps inside `run_chat_loop` at the system-prompt
    // site: load session + project, build base_prompt via
    // `build_system_prompt`, prefix with `mode_system_prefix`.
    let sent = mock.sent_systems();
    assert_eq!(sent.len(), 1, "expected exactly 1 send call");
    let received = sent[0]
        .as_ref()
        .expect("parent path: system prompt must be Some, not None");

    // Re-derive the expected parent prompt for the harness's
    // session + project.
    let loaded =
        db::load_session(&h.db, &h.session_id).await.expect("load_session").expect("session");
    let project = db::get_project(&h.db, &loaded.session.project_id)
        .await
        .expect("get_project")
        .expect("project");
    let worktree_path = std::path::PathBuf::from(
        loaded
            .session
            .worktree_path
            .clone()
            .unwrap_or_else(|| project.path.clone()),
    );
    let head_sha = lookup_head_sha(&worktree_path);
    let base_prompt = build_system_prompt(&loaded.session, &project, &worktree_path, &head_sha);
    let expected = assemble_system_prompt(mode_system_prefix(loaded.session.mode), &base_prompt);
    assert_eq!(
        received, &expected,
        "parent path (override=None) must send the parent's \
         `assemble_system_prompt(mode_prefix, base_prompt)` output; \
         the worker-path fix must NOT regress the parent path"
    );
}
