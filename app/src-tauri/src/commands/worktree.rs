//! Worktree lifecycle Tauri commands (Step 4 follow-up:
//! opt-in worktree).
//!
//! Each command starts with an in-flight cancel hook
//! ([`crate::agent::helpers::cancel_inflight_for_session`]) so a
//! streaming LLM can't write into a half-destroyed session /
//! worktree.

use std::sync::Arc;

use tauri::State;

use crate::agent::helpers::{await_inflight_exit, cancel_inflight_for_session};
use crate::db;
use crate::git;
use crate::state::AppState;

#[tauri::command]
pub async fn attach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("attach_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("attach_worktree: session '{}' not found", session_id))?;
    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| format!("attach_worktree: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "attach_worktree: project '{}' not found",
                loaded.session.project_id
            )
        })?;
    if !project.is_git_repo {
        return Err(format!(
            "attach_worktree: project '{}' is not a git repository",
            project.name
        ));
    }

    // State machine guard: attach only valid from `none` or
    // `detached`. A session in `active` state already has a
    // worktree — attaching again is a user error.
    match loaded.session.worktree_state {
        db::WorktreeState::None | db::WorktreeState::Detached => {}
        db::WorktreeState::Active => {
            return Err(format!(
                "attach_worktree: session '{}' already has an active worktree",
                session_id
            ));
        }
    }

    // Reject if the project root is dirty (REQ-8). The new
    // worktree would diverge from a dirty base, which silently
    // loses the user's WIP.
    let project_path = std::path::Path::new(&project.path);
    if let Err(msg) = git::check_clean(project_path) {
        return Err(format!("attach_worktree: {}", msg));
    }

    // ----- Disk + DB write via shared helper -----
    // The helper is the inner work that the `merge_worker`
    // tool-layer lazy-attach path also calls. The
    // state-machine guard stays here at the IPC boundary
    // (re-IPC-attaching an Active session is still a user
    // error). The helper does NOT re-validate that state.
    let data_dir = state.app_data_dir.clone();
    git::worktree::attach_session(&state.db, &project, &session_id, &data_dir)
        .await
        .map_err(|e| format!("attach_worktree: {}", e))?;

    // Reload and return the canonical row.
    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("attach_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "attach_worktree: session '{}' disappeared after attach",
                session_id
            )
        })?;
    Ok(updated.session)
}

#[tauri::command]
pub async fn detach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    // RULE-E-005 (2026-06-15): wait for the agent loop to actually
    // exit before unbinding — a still-running loop could persist a
    // tool_result whose `cwd` envelope points at this worktree,
    // leaving a stale reference after detach.
    await_inflight_exit(exit_rx, "detach_worktree").await;

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("detach_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("detach_worktree: session '{}' not found", session_id))?;

    if loaded.session.worktree_state != db::WorktreeState::Active {
        return Err(format!(
            "detach_worktree: session '{}' is not in 'active' state (current: {:?})",
            session_id, loaded.session.worktree_state
        ));
    }
    let wt_path_str = loaded
        .session
        .worktree_path
        .clone()
        .ok_or_else(|| "detach_worktree: active session has no worktree_path".to_string())?;
    let wt_path = std::path::PathBuf::from(&wt_path_str);

    // REQ-9: refuse if the worktree has uncommitted changes.
    if let Err(msg) = git::check_clean(&wt_path) {
        return Err(format!("detach_worktree: {}", msg));
    }

    // Write the new state FIRST. If the DB update fails, we
    // haven't touched the disk; user can retry. The `git::
    // destroy_worktree` is intentionally NOT called here —
    // detach is "unbind from the session" not "delete the
    // artifacts".
    db::set_worktree_state(
        &state.db,
        &session_id,
        db::WorktreeState::Detached,
        None,
        Some(&wt_path_str),
    )
    .await
    .map_err(|e| format!("detach_worktree: db update failed: {}", e))?;

    let branch = git::worktree::branch_name(&session_id);
    let event_text = format!(
        "worktree detached from {} (changes preserved on branch {})",
        wt_path.display(),
        branch
    );
    if let Err(e) =
        db::insert_system_event(&state.db, &session_id, &event_text, "detached").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "detach_worktree: insert_system_event failed (non-fatal)"
        );
    }

    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("detach_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "detach_worktree: session '{}' disappeared after detach",
                session_id
            )
        })?;
    Ok(updated.session)
}

#[tauri::command]
pub async fn delete_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    // RULE-E-005 (2026-06-15): wait for the agent loop to fully exit
    // BEFORE destroying the worktree dir. Cancel only sets the token
    // flag; the loop checks it at the next stream boundary / after
    // the current tool, so without this await an in-flight tool
    // could write into the directory we're about to `destroy_worktree`
    // (ENOENT / panic / orphaned fingerprint).
    await_inflight_exit(exit_rx, "delete_worktree").await;

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_worktree: failed to load session: {}", e))?
        .ok_or_else(|| format!("delete_worktree: session '{}' not found", session_id))?;

    // Delete is valid from `active` OR `detached`.
    if loaded.session.worktree_state != db::WorktreeState::Active
        && loaded.session.worktree_state != db::WorktreeState::Detached
    {
        return Err(format!(
            "delete_worktree: session '{}' has no worktree to delete (state: {:?})",
            session_id, loaded.session.worktree_state
        ));
    }

    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| format!("delete_worktree: failed to load project: {}", e))?
        .ok_or_else(|| {
            format!(
                "delete_worktree: project '{}' not found",
                loaded.session.project_id
            )
        })?;

    let worktree_path_for_destroy: Option<std::path::PathBuf> = loaded
        .session
        .worktree_path
        .as_deref()
        .or(loaded.session.last_worktree_path.as_deref())
        .map(std::path::PathBuf::from);
    let branch = git::worktree::branch_name(&session_id);

    if let Some(wtp) = &worktree_path_for_destroy {
        if let Err(e) =
            git::destroy_worktree(std::path::Path::new(&project.path), wtp, &session_id)
        {
            tracing::warn!(
                session_id = %session_id,
                worktree = %wtp.display(),
                error = %e,
                "delete_worktree: destroy_worktree failed (non-fatal)"
            );
        }
    } else {
        // No worktree path stored but the state is active or
        // detached. Best-effort: still try to remove the branch.
        let worktree_lookup = &session_id;
        if let Ok(repo) = git2::Repository::open(std::path::Path::new(&project.path)) {
            if let Ok(mut b) = repo.find_branch(&branch, git2::BranchType::Local) {
                let _ = b.delete();
            }
            if let Ok(wt) = repo.find_worktree(worktree_lookup) {
                let _ = wt.prune(None);
            }
        }
    }

    // DB state: clear worktree_path AND last_worktree_path; the
    // branch is gone so re-attach is no longer meaningful.
    db::set_worktree_state(
        &state.db,
        &session_id,
        db::WorktreeState::None,
        None,
        None,
    )
    .await
    .map_err(|e| format!("delete_worktree: db update failed: {}", e))?;

    let event_text = format!(
        "worktree deleted: branch {} and dir {} removed",
        branch,
        worktree_path_for_destroy
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    );
    if let Err(e) =
        db::insert_system_event(&state.db, &session_id, &event_text, "deleted").await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            "delete_worktree: insert_system_event failed (non-fatal)"
        );
    }

    let updated = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| format!("delete_worktree: reload failed: {}", e))?
        .ok_or_else(|| {
            format!(
                "delete_worktree: session '{}' disappeared after delete",
                session_id
            )
        })?;
    Ok(updated.session)
}