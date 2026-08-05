//! B6 Subagent — worker dispatch (`run_subagent`).
//!
//! Split out of `chat_loop.rs` on 2026-06-23 so the main loop file
//! stays focused on turn orchestration. `run_subagent` is the
//! interceptor helper called from
//! [`crate::agent::chat_loop::run_chat_loop`]'s serial-path tool
//! dispatch when `name == "dispatch_subagent"`; it owns the nested
//! `run_chat_loop` call that drives the worker agent.

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent::chat_loop::run_chat_loop;
use crate::agent::workflow::WorkflowCtx;
use crate::db::subagent_overrides::get_subagent_model_override;
use crate::llm::Provider;
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventSink, ProviderCatalog};
use crate::tools::read_guard::ReadGuard;
use crate::tools::ToolContext;

use super::{
    assemble_subagent_prompt, build_worker_messages, filter_tools_for_subagent,
    filter_tools_readonly, format_dispatch_result_with_model, format_final_text,
    summarize_worker_tool_actions, truncate_messages_for_persistence,
    truncate_transcript_for_persistence, SubagentBufferSink, SubagentCache, SubagentEventSink,
    SubagentStatus, MESSAGES_MAX_BYTES, TRANSCRIPT_MAX_BYTES,
};

// ---------------------------------------------------------------------------
// B6 Subagent (2026-06-19): worker dispatch
//
// `run_subagent` is the interceptor helper called from the
// serial-path tool dispatch loop when `name == "dispatch_subagent"`.
// It owns the nested `run_chat_loop` call that drives the worker
// agent. It was extracted from `chat_loop.rs` into this file on
// 2026-06-23, but it still needs the parent loop's closure
// dependencies (`provider` / `db` / `cancellations` / ...) — the
// alternative would be to thread 22+ parameters through a public
// function, which is the same "too many parameters" cost
// `run_chat_loop` itself pays (see RULE-A-006 docstring at the top
// of `chat_loop.rs`).
//
// The function returns a `(content, is_error, cancel_parent,
// exit_code)` tuple shaped to mirror the `execute_tool` return so
// the caller's serial-path code can treat it uniformly:
//   - `content` = the dispatch_subagent tool_result's content
//     string (status prefix + worker summary).
//   - `is_error` = whether the worker exited non-successfully
//     (cancelled / errored). The caller's serial path emits the
//     tool_result with this flag set so the LLM sees the failure.
//   - `cancel_parent` = whether the worker detected a parent-
//     propagated cancel (user Stop reached the worker). When
//     `true`, the caller's serial loop flips its local `cancelled`
//     flag and drives the existing cancel path — the user's Stop
//     propagates back up through the worker to the parent.
//   - `exit_code` = always `None` (no child process spawned);
//     matches the convention for non-shell tools.
// ---------------------------------------------------------------------------

/// Worker turn budget. Bounded independently of the parent's 50-turn
/// limit so a runaway subagent cannot burn the parent's full budget
/// (PRD §Decisions 8 + review #4). The worker still re-uses C3
/// compaction, so hitting this limit on a long task degrades to
/// compaction rather than an unbounded loop.
///
/// 2026-06-21 (R1): raised from 20 → 200. The original 20-turn
/// cap was sized for the B6 PR1 demo scenarios (small focused
/// tasks). Real `trellis-implement` runs burn 200+ tool calls
/// (code search + edit + verify + RUSTFLAGS / cargo test cycles
/// + DB inspection + spec re-reads), so 20 was an artificial
/// ceiling that hard-terminated workers mid-task. The 200
/// budget is empirically large enough for the heaviest observed
/// `trellis-implement` run while still bounded enough that a
/// runaway worker cannot burn the parent session's full 50-turn
/// budget (a single worker run at 200 turns is 4× the parent
/// budget — a real cost, but acceptable given R3's token-usage
/// fix (this PR) makes the burn visible). Future cost gates
/// (token / wall-clock second-stage) are explicitly deferred.
const SUBAGENT_MAX_TURNS: usize = 200;

// ---------------------------------------------------------------------------
// L3b (2026-06-27): worktree isolation merge + helpers
// ---------------------------------------------------------------------------

