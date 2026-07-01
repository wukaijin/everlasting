//! B6 Subagent — worker agent dispatch + dispatch_subagent ToolDef.
//!
//! `dispatch_subagent` is an **agent-layer control-flow tool**, NOT a
//! regular I/O tool (PRD §"Technical Approach" / research review #3,
//! 2026-06-19). It is registered as a `ToolDef` so the LLM can
//! discover it + go through the ⑨ 关 permission check, but its
//! **execution is intercepted in `chat_loop.rs`'s tool_use handling
//! loop**, NOT routed through `tools::execute_tool` (whose inner
//! dispatch signature has no access to `provider` / `db` /
//! `cancellations`).
//!
//! The interception path:
//!
//! 1. LLM emits `dispatch_subagent({ subagent, task })`.
//! 2. `chat_loop::run_chat_loop`'s tool dispatch sees
//!    `name == "dispatch_subagent"` and calls
//!    `run_subagent` with the full closure dependencies
//!    (provider / db / cancellations / ...).
//! 3. `run_subagent` builds a worker context:
//!    `[memory_blocks (cache_control), delegation_task]` (task
//!    APPENDed, NOT prepended — see prompt-cache invariant in the
//!    PRD).
//! 4. It calls `run_chat_loop` recursively with a fresh rid, a
//!    `CancellationGuard { skip_session_active: true }` (so
//!    worker Drop doesn't evict the parent's
//!    `session_active_request[session_id]`), a worker
//!    `PermissionContext { is_worker: true }`, and
//!    `max_turns: Some(20)`.
//! 5. The worker's `ChatEventSink` is a [`SubagentBufferSink`] —
//!    it records the worker's chat-events / tool calls / tool
//!    results **into an in-memory transcript** but does NOT
//!    forward them to the parent's frontend (otherwise the main
//!    UI would be flooded by worker streams).
//! 6. When the worker exits, `run_subagent` extracts its final
//!    assistant text (the summary) and returns a `(content,
//!    is_error, status)` triple to the parent loop, which
//!    builds a `ContentBlock::ToolResult` (tool_use/tool_result
//!    pairing preserved — same invariant as RULE-A-007).
//!
//! # Why a separate module?
//!
//! The SubagentDef registry, prompt assembly, tool allowlist
//! filtering, and `SubagentBufferSink` all have well-scoped unit
//! tests; keeping them out of `chat_loop.rs` lets the loop stay
//! focused on turn orchestration. The `run_subagent` helper
//! itself lives in the [`dispatch`] submodule — it captures
//! `run_chat_loop`'s closure dependencies (the helper calls
//! `run_chat_loop` recursively and thus needs the same parameter
//! set the parent loop was invoked with).
//!
//! # Submodules (2026-06-23 split)
//!
//! This module was split out of a single 3402-line `subagent.rs`
//! into a directory so each concern has its own scoped unit tests:
//! - [`sink`] — `SubagentBufferSink` (worker-side `ChatEventSink`
//!   + `TEST_COLLECTOR`).
//! - [`transcript`] — `TranscriptEntry` / `TranscriptKind` +
//!   `subagent:event` / `subagent:finished` IPC payload builders.
//! - [`truncate_summary`] — transcript 4 MiB cap,
//!   `format_final_text` / `format_dispatch_result`, and
//!   `summarize_worker_tool_actions`.
//!
//! `mod.rs` (this file) keeps the dispatch tool definition,
//! `SubagentDef` registry, prompt assembly, tool allowlist
//! filtering, and the `SubagentStatus` enum. All items the rest
//! of the crate reaches via `crate::agent::subagent::*` are
//! re-exported below, so the split is invisible to callers.

use std::sync::Arc;

use crate::llm::types::MessageContent;
use crate::llm::{ChatMessage, Role, ToolDef};
use crate::memory::MemoryCache;

mod sink;
mod transcript;
mod truncate_summary;
mod loader;
pub(crate) mod dispatch;

