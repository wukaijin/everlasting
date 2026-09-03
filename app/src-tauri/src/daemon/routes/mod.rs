//! Per-domain axum route modules (P2.2 B5).
//!
//! Each submodule mirrors one `commands/*.rs` file. Handler
//! functions are thin JSON wrappers that:
//! 1. Deserialize the request body into the same scalar / struct
//!    arguments the Tauri command accepts (snake_case — body fields
//!    match Rust serde's default, see design.md §3.2).
//! 2. Forward the args to the corresponding `_inner` function in
//!    `crate::commands::*` (Q0 decision — single source of truth
//!    for business logic; no duplication between Tauri and axum).
//! 3. Wrap the `_inner` result in `Json(...)` for axum's response
//!    body serialization. Errors flow through `AppCommandError`'s
//!    `IntoResponse` impl (`daemon::error`).
//!
//! URL layout: `POST /api/v1/{domain}/{command_name}` where
//! `{command_name}` is the exact Tauri command name (e.g.
//! `list_sessions`, `create_session`). The 1:1 name mapping makes
//! the httpTransport shim in P2.3 a mechanical `invoke("cmd", args)`
//! → `fetch("/api/v1/" + domain + "/" + cmd, {method: "POST", body:
//! JSON.stringify(args)})` translation.
//!
//! Domain → module map (79 handlers total):
//! - `agent` (1): `chat`
//! - `attachments`: attachment save
//! - `background_shells` (2): `list_background_shells`, `kill_background_shell` (2026-09-02)
//! - `cancel` (1): `cancel_chat`
//! - `command_palette` (2): `list_commands`, `get_command_body`
//! - `config` (2): `get_llm_config`, `get_home_dir`
//! - `disk` (2): `get_disk_usage`, `run_disk_cleanup` (F3 磁盘治理 PR3,
//!   2026-09-03)
//! - `files` (2): `list_files`, `list_files_at`
//! - `health`: `GET /api/v1/health` (B3)
//! - `memory` (7): `read_memory_layers`, `read_memory_content`, ...
//! - `panel` (3): `list_panel_items`, `get_skill_body`, `list_subagents`
//! - `permissions` (8): `set_session_mode`, `permission_response`, ...
//! - `projects` (9): `list_projects`, `create_project`, `browse_dir`
//!   (2026-09-02 目录浏览模态框), ...
//! - `providers` (13): `list_providers`, `add_provider`, ...
//! - `question` (5): `resolve_tool_question`, `resolve_mode_change`, ...
//! - `review` (2): `get_review_state`, `get_current_task_slug` (C2)
//! - `sessions` (16): `list_sessions`, `create_session`, ...
//! - `subagent_runs` (4): `list_subagent_runs_by_session`, ...
//! - `subagents` (2): `list_subagents_with_model`, `set_subagent_model`
//! - `task` (2): `create_task`, `archive_task`
//! - `ui` (1): `apply_ui_diff`
//! - `worktree` (4): `attach_worktree`, `detach_worktree`, ...

pub mod agent;
pub mod attachments;
// 2026-09-02 (task `09-02-chat-task-panel`): background-shell UI
// observability routes (mirror commands::background_shells, Q0 单源).
pub mod background_shells;
pub mod cancel;
pub mod command_palette;
pub mod config;
// F3 磁盘治理(2026-09-03, task `09-03-f3-disk-governance` PR3):设置面
// 「存储」分类 IPC(占用概览 + 手动清理;mirror commands::disk,Q0 单源)。
pub mod disk;
pub mod files;
pub mod health;
pub mod memory;
pub mod message_queue;
pub mod panel;
// S2 (2026-08-11, task `08-11-tunnel-client`):配对码生成 route(design
// §3.1 清单第 5 条)。注意与 remote 的 `/api/v1/pairing/redeem` 区分:
// 这是 PC daemon 本地 IPC 镜像,redeem 在云服务器侧。
pub mod pairing;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod question;
// C2 (review visualization view, 2026-07-26): review-state.json
// read IPC routes (mirror commands::review).
pub mod review;
// F2 定时任务(2026-08-28, task `08-28-f2-scheduled-tasks`):管理面
// CRUD 四条 route(mirror commands::scheduled_tasks,Q0 单源)。
pub mod scheduled_tasks;
pub mod sessions;
pub mod stream;
pub mod subagent_runs;
pub mod subagents;
pub mod task;
pub mod ui;
pub mod usage;
pub mod worktree;

use std::sync::Arc;

use axum::Router;

use crate::state::AppState;

/// Assemble the full `/api/v1/...` router from the per-domain
/// modules. Each domain module exposes a `router()` function that
/// returns the nested `Router` for its own endpoints; this helper
/// layers them under `/api/v1/{domain}`.
///
/// The health endpoint is registered separately in
/// `daemon::server::build_router` (it's the only GET + the only
/// endpoint that doesn't require `Arc<AppState>`).
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api/v1/agent", agent::router(state.clone()))
        .nest("/api/v1/attachments", attachments::router(state.clone()))
        .nest(
            "/api/v1/background_shells",
            background_shells::router(state.clone()),
        )
        .nest("/api/v1/cancel", cancel::router(state.clone()))
        .nest(
            "/api/v1/command_palette",
            command_palette::router(state.clone()),
        )
        .nest("/api/v1/config", config::router(state.clone()))
        .nest("/api/v1/disk", disk::router(state.clone()))
        .nest("/api/v1/files", files::router(state.clone()))
        .nest("/api/v1/memory", memory::router(state.clone()))
        .nest(
            "/api/v1/message_queue",
            message_queue::router(state.clone()),
        )
        .nest("/api/v1/panel", panel::router(state.clone()))
        .nest("/api/v1/pairing", pairing::router(state.clone()))
        .nest("/api/v1/permissions", permissions::router(state.clone()))
        .nest("/api/v1/projects", projects::router(state.clone()))
        .nest("/api/v1/providers", providers::router(state.clone()))
        .nest("/api/v1/question", question::router(state.clone()))
        .nest("/api/v1/review", review::router())
        .nest(
            "/api/v1/scheduled_tasks",
            scheduled_tasks::router(state.clone()),
        )
        .nest("/api/v1/sessions", sessions::router(state.clone()))
        .nest(
            "/api/v1/subagent_runs",
            subagent_runs::router(state.clone()),
        )
        .nest("/api/v1/subagents", subagents::router(state.clone()))
        .nest("/api/v1/task", task::router(state.clone()))
        .nest("/api/v1/ui", ui::router(state.clone()))
        .nest("/api/v1/usage", usage::router(state.clone()))
        .merge(stream::router(state.clone()))
        .nest("/api/v1/worktree", worktree::router(state))
}
