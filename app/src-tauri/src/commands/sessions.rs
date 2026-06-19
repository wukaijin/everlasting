//! Session-related Tauri commands.
//!
//! - [`list_sessions`] / [`create_session`] / [`load_session`] /
//!   [`delete_session`] — session CRUD on top of `db::*`.
//! - [`diff_worktree`] — read the session's worktree diff via
//!   [`crate::git::diff`].
//!
//! The worktree lifecycle (attach / detach / delete) is in
//! [`crate::commands::worktree`]; the destructive cancel hook
//! shared with them lives in [`crate::agent::helpers`].

use std::sync::Arc;

use tauri::State;

use crate::agent::helpers::{await_inflight_exit, cancel_inflight_for_session};
use crate::background_shell::BackgroundShellRegistry;
use crate::db;
use crate::git;
use crate::llm::types::MessageContent;
use crate::state::AppState;

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<db::SessionSummary>, String> {
    db::list_sessions(&state.db, &project_id)
        .await
        .map_err(|e| format!("list_sessions failed: {}", e))
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    initial_cwd: String,
    model: Option<String>,
) -> Result<db::SessionRow, String> {
    let model = model.unwrap_or_else(|| state.config.model.clone());
    // Defensive: every session is bound to a project. The frontend
    // is expected to gate this with a "no project = no chat" check,
    // but a stray IPC call should not silently create a
    // legacy-bound session.
    if project_id.trim().is_empty() {
        return Err("create_session: project_id must not be empty".to_string());
    }

    // Step 4 follow-up: worktree is now opt-in. We no longer
    // require the project to be a git repo (that was the step 4
    // v1 hard guard) and we no longer auto-create a worktree. The
    // session is created in `WorktreeState::None`; the user calls
    // `attach_worktree` separately if they want isolation. Non-git
    // projects can now create sessions and send messages.
    let _project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(format!("create_session: project '{}' not found", project_id));
        }
        Err(e) => return Err(format!("create_session: failed to load project: {}", e)),
    };

    let session_id = uuid::Uuid::new_v4().to_string();

    // Read the current default model_id so the session is bound to
    // a specific model at creation time (not just a free-text name).
    let model_id = db::get_config_value(&state.db, "default_model_id")
        .await
        .ok()
        .flatten();

    db::create_session(&state.db, &session_id, &project_id, &initial_cwd, &model, model_id.as_deref())
        .await
        .map_err(|e| format!("create_session: db insert failed: {}", e))
}

#[tauri::command]
pub async fn load_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<db::LoadedSession>, String> {
    db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("load_session failed: {}", e))
}

#[tauri::command]
pub async fn diff_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<git::diff::DiffResult, String> {
    // Look up the session to find its worktree. Pre-step-4
    // sessions (worktree_path NULL) have no diff to show —
    // return an empty result rather than an error so the UI can
    // render "no changes yet" gracefully.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("diff_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("diff_worktree: session '{}' not found", session_id))?;

    let worktree_path = match loaded.session.worktree_path.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            // Pre-step-4 session: no worktree, no diff.
            tracing::debug!(
                session_id = %session_id,
                "diff_worktree: pre-step-4 session, no worktree, returning empty"
            );
            return Ok(git::diff::DiffResult { files: vec![] });
        }
    };

    git::diff::diff_worktree(std::path::Path::new(worktree_path), &session_id)
        .map_err(|e| format!("diff_worktree: {}", e))
}