// L3d PR2 (2026-06-25): re-export the loader's public surface so
// callers reach it via `crate::agent::subagent::{SubagentCache,
// LoadedSubagent, SubagentSource}` (mirrors the B3 / B4 re-export
// convention). PR3 lights up the call sites (`AppState` field,
// `dispatch.rs::run_subagent` lookup, `tools::definition_with_cache`).
// `LoadedSubagent` / `SubagentSource` are part of the public API
// surface but the only production consumer right now is
// `definition_with_cache` (via the local `loaded: Vec<LoadedSubagent>`);
// the `#[allow(unused_imports)]` keeps the re-export contract
// visible to future consumers without churn.
#[allow(unused_imports)]
pub use loader::{LoadedSubagent, SubagentCache, SubagentSource};
pub use sink::SubagentBufferSink;
pub use transcript::TranscriptEntry;
// `TranscriptKind` is referenced only from `cfg(test)` code
// (`db/tests.rs`, `agent/tests.rs`); production callers reach it via
// the module-internal `super::transcript::TranscriptKind` path.
#[cfg(test)]
pub use transcript::TranscriptKind;
pub(crate) use transcript::build_subagent_finished_payload;
pub use truncate_summary::{
    format_dispatch_result, format_final_text, summarize_worker_tool_actions,
    truncate_transcript_for_persistence, TRANSCRIPT_MAX_BYTES,
};

// ---------------------------------------------------------------------------
// Forced dispatch (explicit-agent-dispatch, 2026-06-30)
// ---------------------------------------------------------------------------

/// A **user-forced** subagent dispatch, parsed by the frontend from an
/// `@@<agent> <task>` input prefix and threaded through the `chat`
/// Tauri command → `run_chat_loop`'s turn-1 prefix short-circuit.
///
/// Unlike the LLM-driven `dispatch_subagent` tool_use, this path
/// **bypasses `provider.stream` entirely** — the parent loop never
/// asks the LLM which agent to run; the user already decided. The
/// turn-1 prefix synthesizes a `dispatch_subagent` tool_use from this
/// struct and calls [`dispatch::run_subagent`] directly (same code
/// path as the LLM-driven interceptor at `chat_loop.rs:2374`), then
/// emits the worker's summary as the turn's assistant text and exits.
///
/// Fields are `snake_case` to match the JS wire object
/// (`{ subagent, task }`) verbatim; the surrounding Tauri command arg
/// is `forcedDispatch` (camelCase, like `resendSeq`) and serde-converts.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ForcedDispatch {
    /// The subagent name (must exist in `SubagentCache`: builtin +
    /// user + project). The frontend validates before send; the
    /// backend trusts it (an unknown name surfaces as
    /// `run_subagent`'s error content, same as an LLM naming a
    /// nonexistent worker).
    pub subagent: String,
    /// The self-contained task brief for the worker (the text after
    /// the `@@<agent>` prefix). Written into the synthesized
    /// tool_use's `input.task` verbatim.
    pub task: String,
}

// ---------------------------------------------------------------------------
// Dispatch tool definition
// ---------------------------------------------------------------------------

/// The `dispatch_subagent` tool definition. Registered in
/// `tools::builtin_tools()` so the LLM can discover it + go through
/// the ⑨ 关 permission check. The **execution path is
/// intercepted** in `chat_loop.rs`'s tool dispatch — this ToolDef
/// is discovery-only; the actual `run_subagent` call is in
/// [`dispatch::run_subagent`] (see PRD §"Technical Approach" and review #3).
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
pub async fn definition_with_cache(cache: &SubagentCache, project_path: &str) -> ToolDef {
    let loaded = cache.list(project_path).await;
    let names: Vec<String> = loaded.iter().map(|l| l.def.name.clone()).collect();

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
                }
            },
            "required": ["subagent", "task"]
        }),
    }
}

// ---------------------------------------------------------------------------
// SubagentDef registry
// ---------------------------------------------------------------------------

