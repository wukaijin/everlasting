//! Project-related Tauri commands (PROPOSAL §4.2 — the
//! project binding + top tabs feature).
//!
//! - [`list_projects`] / [`list_hidden_projects`] — Tab-bar + empty
//!   state panel queries.
//! - [`create_project`] / [`update_project_path`] /
//!   [`update_project_name`] / [`hide_project`] / [`unhide_project`]
//!   — Settings panel CRUD.
//! - [`pick_project_dir`] — native directory picker for the
//!   "Add Project" flow (Tauri-only; browser-side UX degradation
//!   lands in P2.4 per PRD R10).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::projects;
use crate::state::AppState;

/// Filter for [`list_projects`]. `hidden: true` returns the
/// "recently hidden" list used by the empty-state panel. The
/// default (`hidden: false` or `filter = null`) is the main Tab
/// bar.
#[derive(Serialize, Clone, Deserialize, Debug)]
pub struct ListProjectsFilter {
    #[serde(default)]
    pub hidden: Option<bool>,
}

/// Phase 2.2 `_inner` (Q0): shared business logic. Callable from
/// the Tauri command wrapper below + the axum route handler in
/// `daemon::routes::projects`.
pub async fn list_projects_inner(
    state: &Arc<AppState>,
    filter: Option<ListProjectsFilter>,
) -> Result<Vec<projects::ProjectRow>, AppCommandError> {
    let include_hidden = filter.as_ref().and_then(|f| f.hidden).unwrap_or(false);
    db::list_projects(&state.db, include_hidden)
        .await
        .map_err(|e| anyhow::anyhow!("list_projects failed: {}", e).into())
}

#[tauri::command]
pub async fn list_projects(
    state: State<'_, Arc<AppState>>,
    filter: Option<ListProjectsFilter>,
) -> Result<Vec<projects::ProjectRow>, AppCommandError> {
    list_projects_inner(&state, filter).await
}

pub async fn list_hidden_projects_inner(
    state: &Arc<AppState>,
) -> Result<Vec<projects::ProjectRow>, AppCommandError> {
    db::list_hidden_projects(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_hidden_projects failed: {}", e).into())
}

#[tauri::command]
pub async fn list_hidden_projects(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<projects::ProjectRow>, AppCommandError> {
    list_hidden_projects_inner(&state).await
}

pub async fn create_project_inner(
    state: &Arc<AppState>,
    path: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    projects::store::create_project(&state.db, &path)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e))
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    create_project_inner(&state, path).await
}

pub async fn update_project_path_inner(
    state: &Arc<AppState>,
    id: String,
    new_path: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    projects::store::update_project_path(&state.db, &id, &new_path)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e))
}

#[tauri::command]
pub async fn update_project_path(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_path: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    update_project_path_inner(&state, id, new_path).await
}

pub async fn update_project_name_inner(
    state: &Arc<AppState>,
    id: String,
    new_name: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    projects::store::update_project_name(&state.db, &id, &new_name)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e))
}

#[tauri::command]
pub async fn update_project_name(
    state: State<'_, Arc<AppState>>,
    id: String,
    new_name: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    update_project_name_inner(&state, id, new_name).await
}

/// P3c(design §2):`update_project_sandbox_policy` 允许写的档位
/// 白名单。与 `SETTABLE_APP_FLAGS` 同款防呆:projects.sandbox_policy
/// 有 DB CHECK 兜底,但 IPC 入口先拒(错误归 `InvalidRequest`),
/// 不让脏值落库。
const SETTABLE_SANDBOX_POLICIES: &[&str] = &["off", "readwrite", "readonly"];

pub async fn update_project_sandbox_policy_inner(
    state: &Arc<AppState>,
    id: String,
    policy: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    if !SETTABLE_SANDBOX_POLICIES.contains(&policy.as_str()) {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "unknown sandbox policy: {policy} (expected one of {})",
                SETTABLE_SANDBOX_POLICIES.join(", ")
            ),
        ));
    }
    crate::db::projects::set_project_sandbox_policy(&state.db, &id, &policy)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))
}

#[tauri::command]
pub async fn update_project_sandbox_policy(
    state: State<'_, Arc<AppState>>,
    id: String,
    policy: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    update_project_sandbox_policy_inner(&state, id, policy).await
}

pub async fn hide_project_inner(state: &Arc<AppState>, id: String) -> Result<(), AppCommandError> {
    projects::store::hide_project(&state.db, &id)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e))
}

#[tauri::command]
pub async fn hide_project(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), AppCommandError> {
    hide_project_inner(&state, id).await
}

pub async fn unhide_project_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<(), AppCommandError> {
    projects::store::unhide_project(&state.db, &id)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e))
}

#[tauri::command]
pub async fn unhide_project(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), AppCommandError> {
    unhide_project_inner(&state, id).await
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
///
/// **Phase 2.2 note**: this command is Tauri-only — there is no
/// `_inner` extraction because the entire body is the Tauri dialog
/// API call. The daemon route handler (`daemon::routes::projects`)
/// surfaces a "use manual path input" error per PRD R10 (browser
/// UX degradation); P2.4 lands the unified `<ProjectDirPicker
/// mode="auto">` abstraction.
#[tauri::command]
pub async fn pick_project_dir(
    app: AppHandle,
    #[allow(unused_variables)] fallback: bool,
) -> Result<Option<String>, AppCommandError> {
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
        Err(_) => Err(AppCommandError::new(
            ErrorCategory::Server,
            "dialog channel closed",
        )),
    }
}