/// Resolve the worker's worktree-isolation decision by merging the
/// per-agent frontmatter default with the per-dispatch override.
///
/// Truth table (matches the PRD's "已闭合" merge semantics):
///
/// | frontmatter default | dispatch `isolation` | result |
/// |---------------------|----------------------|--------|
/// | `Some(true)`        | not specified        | isolated |
/// | `Some(true)`        | `Some(false)`        | shared (LLM opted out) |
/// | `Some(false)`/`None`| `Some(true)`         | isolated (LLM opted in) |
/// | `Some(false)`/`None`| not specified        | shared (legacy behavior) |
/// | `Some(false)`/`None`| `Some(false)`        | shared |
/// | `Some(true)`        | `Some(true)`         | isolated |
///
/// Precedence: **dispatch input > frontmatter default > not isolated**.
/// The dispatch input is the LLM's per-call override (`dispatch_subagent`'s
/// `isolation` parameter); the frontmatter default is the SubagentDef's
/// `isolation` field (builtin `general-purpose` = `Some(true)`,
/// `researcher` = `None`).
pub fn resolve_isolation(frontmatter_default: Option<bool>, dispatch_input: Option<bool>) -> bool {
    // Dispatch input wins if present; otherwise the frontmatter
    // default; otherwise `false` (legacy shared-cwd behavior).
    dispatch_input.or(frontmatter_default).unwrap_or(false)
}

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
fn worker_is_writable(def: &super::SubagentDef) -> bool {
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
fn task_with_env_hint(task: &str, isolated: bool, run_id: &str) -> String {
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
async fn build_resume_messages(
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
fn build_clarification_message(
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

/// tool_result. Built by scanning the worker worktree's diff against
/// its base commit (the `worker/<run_id>` branch tip vs its parent).
/// When non-empty, the worker's branch + worktree are PRESERVED so a
/// future PR3 `merge_worker` / `discard_worker` tool can act on them;
/// when empty, the worktree is destroyed immediately.
struct WorkerChanges {
    /// True iff the worker's worktree has any tracked or untracked
    /// changes vs its base commit.
    has_changes: bool,
    /// A short, LLM-friendly summary of the changes (file list +
    /// per-file +/- counts). Empty when `has_changes` is false.
    summary: String,
}

/// Probe the worker worktree for changes vs its base commit. Used by
/// `run_subagent` after the worker exits to decide:
/// 1. **No changes** → destroy the worktree immediately (the branch
///    carries nothing useful); clear `subagent_runs.worktree_path`.
/// 2. **Has changes** → preserve the worktree + branch; the diff
///    summary is appended to the dispatch_subagent tool_result so
///    the parent LLM knows where the worker's edits live.
///
/// Implementation: delegates to `git::diff::diff_worktree`, which
/// already handles tracked + untracked files. We pass a synthetic
/// `session_id` of `<run_id>` so the diff is computed against the
/// `worker/<run_id>` branch (NOT the project's `session/<id>`
/// branch). On any error we conservatively report "has changes"
/// (preserving the worktree is the safe fallback — destroying it
/// could lose the worker's work).
fn probe_worker_changes(worker_worktree_path: &std::path::Path, run_id: &str) -> WorkerChanges {
    match crate::git::diff::diff_worker_worktree(worker_worktree_path, run_id) {
        Ok(result) => {
            if result.files.is_empty() {
                WorkerChanges {
                    has_changes: false,
                    summary: String::new(),
                }
            } else {
                // Build a compact summary: file list + per-file +/-
                // counts. Cap at 10 files to keep the tool_result
                // scannable (the full diff lives on the branch).
                let mut lines: Vec<String> = Vec::new();
                for f in result.files.iter().take(10) {
                    lines.push(format!(
                        "- {} ({}, +{}/-{})",
                        f.path, f.status, f.added, f.removed
                    ));
                }
                let omitted = result.files.len().saturating_sub(10);
                if omitted > 0 {
                    lines.push(format!("... and {} more", omitted));
                }
                WorkerChanges {
                    has_changes: true,
                    summary: lines.join("\n"),
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                worker_worktree = %worker_worktree_path.display(),
                run_id = %run_id,
                error = %e,
                "probe_worker_changes: diff failed; preserving worktree as conservative fallback"
            );
            // Conservative fallback: assume changes exist so we
            // don't destroy a worktree that might hold the worker's
            // edits.
            WorkerChanges {
                has_changes: true,
                summary: "(diff probe failed; changes status unknown)".to_string(),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_subagent(
    provider: &Arc<dyn Provider>,
    // task 07-03-subagent-frontmatter-model: the process-wide provider
    // catalog, threaded so the worker can resolve its own provider when
    // `def.model` is `Some(model_id)`. `None` in unit tests (no
    // `AppState`) and on any production path without an `AppHandle`;
    // `run_subagent` falls back to the parent provider on `None` or
    // catalog miss (see `resolve_worker_provider` below). The value is
    // `Arc<RwLock<ProviderCatalog>>` (clone-cheap) so the caller can
    // pass `state.catalog.clone()` or `None` uniformly.
    catalog: Option<Arc<RwLock<ProviderCatalog>>>,
    context_window: u32,
    parent_rid: &str,
    parent_session_id: &str,
    memory_cache: &Arc<MemoryCache>,
    read_guard: &ReadGuard,
    skill_cache: &Arc<SkillCache>,
    permission_asks: &crate::agent::permissions::PermissionStore,
    cancellations: &Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    _session_active_request: &Arc<Mutex<std::collections::HashMap<String, String>>>,
    background_shells: &crate::background_shell::DefaultRegistry,
    db: &SqlitePool,
    current_ctx: &ToolContext,
    tool_use_id: &str,
    input: &serde_json::Value,
    parent_token: &CancellationToken,
    _parent_sink: &Arc<dyn ChatEventSink>,
    // P2.4 C5 (2026-07-22): the worker's `SubagentEventSink`,
    // replacing the `app_handle: Option<AppHandle>` 渐进方案.
    // Injected into the worker's `SubagentBufferSink` (via
    // `new_with_event_sink`) so worker `subagent:event` /
    // `subagent:finished` reach the transport live. Tauri passes
    // `AppHandleSubagentSink` (IPC); daemon passes
    // `HttpSseSubagentSink` (SSE — was buffer-only pre-C5, the
    // gap this closes); tests pass `ThreadLocalSubagentSink`. The
    // sibling `catalog` param (line ~244) covers the model-
    // resolution use the old `app_handle` also served.
    worker_event_sink: Arc<dyn SubagentEventSink>,
    // L3a (2026-06-24) / L3b PR2 (2026-06-27): when `true`, the
    // worker's toolset is additionally forced down to read-only
    // tools (`filter_tools_readonly`) on top of
    // `filter_tools_for_subagent`. **Post-PR2 this is the
    // SERIAL-ONLY path** (the concurrent dispatch branch in
    // `chat_loop.rs` no longer passes `true` — see L3b PR2). The
    // serial path (single dispatch or mixed batch) keeps passing
    // `false`, and the L3a regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) continues
    // to pin the behavior.
    //
    // **Why kept after PR2**: the parameter is retained (instead
    // of removed) for two reasons:
    // 1. L3a test compat — the regression test
    //    `l3a_single_dispatch_runs_serial_path_unchanged` was
    //    written against the `force_readonly=true` API shape;
    //    removing it would force that test to re-thread its mock
    //    fixtures.
    // 2. Future "force read-only at the subagent level" feature
    //    (e.g. an LLM opts `general-purpose` into read-only for a
    //    single dispatch) can repurpose this param instead of
    //    adding a new one.
    //
    // The concurrent branch's race-dissolution proof (see
    // `.trellis/spec/backend/agent-loop-architecture.md`
    // §"Pattern: Concurrent isolated dispatch (L3b PR2)") no
    // longer depends on the read-only scope; per-worker worktree
    // isolation (PR1) handles the write race. The `force_readonly`
    // arg remains a SERIAL-only behavioral switch.
    force_readonly: bool,
    // L3d (2026-06-25): the process-wide subagent cache, used to
    // look up the dispatched subagent across builtin + user +
    // project layers (replaces the static `lookup_subagent(name)`
    // — `cache.lookup(project_path, name)` returns a cloned
    // `LoadedSubagent` honoring the project > user > builtin
    // precedence + Q2 tools-inheritance). Read-through + mtime-
    // fenced, so a freshly-written `.md` is picked up on the next
    // chat turn without a reload command.
    subagent_cache: &Arc<SubagentCache>,
    // L3b (2026-06-27): the app's data directory, used to compute
    // the worker worktree path (`<app_data_dir>/worktrees/
    // <project_uuid>/worker/<run_id>`). Production threads the real
    // `AppState.app_data_dir`; tests pass an empty path
    // (`Path::new("")`) since worker isolation is opted into
    // per-subagent and most integration tests dispatch `researcher`
    // (no isolation) or `general-purpose` against a non-isolating
    // fixture. A test that wants to exercise isolation passes a
    // tempdir path + sets up a real git repo.
    app_data_dir: &std::path::Path,
    // B (2026-06-30): `true` when this dispatch comes from the
    // concurrent batch (`DispatchBatch::Concurrent` in `chat_loop.rs`).
    // Combined with `worker_is_writable(def)`, this force-isolates
    // concurrent *write-capable* workers onto their own
    // `worker/<run_id>` branch (replaces the old "general-purpose
    // defaults to isolated" safety argument). Concurrent *read-only*
    // workers and all serial dispatches ignore this and fall back to
    // the subagent's `isolation` default (now `None` = shared).
    parallel: bool,
    // 2026-06-30 (`ask_user_question` task): the parent's
    // `QuestionStore` handle. Threaded into the nested
    // `run_chat_loop` so the signature is shape-identical with
    // the parent path; the worker never reaches the
    // `ask_user_question` interception (the tool is in
    // `STRUCTURALLY_DISABLED` and stripped by
    // `filter_tools_for_subagent`), so the store is unused on
    // this path.
    parent_question_store: &crate::agent::question_store::QuestionStore,
    // W1 (Workflow integration, Step 2.4 — 2026-07-08):
    // the workflow session's context. The role-gate
    // check (see `check_workflow_role_gate` below) reads
    // `workflow_def.roles_by_state` and
    // `current_task.status` from this param. `None` for
    // non-workflow sessions — the gate short-circuits
    // (legacy dispatch shape preserved end-to-end).
    //
    // Step 2.5 will read `workflow_def.delegation_templates`
    // from the same param to inject the delegation
    // template; no signature change at Step 2.5.
    workflow_ctx: Option<&WorkflowCtx>,
) -> (String, bool, bool, Option<i32>) {
    // Parse the LLM-supplied { subagent, task } arguments.
    let subagent_name = input.get("subagent").and_then(|v| v.as_str()).unwrap_or("");
    let task = input.get("task").and_then(|v| v.as_str()).unwrap_or("");
    let tool_use_id_owned = tool_use_id.to_string();

    // Resolve the parent session's project_id + path so the worker
    // reads the same memory cache slots the parent uses. The
    // `project_path` is also the key the `SubagentCache` uses to
    // scope its `<project>/.everlasting/agents/` dir.
    let project_id = resolve_project_id(db, parent_session_id).await;
    let project_path = current_ctx.worktree_path.to_string_lossy().to_string();

    // L3d (2026-06-25): resolve the SubagentDef via the cache
    // (builtin + user + project merged with project > user >
    // builtin precedence). Replaces the static `lookup_subagent`.
    // Unknown name → error tool_result (keeps the
    // tool_use/tool_result pairing invariant).
    //
    // W1 (Workflow integration, Step 2.7 — 2026-07-09):
    // workflow sessions consult the *workflow-aware* lookup so the
    // plugin `.everlasting/workflow/<wf>/agents/` layer
    // (Step 2.3, highest precedence) is honored. Before this the
    // dispatch path always used the legacy `lookup`, so a plugin's
    // `researcher.md` / `implementer.md` / `checker.md` was never
    // loaded — the role-gate (Step 2.4) would correctly *allow* the
    // role but the worker fell back to the builtin/project/user body.
    // Non-workflow callers (`workflow_ctx = None`) keep the legacy
    // path byte-for-byte.
    let wf_name = workflow_ctx.map(|c| c.workflow_def.name.as_str());
    let Some(loaded) = (match wf_name {
        Some(wf) => {
            subagent_cache
                .lookup_with_workflow(&project_path, Some(wf), subagent_name)
                .await
        }
        None => subagent_cache.lookup(&project_path, subagent_name).await,
    }) else {
        // Build a friendly "available" hint by re-listing (cheap;
        // the cache is mtime-fenced so this is a HashMap lookup
        // when nothing changed since the dispatch_def was built).
        // Same workflow/legacy split as the lookup above so the
        // hint reflects the plugin layer the caller is dispatching in.
        let available: Vec<String> = match wf_name {
            Some(wf) => {
                subagent_cache
                    .list_with_workflow(&project_path, Some(wf))
                    .await
            }
            None => subagent_cache.list(&project_path).await,
        }
        .into_iter()
        .map(|l| l.def.name)
        .collect();
        let content = format!(
            "Unknown subagent '{}'. Available: {}.",
            subagent_name,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        );
        return (content, true, false, None);
    };
    let def = &loaded.def;
    if task.trim().is_empty() {
        let content = "Missing or empty 'task' parameter. The delegation task must be a                        non-empty string."
            .to_string();
        return (content, true, false, None);
    }

    // W1 (Workflow integration, Step 2.4 — 2026-07-08):
    // workflow state-machine gate. Pure check extracted
    // into `check_workflow_role_gate` (see below) so the
    // logic is unit-testable without standing up the full
    // `run_subagent` signature.
    //
    // **Why a gate**: the workflow session is a guided
    // state machine — the agent SHOULD follow the
    // breadcrumb (planning → implement → check → done)
    // and dispatch the role that matches the current
    // state. Without a gate, an agent in `planning` could
    // dispatch `implementer` early and skip the research
    // step, breaking the workflow contract.
    //
    // **One-shot bypass via `force: true`**: the LLM (or
    // a power-user via the dispatch tool UI) can pass
    // `force: true` to override the gate for a single
    // dispatch — useful when the user explicitly wants to
    // run a researcher task while in `implement` (e.g.
    // "go back and re-research this decision"). The
    // bypass is one-shot (no persistence) and logs a
    // `warn!` so the audit log captures the overstep.
    //
    // **Non-workflow callers / no active task**:
    // `workflow_ctx = None` OR `current_task = None`
    // short-circuits the gate — same as pre-Step-2.4
    // behavior (legacy dispatch shape preserved
    // end-to-end).
    if let Some(denial) = check_workflow_role_gate(workflow_ctx, subagent_name, input) {
        return (denial, true, false, None);
    }

    // L3b (2026-06-27): resolve the worktree-isolation decision.
    // Merge the per-agent frontmatter default (`def.isolation`) with
    // the per-dispatch `isolation` input the LLM may have supplied.
    // Precedence: dispatch input > frontmatter default > not isolated.
    // When isolated, the worker runs in its own git worktree
    // (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`)
    // on branch `worker/<run_id>`, based off the parent session's
    // current worktree HEAD. When not isolated, the worker reuses
    // the parent session's worktree (legacy behavior).
    //
    // **L3a backward-compat**: when `force_readonly=true` (the
    // L3a concurrent dispatch path; the only call site that ever
    // passed `true`), isolation was historically forced off — the
    // concurrent branch was scoped to read-only + shared cwd per
    // the L3a race-dissolution proof. Post-L3b PR2, the concurrent
    // branch no longer passes `true`; isolation now propagates
    // from `def.isolation` + `dispatch.isolation` even in the
    // concurrent path. The short-circuit is retained for
    // `force_readonly=true` so the L3a serial-only regression
    // (`l3a_single_dispatch_runs_serial_path_unchanged`) + any
    // future explicit read-only call site preserve the old
    // "read-only + shared cwd" semantics.
    let dispatch_isolation = input.get("isolation").and_then(|v| v.as_bool());
    // B6+ B (task 07-06-b6plus-b-dispatch-model-arg): per-dispatch
    // model override. The LLM path sends a display_name (the schema
    // `model` enum's values are display_names); the user `@@ --model=`
    // path sends an id (the frontend resolves display_name→id before
    // IPC). Both converge here: `resolve_model_by_name_or_id` accepts
    // either form. A miss (deleted model / typo / empty) → `None`,
    // which means "no dispatch override" — the dispatch then falls
    // through to `resolve_final_model` (DB > frontmatter > parent),
    // preserving A/C zero-regression. A `warn!` makes the silent
    // fallback visible in logs.
    let dispatch_model_raw: Option<&str> = input
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let dispatch_model: Option<String> = match dispatch_model_raw {
        Some(raw) => match resolve_model_by_name_or_id(db, raw).await {
            Ok(Some(id)) => Some(id),
            Ok(None) => {
                tracing::warn!(
                    input = raw,
                    "dispatch model not found (deleted / typo); ignoring, using agent default"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    input = raw,
                    error = %e,
                    "dispatch model lookup failed; ignoring, using agent default"
                );
                None
            }
        },
        None => None,
    };
    let isolated = if force_readonly {
        // Serial-only switch; force isolation off so the read-only +
        // shared-cwd scope is preserved (L3a legacy compat).
        false
    } else if let Some(explicit) = dispatch_isolation {
        // Explicit per-dispatch input always wins — including
        // `isolation: false` to opt OUT of isolation even in a
        // concurrent batch (the caller then owns any write race).
        // Mirrors resolve_isolation's precedence (dispatch > default).
        explicit
    } else if parallel && worker_is_writable(def) {
        // B (2026-06-30): concurrent batch + writable worker + no
        // explicit input → default to isolated, so concurrent writes
        // land on separate `worker/<run_id>` branches. Replaces the
        // old "general-purpose defaults to isolated" safety argument
        // but, unlike a hard force, a caller can still opt out with
        // `isolation: false` (handled above). Read-only concurrent
        // workers (researcher) fall through to def default (shared) —
        // no write race, saves the checkout.
        true
    } else {
        resolve_isolation(def.isolation, dispatch_isolation)
    };

    // The worker_run_id is the `subagent_runs.id` we'll insert below
    // (a UUID). We need it BEFORE the insert to compute the worktree
    // path (the branch name + on-disk dir are derived from it). So
    // we pre-generate the UUID here and pass it into `insert_run`'s
    // slot. This is a small departure from the existing flow (which
    // let `insert_run` generate the id), but it keeps the worktree
    // path + DB row id in lockstep.
    let worker_run_id = uuid::Uuid::new_v4().to_string();
    let worker_branch = crate::git::worktree::worker_branch_name(&worker_run_id);

    // Compute the worker worktree path + create the worktree when
    // isolated. On any failure we FAIL the dispatch (return an error
    // tool_result) — per the PRD's Edge Cases: "worktree 创建失败 →
    // fail dispatch,不降级到不隔离" (avoids silent behavior
    // inconsistency where the LLM thinks isolation is active but
    // it isn't).
    //
    // `worker_worktree_opt` carries the path (Some) when isolation
    // is active + the worktree was created successfully. It's the
    // value threaded into `run_chat_loop`'s `worktree_override`
    // parameter (Some) below, and the value written to
    // `subagent_runs.worktree_path`.
    let worker_worktree_opt: Option<PathBuf> = if isolated {
        match create_worker_worktree(
            db,
            parent_session_id,
            &project_id,
            &worker_run_id,
            app_data_dir,
            &current_ctx.worktree_path,
        )
        .await
        {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::warn!(
                    parent_session_id = %parent_session_id,
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: worker worktree creation failed; failing dispatch (no fallback to non-isolated)"
                );
                let content = format!(
                    "[status: error]\nFailed to create isolated worker worktree on branch \
                     `worker/{}`: {}. The dispatch was aborted — the worker did not run. \
                     Either retry without isolation, or resolve the underlying git error.",
                    worker_run_id, e
                );
                return (content, true, false, None);
            }
        }
    } else {
        None
    };

    // project_main_override (2026-07-29): for an isolated worker,
    // resolve the ORIGINAL project main repo path so the nested
    // `run_chat_loop` can anchor the permission layer's inside-check on
    // the project root (not the worker's checkout subtree — see
    // PermissionContext.project_main_path). Non-isolated workers / paths
    // pass `None` and let `run_chat_loop` fall back to `worktree_path`
    // (which is the project root for them). Reuse the same
    // session→project→project.path resolution as `create_worker_worktree`.
    let project_main_override: Option<PathBuf> = if isolated {
        let main = resolve_project_main_path(db, parent_session_id).await;
        if main.is_empty() {
            tracing::warn!(
                parent_session_id = %parent_session_id,
                worker_run_id = %worker_run_id,
                "run_subagent: failed to resolve project main path for worker; \
                 permission inside-check will anchor on the worktree (may cause \
                 spurious permission prompts on project-root reads)"
            );
            None
        } else {
            Some(PathBuf::from(main))
        }
    } else {
        None
    };

    // L3b (2026-06-27): when isolated, RESET the ReadGuard for the
    // worker. The worker starts in a fresh checkout with no
    // inherited "already-read" file set — if we passed the parent's
    // ReadGuard through, the worker's edit_file would pass the
    // verify_read check for files the parent read (in a DIFFERENT
    // checkout), then fail at verify_fresh (the file doesn't exist
    // in the worker's tree). A fresh empty ReadGuard forces the
    // worker to read files in its own tree before editing.
    //
    // We construct a fresh guard and swap it in for the nested
    // run_chat_loop call; the parent's guard is borrowed (`&`) and
    // untouched. The fresh guard dies with this run_subagent call
    // (no shared state to clean up — ReadGuard is per-session and
    // the worker has no session of its own).
    let worker_read_guard: ReadGuard = if isolated {
        ReadGuard::new()
    } else {
        // Non-isolated: clone the parent's guard (legacy behavior).
        // The clone is cheap (Arc inside).
        read_guard.clone()
    };
    let worker_read_guard_ref = &worker_read_guard;

    // Build the worker's toolset (allowlist + structural-disabled
    // strip). The worker's run_chat_loop call gets this filtered
    // Vec; the parent's tool_defs is unaffected.
    //
    // L3d (2026-06-25): we clone the resolved `def` (the cache
    // returns an owned `LoadedSubagent`) so the worker's filter
    // can consume it. `filter_tools_for_subagent` takes `&SubagentDef`
    // so we just borrow.
    let worker_tool_defs = filter_tools_for_subagent(crate::tools::builtin_tools(), def);
    // L3a (2026-06-24): concurrent dispatch branch forces the
    // worker's toolset down to read-only tools. The serial path
    // passes `force_readonly = false` so `general-purpose` in
    // the serial path keeps its full write/shell/web toolset
    // (gated by `is_worker: true` at the ⑨ permission layer).
    // For `researcher` this is a no-op (its allowlist is already
    // exactly the 4 read-only tools).
    let worker_tool_defs = if force_readonly {
        filter_tools_readonly(worker_tool_defs)
    } else {
        worker_tool_defs
    };

    // Build the worker's messages: [memory_blocks (cache_control),
    // delegation_task]. The task is APPENDed (prompt-cache invariant
    // — see PRD §Decisions 6 + research §10.5). `project_id` +
    // `project_path` were resolved above (before the cache lookup,
    // since the cache scopes its `<project>/.everlasting/agents/`
    // dir by `project_path`).
    // C (2026-06-30): append an isolation environment hint to the
    // delegation task when the worker runs isolated (shared: raw task).
    let final_task = task_with_env_hint(task, isolated, &worker_run_id);

    // C1 (07-26-subagent-resume): branch on `resume_from`. When the
    // caller asks to resume a prior run, the worker's initial
    // messages = the prior run's persisted history + a clarification
    // user message + this round's task. We BYPASS `build_worker_messages`
    // on the resume path (the history already carries the prior
    // memory snapshot — re-injecting would duplicate; design §2
    // trade-off: mid-run memory edits don't apply to resumed runs).
    // Any validation failure (run missing / truncated / cross-session
    // / still-running) falls back to a fresh dispatch and surfaces a
    // `[resume: fallback, reason: <code>]` line in the tool_result so
    // the parent LLM knows the delegation was NOT a continuation.
    // No `resume_from` → the original `build_worker_messages` path
    // (zero regression — existing callers never set the field).
    let resume_from = input.get("resume_from").and_then(|v| v.as_str());
    let (mut worker_messages, resume_fallback_note) = if let Some(run_id) = resume_from {
        build_resume_messages(
            db,
            parent_session_id,
            run_id,
            &final_task,
            input,
            memory_cache,
            &project_id,
            &project_path,
        )
        .await
    } else {
        (
            build_worker_messages(memory_cache, &project_id, &project_path, &final_task).await,
            None,
        )
    };

    // W1 (Workflow integration, Step 2.5 — 2026-07-08):
    // append the filled delegation template to
    // `worker_messages[0]`'s block list. Only fires when:
    // - workflow session (`workflow_ctx.is_some()`)
    // - plugin defines a template for the dispatched role
    //
    // The helper handles both guards (`append_delegation_template`
    // returns `false` on `None` template; the S-B guard inside
    // catches the not-a-user-Blocks-message case). On
    // non-workflow callers OR no plugin template → no-op
    // (legacy behavior preserved).
    //
    // **Per M-E**: this is a dispatch-turn-only injection.
    // The parent's chat_loop's messages[0] is untouched; we
    // only mutate the worker's messages[0]. The worker
    // sees the template as part of its initial context.
    let filled = workflow_ctx.and_then(|ctx| {
        crate::agent::workflow::compute_delegation_template(ctx, &project_path, subagent_name)
    });
    crate::agent::workflow::append_delegation_template(&mut worker_messages, filled);

    // task 07-03-subagent-frontmatter-model: resolve the worker's
    // provider / context_window / display from `def.model` via
    // [`resolve_worker_provider`] (the pure-over-(catalog, db) core,
    // unit-tested). We hold the catalog read lock here and pass
    // `&ProviderCatalog` in; `worker_display` threads to
    // `format_dispatch_result_with_model` so the parent LLM sees which
    // model the worker actually used.
    //
    // 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 1):
    // the resolved model is `resolve_final_model` — DB override
    // (per-agent UI preference) > frontmatter `model:` (file
    // declaration) > parent. The two priority arms collapse into a
    // single `Option<model_id>` before reaching
    // `resolve_worker_provider`, which is **unchanged** (its 6
    // existing unit tests stay green; the priority change is
    // upstream of the resolver).
    //
    // 2026-07-07 (task 07-06-b6plus-b-dispatch-model-arg, B6+ B):
    // the per-dispatch override (`dispatch_model`, parsed above from
    // `input.model`) sits ABOVE `resolve_final_model` in the priority
    // chain: dispatch > DB > frontmatter > parent. The overlay is a
    // single `Option::or`, so `dispatch_model=None` (no per-dispatch
    // override) collapses to exactly the prior behavior (A/C
    // zero-regression). `dispatch_model` is always an id by the time
    // it reaches here (display_name reverse-lookup already happened
    // during parsing).
    let resolved_lower =
        match resolve_final_model(db, def.name.as_str(), def.model.as_deref()).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    agent_name = %def.name,
                    error = %e,
                    "run_subagent: resolve_final_model failed; falling back to frontmatter-only"
                );
                def.model.clone()
            }
        };
    let final_model = dispatch_model.clone().or(resolved_lower);
    let cat_guard = match &catalog {
        Some(c) => Some(c.read().await),
        None => None,
    };
    let (worker_provider, worker_ctx, mut worker_display): (
        Arc<dyn Provider>,
        u32,
        Option<String>,
    ) = resolve_worker_provider(
        final_model.as_deref(),
        provider,
        context_window,
        cat_guard.as_deref(),
        db,
    )
    .await;
    // 2026-07-29 (reviewer model-assignment gap): when `worker_display`
    // is `None` the worker is silently inheriting the PARENT session's
    // model. That's correct behavior, but recording NULL in
    // `subagent_runs.model_display` makes post-hoc DB inspection blind
    // to "which model actually ran" — e.g. a multi-model review where the
    // LLM forgot the `model` arg looks identical to a correct one in the
    // DB (both NULL), so "multi-model disagreement never materialized"
    // is undetectable after the fact (the exact failure in session
    // 6b313ce4: two reviewers, both NULL, both actually ran the parent
    // default model).
    //
    // Backfill the EFFECTIVE model: read the parent session's `model_id`,
    // resolve it to a display_name, and use it. `worker_display` stays
    // `None` only if the parent session itself has no model (degenerate);
    // the wire `[model: ...]` line + DB row then both reflect the actual
    // model the worker ran, instead of hiding it. Best-effort: a DB miss
    // logs at `warn!` and `worker_display` stays `None` (unchanged behavior).
    if worker_display.is_none() {
        match crate::db::sessions::load_session(db, parent_session_id).await {
            Ok(Some(s)) => {
                if let Some(mid) = s.session.model_id.as_deref().filter(|m| !m.is_empty()) {
                    match crate::db::models::get_model(db, mid).await {
                        Ok(Some(m)) => {
                            worker_display = Some(m.display_name);
                        }
                        Ok(None) => tracing::warn!(
                            parent_session_id = %parent_session_id,
                            model_id = %mid,
                            "run_subagent: parent session model_id not in models table; \
                             worker model_display stays None"
                        ),
                        Err(e) => tracing::warn!(
                            parent_session_id = %parent_session_id,
                            error = %e,
                            "run_subagent: get_model failed; worker model_display stays None"
                        ),
                    }
                }
            }
            Ok(None) => tracing::warn!(
                parent_session_id = %parent_session_id,
                "run_subagent: parent session not found; worker model_display stays None"
            ),
            Err(e) => tracing::warn!(
                parent_session_id = %parent_session_id,
                error = %e,
                "run_subagent: load_session failed; worker model_display stays None"
            ),
        }
    }

    // Assemble the worker's system prompt — fully replaces the
    // parent's behavior_prompt + mode_prefix + base_prompt layers.
    // The assembled prompt is threaded as the 23rd
    // `system_prompt_override` argument to the nested
    // `run_chat_loop` call below (was previously dead code
    // discarded at this site; see `docs/review/b6-subagent-assessment.md`
    // §2 + the doc comment on `run_chat_loop.system_prompt_override`).

    // Worker rid + token. The rid is registered into `cancellations`
    // (so user Stop propagates from the parent via the shared map)
    // but NOT into `session_active_request` — that map is
    // session→request 1:1 and a worker entry would evict the
    // parent's mapping, corrupting
    // `cancel_inflight_for_session` / RULE-E-005. The
    // CancellationGuard inside run_chat_loop is constructed with
    // `skip_session_active: true` for the worker path so its Drop
    // does NOT remove the parent's session_active_request entry.
    //
    // The rid suffix uses the tool_use_id so a future PR2
    // transcript row can correlate back to the parent's
    // dispatch_subagent tool_use.
    let worker_rid = format!("{}-sub-{}", parent_rid, tool_use_id_owned);
    let worker_token = parent_token.child_token();
    {
        let mut map = cancellations.lock().await;
        map.insert(worker_rid.clone(), worker_token.clone());
    }

    // B6 PR2: insert the worker's `running` row into
    // `subagent_runs` BEFORE the nested `run_chat_loop` call. The
    // returned id is the `worker_run_id` that the
    // `update_run_finished` call (after the worker returns)
    // targets. The insert is best-effort: a DB failure logs at
    // `warn!` and the worker still runs (the user's dispatch
    // experience is not gated on the audit row). A failed insert
    // leaves `worker_run_id_opt = None`; the post-loop
    // `update_run_finished` is then a no-op.
    //
    // L3b (2026-06-27): we pass the pre-generated `worker_run_id`
    // (computed above so the worktree path + branch name could be
    // derived from it BEFORE the insert). On success, the DB row's
    // id matches the worktree's branch name; on failure, the
    // worktree (if created) is orphaned and the post-loop cleanup
    // handles destruction via the `worker_worktree_opt` local
    // (independent of the DB row's existence).
    let worker_run_id_opt: Option<String> = match crate::db::subagent_runs::insert_run_with_id(
        db,
        &worker_run_id,
        parent_session_id,
        &worker_rid,
        subagent_name,
        Some(task),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui,
        // AC13): thread the worker's *actual* model display into
        // the row. `worker_display` is `Some(name)` on catalog
        // hit (i.e. the worker resolved a model override /
        // frontmatter), `None` on parent inheritance / catalog
        // miss. The frontend reads this for the card / drawer
        // model chip (AC14-15). The wire `[model: <name>]`
        // line in `format_dispatch_result_with_model` follows
        // the same `Option<String>` shape — when
        // `worker_display` is `None`, the line is omitted (no
        // redundant "inherited parent" line; the parent
        // fallback is implied).
        worker_display.as_deref(),
    )
    .await
    {
        Ok(()) => Some(worker_run_id.clone()),
        Err(e) => {
            tracing::warn!(
                parent_session_id = %parent_session_id,
                worker_rid = %worker_rid,
                error = %e,
                "run_subagent: failed to insert subagent_runs row (non-fatal; worker still runs)"
            );
            None
        }
    };

    // L3b (2026-06-27): if isolation is active + the DB row was
    // inserted, record the worktree path on the row. Best-effort
    // (warn+continue on failure — the path is a forward-compat
    // breadcrumb for PR3's merge/discard tool).
    if let (Some(_), Some(ref wt_path)) = (&worker_run_id_opt, &worker_worktree_opt) {
        if let Err(e) = crate::db::subagent_runs::set_worktree_path(
            db,
            &worker_run_id,
            Some(&wt_path.to_string_lossy()),
        )
        .await
        {
            tracing::warn!(
                worker_run_id = %worker_run_id,
                error = %e,
                "run_subagent: failed to record worker worktree_path (non-fatal)"
            );
        }
    }

    // B6 PR2b (RULE-A-014, 2026-06-20): the worker's
    // `PermissionContext.is_worker` override is now threaded via
    // the 21st `is_worker: Option<bool>` parameter on the nested
    // `run_chat_loop` call below (passes `Some(true)`). The
    // pre-PR2b local `_worker_permission_ctx` constructed here
    // (PR1b) was dead code: `run_chat_loop` rebuilds its own
    // `PermissionContext` internally from the session row, so the
    // local value was never consulted on the worker path. PR2b
    // removes the local construction and the parallel comment
    // that documented the (now-resolved) deviation.

    // SubagentBufferSink: records the worker's emits into an in-
    // memory transcript AND (PR2 hotfix) emits each event on the
    // `subagent:event` channel so the frontend `<SubagentDrawer>`
    // (PR3b) can stream the transcript live. Does NOT forward to
    // the parent sink — the parent's frontend only sees the
    // dispatch_subagent tool_call / tool_result pair; the worker's
    // stream stays isolated (Claude Code convention).
    //
    // P2.4 C5 (2026-07-22): the sink's event channel is the
    // injected `worker_event_sink` (Tauri `AppHandleSubagentSink` /
    // daemon `HttpSseSubagentSink` / test `ThreadLocalSubagentSink`)
    // — the old `app_handle: Option<AppHandle>` Some/None branching
    // (and its double-clone) is gone. See `run_chat_loop`'s
    // `worker_event_sink` param doc.
    // Bug1 fix (2026-06-21): the sink's `run_id` becomes the
    // `subagent:event` payload's `runId`, which the frontend store
    // uses as the key for `liveTranscript` / `getRunCache`. It MUST
    // equal `summary.id` (= the DB row id `worker_run_id`), NOT the
    // human-readable `worker_rid` — otherwise the drawer opens with
    // `openRunId = summary.id` but the transcript cache is keyed by
    // `worker_rid`, so the drawer renders blank + stuck-on-running.
    // `worker_run_id_opt` is `None` only when `insert_run` failed
    // (no DB row → no summary → drawer can't open), so the
    // `worker_rid` fallback is unreachable in practice but keeps
    // the sink construction total.
    let event_run_id = worker_run_id_opt
        .clone()
        .unwrap_or_else(|| worker_rid.clone());
    // P2.4 C5 (2026-07-22): the worker's `SubagentBufferSink` is
    // now wired via the injected `SubagentEventSink`. The old
    // `app_handle` match (`new` / `new_without_app_handle` split)
    // is gone — one `new_with_event_sink` path now serves the
    // Tauri IPC sink, the daemon SSE sink, and the test ThreadLocal
    // collector uniformly.
    let worker_sink: Arc<SubagentBufferSink> = Arc::new(SubagentBufferSink::new_with_event_sink(
        event_run_id.clone(),
        parent_session_id.to_string(),
        worker_event_sink.clone(),
    ));
    let worker_sink_dyn: Arc<dyn ChatEventSink> = worker_sink.clone();

    // Nested run_chat_loop. The worker reuses the parent's
    // session_id for DB linkage (its turns land in the same
    // `messages` table), but:
    //   - `skip_session_active: true` so the guard's Drop does NOT
    //     evict the parent's session_active_request entry.
    //   - `max_turns: Some(SUBAGENT_MAX_TURNS)` to bound the worker's
    //     turn budget.
    //   - The worker_token is the parent_token's child, so a user
    //     Stop that reaches the parent also fires the worker
    //     (cancel propagation).
    //
    // Boxed: `run_subagent` → `run_chat_loop` → `run_subagent`
    // (worker dispatches its own subagent? No — workers have
    // `dispatch_subagent` stripped from their tools, so the
    // recursion is bounded at depth 1). Still, the async-fn
    // recursion is statically unbounded (the compiler cannot prove
    // the depth-1 invariant), so `Box::pin` breaks the size-
    // infinite Future chain. The cost is one heap allocation per
    // worker dispatch — negligible relative to the LLM round-trip.
    //
    // 2026-06-26 (task 06-26-subagent-per-run-grant): construct a
    // fresh per-run grant cache for THIS worker. The Arc dies with
    // this `run_chat_loop` call (no shared state across workers,
    // no leakage to the parent session). L3a concurrent dispatch
    // → each worker's `run_subagent` constructs its own Arc →
    // isolated caches (a grant on one worker's `cargo` does not
    // authorize another worker's `cargo`).
    let run_grants =
        std::sync::Arc::new(crate::agent::permissions::run_grant::RunGrantCache::new());
    Box::pin(run_chat_loop(
        worker_tool_defs,
        worker_provider.clone(),
        worker_ctx,
        worker_rid.clone(),
        parent_session_id.to_string(),
        worker_messages,
        worker_sink_dyn,
        db.clone(),
        cancellations.clone(),
        _session_active_request.clone(),
        // L3b (2026-06-27): pass the (possibly reset) worker
        // ReadGuard. When isolated, this is a fresh empty guard
        // (the worker starts in a new checkout with no inherited
        // reads); when not isolated, it's a clone of the parent's
        // guard (legacy behavior).
        worker_read_guard_ref.clone(),
        memory_cache.clone(),
        skill_cache.clone(),
        permission_asks.clone(),
        worker_token,
        None,
        background_shells.clone(),
        Some(SUBAGENT_MAX_TURNS),
        // B6 PR1b review #2: worker path — skip_session_active = true
        // so the worker's guard Drop does not evict the parent's
        // session_active_request[parent_session_id] entry.
        true,
        // B6 PR1b: worker path — skip_persist = true so the worker's
        // intermediate turns stay in-memory only. The
        // SubagentBufferSink captures them; PR2 will persist the
        // transcript into `subagent_runs`. Without this, the worker
        // would race the parent's persist_turn calls on the same
        // `(session_id, seq)` key (UNIQUE collision).
        true,
        // B6 PR2b (RULE-A-014, 2026-06-20): worker path — is_worker
        // = Some(true) so the nested run_chat_loop builds a
        // PermissionContext with is_worker: true. Pre-2026-06-22
        // (RULE-FrontSubagent-003) this collapsed Tier 4
        // ask_path / ask_shell to Decision::Deny (the worker had no
        // UI sink — a permission:ask would hang forever on the
        // oneshot); since 2026-06-22 worker asks route through the
        // `WorkerAskBanner` round-trip (see permission-layer.md §5b
        // — biased select over parent cancel / 120s timeout /
        // oneshot response). The `is_worker` flag now mainly scopes
        // the ask's internal session key (`"worker:{run_id}"`) and
        // stops a worker `AllowAlways` from persisting into the
        // parent's `session_tool_permissions` (cross-privilege
        // boundary). Pre-PR2b the worker path constructed
        // `_worker_permission_ctx` here but never threaded it into
        // the nested call, so the override was unreachable on the
        // worker's actual permission checks.
        Some(true),
        // P2.4 C5 (2026-07-22): the worker's nested `run_chat_loop`
        // receives the injected `worker_event_sink` + `catalog`
        // (replacing the forwarded `app_handle`). The worker itself
        // never dispatches a subagent (`dispatch_subagent` is
        // stripped from its toolset), so `catalog = None` is honest
        // here — the param is a dead carry-through on the worker
        // path (model resolution happened above via the sibling
        // `catalog` arg + `resolve_worker_provider`). The sink is
        // cloned (not moved) so the post-loop `subagent:finished`
        // emit below can still use it.
        None,
        worker_event_sink.clone(),
        // 2026-06-21 fix (B6 review defect A): thread the
        // worker's `SubagentDef.system_prompt` (built via
        // `assemble_subagent_prompt` above) as the 23rd
        // `system_prompt_override` parameter. When `Some`, the
        // nested `run_chat_loop` uses this string directly and
        // skips the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` step. Pre-fix the worker was getting the
        // parent's system prompt (the worker's own prompt was
        // dead code), causing prompt / permission contradictions
        // (worker told "you can write" but `is_worker=true` made
        // Tier 4 ask_path collapse to Deny pre-2026-06-22).
        Some(assemble_subagent_prompt(def, task)),
        // 2026-06-22 (RULE-FrontSubagent-003 fix): thread the
        // worker's `subagent_runs.id` (DB row UUID) so the
        // nested run_chat_loop can build the worker-owned
        // permission session id and propagate `worker_run_id`
        // into `PermissionAskPayload.worker_run_id`. `None` when
        // `insert_run` failed (no DB row → no drawer can open →
        // worker ask interactive would have nothing to route to;
        // the ask_path worker branch will fall back to a logging
        // sentinel via the unwrap_or_else in permissions::ask_path,
        // but the practical case is "spawn failed" — the parent
        // gets an Error tool_result anyway).
        worker_run_id_opt.clone(),
        // L3d (2026-06-25): thread the subagent cache so the
        // worker's own per-turn tool list construction can append
        // the dynamic `dispatch_subagent` ToolDef (the worker's
        // `filter_tools_for_subagent` then strips it via
        // `STRUCTURALLY_DISABLED`, preventing nesting). Also
        // powers any future sub-subagent dispatch (also structurally
        // disabled in MVP). The cache is shared (Arc clone), so the
        // worker sees the same mtime-fenced view as the parent.
        subagent_cache.clone(),
        // 2026-06-26 (task 06-26-subagent-per-run-grant): per-run
        // grant cache for this worker. `Some(Arc<...>)` threads
        // the cache into the worker's `PermissionContext.run_grants`
        // so `check.rs` Tier 4 can consult it before falling through
        // to `ask_path`, and the worker's `AllowAlways` arm in
        // `ask_path` can write to it. Dies with this `run_chat_loop`
        // call — no persistence to `session_tool_permissions`.
        Some(run_grants),
        // L3b (2026-06-27): the worker's isolated worktree path.
        // When `Some(path)`, the nested `run_chat_loop` uses `path`
        // as the worker's worktree root (redirecting the worker's
        // tools into the isolated checkout) INSTEAD of the parent
        // session's worktree_path. When `None`, the loop builds the
        // worktree_path from the session row (legacy shared-cwd
        // behavior).
        worker_worktree_opt.clone(),
        // project_main_override (2026-07-29): the worker's original
        // project main repo path when isolated, threaded into the nested
        // loop's `PermissionContext.project_main_path`. See the
        // `project_main_override` local above.
        project_main_override.clone(),
        // L3b (2026-06-27): thread the app_data_dir so the worker's
        // own (structurally-disabled) dispatch_subagent interceptor
        // would have it — in practice the worker never dispatches
        // a sub-subagent (STRUCTURALLY_DISABLED), so this is purely
        // for signature uniformity. We pass the same path the parent
        // passed us.
        app_data_dir.to_path_buf(),
        // explicit-agent-dispatch (2026-06-30): worker path —
        // no forced dispatch on the nested call. User-forced
        // dispatch is a parent-LLM bypass; only the parent chat
        // command honors `@@` prefixes.
        None,
        // 2026-06-30 (`ask_user_question` task): the worker's
        // toolset strips `ask_user_question` via
        // `STRUCTURALLY_DISABLED`, so the worker never reaches
        // the `chat_loop.rs` interception branch and this store
        // is unused on the worker path. We still pass it (the
        // parameter is non-optional) so the signature stays
        // shape-identical with the parent — pass the parent's
        // handle cloned, defensively. If a future change ever
        // re-enables the tool for workers, the same store is
        // already wired in (no further changes needed).
        parent_question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5
        // — 2026-07-08): worker nested call passes `None`
        // — the worker focuses on its dispatched task, NOT
        // the parent session's workflow state. Workflow
        // breadcrumbs stay parent-scoped.
        None,
        // Group chat (07-29-group-chat): worker nested call passes
        // `None` — group-chat turn-taking is an outer-loop concern
        // (`run_group_chat_loop`), not a worker concern. The
        // nominate/end tools are also stripped from the worker's
        // toolset (only surfaced via builtin_tools to the moderator's
        // loop), so the interception never fires here.
        None,
        // Group chat (07-29-group-chat, Phase 4 TODO-A): worker
        // nested call passes `None` — workers don't carry a
        // speaker (they ARE the assistant's inner sub-task, not a
        // distinct participant in the outer session's transcript).
        // Additionally, the worker path above sets `skip_persist =
        // true`, so the carried value would never reach the DB
        // anyway. Pass `None` for symmetry with the parent's
        // classic-chat call site.
        None,
    ))
    .await;

    // Drain the worker's accumulated state.
    //
    // 2026-06-21 (R2): the status picker now distinguishes
    // `max_turns` (soft-terminal, worker burned its 200-turn
    // budget without cleanly finishing) from `end_turn` /
    // `tool_use` (clean completion). The `was_incomplete` flag
    // is set by the sink's `Done{max_turns}` arm; the
    // `was_cancelled` flag is set by the `Done{cancelled}` arm;
    // `had_error` is set by the `Error` arm. The three are
    // mutually exclusive in practice (the agent loop's max_turns
    // branch is reached only when no cancel or error fired).
    let worker_text = worker_sink.final_text();
    // C2+ (2026-07-05, task `07-05-c2-loop-active-intervention` PR3):
    // capture the worker's loop-terminated signal BEFORE the status
    // picker so we can route it to `Incomplete` (the worker did not
    // cleanly finish — it was force-stopped by the harness mid-loop
    // after consecutive loop-detection hits ≥ 3). Kept as a separate
    // bool from the status so the `format_dispatch_result_with_model`
    // call + the loop-terminated line append below can read it
    // without re-parsing the status enum.
    let worker_loop_terminated = worker_sink.was_loop_terminated();
    let status = if worker_sink.was_cancelled() {
        SubagentStatus::Cancelled
    } else if worker_sink.had_error() {
        SubagentStatus::Error
    } else if worker_sink.was_incomplete() {
        // 2026-06-21 (R2): the `max_turns` soft-terminal path
        // is its own status (NOT `Completed` — the worker did
        // not cleanly finish). The DB `incomplete` row is the
        // signal for "useful partial output, did not exhaust
        // the task"; the `[status: incomplete]\n<partial>\n
        // [INCOMPLETE_MARKER]` wire shape makes it visible in
        // the parent's tool_result.
        SubagentStatus::Incomplete
    } else if worker_loop_terminated {
        // C2+ (PR3): the worker's loop-detection state machine
        // hit the consecutive-hits threshold and the worker
        // path took the direct-break short-circuit (no
        // `QuestionStore` round-trip, no audit row — R5).
        // Treat as `Incomplete`: the worker did useful partial
        // work but was force-stopped before completing the
        // task. The `[loop terminated: ...]` line appended
        // below carries the harness signal to the parent LLM.
        SubagentStatus::Incomplete
    } else {
        SubagentStatus::Completed
    };

    // B6 PR2: persist the worker run to `subagent_runs`. The flow:
    // 1. Snapshot the transcript from the sink.
    // 2. Apply the 4 MiB cap (returns the head+tail truncated
    //    vector + a `truncated` flag).
    // 3. Build the terminal `TokenUsage` (sum of per-turn usage
    //    the sink accumulated from `ChatEvent::Done { usage }`
    //    events).
    // 4. UPDATE the `running` row to the terminal state.
    //
    // **Worker token isolation** (2026-06-26 reversal of
    // RULE-A-015/PR2a): the parent's `sessions.last_*` snapshot
    // is NOT updated by the worker. `update_last_turn_usage` is
    // back inside the `!skip_persist` gate at `chat_loop.rs`, so
    // worker turns (which run with `skip_persist=true`) don't
    // touch the parent's snapshot. The sink's per-turn
    // accumulator is the ONLY path by which the worker's
    // `TokenUsage` reaches disk — `cumulative_usage()` produces
    // the worker-run-level total for `token_usage_json`, written
    // here. Worker token usage is visible to the parent only via
    // `<SubagentDrawer>`.
    //
    // The UPDATE is best-effort: a DB failure logs at `warn!` and
    // continues (the dispatch_subagent tool_result is the
    // user-visible artifact; the DB row is for PR3's expand UI and
    // audit reads). Failing the dispatch on a DB error would mask
    // the worker's actual outcome and could re-fire the
    // tool_use/tool_result mismatch (RULE-A-007 invariant).
    if let Some(worker_run_id) = worker_run_id_opt.as_ref() {
        let transcript_snapshot = worker_sink.transcript_snapshot();
        let (truncated_transcript, transcript_truncated) =
            truncate_transcript_for_persistence(transcript_snapshot, TRANSCRIPT_MAX_BYTES);
        // C1 (07-26-subagent-resume): snapshot the worker's final
        // messages (captured by `record_worker_messages` on the chat
        // loop's normal completion path) and truncate for persistence.
        // Empty for cancel/error/incomplete exits (those skip the
        // snapshot call) → messages_json persists "[]" and resume
        // falls back to fresh dispatch. Over-cap runs also collapse
        // to empty + truncated=1 (partial history is unsafe to resume).
        let worker_messages = worker_sink.worker_messages();
        let (truncated_messages, messages_truncated) =
            truncate_messages_for_persistence(worker_messages, MESSAGES_MAX_BYTES);
        let cumulative_usage = worker_sink.cumulative_usage();
        let finished_at = chrono::Utc::now().to_rfc3339();
        let status_db = match status {
            SubagentStatus::Completed => crate::db::subagent_runs::SubagentStatusDb::Completed,
            SubagentStatus::Cancelled => crate::db::subagent_runs::SubagentStatusDb::Cancelled,
            SubagentStatus::Error => crate::db::subagent_runs::SubagentStatusDb::Error,
            // 2026-06-21 (R2): max_turns soft-terminal. The DB
            // CHECK constraint was widened to include
            // `'incomplete'` by the
            // `widen_subagent_runs_status_check_for_incomplete`
            // migration; the `Incomplete` variant was added to
            // both `agent::subagent::SubagentStatus` and
            // `db::subagent_runs::SubagentStatusDb` in lockstep.
            SubagentStatus::Incomplete => crate::db::subagent_runs::SubagentStatusDb::Incomplete,
        };
        match crate::db::subagent_runs::update_run_finished(
            db,
            worker_run_id,
            status_db,
            &finished_at,
            &worker_text,
            // B6 redesign PR1 (2026-06-21): the prefix-stripped
            // final text that the drawer renders in its Reply
            // segment. `summary` carries the same string for
            // backward compat (the legacy wire field); `final_text`
            // is the new consumer-facing field. Both land in
            // distinct DB columns so legacy `summary` consumers
            // (e.g. PR3 list-view summaries) keep working unchanged.
            &format_final_text(status, &worker_text),
            &cumulative_usage,
            &truncated_transcript,
            transcript_truncated,
            // 2026-06-22 (RULE-FrontSubagent-004): thread the actual
            // completed turn count so the drawer's `statusDisplay`
            // can render "stopped at turn N" / "incomplete at turn N"
            // for the cancelled / incomplete terminal states.
            // Completed runs also carry the count (harmless; the
            // drawer only reads it for cancelled + incomplete).
            // The counter is the sink's REAL per-turn Done count
            // (synthetic cancelled / max_turns terminals don't
            // increment — see `SubagentBufferSink::turns_completed`).
            Some(worker_sink.turns_completed() as i64),
            // C1 (07-26-subagent-resume): messages payload for resume.
            &truncated_messages,
            messages_truncated,
        )
        .await
        {
            Ok(()) => {
                // Bug2 fix (2026-06-21): emit a one-shot
                // `subagent:finished` terminal signal so the frontend
                // `<SubagentDrawer>` / `<ToolCallCard>` flip from
                // `running` to the terminal state without polling.
                // The frontend listener refetches `get_subagent_run`
                // (drawer: status + finishedAt + full transcript)
                // and `list_subagent_runs_by_session` (card: status).
                // Emitted only on the Ok arm — a DB failure leaves
                // the row `running`, so emitting here would cache a
                // stale `running` row as terminal. Best-effort: a
                // Tauri emit failure is non-fatal (the dispatch
                // tool_result is the user-visible terminal signal).
                // transport-abstraction 2026-07-20 (P1.3):
                // route the `subagent:finished` IPC emit through
                // the `SubagentEventSink` trait. The trait
                // handles the `app.emit` (production) /
                // no-op (test) split internally — no
                // `Option<AppHandle>` branching here.
                worker_sink.emit_subagent_finished(
                    worker_run_id,
                    parent_session_id,
                    status_db.as_str(),
                    &finished_at,
                );
            }
            Err(e) => {
                tracing::warn!(
                    worker_run_id = %worker_run_id,
                    error = %e,
                    "run_subagent: failed to persist subagent_runs update (non-fatal)"
                );
            }
        }
    }

    // Detect parent-driven cancel: the parent token fired while the
    // worker was running. The worker's own cancel_done event may
    // NOT have fired if the cancel arrived after the worker loop
    // already returned (e.g. worker finished turn 1 cleanly, then
    // parent cancel propagated before turn 2's select! polled).
    // The child_token relationship makes the worker_token fire when
    // the parent fires; check parent_token directly so the caller's
    // serial loop flips its `cancelled` flag and drives the existing
    // cancel path (matches the user's Stop intent).
    let cancel_parent = parent_token.is_cancelled() && status == SubagentStatus::Cancelled;

    // RULE-BackSubagent-001 (PR2): for non-completed terminal states,
    // summarize the worker's executed tool_calls so the parent LLM can
    // do compensatory repair (skip already-landed writes, retry failed
    // tools). Completed gets `None`; an empty summary (worker executed
    // no tool_calls before exiting) also gets `None` so no empty
    // "Worker partial actions:" header lands in the tool_result.
    let partial_actions = if matches!(status, SubagentStatus::Completed) {
        None
    } else {
        let summary = summarize_worker_tool_actions(&worker_sink.transcript_snapshot());
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    };

    // L3b (2026-06-27): worktree change-detection + lifecycle.
    //
    // When the worker ran in an isolated worktree (`worker_worktree_opt`
    // is Some), we probe the worktree for changes vs its base commit
    // after the worker exits:
    //   - **No changes** → destroy the worktree immediately (the
    //     branch carries nothing useful). Clear `subagent_runs.worktree_path`.
    //   - **Has changes** → preserve the worktree + branch; the diff
    //     summary is appended to the dispatch_subagent tool_result
    //     (below) so the parent LLM knows where the worker's edits
    //     live ("changes left on branch worker/<run_id>"). A future
    //     PR3 `merge_worker` / `discard_worker` tool acts on the
    //     preserved branch.
    //
    // The change-detection + destroy/preserve decision happens
    // REGARDLESS of terminal status (completed / cancelled / error /
    // incomplete) — per the PRD's Edge Cases: "worker 取消 → 按正常
    // 完成处理 (有 changes 保留 branch, 无 destroy)". A cancelled
    // worker that landed partial writes still has useful artifacts
    // worth preserving for inspection.
    let mut worker_changes_summary: Option<String> = None;
    if let Some(wt_path) = worker_worktree_opt.as_ref() {
        let changes = probe_worker_changes(wt_path, &worker_run_id);
        if changes.has_changes {
            // A (auto-commit, 2026-06-30): commit the worker's
            // working-tree changes onto `worker/<run_id>` so the branch
            // tip advances past the base. `probe_worker_changes` diffs
            // the working tree while `do_merge_blocking` merges branch
            // tips — without this, a worker that never commits leaves
            // worker_tip == parent_tip and merge_worker hits the
            // is_ancestor == short-circuit (merge_worker.rs:651) →
            // "merged fast-forward" with zero changes actually merged
            // (silent false-success). Failure is non-fatal: warn +
            // preserve the worktree anyway; merge degrades to legacy.
            if let Err(e) = crate::git::worktree::commit_worker_changes(wt_path, &worker_run_id) {
                tracing::warn!(
                    worker_run_id = %worker_run_id,
                    worktree = %wt_path.display(),
                    error = %e,
                    "run_subagent: auto-commit of worker changes failed; preserving worktree (merge may degrade to legacy behavior)"
                );
            }
            // Preserve the worktree + branch. The DB row's
            // `worktree_path` column was already set to `wt_path`
            // above (after `insert_run`); leave it as-is.
            worker_changes_summary = Some(format!(
                "Worker changes left on branch `{}` (worktree at `{}`). \
                 Use `git diff` in that worktree to review, or merge/discard \
                 via a future tool.\n\n{}",
                worker_branch,
                wt_path.display(),
                changes.summary
            ));
        } else {
            // No changes — destroy the worktree + branch. Best-effort
            // (a destroy failure leaves a stale worktree; a future
            // sweep would clean it up — out of scope for PR1).
            let project_main_path = resolve_project_main_path(db, parent_session_id).await;
            if !project_main_path.is_empty() {
                if let Err(e) = crate::git::worktree::destroy_worker(
                    std::path::Path::new(&project_main_path),
                    wt_path,
                    &worker_run_id,
                ) {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        worktree = %wt_path.display(),
                        error = %e,
                        "run_subagent: destroy_worker failed on no-changes exit (non-fatal; stale worktree left behind)"
                    );
                }
            }
            // Clear the DB column (best-effort).
            if worker_run_id_opt.is_some() {
                if let Err(e) =
                    crate::db::subagent_runs::set_worktree_path(db, &worker_run_id, None).await
                {
                    tracing::warn!(
                        worker_run_id = %worker_run_id,
                        error = %e,
                        "run_subagent: failed to clear worktree_path after destroy (non-fatal)"
                    );
                }
            }
        }
    }

    let (content, is_error) = format_dispatch_result_with_model(
        status,
        &worker_text,
        partial_actions.as_deref(),
        worker_display.as_deref(),
    );

    // C2+ (2026-07-05, task `07-05-c2-loop-active-intervention` PR3):
    // when the worker exited via the C2+ direct-break short-circuit
    // (loop detection fired 3 turns in a row), append a
    // harness-generated line to the dispatch_result content so the
    // parent LLM sees the loop-termination signal and can decide
    // whether to retry / change strategy / accept (R5). The line is
    // appended AFTER `format_dispatch_result_with_model` so the
    // existing `[status: incomplete]` prefix + partial-actions
    // section stay unchanged; the loop-terminated line is a new
    // trailing signal.
    //
    // Why a trailing line (vs a new `SubagentStatus::LoopTerminated`
    // variant): adding a 5th status would ripple through
    // `format_final_text` / `format_dispatch_result` /
    // `SubagentStatusDb` (DB CHECK constraint + migration) /
    // frontend drawer status pill. C2+ R5 explicitly defers that
    // (worker runs have their own transcript; the parent only needs
    // to know the worker was force-stopped). The trailing-line
    // approach matches the existing `worker_changes_summary`
    // pattern below (also a trailing append on a non-clean exit).
    let content = if worker_loop_terminated {
        format!(
            "{}\n\n{}",
            content, "[loop terminated: worker 因循环重复操作被自动终止，未完成全部步骤]"
        )
    } else {
        content
    };

    // L3b (2026-06-27): append the worker-changes summary to the
    // tool_result content when the worker left changes on its branch.
    // The summary tells the parent LLM where to find the worker's
    // edits (branch name + worktree path + diff file list). We
    // append AFTER `format_dispatch_result` so the existing
    // `[status: ...]` prefix + partial-actions section stay
    // unchanged; the changes summary is a new trailing section.
    let content = if let Some(summary) = worker_changes_summary {
        format!("{}\n\n{}", content, summary)
    } else {
        content
    };
    // C1 (07-26-subagent-resume): when the caller asked to resume a
    // prior run but the resume path fell back to a fresh dispatch
    // (run missing / truncated / cross-session / still-running),
    // surface a trailing `[resume: fallback, reason: <code>]` line so
    // the parent LLM knows the worker did NOT continue the prior
    // conversation. Appended last so all other trailing sections
    // (loop-terminated, changes summary) stay in their existing order.
    let content = if let Some(note) = resume_fallback_note {
        format!("{}\n\n{}", content, note)
    } else {
        content
    };
    (content, is_error, cancel_parent, None)
}

/// task 07-03-subagent-per-agent-model-ui: priority chain
/// `DB override > frontmatter > parent`. Pure (modulo the DB call
/// for the override lookup) so unit tests can cover the merge
/// without spinning up a full `run_subagent` fixture.
///
/// Decision: read the DB override FIRST (always — even if the
/// frontmatter is `Some(...)`); the DB row is the user-managed
/// "set this agent to this model" affordance, which by design
/// overrides anything the `.md` file declares. If the DB row
/// doesn't exist OR the lookup fails, fall through to the
/// frontmatter `model:` value. If both are absent, the returned
/// `None` lets [`resolve_worker_provider`] handle the
/// parent-inheritance fallback.
///
/// **Catalog-miss decision (NOT a fallback chain)**: when the DB
/// override is `Some(mid)` but the catalog later misses (model
/// was deleted / provider's `api_key` is empty), the
/// `resolve_worker_provider` path already logs `warn!` + falls
/// back to the parent provider (NOT to the frontmatter — the
/// frontmatter is a *declaration* of intent, not a *fallback*).
/// The DB override is the highest-priority declaration; the
/// frontmatter is the second-priority declaration; both are
/// declarations of "which model to use", and the parent
/// inheritance is the catch-all when there's no declaration.
/// This matches the design's stated priority chain (DB > fm >
/// parent) — a missing highest-priority declaration does NOT
/// silently defer to the second-priority declaration; it errors
/// to the parent. Settings UI surfaces invalid overrides with a
/// red "model 已删除" badge so the user can fix them.
///
/// Failure mode: a transient DB error during the override
/// lookup logs `warn!` (in the caller) + falls through to the
/// frontmatter `model:` (NOT to the parent). The Settings UI
/// works on a stable DB; a transient error is rare and the
/// frontmatter is a sensible "default to file" fallback for
/// the duration of the error.
/// B6+ B (task 07-06-b6plus-b-dispatch-model-arg): resolve a model
/// id from either an id (passthrough) or a display_name (reverse
/// lookup). Serves the LLM-driven dispatch path where the
/// `dispatch_subagent` schema's `model` enum values are
/// display_names (human-readable; the LLM has no other way to learn
/// which models exist — `build_system_prompt` does not list models).
///
/// - Exact id match first (`get_model`, O(1)).
/// - Miss → `list_models` reverse-lookup on `display_name`; first
///   match wins (display_name should be unique but DB does not
///   enforce it — the rare ambiguity takes the first row, which is
///   deterministic for a given DB state).
/// - Empty / whitespace-only input → `Ok(None)`.
/// - Not found → `Ok(None)` (NOT an error): the caller treats `None`
///   as "no dispatch override" and falls through to
///   `resolve_final_model`, so a deleted model / typo degrades
///   gracefully to the agent's configured default.
///
/// Returns the resolved model id (catalog key) for the caller to
/// feed into [`resolve_worker_provider`].
pub(crate) async fn resolve_model_by_name_or_id(
    db: &SqlitePool,
    input: &str,
) -> Result<Option<String>, sqlx::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // ① exact id match (passthrough — the LLM may legitimately send
    //    an id it learned from another tool's description).
    if let Some(row) = crate::db::models::get_model(db, trimmed).await? {
        return Ok(Some(row.id));
    }
    // ② display_name reverse lookup (first match wins).
    let models = crate::db::list_models(db).await?;
    Ok(models
        .into_iter()
        .find(|m| m.model.display_name == trimmed)
        .map(|m| m.model.id))
}

pub(crate) async fn resolve_final_model(
    db: &SqlitePool,
    agent_name: &str,
    frontmatter_model: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    // ① DB override (highest priority).
    if let Some(mid) = get_subagent_model_override(db, agent_name).await? {
        return Ok(Some(mid));
    }
    // ② Frontmatter declaration (lowest priority declaration).
    Ok(frontmatter_model.map(str::to_string))
}

/// task 07-03-subagent-frontmatter-model: resolve the worker's
/// provider / context_window / display_name from `def.model`. Pure over
/// (catalog, db) so it's unit-testable without spinning up
/// `run_chat_loop` — the caller (`run_subagent`) holds the catalog read
/// lock and passes `&ProviderCatalog` here.
///
/// - `def_model=None` (or empty after trim) → inherit parent provider + ctx.
/// - `def_model=Some(mid)` + catalog hit → worker provider from catalog;
///   ctx/display from `get_model(mid)` (one DB roundtrip; ctx falls back
///   to parent if the row vanished between catalog build + now).
/// - `def_model=Some(mid)` + catalog miss → `warn!` + inherit parent.
pub(crate) async fn resolve_worker_provider(
    def_model: Option<&str>,
    parent_provider: &Arc<dyn Provider>,
    parent_ctx: u32,
    catalog: Option<&ProviderCatalog>,
    db: &SqlitePool,
) -> (Arc<dyn Provider>, u32, Option<String>) {
    let mid = match def_model.map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) => m,
        None => return (parent_provider.clone(), parent_ctx, None),
    };
    let hit: Option<Arc<dyn Provider>> = catalog.and_then(|c| c.get(mid).cloned());
    match hit {
        Some(p) => {
            let model_row = crate::db::models::get_model(db, mid).await.ok().flatten();
            let ctx = model_row
                .as_ref()
                .map(|m| m.context_window)
                .unwrap_or(parent_ctx);
            let disp = model_row.as_ref().map(|m| m.display_name.clone());
            (p, ctx, disp)
        }
        None => {
            tracing::warn!(
                model = mid,
                "subagent model not in catalog (deleted / provider api_key missing); \
                 falling back to parent provider"
            );
            (parent_provider.clone(), parent_ctx, None)
        }
    }
}

/// Resolve the project_id for a session. Best-effort DB lookup of
/// `sessions.project_id` — the worker's memory loader needs the
/// project_id to slot into the right MemoryCache entry.
async fn resolve_project_id(db: &SqlitePool, session_id: &str) -> String {
    match crate::db::load_session(db, session_id).await {
        Ok(Some(loaded)) => loaded.session.project_id,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                "run_subagent: failed to load session for project_id; falling back to empty"
            );
            String::new()
        }
    }
}