/// One built-in subagent definition. MVP ships 2 (`researcher` +
/// `general-purpose`); a future PR will load these from Markdown
/// frontmatter (`.everlasting/agents/*.md`, mirroring `.claude/agents/*.md`).
///
/// - `tools` is an **allowlist** — the worker only sees the tools
///   named here. The interceptor additionally strips the
///   structural-disabled set (see `filter_tools_for_subagent`) so
///   even if a future frontmatter definition named
///   `update_checklist` / `dispatch_subagent` / the L1a triple,
///   they would still be removed.
/// - `system_prompt` **fully replaces** the parent's behavior_prompt
///   layer — the worker does NOT inherit the main system prompt
///   (Claude Code convention, see PRD §Decisions 6 + research §5).
#[derive(Clone, Debug)]
pub struct SubagentDef {
    pub name: String,
    /// User-facing description. Consumed by L3d PR3's
    /// `definition_with_cache` to render the per-subagent source
    /// tag + summary in the `dispatch_subagent` tool description
    /// (so the LLM sees builtin + user + project agents with their
    /// provenance). Also kept on the struct so the frontmatter
    /// loader (PR2) can populate it from the Markdown front-matter.
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    /// L3b (2026-06-27): per-agent default for worktree isolation.
    /// When `Some(true)`, workers dispatched under this subagent
    /// run in an isolated git worktree (independent checkout +
    /// `worker/<run_id>` branch) unless the dispatch-time
    /// `isolation` input parameter explicitly overrides to `false`.
    /// `Some(false)` or `None` keeps the legacy shared-cwd behavior
    /// (worker reuses the parent session's worktree). Builtin
    /// `general-purpose` ships with `Some(true)` (write-capable
    /// workers benefit most from isolation); `researcher` ships
    /// with `None` (read-only workers don't need a separate
    /// checkout — saves the per-dispatch checkout cost).
    ///
    /// The final isolation decision is the merge of this default
    /// with the dispatch-time override; see
    /// [`resolve_isolation`] in `dispatch.rs`.
    pub isolation: Option<bool>,
}

/// The two MVP subagent definitions, keyed by name. Used by
/// `run_subagent` to resolve the LLM-supplied `subagent` argument.
pub fn builtin_subagents() -> &'static [SubagentDef] {
    // `OnceLock<Vec<SubagentDef>>` holds the registry; `get_or_init`
    // builds it exactly once on first read. The `'static` borrow is
    // sound because the OnceLock itself lives in a `static`.
    static REGISTRY: std::sync::OnceLock<Vec<SubagentDef>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            SubagentDef {
                name: "researcher".to_string(),
                description: "Read-only research subagent. Can read files, grep, glob, list \
                              directories, and fetch web pages, but cannot edit, write, or run \
                              shells. Use for focused code exploration or web research where \
                              the verbose search output would otherwise pollute the main \
                              conversation."
                    .to_string(),
                system_prompt: "You are a read-only research subagent dispatched by the main \
                                agent to investigate a focused question. You have access to \
                                `read_file`, `grep`, `glob`, `list_dir`, and `web_fetch` — use \
                                them to \
                                answer the task as completely as you can. You CANNOT edit, \
                                write, or run shell commands, and you CANNOT dispatch further \
                                subagents (no nesting). When you have gathered enough, write a \
                                concise final summary of what you found — the summary will be \
                                returned to the main agent verbatim as the tool_result of the \
                                dispatch_subagent call, so it should be self-contained. Keep \
                                the summary focused: the main agent has its own full context \
                                and does not need your intermediate tool logs.\n\nReply in the \
                                user's language."
                    .to_string(),
                tools: vec![
                    "read_file".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "web_fetch".to_string(),
                ],
                // L3b (2026-06-27): researcher is read-only, so it
                // does not benefit from a separate worktree (no
                // write conflicts to isolate). Leaving isolation
                // `None` keeps the legacy shared-cwd behavior and
                // saves the per-dispatch checkout cost.
                isolation: None,
            },
            SubagentDef {
                name: "general-purpose".to_string(),
                description: "General-purpose subagent. Has the full toolset minus the \
                              structural-disabled set (dispatch_subagent, update_checklist, \
                              background-shell tools). Use for self-contained sub-tasks that \
                              would benefit from isolated context (e.g. a focused refactor, \
                              a full test+fix loop, a multi-file search-and-edit)."
                    .to_string(),
                system_prompt: "You are a general-purpose subagent dispatched by the main \
                                agent to work on a self-contained sub-task in your own \
                                isolated context. You have access to the full toolset minus \
                                `dispatch_subagent` (no nesting), `update_checklist`, and the \
                                background-shell tools. The main agent's conversation history \
                                is NOT visible to you — work only from the task prompt you \
                                were given. When you finish, write a concise summary of what \
                                you did (what files you changed, what commands you ran, any \
                                failures) — the summary will be returned to the main agent \
                                verbatim as the tool_result of the dispatch_subagent call, so \
                                it should be self-contained.\n\nReply in the user's language."
                    .to_string(),
                // Empty Vec = "inherit builtin_tools() minus structural-disabled".
                // `filter_tools_for_subagent` reads `tools.is_empty()` as "full set
                // minus disabled"; this keeps the general-purpose subagent's tool
                // list self-maintaining as new tools are added to builtin_tools().
                tools: vec![],
                // L3b (2026-06-27): general-purpose is write-capable, so it
                // benefits most from worktree isolation — concurrent workers
                // can each land writes in their own checkout without racing.
                // B (2026-06-30): default changed to `None` (shared) so
                // a single serial dispatch reuses the parent cwd — zero
                // merge, matches Claude Code's default. Concurrent
                // dispatch is force-isolated in `chat_loop.rs`'s
                // `DispatchBatch::Concurrent` branch (gated by
                // `worker_is_writable`), so concurrent-write safety no
                // longer relies on this default being `Some(true)`.
                isolation: None,
            },
        ]
    })
}

