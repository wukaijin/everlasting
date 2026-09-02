//! Tauri commands for the background-shell UI observability surface
//! (2026-09-02, task `09-02-chat-task-panel`).
//!
//! Two commands, both thin wrappers around the
//! [`crate::background_shell`] registry (Q0 single-source pattern,
//! mirrors `commands::subagent_runs`):
//!
//! - [`list_background_shells`] — returns every non-pruned shell
//!   entry for a session as [`BackgroundShellSummary`] list
//!   (running-first, newest-start first — the registry's
//!   `list_for_session` ordering). Consumed by the frontend
//!   `stores/backgroundShells.ts` on session switch / panel mount;
//!   live updates ride the `background_shell:update` event, this is
//!   the authoritative pull path (page reload, missed events).
//! - [`kill_background_shell`] — force-kill the shell's process
//!   group (registry `kill` passthrough; idempotent on
//!   already-terminal shells). The row flips to its terminal state
//!   via the `exited` event, NOT via this command's return.
//!
//! Errors flow through [`AppCommandError`] per the IPC convention —
//! `BackgroundShellError` already impls the category mapping
//! (`NotFound` → InvalidRequest; `Spawn`/`Poisoned` → Server, see
//! `error.rs`).

use std::sync::Arc;

use tauri::State;

use crate::background_shell::{BackgroundShellRegistry, BackgroundShellSummary};
use crate::error::AppCommandError;
use crate::state::AppState;

/// List every non-pruned background shell for `session_id` (UI
/// summary projection). Unknown session → empty `Vec` (NOT an
/// error); swept entries are naturally absent.
pub async fn list_background_shells_inner(
    session_id: String,
    state: &Arc<AppState>,
) -> Result<Vec<BackgroundShellSummary>, AppCommandError> {
    Ok(state.background_shells.list_for_session(&session_id).await)
}

#[tauri::command]
pub async fn list_background_shells(
    session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<BackgroundShellSummary>, AppCommandError> {
    list_background_shells_inner(session_id, &state).await
}

/// Kill one background shell's process group. Idempotent (`Ok(())`
/// for already-terminal shells, mirroring the `shell_kill` tool UX).
/// Errors (`NotFound` / `WrongSession`) surface as typed
/// `AppCommandError`s for the frontend toast.
pub async fn kill_background_shell_inner(
    session_id: String,
    shell_session_id: String,
    state: &Arc<AppState>,
) -> Result<(), AppCommandError> {
    state
        .background_shells
        .kill(&session_id, &shell_session_id)
        .await
        .map_err(AppCommandError::from)
}

#[tauri::command]
pub async fn kill_background_shell(
    session_id: String,
    shell_session_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), AppCommandError> {
    kill_background_shell_inner(session_id, shell_session_id, &state).await
}
