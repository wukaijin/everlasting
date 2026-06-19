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
//!    [`run_subagent`] with the full closure dependencies
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
//! # Why a separate file?
//!
//! The SubagentDef registry, prompt assembly, tool allowlist
//! filtering, and `SubagentBufferSink` all have well-scoped unit
//! tests; keeping them out of `chat_loop.rs` lets the loop stay
//! focused on turn orchestration. The `run_subagent` helper
//! itself lives in `chat_loop.rs` because it captures
//! `run_chat_loop`'s closure dependencies — the helper calls
//! `run_chat_loop` recursively and thus needs the same parameter
//! set the parent loop was invoked with.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::agent::permissions::PermissionAskPayload;
use crate::llm::types::{ChatEvent, MessageContent};
use crate::llm::{ChatMessage, Role, ToolDef};
use crate::memory::MemoryCache;
use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};

// ---------------------------------------------------------------------------
// Dispatch tool definition
// ---------------------------------------------------------------------------

/// The `dispatch_subagent` tool definition. Registered in
/// `tools::builtin_tools()` so the LLM can discover it + go through
/// the ⑨ 关 permission check. The **execution path is
/// intercepted** in `chat_loop.rs`'s tool dispatch — this ToolDef
/// is discovery-only; the actual `run_subagent` call is in
/// `chat_loop.rs` (see PRD §"Technical Approach" and review #3).
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

// ---------------------------------------------------------------------------
// SubagentBufferSink — worker-side ChatEventSink
// ---------------------------------------------------------------------------

/// The terminal status a worker exited with. Used by `run_subagent`
/// to format the dispatch_subagent tool_result's status prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    Completed,
    Cancelled,
    Error,
}

impl SubagentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

/// One entry in the worker's in-memory transcript. PR1b keeps it
/// **in memory only** — no DB writes (that's PR2's `subagent_runs`
/// table). The transcript accumulates the worker's chat-events /
/// tool calls / tool results so the parent + (future PR2/PR3) the
/// frontend can expand "what did the worker do?" after the fact.
#[derive(Debug, Clone)]
#[allow(dead_code)] // PR1b: in-memory only; PR2 persists, PR3 renders.
pub struct TranscriptEntry {
    pub kind: TranscriptKind,
    pub payload_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // paired with TranscriptEntry
pub enum TranscriptKind {
    ChatEvent,
    ToolCall,
    ToolResult,
    PermissionAsk,
}

/// `ChatEventSink` impl that records every worker emit into an
/// in-memory `Vec<TranscriptEntry>` + tracks the worker's final
/// assistant text (the summary).
///
/// The sink does **NOT** forward to the parent sink — doing so
/// would flood the parent's frontend with worker stream events
/// (Claude Code convention: the worker is isolated from the main
/// UI; only the final summary returns as a tool_result). The
/// parent's frontend sees `dispatch_subagent` as a single opaque
/// tool_use/tool_result pair; the worker's transcript is
/// retrievable separately (PR2: `subagent_runs.transcript`;
/// PR3: ToolCallCard expand UI).
pub struct SubagentBufferSink {
    transcript: StdMutex<Vec<TranscriptEntry>>,
    /// Accumulated assistant text deltas. Read by `run_subagent`
    /// after `run_chat_loop` returns to extract the worker's
    /// final summary.
    text_parts: StdMutex<Vec<String>>,
    /// Set when the worker emitted a terminal `Error` event.
    /// `run_subagent` reads this to pick the `status: error`
    /// prefix.
    had_error: std::sync::atomic::AtomicBool,
    /// Set when the worker emitted a terminal `Done{cancelled}`
    /// event (stop_reason == "cancelled"). `run_subagent` reads
    /// this to pick the `status: cancelled` prefix.
    was_cancelled: std::sync::atomic::AtomicBool,
}

impl SubagentBufferSink {
    pub fn new() -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn record(&self, kind: TranscriptKind, payload_json: serde_json::Value) {
        self.transcript
            .lock()
            .expect("SubagentBufferSink transcript mutex poisoned")
            .push(TranscriptEntry {
                kind,
                payload_json,
            });
    }

    /// Snapshot of the worker's accumulated text deltas, joined.
    /// Called by `run_subagent` after the worker loop returns.
    pub fn final_text(&self) -> String {
        let guard = self
            .text_parts
            .lock()
            .expect("SubagentBufferSink text_parts mutex poisoned");
        guard.join("")
    }

    pub fn had_error(&self) -> bool {
        self.had_error
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn was_cancelled(&self) -> bool {
        self.was_cancelled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Snapshot of the transcript (clone). Used by future PR2/PR3
    /// to persist into `subagent_runs.transcript_json`.
    #[allow(dead_code)] // PR2: persists transcript; PR3: expands it.
    pub fn transcript_snapshot(&self) -> Vec<TranscriptEntry> {
        self.transcript
            .lock()
            .expect("SubagentBufferSink transcript mutex poisoned")
            .clone()
    }
}

impl Default for SubagentBufferSink {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::state::ChatEventSink for SubagentBufferSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        // Track terminal signals + accumulate text deltas for the
        // final summary.
        match &payload.event {
            ChatEvent::Delta { text } => {
                self.text_parts
                    .lock()
                    .expect("SubagentBufferSink text_parts mutex poisoned")
                    .push(text.clone());
            }
            ChatEvent::Error { .. } => {
                self.had_error
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ChatEvent::Done { stop_reason, .. } => {
                if stop_reason.as_deref() == Some("cancelled")
                    || stop_reason.as_deref() == Some("max_turns")
                {
                    // Treat max_turns as a soft "ran out of budget"
                    // — the worker did useful work but didn't
                    // cleanly finish. The summary still carries
                    // whatever it produced. Status prefix =
                    // "completed" with a note appended; for
                    // cancelled (user Stop propagated to worker)
                    // we use status=cancelled.
                    if stop_reason.as_deref() == Some("cancelled") {
                        self.was_cancelled
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            _ => {}
        }
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::ChatEvent, payload_json);
    }

    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::ToolCall, payload_json);
    }

    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::ToolResult, payload_json);
    }

    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        // Worker permission asks are auto-denied by the Tier 4
        // is_worker collapse (see `permissions::check`); this
        // method should never be called in practice. We still
        // record the entry for diagnosis — if it ever fires, the
        // transcript shows the worker tried to ask (which is the
        // bug).
        let payload_json = serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::PermissionAsk, payload_json);
    }
}