#[tauri::command]
pub async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    // Step 4 follow-up: in-flight cancel hook. If a chat stream
    // is running for this session, cancel it BEFORE the
    // destructive work. The frontend is expected to disable the
    // delete button while streaming (REQ-13) and to call
    // `cancel_chat` first, but the backend is the last line of
    // defense.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    // RULE-E-005 (2026-06-15): wait for the agent loop to exit
    // before deleting DB rows. Without this, an in-flight
    // `persist_turn` after deletion writes to a session that no
    // longer exists (orphan rows / FK violation / blank reload).
    await_inflight_exit(exit_rx, "delete_session").await;

    // RULE-B-001 (2026-06-16): drop any pending `permission:ask`
    // oneshot senders for this session. With the agent loop
    // already exited this mostly clears residual entries (its
    // CancellationToken already raced the ask future to Deny),
    // but wiring it explicitly removes the latent dependency on
    // the biased select! and stops the store leaking entries
    // across session churn. cancel_session_asks filters by
    // session_id (RULE-B-002), so other sessions' pending asks
    // are untouched.
    crate::agent::permissions::cancel_session_asks(
        &state.permission_asks,
        &session_id,
    )
    .await;

    // Clear the in-memory ReadGuard for this session so we don't
    // leak fingerprints for a session the user just deleted.
    state.read_guard.clear_session(&session_id).await;

    // Load the session row BEFORE the destructive work so the
    // cwd / worktree cleanup below knows what to tear down.
    // (The memory cache needs no explicit invalidation: the
    // mtime fence in `load_for_session` re-reads on the next
    // access, and deleting a session does not touch the
    // project's memory files anyway.)
    let session_for_cleanup = db::load_session(&state.db, &session_id)
        .await
        .ok()
        .flatten();

    if let Some(ref loaded) = session_for_cleanup {
        let cwd = &loaded.session.current_cwd;
        if !cwd.trim().is_empty() {
            crate::tools::shell::cleanup_outputs_dir(std::path::Path::new(cwd)).await;
        }
    }

    // L1a (2026-06-19): kill every background shell belonging to
    // this chat session. Fire-and-forget — `kill_all_for_session`
    // sends kill signals to the running senders and returns
    // immediately; the spawned tasks do the actual teardown async
    // (process-group SIGKILL + reaping). Not awaiting the teardown
    // here is intentional: the delete IPC should not block on
    // process cleanup, and a brief window of dangling process
    // group is preferable to a hung delete button. The cancel +
    // ReadGuard + permission-asks cleanup above already took the
    // in-flight agent loop out of the equation.
    if let Err(e) = state
        .background_shells
        .kill_all_for_session(&session_id)
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            error = %e,
            "delete_session: background_shells.kill_all_for_session failed (non-fatal)"
        );
    }

    // Step 4 follow-up: best-effort worktree + branch cleanup.
    // Triggered when the session's `worktree_state` is `active`
    // (NOT `detached` — a detached session's worktree was already
    // removed; deleting a detached session should NOT touch the
    // on-disk artifacts).
    if let Some(ref loaded) = session_for_cleanup {
        if loaded.session.worktree_state == db::WorktreeState::Active {
            if let Some(wt_path) = loaded.session.worktree_path.as_deref() {
                if let Ok(Some(project)) =
                    db::get_project(&state.db, &loaded.session.project_id).await
                {
                    if let Err(e) = git::destroy_worktree(
                        std::path::Path::new(&project.path),
                        std::path::Path::new(wt_path),
                        &session_id,
                    ) {
                        tracing::warn!(
                            session_id = %session_id,
                            worktree = %wt_path,
                            error = %e,
                            "worktree cleanup failed during session delete (non-fatal)"
                        );
                    }
                }
            }
        }
    }

    db::delete_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_session failed: {}", e))
}

/// B3 `/clear`: clear the current session's messages but keep the
/// session row (title/color/mode/model/project/timestamps).
///
/// Mirrors the in-flight cleanup `delete_session` does (cancel any
/// running chat, drop pending permission asks, clear the ReadGuard)
/// so a cleared session starts from a clean runtime slate — but does
/// NOT tear down the worktree or delete the session row. Audit events
/// are kept (they record agent actions, not the live buffer).
#[tauri::command]
pub async fn clear_session_messages(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), String> {
    // Cancel any in-flight chat first (the backend is the last line
    // of defense — the frontend disables the trigger while
    // streaming). Wait for the loop to exit so a late `persist_turn`
    // can't re-write messages we just cleared.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    await_inflight_exit(exit_rx, "clear_session_messages").await;

    // Drop pending permission asks + read fingerprints so the fresh
    // conversation starts clean.
    crate::agent::permissions::cancel_session_asks(&state.permission_asks, &session_id).await;
    state.read_guard.clear_session(&session_id).await;

    // Delete messages only; the session row + audit log survive.
    db::delete_messages_by_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("clear_session_messages failed: {}", e))
}

#[tauri::command]
pub async fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    new_title: String,
) -> Result<(), String> {
    if new_title.trim().is_empty() {
        return Err("rename_session: title must not be empty".to_string());
    }
    db::rename_session(&state.db, &session_id, &new_title)
        .await
        .map_err(|e| format!("rename_session failed: {}", e))
}

#[tauri::command]
pub async fn set_session_color(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    color_tag: Option<i32>,
) -> Result<(), String> {
    db::set_session_color(&state.db, &session_id, color_tag)
        .await
        .map_err(|e| format!("set_session_color failed: {}", e))
}

