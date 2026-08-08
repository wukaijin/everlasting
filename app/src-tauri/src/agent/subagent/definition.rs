//! `dispatch_subagent` tool definition (static + cache-backed dynamic).
//!
//! Split out of `agent/subagent/mod.rs` (2026-08-08 batch3).

use crate::llm::ToolDef;

use super::{ModelBrief, SubagentCache};

/// The `dispatch_subagent` tool definition. Registered in
/// `tools::builtin_tools()` so the LLM can discover it + go through
/// the ⑨ 关 permission check. The **execution path is
/// intercepted** in `chat_loop.rs`'s tool dispatch — this ToolDef
/// is discovery-only; the actual `run_subagent` call is in
/// [`super::dispatch::run_subagent`] (see PRD §"Technical Approach" and review #3).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "dispatch_subagent".to_string(),
        description: Some(
            "Dispatch a worker subagent to run a sub-task in its own isolated context \
             (independent messages, independent turn budget). The worker runs to \
             completion (synchronous — the parent chat blocks until the worker \
             returns). When the worker finishes, its final summary is injected as \
             the tool_result of this call. Use this for focused sub-tasks that \
             would otherwise pollute the main conversation context with verbose \
             search / exploration output. Two built-in subagents are available: \
             `researcher` (read-only: read_file / grep / glob / list_dir / \
             web_fetch) and `general-purpose` (full toolset minus dispatch_subagent \
             / update_checklist / background-shell tools). The worker inherits the \
             parent's permission Mode.\n\n\
             B (2026-06-30): worktree isolation is decided automatically by the \
             system based on dispatch shape — you usually do NOT need to set \
             `isolation`. A single dispatch_subagent per turn runs in the parent \
             cwd (shared: edits land immediately, zero merge). Multiple \
             dispatch_subagent calls in one turn run concurrently and \
             write-capable workers are auto-isolated to their own \
             `worker/<run_id>` branch so concurrent writes never race (each \
             worker's edits merge back via `merge_worker`). The optional \
             `isolation` input overrides this: `true` forces a worktree even for \
             a single dispatch; `false` forces shared-cwd. Omit for the system \
             default."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "subagent": {
                    "type": "string",
                    "enum": ["researcher", "general-purpose"],
                    "description": "Which built-in subagent to dispatch."
                },
                "task": {
                    "type": "string",
                    "description": "The delegation prompt for the worker. The worker \
                                    starts with a fresh context containing ONLY this \
                                    task string + the project memory files — it does \
                                    NOT inherit the parent's conversation history. \
                                    Write the task as a self-contained brief."
                },
                "isolation": {
                    "type": "boolean",
                    "description": "Override the system's automatic worktree-isolation \
                                    decision. A single dispatch defaults to shared-cwd \
                                    (edits land immediately); concurrent dispatch \
                                    (multiple dispatch_subagent in one turn) auto-isolates \
                                    write-capable workers. `true` forces a worktree even \
                                    for a single dispatch; `false` forces shared-cwd. Omit \
                                    for the system default."
                },
                "model": {
                    "type": "string",
                    "enum": [],
                    "description": "Override the worker's model for THIS dispatch only \
                                    (does not persist). The dynamic enum lists available \
                                    model display names; this static `definition()` has \
                                    no model context so its enum is empty. When omitted, \
                                    the worker uses its configured default (Settings \
                                    per-agent override > frontmatter `model:` > parent's \
                                    model). Use this for cross-model adversarial review."
                }
            },
            "required": ["subagent", "task"]
        }),
    }
}

/// The canonical name of the dispatch tool. Used by the
/// interceptor in `chat_loop.rs` to recognize it.
pub const DISPATCH_TOOL_NAME: &str = "dispatch_subagent";

