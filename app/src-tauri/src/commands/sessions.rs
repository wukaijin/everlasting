//! Session-related Tauri commands.
//!
//! - [`list_sessions`] / [`create_session`] / [`load_session`] /
//!   [`delete_session`] — session CRUD on top of `db::*`.
//! - [`diff_worktree`] — read the session's worktree diff via
//!   [`crate::git::diff`].
//!
//! The worktree lifecycle (attach / detach / delete) is in
//! [`crate::commands::worktree`]; the destructive cancel hook
//! shared with them lives in [`crate::agent::helpers`].

use std::sync::Arc;

use tauri::State;

use crate::agent::helpers::{await_inflight_exit, cancel_inflight_for_session};
use crate::background_shell::BackgroundShellRegistry;
use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::git;
use crate::llm::types::MessageContent;
use crate::state::AppState;

pub async fn list_sessions_inner(
    state: &Arc<AppState>,
    project_id: String,
) -> Result<Vec<db::SessionSummary>, AppCommandError> {
    let mut sessions =
        db::list_sessions(&state.db, &project_id)
            .await
            .map_err(|e| -> AppCommandError {
                anyhow::anyhow!("list_sessions failed: {}", e).into()
            })?;
    // F6 (2026-08-27): `busy` is runtime state — the DB layer hardcodes
    // false and this is the single enrichment point (both transports hit
    // this fn, Q0 single source of truth). Semantics: busy = accepted
    // in-flight, INCLUDING loops still waiting on the global loop
    // semaphore permit (claim registers before spawn acquires).
    if !sessions.is_empty() {
        let active = state.session_active_request.lock().await;
        for s in sessions.iter_mut() {
            s.busy = active.contains_key(&s.id);
        }
    }
    Ok(sessions)
}

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<db::SessionSummary>, AppCommandError> {
    list_sessions_inner(&state, project_id).await
}

pub async fn create_session_inner(
    state: &Arc<AppState>,
    project_id: String,
    initial_cwd: String,
    model: Option<String>,
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E2):
    // optional session type discriminator. `None` / `Some("chat")`
    // for classic chat (default; matches the column DEFAULT 'chat').
    // `Some("group_chat")` for the multi-LLM session type. Threaded
    // straight into `db::create_session`'s INSERT.
    session_type: Option<String>,
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E2):
    // optional per-session JSON metadata blob (e.g. the
    // `GroupChatConfig { participants: [...] }` for group-chat
    // sessions). `None` for classic chat. Stored verbatim in
    // `sessions.metadata` (TEXT column, JSON string).
    metadata: Option<serde_json::Value>,
) -> Result<db::SessionRow, AppCommandError> {
    // `model` defaults to an empty string when the caller doesn't
    // pass one. The frontend's `create_session` call never sends a
    // model, and the actual provider/model used for chat is resolved
    // per-turn from the DB catalog (`app_config.default_model_id`),
    // not from this column. An empty string just means "no legacy
    // model label recorded for this session".
    let model = model.unwrap_or_default();
    // Group chat (Phase 4 Step 3): serialize the metadata JSON
    // for the SQL bind. `None` passes through to `NULL` in the
    // column. Errors only surface if the input is malformed
    // (group-chat config is built client-side and validated
    // before IPC, so this is defense-in-depth).
    let metadata_str = match metadata.as_ref() {
        Some(v) => Some(serde_json::to_string(v).map_err(|e| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("create_session: metadata serialization failed: {}", e),
            )
        })?),
        None => None,
    };
    // Defensive: every session is bound to a project. The frontend
    // is expected to gate this with a "no project = no chat" check,
    // but a stray IPC call should not silently create a
    // legacy-bound session.
    if project_id.trim().is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "create_session: project_id must not be empty",
        ));
    }

    // Step 4 follow-up: worktree is now opt-in. We no longer
    // require the project to be a git repo (that was the step 4
    // v1 hard guard) and we no longer auto-create a worktree. The
    // session is created in `WorktreeState::None`; the user calls
    // `attach_worktree` separately if they want isolation. Non-git
    // projects can now create sessions and send messages.
    let _project = match db::get_project(&state.db, &project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("create_session: project '{}' not found", project_id),
            ));
        }
        Err(e) => {
            return Err(anyhow::anyhow!("create_session: failed to load project: {}", e).into())
        }
    };

    let session_id = uuid::Uuid::new_v4().to_string();

    // Read the current default model_id so the session is bound to
    // a specific model at creation time (not just a free-text name).
    let model_id = db::get_config_value(&state.db, "default_model_id")
        .await
        .ok()
        .flatten();

    db::create_session(
        &state.db,
        &session_id,
        &project_id,
        &initial_cwd,
        &model,
        model_id.as_deref(),
        session_type.as_deref(),
        metadata_str.as_deref(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_session: db insert failed: {}", e).into())
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    initial_cwd: String,
    model: Option<String>,
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E2):
    // optional session type + metadata. Both null in the
    // classic-chat path (the existing IPC contract for the 1
    // production caller — `chat.ts::createNewSession`).
    session_type: Option<String>,
    metadata: Option<serde_json::Value>,
) -> Result<db::SessionRow, AppCommandError> {
    create_session_inner(
        &state,
        project_id,
        initial_cwd,
        model,
        session_type,
        metadata,
    )
    .await
}

pub async fn load_session_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Option<db::LoadedSession>, AppCommandError> {
    db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("load_session failed: {}", e).into())
}

#[tauri::command]
pub async fn load_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<db::LoadedSession>, AppCommandError> {
    load_session_inner(&state, session_id).await
}

// Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E3/E5):
// per-session metadata overwrite. Currently the only primary
// consumer is the `GroupChatConfigModal` re-edit flow (which
// writes the participant roster as a JSON blob into
// `sessions.metadata`). The backend is intentionally
// type-agnostic — it accepts any JSON value and writes it
// verbatim → the `GroupChatConfig` parser is the source of
// truth for the shape on the frontend side. Returns
// `Ok(())` on success; the IPC layer propagates errors.
pub async fn update_session_metadata_inner(
    state: &Arc<AppState>,
    session_id: String,
    metadata: serde_json::Value,
) -> Result<(), AppCommandError> {
    // Defensive: refuse empty session_id (would silently
    // update zero rows). The frontend's modal flow never
    // sends empty, but a stray IPC call should not silently
    // succeed.
    if session_id.trim().is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "update_session_metadata: session_id must not be empty",
        ));
    }
    let metadata_str = serde_json::to_string(&metadata).map_err(|e| {
        AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "update_session_metadata: metadata serialization failed: {}",
                e
            ),
        )
    })?;
    // 0 rows = no such session (FK / session_id not found).
    // We surface this as InvalidRequest so the frontend can
    // decide whether to retry / drop the modal.
    let rows = sqlx::query("UPDATE sessions SET metadata = ?2 WHERE id = ?1")
        .bind(&session_id)
        .bind(&metadata_str)
        .execute(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("update_session_metadata: db update failed: {}", e))?
        .rows_affected();
    if rows == 0 {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "update_session_metadata: session '{}' not found",
                session_id
            ),
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn update_session_metadata(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    metadata: serde_json::Value,
) -> Result<(), AppCommandError> {
    update_session_metadata_inner(&state, session_id, metadata).await
}

pub async fn diff_worktree_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<git::diff::DiffResult, AppCommandError> {
    // Look up the session to find its worktree. Pre-step-4
    // sessions (worktree_path NULL) have no diff to show —
    // return an empty result rather than an error so the UI can
    // render "no changes yet" gracefully.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("diff_worktree: failed to load session: {}", e))?
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("diff_worktree: session '{}' not found", session_id),
            )
        })?;

    let worktree_path = match loaded.session.worktree_path.as_deref() {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            // Pre-step-4 session: no worktree, no diff.
            tracing::debug!(
                session_id = %session_id,
                "diff_worktree: pre-step-4 session, no worktree, returning empty"
            );
            return Ok(git::diff::DiffResult { files: vec![] });
        }
    };

    git::diff::diff_worktree(std::path::Path::new(worktree_path), &session_id)
        .map_err(|e| anyhow::anyhow!("diff_worktree: {}", e).into())
}

#[tauri::command]
pub async fn diff_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<git::diff::DiffResult, AppCommandError> {
    diff_worktree_inner(&state, session_id).await
}

pub async fn delete_session_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<(), AppCommandError> {
    // Step 4 follow-up: in-flight cancel hook. If a chat stream
    // is running for this session, cancel it BEFORE the
    // destructive work. The frontend is expected to disable the
    // delete button while streaming (REQ-13) and to call
    // `cancel_chat` first, but the backend is the last line of
    // defense.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    // F1(2026-08-25):队列随 session 删除(design §4 取消矩阵)。
    crate::agent::message_queue::clear_session(&state.message_queues, &session_id).await;
    // RULE-E-005 (2026-06-15): wait for the agent loop to exit
    // before deleting DB rows. Without this, an in-flight
    // `persist_turn` after deletion writes to a session that no
    // longer exists (orphan rows / FK violation / blank reload).
    await_inflight_exit(exit_rx, "delete_session").await;

    // RULE-B-001 (2026-06-16): drop any pending `permission:ask`
    // oneshot senders for this session. With the agent loop
    // already exited this mostly clears residual entries (its
    // CancellationToken already raced the ask future to Deny),
    // but wiring it explicitly removes the latent dependency on
    // the biased select! and stops the store leaking entries
    // across session churn. cancel_session_asks filters by
    // session_id (RULE-B-002), so other sessions' pending asks
    // are untouched.
    crate::agent::permissions::cancel_session_asks(&state.permission_asks, &session_id).await;

    // Clear the in-memory ReadGuard for this session so we don't
    // leak fingerprints for a session the user just deleted.
    state.read_guard.clear_session(&session_id).await;

    // Load the session row BEFORE the destructive work so the
    // cwd / worktree cleanup below knows what to tear down.
    // (The memory cache needs no explicit invalidation: the
    // mtime fence in `load_for_session` re-reads on the next
    // access, and deleting a session does not touch the
    // project's memory files anyway.)
    let session_for_cleanup = db::load_session(&state.db, &session_id)
        .await
        .ok()
        .flatten();

    if let Some(ref loaded) = session_for_cleanup {
        let cwd = &loaded.session.current_cwd;
        if !cwd.trim().is_empty() {
            crate::tools::shell::cleanup_outputs_dir(std::path::Path::new(cwd)).await;
        }
    }

    // L1a (2026-06-19): kill every background shell belonging to
    // this chat session. Fire-and-forget — `kill_all_for_session`
    // sends kill signals to the running senders and returns
    // immediately; the spawned tasks do the actual teardown async
    // (process-group SIGKILL + reaping). Not awaiting the teardown
    // here is intentional: the delete IPC should not block on
    // process cleanup, and a brief window of dangling process
    // group is preferable to a hung delete button. The cancel +
    // ReadGuard + permission-asks cleanup above already took the
    // in-flight agent loop out of the equation.
    if let Err(e) = state
        .background_shells
        .kill_all_for_session(&session_id)
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            error = %e,
            "delete_session: background_shells.kill_all_for_session failed (non-fatal)"
        );
    }

    // D (2026-08-14, `08-14-c7d-tools-stub-registration`): clear the
    // session's stub loaded-set(渐进式披露 D 的粘性 registry 清理)。
    // 对齐 `kill_all_for_session` 接线点 — 删除 session 后其 loaded-set
    // 必须清空,否则 session_id 复用(新建同名/复用 UUID)会拿到残留的
    // 「已 load」状态。纯内存操作,同步完成。
    state.stub_loaded.clear(&session_id).await;

    // memory-block-governance WP2 (2026-08-15): clear the session's
    // memory digest loaded-set(粘性 registry 清理,同上 — session_id
    // 复用不得拿到残留的「已加载节」状态)。进程级 OnceLock 单例,不
    // 经 AppState(72 个 run_chat_loop 调用点零穿参,见 digest.rs 模块
    // 注释);本 inner 函数是 Tauri command 与 daemon route 的共用路径,
    // 一处清理两边生效。
    crate::memory::digest::registry().clear(&session_id).await;

    // C3 摘要压缩 PR2 (08-18-llm-context-compaction): clear the
    // session's compaction breaker count(熔断 registry 清理,同上 ——
    // session_id 复用不得继承旧 session 的连续失败计数,否则新 session
    // 第一条消息就被跳过摘要直达机械)。进程级 OnceLock 单例,不经
    // AppState(选型同 digest:run_chat_loop 签名硬约束),本 inner
    // 函数一处清理两边生效。
    crate::agent::compaction::compaction_registry()
        .clear(&session_id)
        .await;

    // B1 (2026-08-16): delete the session's image-attachments
    // directory (best-effort, same wiring point as the stub/digest
    // registry cleanup above — a deleted session leaves no orphan
    // image files behind).
    crate::attachments::delete_session_attachments(&state.app_data_dir, &session_id);

    // Step 4 follow-up: best-effort worktree + branch cleanup.
    // Triggered when the session's `worktree_state` is `active`
    // (NOT `detached` — a detached session's worktree was already
    // removed; deleting a detached session should NOT touch the
    // on-disk artifacts).
    if let Some(ref loaded) = session_for_cleanup {
        if loaded.session.worktree_state == db::WorktreeState::Active {
            if let Some(wt_path) = loaded.session.worktree_path.as_deref() {
                if let Ok(Some(project)) =
                    db::get_project(&state.db, &loaded.session.project_id).await
                {
                    if let Err(e) = git::destroy_worktree(
                        std::path::Path::new(&project.path),
                        std::path::Path::new(wt_path),
                        &session_id,
                    ) {
                        tracing::warn!(
                            session_id = %session_id,
                            worktree = %wt_path,
                            error = %e,
                            "worktree cleanup failed during session delete (non-fatal)"
                        );
                    }
                }
            }
        }
    }

    db::delete_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_session failed: {}", e).into())
}

