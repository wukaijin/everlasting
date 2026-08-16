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

pub mod attachments;
pub mod cancel;
pub mod command_palette;
pub mod config;
pub mod files;
pub mod memory;
pub mod panel;
// S2 (2026-08-11, task `08-11-tunnel-client`):配对码生成 IPC —— 经 tunnel
// WSS 调 remote 的 `/internal/pairing/generate` 内部 RPC(design §2.5)。
pub mod pairing;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod question;
pub mod sessions;
pub mod subagent_runs;
pub mod subagents;
// C2 (review visualization view, 2026-07-26): review-state.json
// read IPCs for the frontend `<ReviewMatrix>` panel.
// `get_review_state` returns a three-state payload
// (State/Missing/Invalid); `get_current_task_slug` reuses the
// engine's resolve_current_task. Both are read-only — refresh
// is frontend-driven via streamController's tool:call route
// (no backend event). See commands/review.rs.
pub mod review;
// W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
// task IPC surface. Phase 0 ships `create_task` only;
// Phase 2 Step 2.6 adds `update_task` (B12 checklist
// sync); Phase 3 Step 3.3 adds `archive_task`.
pub mod task;
// B9+ D4 (2026-07-13): user-triggered IPC surface for
// the generative-UI flow. `apply_ui_diff` is the human-in-
// the-loop "apply proposed diff to disk" command; NOT
// registered as an LLM tool (no `builtin_tools()` entry)
// so `filter_tools_for_mode` doesn't see it — Plan mode
// users can still apply diffs the LLM proposes.
pub mod ui;
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
        // W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
        // `create_task` — seed `.everlasting/tasks/<slug>/`
        // with v1 `task.json` + `prd.md` skeleton. Phase 3
        // Step 3.3 adds the `archive_task` companion
        // (move to `.everlasting/tasks/archive/<YYYY-MM>/`
        // + `status=completed` + `completed_at` + (default)
        // `git add` + commit). The two IPCs are the
        // engine's authoritative task-CRUD pair; Step 2.6
        // owns `update_task` for B12 checklist sync.
        "create_task",
        "archive_task",
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
        // explicit-agent-dispatch (2026-06-30): @@-trigger agent panel
        "list_subagents",
        "get_skill_body",
        // B2 @文件补全
        "list_files",
        // A2 + B7 (Permission system + per-session Mode)
        "set_session_mode",
        "permission_response",
        "grant_tool_permission",
        // C4 (Audit-log query UI, 2026-06-14)
        "list_session_audit_events",
        // E2 (harness trace pipeline, 2026-07-14): trace viewer IPCs.
        "list_turn_traces",
        "clear_session_trace",
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
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 3):
        // Settings-UI IPCs for per-subagent model configuration.
        // `list_subagents_with_model` extends the existing
        // `list_subagents` panel IPC with the resolved model
        // (DB override > frontmatter > inherit); `set_subagent_model`
        // dispatches by source to the DB table (builtin) or the
        // agent's frontmatter file (user / project).
        "list_subagents_with_model",
        "set_subagent_model",
        // 2026-06-30 (`ask_user_question` task): tool
        // question IPCs (frontend `/<AskUserQuestionCard>`
        // submit/跳过 + session-switch source-of-truth lookup).
        "resolve_tool_question",
        "get_pending_question",
        // 2026-07-07 (`request_mode_change` task): tool
        // mode-change IPCs (frontend `<RequestModeChangeCard>`
        // allow/拒绝 + session-switch source-of-truth lookup
        // via the unified `PendingInteraction` enum).
        "resolve_mode_change",
        "get_pending_interaction",
        // 2026-07-08 (`07-08-workflow-integration` Phase 3
        // Step 3.1): tool state-transition IPC (frontend
        // `<RequestTaskStateTransitionCard>` allow/拒绝).
        // The frontend reuses `get_pending_interaction` for
        // the unified session-switch source-of-truth lookup
        // — only the new resolve IPC is unique to this kind.
        "resolve_task_state_transition",
        // 2026-07-13 (B9+ D4): user-triggered diff apply IPC.
        // Sibling to `merge_worker_run` — NOT a tool, NOT in
        // `builtin_tools()`. Frontend `<DiffPrimitive>` /
        // `<ButtonPrimitive>` (apply_diff action) invoke
        // this on user click. Returns
        // `{ok, files?, kind?, error?}` — `kind` ∈
        // {"boundary", "parse", "conflict", "io", "empty"}
        // drives the inline error UX.
        "apply_ui_diff",
    ]
}