/// Resolve a built-in subagent by name. Returns `None` for unknown
/// names (the interceptor synthesizes an error tool_result).
///
/// **L3d PR3 (2026-06-25)**: production code now resolves subagents
/// via `SubagentCache::lookup` (which merges builtin + user + project
/// with precedence). This function is retained for the unit tests
/// in this module + `tests_subagent.rs` that want a direct builtin
/// lookup without spinning up a `SubagentCache`. The
/// `#[allow(dead_code)]` silences the "never used" warning from the
/// production build (the function is only called from `#[cfg(test)]`
/// code).
#[allow(dead_code)]
pub fn lookup_subagent(name: &str) -> Option<&'static SubagentDef> {
    builtin_subagents().iter().find(|s| s.name == name)
}

// ---------------------------------------------------------------------------
// Prompt assembly
// ---------------------------------------------------------------------------

/// Assemble the worker's system prompt. **Fully replaces** the
/// parent's behavior_prompt + mode_prefix + base_prompt layers —
/// the worker does NOT inherit the main system prompt (Claude Code
/// convention). The mode-specific permission boundary is enforced
/// at the ⑨ 关 layer, not in the prompt.
///
/// **Active since 2026-06-21 (B6 review defect A fix).** The
/// `assemble_subagent_prompt(def, task)` output is now threaded
/// as the 23rd `system_prompt_override` parameter on the
/// `run_chat_loop` nested call (see
/// `agent::subagent::dispatch::run_subagent`); the loop body short-
/// circuits the parent's `assemble_system_prompt(mode_prefix,
/// base_prompt)` step when the override is `Some(_)`. Pre-fix
/// the prompt was effectively dead code (the worker's
/// `SubagentDef.system_prompt` was discarded, and the worker
/// silently received the parent's prompt — contradicting the
/// mode-specific permission behaviour enforced at the ⑨ 关
/// layer). See `docs/review/b6-subagent-assessment.md` §2 +
/// the doc comment on `run_chat_loop.system_prompt_override`
/// for the full rationale.
pub fn assemble_subagent_prompt(def: &SubagentDef, _task: &str) -> String {
    // The task itself is delivered as a user message (see
    // `build_worker_messages`); the system prompt is just the
    // worker's role + behavior guidance. The `_task` is reserved
    // for a future "task summary header" if we want to echo it.
    def.system_prompt.clone()
}