#[tauri::command]
pub async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), AppCommandError> {
    delete_session_inner(&state, session_id).await
}

/// B3 `/clear`: clear the current session's messages but keep the
/// session row (title/color/mode/model/project/timestamps).
///
/// Mirrors the in-flight cleanup `delete_session` does (cancel any
/// running chat, drop pending permission asks, clear the ReadGuard)
/// so a cleared session starts from a clean runtime slate — but does
/// NOT tear down the worktree or delete the session row. Audit events
/// are kept (they record agent actions, not the live buffer).
pub async fn clear_session_messages_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<(), AppCommandError> {
    // Cancel any in-flight chat first (the backend is the last line
    // of defense — the frontend disables the trigger while
    // streaming). Wait for the loop to exit so a late `persist_turn`
    // can't re-write messages we just cleared.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    await_inflight_exit(exit_rx, "clear_session_messages").await;
    // F1(2026-08-25):清空消息即清空排队(design §4)。
    crate::agent::message_queue::clear_session(&state.message_queues, &session_id).await;

    // Drop pending permission asks + read fingerprints so the fresh
    // conversation starts clean.
    crate::agent::permissions::cancel_session_asks(&state.permission_asks, &session_id).await;
    state.read_guard.clear_session(&session_id).await;

    // Delete messages only; the session row + audit log survive.
    db::delete_messages_by_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("clear_session_messages failed: {}", e).into())
}

#[tauri::command]
pub async fn clear_session_messages(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<(), AppCommandError> {
    clear_session_messages_inner(&state, session_id).await
}

pub async fn rename_session_inner(
    state: &Arc<AppState>,
    session_id: String,
    new_title: String,
) -> Result<(), AppCommandError> {
    if new_title.trim().is_empty() {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "rename_session: title must not be empty",
        ));
    }
    db::rename_session(&state.db, &session_id, &new_title)
        .await
        .map_err(|e| anyhow::anyhow!("rename_session failed: {}", e).into())
}

#[tauri::command]
pub async fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    new_title: String,
) -> Result<(), AppCommandError> {
    rename_session_inner(&state, session_id, new_title).await
}

pub async fn set_session_color_inner(
    state: &Arc<AppState>,
    session_id: String,
    color_tag: Option<i32>,
) -> Result<(), AppCommandError> {
    db::set_session_color(&state.db, &session_id, color_tag)
        .await
        .map_err(|e| anyhow::anyhow!("set_session_color failed: {}", e).into())
}

#[tauri::command]
pub async fn set_session_color(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    color_tag: Option<i32>,
) -> Result<(), AppCommandError> {
    set_session_color_inner(&state, session_id, color_tag).await
}

/// W1 (Workflow integration, Step 0.2 — 2026-07-08):
/// per-session workflow opt-in toggle. Mirrors
/// `set_session_color`'s contract — a plain column flip,
/// no audit row. Used by the frontend's `WorkflowToggle.vue`
/// (mounted in `ChatInput.vue` next to `ModeSelect`).
///
/// **State change semantics (READ THIS BEFORE TOUCHING):**
/// the toggle takes effect IMMEDIATELY for the next agent
/// turn boundary, matching `set_session_mode`'s "applies on
/// the next turn" contract (see `agent/chat_loop.rs:396`).
/// Mid-stream flips do NOT abort the in-flight loop — the
/// breadcrumb + state-machine injection (Phase 0 Step 0.5
/// + Phase 2 Step 2.5) only kicks in for subsequent turns.
///
/// **No Yolo / root guard**: workflow toggling is a UI
/// preference, not a privileged operation. Unlike
/// `set_session_mode` we do NOT consult `is_running_as_root`
/// here.
///
/// **Audit deferred to Phase 3**: the `workflow_toggled`
/// audit row lands with the state-transition hooks in Step
/// 3.1; this command is intentionally audit-free in Step
/// 0.2 to keep the toggle synchronous + light. The
/// frontend's `WorkflowToggle.vue` optimistically updates
/// its local `SessionSummary` before awaiting the IPC so
/// the user sees the chip flip instantly even when the
/// backend round-trip is slow.
pub async fn set_session_workflow_enabled_inner(
    state: &Arc<AppState>,
    session_id: String,
    enabled: bool,
) -> Result<(), AppCommandError> {
    db::set_session_workflow_enabled(&state.db, &session_id, enabled)
        .await
        .map_err(|e| anyhow::anyhow!("set_session_workflow_enabled failed: {}", e).into())
}

#[tauri::command]
pub async fn set_session_workflow_enabled(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    enabled: bool,
) -> Result<(), AppCommandError> {
    set_session_workflow_enabled_inner(&state, session_id, enabled).await
}

