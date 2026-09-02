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

/// One subdirectory row of the browser-mode directory browser
/// (`browse_dir`).
#[derive(Debug, Clone, Serialize)]
pub struct BrowseDirEntry {
    pub name: String,
    pub path: String,
}

/// Payload returned by [`browse_dir_inner`]: the canonical directory
/// the browser modal is currently showing, its parent (for the
/// ".." / 上一步 row — `None` at the filesystem root), and the
/// visible subdirectory entries.
#[derive(Debug, Clone, Serialize)]
pub struct BrowseDirPayload {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<BrowseDirEntry>,
}

/// Browser-mode directory listing for the "添加项目" modal (web
/// 前端目录浏览选择 —— the degrade when the Tauri native picker is
/// unavailable; mirrors the GUI picker's outcome: a picked absolute
/// path fed to `create_project`).
///
/// Lists **directories only** (the picker's semantics — files are
/// not selectable project roots). Dot-directories are filtered
/// unless `show_hidden`. `path` accepts a leading `~` (expanded via
/// `dirs::home_dir`, same source as `get_home_dir_inner`). The
/// path is canonicalized first so `..` segments and symlinks
/// resolve and the returned `path`/`parent`/entry paths are all
/// absolute and stable for round-tripping back into
/// `create_project`.
///
/// No `_inner` state dependency (pure filesystem read, like
/// `get_home_dir_inner`), so the daemon route handler needs no
/// `AppState`.
pub async fn browse_dir_inner(
    path: String,
    show_hidden: bool,
) -> Result<BrowseDirPayload, AppCommandError> {
    let trimmed = path.trim();
    let expanded: PathBuf = if trimmed == "~" || trimmed.starts_with("~/") {
        match dirs::home_dir() {
            Some(home) => {
                let suffix = trimmed.trim_start_matches('~').trim_start_matches('/');
                if suffix.is_empty() {
                    home
                } else {
                    home.join(suffix)
                }
            }
            None => PathBuf::from(trimmed),
        }
    } else {
        PathBuf::from(trimmed)
    };

    let canonical = tokio::fs::canonicalize(&expanded).await.map_err(|e| {
        AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("路径不存在或不可访问: {e}"),
        )
    })?;
    if !canonical.is_dir() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("不是目录: {}", canonical.display()),
        ));
    }

    let mut read = tokio::fs::read_dir(&canonical).await.map_err(|e| {
        AppCommandError::new(ErrorCategory::InvalidRequest, format!("读取目录失败: {e}"))
    })?;
    let mut entries: Vec<BrowseDirEntry> = Vec::new();
    while let Some(e) = read.next_entry().await.map_err(|e| {
        AppCommandError::new(ErrorCategory::InvalidRequest, format!("读取目录失败: {e}"))
    })? {
        let name = e.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        // `e.path().is_dir()` follows symlinks (unlike
        // `e.file_type()`), so a symlinked project directory shows
        // up — matching what a native folder picker offers.
        if e.path().is_dir() {
            entries.push(BrowseDirEntry {
                name,
                path: e.path().to_string_lossy().into_owned(),
            });
        }
    }
    entries.sort_by_key(|a| a.name.to_lowercase());

    Ok(BrowseDirPayload {
        path: canonical.to_string_lossy().into_owned(),
        parent: canonical.parent().map(|p| p.to_string_lossy().into_owned()),
        entries,
    })
}

/// Tauri wrapper around [`browse_dir_inner`]. Also routed on the
/// daemon (`POST /api/v1/projects/browse_dir`) — that is the path
/// the browser-mode modal actually exercises.
#[tauri::command]
pub async fn browse_dir(
    path: String,
    show_hidden: bool,
) -> Result<BrowseDirPayload, AppCommandError> {
    browse_dir_inner(path, show_hidden).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hidden dirs filtered by default (`show_hidden = false`), files
    /// never listed, entries sorted case-insensitively.
    #[tokio::test]
    async fn browse_dir_lists_dirs_only_sorted_hidden_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Zeta")).unwrap();
        std::fs::create_dir(tmp.path().join("alpha")).unwrap();
        std::fs::create_dir(tmp.path().join(".secret")).unwrap();
        std::fs::write(tmp.path().join("file.txt"), "x").unwrap();

        let payload = browse_dir_inner(tmp.path().to_string_lossy().into_owned(), false)
            .await
            .unwrap();
        let names: Vec<&str> = payload.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "Zeta"],
            "大小写不敏感排序、隐藏目录与文件被过滤"
        );
        assert_eq!(
            payload.path,
            tmp.path().to_string_lossy(),
            "返回 canonical 路径"
        );
        assert!(payload.parent.is_some(), "tempdir 有父目录");

        // show_hidden = true:dot 目录出现且排最前("." < 字母)
        let payload = browse_dir_inner(tmp.path().to_string_lossy().into_owned(), true)
            .await
            .unwrap();
        let names: Vec<&str> = payload.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".secret", "alpha", "Zeta"]);
    }

    /// 不存在的路径 / 指向文件的路径 → InvalidRequest 错误(前端模态框
    /// 显示行内错误,列表保持上一次状态)。
    #[tokio::test]
    async fn browse_dir_rejects_missing_path_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();

        let err = browse_dir_inner(
            tmp.path().join("nope").to_string_lossy().into_owned(),
            false,
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("路径不存在"), "{}", err.message);

        let err = browse_dir_inner(file.to_string_lossy().into_owned(), false)
            .await
            .unwrap_err();
        assert!(err.message.contains("不是目录"), "{}", err.message);
    }

    /// `~` 前缀展开到 home(`get_home_dir` 同源的 `dirs::home_dir`),
    /// 展开后路径存在且是目录(CI 盒子必有 $HOME)。
    #[tokio::test]
    async fn browse_dir_expands_tilde_to_home() {
        let payload = browse_dir_inner("~".to_string(), false).await.unwrap();
        assert_eq!(
            payload.path,
            dirs::home_dir().unwrap().to_string_lossy(),
            "~ 必须展开为 home 绝对路径"
        );
    }
}