// ---------------------------------------------------------------------------
// F5 (LLM Latency Tracking): per-message latency + per-tool duration IPCs
//
// Two new commands; both are write-only and fire-and-forget from the
// frontend's `streamController` (the agent loop does not call them).
// The IPC layer's `serde(rename_all)` mirrors the TypeScript payload
// names — see `app/src/stores/streamController.ts` for the consumer.
// ---------------------------------------------------------------------------

/// Update the latency + thinking-time columns on an
/// assistant message row (TTFB / gen / total in
/// milliseconds, plus `thinking_ms` — the F5 follow-up
/// thinking-phase wall-clock). The frontend measures
/// the four values via `Date.now()` deltas around the
/// `start` / first `delta` / `done` events of one chat
/// invocation (and the `thinking_delta` ↔ boundary
/// events for `thinking_ms`), then issues this IPC at
/// `done`.
///
/// The controller tracks the assistant message by its
/// caller-managed `seq` (the same handle it shares with the
/// agent loop), so the IPC takes `(session_id, seq)` and the
/// backend resolves the SQLite row id internally via
/// `find_message_id_by_seq`. Each of the four millisecond
/// values is optional so a cancel / error path can pass
/// `None` for the sub-components (`ttfbMs` / `genMs` /
/// `thinkingMs`) and still record the total
/// time-to-cancel. `thinkingMs` is `None` for messages
/// that never entered the thinking phase — the frontend
/// just doesn't include it in the payload in that case.
#[tauri::command]
pub async fn update_message_latency(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    seq: i64,
    ttfb_ms: Option<i64>,
    gen_ms: Option<i64>,
    total_ms: Option<i64>,
    thinking_ms: Option<i64>,
) -> Result<bool, String> {
    // Resolve the (session_id, seq) pair to the auto-incrementing
    // row id. The seq was assigned by the agent loop in the order
    // user → assistant → user(tool_result) → ... so it's unique
    // within a session by construction (UNIQUE(session_id, seq)
    // constraint in the schema).
    let message_id = match crate::db::find_message_id_by_seq(&state.db, &session_id, seq)
        .await
        .map_err(|e| format!("update_message_latency: lookup failed: {}", e))?
    {
        Some(id) => id,
        None => {
            // No matching row — the agent loop hasn't persisted
            // the assistant turn yet (cancel cleanup can persist
            // after the controller's `done` event fires). Treat
            // as a no-op so the frontend doesn't surface an error
            // for the cancel race.
            return Ok(false);
        }
    };
    let latency = crate::db::sessions::MessageLatency {
        ttfb_ms,
        gen_ms,
        total_ms,
        thinking_ms,
    };
    crate::db::update_message_latency(&state.db, message_id, &latency)
        .await
        .map_err(|e| format!("update_message_latency failed: {}", e))?;
    Ok(true)
}

/// Patch a `duration_ms` field onto the `tool_result` block
/// inside `messages.content` JSON for the given `tool_use_id`.
/// Per PRD ADR-lite decision 1, the per-tool duration lives in
/// the tool_result block itself (no schema change for the tool
/// side). The frontend measures duration as
/// `Date.now() - tool_call_received_at` and issues this IPC
/// on every `tool:result` event.
///
/// `duration_ms` is an i64 (not `Option`) — the IPC always
/// records a value; a missing block in the DB returns `Ok(false)`
/// from the backend (no error), and the frontend treats that as
/// a benign no-op.
#[tauri::command]
pub async fn record_tool_duration(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    duration_ms: i64,
) -> Result<bool, String> {
    crate::db::record_tool_duration(&state.db, &session_id, &tool_use_id, duration_ms)
        .await
        .map_err(|e| format!("record_tool_duration failed: {}", e))
}