/// W1 (Workflow integration, Step 2.2 — 2026-07-08):
/// per-session workflow plugin name. Frontend
/// `PluginSelect.vue` fires this on click; the engine's
/// `build_workflow_ctx` reads the persisted name on the
/// next IPC entry to call `load_workflow(name, project_path)`.
///
/// **Empty-string guard**: an empty `name` is rejected
/// because the loader (`load_workflow`) treats `Some("")`
/// and `None` identically (no plugin layer consulted), which
/// would silently turn a workflow session into a
/// non-workflow one in the breadcrumb injection — surprising
/// behavior. Better to surface the invalid input.
///
/// **No disk-existence check**: the loader's
/// `load_workflow` falls back to `default_workflow()` on
/// missing-file, so a stale name never breaks the engine.
/// We don't pre-validate the directory here — the
/// `list_workflow_plugins` IPC is the canonical source of
/// available names; the chip's popover only offers those.
pub async fn set_session_plugin_name_inner(
    state: &Arc<AppState>,
    session_id: String,
    name: String,
) -> Result<(), AppCommandError> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err(
            anyhow::anyhow!("set_session_plugin_name: plugin name must be non-empty").into(),
        );
    }

    // C5 (2026-07-28): when the session switches plugins mid-task,
    // re-point the active task's `workflow_plugin` to the new plugin
    // and remap its `status` to the new plugin's `initial`. Without
    // this, the role gate / transition (which read task.workflow_plugin)
    // would keep using the old plugin's state machine, but the LLM's
    // tools/breadcrumb would reflect the new plugin — a mismatch that
    // dead-locks cross-plugin flows (dev→review→dev in one session).
    //
    // The remap is lossy at the status level (review's `reviewing`
    // → dev's `planning`), but the prd.md is the single source of
    // truth and survives the switch, so the dev→review→dev round-trip
    // preserves all review findings.
    remap_task_plugin_on_switch(&state.db, &session_id, &trimmed).await;

    db::set_session_plugin_name(&state.db, &session_id, &trimmed)
        .await
        .map_err(|e| anyhow::anyhow!("set_session_plugin_name failed: {}", e).into())
}

/// C5: if the session has an active task whose `workflow_plugin`
/// differs from the new session plugin, rewrite the task's
/// `workflow_plugin` + `status` (to the new plugin's `initial`).
/// Best-effort — failures log a warning but do not block the plugin
/// switch (the session plugin_name DB write is the source of truth
/// for the session; the task remap is a consistency optimization).
async fn remap_task_plugin_on_switch(db: &sqlx::SqlitePool, session_id: &str, new_plugin: &str) {
    use crate::agent::workflow::{load_workflow, resolve_current_task, write_task, TaskStatus};

    let loaded = match crate::db::load_session(db, session_id).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            tracing::warn!(
                session_id = session_id,
                "set_session_plugin_name: session not found; skipping task remap",
            );
            return;
        }
        Err(e) => {
            tracing::warn!(
                session_id = session_id,
                error = %e,
                "set_session_plugin_name: load_session failed; skipping task remap",
            );
            return;
        }
    };

    let project = match crate::db::get_project(db, &loaded.session.project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return, // no project → no task to remap
        Err(e) => {
            tracing::warn!(error = %e, "set_session_plugin_name: get_project failed; skipping");
            return;
        }
    };

    let project_path = std::path::PathBuf::from(&project.path);
    let mut task = match resolve_current_task(&project_path).await {
        Some(t) => t,
        None => return, // no active task → nothing to remap
    };

    if task.workflow_plugin == new_plugin {
        return; // same plugin → no remap needed
    }

    let new_initial = load_workflow(new_plugin, &project.path).initial;
    let old_plugin = task.workflow_plugin.clone();
    let old_status = task.status.as_str().to_string();
    task.workflow_plugin = new_plugin.to_string();
    task.status = TaskStatus::from_str_opt(&new_initial);

    if let Err(e) = write_task(&project_path, &task) {
        tracing::warn!(
            slug = %task.slug,
            error = %e,
            "set_session_plugin_name: failed to remap task; session switched but task keeps old plugin",
        );
        return;
    }
    tracing::info!(
        slug = %task.slug,
        old_plugin = %old_plugin,
        new_plugin = %new_plugin,
        old_status = %old_status,
        new_status = %new_initial,
        "set_session_plugin_name: remapped active task to new plugin",
    );
}

#[tauri::command]
pub async fn set_session_plugin_name(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    name: String,
) -> Result<(), AppCommandError> {
    set_session_plugin_name_inner(&state, session_id, name).await
}

/// W1 (Workflow integration, Step 2.2 — 2026-07-08):
/// discover available workflow plugins under
/// `<project>/.everlasting/workflow/<dir>/workflow.json`.
/// Returns the list of valid plugin names (alphabetical) so
/// the frontend's `PluginSelect.vue` popover can populate
/// itself without hard-coding "dev".
///
/// **Discovery rule**: a directory is a valid plugin iff
/// `workflow.json` exists inside it. Empty directories
/// are ignored (no warning — they're typical scratch
/// state). Missing root dir → empty list (matches the
/// `load_workflow` not-found contract: no plugins = just
/// the default dev workflow).
///
/// Returns `Vec<String>` (just names, not full
/// `WorkflowDef`s — `PluginSelect` only needs the
/// identifier; `load_workflow` does the heavy lifting on
/// the engine side).
#[tauri::command]
pub async fn list_workflow_plugins(project_path: String) -> Result<Vec<String>, AppCommandError> {
    Ok(crate::agent::workflow::list_plugins(&project_path))
}

// ---------------------------------------------------------------------------
// F5 (LLM Latency Tracking): per-message latency + per-tool duration IPCs
//
// Two new commands; both are write-only and fire-and-forget from the
// frontend's `streamController` (the agent loop does not call them).
// The IPC layer's `serde(rename_all)` mirrors the TypeScript payload
// names — see `app/src/stores/streamController.ts` for the consumer.
// ---------------------------------------------------------------------------

