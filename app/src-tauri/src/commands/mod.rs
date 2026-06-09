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
pub mod config;
pub mod projects;
pub mod providers;
pub mod sessions;
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
    ]
}