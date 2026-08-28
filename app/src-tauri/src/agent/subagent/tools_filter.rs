//! Worker tool allowlist + structural-disabled filter.
//!
//! Split out of `agent/subagent/mod.rs` (2026-08-08 batch3).

use crate::llm::ToolDef;

use super::registry::SubagentDef;

/// Tools that are **structurally disabled** for every worker,
/// regardless of the SubagentDef's allowlist. Mirrors the
/// `update_checklist` / `dispatch_subagent` (no nesting) / L1a
/// background-shell trio.
///
/// - `update_checklist` is the parent's progress tracker — a
///   worker scribbling into it would corrupt the parent's plan.
/// - `dispatch_subagent` is disabled to keep MVP single-layer
///   (research §4 / PRD §OOS).
/// - The 3 L1a tools (`run_background_shell` / `shell_status` /
///   `shell_kill`) are session-scoped: their completion
///   notifications are drained per-session at the start of every
///   parent turn. A worker starting a background shell would leave
///   its notification in the same session queue, leaking into the
///   parent's next-turn drain.
pub(crate) const STRUCTURALLY_DISABLED: &[&str] = &[
    "update_checklist",
    // 07-10-workflow-task-json-hardening R2: only the parent LLM
    // creates workflow tasks — a worker must not seed a new
    // `.everlasting/tasks/<slug>/` (same rationale as update_checklist:
    // task lifecycle is the orchestrator's, not a worker's).
    "create_task",
    "dispatch_subagent",
    "run_background_shell",
    "shell_status",
    "shell_kill",
    // L3b PR3 B3 fix (2026-06-28): only the parent LLM / user (via the
    // PR4 SubagentDrawer) may merge or discard a worker branch — a
    // worker must not rewrite the parent session's history (it could
    // otherwise merge a SIBLING worker's branch using a run_id visible
    // in the dispatch tool_result). Stripped unconditionally.
    "merge_worker",
    "discard_worker",
    // 2026-06-30 (`ask_user_question` task): worker subagents must
    // NOT block on user input. Worker has no UI sink (the
    // `WorkerAskBanner` affordance is for `permission:ask` style
    // Tier-4 decisions, not for an interactive Q&A card); the
    // blocking oneshot would hang the worker's tokio task
    // forever (or until parent cancel). Stripped here as the
    // first line of defense; the per-turn tool-list construction
    // in `chat_loop.rs` also gates any per-turn dynamic append
    // on `effective_is_worker == false` (mirroring the
    // `dispatch_subagent` no-nesting pattern).
    "ask_user_question",
    // 2026-07-07 (`request_mode_change` task): same rationale as
    // `ask_user_question` above — worker subagents must not block
    // on user input (the worker has no UI sink, the blocking
    // oneshot would hang the worker's tokio task until parent
    // cancel). Worker subagents that want to suggest a mode
    // change must return a result and let the parent surface
    // the change request on the user's behalf.
    "request_mode_change",
    // 2026-07-08 (`07-08-workflow-integration` Phase 3 Step 3.1):
    // same rationale as `ask_user_question` /
    // `request_mode_change` above — worker subagents must not
    // block on user input (no UI sink, the blocking oneshot
    // would hang the worker's tokio task until parent
    // cancel). Worker subagents that want to suggest a state transition
    // must return a result and let the parent surface
    // the transition request on the user's behalf.
    "request_task_state_transition",
    // 08-29-schedule-task-tool: the scheduling family is the
    // orchestrator's surface (same rationale as `create_task` — a
    // worker must not seed detached tasks; task lifecycle belongs to
    // the parent / user).
    "schedule_task",
    "schedule_status",
    "schedule_cancel",
];