/// Update the latency + thinking-time columns on an
/// assistant message row (TTFB / gen / total in
/// milliseconds, plus `thinking_ms` — the F5 follow-up
/// thinking-phase wall-clock). The frontend measures
/// the four values via `Date.now()` deltas around the
/// `start` / first `delta` / `done` events of one chat
/// invocation (and the `thinking_delta` ↔ boundary
/// events for `thinking_ms`), then issues this IPC at
/// `done`.
///
/// The controller tracks the assistant message by its
/// caller-managed `seq` (the same handle it shares with the
/// agent loop), so the IPC takes `(session_id, seq)` and the
/// backend resolves the SQLite row id internally via
/// `find_message_id_by_seq`. Each of the four millisecond
/// values is optional so a cancel / error path can pass
/// `None` for the sub-components (`ttfbMs` / `genMs` /
/// `thinkingMs`) and still record the total
/// time-to-cancel. `thinkingMs` is `None` for messages
/// that never entered the thinking phase — the frontend
/// just doesn't include it in the payload in that case.
pub async fn update_message_latency_inner(
    state: &Arc<AppState>,
    session_id: String,
    seq: i64,
    ttfb_ms: Option<i64>,
    gen_ms: Option<i64>,
    total_ms: Option<i64>,
    thinking_ms: Option<i64>,
) -> Result<bool, AppCommandError> {
    // Resolve the (session_id, seq) pair to the auto-incrementing
    // row id. The seq was assigned by the agent loop in the order
    // user → assistant → user(tool_result) → ... so it's unique
    // within a session by construction (UNIQUE(session_id, seq)
    // constraint in the schema).
    let message_id = match crate::db::find_message_id_by_seq(&state.db, &session_id, seq)
        .await
        .map_err(|e| anyhow::anyhow!("update_message_latency: lookup failed: {}", e))?
    {
        Some(id) => id,
        None => {
            // No matching row — the agent loop hasn't persisted
            // the assistant turn yet (cancel cleanup can persist
            // after the controller's `done` event fires). Treat
            // as a no-op so the frontend doesn't surface an error
            // for the cancel race.
            return Ok(false);
        }
    };
    let latency = crate::db::sessions::MessageLatency {
        ttfb_ms,
        gen_ms,
        total_ms,
        thinking_ms,
    };
    crate::db::update_message_latency(&state.db, message_id, &latency)
        .await
        .map_err(|e| anyhow::anyhow!("update_message_latency failed: {}", e))?;
    Ok(true)
}

#[tauri::command]
pub async fn update_message_latency(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    seq: i64,
    ttfb_ms: Option<i64>,
    gen_ms: Option<i64>,
    total_ms: Option<i64>,
    thinking_ms: Option<i64>,
) -> Result<bool, AppCommandError> {
    update_message_latency_inner(
        &state,
        session_id,
        seq,
        ttfb_ms,
        gen_ms,
        total_ms,
        thinking_ms,
    )
    .await
}

/// Patch a `duration_ms` field onto the `tool_result` block
/// inside `messages.content` JSON for the given `tool_use_id`.
/// Per PRD ADR-lite decision 1, the per-tool duration lives in
/// the tool_result block itself (no schema change for the tool
/// side). The frontend measures duration as
/// `Date.now() - tool_call_received_at` and issues this IPC
/// on every `tool:result` event.
///
/// `duration_ms` is an i64 (not `Option`) — the IPC always
/// records a value; a missing block in the DB returns `Ok(false)`
/// from the backend (no error), and the frontend treats that as
/// a benign no-op.
pub async fn record_tool_duration_inner(
    state: &Arc<AppState>,
    session_id: String,
    tool_use_id: String,
    duration_ms: i64,
) -> Result<bool, AppCommandError> {
    crate::db::record_tool_duration(&state.db, &session_id, &tool_use_id, duration_ms)
        .await
        .map_err(|e| anyhow::anyhow!("record_tool_duration failed: {}", e).into())
}

#[tauri::command]
pub async fn record_tool_duration(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    duration_ms: i64,
) -> Result<bool, AppCommandError> {
    record_tool_duration_inner(&state, session_id, tool_use_id, duration_ms).await
}

// ---------------------------------------------------------------------------
// D3 PR1 (2026-06-17): edit_user_message
//
// User-driven IPC: edit a user message in place + cascade-delete every
// strictly-later message in the session + append an audit row. The
// frontend then re-runs the chat send pipeline (separate IPC, PR2/3
// work) so the agent loop regenerates the assistant + tool_use chain
// against the new prompt.
//
// Three concerns the command owns:
//
// 1. **Stream race (cancel-first)**: if a chat stream is in-flight
//    for this session, cancel it and wait for the loop to exit
//    BEFORE touching the DB. Mirrors `delete_session` /
//    `clear_session_messages` (both call `cancel_inflight_for_session`
//    + `await_inflight_exit`). Without this gate, the in-flight
//    loop's next `persist_turn` writes into a session where the
//    user just deleted the assistant turn.
//
// 2. **Permission bypass**: edit does NOT consult the ⑨ 关
//    permission layer. The industry consensus (Cursor / Cline /
//    Cody / OpenHands / OpenCode) is to bypass the modal entirely
//    for user-initiated direct IPCs; the audit log captures every
//    edit so the user can review later.
//
// 3. **Atomicity**: the DB layer wraps the UPDATE + cascade DELETE
//    + INSERT audit in a single transaction. Any failure rolls
//    back all three. The command surfaces the error as a
//    `Result::Err` string (no `ChatEvent::Error` — there is no
//    active stream to emit on; the caller is the Tauri IPC
//    surface, which already converts `Err` to a rejected JS
//    promise on the frontend).
// ---------------------------------------------------------------------------

/// D3 PR1 IPC: edit a user message in place + cascade-delete tail
/// + record an audit row. The frontend re-runs the chat send
/// pipeline (separate IPC) to regenerate the assistant chain
/// against the new prompt.
///
/// `new_content` is the new `MessageContent` for the user row.
/// The wire shape is the standard Anthropic `ContentBlockPayload`
/// union — `Text` for plain text or `Blocks` for richer content
/// (mirrors what `toPayloadContent` accepts on the send path).
/// The DB layer serializes it via `MessageContent`'s `Serialize`
/// impl so the round-trip is lossless.
///
/// Returns the `EditMessageOutcome` on success. Errors are wrapped as `String`
/// for the Tauri IPC rejection (the frontend surfaces them as a
/// toast — same contract as `delete_session` /
/// `clear_session_messages`).
/// F1(2026-08-25):编辑会取消在途轮并顺带清空该 session 队列
/// (design §4);`cleared_queued` 供前端「已丢弃 N 条」toast(PRD R7)。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditMessageOutcome {
    pub cleared_queued: usize,
}

