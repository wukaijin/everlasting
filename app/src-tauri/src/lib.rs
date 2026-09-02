// clippy 1.96 的 `doc_lazy_continuation` / `doc_overindented_list_items`
// 对本 crate 里大量中/英文混排的 `///` 自然语言段落会误判成 markdown
// 列表续行(建议在纯文本续写处加缩进,会破坏语义)。这些 lint 不区分
// "列表项续行" 与 "普通段落换行",对本项目的文档注释基本是噪音 —— CI
// 也只跑 `cargo test --lib` + `cargo fmt`,不跑 clippy。crate 级 allow
// 抑制这两条,其余 clippy lint 仍保持默认 deny-on-check(见本地手动跑法)。
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]

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
mod attachments;
mod background_shell;
mod commands;
mod crypto;
mod db;
// B9+ D4 (2026-07-13): hand-written unified-diff parser + hunk
// applier. Zero new dependency (TECH §1.4). Lives at top level (not
// under `tools/`) because it's the structural backbone of the
// `apply_ui_diff` IPC, not a standalone tool — the IPC handler does
// the I/O, this module owns only the textual transformation.
mod diff_apply;
// Phase 2.2 (2026-07-21, task `07-20-remote-access-daemon-split`):
// HTTP daemon stack — `pub` so the `everlasting-daemon` bin target
// (a separate crate that depends on this library) can reach
// `everlasting_lib::daemon::serve_daemon`. Inside the daemon module,
// references to other top-level modules use `crate::xxx` paths, which
// work because the daemon module is itself part of the lib crate
// (the private `mod commands` etc. are visible to sibling modules
// inside the same crate, just not to external consumers).
pub mod daemon;
mod error;
mod files;
mod git;
mod llm;
mod memory;
mod projects;
mod resource_loader;
// P3b 执行期沙盒 (2026-08-31, task `08-31-a2-p3b-sandbox-executor`):
// Landlock + seccomp 执行器,ReadOnly 档 shell 命令的限损层。crate 私有:
// 消费者是 tools/shell.rs、tools/run_background_shell.rs (PR2) 与
// commands/config.rs (设置面读出口)。
mod sandbox;
// F2 定时任务 (2026-08-28, task `08-28-f2-scheduled-tasks`): 调度内核
// (preset 档位纯函数 + 30s tick 单一扫描算法 + TaskOrigin 来源标记)。
// crate 私有:唯一消费者是 daemon/server.rs 的 spawn_task_scheduler
// wrapper、agent 的 origin 载体链与本模块测试。
mod scheduler;
// P2.4 D2/D5 (2026-07-22, task `07-20-remote-access-daemon-split`):
// GUI-side daemon sidecar lifecycle. The Thin-client mode spawns the
// `everlasting-daemon` binary via `tauri-plugin-shell` and kills it on
// `RunEvent::Exit`. See the module docs for the Thin vs Full mode
// decision + `?transport=tauri` escape hatch.
mod sidecar;
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
        tracing::info!(total_destroyed, cleanup_days, "startup sweep: complete");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_os::init())
        // P2.4 D2: shell plugin for spawning the everlasting-daemon
        // sidecar (`app.shell().sidecar(...)` in `sidecar.rs`). The
        // scoped `shell:allow-execute` capability gates which binary +
        // args the spawn may use.
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let app_handle = app.handle().clone();

            // P2.4 D5: pick the GUI mode BEFORE touching AppState.
            // Thin (default) skips AppState::load entirely — no
            // SqlitePool is opened in the GUI process (the AC
            // "lsof -p <gui-pid> | grep sqlite" stays empty). Full
            // (`?transport=tauri` / `EVERLASTING_GUI_FULL_STATE=1`)
            // keeps the legacy in-process behavior as the escape hatch.
            let mode = sidecar::GuiMode::resolve(&app_handle);

            if mode == sidecar::GuiMode::Thin {
                // Thin client: resolve the same data dir the daemon
                // should use (matching P2.1 path-consistency invariant)
                // WITHOUT opening a pool. `app_data_dir` here is just
                // a PathBuf; the daemon opens the SQLite file, not us.
                let data_dir = app_handle
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|e| {
                        panic!("failed to resolve app_data_dir for sidecar: {e}")
                    });
                sidecar::spawn_and_manage(&app_handle, &data_dir);
                // NOTE: AppState is intentionally NOT loaded and NOT
                // managed. The 79 invoke_handler commands below are
                // still registered (so the Tauri capability schema
                // compiles + the escape `?transport=tauri` Full mode
                // works), but in Thin mode the frontend uses
                // httpTransport and never invokes them — so the
                // absent `Arc<AppState>` Tauri state never panics.
                return Ok(());
            }

            // ── Full mode (legacy pre-P2.4 path) ──────────────────
            // Clone BEFORE the `async move` below: the background-shell
            // emitter closure needs its own AppHandle.
            let shell_event_handle = app_handle.clone();
            let state = tauri::async_runtime::block_on(async move {
                std::sync::Arc::new(state::AppState::load(&app_handle).await)
            });
            app.manage(state.clone());

            // 2026-09-02 (task `09-02-chat-task-panel`): wire the
            // background-shell UI event emitter for Full mode. Payloads
            // are pre-serialized `serde_json::Value`s (Serialize is
            // satisfied), so `AppHandle::emit` just forwards them.
            // Failure only warns (best-effort, mirrors the AppHandleSink
            // precedent — a dropped panel event self-heals via the next
            // `list_background_shells` pull). Thin mode returned above
            // and never wires an emitter (the daemon process owns the
            // SSE path), so events are a natural no-op there.
            state.background_shells.set_event_emitter(
                std::sync::Arc::new(move |name, payload| {
                    use tauri::Emitter;
                    if let Err(e) = shell_event_handle.emit(name, payload) {
                        tracing::warn!(
                            error = %e,
                            event = name,
                            "background_shell event emit failed (non-fatal)"
                        );
                    }
                }),
            );
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
            commands::message_queue::list_queued_messages,
            commands::message_queue::remove_queued_message,
            commands::message_queue::recall_queued_message,
            commands::config::get_llm_config,
            commands::config::get_home_dir,
            // S2 remote tunnel 配置(2026-08-11, task `08-11-tunnel-client`):
            // 注册但不改 sidecar/双模式逻辑(P1-1 —— tunnel 只由 daemon bin
            // spawn;这些 command 在 Thin 模式不会被调用,Tauri Full 逃生通道
            // 下写入 DB + 通知空壳 manager,行为安全)。
            commands::config::get_remote_config,
            commands::config::set_remote_config,
            commands::config::set_tunnel_node_id,
            commands::config::set_tunnel_display_name,
            commands::config::get_tunnel_status,
            // F4 web_search 配置(2026-08-25, task 08-25-web-search-tool):
            // provider 三值 + tavily key 三态(Some(非空)/Some("")清除/
            // None 不动);明文 key 不出后端,GET 只回 masked。
            commands::config::get_web_search_config,
            commands::config::set_web_search_config,
            // F6 异步 agent 任务(2026-08-27):前端可读 app_config
            // 开关面(turn_complete_notify_enabled + F2 的
            // scheduled_tasks_enabled,additive)。
            commands::config::get_app_config,
            // Settings「通用」开关写入口(2026-08-29 settings-shell,
            // 白名单见 commands::config::SETTABLE_APP_FLAGS)。
            commands::config::set_app_config_flag,
            // P3b(2026-08-31,评审 W1):列表型 app_config 字段写通道
            // (sandbox_extra_writable,白名单见 SETTABLE_APP_LISTS)。
            commands::config::set_app_config_list,
            // F2 定时任务(2026-08-28, task `08-28-f2-scheduled-tasks`):
            // 管理面 CRUD 四件;调度循环只在 daemon bin 装配(GUI 零
            // timer),这些命令只读写 scheduled_tasks 表 + 校验。
            commands::scheduled_tasks::list_scheduled_tasks,
            commands::scheduled_tasks::create_scheduled_task,
            commands::scheduled_tasks::update_scheduled_task,
            commands::scheduled_tasks::delete_scheduled_task,
            // S2 配对码生成(经 tunnel WSS 调 remote 内部 RPC)
            commands::pairing::generate_pairing_code,
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
            commands::providers::test_model,
            // Sessions
            commands::attachments::save_attachment,
            commands::sessions::list_sessions,
            commands::sessions::create_session,
            commands::sessions::load_session,
            commands::sessions::update_session_metadata,
            commands::sessions::delete_session,
            commands::sessions::clear_session_messages,
            commands::sessions::diff_worktree,
            commands::sessions::rename_session,
            commands::sessions::set_session_color,
            // W1 (Workflow integration, Step 0.2 — 2026-07-08):
            // per-session workflow opt-in toggle. UI preference
            // (no audit row in Step 0.2); the audit-grade
            // `workflow_toggled` event lands with Phase 3's
            // state-transition hook (Step 3.1).
            commands::sessions::set_session_workflow_enabled,
            // W1 (Workflow integration, Step 2.2 — 2026-07-08):
            // per-session active plugin name. The frontend
            // `PluginSelect.vue` writes this; `build_workflow_ctx`
            // reads it on the next IPC entry to reload the
            // workflow JSON. `list_workflow_plugins` backs the
            // popover data source.
            commands::sessions::set_session_plugin_name,
            commands::sessions::list_workflow_plugins,
            // 08-10-group-chat-cache-rate: per-speaker (participants
            // + "moderator") latest-turn cache-usage read for the
            // GroupChatConfigModal edit view. Read-only derived
            // data (`turn_trace` ↔ `messages.speaker` join); the
            // modal swallows failures and renders "—".
            commands::sessions::group_chat_cache_rates,
            // D2 (cross-session search, 2026-08-17): user-facing
            // full-text search over all sessions (FTS5 + LIKE
            // fallback + title rider). Read-only.
            commands::sessions::search_messages,
            // Manual /compact (08-18-manual-compact-command): idle-time
            // LLM summary compaction for the current session. Shares the
            // C3+ auto path's pure functions; bypasses the 0.85 trigger
            // line but rejects in-flight turns.
            commands::sessions::compact_session,
            // /handoff (08-18-handoff-mechanism): full-coverage summary
            // becomes the first context of a NEW child session; parent
            // session rows untouched.
            commands::sessions::handoff_session,
            // C2 (review visualization view, 2026-07-26):
            // review-state.json read IPCs for the frontend
            // `<ReviewMatrix>` panel. `get_review_state`
            // returns a three-state payload
            // (State/Missing/Invalid); `get_current_task_slug`
            // reuses resolve_current_task. Read-only — refresh
            // is frontend-driven (streamController tool:call).
            commands::review::get_review_state,
            commands::review::get_current_task_slug,
            // W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
            // `create_task` — seed `.everlasting/tasks/<slug>/`
            // with v1 `task.json` + `prd.md` skeleton. Phase 0
            // ships this only; Phase 2 Step 2.6 adds
            // `update_task`; Phase 3 Step 3.3 adds
            // `archive_task`.
            commands::task::create_task,
            // W1 (Workflow integration, Phase 3 Step 3.3 — 2026-07-09):
            // `archive_task` — finalize a workflow task by
            // moving it under `.everlasting/tasks/archive/<YYYY-MM>/`
            // + setting `status = completed` + `completed_at`
            // + (default) `git add` + commit. Companion to
            // `create_task`; together they are the engine's
            // authoritative task-CRUD IPC pair (Step 2.6
            // owns `update_task` for B12 checklist sync).
            commands::task::archive_task,
            // A2 + B7 (Permission system + per-session Mode, 2026-06-13)
            commands::permissions::set_session_mode,
            commands::permissions::permission_response,
            commands::permissions::grant_tool_permission,
            // Permission-grant management UI (task 07-01):
            // list + per-PK revoke of "always allow" rows.
            commands::permissions::list_session_tool_permissions,
            commands::permissions::revoke_tool_permission,
            // C4 (Audit-log query UI, 2026-06-14) — read-side
            // command for the AuditLogModal. The write side (⑩
            // `tool_executed`) lands in the agent loop.
            commands::permissions::list_session_audit_events,
            // RULE-PERM-001 (2026-08-30): keyset-paginated sibling of
            // list_session_audit_events (AuditLogModal「加载更多」).
            // Purely additive — the full-pull command above stays for
            // traceStore (design D3 / R6).
            commands::permissions::list_session_audit_events_page,
            // E2 (harness trace pipeline, 2026-07-14): trace viewer
            // IPCs — list_turn_traces for 回看, clear_session_trace
            // for the manual cleanup button.
            commands::permissions::list_turn_traces,
            commands::permissions::clear_session_trace,
            // 08-20-worker-turn-trace-persist: per-run worker turn
            // rows for the SubagentDrawer "Token 明细" view (PR3
            // frontend; backend read path ships first).
            commands::permissions::list_worker_turn_traces,
            commands::usage::set_quota_settings,
            commands::usage::usage_window,
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
            // 2026-09-02 (task `09-02-chat-task-panel`): background-
            // shell UI observability IPCs for the ActivityPanel.
            commands::background_shells::list_background_shells,
            commands::background_shells::kill_background_shell,
            // L3b PR3 (2026-06-27): merge / discard worker IPCs.
            // The LLM-side path is the `merge_worker` /
            // `discard_worker` tools (tool layer); these commands
            // exist for the PR4 `<SubagentDrawer>` manual
            // merge / discard buttons.
            commands::subagent_runs::merge_worker_run,
            commands::subagent_runs::discard_worker_run,
            // 2026-07-03 (task 07-03-subagent-per-agent-model-ui,
            // 阶段 3): Settings-UI IPCs for per-subagent model
            // configuration (DB override + frontmatter write-back).
            commands::subagents::list_subagents_with_model,
            commands::subagents::set_subagent_model,
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
            commands::projects::update_project_sandbox_policy,
            commands::projects::hide_project,
            commands::projects::unhide_project,
            commands::projects::pick_project_dir,
            // 2026-09-02 目录浏览模态框:browser-mode 选项目目录的
            // 数据源(pick_project_dir 的 web degrade)。
            commands::projects::browse_dir,
            // Memory (B5: user + project 2-layer loader)
            commands::memory::read_memory_layers,
            commands::memory::read_memory_content,
            commands::memory::open_memory_in_editor,
            // P2 (2026-06-29): runtime autonomous-memory CRUD.
            commands::memory::list_autonomous_memories,
            commands::memory::delete_autonomous_memory,
            // 07-06 (am-observability-panel): management IPCs for
            // the runtime memory panel — state transitions + user edit.
            commands::memory::update_autonomous_memory_status,
            commands::memory::update_autonomous_memory,
            // B3 /command palette (2026-06-16)
            commands::command_palette::list_commands,
            commands::command_palette::get_command_body,
            // B4 skill stretches (2026-06-18): merged /-trigger panel
            commands::panel::list_panel_items,
            commands::panel::get_skill_body,
            // explicit-agent-dispatch (2026-06-30): @@-trigger agent panel
            commands::panel::list_subagents,
            // B2 @文件补全 (2026-06-17)
            commands::files::list_files,
            // B2 system-root @/ panel: literal `/` walk under SYSTEM_EXCLUDE.
            commands::files::list_files_at,
            // 2026-06-30 (`ask_user_question` task): tool
            // question IPCs (frontend `<AskUserQuestionCard>`
            // resolve + pending-state source-of-truth lookup).
            commands::question::resolve_tool_question,
            // 2026-07-07 (`request_mode_change` task): unified
            // mode-change IPCs (frontend `<RequestModeChangeCard>`
            // resolve + pending-interaction source-of-truth
            // lookup).
            commands::question::resolve_mode_change,
            commands::question::get_pending_interaction,
            // 2026-07-08 (`07-08-workflow-integration` Phase 3
            // Step 3.1): tool state-transition IPC. Frontend
            // reuses `get_pending_interaction` for the
            // unified session-switch source-of-truth lookup.
            commands::question::resolve_task_state_transition,
            // 2026-07-13 (B9+ D4): user-triggered diff apply
            // IPC. Sibling to `merge_worker_run` — NOT a tool,
            // NOT in `builtin_tools()`; lives outside the LLM
            // tool registry so `filter_tools_for_mode` doesn't
            // see it. Plan-mode users can still apply proposed
            // diffs. See `commands/ui.rs::apply_ui_diff` for
            // the boundary check + audit shape.
            commands::ui::apply_ui_diff,
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
                // P2.4 D2/D5: in Thin mode there is no managed
                // `Arc<AppState>` (the GUI never opened a DB pool),
                // so `app_handle.state::<Arc<AppState>>()` would
                // panic. We branch on the presence of the sidecar
                // handle instead — Thin mode kills the daemon
                // sidecar; Full mode kills the background shells.
                // Both are idempotent + best-effort.
                if app_handle
                    .try_state::<sidecar::SidecarHandle>()
                    .is_some()
                {
                    sidecar::kill_managed(app_handle);
                    return;
                }

                // Full mode (legacy): AppState + background_shells
                // are managed. The shell's process-group SIGKILL is
                // async (RULE-E-002), but the kill signals
                // themselves fire synchronously inside `kill_all` —
                // by the time `Exit` resolves, every spawned
                // `sh -c <command>` has already received its SIGKILL.
                // Any descendants (`&` / `nohup` / pipelines) are in
                // the same process group and are reaped along with
                // the direct child. No leak.
                let state = app_handle.state::<std::sync::Arc<state::AppState>>();
                let registry = state.background_shells.clone();
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
