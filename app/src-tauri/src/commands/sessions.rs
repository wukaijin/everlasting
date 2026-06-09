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

use crate::agent::helpers::cancel_inflight_for_session;
use crate::db;
use crate::git;
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
    db::create_session(&state.db, &session_id, &project_id, &initial_cwd, &model)
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
    cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &session_id,
    )
    .await;

    // Clear the in-memory ReadGuard for this session so we don't
    // leak fingerprints for a session the user just deleted.
    state.read_guard.clear_session(&session_id).await;

    // Best-effort cleanup of disk-spilled shell outputs (PRD §R8).
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