pub async fn edit_user_message_inner(
    state: &Arc<AppState>,
    session_id: String,
    message_seq: i64,
    new_content: MessageContent,
) -> Result<EditMessageOutcome, AppCommandError> {
    // 1. Stream race: cancel any in-flight chat on this session
    // first. Mirrors `delete_session` /
    // `clear_session_messages`. Wait for the loop to exit so the
    // DB-layer cascade DELETE can't race an in-flight
    // `persist_turn`.
    let exit_rx = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &state.inflight_exits,
        &session_id,
    )
    .await;
    await_inflight_exit(exit_rx, "edit_user_message").await;
    // F1(2026-08-25):cancel 链路顺带清队 + 计数(design §4)。
    let cleared_queued =
        crate::agent::message_queue::clear_session(&state.message_queues, &session_id).await;

    // 2. Confirm the session exists. The DB-layer helper is a
    // silent no-op on unknown session (matches the F5 latency
    // IPC contract) — but the user-facing command should
    // surface an explicit error so the frontend doesn't silently
    // succeed on a stale session id.
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("edit_user_message: load_session failed: {}", e))?
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("edit_user_message: session '{}' not found", session_id),
            )
        })?;
    // Confirm the user message we're editing exists too — same
    // UX rationale (silent no-op on the DB layer is the F5
    // latency patch pattern; explicit error here so the frontend
    // can toast "message not found").
    let edited_msg = loaded
        .messages
        .iter()
        .find(|m| m.seq == message_seq && m.role == "user")
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!(
                    "edit_user_message: user message at seq {} not found in session '{}'",
                    message_seq, session_id
                ),
            )
        })?;
    // Defensive: confirm we resolved the same row we're about to
    // patch. The DB layer uses (session_id, seq) as the lookup
    // key — if the loaded row's id differs from what
    // `find_message_id_by_seq` returns, the helper and the
    // loader disagree, which means a corrupt DB. Surface as an
    // error rather than silently editing the wrong row.
    let resolved_id = db::find_message_id_by_seq(&state.db, &session_id, message_seq)
        .await
        .map_err(|e| anyhow::anyhow!("edit_user_message: lookup failed: {}", e))?
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!(
                    "edit_user_message: user message at seq {} not found in session '{}' (resolver mismatch)",
                    message_seq, session_id
                ),
            )
        })?;
    if resolved_id != edited_msg.id {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!(
                "edit_user_message: resolved id {} != loaded id {} for seq {} — refusing to edit",
                resolved_id, edited_msg.id, message_seq
            ),
        ));
    }

    // 3. Hand off to the DB layer. Single transaction wraps the
    // UPDATE + cascade DELETE + INSERT audit; a failure returns
    // `sqlx::Error` which we wrap via anyhow for the IPC
    // rejection. The frontend surfaces as a toast.
    db::edit_user_message(&state.db, &session_id, message_seq, &new_content)
        .await
        .map_err(|e| -> AppCommandError {
            anyhow::anyhow!("edit_user_message: db failed: {}", e).into()
        })?;

    Ok(EditMessageOutcome { cleared_queued })
}

#[tauri::command]
pub async fn edit_user_message(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message_seq: i64,
    new_content: MessageContent,
) -> Result<EditMessageOutcome, AppCommandError> {
    edit_user_message_inner(&state, session_id, message_seq, new_content).await
}

// ---------------------------------------------------------------------------
// Group-chat cache rates (08-10-group-chat-cache-rate)
// ---------------------------------------------------------------------------

/// Per-speaker latest-turn cache-usage read for the group-chat
/// cache-rate display (edit-modal read-only rows). Derived data
/// over `turn_trace` + `messages.speaker` — zero new storage. The
/// frontend computes the percentage (`cache_read / context_input`)
/// via `utils/tokenUsage.ts::cacheRatePercent`; `context_input = 0`
/// (legacy rows) renders "—". Failure here is auxiliary-only: the
/// modal swallows it and shows "—" (does not block editing).
pub async fn group_chat_cache_rates_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Vec<db::trace::SpeakerCacheUsage>, AppCommandError> {
    db::trace::list_speaker_cache_usage(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("group_chat_cache_rates failed: {}", e).into())
}

