//! B3 `/command` palette — Tauri command surface.
//!
//! Thin IPC over [`crate::resource_loader`]. The frontend's
//! `<TriggerMenu>` calls `list_commands` when the user types `/` to
//! populate the autocomplete panel (builtin + user + project commands,
//! project-over-user precedence, builtin highest).

use std::sync::Arc;

use tauri::State;

use crate::resource_loader::{CommandInfo, list_all};
use crate::state::AppState;

/// List all commands available to the command palette: builtins
/// (`/help` `/clear` `/new`) + user-layer (`~/.config/everlasting/commands/`)
/// + project-layer (`<project>/.everlasting/commands/`), merged with
/// builtin > project > user precedence.
///
/// `project_id` is `Option` so a session-less context (e.g. the panel
/// open before a project is selected) still lists builtins + user
/// commands. When provided, the project's path is resolved so its
/// commands are scanned (mtime-fenced via `AppState::command_cache`).
#[tauri::command]
pub async fn list_commands(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<Vec<CommandInfo>, String> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| format!("list_commands: get_project failed: {}", e))?
            .map(|p| p.path),
        None => None,
    };
    Ok(list_all(&state.command_cache, project_path.as_deref()).await)
}

/// Fetch a custom command's body for template expansion. Called by
/// the frontend when the user invokes a user/project command — the
/// body is sent to the LLM as the user message. Builtins are handled
/// client-side (no body) and never call this. Returns `None` if no
/// custom command matches `name`.
#[tauri::command]
pub async fn get_command_body(
    state: State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<Option<String>, String> {
    let project_path = match project_id {
        Some(pid) => crate::db::get_project(&state.db, &pid)
            .await
            .map_err(|e| format!("get_command_body: get_project failed: {}", e))?
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
