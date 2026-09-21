//! Project-related Tauri commands (PROPOSAL §4.2 — the
//! project binding + top tabs feature).
//!
//! - [`list_projects`] / [`list_hidden_projects`] — Tab-bar + empty
//!   state panel queries.
//! - [`create_project`] / [`update_project_path`] /
//!   [`update_project_name`] / [`hide_project`] / [`unhide_project`]
//!   — Settings panel CRUD.
//! - [`browse_dir`] — directory listing for the "Add Project"
//!   DirBrowserModal (unified entry for all modes since 2026-09-03;
//!   the former Tauri-only native picker was removed — see task
//!   `09-03-dirbrowser-desktop-unify`).

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

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

// ---------------------------------------------------------------------------
// 09-21-sandbox-net-bindonly: net-tier write channel (R4/R8). The
// authorization truth is the operator-confirmed SNAPSHOT; proposals
// (LLM/manifest) are suggestions that only enter the confirm flow.
// ---------------------------------------------------------------------------

/// The UI read shape for one project's net state.
#[derive(serde::Serialize)]
pub struct ProjectNetState {
    /// Verbatim `projects.sandbox_net` (None = block default).
    pub tier: Option<String>,
    pub snapshots: Vec<crate::db::projects::NetSnapshotRow>,
    pub proposals: Vec<crate::db::projects::NetProposalRow>,
    /// Server-side capability conjunction input: whether BindOnly
    /// can actually be enforced on this kernel (`landlock_net`).
    pub bind_only_supported: bool,
}

/// Validate a net-tier write: exactly `block` or `bind_only:<ports>`
/// (parse shares the effective-read grammar). `allow_all` has NO
/// write surface this task (capability-token precondition — 挂账).
fn validate_settable_net(net: &str) -> Result<crate::sandbox::policy::NetPolicy, AppCommandError> {
    let parsed = crate::sandbox::policy::NetPolicy::parse(net).ok_or_else(|| {
        AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("unknown sandbox net tier: {net} (expected 'block' or 'bind_only:<ports>')"),
        )
    })?;
    if parsed == crate::sandbox::policy::NetPolicy::AllowAll {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "allow_all has no configuration entry in this version (capability-token precondition)",
        ));
    }
    Ok(parsed)
}

/// Validate + clamp-check a port list: 1..=65535, ≤ MAX_BIND_PORTS,
/// and the daemon-port clamp (R4: `snapshot ∩ daemon_listen_ports =
/// ∅` — the authoritative gate at write time; `prepare()` re-clamps
/// defensively).
fn validate_ports(ports: &[u16]) -> Result<Vec<u16>, AppCommandError> {
    if ports.is_empty() || ports.len() > crate::sandbox::policy::MAX_BIND_PORTS {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "port list must have 1..={} entries, got {}",
                crate::sandbox::policy::MAX_BIND_PORTS,
                ports.len()
            ),
        ));
    }
    if ports.contains(&0) {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "port 0 is invalid".to_string(),
        ));
    }
    let reserved = crate::sandbox::policy::daemon_listen_ports();
    let conflicts: Vec<u16> = ports
        .iter()
        .copied()
        .filter(|p| reserved.contains(p))
        .collect();
    if !conflicts.is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "ports {:?} are daemon control-plane ports and can never be in a bind snapshot",
                conflicts
            ),
        ));
    }
    Ok(ports.to_vec())
}

fn ports_to_string(ports: &[u16]) -> String {
    ports
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub async fn get_project_net_state_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<ProjectNetState, AppCommandError> {
    let snapshots = crate::db::projects::list_net_snapshots(&state.db, &id)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?;
    let proposals = crate::db::projects::list_net_proposals(&state.db, &id)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?;
    let tier = crate::db::projects::get_project(&state.db, &id)
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?
        .and_then(|p| p.sandbox_net);
    Ok(ProjectNetState {
        tier,
        snapshots,
        proposals,
        bind_only_supported: crate::sandbox::Capability::probe().landlock_net,
    })
}

pub async fn set_project_sandbox_net_inner(
    state: &Arc<AppState>,
    id: String,
    net: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    let parsed = validate_settable_net(&net)?;
    // Setting the tier directly does NOT mint an authorization: a
    // bind_only tier without a snapshot row for the session's
    // worktree degrades to block at read time (by design).
    crate::db::projects::set_project_sandbox_net(&state.db, &id, Some(parsed.as_str().as_str()))
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))
}

