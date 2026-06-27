//! Tauri command surface (the IPC layer).
//!
//! Post-PR1 of the audit task, this module owns every
//! `#[tauri::command]` function. The functions are thin: they
//! dispatch into [`crate::agent`] for the chat loop, [`crate::db`]
//! for CRUD, and [`crate::git`] for worktree lifecycle.
//!
//! Submodules:
//! - [`config`] — `get_llm_config`, `get_home_dir`
//! - [`providers`] — Provider / Model CRUD + `test_model` (and the
//!   deprecated `test_provider`)
//! - [`sessions`] — Session CRUD + `diff_worktree`
//! - [`worktree`] — `attach_worktree` / `detach_worktree` /
//!   `delete_worktree` + the destructive `cancel_inflight_for_session`
//!   hook
//! - [`projects`] — Project CRUD + `pick_project_dir`
//! - [`cancel`] — `cancel_chat`
//!
//! The `chat` command itself lives in [`crate::agent::chat`]
//! because it owns the agent loop, which is not a "thin IPC
//! shim". It is registered in [`crate::commands::all_commands`]
//! alongside the rest.

pub mod cancel;
pub mod command_palette;
pub mod config;
pub mod files;
pub mod memory;
pub mod panel;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod sessions;
pub mod subagent_runs;
pub mod worktree;

/// The full set of Tauri commands, used by `lib.rs::run` to
/// build the `invoke_handler`. This is the single source of
/// truth — adding a new command means adding it here AND in the
/// `tauri::generate_handler!` macro call below.
///
/// Kept as documentation / a sanity check; `lib.rs::run` itself
/// builds the `invoke_handler!` macro from the explicit paths
/// above. The function is `#[allow(dead_code)]` because it's
/// referenced only when running `cargo test` patterns like
/// "did we register everything?".
#[allow(dead_code)]
pub fn all_command_names() -> Vec<&'static str> {
    vec![
        "chat",
        "cancel_chat",
        "get_llm_config",
        "get_home_dir",
        "list_providers",
        "add_provider",
        "update_provider",
        "delete_provider",
        "list_models",
        "add_model",
        "update_model",
        "delete_model",
        "get_default_model",
        "set_default_model",
        "update_session_model_id",
        "test_provider",
        "test_model",
        "list_sessions",
        "create_session",
        "load_session",
        "delete_session",
        "clear_session_messages",
        "attach_worktree",
        "detach_worktree",
        "delete_worktree",
        "diff_worktree",
        "list_projects",
        "list_hidden_projects",
        "create_project",
        "update_project_path",
        "update_project_name",
        "hide_project",
        "unhide_project",
        "pick_project_dir",
        "read_memory_layers",
        "read_memory_content",
        "open_memory_in_editor",
        // B3 /command palette
        "list_commands",
        "get_command_body",
        // B4 skill stretches (2026-06-18): merged panel IPC
        "list_panel_items",
        "get_skill_body",
        // B2 @文件补全
        "list_files",
        // A2 + B7 (Permission system + per-session Mode)
        "set_session_mode",
        "permission_response",
        "grant_tool_permission",
        // C4 (Audit-log query UI, 2026-06-14)
        "list_session_audit_events",
        // D3 PR1 (2026-06-17): edit a user message in place.
        "edit_user_message",
        // B6 PR3a (2026-06-20): subagent_runs list/get for the
        // PR3 frontend `<SubagentDrawer>`. `list_*` is the cheap
        // per-session list (no transcript); `get_*` is the
        // per-run detail (with transcript).
        "list_subagent_runs_by_session",
        "get_subagent_run",
        // L3b PR3 (2026-06-27): merge / discard worker IPCs.
        // The LLM-side path is the `merge_worker` /
        // `discard_worker` tools (tool layer); these commands
        // exist for the PR4 `<SubagentDrawer>` manual
        // merge / discard buttons.
        "merge_worker_run",
        "discard_worker_run",
    ]
}