// ---------------------------------------------------------------------------
// D3 PR1 (2026-06-17): edit_user_message
//
// User-driven IPC: edit a user message in place + cascade-delete every
// strictly-later message in the session + append an audit row. The
// frontend then re-runs the chat send pipeline (separate IPC, PR2/3
// work) so the agent loop regenerates the assistant + tool_use chain
// against the new prompt.
//
// Three concerns the command owns:
//
// 1. **Stream race (cancel-first)**: if a chat stream is in-flight
//    for this session, cancel it and wait for the loop to exit
//    BEFORE touching the DB. Mirrors `delete_session` /
//    `clear_session_messages` (both call `cancel_inflight_for_session`
//    + `await_inflight_exit`). Without this gate, the in-flight
//    loop's next `persist_turn` writes into a session where the
//    user just deleted the assistant turn.
//
// 2. **Permission bypass**: edit does NOT consult the ⑨ 关
//    permission layer. The industry consensus (Cursor / Cline /
//    Cody / OpenHands / OpenCode) is to bypass the modal entirely
//    for user-initiated direct IPCs; the audit log captures every
//    edit so the user can review later.
//
// 3. **Atomicity**: the DB layer wraps the UPDATE + cascade DELETE
//    + INSERT audit in a single transaction. Any failure rolls
//    back all three. The command surfaces the error as a
//    `Result::Err` string (no `ChatEvent::Error` — there is no
//    active stream to emit on; the caller is the Tauri IPC
//    surface, which already converts `Err` to a rejected JS
//    promise on the frontend).
// ---------------------------------------------------------------------------

/// D3 PR1 IPC: edit a user message in place + cascade-delete tail
/// + record an audit row. The frontend re-runs the chat send
/// pipeline (separate IPC) to regenerate the assistant chain
/// against the new prompt.
///
/// `new_content` is the new `MessageContent` for the user row.
/// The wire shape is the standard Anthropic `ContentBlockPayload`
/// union — `Text` for plain text or `Blocks` for richer content
/// (mirrors what `toPayloadContent` accepts on the send path).
/// The DB layer serializes it via `MessageContent`'s `Serialize`
/// impl so the round-trip is lossless.
///
/// Returns `Ok(())` on success. Errors are wrapped as `String`
/// for the Tauri IPC rejection (the frontend surfaces them as a
/// toast — same contract as `delete_session` /
/// `clear_session_messages`).
#[tauri::command]
pub async fn edit_user_message(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message_seq: i64,
    new_content: MessageContent,
) -> Result<(), String> {
    // 1. Stream race: cancel any in-flight chat on this session
    // first. Mirrors `delete_session` /
    // `clear_session_messages`. Wait for the loop to exit so the
    // DB-layer cascade DELETE can't race an in-flight
    // `persist_turn`.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    await_inflight_exit(exit_rx, "edit_user_message").await;

    // 2. Confirm the session exists. The DB-layer helper is a
    // silent no-op on unknown session (matches the F5 latency
    // IPC contract) — but the user-facing command should
    // surface an explicit error so the frontend doesn't silently
    // succeed on a stale session id.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("edit_user_message: load_session failed: {}", e))?
        .ok_or_else(|| {
            format!("edit_user_message: session '{}' not found", session_id)
        })?;
    // Confirm the user message we're editing exists too — same
    // UX rationale (silent no-op on the DB layer is the F5
    // latency patch pattern; explicit error here so the frontend
    // can toast "message not found").
    let edited_msg = loaded
        .messages
        .iter()
        .find(|m| m.seq == message_seq && m.role == "user")
        .ok_or_else(|| {
            format!(
                "edit_user_message: user message at seq {} not found in session '{}'",
                message_seq, session_id
            )
        })?;
    // Defensive: confirm we resolved the same row we're about to
    // patch. The DB layer uses (session_id, seq) as the lookup
    // key — if the loaded row's id differs from what
    // `find_message_id_by_seq` returns, the helper and the
    // loader disagree, which means a corrupt DB. Surface as an
    // error rather than silently editing the wrong row.
    let resolved_id = db::find_message_id_by_seq(&state.db, &session_id, message_seq)
        .await
        .map_err(|e| format!("edit_user_message: lookup failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "edit_user_message: user message at seq {} not found in session '{}' (resolver mismatch)",
                message_seq, session_id
            )
        })?;
    if resolved_id != edited_msg.id {
        return Err(format!(
            "edit_user_message: resolved id {} != loaded id {} for seq {} — refusing to edit",
            resolved_id, edited_msg.id, message_seq
        ));
    }

    // 3. Hand off to the DB layer. Single transaction wraps the
    // UPDATE + cascade DELETE + INSERT audit; a failure returns
    // `sqlx::Error` which we wrap as a `String` for the IPC
    // rejection. The frontend surfaces as a toast.
    db::edit_user_message(&state.db, &session_id, message_seq, &new_content)
        .await
        .map_err(|e| format!("edit_user_message: db failed: {}", e))
}