#[tauri::command]
pub async fn group_chat_cache_rates(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<db::trace::SpeakerCacheUsage>, AppCommandError> {
    group_chat_cache_rates_inner(&state, session_id).await
}

// ---------------------------------------------------------------------------
// D2 cross-session full-text search (08-17-cross-session-search)
// ---------------------------------------------------------------------------

/// User-facing search over all sessions (`messages_fts` FTS5 for
/// ≥3-char queries, LIKE fallback below that so 2-char Chinese
/// words stay searchable). Title hits ride along in the same
/// response, discriminated by `kind`. This is the shared query
/// layer's IPC surface; the agent-driven `search_history` tool
/// (D2 driver ②, follow-up) calls `db::search::search_messages`
/// directly without this command.
pub async fn search_messages_inner(
    state: &Arc<AppState>,
    query: String,
    project_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<db::search::MessageSearchHit>, AppCommandError> {
    db::search::search_messages(&state.db, &query, project_id.as_deref(), limit)
        .await
        .map_err(|e| anyhow::anyhow!("search_messages failed: {}", e).into())
}

#[tauri::command]
pub async fn search_messages(
    state: State<'_, Arc<AppState>>,
    query: String,
    project_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<db::search::MessageSearchHit>, AppCommandError> {
    search_messages_inner(&state, query, project_id, limit).await
}

// ---------------------------------------------------------------------------
// Manual /compact (08-18-manual-compact-command)
// ---------------------------------------------------------------------------

/// 手动 /compact:空闲期对 session 现存历史执行一次 LLM 摘要压缩
/// (与 C3+ 自动路径同源,不受 0.85 触发线限制)。编排主体在
/// `agent::compaction::run_manual_compaction`;本层负责 gate 链:
/// session 存在 → 非群聊(scope 同 C3+)→ `llm_compaction_enabled`
/// 开关(回滚开关,关 = 摘要路径整体停用,含手动 → 水位替换也不吃,
/// 写了无用,明确报错)→ in-flight 拒绝(streaming 中不排队不自动
/// 取消,prd D4)→ provider 解析(与 chat 主路径同源)。失败全部
/// 返回用户可读错误(前端 toast),零静默;summary 行落库后下一次
/// 请求由 init 的水位替换自然吃到(不重付)。
pub async fn compact_session_inner(
    state: &Arc<AppState>,
    session_id: String,
    focus: Option<String>,
) -> Result<crate::agent::compaction::ManualCompactionOutcome, AppCommandError> {
    use crate::agent::compaction::{run_manual_compaction, ManualCompactionError};

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("compact_session: load_session failed: {}", e))?
        .ok_or_else(|| AppCommandError::new(ErrorCategory::InvalidRequest, "会话不存在或已删除"))?;

    if loaded.session.session_type == db::SessionType::GroupChat {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "群聊会话不支持手动压缩",
        ));
    }

    // 回滚开关同口径(init.rs compaction_on):"false" 才关,fail-open。
    if let Ok(Some(v)) = db::config::get_config_value(&state.db, "llm_compaction_enabled").await {
        if v == "false" {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                "LLM 压缩已禁用(config llm_compaction_enabled=false)",
            ));
        }
    }

    // in-flight guard:run_manual_compaction 的 seq=MAX+1 只在无活跃
    // loop 时安全(pattern-llm-compaction 的 seq 契约);streaming 中
    // 拒绝,不排队不自动取消(prd D4)。
    if state
        .session_active_request
        .lock()
        .await
        .contains_key(&session_id)
    {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "当前有轮次进行中,请先停止后再压缩",
        ));
    }

    let resolved =
        crate::agent::chat::lookup_provider_for_session(&session_id, &state.db, &state.catalog)
            .await
            .map_err(|e| {
                let (msg, cat) = e.user_message_and_category();
                AppCommandError::new(ErrorCategory::from(cat), format!("压缩失败:{}", msg))
            })?;

    match run_manual_compaction(
        &state.db,
        &session_id,
        resolved.provider,
        resolved.context_window,
        focus.as_deref(),
        &loaded.messages,
    )
    .await
    {
        Ok(outcome) => Ok(outcome),
        Err(ManualCompactionError::NothingToCompress) => Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "无可压缩内容:近期历史都在保留区内,无需压缩",
        )),
        Err(ManualCompactionError::SummaryFailed(reason)) => Err(AppCommandError::new(
            ErrorCategory::Server,
            format!("摘要压缩失败({}),上下文未改动", reason),
        )),
        Err(ManualCompactionError::PersistFailed) => Err(AppCommandError::new(
            ErrorCategory::Server,
            "摘要落库失败,上下文未改动",
        )),
    }
}

#[tauri::command]
pub async fn compact_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    focus: Option<String>,
) -> Result<crate::agent::compaction::ManualCompactionOutcome, AppCommandError> {
    compact_session_inner(&state, session_id, focus).await
}

/// 接力子会话标题:`接力: {parent 去掉一层已有 "接力: " 前缀}` —— 防
/// 接力链嵌套前缀;80 字符截断由 `rename_session` 服务端兜底。显式设
/// 标题同时防新会话首条用户消息(常是"继续")抢注 auto-title。
fn handoff_child_title(parent_title: &str) -> String {
    let base = parent_title.strip_prefix("接力: ").unwrap_or(parent_title);
    format!("接力: {}", base)
}

/// best-effort 清理接力失败残留的空壳子 session(无 messages,删除
/// 无损;parent 不动)。
async fn cleanup_shell_child(db_pool: &sqlx::SqlitePool, child_id: &str) {
    if let Err(e) = db::delete_session(db_pool, child_id).await {
        tracing::warn!(
            error = %e,
            child_id = %child_id,
            "handoff: cleanup of shell child session failed"
        );
    }
}

