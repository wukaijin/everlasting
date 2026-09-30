//! B3 `/command` palette — Tauri command surface.
//!
//! Thin IPC over [`crate::resource_loader`]. The frontend's
//! `<TriggerMenu>` calls `list_commands` when the user types `/` to
//! populate the autocomplete panel (builtin + user + project commands,
//! project-over-user precedence, builtin highest).

use std::sync::Arc;

use crate::error::AppCommandError;
use crate::resource_loader::{list_all, CommandInfo};
use crate::state::AppState;

/// Phase 2.2 `_inner` (Q0): shared business logic, callable from
/// the Tauri command wrapper below + the axum route handler in
/// `daemon::routes::command_palette`.
pub async fn list_commands_inner(
    state: &Arc<AppState>,
    project_id: Option<String>,
) -> Result<Vec<CommandInfo>, AppCommandError> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| anyhow::anyhow!("list_commands: get_project failed: {}", e))?
            .map(|p| p.path),
        None => None,
    };
    Ok(list_all(&state.command_cache, project_path.as_deref()).await)
}

/// Phase 2.2 `_inner` (Q0): shared business logic.
pub async fn get_command_body_inner(
    state: &Arc<AppState>,
    name: String,
    project_id: Option<String>,
) -> Result<Option<String>, AppCommandError> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| anyhow::anyhow!("get_command_body: get_project failed: {}", e))?
            .map(|p| p.path),
        None => None,
    };
    match crate::resource_loader::find_command(&state.command_cache, &name, project_path.as_deref())
        .await
    {
        Some(cmd) => {
            tracing::info!(
                name = %cmd.name,
                path = %cmd.path.display(),
                "command body fetched"
            );
            Ok(Some(cmd.body))
        }
        None => Ok(None),
    }
}
