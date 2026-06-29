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
mod background_shell;
mod commands;
mod crypto;
mod db;
mod files;
mod git;
mod llm;
mod memory;
mod projects;
mod resource_loader;
mod skill;
mod state;
mod tools;

use crate::background_shell::BackgroundShellRegistry;
use tauri::Manager;

/// L3b PR3 (2026-06-27): sweep helper called once at startup.
/// Walks every project in the DB and sweeps stale worker
/// worktrees. Best-effort: a project-row load failure is
/// logged + skipped; a per-project sweep failure is logged +
/// skipped. The function does not return any value — the
/// total count is emitted as a `tracing::info!` event at the
/// end.
async fn sweep_stale_workers(db: sqlx::SqlitePool, app_data_dir: std::path::PathBuf) {
    let projects = match crate::db::list_projects(&db, false).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "startup sweep: failed to list projects (non-fatal; skipping sweep)"
            );
            return;
        }
    };
    let cleanup_days = crate::git::worktree::resolve_cleanup_period_days(None);
    let mut total_destroyed = 0usize;
    for project in &projects {
        match crate::git::worktree::sweep_stale_worker_worktrees(
            &app_data_dir,
            &project.id,
            std::path::Path::new(&project.path),
            cleanup_days,
        ) {
            Ok(n) => {
                if n > 0 {
                    tracing::info!(
                        project_id = %project.id,
                        project_name = %project.name,
                        destroyed = n,
                        "startup sweep: destroyed stale worker worktrees"
                    );
                }
                total_destroyed += n;
            }
            Err(e) => {
                tracing::warn!(
                    project_id = %project.id,
                    project_name = %project.name,
                    error = %e,
                    "startup sweep: project sweep failed (non-fatal; continuing)"
                );
            }
        }
    }
    if total_destroyed > 0 {
        tracing::info!(
            total_destroyed,
            cleanup_days,
            "startup sweep: complete"
        );
    }
}

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
            app.manage(state.clone());

            // L3b PR3 (2026-06-27): one-time startup sweep of
            // stale worker worktrees. We iterate every project
            // and call `sweep_stale_worker_worktrees` for each;
            // the sweep destroys worker worktrees whose mtime
            // is older than `EVERLASTING_CLEANUP_PERIOD_DAYS`
            // (default 7 days) AND whose libgit2 lock is not
            // present (a locked worktree is an active worker;
            // skip it). Best-effort — failures are logged at
            // `warn!` and never abort the startup sequence.
            //
            // Runs as a one-shot background task (not awaited
            // from the setup closure) so the sweep doesn't
            // block the Tauri window's first paint. The
            // `state.db` pool is `Clone` (Arc-internal) so we
            // can move it into the spawn; the `app_data_dir`
            // is the same path the AppState already computed.
            let sweep_db = state.db.clone();
            let sweep_data_dir = state.app_data_dir.clone();
            tauri::async_runtime::spawn(async move {
                sweep_stale_workers(sweep_db, sweep_data_dir).await;
            });

            // P5 (2026-06-29, 06-29-am-p5-quality): one-time startup
            // hygiene pass over the autonomous-memory library —
            // dedup-merge high-Jaccard pairs + age-out stale low-hit
            // rows that accumulated while the app was closed
            // (design D4 / §6). Fire-and-forget, best-effort: every
            // error is `warn!`-logged inside `run_hygiene_pass`, never
            // aborts startup. The event trigger in `insert_memory`
            // covers steady-state; this startup pass covers "user wrote
            // 200 rows then quit before the 10th-of-each-bucket tick".
            let hygiene_db = state.db.clone();
            tauri::async_runtime::spawn(async move {
                crate::agent::memory_hygiene::run_hygiene_pass(hygiene_db).await;
            });

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
            commands::sessions::clear_session_messages,
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
            // D3 PR1 (2026-06-17): edit a user message in place +
            // cascade-delete tail + audit. UI / Resend are PR2/3
            // (the frontend re-uses `chat` IPC for the resend).
            commands::sessions::edit_user_message,
            // B6 PR3a (2026-06-20): subagent_runs list/get IPCs for
            // the PR3 frontend `<SubagentDrawer>`. `list_*` returns
            // a `SubagentRunSummary` list (no transcript); `get_*`
            // returns the full `SubagentRunRow` (with transcript).
            commands::subagent_runs::list_subagent_runs_by_session,
            commands::subagent_runs::get_subagent_run,
            // L3b PR3 (2026-06-27): merge / discard worker IPCs.
            // The LLM-side path is the `merge_worker` /
            // `discard_worker` tools (tool layer); these commands
            // exist for the PR4 `<SubagentDrawer>` manual
            // merge / discard buttons.
            commands::subagent_runs::merge_worker_run,
            commands::subagent_runs::discard_worker_run,
            // Worktrees
            commands::worktree::attach_worktree,
            commands::worktree::detach_worktree,
            commands::worktree::delete_worktree,
            commands::worktree::publish_session_to_main,
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
            // P2 (2026-06-29): runtime autonomous-memory CRUD.
            commands::memory::list_autonomous_memories,
            commands::memory::delete_autonomous_memory,
            // B3 /command palette (2026-06-16)
            commands::command_palette::list_commands,
            commands::command_palette::get_command_body,
            // B4 skill stretches (2026-06-18): merged /-trigger panel
            commands::panel::list_panel_items,
            commands::panel::get_skill_body,
            // B2 @文件补全 (2026-06-17)
            commands::files::list_files,
            // B2 system-root @/ panel: literal `/` walk under SYSTEM_EXCLUDE.
            commands::files::list_files_at,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // L1a (2026-06-19): kill every background shell on app
        // shutdown. The shell's process-group SIGKILL is async
        // (RULE-E-002), but the kill signals themselves fire
        // synchronously inside `kill_all` — by the time `Exit`
        // resolves, every spawned `sh -c <command>` has already
        // received its SIGKILL. Any descendants (`&` / `nohup` /
        // pipelines) are in the same process group and are reaped
        // along with the direct child. No leak.
        //
        // We use `Exit` (not `ExitRequested`) because
        // `ExitRequested` is fired DURING the close handshake and
        // can be denied by a hook; `Exit` is the terminal
        // "the app is going down now" signal — there's no hook
        // between us and process termination, so cleanup is
        // unconditional.
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<std::sync::Arc<state::AppState>>();
                let registry = state.background_shells.clone();
                // `block_on` is appropriate here: the app is
                // exiting and we need the kill signals to land
                // BEFORE the OS starts reaping our process. The
                // registry's `kill_all` takes the lock once,
                // snapshots the senders, and sends — typically
                // sub-millisecond. Any teardown race with the
                // spawned background tasks is irrelevant: they're
                // being killed anyway, and the OS will reap the
                // descendants.
                tauri::async_runtime::block_on(async move {
                    if let Err(e) = registry.kill_all().await {
                        tracing::warn!(
                            error = %e,
                            "lifecycle hook: background_shells.kill_all failed on app exit (non-fatal)"
                        );
                    }
                });
            }
        });
}