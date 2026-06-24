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
pub(crate) mod dispatch;

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
             `researcher` (read-only: read_file / grep / glob / list_dir) and \
             `general-purpose` (full toolset minus dispatch_subagent / \
             update_checklist / background-shell tools). The worker inherits the \
             parent's permission Mode (Yolo → all-allow; Edit/Plan → writes / \
             shells auto-denied because the worker has no UI to surface a \
             permission modal)."
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
                }
            },
            "required": ["subagent", "task"]
        }),
    }
}

/// The canonical name of the dispatch tool. Used by the
/// interceptor in `chat_loop.rs` to recognize it.
pub const DISPATCH_TOOL_NAME: &str = "dispatch_subagent";

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
pub struct SubagentDef {
    pub name: &'static str,
    /// User-facing description. Used by future PR3 (frontend picker
    /// UI) and the dispatch_subagent tool description; kept on the
    /// struct so a future frontmatter loader can populate it from
    /// the Markdown front-matter.
    #[allow(dead_code)]
    pub description: &'static str,
    pub system_prompt: String,
    pub tools: &'static [&'static str],
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
                name: "researcher",
                description: "Read-only research subagent. Can read files, grep, glob, and list \
                              directories but cannot edit, write, or run shells. Use for \
                              focused code exploration where the verbose search output would \
                              otherwise pollute the main conversation.",
                system_prompt: "You are a read-only research subagent dispatched by the main \
                                agent to investigate a focused question. You have access to \
                                `read_file`, `grep`, `glob`, and `list_dir` — use them to \
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
                tools: &["read_file", "grep", "glob", "list_dir"],
            },
            SubagentDef {
                name: "general-purpose",
                description: "General-purpose subagent. Has the full toolset minus the \
                              structural-disabled set (dispatch_subagent, update_checklist, \
                              background-shell tools). Use for self-contained sub-tasks that \
                              would benefit from isolated context (e.g. a focused refactor, \
                              a full test+fix loop, a multi-file search-and-edit).",
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
                // Empty slice = "inherit builtin_tools() minus structural-disabled".
                // `filter_tools_for_subagent` reads `tools.is_empty()` as "full set
                // minus disabled"; this keeps the general-purpose subagent's tool
                // list self-maintaining as new tools are added to builtin_tools().
                tools: &[],
            },
        ]
    })
}

/// Resolve a built-in subagent by name. Returns `None` for unknown
/// names (the interceptor synthesizes an error tool_result).
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
        Some(def.tools.iter().copied().collect())
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
/// 2026-06-24). This is the **runtime-forced read-only layer**
/// (the 2nd of 3 — see L3a PRD "只读保证三层"): when multiple
/// workers run concurrently in a pure dispatch batch, the
/// concurrent branch forces every worker's toolset down to just
/// these 4 read-only tools regardless of its `SubagentDef`
/// allowlist. For `researcher` this is a no-op (its
/// `SubagentDef.tools` is already exactly these 4); for
/// `general-purpose` it strips write/edit/shell/web_fetch/etc.
/// The safety baseline is still the `is_worker: true` permission
/// layer (`ask_path`/`ask_shell` collapse to `Deny` for workers,
/// 3rd layer) — `filter_tools_readonly` is defense-in-depth that
/// keeps the concurrent branch's tool discovery surface aligned
/// with its read-only contract so the LLM never even sees a
/// write tool in the concurrent path.
pub const READONLY_TOOL_ALLOWLIST: &[&str] = &["read_file", "grep", "glob", "list_dir"];

/// Force a worker's toolset down to read-only tools only (L3a,
/// 2026-06-24). Applied by the concurrent dispatch branch in
/// `chat_loop.rs` AFTER `filter_tools_for_subagent` so the
/// concurrent batch's workers can never see a write / shell /
/// web tool. Mirrors the `STRUCTURALLY_DISABLED` filter pattern
/// (same `.filter(|t| allowlist.contains(t.name))` shape).
///
/// `researcher` is unaffected (its allowlist is already exactly
/// `READONLY_TOOL_ALLOWLIST`); `general-purpose` is downgraded
/// from its full-minus-disabled set to just the 4 read-only
/// tools. Returns a fresh `Vec<ToolDef>` (consumes the input).
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

    // ---- builtin_subagents ----

    #[test]
    fn builtin_subagents_has_two_entries() {
        let defs = builtin_subagents();
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn builtin_subagents_researcher_tool_allowlist() {
        let r = lookup_subagent("researcher").expect("researcher exists");
        assert_eq!(r.tools, &["read_file", "grep", "glob", "list_dir"]);
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
            name: "synthetic",
            description: "",
            system_prompt: String::new(),
            tools: &[
                "read_file",
                "dispatch_subagent",
                "update_checklist",
                "run_background_shell",
                "shell_status",
                "shell_kill",
            ],
        };
        let all = vec![
            tool("read_file"),
            tool("dispatch_subagent"),
            tool("update_checklist"),
            tool("run_background_shell"),
            tool("shell_status"),
            tool("shell_kill"),
        ];
        let filtered = filter_tools_for_subagent(all, &synthetic);
        let names = tool_names(&filtered);
        assert_eq!(names, vec!["read_file".to_string()]);
    }
}