/// Filter `builtin_tools()` for a worker.
///
/// - If `def.tools` is empty, start from the full `builtin_tools()`
///   set (the general-purpose convention).
/// - Otherwise start from the allowlist.
/// - Then strip [`STRUCTURALLY_DISABLED`] unconditionally (so a
///   future frontmatter can't accidentally re-enable nesting or
///   the L1a trio).
pub fn filter_tools_for_subagent(all_tools: Vec<ToolDef>, def: &SubagentDef) -> Vec<ToolDef> {
    let allow: Option<std::collections::HashSet<&str>> = if def.tools.is_empty() {
        None
    } else {
        Some(def.tools.iter().map(|s| s.as_str()).collect())
    };
    all_tools
        .into_iter()
        .filter(|t| {
            // Strip structural-disabled ALWAYS.
            if STRUCTURALLY_DISABLED.contains(&t.name.as_str()) {
                return false;
            }
            // If an allowlist is set, also require membership.
            match &allow {
                Some(set) => set.contains(t.name.as_str()),
                None => true,
            }
        })
        .collect()
}

/// Tool names permitted in the **read-only** worker toolset (L3a,
/// 2026-06-24; `web_fetch` added 2026-06-25, task
/// 06-25-subagent-web-access; `search_history` added 2026-08-17,
/// D2②). This is the **runtime-forced read-only layer** (the 2nd of
/// 3 — see L3a PRD "只读保证三层"): when multiple workers run
/// concurrently in a pure dispatch batch, the concurrent branch
/// forces every worker's toolset down to just these read-only tools
/// regardless of its `SubagentDef` allowlist. `web_fetch` is kept —
/// it is a read-only network op, `Risk::Low`, and SSRF-guarded in
/// `tools/web_fetch.rs`; a worker's `web_fetch` still goes through
/// the Tier 4 permission check, inheriting the parent session's
/// `web_fetch` grant or surfacing a `WorkerAskBanner`.
/// `search_history` is a read-only DB query (Tier 5 silent Allow),
/// so concurrent read-only workers keep it — serial `general-
/// purpose` workers have it via `builtin_tools()` (not in
/// `STRUCTURALLY_DISABLED`); the builtin `researcher`'s hardcoded
/// 5-tool `SubagentDef.tools` was deliberately NOT extended (its
/// system prompt enumerates its tools; frontmatter agents can opt
/// in), so researcher's allowlist is no longer exactly this list.
/// The safety baseline is still the `is_worker: true` permission
/// layer (worker asks route through `WorkerAskBanner` since the
/// 2026-06-22 RULE-FrontSubagent-003 fix — they no longer collapse
/// to `Deny`; 3rd layer) — `filter_tools_readonly` is
/// defense-in-depth that keeps the concurrent branch's tool
/// discovery surface aligned with its read-only contract so the LLM
/// never even sees a write tool in the concurrent path.
pub const READONLY_TOOL_ALLOWLIST: &[&str] = &[
    "read_file",
    "grep",
    "glob",
    "list_dir",
    "web_fetch",
    "search_history",
    // F4 (2026-08-25): snippet-only web search — read-only fixed-endpoint
    // network op (no user-controllable URL → no SSRF surface), Tier 5
    // silent Allow via `ToolKind::Other`. Pairs naturally with `web_fetch`
    // for "search then read" research flows in concurrent read-only workers.
    "web_search",
];

/// Force a worker's toolset down to read-only tools only (L3a,
/// 2026-06-24). Applied by the concurrent dispatch branch in
/// `chat_loop.rs` AFTER `filter_tools_for_subagent` so the
/// concurrent batch's workers can never see a write or shell tool
/// (`web_fetch` is kept — read-only network op, see
/// `READONLY_TOOL_ALLOWLIST`). Mirrors the `STRUCTURALLY_DISABLED`
/// filter pattern (same `.filter(|t| allowlist.contains(t.name))`
/// shape).
///
/// `researcher` is unaffected (its `SubagentDef.tools` allowlist is
/// the original 5 research tools — see `READONLY_TOOL_ALLOWLIST`
/// for the D2② divergence note); `general-purpose` is downgraded
/// from its full-minus-disabled set to just the read-only tools
/// above. Returns a fresh `Vec<ToolDef>` (consumes the input).
pub fn filter_tools_readonly(tools: Vec<ToolDef>) -> Vec<ToolDef> {
    tools
        .into_iter()
        .filter(|t| READONLY_TOOL_ALLOWLIST.contains(&t.name.as_str()))
        .collect()
}