/// Resolve the project's MAIN repo path (the directory containing
/// `.git/`) for a session. L3b (2026-06-27): used by
/// `create_worker` / `destroy_worker` which need the main repo to
/// open libgit2 + manage linked worktrees.
///
/// This is distinct from `current_ctx.worktree_path` (which is the
/// PARENT SESSION's worktree — a linked worktree, NOT the main
/// repo). The project row's `path` field is the main repo path.
pub(crate) async fn resolve_project_main_path(db: &SqlitePool, session_id: &str) -> String {
    let project_id = resolve_project_id(db, session_id).await;
    if project_id.is_empty() {
        return String::new();
    }
    match crate::db::get_project(db, &project_id).await {
        Ok(Some(p)) => p.path,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                project_id = %project_id,
                "run_subagent: failed to load project for main path; falling back to empty"
            );
            String::new()
        }
    }
}

/// L3b (2026-06-27): create the worker's isolated git worktree.
/// Returns the on-disk path on success.
///
/// Resolves:
/// 1. The project's main repo path (`.git/` lives here) — needed
///    for `git::worktree::create_worker`'s libgit2 open.
/// 2. The worker worktree path (`<app_data_dir>/worktrees/
///    <project_uuid>/worker/<run_id>`).
/// 3. The base worktree (the parent session's worktree) — the
///    worker's branch is based off this worktree's HEAD commit.
///
/// On ANY error we return `Err` — the caller (`run_subagent`) fails
/// the dispatch (no fallback to non-isolated, per the PRD's Edge
/// Cases). Errors include: project not found, project main path
/// not a git repo, worktree creation libgit2 failure.
async fn create_worker_worktree(
    db: &SqlitePool,
    parent_session_id: &str,
    project_id: &str,
    worker_run_id: &str,
    app_data_dir: &std::path::Path,
    parent_worktree_path: &std::path::Path,
) -> Result<PathBuf, String> {
    // 1. Resolve the project's main repo path.
    let project_main_path = resolve_project_main_path(db, parent_session_id).await;
    if project_main_path.is_empty() {
        return Err("could not resolve the project's main repo path for the session".to_string());
    }
    let project_main = std::path::Path::new(&project_main_path);
    if !project_main.join(".git").exists() {
        return Err(format!(
            "project main path '{}' is not a git repository (no .git found)",
            project_main.display()
        ));
    }

    // 2. Compute the worker worktree path.
    let worker_wt_path =
        crate::git::worktree::worker_worktree_path(app_data_dir, project_id, worker_run_id);

    // 3. Create the worktree. `create_worker` self-heals any stale
    //    state for this run_id (orphan dir / stale branch / stale
    //    metadata), then creates branch `worker/<run_id>` off the
    //    parent session's worktree HEAD + checks out the worktree.
    crate::git::worktree::create_worker(
        project_main,
        &worker_wt_path,
        parent_worktree_path,
        worker_run_id,
    )
    .map_err(|e| e.to_string())?;

    Ok(worker_wt_path)
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.4 — 2026-07-08):
// pure workflow role-gate. Lives outside `run_subagent`
// so the gate logic is unit-testable without standing up
// the 25-arg signature.
//
// **Returns**: `Some(content)` when the dispatch must be
// denied (content is the tool_error body — the agent
// reads it and self-corrects on the next turn);
// `None` when the dispatch is allowed to proceed.
//
// **Side effect**: a single `tracing::warn!` per denial
// + per `force=true` bypass, so the audit log captures
// the overstep. No I/O, no LLM, no DB.
// ---------------------------------------------------------------------------
pub(crate) fn check_workflow_role_gate(
    workflow_ctx: Option<&WorkflowCtx>,
    subagent_name: &str,
    input: &serde_json::Value,
) -> Option<String> {
    let ctx = workflow_ctx?;
    // State derived from current_task.status; no
    // task → no gate (matches the breadcrumb-less
    // state at task bootstrap).
    let state = ctx.current_task.as_ref()?.status.as_str();

    // C5 (2026-07-28): resolve the state machine via the TASK's
    // owning plugin (`task_workflow_def`), not the session plugin.
    // A dev-created task keeps dev's role rules even when the
    // session switches to review mid-task — preventing the cross-
    // plugin dead-lock where review's `roles_by_state` has no entry
    // for dev's `planning` status (and vice versa).
    let task_def = &ctx.task_workflow_def;

    let allowed = crate::agent::workflow::allowed_roles(task_def, state)
        .iter()
        .any(|r| r == subagent_name);
    let forced = input
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if allowed {
        return None;
    }
    if forced {
        tracing::warn!(
            subagent = %subagent_name,
            state = %state,
            "run_subagent: role gate bypassed via force=true"
        );
        return None;
    }

    let allowed_in_state: Vec<String> =
        crate::agent::workflow::allowed_roles(task_def, state).to_vec();
    let allowed_str = if allowed_in_state.is_empty() {
        "(none)".to_string()
    } else {
        allowed_in_state.join(", ")
    };
    tracing::warn!(
        subagent = %subagent_name,
        state = %state,
        "run_subagent: role gate denied (workflow session)"
    );
    Some(format!(
        "Role gate denied: '{subagent}' is not allowed in state '{state}' \
         (allowed: {allowed_str}). Either transition to a state that \
         allows this role, or re-dispatch with force: true for a one-shot \
         override. Current breadcrumb: see messages[0].",
        subagent = subagent_name,
        state = state,
        allowed_str = allowed_str,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests_common::{commit_all_for_test, init_repo_for_test};

    // -----------------------------------------------------------------------
    // resolve_isolation truth table (PRD §"已闭合" merge semantics)
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_isolation_frontmatter_true_no_override_isolates() {
        // frontmatter `isolation: worktree` + dispatch omits → isolated.
        assert!(resolve_isolation(Some(true), None));
    }

    #[test]
    fn resolve_isolation_frontmatter_true_dispatch_false_opts_out() {
        // frontmatter `isolation: worktree` + dispatch `isolation: false`
        // → NOT isolated (LLM opted out).
        assert!(!resolve_isolation(Some(true), Some(false)));
    }

    #[test]
    fn resolve_isolation_frontmatter_none_dispatch_true_opts_in() {
        // frontmatter not declared + dispatch `isolation: true`
        // → isolated (LLM opted in).
        assert!(resolve_isolation(None, Some(true)));
    }

    #[test]
    fn resolve_isolation_frontmatter_false_dispatch_false_stays_shared() {
        // frontmatter `isolation: false` + dispatch `isolation: false`
        // → NOT isolated.
        assert!(!resolve_isolation(Some(false), Some(false)));
    }

    #[test]
    fn resolve_isolation_no_default_no_override_is_legacy_shared() {
        // frontmatter not declared + dispatch omits → NOT isolated
        // (legacy shared-cwd behavior — the researcher builtin path).
        assert!(!resolve_isolation(None, None));
    }

    #[test]
    fn resolve_isolation_dispatch_input_wins_over_frontmatter() {
        // Dispatch input always wins (precedence rule).
        assert!(resolve_isolation(Some(false), Some(true)));
        assert!(!resolve_isolation(Some(true), Some(false)));
    }

    // -----------------------------------------------------------------------
    // builtin SubagentDef isolation defaults
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_general_purpose_defaults_to_shared() {
        // B (2026-06-30): general-purpose ships with isolation = None
        // (shared) so a single serial dispatch reuses the parent cwd
        // (zero merge, matches Claude Code). Concurrent dispatch is
        // force-isolated in chat_loop's DispatchBatch::Concurrent branch
        // (gated by worker_is_writable) — concurrent-write safety no
        // longer relies on this default being Some(true).
        let g = super::super::lookup_subagent("general-purpose").expect("general-purpose exists");
        assert_eq!(g.isolation, None);
    }

    // -----------------------------------------------------------------------
    // worker_is_writable (B, 2026-06-30) — drives the concurrent-force-
    // isolate decision (only writable workers need a worktree when
    // dispatched concurrently).
    // -----------------------------------------------------------------------

    fn writable_def(name: &str, tools: &[&str]) -> crate::agent::subagent::SubagentDef {
        crate::agent::subagent::SubagentDef {
            name: name.to_string(),
            description: String::new(),
            system_prompt: String::new(),
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            isolation: None,
            model: None,
        }
    }

    #[test]
    fn worker_is_writable_empty_tools_inherits_full_set() {
        // Empty `tools` = inherit the full builtin set (write/shell) → writable.
        assert!(worker_is_writable(&writable_def("gp-like", &[])));
    }

    #[test]
    fn worker_is_writable_readonly_only_is_not_writable() {
        // A toolset that is exactly READONLY_TOOL_ALLOWLIST (researcher)
        // → not writable → concurrent dispatch stays shared (no write race).
        assert!(!worker_is_writable(&writable_def(
            "researcher-like",
            &["read_file", "grep", "glob", "list_dir", "web_fetch"]
        )));
    }

    #[test]
    fn worker_is_writable_with_write_tool_is_writable() {
        // A declared toolset containing a write tool → writable.
        assert!(worker_is_writable(&writable_def(
            "writer",
            &["read_file", "write_file"]
        )));
    }

    #[test]
    fn builtin_researcher_defaults_to_no_isolation() {
        // The researcher builtin ships with isolation = None (read-only
        // workers don't need a separate checkout — saves the per-
        // dispatch checkout cost).
        let r = super::super::lookup_subagent("researcher").expect("researcher exists");
        assert_eq!(r.isolation, None);
    }

    // -----------------------------------------------------------------------
    // task_with_env_hint (C, 2026-06-30)
    // -----------------------------------------------------------------------

    #[test]
    fn task_with_env_hint_isolated_appends_hint() {
        let out = task_with_env_hint("do the thing", true, "run-xyz");
        assert!(out.contains("do the thing"), "original task preserved");
        assert!(out.contains("ISOLATED git worktree"), "env hint present");
        assert!(out.contains("worker/run-xyz"), "run_id interpolated");
        assert!(out.contains("do NOT need to run"), "told not to commit");
    }

    #[test]
    fn task_with_env_hint_shared_is_unchanged() {
        let out = task_with_env_hint("do the thing", false, "run-xyz");
        assert_eq!(out, "do the thing");
    }

    // -----------------------------------------------------------------------
    // probe_worker_changes
    // -----------------------------------------------------------------------

    #[test]
    fn probe_worker_changes_empty_repo_reports_no_changes() {
        // A fresh worktree with no edits vs its base commit → no changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed an empty-repo-friendly initial commit so the worker
        // worktree has a base commit to branch from (create_worker
        // resolves `base_worktree_path`'s HEAD).
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        // Create a worker worktree off the project HEAD.
        let run_id = "probe-empty";
        let worker_wt = project.join("worker_empty");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(
            !changes.has_changes,
            "empty worktree should have no changes"
        );
        assert!(changes.summary.is_empty());
    }

    #[test]
    fn probe_worker_changes_with_edits_reports_changes() {
        // A worker worktree with an edited file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed a tracked file so the worker can modify it.
        std::fs::write(project.join("a.txt"), "v1").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-edits";
        let worker_wt = project.join("worker_edits");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Edit the tracked file in the worker's checkout.
        std::fs::write(worker_wt.join("a.txt"), "v2-from-worker").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(changes.has_changes, "edited worktree should report changes");
        assert!(
            changes.summary.contains("a.txt"),
            "summary should mention the changed file: {}",
            changes.summary
        );
    }

    #[test]
    fn probe_worker_changes_with_untracked_file_reports_changes() {
        // A worker worktree that added a new (untracked) file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed initial commit so create_worker has a base commit.
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-untracked";
        let worker_wt = project.join("worker_untracked");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Add an untracked file in the worker's checkout.
        std::fs::write(worker_wt.join("new_file.txt"), "fresh").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(
            changes.has_changes,
            "untracked file should count as a change"
        );
        assert!(
            changes.summary.contains("new_file.txt"),
            "summary should mention the untracked file: {}",
            changes.summary
        );
    }

    // -----------------------------------------------------------------------
    // resolve_worker_provider (task 07-03-subagent-frontmatter-model)
    // AC1 (hit swaps provider) / AC2 (None inherits) / AC3 (miss falls
    // back) / AC4 (ctx + display from model row).
    // -----------------------------------------------------------------------

    use crate::llm::provider::mock::MockProvider;
    use std::collections::HashMap;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    fn mock_provider() -> Arc<dyn Provider> {
        Arc::new(MockProvider::new(vec![]))
    }

    #[tokio::test]
    async fn resolve_worker_provider_none_inherits_parent() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let catalog: ProviderCatalog = HashMap::new();
        let (wp, ctx, disp) =
            resolve_worker_provider(None, &parent, 100_000, Some(&catalog), &pool).await;
        assert!(Arc::ptr_eq(&wp, &parent), "None model must inherit parent");
        assert_eq!(ctx, 100_000);
        assert!(disp.is_none());
    }

    #[tokio::test]
    async fn resolve_worker_provider_hit_swaps_provider() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let worker = mock_provider();
        let mut catalog: ProviderCatalog = HashMap::new();
        catalog.insert("model-worker".to_string(), worker.clone());
        let (wp, _ctx, _disp) = resolve_worker_provider(
            Some("model-worker"),
            &parent,
            100_000,
            Some(&catalog),
            &pool,
        )
        .await;
        assert!(
            !Arc::ptr_eq(&wp, &parent),
            "catalog hit must swap away from parent"
        );
        assert!(
            Arc::ptr_eq(&wp, &worker),
            "worker provider must be the catalog entry"
        );
    }

    #[tokio::test]
    async fn resolve_worker_provider_miss_falls_back_to_parent() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let catalog: ProviderCatalog = HashMap::new();
        let (wp, _ctx, disp) = resolve_worker_provider(
            Some("nonexistent-id"),
            &parent,
            100_000,
            Some(&catalog),
            &pool,
        )
        .await;
        assert!(
            Arc::ptr_eq(&wp, &parent),
            "catalog miss must fall back to parent"
        );
        assert!(disp.is_none());
    }

    #[tokio::test]
    async fn resolve_worker_provider_catalog_none_falls_back() {
        // catalog=None (tests, no AppHandle) + model=Some → parent.
        let pool = test_pool().await;
        let parent = mock_provider();
        let (wp, _, _) =
            resolve_worker_provider(Some("any-id"), &parent, 100_000, None, &pool).await;
        assert!(Arc::ptr_eq(&wp, &parent));
    }

    #[tokio::test]
    async fn resolve_worker_provider_hit_reads_ctx_and_display() {
        // AC4: hit + DB has the model row → ctx = model.context_window,
        // disp = display_name (NOT the parent's).
        let pool = test_pool().await;
        let provider_row = crate::db::providers::create_provider(
            &pool,
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com",
            "sk-test",
        )
        .await
        .unwrap();
        let model_row = crate::db::models::create_model(
            &pool,
            &provider_row.id,
            "claude-test",
            "Claude Test",
            None,
            None,
            false,
            50_000,
        )
        .await
        .unwrap();
        let worker = mock_provider();
        let mut catalog: ProviderCatalog = HashMap::new();
        catalog.insert(model_row.id.clone(), worker.clone());
        let parent = mock_provider();
        let (wp, ctx, disp) =
            resolve_worker_provider(Some(&model_row.id), &parent, 100_000, Some(&catalog), &pool)
                .await;
        assert!(Arc::ptr_eq(&wp, &worker));
        assert_eq!(ctx, 50_000, "ctx must come from the model row, not parent");
        assert_eq!(disp.as_deref(), Some("Claude Test"));
    }

    // -----------------------------------------------------------------------
    // resolve_final_model (task 07-03-subagent-per-agent-model-ui, 阶段 1)
    //
    // AC1 (UI: builtin override wins) / AC2 (DB > frontmatter) /
    // AC3 (frontmatter > parent) / AC4 (都无 → parent) / AC9 (DB miss
    // 指向失效 model: catalog miss 走 parent, NOT frontmatter fallback).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_final_model_db_wins_over_frontmatter() {
        // AC2: both DB and frontmatter declare a model → DB wins.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-db"));
    }

    #[tokio::test]
    async fn resolve_final_model_only_frontmatter() {
        // AC3: only frontmatter → frontmatter.
        let pool = test_pool().await;
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-fm"));
    }

    #[tokio::test]
    async fn resolve_final_model_only_db() {
        // Only DB → DB (frontmatter is None).
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-db"));
    }

    #[tokio::test]
    async fn resolve_final_model_neither_returns_none_for_parent_inheritance() {
        // AC4: no DB + no frontmatter → None (resolve_worker_provider
        // then inherits parent provider + ctx).
        let pool = test_pool().await;
        let got = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn resolve_final_model_dangling_db_override_still_returns_some() {
        // AC9 (priority chain invariant): `resolve_final_model` is
        // intentionally catalog-agnostic — a DB override pointing
        // at a deleted model STILL returns `Some(<deleted-id>)` so
        // the resolver can chain into `resolve_worker_provider`
        // (which logs `warn!` + falls back to parent on catalog
        // miss). The fall-back is NOT to frontmatter (per the
        // priority decision in the doc comment); it's directly to
        // parent. This test pins that behavior so a future refactor
        // doesn't silently change the catalog-miss path.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-deleted",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("model-deleted"),
            "DB override wins even when frontmatter is also set"
        );
        // The catalog-miss fallback to parent is tested in
        // `resolve_worker_provider_miss_falls_back_to_parent` above
        // (the same `model-id-not-in-catalog` case there covers the
        // downstream half of AC9).
    }

    // -----------------------------------------------------------------------
    // resolve_model_by_name_or_id (task 07-06-b6plus-b-dispatch-model-arg)
    //
    // B6+ B: the display_name→id reverse-lookup for the LLM-driven
    // dispatch path (schema `model` enum values are display_names).
    // AC1 (display_name→id) / id passthrough / miss→None.
    // -----------------------------------------------------------------------

    /// Helper: create a provider + model row, return the model row.
    async fn create_provider_and_model(
        pool: &SqlitePool,
        display_name: &str,
        model_name: &str,
        ctx: u32,
    ) -> crate::db::ModelRow {
        let provider_row = crate::db::providers::create_provider(
            pool,
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com",
            "sk-test",
        )
        .await
        .unwrap();
        crate::db::models::create_model(
            pool,
            &provider_row.id,
            model_name,
            display_name,
            None,
            None,
            false,
            ctx,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_id_passthrough() {
        // An exact model id is returned verbatim.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "Claude Test", "claude-test", 50_000).await;
        let got = resolve_model_by_name_or_id(&pool, &row.id).await.unwrap();
        assert_eq!(got.as_deref(), Some(row.id.as_str()));
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_display_name_lookup() {
        // A display_name resolves to the corresponding id.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "GPT-4o", "gpt-4o", 128_000).await;
        let got = resolve_model_by_name_or_id(&pool, "GPT-4o").await.unwrap();
        assert_eq!(got.as_deref(), Some(row.id.as_str()));
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_display_name_first_match_wins() {
        // Multiple models share a display_name → first match (by
        // list_models ordering) wins. Deterministic for a given DB
        // state; the design accepts this (display_name should be
        // unique but DB does not enforce it).
        let pool = test_pool().await;
        let _row_a = create_provider_and_model(&pool, "Dup", "dup-a", 50_000).await;
        let _row_b = create_provider_and_model(&pool, "Dup", "dup-b", 60_000).await;
        let got = resolve_model_by_name_or_id(&pool, "Dup").await.unwrap();
        assert!(got.is_some(), "duplicate display_name must still resolve");
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_miss_returns_none() {
        // Unknown display_name / id → Ok(None) (NOT an error).
        let pool = test_pool().await;
        let _row = create_provider_and_model(&pool, "Real", "real", 50_000).await;
        let got = resolve_model_by_name_or_id(&pool, "nonexistent")
            .await
            .unwrap();
        assert!(
            got.is_none(),
            "unknown input must resolve to None, not error"
        );
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_empty_returns_none() {
        // Empty / whitespace input → None (the parser filters these,
        // but the function is defensive).
        let pool = test_pool().await;
        assert!(resolve_model_by_name_or_id(&pool, "")
            .await
            .unwrap()
            .is_none());
        assert!(resolve_model_by_name_or_id(&pool, "   ")
            .await
            .unwrap()
            .is_none());
    }

    // -----------------------------------------------------------------------
    // Priority overlay (task 07-06-b6plus-b-dispatch-model-arg)
    //
    // The overlay `final_model = dispatch_model.or(resolved_lower)` lives
    // inside `run_subagent` as a one-liner; these tests pin the priority
    // semantics by exercising the composition directly. The dispatch_model
    // arm is the reverse-lookup result; resolved_lower is
    // `resolve_final_model`. Together: dispatch > DB > frontmatter > parent.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn priority_overlay_dispatch_overrides_db_override() {
        // AC2: dispatch_model=X + DB override=Y → final=X.
        let pool = test_pool().await;
        let row_x = create_provider_and_model(&pool, "X", "x", 50_000).await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model = Some(row_x.id.clone());
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some(row_x.id.as_str()));
    }

    #[tokio::test]
    async fn priority_overlay_dispatch_overrides_frontmatter() {
        // AC3: dispatch_model=X + frontmatter=Y (no DB) → final=X.
        let pool = test_pool().await;
        let row_x = create_provider_and_model(&pool, "X", "x", 50_000).await;
        let dispatch_model = Some(row_x.id.clone());
        let resolved_lower = resolve_final_model(&pool, "researcher", Some("fm-y"))
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some(row_x.id.as_str()));
    }

    #[tokio::test]
    async fn priority_overlay_none_dispatch_falls_to_db() {
        // AC4 zero-regression: no dispatch_model + DB override=Y → final=Y.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some("model-from-db-y"));
    }

    #[tokio::test]
    async fn priority_overlay_none_dispatch_none_db_falls_to_frontmatter() {
        // AC4: no dispatch + no DB → frontmatter.
        let pool = test_pool().await;
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", Some("fm-y"))
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some("fm-y"));
    }

    #[tokio::test]
    async fn priority_overlay_all_none_inherits_parent() {
        // AC4: no dispatch + no DB + no frontmatter → None (parent).
        let pool = test_pool().await;
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert!(final_model.is_none());
    }

    #[tokio::test]
    async fn priority_overlay_unknown_dispatch_display_name_becomes_none() {
        // AC7: the LLM sends a display_name that reverse-lookup misses
        // → dispatch_model=None → final falls to resolve_final_model.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model = resolve_model_by_name_or_id(&pool, "nonexistent-display")
            .await
            .unwrap();
        assert!(dispatch_model.is_none(), "miss must produce None");
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(
            final_model.as_deref(),
            Some("model-from-db-y"),
            "miss dispatch must degrade to the DB override, not parent"
        );
    }

    #[tokio::test]
    async fn priority_overlay_dispatch_miss_inherits_parent_when_no_lower() {
        // AC7: miss dispatch + no DB/frontmatter → None (parent).
        let pool = test_pool().await;
        let dispatch_model = resolve_model_by_name_or_id(&pool, "ghost").await.unwrap();
        assert!(dispatch_model.is_none());
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert!(final_model.is_none());
    }

    #[tokio::test]
    async fn priority_overlay_idempotent_across_dispatches() {
        // R3: per-dispatch override does NOT persist. Two identical
        // resolve_final_model calls (no DB write between) return the
        // same value — the dispatch overlay is per-call only.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "X", "x", 50_000).await;
        let resolved_1 = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        // Simulate a dispatch with model X (does not write DB/frontmatter).
        let _final_1 = Some(row.id.clone()).or(resolved_1.clone());
        let resolved_2 = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert_eq!(resolved_1, resolved_2, "dispatch must not persist");
    }

    #[tokio::test]
    async fn dispatch_input_model_field_parsed_as_dispatch_model() {
        // §3.2: `input.model` (a display_name, as the LLM would send
        // from the schema enum) reverse-resolves to an id. This mirrors
        // the parse logic in run_subagent: input.model → raw →
        // resolve_model_by_name_or_id → dispatch_model.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "GPT-4o", "gpt-4o", 128_000).await;
        let input = serde_json::json!({ "model": "GPT-4o" });
        let raw: Option<&str> = input
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let dispatch_model = match raw {
            Some(r) => resolve_model_by_name_or_id(&pool, r).await.unwrap(),
            None => None,
        };
        assert_eq!(dispatch_model.as_deref(), Some(row.id.as_str()));
    }

    // ---- Step 2.4: workflow role-gate (pure helper) ----
    //
    // These tests target `check_workflow_role_gate`
    // directly — the gate logic is a pure function so
    // no LLM mocks / provider wiring are required. The
    // integration with `run_subagent` is verified by
    // the existing dispatch tests + the manual end-to-end
    // checklist in `implement.md` Step 2.4 validation.

    use crate::agent::workflow::{Coordination, TaskJson, TaskStatus, WorkflowCtx, WorkflowDef};

    fn dev_workflow_def() -> WorkflowDef {
        WorkflowDef {
            name: "dev".to_string(),
            description: "test".to_string(),
            states: vec!["planning".into(), "in_progress".into(), "done".into()],
            initial: "planning".into(),
            transitions: vec![],
            roles_by_state: HashMap::from([
                ("planning".to_string(), vec!["researcher".to_string()]),
                (
                    "in_progress".to_string(),
                    vec!["implementer".to_string(), "checker".to_string()],
                ),
                ("done".to_string(), vec![]),
            ]),
            breadcrumb: HashMap::new(),
            delegation_templates: HashMap::new(),
            coordination: Coordination::Pipeline,
            gather_strategy: HashMap::new(),
        }
    }

    fn ctx_with_status(status: TaskStatus) -> WorkflowCtx {
        let workflow_def = dev_workflow_def();
        WorkflowCtx {
            task_workflow_def: workflow_def.clone(),
            workflow_def,
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: "x".into(),
                slug: "x".into(),
                status,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
                workflow_plugin: "dev".into(),
            }),
        }
    }

    /// Gate denial: planning session dispatching a role
    /// not in planning's allowed list → tool_error body.
    #[test]
    fn gate_denies_role_not_allowed_in_current_state() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        let msg = denial.expect("gate must deny general-purpose in planning");
        assert!(msg.contains("Role gate denied"), "got: {msg}");
        assert!(msg.contains("general-purpose"), "must name role");
        assert!(msg.contains("planning"), "must name state");
        assert!(msg.contains("researcher"), "must enumerate allowed");
    }

    /// Gate allowance: planning session dispatching
    /// `researcher` (in planning's allowed list) → None.
    #[test]
    fn gate_allows_role_in_current_state() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"subagent": "researcher"});
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        assert!(denial.is_none(), "researcher IS allowed in planning");
    }

    /// One-shot bypass: `force: true` overrides denial.
    #[test]
    fn gate_force_bypass_overrides_denial() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"force": true});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        assert!(
            denial.is_none(),
            "force=true must bypass the gate (got: {:?})",
            denial
        );
    }

    /// State-driven enforcement: same role, different
    /// states → different verdicts. Confirms the gate
    /// consults `current_task.status`, not just the role.
    #[test]
    fn gate_enforcement_is_state_driven() {
        let input = serde_json::json!({"subagent": "implementer"});

        // in_progress + implementer → allowed
        let ctx_impl = ctx_with_status(TaskStatus::InProgress);
        assert!(check_workflow_role_gate(Some(&ctx_impl), "implementer", &input).is_none());

        // in_progress + checker → also allowed (post-merge: both roles valid in in_progress)
        assert!(
            check_workflow_role_gate(Some(&ctx_impl), "checker", &input).is_none(),
            "checker must be allowed in in_progress post-merge"
        );

        // planning + implementer → denied
        let ctx_plan = ctx_with_status(TaskStatus::Planning);
        let denial = check_workflow_role_gate(Some(&ctx_plan), "implementer", &input);
        assert!(denial.is_some(), "implementer must be denied in planning");
    }

    /// Non-workflow short-circuit: `workflow_ctx = None`
    /// → gate does not engage (legacy dispatch
    /// behavior preserved).
    #[test]
    fn gate_short_circuits_when_no_workflow_ctx() {
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(None, "general-purpose", &input);
        assert!(
            denial.is_none(),
            "non-workflow session must short-circuit the gate",
        );
    }

    /// No-active-task short-circuit: workflow session with
    /// `current_task = None` (task bootstrap state) → no
    /// enforcement, no error.
    #[test]
    fn gate_short_circuits_when_no_current_task() {
        let workflow_def = dev_workflow_def();
        let ctx = WorkflowCtx {
            task_workflow_def: workflow_def.clone(),
            workflow_def,
            current_task: None,
        };
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        assert!(
            denial.is_none(),
            "no current_task (bootstrap state) must short-circuit the gate",
        );
    }

    /// Done-state enforcement: planning's `done` state
    /// has empty allowed roles; ANY dispatch is denied
    /// (mirrors the dev plugin's "done triggers archive"
    /// semantics — no further sub-agent work after done).
    #[test]
    fn gate_done_state_has_no_allowed_roles() {
        let ctx = ctx_with_status(TaskStatus::Done);
        let input = serde_json::json!({"subagent": "researcher"});
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        let msg = denial.expect("researcher must be denied in done");
        assert!(
            msg.contains("(none)"),
            "done's allowed list is empty: {msg}"
        );
    }

    /// C5 (2026-07-28): the role gate MUST use the task's owning
    /// plugin (`task_workflow_def`), not the session plugin. This
    /// test constructs the exact dead-lock scenario from session
    /// 04c62fab: a dev-created task (status=planning) opened in a
    /// review session. Before the fix, the gate queried review's
    /// `roles_by_state` with key "planning" → empty → denied all.
    /// After the fix, the gate uses dev's state machine and correctly
    /// allows `researcher` in `planning`.
    #[test]
    fn gate_uses_task_owning_plugin_not_session_plugin() {
        let dev_def = dev_workflow_def();
        // Minimal review workflow def: states don't include "planning",
        // so review's roles_by_state["planning"] is absent (empty).
        let review_def = crate::agent::workflow::WorkflowDef {
            name: "review".into(),
            description: String::new(),
            states: vec!["intake".into(), "reviewing".into()],
            initial: "intake".into(),
            transitions: vec![],
            roles_by_state: {
                let mut m = std::collections::HashMap::new();
                m.insert("reviewing".into(), vec!["reviewer".into()]);
                m
            },
            breadcrumb: std::collections::HashMap::new(),
            delegation_templates: std::collections::HashMap::new(),
            coordination: crate::agent::workflow::Coordination::Pipeline,
            gather_strategy: std::collections::HashMap::new(),
        };
        // Session is review, but task belongs to dev (status=planning).
        let ctx = WorkflowCtx {
            workflow_def: review_def,   // session plugin (review)
            task_workflow_def: dev_def, // task's owning plugin (dev)
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: "x".into(),
                slug: "x".into(),
                status: TaskStatus::Planning,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                completed_at: None,
                workflow_plugin: "dev".into(),
            }),
        };
        let input = serde_json::json!({"subagent": "researcher"});
        // dev's planning allows researcher — gate must pass even
        // though the session plugin (review) has no "planning" entry.
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        assert!(
            denial.is_none(),
            "role gate must use task's dev plugin (planning allows researcher), \
             not session's review plugin; got denial: {:?}",
            denial
        );
    }

    // ---- Step 2.7: workflow-aware dispatch resolution wiring ----
    //
    // `run_subagent`'s lookup branch (dispatch.rs ~line 377) now
    // routes to `lookup_with_workflow` when `workflow_ctx` carries
    // a plugin name, so the plugin `.everlasting/workflow/<wf>/agents/`
    // layer wins over builtin/user/project. Before Step 2.7 the
    // dispatch path always used the legacy `lookup`, so a plugin's
    // `researcher.md` (Step 2.3) was never loaded even though the
    // role-gate (Step 2.4) correctly *allowed* the role.
    //
    // `run_subagent` itself needs 25+ args + a live provider/db, so
    // a full end-to-end dispatch test is out of scope here. Instead
    // this test pins the cache-level contract the dispatch branch
    // depends on: a plugin-only agent body is reachable via
    // `lookup_with_workflow` under the workflow name the dispatch
    // branch reads from `workflow_ctx.workflow_def.name`. If this
    // test regresses, the dispatch branch silently falls back to
    // the builtin body (the exact Step 2.7 bug class).

    #[tokio::test]
    async fn workflow_dispatch_resolves_plugin_agent_body() {
        use crate::agent::subagent::SubagentSource;

        // Plugin agent lives ONLY in the workflow plugin layer.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let plugin_dir = proj_tmp
            .path()
            .join(".everlasting")
            .join("workflow")
            .join("dev")
            .join("agents");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("researcher.md"),
            "---\nname: researcher\ndescription: \"\"\n---\nPLUGIN_BODY_STEP27",
        )
        .unwrap();

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        // Workflow name pulled from `WorkflowCtx.workflow_def.name`
        // exactly as dispatch.rs ~line 376 does.
        let wf_name = dev_workflow_def().name; // "dev"
        let loaded = cache
            .lookup_with_workflow(&project_path, Some(&wf_name), "researcher")
            .await
            .expect("plugin researcher must resolve via lookup_with_workflow");
        assert_eq!(
            loaded.source,
            SubagentSource::Plugin,
            "dispatch branch must see the plugin layer, not the builtin",
        );
        assert!(
            loaded.def.system_prompt.contains("PLUGIN_BODY_STEP27"),
            "dispatch branch must load the plugin body, not the builtin body",
        );
    }

    // ----- C1 (07-26-subagent-resume): build_clarification_message -----

    #[test]
    fn c1_clarification_none_when_field_absent() {
        let input = serde_json::json!({"task": "do thing"});
        assert!(build_clarification_message(&input).is_none());
    }

    #[test]
    fn c1_clarification_none_without_purpose() {
        // this_round_purpose is required — without it the clarification
        // is meaningless, so we drop it (resume proceeds with just the
        // replayed history + task, no clarification stanza).
        let input = serde_json::json!({
            "resume_clarification": {"current_state": "x"}
        });
        assert!(build_clarification_message(&input).is_none());
    }

    #[test]
    fn c1_clarification_full_stanza_with_all_fields() {
        let input = serde_json::json!({
            "resume_clarification": {
                "current_state": "PRD revised: scope trimmed to MVP",
                "changes_since_last": ["§2 scope reduced", "§4 added acceptance criteria"],
                "this_round_purpose": "verify the high-severity findings are resolved"
            }
        });
        let msg = build_clarification_message(&input).expect("present");
        match &msg.content {
            crate::llm::types::MessageContent::Text(t) => {
                assert!(t.contains("[resume clarification"));
                assert!(t.contains("**Current state:** PRD revised"));
                assert!(t.contains("**Changes since your last turn:**"));
                assert!(t.contains("- §2 scope reduced"));
                assert!(t.contains("- §4 added acceptance criteria"));
                assert!(t.contains("**This round's purpose:** verify the high-severity"));
            }
            other => panic!("expected Text, got {:?}", other),
        }
        assert_eq!(msg.role, crate::llm::types::Role::User);
    }

    #[test]
    fn c1_clarification_omits_empty_optional_sections() {
        // current_state empty + changes_since_last empty/missing →
        // those sections are dropped; only the header + purpose remain.
        let input = serde_json::json!({
            "resume_clarification": {
                "current_state": "",
                "this_round_purpose": "just check again"
            }
        });
        let msg = build_clarification_message(&input).expect("present");
        match &msg.content {
            crate::llm::types::MessageContent::Text(t) => {
                assert!(t.contains("[resume clarification"));
                assert!(!t.contains("**Current state:**"));
                assert!(!t.contains("**Changes since your last turn:**"));
                assert!(t.contains("**This round's purpose:** just check again"));
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }
}