/// L3d PR3 (2026-06-25): the dynamic, cache-backed
/// `dispatch_subagent` ToolDef. Replaces the static `definition()`
/// at the per-turn tool list construction site (`chat_loop.rs:957`)
/// so the LLM's enum reflects builtin + user + project subagents
/// merged by [`SubagentCache::list`] (mtime-fenced scan).
///
/// - The `enum` is built from `cache.list(project_path)` — every
///   subagent's `def.name`, sorted alphabetically by the loader.
/// - The description appends a per-subagent `Available subagents:`
///   line carrying the source tag (`builtin` / `user` / `project`)
///   + the subagent's own `description` field. The LLM uses the
///   source tag for debugging (it does not affect dispatch
///   routing); the description helps the LLM pick the right agent.
/// - The static `definition()` is kept for the existing unit tests
///   (`definition_*`) + any caller that wants the no-cache version;
///   the dynamic path is the production path.
///
/// `project_path` is the canonical worktree path (same string the
/// agent loop uses for memory / skill lookups — see `chat_loop.rs`
/// `worktree_path`). The cache is read-through + mtime-fenced, so
/// adding / editing / deleting a `.md` is picked up on the next
/// chat turn without a reload command.
pub async fn definition_with_cache(
    cache: &SubagentCache,
    project_path: &str,
    workflow_name: Option<&str>,
    models: &[ModelBrief],
) -> ToolDef {
    // C5 (2026-07-27): MUST consult the workflow-aware list so the
    // plugin-layer agents (e.g. review plugin's `reviewer`) appear in
    // the subagent enum. The previous `cache.list` only merged 3
    // layers (builtin + user + project) and silently dropped plugin
    // agents — so the LLM could never pick `reviewer`, while the
    // role gate (which reads `roles_by_state` from the same plugin)
    // demanded it. That mismatch dead-locked the entire review
    // workflow (session 04c62fab). `list_with_workflow` adds the two
    // plugin layers (builtin-plugin + project-plugin) and degrades to
    // the same 3-layer merge when `workflow_name` is None/empty.
    let loaded = cache.list_with_workflow(project_path, workflow_name).await;
    let names: Vec<String> = loaded.iter().map(|l| l.def.name.clone()).collect();
    // B6+ B: the `model` enum values are display_names (human-readable;
    // the system prompt does not list models, so this enum is the
    // LLM's only discovery channel). The id↔display_name mapping is
    // resolved at dispatch time by `resolve_model_by_name_or_id`.
    let model_names: Vec<String> = models.iter().map(|m| m.display_name.clone()).collect();

    // Build the `Available subagents:` line. Each entry carries
    // the source tag + the subagent's own description (truncated
    // for brevity if long). Sorted alphabetically by name (the
    // loader already sorts; we re-derive for safety).
    let mut entries: Vec<String> = loaded
        .iter()
        .map(|l| {
            let desc = l.def.description.trim();
            if desc.is_empty() {
                format!("{} (source: {})", l.def.name, l.source.as_str())
            } else {
                // Truncate long descriptions at one line (~80 chars)
                // so the tool description stays scannable.
                let one_line: String = desc.lines().next().unwrap_or("").trim().to_string();
                let clipped = if one_line.chars().count() > 80 {
                    let cutoff: String = one_line.chars().take(77).collect();
                    format!("{}...", cutoff)
                } else {
                    one_line
                };
                format!(
                    "{} (source: {}): {}",
                    l.def.name,
                    l.source.as_str(),
                    clipped
                )
            }
        })
        .collect();
    entries.sort();

    let base = definition();
    let available_line = if entries.is_empty() {
        // Defensive — builtins are always present, so this is
        // unreachable in practice; keep the description honest
        // rather than listing an empty set.
        "Available subagents: (none).".to_string()
    } else {
        format!("Available subagents: {}.", entries.join("; "))
    };
    let description = format!(
        "{}\n\n{}",
        base.description.unwrap_or_default(),
        available_line
    );

    ToolDef {
        name: DISPATCH_TOOL_NAME.to_string(),
        description: Some(description),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "subagent": {
                    "type": "string",
                    "enum": names,
                    "description": "Which subagent to dispatch. Source tag (builtin/user/project) \
                                    is informational; the worker inherits the parent's permission \
                                    Mode regardless of source."
                },
                "task": {
                    "type": "string",
                    "description": "The delegation prompt for the worker. The worker \
                                    starts with a fresh context containing ONLY this \
                                    task string + the project memory files — it does \
                                    NOT inherit the parent's conversation history. \
                                    Write the task as a self-contained brief."
                },
                "isolation": {
                    "type": "boolean",
                    "description": "Override the system's automatic worktree-isolation \
                                    decision. A single dispatch defaults to shared-cwd \
                                    (edits land immediately); concurrent dispatch \
                                    (multiple dispatch_subagent in one turn) auto-isolates \
                                    write-capable workers. `true` forces a worktree even \
                                    for a single dispatch; `false` forces shared-cwd. Omit \
                                    for the system default."
                },
                "model": {
                    "type": "string",
                    "enum": model_names,
                    "description": "Override the worker's model for THIS dispatch only \
                                    (does not persist). Pick a value from the enum (a model \
                                    display name). When omitted, the worker uses its \
                                    configured default (Settings per-agent override > \
                                    frontmatter `model:` > parent's model). Use this for \
                                    cross-model adversarial review (e.g. dispatch reviewer \
                                    with a stronger / different-family model)."
                },
                "resume_from": {
                    "type": "string",
                    "description": "Resume a prior worker run by replaying its conversation \
                                    history as this worker's initial messages (saves the \
                                    worker re-reading the same context). Value is the prior \
                                    run's id (subagent_runs.id, returned in prior \
                                    dispatch_subagent results). Omit for a fresh dispatch \
                                    (default). Restrictions: the prior run must be in the \
                                    SAME session, must be finished (not still running), and \
                                    must have a non-truncated message history. On any \
                                    violation the dispatch falls back to a fresh worker and \
                                    appends `[resume: fallback, reason: <code>]` to the result."
                },
                "resume_clarification": {
                    "type": "object",
                    "description": "Context update injected at the resume point so the \
                                    continued worker can reconcile stale references in the \
                                    replayed history. Required when `resume_from` is set \
                                    (a resumed worker needs to know what changed). Ignored \
                                    when `resume_from` is omitted.",
                    "properties": {
                        "current_state": {
                            "type": "string",
                            "description": "Short summary of the current state (e.g. the \
                                            revised PRD's key points) so the worker can \
                                            orient without re-reading everything."
                        },
                        "changes_since_last": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Explicit list of what changed since the prior \
                                            run's last turn (e.g. revised sections, new \
                                            decisions). The worker treats prior-history \
                                            references that contradict these as stale."
                        },
                        "this_round_purpose": {
                            "type": "string",
                            "description": "What this resumed round is for (e.g. 'verify the \
                                            high-severity findings from last round are \
                                            resolved in the revised PRD')."
                        }
                    },
                    "required": ["this_round_purpose"]
                }
            },
            "required": ["subagent", "task"]
        }),
    }
}
