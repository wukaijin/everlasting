//! run_subagent 的纯辅助(拆分自 dispatch.rs, 08-07-large-file-splitting)。
//! 可写性判定 / task 环境提示 / resume 与澄清消息构建。

use std::sync::Arc;

use sqlx::SqlitePool;

use super::build_worker_messages;
use crate::memory::MemoryCache;

/// Whether a subagent's declared toolset can write (files / shell) —
/// used by the isolation decision. Only **writable** workers need a
/// worktree when dispatched concurrently: read-only workers (e.g.
/// `researcher`) share the parent cwd with no write race, so we save
/// the per-dispatch checkout cost.
///
/// Precedence:
/// - `tools.is_empty()` → inherits the full builtin set (which includes
///   write/shell tools) → writable.
/// - otherwise → writable iff any declared tool is **outside**
///   [`READONLY_TOOL_ALLOWLIST`] (i.e. a write/shell/other tool).
pub(crate) fn worker_is_writable(def: &super::SubagentDef) -> bool {
    if def.tools.is_empty() {
        return true;
    }
    def.tools
        .iter()
        .any(|t: &String| !super::READONLY_TOOL_ALLOWLIST.contains(&t.as_str()))
}

/// C (2026-06-30): wrap the delegation `task` with an isolation
/// environment hint when the worker runs in its own worktree. The hint
/// tells the worker it is on `worker/<run_id>`, its edits are
/// auto-committed by the system (child1 A's `commit_worker_changes`)
/// and merged back by the parent, and it should NOT run `git commit`
/// itself. Shared dispatches get the raw task unchanged.
pub(crate) fn task_with_env_hint(task: &str, isolated: bool, run_id: &str) -> String {
    if !isolated {
        return task.to_string();
    }
    format!(
        "{task}\n\n---\n\
         [environment] You are running in an ISOLATED git worktree on branch \
         `worker/{run_id}`. Your file edits land on that branch, are \
         auto-committed by the system when you finish, and the parent agent \
         will merge them back. You do NOT need to run `git commit` yourself — \
         focus on the task."
    )
}

// ---------------------------------------------------------------------------
// C1 (07-26-subagent-resume): resume message construction
// ---------------------------------------------------------------------------

/// Build the worker's initial `Vec<ChatMessage>` for a resume dispatch,
/// or fall back to a fresh `build_worker_messages` dispatch when the
/// resume is unsafe (design §5: every failure mode falls back rather
/// than erroring — the parent LLM still gets a worker result, just not
/// a continuation).
///
/// Returns `(messages, fallback_note)`:
/// - resume success → `(history + clarification + task, None)`
/// - resume fallback → `(fresh build_worker_messages output, Some("[resume: fallback, reason: <code>]"))`
///
/// Validation order (first failure wins, all → fallback):
/// 1. run_id not found → `resume_run_not_found`
/// 2. run still `running` → `resume_run_still_running`
/// 3. cross-session (`parent_session_id` mismatch) → `resume_run_other_session`
/// 4. messages empty (legacy run / cancel-or-error exit) → `resume_messages_unavailable`
/// 5. messages truncated → `resume_messages_truncated`
#[allow(clippy::too_many_arguments)] // 8 args mirror build_worker_messages' call-site ergonomics
pub(crate) async fn build_resume_messages(
    db: &SqlitePool,
    current_session_id: &str,
    run_id: &str,
    final_task: &str,
    input: &serde_json::Value,
    memory_cache: &Arc<MemoryCache>,
    project_id: &str,
    project_path: &str,
) -> (Vec<crate::llm::types::ChatMessage>, Option<String>) {
    let fresh = || async {
        build_worker_messages(memory_cache, project_id, project_path, final_task).await
    };
    let loaded = match crate::db::subagent_runs::load_messages_by_run_id(db, run_id).await {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "resume: load_messages_by_run_id failed, falling back to fresh dispatch"
            );
            return (
                fresh().await,
                Some("[resume: fallback, reason: load_failed]".to_string()),
            );
        }
    };
    let Some(loaded) = loaded else {
        tracing::warn!(run_id = %run_id, "resume: run not found, falling back");
        return (
            fresh().await,
            Some("[resume: fallback, reason: resume_run_not_found]".to_string()),
        );
    };
    if loaded.status == "running" {
        tracing::warn!(run_id = %run_id, "resume: run still running, falling back");
        return (
            fresh().await,
            Some("[resume: fallback, reason: resume_run_still_running]".to_string()),
        );
    }
    if loaded.parent_session_id != current_session_id {
        tracing::warn!(
            run_id = %run_id,
            run_session = %loaded.parent_session_id,
            current_session = %current_session_id,
            "resume: cross-session run, falling back"
        );
        return (
            fresh().await,
            Some("[resume: fallback, reason: resume_run_other_session]".to_string()),
        );
    }
    if loaded.messages.is_empty() {
        tracing::warn!(run_id = %run_id, "resume: messages empty (legacy/cancel/error), falling back");
        return (
            fresh().await,
            Some("[resume: fallback, reason: resume_messages_unavailable]".to_string()),
        );
    }
    if loaded.truncated {
        tracing::warn!(run_id = %run_id, "resume: messages truncated, falling back");
        return (
            fresh().await,
            Some("[resume: fallback, reason: resume_messages_truncated]".to_string()),
        );
    }
    // Resume success: replay history + clarification + this round's task.
    let mut messages = loaded.messages;
    if let Some(clar) = build_clarification_message(input) {
        messages.push(clar);
    }
    messages.push(crate::llm::types::ChatMessage {
        role: crate::llm::types::Role::User,
        content: crate::llm::types::MessageContent::Text(final_task.to_string()),
        speaker: None,
    });
    tracing::info!(
        run_id = %run_id,
        replayed = messages.len(),
        "resume: continuing prior worker run"
    );
    (messages, None)
}

/// Build the structured clarification user message injected at the
/// resume point (design §6: stale-context handling). The message
/// tells the resumed worker what changed since its prior turn and
/// what this round is for, so it can reconcile any now-stale
/// references in the replayed history. Returns `None` when the
/// caller didn't supply `resume_clarification` (the resumed worker
/// then just sees the replayed history + the new task).
pub(crate) fn build_clarification_message(
    input: &serde_json::Value,
) -> Option<crate::llm::types::ChatMessage> {
    let clar = input.get("resume_clarification")?;
    let purpose = clar.get("this_round_purpose").and_then(|v| v.as_str())?;
    let mut lines: Vec<String> = Vec::new();
    lines.push("[resume clarification — update your context before proceeding]".to_string());
    if let Some(state) = clar.get("current_state").and_then(|v| v.as_str()) {
        if !state.is_empty() {
            lines.push(format!("**Current state:** {}", state));
        }
    }
    if let Some(changes) = clar.get("changes_since_last").and_then(|v| v.as_array()) {
        let non_empty: Vec<&str> = changes
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if !non_empty.is_empty() {
            lines.push("**Changes since your last turn:**".to_string());
            for c in non_empty {
                lines.push(format!("- {}", c));
            }
        }
    }
    lines.push(format!("**This round's purpose:** {}", purpose));
    Some(crate::llm::types::ChatMessage {
        role: crate::llm::types::Role::User,
        content: crate::llm::types::MessageContent::Text(lines.join("\n")),
        speaker: None,
    })
}
