//! Project-related Tauri commands (PROPOSAL §4.2 — the
//! project binding + top tabs feature).
//!
//! - [`list_projects`] / [`list_hidden_projects`] — Tab-bar + empty
//!   state panel queries.
//! - [`create_project`] / [`update_project_path`] /
//!   [`update_project_name`] / [`hide_project`] / [`unhide_project`]
//!   — Settings panel CRUD.
//! - [`pick_project_dir`] — native directory picker for the
//!   "Add Project" flow.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::db;
use crate::projects;
use crate::state::AppState;

/// Filter for [`list_projects`]. `hidden: true` returns the
/// "recently hidden" list used by the empty-state panel. The
/// default (`hidden: false` or `filter = null`) is the main Tab
/// bar.
#[derive(Serialize, Clone, Deserialize)]
pub struct ListProjectsFilter {
    #[serde(default)]
    pub hidden: Option<bool>,
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, Arc<AppState>>,
    filter: Option<ListProjectsFilter>,
) -> Result<Vec<projects::ProjectRow>, String> {
    let include_hidden = filter
        .as_ref()
        .and_then(|f| f.hidden)
        .unwrap_or(false);
    db::list_projects(&state.db, include_hidden)
        .await
        .map_err(|e| format!("list_projects failed: {}", e))
}

#[tauri::command]
pub async fn list_hidden_projects(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<projects::ProjectRow>, String> {
    db::list_hidden_projects(&state.db)
        .await
        .map_err(|e| format!("list_hidden_projects failed: {}", e))
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::create_project(&state.db, &path).await
}

#[tauri::command]
pub async fn update_project_path(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_path: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::update_project_path(&state.db, &id, &new_path).await
}

#[tauri::command]
pub async fn update_project_name(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_name: String,
) -> Result<projects::ProjectRow, String> {
    projects::store::update_project_name(&state.db, &id, &new_name).await
}

#[tauri::command]
pub async fn hide_project(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    projects::store::hide_project(&state.db, &id).await
}

#[tauri::command]
pub async fn unhide_project(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    projects::store::unhide_project(&state.db, &id).await
}

/// Show a native directory picker. Returns `Some(path)` if the
/// user picked a directory, `None` if they cancelled or the dialog
/// is unavailable.
///
/// The `fallback` argument is reserved for a future "show manual
/// input dialog" UX — for now the frontend uses it to decide
/// whether to surface the fallback input. We do not short-circuit
/// on it here, because the dialog itself either succeeds or the
/// frontend reads `None` and shows the manual input.
#[tauri::command]
pub async fn pick_project_dir(
    app: AppHandle,
    #[allow(unused_variables)] fallback: bool,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PathBuf>>();
    app.dialog()
        .file()
        .set_title("选择项目目录")
        .pick_folder(move |folder| {
            // The callback may fire on the UI thread depending on
            // the platform; we just need to forward the value.
            let path = folder.and_then(|fp| fp.into_path().ok());
            let _ = tx.send(path);
        });
    match rx.await {
        Ok(Some(p)) => Ok(Some(p.to_string_lossy().into_owned())),
        Ok(None) => Ok(None),
        Err(_) => Err("dialog channel closed".to_string()),
    }
}