/// Build the worker's initial `messages` Vec.
///
/// Per PRD §Decisions 6 + review #6:
/// 1. `messages[0]` = memory instructions synthetic user message
///    (loaded via `build_instructions_blocks`, banner block carries
///    `cache_control: Some(Ephemeral)` so the worker has its OWN
///    cache breakpoint, independent of the parent).
/// 2. `messages[1]` = delegation task (APPEND, never prepend —
///    see prompt-cache invariant: the memory breakpoint must stay
///    at position 0).
///
/// When the project has no loaded memory layers, only the task
/// message is emitted (the parent's behavior — skip the synthetic
/// message entirely on a fresh install — is preserved).
pub async fn build_worker_messages(
    memory_cache: &Arc<MemoryCache>,
    project_id: &str,
    project_path: &str,
    task: &str,
) -> Vec<ChatMessage> {
    let layers =
        crate::memory::loader::load_for_session(memory_cache, project_id, project_path).await;
    let instructions_blocks = crate::memory::loader::build_instructions_blocks(&layers);
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(2);
    if !instructions_blocks.is_empty() {
        // messages[0] — memory synthetic user message, banner carries
        // cache_control: Ephemeral. Worker's own breakpoint.
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(instructions_blocks),
        });
        // Mirror the parent loop's memory pair: a synthetic assistant
        // ack keeps the Anthropic wire shape happy (user/assistant
        // alternation) and signals the worker has acknowledged the
        // instructions before the task arrives.
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(
                "Understood. I will follow these instructions while working on the \
                 delegated task."
                    .to_string(),
            ),
        });
    }
    // The delegation task. APPEND — the memory breakpoint (if any)
    // stays at messages[0]; the task's position is independent of
    // whether memory is loaded. Anthropic accepts a user-role
    // message after an assistant-role message.
    messages.push(ChatMessage {
        role: Role::User,
        content: MessageContent::Text(task.to_string()),
    });
    messages
}

// ---------------------------------------------------------------------------
// Tool allowlist + structural-disabled filter
// ---------------------------------------------------------------------------

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
const STRUCTURALLY_DISABLED: &[&str] = &[
    "update_checklist",
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
];