// ---------------------------------------------------------------------------
// Status-prefix formatter for the dispatch_subagent tool_result
// ---------------------------------------------------------------------------

/// Format the dispatch_subagent tool_result content from the
/// worker's final state. Per PRD §"summary 回填" + research §2:
///
/// - `status: completed` → `[status: completed]\n<summary>`
/// - `status: cancelled` → `[status: cancelled]\n[CANCELLED_MARKER]\n<partial>`
/// - `status: error`     → `[status: error]\n<error text>`
///
/// Returns `(content, is_error)`. `is_error` is `true` for cancel
/// and error so the LLM knows the worker did not succeed; `false`
/// for completed.
pub fn format_dispatch_result(
    status: SubagentStatus,
    worker_text: &str,
) -> (String, bool) {
    let prefix = format!("[status: {}]", status.as_str());
    match status {
        SubagentStatus::Completed => {
            let content = if worker_text.is_empty() {
                format!("{}\n(worker produced no final text)", prefix)
            } else {
                format!("{}\n{}", prefix, worker_text)
            };
            (content, false)
        }
        SubagentStatus::Cancelled => {
            // Reuse the same CANCELLED_MARKER the parent loop uses
            // for its own cancel path — keeps the wire shape
            // consistent across parent + worker.
            let marker = crate::agent::helpers::CANCELLED_MARKER;
            let content = if worker_text.is_empty() {
                format!("{}\n{}", prefix, marker)
            } else {
                format!("{}\n{}\n\n{}", prefix, worker_text, marker)
            };
            (content, true)
        }
        SubagentStatus::Error => {
            let error_text = if worker_text.is_empty() {
                "(no error text captured)"
            } else {
                worker_text
            };
            let content = format!("{}\n{}", prefix, error_text);
            (content, true)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ChatEventSink;

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

    // ---- SubagentBufferSink ----

    #[test]
    fn buffer_sink_accumulates_text_deltas() {
        let sink = SubagentBufferSink::new();
        let rid = "rid-test".to_string();
        for t in ["hello", " ", "world"] {
            sink.emit_chat_event(&ChatEventPayload {
                request_id: rid.clone(),
                event: ChatEvent::Delta {
                    text: t.to_string(),
                },
            });
        }
        assert_eq!(sink.final_text(), "hello world");
    }

    #[test]
    fn buffer_sink_tracks_cancelled_done() {
        let sink = SubagentBufferSink::new();
        let rid = "rid-cancel".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        assert!(sink.was_cancelled());
        assert!(!sink.had_error());
    }

    #[test]
    fn buffer_sink_tracks_error_event() {
        use crate::llm::LlmErrorCategory;
        let sink = SubagentBufferSink::new();
        let rid = "rid-err".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Error {
                message: "boom".to_string(),
                category: LlmErrorCategory::Server,
            },
        });
        assert!(sink.had_error());
        assert!(!sink.was_cancelled());
    }

    #[test]
    fn buffer_sink_records_transcript_entries() {
        let sink = SubagentBufferSink::new();
        let rid = "rid-transcript".to_string();
        sink.emit_chat_event(&ChatEventPayload {
            request_id: rid.clone(),
            event: ChatEvent::Start,
        });
        sink.emit_tool_call(&ToolCallPayload {
            request_id: rid.clone(),
            id: "toolu_1".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "/x"}),
        });
        sink.emit_tool_result(&ToolResultPayload {
            request_id: rid,
            tool_use_id: "toolu_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
        });
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
    }

    // ---- format_dispatch_result ----

    #[test]
    fn format_completed_with_summary() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Completed, "found 3 files");
        assert!(!is_error);
        assert!(content.starts_with("[status: completed]"));
        assert!(content.contains("found 3 files"));
    }

    #[test]
    fn format_completed_with_empty_text_falls_back_to_note() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Completed, "");
        assert!(!is_error);
        assert!(content.contains("worker produced no final text"));
    }

    #[test]
    fn format_cancelled_includes_marker() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Cancelled, "partial");
        assert!(is_error);
        assert!(content.starts_with("[status: cancelled]"));
        assert!(content.contains(crate::agent::helpers::CANCELLED_MARKER));
        assert!(content.contains("partial"));
    }

    #[test]
    fn format_cancelled_empty_text_uses_marker_alone() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Cancelled, "");
        assert!(is_error);
        assert!(content.contains(crate::agent::helpers::CANCELLED_MARKER));
    }

    #[test]
    fn format_error_includes_status_prefix() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Error, "LLM stream errored");
        assert!(is_error);
        assert!(content.starts_with("[status: error]"));
        assert!(content.contains("LLM stream errored"));
    }
}