/// handoff 持久化半边(生成在 `agent::compaction::generate_handoff_summary`):
/// 建子 session(继承 parent 字段)→ 接力行落库 → parent 侧 metadata
/// 关联。继承走 create 后 UPDATE(`db::create_session` 只收 7 参,
/// mode/worktree/workflow/plugin/title 是硬编码默认值,全部用既有
/// setter 纯新增调用);mode 继承经 `set_session_mode_internal` 复用
/// mode_changed 审计 + Yolo root guard(commands 层单一真源,不绕行
/// 裸列写)。任一步失败 → 清理空壳再报错,保住"失败零副作用"。
pub(crate) async fn persist_handoff_child(
    db_pool: &sqlx::SqlitePool,
    parent: &db::SessionRow,
    gen: &crate::agent::compaction::HandoffSummary,
    focus: Option<&str>,
) -> Result<crate::agent::compaction::HandoffOutcome, AppCommandError> {
    use crate::agent::compaction::{HANDOFF_SUMMARY_KIND, SUMMARY_CONTEXT_PREFIX};

    let child_id = uuid::Uuid::new_v4().to_string();
    let new_session_title = handoff_child_title(&parent.title);

    // child 侧 metadata 创建即写入(零读-改-写)。
    let child_metadata = serde_json::json!({
        "handoff": {
            "parent_session_id": parent.id,
            "parent_title": parent.title,
            "focus": focus,
        }
    });
    let child_metadata_str = child_metadata.to_string();

    let persist = async {
        db::create_session(
            db_pool,
            &child_id,
            &parent.project_id,
            &parent.current_cwd,
            &parent.model,
            parent.model_id.as_deref(),
            None,
            Some(child_metadata_str.as_str()),
        )
        .await
        .map_err(|e| anyhow::anyhow!("建子会话失败: {}", e))?;

        db::rename_session(db_pool, &child_id, &new_session_title)
            .await
            .map_err(|e| anyhow::anyhow!("设子会话标题失败: {}", e))?;

        // worktree 三列按 parent 原样复制(双 session 共享目录是接受
        // 的 MVP 边界 —— task design §7;全默认时跳过写入)。
        if parent.worktree_state != db::WorktreeState::None
            || parent.worktree_path.is_some()
            || parent.last_worktree_path.is_some()
        {
            db::set_worktree_state(
                db_pool,
                &child_id,
                parent.worktree_state,
                parent.worktree_path.as_deref(),
                parent.last_worktree_path.as_deref(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("继承 worktree 失败: {}", e))?;
        }
        if parent.workflow_enabled {
            db::set_session_workflow_enabled(db_pool, &child_id, true)
                .await
                .map_err(|e| anyhow::anyhow!("继承 workflow 状态失败: {}", e))?;
        }
        if parent.plugin_name != "dev" {
            db::set_session_plugin_name(db_pool, &child_id, &parent.plugin_name)
                .await
                .map_err(|e| anyhow::anyhow!("继承插件失败: {}", e))?;
        }
        if parent.mode != db::Mode::Edit {
            crate::commands::question::set_session_mode_internal(db_pool, &child_id, parent.mode)
                .await
                .map_err(|e| anyhow::anyhow!("继承模式失败: {:?}", e))?;
        }

        // 接力行:prefix 落库自包含(HANDOFF_SUMMARY_KIND 契约);
        // 新 session 空表,seq=1;复用 insert_compaction_summary 纯机械
        // (两列落库 + 游标契约,kind 由 metadata 透传)。
        let prefixed = format!("{}\n\n{}", SUMMARY_CONTEXT_PREFIX, gen.summary_text);
        let row_metadata = serde_json::json!({
            "kind": HANDOFF_SUMMARY_KIND,
            "parent_session_id": parent.id,
            "parent_title": parent.title,
            "trigger": "handoff",
            "focus": focus,
            "cutoff_seq": gen.cutoff_seq,
            "tokens_before": gen.tokens_before,
            "tokens_after": gen.tokens_after,
            "model": gen.model,
            "summary_usage": gen.summary_usage,
            "from_prior_fast_path": gen.from_prior_fast_path,
        });
        db::insert_compaction_summary(db_pool, &child_id, &prefixed, 1, &row_metadata)
            .await
            .map_err(|e| anyhow::anyhow!("接力摘要落库失败: {}", e))?;

        // parent 侧读-改-写合并(不 clobber 已有键;低频接受非原子)。
        // 合并基线**现读** DB(不信调用方快照 —— 两次接力的 parent 行
        // 可能是同一个陈旧引用,信任快照会 clobber 上一次的 children)。
        let fresh_parent = db::load_session(db_pool, &parent.id)
            .await
            .map_err(|e| anyhow::anyhow!("回读父会话失败: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("父会话在接力中途消失"))?;
        let mut base = fresh_parent
            .session
            .metadata
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        if !base.is_object() {
            base = serde_json::json!({});
        }
        let mut children = base
            .get("handoff_children")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        children.push(serde_json::json!(child_id));
        base["handoff_children"] = serde_json::Value::Array(children);
        db::set_session_metadata(db_pool, &parent.id, &base)
            .await
            .map_err(|e| anyhow::anyhow!("回写父会话关联失败: {}", e))?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Err(e) = persist {
        cleanup_shell_child(db_pool, &child_id).await;
        return Err(AppCommandError::new(
            ErrorCategory::Server,
            format!("接力失败(子会话已回滚,原会话未改动):{}", e),
        ));
    }

    tracing::info!(
        parent_id = %parent.id,
        child_id = %child_id,
        cutoff_seq = gen.cutoff_seq,
        tokens_before = gen.tokens_before,
        tokens_after = gen.tokens_after,
        "handoff applied"
    );
    Ok(crate::agent::compaction::HandoffOutcome {
        new_session_id: child_id,
        new_session_title,
        cutoff_seq: gen.cutoff_seq,
        tokens_before: gen.tokens_before,
        tokens_after: gen.tokens_after,
        summary_usage: gen.summary_usage,
        model: gen.model.clone(),
    })
}

/// handoff 跨 session 接力(08-18-handoff-mechanism):把当前会话的
/// 全量覆盖摘要作为新 session 的首条 context,长任务换会话续跑。
/// gate 链逐条镜像 `compact_session_inner`(群聊拒绝 / 回滚开关 /
/// in-flight 拒绝 —— 摘要生成读 parent 行集快照,streaming 中拒绝 /
/// provider 解析);生成半边在 `agent::compaction::
/// generate_handoff_summary`(D4 缺段校验重试),持久化半边在
/// [`persist_handoff_child`]。原 session 消息行不动(prd R5/AC4)。
pub async fn handoff_session_inner(
    state: &Arc<AppState>,
    session_id: String,
    focus: Option<String>,
) -> Result<crate::agent::compaction::HandoffOutcome, AppCommandError> {
    use crate::agent::compaction::{generate_handoff_summary, HandoffGenerationError};

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("handoff_session: load_session failed: {}", e))?
        .ok_or_else(|| AppCommandError::new(ErrorCategory::InvalidRequest, "会话不存在或已删除"))?;

    if loaded.session.session_type == db::SessionType::GroupChat {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "群聊会话不支持接力",
        ));
    }

    // 回滚开关同口径(compact_session_inner / init.rs compaction_on)。
    if let Ok(Some(v)) = db::config::get_config_value(&state.db, "llm_compaction_enabled").await {
        if v == "false" {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                "LLM 压缩已禁用,无法生成接力摘要",
            ));
        }
    }

    if state
        .session_active_request
        .lock()
        .await
        .contains_key(&session_id)
    {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            "当前有轮次进行中,请先停止后再接力",
        ));
    }

    let resolved =
        crate::agent::chat::lookup_provider_for_session(&session_id, &state.db, &state.catalog)
            .await
            .map_err(|e| {
                let (msg, cat) = e.user_message_and_category();
                AppCommandError::new(ErrorCategory::from(cat), format!("接力失败:{}", msg))
            })?;

    let gen = match generate_handoff_summary(
        &session_id,
        resolved.provider,
        resolved.context_window,
        focus.as_deref(),
        &loaded.messages,
    )
    .await
    {
        Ok(g) => g,
        Err(HandoffGenerationError::NothingToHandoff) => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                "会话没有可接力的内容",
            ));
        }
        Err(HandoffGenerationError::SummaryFailed(reason)) => {
            return Err(AppCommandError::new(
                ErrorCategory::Server,
                format!("接力摘要生成失败({}),原会话未改动", reason),
            ));
        }
        Err(HandoffGenerationError::SummaryMissingSections(sections)) => {
            return Err(AppCommandError::new(
                ErrorCategory::Server,
                format!(
                    "接力摘要缺少必含段({}),重试后仍失败,原会话未改动",
                    sections.join(", ")
                ),
            ));
        }
    };

    persist_handoff_child(&state.db, &loaded.session, &gen, focus.as_deref()).await
}

#[tauri::command]
pub async fn handoff_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    focus: Option<String>,
) -> Result<crate::agent::compaction::HandoffOutcome, AppCommandError> {
    handoff_session_inner(&state, session_id, focus).await
}
