//! Everlasting Tauri app entry point.
//!
//! Post-PR1 of the audit task (Step 8-PR1): this file is now a
//! thin shell. It only declares the modules and registers the
//! Tauri command surface. The actual logic lives in:
//!
//! - [`state`] — `AppState`, `CancellationGuard`, event payloads,
//!   `ProviderCatalog` (grill decision #3).
//! - [`commands`] — every `#[tauri::command]` function (the IPC
//!   surface), grouped by concern (config, providers, sessions,
//!   worktree, projects, cancel).
//! - [`agent`] — the chat command + spawned agent loop,
//!   `resolve_chat_provider` + `PreFlightError`, the system prompt
//!   builder, the thinking-block accumulator, and the helper
//!   utilities (tool result envelope, synthetic tool_result,
//!   emit helpers).
//!
//! The original god-module grew to 3195 lines because every new
//! feature accreted onto `lib.rs`. The audit task's goal is to
//! invert that: new features land in the module that owns their
//! concern, and `lib.rs` stays a 100-line-or-so bootstrap.
//!
//! `init_tracing` was extracted to `main.rs` (grill decision #4)
//! so the platform entry point owns the platform concerns
//! (Windows console subsystem on/off, env-filter defaults).

mod agent;
mod commands;
mod db;
mod git;
mod llm;
mod memory;
mod projects;
mod state;
mod tools;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(async move {
                std::sync::Arc::new(state::AppState::load(&app_handle).await)
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Agent loop (lives in `agent::chat` because it owns the
            // 20-turn agent loop, not a thin IPC shim).
            agent::chat::chat,
            // Cancel / config
            commands::cancel::cancel_chat,
            commands::config::get_llm_config,
            commands::config::get_home_dir,
            // Providers / models / default model
            commands::providers::list_providers,
            commands::providers::add_provider,
            commands::providers::update_provider,
            commands::providers::delete_provider,
            commands::providers::list_models,
            commands::providers::add_model,
            commands::providers::update_model,
            commands::providers::delete_model,
            commands::providers::get_default_model,
            commands::providers::set_default_model,
            commands::providers::update_session_model_id,
            commands::providers::test_provider,
            commands::providers::test_model,
            // Sessions
            commands::sessions::list_sessions,
            commands::sessions::create_session,
            commands::sessions::load_session,
            commands::sessions::delete_session,
            commands::sessions::diff_worktree,
            commands::sessions::rename_session,
            commands::sessions::set_session_color,
            // A2 + B7 (Permission system + per-session Mode, 2026-06-13)
            commands::permissions::set_session_mode,
            commands::permissions::permission_response,
            commands::permissions::grant_tool_permission,
            // C4 (Audit-log query UI, 2026-06-14) — read-side
            // command for the AuditLogModal. The write side (⑩
            // `tool_executed`) lands in the agent loop.
            commands::permissions::list_session_audit_events,
            // F5 (LLM Latency Tracking): per-message latency +
            // per-tool duration persistence. Called by the
            // frontend `streamController` on `done` / `tool:result`
            // events; the agent loop itself does not call them.
            // F5 follow-up: `update_message_latency` now also
            // carries the thinking-phase duration (4th bind,
            // `thinking_ms`); same command, same fire path,
            // same idempotency contract.
            commands::sessions::update_message_latency,
            commands::sessions::record_tool_duration,
            // Worktrees
            commands::worktree::attach_worktree,
            commands::worktree::detach_worktree,
            commands::worktree::delete_worktree,
            // Projects
            commands::projects::list_projects,
            commands::projects::list_hidden_projects,
            commands::projects::create_project,
            commands::projects::update_project_path,
            commands::projects::update_project_name,
            commands::projects::hide_project,
            commands::projects::unhide_project,
            commands::projects::pick_project_dir,
            // Memory (B5: user + project 2-layer loader)
            commands::memory::read_memory_layers,
            commands::memory::read_memory_content,
            commands::memory::open_memory_in_editor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}