/// Filter `builtin_tools()` for a worker.
///
/// - If `def.tools` is empty, start from the full `builtin_tools()`
///   set (the general-purpose convention).
/// - Otherwise start from the allowlist.
/// - Then strip [`STRUCTURALLY_DISABLED`] unconditionally (so a
///   future frontmatter can't accidentally re-enable nesting or
///   the L1a trio).
pub fn filter_tools_for_subagent(
    all_tools: Vec<ToolDef>,
    def: &SubagentDef,
) -> Vec<ToolDef> {
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
/// 06-25-subagent-web-access). This is the **runtime-forced
/// read-only layer** (the 2nd of 3 — see L3a PRD "只读保证三层"):
/// when multiple workers run concurrently in a pure dispatch
/// batch, the concurrent branch forces every worker's toolset
/// down to just these 5 read-only tools regardless of its
/// `SubagentDef` allowlist. For `researcher` this is a no-op (its
/// `SubagentDef.tools` is already exactly these 5); for
/// `general-purpose` it strips write/edit/shell/etc. (`web_fetch`
/// is kept — it is a read-only network op, `Risk::Low`, and
/// SSRF-guarded in `tools/web_fetch.rs`; a worker's `web_fetch`
/// still goes through the Tier 4 permission check, inheriting the
/// parent session's `web_fetch` grant or surfacing a
/// `WorkerAskBanner`). The safety baseline is still the
/// `is_worker: true` permission layer (worker asks route through
/// `WorkerAskBanner` since the 2026-06-22 RULE-FrontSubagent-003
/// fix — they no longer collapse to `Deny`; 3rd layer) —
/// `filter_tools_readonly` is defense-in-depth that keeps the
/// concurrent branch's tool discovery surface aligned with its
/// read-only contract so the LLM never even sees a write tool in
/// the concurrent path.
pub const READONLY_TOOL_ALLOWLIST: &[&str] = &["read_file", "grep", "glob", "list_dir", "web_fetch"];

/// Force a worker's toolset down to read-only tools only (L3a,
/// 2026-06-24). Applied by the concurrent dispatch branch in
/// `chat_loop.rs` AFTER `filter_tools_for_subagent` so the
/// concurrent batch's workers can never see a write or shell tool
/// (`web_fetch` is kept — read-only network op, see
/// `READONLY_TOOL_ALLOWLIST`). Mirrors the `STRUCTURALLY_DISABLED`
/// filter pattern (same `.filter(|t| allowlist.contains(t.name))`
/// shape).
///
/// `researcher` is unaffected (its allowlist is already exactly
/// `READONLY_TOOL_ALLOWLIST`); `general-purpose` is downgraded
/// from its full-minus-disabled set to just the 5 read-only tools
/// (incl. `web_fetch`). Returns a fresh `Vec<ToolDef>` (consumes
/// the input).
pub fn filter_tools_readonly(tools: Vec<ToolDef>) -> Vec<ToolDef> {
    tools
        .into_iter()
        .filter(|t| READONLY_TOOL_ALLOWLIST.contains(&t.name.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// SubagentStatus
// ---------------------------------------------------------------------------

/// The terminal status a worker exited with. Used by `run_subagent`
/// to format the dispatch_subagent tool_result's status prefix.
///
/// 2026-06-21 (R2): added `Incomplete` for the `max_turns` soft-
/// terminal path. The pre-existing 3 variants were
/// `Completed` / `Cancelled` / `Error`. `Incomplete` is the
/// budget-exhaustion terminal: the worker produced useful
/// intermediate output (transcript is non-empty) but did not
/// cleanly finish within the 200-turn budget. The DB-side enum
/// `db::subagent_runs::SubagentStatusDb` mirrors this 4th variant
/// in lockstep — `as_str` and `from_str_opt` must stay in
/// lockstep across the two enums.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    Completed,
    Cancelled,
    Error,
    Incomplete,
}

impl SubagentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
            Self::Incomplete => "incomplete",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- definition ----

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, DISPATCH_TOOL_NAME);
    }

    #[test]
    fn definition_schema_requires_subagent_and_task() {
        let schema = definition().input_schema;
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array present");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"subagent"));
        assert!(names.contains(&"task"));
    }

    #[test]
    fn definition_schema_subagent_enum_covers_two() {
        let schema = definition().input_schema;
        let enum_vals: Vec<&str> = schema
            .pointer("/properties/subagent/enum")
            .and_then(|v| v.as_array())
            .expect("subagent enum present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(enum_vals, vec!["researcher", "general-purpose"]);
    }

    // ---- definition_with_cache (L3d PR3) ----

    #[tokio::test]
    async fn definition_with_cache_enum_includes_builtins() {
        // Fresh cache + empty project dir → enum is the 2 builtins
        // (alphabetical: general-purpose, researcher).
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = SubagentCache::arc();
        let project_path = tmp.path().to_string_lossy().to_string();
        let def = definition_with_cache(&cache, &project_path).await;
        assert_eq!(def.name, DISPATCH_TOOL_NAME);
        let enum_vals: Vec<String> = def
            .input_schema
            .pointer("/properties/subagent/enum")
            .and_then(|v| v.as_array())
            .expect("enum present")
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        assert_eq!(
            enum_vals,
            vec!["general-purpose".to_string(), "researcher".to_string()]
        );
    }

    #[tokio::test]
    async fn definition_with_cache_description_has_source_tags() {
        // The description must carry `Available subagents:` + per-agent
        // `(source: ...)` tags so the LLM (and the user debugging)
        // can see each subagent's provenance.
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = SubagentCache::arc();
        let project_path = tmp.path().to_string_lossy().to_string();
        let def = definition_with_cache(&cache, &project_path).await;
        let desc = def.description.expect("description present");
        assert!(
            desc.contains("Available subagents:"),
            "description must carry the available-agents line: {}",
            desc
        );
        // Both builtins are source: builtin.
        assert!(
            desc.contains("researcher (source: builtin)"),
            "missing researcher builtin tag: {}",
            desc
        );
        assert!(
            desc.contains("general-purpose (source: builtin)"),
            "missing general-purpose builtin tag: {}",
            desc
        );
    }

    #[tokio::test]
    async fn definition_with_cache_picks_up_user_md() {
        // mtime fence: writing a user .md between calls changes the
        // enum on the next `definition_with_cache` invocation.
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join("agents");
        std::fs::create_dir_all(&user_agents).unwrap();

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let prev = crate::memory::file::set_user_dir_for_test(
            Some(user_tmp.path().to_path_buf()),
        );
        let cache = SubagentCache::arc();

        // Initially only builtins.
        let def = definition_with_cache(&cache, &project_path).await;
        let enum_vals: Vec<String> = def
            .input_schema
            .pointer("/properties/subagent/enum")
            .and_then(|v| v.as_array())
            .expect("enum")
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        assert_eq!(enum_vals.len(), 2);

        // Write a user .md → next call sees it.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        std::fs::write(
            user_agents.join("custom.md"),
            "---\nname: custom\ndescription: my custom agent\ntools: [read_file]\n---\nbody",
        )
        .unwrap();

        let def = definition_with_cache(&cache, &project_path).await;
        let enum_vals: Vec<String> = def
            .input_schema
            .pointer("/properties/subagent/enum")
            .and_then(|v| v.as_array())
            .expect("enum")
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        assert_eq!(enum_vals.len(), 3);
        assert!(enum_vals.contains(&"custom".to_string()));

        // Source tag for the user agent is in the description.
        let desc = def.description.expect("description");
        assert!(
            desc.contains("custom (source: user)"),
            "missing custom user tag: {}",
            desc
        );

        crate::memory::file::set_user_dir_for_test(prev);
    }

    #[tokio::test]
    async fn definition_with_cache_project_overrides_builtin() {
        // project > user > builtin precedence: a project .md with the
        // same name as a builtin shows source: project.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let proj_agents = proj_tmp
            .path()
            .join(".everlasting")
            .join("agents");
        std::fs::create_dir_all(&proj_agents).unwrap();
        std::fs::write(
            proj_agents.join("researcher.md"),
            "---\nname: researcher\ndescription: project researcher\n---\nCustom prompt.",
        )
        .unwrap();

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let def = definition_with_cache(&cache, &project_path).await;
        let desc = def.description.expect("description");
        // project researcher wins, source tag is project.
        assert!(
            desc.contains("researcher (source: project)"),
            "expected project source tag for researcher: {}",
            desc
        );
        // No source: builtin for researcher (overridden).
        assert!(
            !desc.contains("researcher (source: builtin)"),
            "builtin tag should be overridden: {}",
            desc
        );
    }

    // ---- builtin_subagents ----

    #[test]
    fn builtin_subagents_has_two_entries() {
        let defs = builtin_subagents();
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn builtin_subagents_researcher_tool_allowlist() {
        let r = lookup_subagent("researcher").expect("researcher exists");
        assert_eq!(
            r.tools,
            vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "list_dir".to_string(),
                "web_fetch".to_string(),
            ]
        );
    }

    #[test]
    fn builtin_subagents_general_purpose_empty_allowlist() {
        let g = lookup_subagent("general-purpose").expect("general-purpose exists");
        assert!(g.tools.is_empty(), "general-purpose inherits full set");
    }

    #[test]
    fn lookup_subagent_unknown_returns_none() {
        assert!(lookup_subagent("nope").is_none());
    }

    // ---- filter_tools_for_subagent ----

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn tool_names(tools: &[ToolDef]) -> Vec<String> {
        tools.iter().map(|t| t.name.clone()).collect()
    }

    #[test]
    fn filter_researcher_keeps_only_read_tools_and_strips_disabled() {
        let def = lookup_subagent("researcher").unwrap();
        let all = vec![
            tool("read_file"),
            tool("grep"),
            tool("glob"),
            tool("list_dir"),
            tool("write_file"),
            tool("edit_file"),
            tool("shell"),
            tool("web_fetch"),
            tool("use_skill"),
            tool("update_checklist"),
            tool("dispatch_subagent"),
            tool("run_background_shell"),
            tool("shell_status"),
            tool("shell_kill"),
        ];
        let filtered = filter_tools_for_subagent(all, def);
        let names = tool_names(&filtered);
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"grep".to_string()));
        assert!(names.contains(&"glob".to_string()));
        assert!(names.contains(&"list_dir".to_string()));
        // web_fetch is now in the researcher allowlist (06-25-subagent-web-access).
        assert!(names.contains(&"web_fetch".to_string()));
        // Read-only — no writes.
        assert!(!names.contains(&"write_file".to_string()));
        assert!(!names.contains(&"edit_file".to_string()));
        assert!(!names.contains(&"shell".to_string()));
        // Structural-disabled ALWAYS stripped.
        assert!(!names.contains(&"update_checklist".to_string()));
        assert!(!names.contains(&"dispatch_subagent".to_string()));
        assert!(!names.contains(&"run_background_shell".to_string()));
        assert!(!names.contains(&"shell_status".to_string()));
        assert!(!names.contains(&"shell_kill".to_string()));
    }

    #[test]
    fn filter_general_purpose_keeps_full_set_minus_disabled() {
        let def = lookup_subagent("general-purpose").unwrap();
        let all = vec![
            tool("read_file"),
            tool("write_file"),
            tool("edit_file"),
            tool("shell"),
            tool("grep"),
            tool("glob"),
            tool("list_dir"),
            tool("web_fetch"),
            tool("use_skill"),
            tool("update_checklist"),
            tool("dispatch_subagent"),
            tool("run_background_shell"),
            tool("shell_status"),
            tool("shell_kill"),
        ];
        let filtered = filter_tools_for_subagent(all, def);
        let names = tool_names(&filtered);
        // general-purpose keeps the full write/shell/web_fetch set.
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"edit_file".to_string()));
        assert!(names.contains(&"shell".to_string()));
        assert!(names.contains(&"web_fetch".to_string()));
        // Structural-disabled still stripped.
        assert!(!names.contains(&"update_checklist".to_string()));
        assert!(!names.contains(&"dispatch_subagent".to_string()));
        assert!(!names.contains(&"run_background_shell".to_string()));
        assert!(!names.contains(&"shell_status".to_string()));
        assert!(!names.contains(&"shell_kill".to_string()));
    }

    #[test]
    fn filter_strips_structurally_disabled_even_if_allowlist_lists_them() {
        // Defensive: build a synthetic SubagentDef that explicitly
        // allows dispatch_subagent + the L1a trio. The filter MUST
        // still strip them (structural-disabled wins over the
        // allowlist).
        let synthetic = SubagentDef {
            name: "synthetic".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            tools: vec![
                "read_file".to_string(),
                "dispatch_subagent".to_string(),
                "update_checklist".to_string(),
                "run_background_shell".to_string(),
                "shell_status".to_string(),
                "shell_kill".to_string(),
                "ask_user_question".to_string(),
            ],
            isolation: None,
        };
        let all = vec![
            tool("read_file"),
            tool("dispatch_subagent"),
            tool("update_checklist"),
            tool("run_background_shell"),
            tool("shell_status"),
            tool("shell_kill"),
            tool("ask_user_question"),
        ];
        let filtered = filter_tools_for_subagent(all, &synthetic);
        let names = tool_names(&filtered);
        // ask_user_question (06-30 task) is structurally disabled for
        // workers — workers have no UI sink and would hang the oneshot
        // forever. Assert it explicitly alongside the other disabled
        // tools so AC3 has a named coverage point.
        assert_eq!(names, vec!["read_file".to_string()]);
    }
}