pub async fn propose_net_ports_inner(
    state: &Arc<AppState>,
    id: String,
    worktree_key: String,
    ports: Vec<u16>,
    source: String,
) -> Result<(), AppCommandError> {
    let ports = validate_ports(&ports)?;
    // Clamp-rejected suggestions are rejected AT PROPOSAL time too —
    // a port that can never be confirmed should not queue.
    crate::db::projects::upsert_net_proposal(
        &state.db,
        &id,
        &worktree_key,
        &ports_to_string(&ports),
        &source,
    )
    .await
    .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))
}

pub async fn confirm_net_snapshot_inner(
    state: &Arc<AppState>,
    id: String,
    worktree_key: String,
    ports: Vec<u16>,
    confirmed_by: Option<String>,
) -> Result<ProjectNetState, AppCommandError> {
    let ports = validate_ports(&ports)?;
    let ports_str = ports_to_string(&ports);
    let confirmed_by = confirmed_by.unwrap_or_else(|| "operator".to_string());
    crate::db::projects::upsert_net_snapshot(
        &state.db,
        &id,
        &worktree_key,
        &ports_str,
        &confirmed_by,
    )
    .await
    .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?;
    // The tier column mirrors the latest confirmation (display /
    // roundtrip); the authorization truth remains the snapshot row.
    crate::db::projects::set_project_sandbox_net(
        &state.db,
        &id,
        Some(&format!("bind_only:{ports_str}")),
    )
    .await
    .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?;
    let _ =
        crate::db::projects::set_net_proposal_status(&state.db, &id, &worktree_key, "confirmed")
            .await;
    get_project_net_state_inner(state, id).await
}

pub async fn reject_net_proposal_inner(
    state: &Arc<AppState>,
    id: String,
    worktree_key: String,
) -> Result<ProjectNetState, AppCommandError> {
    crate::db::projects::set_net_proposal_status(&state.db, &id, &worktree_key, "rejected")
        .await
        .map_err(|e| AppCommandError::new(ErrorCategory::InvalidRequest, e.to_string()))?;
    get_project_net_state_inner(state, id).await
}

#[tauri::command]
pub async fn get_project_net_state(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<ProjectNetState, AppCommandError> {
    get_project_net_state_inner(&state, id).await
}

#[tauri::command]
pub async fn set_project_sandbox_net(
    state: State<'_, Arc<AppState>>,
    id: String,
    net: String,
) -> Result<projects::ProjectRow, AppCommandError> {
    set_project_sandbox_net_inner(&state, id, net).await
}

#[tauri::command]
pub async fn propose_net_ports(
    state: State<'_, Arc<AppState>>,
    id: String,
    worktree_key: String,
    ports: Vec<u16>,
    source: String,
) -> Result<(), AppCommandError> {
    propose_net_ports_inner(&state, id, worktree_key, ports, source).await
}

#[tauri::command]
pub async fn confirm_net_snapshot(
    state: State<'_, Arc<AppState>>,
    id: String,
    worktree_key: String,
    ports: Vec<u16>,
    confirmed_by: Option<String>,
) -> Result<ProjectNetState, AppCommandError> {
    confirm_net_snapshot_inner(&state, id, worktree_key, ports, confirmed_by).await
}

#[tauri::command]
pub async fn reject_net_proposal(
    state: State<'_, Arc<AppState>>,
    id: String,
    worktree_key: String,
) -> Result<ProjectNetState, AppCommandError> {
    reject_net_proposal_inner(&state, id, worktree_key).await
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

/// One subdirectory row of the directory browser (`browse_dir`).
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

/// Directory listing for the "添加项目" DirBrowserModal — the
/// unified entry for every mode (desktop / browser / sidecar /
/// remote) since 2026-09-03. Mirrors the outcome of a folder
/// picker: a picked absolute path fed to `create_project`.
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
/// the modal exercises under httpTransport (sidecar / browser /
/// remote).
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
