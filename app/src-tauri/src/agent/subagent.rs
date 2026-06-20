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

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::agent::permissions::PermissionAskPayload;
use crate::llm::types::{ChatEvent, MessageContent, TokenUsage};
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
///
/// **Active since 2026-06-21 (B6 review defect A fix).** The
/// `assemble_subagent_prompt(def, task)` output is now threaded
/// as the 23rd `system_prompt_override` parameter on the
/// `run_chat_loop` nested call (see
/// `agent::chat_loop::run_subagent`); the loop body short-
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
///
/// `Serialize` / `Deserialize` are derived in PR2 so the
/// `Vec<TranscriptEntry>` can round-trip through the
/// `subagent_runs.transcript_json` column (and through the
/// `truncate_transcript_for_persistence` head+tail reparse path).
/// The shape is `{"kind": "<variant>", "payload_json": <any JSON>}` —
/// the inner `payload_json` is already a `serde_json::Value`, so
/// the `#[serde(other)]` on the kind enum (below) is irrelevant
/// here; we just need the outer struct to derive the traits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // PR1b: in-memory only; PR2 persists, PR3 renders.
pub struct TranscriptEntry {
    pub kind: TranscriptKind,
    pub payload_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // paired with TranscriptEntry
pub enum TranscriptKind {
    ChatEvent,
    ToolCall,
    ToolResult,
    PermissionAsk,
}

/// Build the IPC payload for the `subagent:event` Tauri channel.
/// Pure function — keeps the wire shape in exactly one place so
/// the TS mirror (`runId` / `sessionId` / `kind` / `payload` /
/// `timestamp`) can be locked by unit tests without spinning up a
/// Tauri runtime.
///
/// **Wire shape** (matches prd.md §"PR2 hotfix" decision + the
/// the `transcript_kind_str` mapping below):
/// ```json
/// {
///   "runId": "<DB row id (worker_run_id) — MUST equal summary.id>",
///   "sessionId": "<parent session_id>",
///   "kind": "chat_event" | "tool_call" | "tool_result" | "permission_ask",
///   "payload": <the original chat-event / tool-call / tool-result payload>,
///   "timestamp": "<RFC 3339>"
/// }
/// ```
///
/// The `kind` string is the snake_case of the `TranscriptKind`
/// enum variant (`#[serde(rename_all = "snake_case")]` on the
/// enum). The TS enum must stay in lockstep with this mapping —
/// `trellis-check` verifies line-by-line parity.
///
/// **`runId` contract (B6 PR3b hotfix, 2026-06-21)**: `run_id` MUST
/// be the `subagent_runs.id` DB row id (the UUID `insert_run`
/// returns as `worker_run_id`), NOT the human-readable
/// `worker_rid` (`"{parent_rid}-sub-{tool_use_id}"`). The frontend
/// `subagentRuns` store keys `liveTranscript` / `getRunCache` by
/// `event.runId`, while `ToolCallCard` opens the drawer with
/// `summary.id` (= the same DB id). If the two diverge, the drawer's
/// `transcript`/`status` computeds look up the wrong key and render
/// blank + stuck-on-running. `run_subagent` threads
/// `worker_run_id_opt` (fallback `worker_rid` only when the insert
/// failed — no DB row exists, so the drawer can't open anyway).
fn build_subagent_event_payload(
    run_id: &str,
    session_id: &str,
    kind: TranscriptKind,
    payload: serde_json::Value,
) -> serde_json::Value {
    let kind_str = match kind {
        TranscriptKind::ChatEvent => "chat_event",
        TranscriptKind::ToolCall => "tool_call",
        TranscriptKind::ToolResult => "tool_result",
        TranscriptKind::PermissionAsk => "permission_ask",
    };
    serde_json::json!({
        "runId": run_id,
        "sessionId": session_id,
        "kind": kind_str,
        "payload": payload,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// Build the IPC payload for the `subagent:finished` Tauri channel —
/// a one-shot terminal signal emitted by `run_subagent` AFTER
/// `update_run_finished` commits the run's terminal state. Distinct
/// from `subagent:event` (which streams transcript entries while the
/// worker runs): `subagent:finished` carries no transcript entry,
/// only the terminal status + timestamp, so the frontend can refetch
/// `get_subagent_run` + `list_subagent_runs_by_session` and flip the
/// drawer / card from `running` to the terminal state without
/// polling.
///
/// **Wire shape**:
/// ```json
/// {
///   "runId": "<DB row id — same value subagent:event uses>",
///   "sessionId": "<parent session_id>",
///   "status": "completed" | "cancelled" | "error",
///   "finishedAt": "<RFC 3339>"
/// }
/// ```
///
/// `status` is the lowercase wire form of `SubagentStatusDb::as_str`
/// (passed in as `status_str` by the caller to keep this module free
/// of a `db::subagent_runs` type dependency). The frontend
/// `coerceStatus` parses it leniently (unknown → `running`, but the
/// only emitters are the three terminal arms in `run_subagent`).
///
/// Emitted only on the `Ok(())` arm of `update_run_finished` — a DB
/// write failure leaves the row `running`, so emitting `finished`
/// would mislead the frontend into caching a stale `running` row as
/// terminal. The emit itself is best-effort (`tracing::warn!` on
/// failure, mirroring the `subagent:event` emit policy).
pub(crate) fn build_subagent_finished_payload(
    run_id: &str,
    session_id: &str,
    status_str: &str,
    finished_at: &str,
) -> serde_json::Value {
    serde_json::json!({
        "runId": run_id,
        "sessionId": session_id,
        "status": status_str,
        "finishedAt": finished_at,
    })
}

// Test-only thread-local collector for `subagent:event` IPC
// payloads. The test constructor `SubagentBufferSink::new_with_collector`
// arms this cell; `record()` forwards the IPC payload here when
// no `app_handle` is wired. Production code never reads the
// cell (the cell is always `None`). The
// `Arc<StdMutex<Vec>>` lets the test snapshot the collected
// payloads after the run.
//
// The thread-local is declared at module scope (not under
// `#[cfg(test)]`) because `record()` consults it from the
// production code path — without the declaration, a non-test
// binary that constructs a sink with `app_handle = None` (which
// the codebase never does in production, but the compiler still
// has to verify the code path) would fail to compile. The cell
// stays `None` for the entire production lifetime; only test
// code arms it.
thread_local! {
    static TEST_COLLECTOR: RefCell<Option<Arc<StdMutex<Vec<serde_json::Value>>>>> =
        const { RefCell::new(None) };
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
///
/// **PR2 hotfix (B6 PR3, 2026-06-20)**: each emit ALSO fires the
/// `subagent:event` Tauri event on the parent `AppHandle`, so the
/// frontend `<SubagentDrawer>` (PR3b) can stream the worker's
/// transcript live (debounced 200ms in the frontend store) without
/// waiting for the worker to finish. The `app_handle` is `None` in
/// tests where no Tauri runtime is present — the emit becomes a
/// no-op and the transcript-only path still works (test coverage
/// of `transcript_snapshot` is unchanged).
pub struct SubagentBufferSink {
    transcript: StdMutex<Vec<TranscriptEntry>>,
    /// Accumulated assistant text deltas. Read by `run_subagent`
    /// after `run_chat_loop` returns to extract the worker's
    /// final summary.
    text_parts: StdMutex<Vec<String>>,
    /// Per-turn `TokenUsage` accumulated from `ChatEvent::Done { usage: Some(t) }`
    /// events. Read by `run_subagent` after the worker loop returns
    /// to populate `subagent_runs.token_usage_json` and to
    /// **streaming-fold** the per-turn usage into the parent
    /// session's `sessions.input_tokens_total` columns via
    /// `db::subagent_runs::add_token_usage_streaming` (B6 PR2).
    /// The sink does this fold itself so the parent's UI sees the
    /// worker burning tokens in real time (vs. a one-shot fold
    /// at worker exit which would leave the parent's counter
    /// stale until the worker returned).
    per_turn_usage: StdMutex<Vec<TokenUsage>>,
    /// Set when the worker emitted a terminal `Error` event.
    /// `run_subagent` reads this to pick the `status: error`
    /// prefix.
    had_error: std::sync::atomic::AtomicBool,
    /// Set when the worker emitted a terminal `Done{cancelled}`
    /// event (stop_reason == "cancelled"). `run_subagent` reads
    /// this to pick the `status: cancelled` prefix.
    was_cancelled: std::sync::atomic::AtomicBool,
    /// PR2 hotfix (B6 PR3, 2026-06-20): optional Tauri
    /// `AppHandle` used to emit the `subagent:event` IPC channel
    /// on every emit. `None` in tests (no Tauri runtime) — the
    /// emit side becomes a silent no-op, but the transcript
    /// accumulation path is unaffected.
    app_handle: Option<tauri::AppHandle>,
    /// PR2 hotfix: the worker's `run_id` (the `parent_rid-sub-<seq>`
    /// string `run_subagent` builds at chat_loop.rs:2050). Carried
    /// on the sink so each `subagent:event` payload can identify
    /// which worker run the event belongs to.
    run_id: String,
    /// PR2 hotfix: the parent session_id (worker reuses parent's
    /// session_id). Each `subagent:event` payload includes this so
    /// the frontend can route events to the right session's drawer.
    session_id: String,
}

impl SubagentBufferSink {
    /// Construct a sink with Tauri IPC. Used by production
    /// (`run_subagent` threads the parent's `AppHandle` into the
    /// worker via `run_chat_loop`'s 22nd parameter).
    pub fn new(app_handle: tauri::AppHandle, run_id: String, session_id: String) -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            app_handle: Some(app_handle),
            run_id,
            session_id,
        }
    }

    /// Construct a sink without Tauri IPC (test path). The emit
    /// side becomes a silent no-op; transcript accumulation works
    /// identically.
    #[allow(dead_code)] // exposed for unit tests that exercise the sink in isolation
    pub fn new_without_app_handle(run_id: String, session_id: String) -> Self {
        Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            app_handle: None,
            run_id,
            session_id,
        }
    }

    /// Construct a sink whose IPC path is delegated to an injected
    /// collector. The collector runs in place of `app_handle.emit`
    /// so tests can assert the exact IPC payload shape without
    /// needing a real Tauri runtime. Used by the
    /// `subagent_buffer_sink_emits_ipc_event` test to lock the
    /// `subagent:event` wire shape end-to-end.
    #[cfg(test)]
    pub fn new_with_collector(
        run_id: String,
        session_id: String,
        collector: Arc<StdMutex<Vec<serde_json::Value>>>,
    ) -> Self {
        // The production path uses `app_handle.emit`; the test
        // path stores the payload in the collector. We can't have
        // both wired simultaneously through the same struct field
        // without complicating the type, so the production field
        // stays `None` for the test constructor and we route the
        // emit through a separate `emit_override` field instead.
        let sink = Self {
            transcript: StdMutex::new(Vec::new()),
            text_parts: StdMutex::new(Vec::new()),
            per_turn_usage: StdMutex::new(Vec::new()),
            had_error: std::sync::atomic::AtomicBool::new(false),
            was_cancelled: std::sync::atomic::AtomicBool::new(false),
            app_handle: None,
            run_id,
            session_id,
        };
        // Stash the collector on a thread-local for the duration
        // of the test; the record() method consults it. We use a
        // thread-local (not a field) to keep the production
        // struct unchanged — the alternative is making
        // `app_handle` an enum variant, which complicates every
        // call site.
        TEST_COLLECTOR.with(|c| {
            *c.borrow_mut() = Some(collector);
        });
        sink
    }

    fn record(&self, kind: TranscriptKind, payload_json: serde_json::Value) {
        // PR2 hotfix (B6 PR3, 2026-06-20): emit the `subagent:event`
        // IPC channel in parallel with the transcript append so the
        // frontend `<SubagentDrawer>` (PR3b) can stream the
        // worker's transcript live. The payload is a
        // `serde_json::Value` (not a typed struct) to keep the
        // Tauri channel wire shape exactly the shape documented in
        // the prd.md "PR2 hotfix" decision:
        //   { runId, sessionId, kind, payload, timestamp }
        // The kind string mirrors the Rust `TranscriptKind` enum's
        // `#[serde(rename_all = "snake_case")]` serialization
        // (`ChatEvent` / `ToolCall` / `ToolResult` / `PermissionAsk`)
        // so the TS side stays lockstep with the Rust enum.
        let ipc_payload = build_subagent_event_payload(
            &self.run_id,
            &self.session_id,
            kind,
            payload_json.clone(),
        );
        if let Some(handle) = &self.app_handle {
            if let Err(e) = handle.emit("subagent:event", ipc_payload) {
                tracing::warn!(
                    error = %e,
                    run_id = %self.run_id,
                    "subagent:event emit failed (non-fatal; transcript still recorded)"
                );
            }
        } else {
            // Test-only: forward to the in-memory collector if one
            // is armed via `new_with_collector`.
            TEST_COLLECTOR.with(|c| {
                if let Some(collector) = c.borrow().as_ref() {
                    collector.lock().unwrap().push(ipc_payload);
                }
            });
        }
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

    /// Drain the accumulated per-turn `TokenUsage` entries. Returns
    /// the union sum and clears the sink's buffer (the sink is
    /// single-shot — the caller is `run_subagent`, which runs once
    /// per worker dispatch).
    ///
    /// B6 PR2: `run_subagent` would call this **once per worker turn**
    /// to fold the new turn's usage into the parent session's
    /// `sessions.input_tokens_total`. The current production
    /// implementation routes the per-turn fold through
    /// `db::add_token_usage` (decoupled from `skip_persist` —
    /// see `chat_loop.rs:907`), so the sink-side drain is
    /// not invoked by production. The method is **retained** as
    /// the public API surface (the PRD §"SubagentBufferSink"
    /// mentions streaming accumulation) and is exercised by the
    /// `buffer_sink_drain_per_turn_usage_clears_buffer` test in
    /// this module.
    #[allow(dead_code)]
    pub fn drain_per_turn_usage(&self) -> TokenUsage {
        let mut guard = self
            .per_turn_usage
            .lock()
            .expect("SubagentBufferSink per_turn_usage mutex poisoned");
        let drained: Vec<TokenUsage> = guard.drain(..).collect();
        sum_usage(&drained)
    }

    /// Cumulative per-turn `TokenUsage` snapshot (no drain). Read
    /// by `run_subagent` at worker exit to populate
    /// `subagent_runs.token_usage_json`.
    pub fn cumulative_usage(&self) -> TokenUsage {
        let guard = self
            .per_turn_usage
            .lock()
            .expect("SubagentBufferSink per_turn_usage mutex poisoned");
        sum_usage(&guard)
    }
}

/// Sum a slice of `TokenUsage` into one. Helper for the sink's
/// `drain_per_turn_usage` / `cumulative_usage` paths.
fn sum_usage(items: &[TokenUsage]) -> TokenUsage {
    let mut total = TokenUsage::default();
    for u in items {
        total.input_tokens = total.input_tokens.saturating_add(u.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(u.output_tokens);
        total.cache_creation_input_tokens = total
            .cache_creation_input_tokens
            .saturating_add(u.cache_creation_input_tokens);
        total.cache_read_input_tokens = total
            .cache_read_input_tokens
            .saturating_add(u.cache_read_input_tokens);
    }
    total
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
            ChatEvent::Done { stop_reason, usage } => {
                // B6 PR2: capture per-turn token usage for the
                // worker run. The worker reuses the parent
                // session_id but `run_chat_loop`'s `add_token_usage`
                // call is gated by `!skip_persist` (worker passes
                // `true`); the sink's per-turn accumulator is the
                // path that folds the worker's usage into the
                // parent's `sessions.input_tokens_total` column
                // via `db::subagent_runs::add_token_usage_streaming`.
                if let Some(u) = usage {
                    self.per_turn_usage
                        .lock()
                        .expect("SubagentBufferSink per_turn_usage mutex poisoned")
                        .push(*u);
                }
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
// Transcript 4MB cap (B6 PR2)
// ---------------------------------------------------------------------------

/// Maximum size (in bytes) of a serialized `Vec<TranscriptEntry>`
/// that will be persisted into `subagent_runs.transcript_json`.
///
/// **4 MiB** is the B6 PR2 design decision (see PRD §"transcript
/// 大小 cap"): safely under SQLite's TEXT default cap (1 GiB) while
/// far above the 20-turn worker's realistic worst case (a busy
/// tool-use turn produces ~2-5 KiB of transcript, so 20 turns ≈
/// 100 KiB — three orders of magnitude under the cap). When the
/// transcript exceeds the cap, [`truncate_transcript_for_persistence`]
/// marks `transcript_truncated=1` and keeps a head + tail
/// representative slice so PR3's expand UI still shows both the
/// "what did the worker start with?" and "what did it end with?"
/// context.
pub const TRANSCRIPT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Serialize-then-cap helper. Returns `(transcript, truncated)`:
///
/// - If the JSON-serialized transcript fits in `max_bytes`, the
///   original is returned and `truncated=false`.
/// - If it doesn't, the function keeps the head and tail halves
///   of the byte representation (each `max_bytes / 2` bytes),
///   parses them back into `TranscriptEntry` vectors, and returns
///   the **union** (head + tail entries) plus `truncated=true`.
///   The parsing may fail on a half-element boundary (e.g. the
///   head cut lands in the middle of a JSON value); in that case
///   the function falls back to keeping just the head bytes (no
///   parse) under the assumption that PR3's render path will
///   surface the raw bytes — a degraded but never-empty result.
///
/// The function is **pure** (no I/O) and lives next to the sink
/// so the cap semantics are co-located with the type the cap
/// bounds. PR2's `run_subagent` calls this immediately before
/// `db::subagent_runs::update_run_finished` so the DB write
/// receives a transcript that already meets the cap.
pub fn truncate_transcript_for_persistence(
    transcript: Vec<TranscriptEntry>,
    max_bytes: usize,
) -> (Vec<TranscriptEntry>, bool) {
    let json = match serde_json::to_string(&transcript) {
        Ok(s) => s,
        Err(_) => {
            // Serialization should never fail for `TranscriptEntry`
            // (its `payload_json` is already `serde_json::Value`),
            // but the safe fallback is "return as-is, mark
            // truncated" so the caller still persists SOMETHING.
            return (transcript, true);
        }
    };
    if json.len() <= max_bytes {
        return (transcript, false);
    }
    // Over cap: keep head + tail halves (each `max_bytes / 2` bytes).
    // The cap is large enough that we don't need to worry about
    // the head/tail split landing inside a single-element JSON —
    // the reparse failure falls back to keeping the head bytes as
    // a single-element vector.
    let half = max_bytes / 2;
    let head_end = half.min(json.len());
    let tail_start = json.len().saturating_sub(half);
    // Build a synthetic JSON array: `[<head_bytes_trimmed_to_array_end>..., <tail_bytes_trimmed_to_array_end>...]`
    // by attempting to find the last `}` in the head and the first
    // `{` after `tail_start`. If the parse fails, the caller
    // persists the head bytes as a single-element vector (raw
    // JSON fragment). This branch should be unreachable in
    // practice — 4 MiB of transcript contains millions of `}`
    // chars — but the defensive fallback is cheap.
    let head_trim = find_last_close_brace(&json[..head_end]);
    let tail_trim_start = find_first_open_brace(&json[tail_start..])
        .map(|i| tail_start + i)
        .unwrap_or(tail_start);
    let truncated_json = if let (Some(h), true) = (head_trim, tail_trim_start < json.len()) {
        // Concatenate head[..h] + tail[tail_trim_start..] wrapped in
        // a synthetic array. The two halves are JSON-serialized
        // TranscriptEntry values; we wrap them in a JSON array.
        format!(
            "[{}]",
            [&json[..h], &json[tail_trim_start..]].join(",")
        )
    } else if let Some(h) = head_trim {
        // Tail parse failed; keep just the head (truncated).
        format!("[{}]", &json[..h])
    } else {
        // Head parse failed too; keep the raw head bytes as a
        // single-element fallback (last resort). The shape is
        // invalid JSON but the transcript_truncated=1 flag tells
        // PR3's render to surface a degraded view.
        return (vec![make_raw_fallback_entry(&json[..head_end])], true);
    };
    match serde_json::from_str::<Vec<TranscriptEntry>>(&truncated_json) {
        Ok(parsed) => (parsed, true),
        Err(_) => (vec![make_raw_fallback_entry(&json[..head_end])], true),
    }
}

/// Find the byte index of the last `}` in `s[..=]` that is at or
/// before `s.len()`. Returns `None` if no `}` is found.
fn find_last_close_brace(s: &str) -> Option<usize> {
    s.rfind('}').map(|i| i + 1)
}

/// Find the byte index of the first `{` in `s[i..]`. Returns
/// `None` if no `{` is found. The returned offset is relative to
/// `s`, not the caller's slice.
fn find_first_open_brace(s: &str) -> Option<usize> {
    s.find('{')
}

/// Build a single fallback `TranscriptEntry` carrying a raw JSON
/// fragment as its payload. Used when the head+tail reparse fails
/// (extremely rare; documented in [`truncate_transcript_for_persistence`]).
fn make_raw_fallback_entry(raw: &str) -> TranscriptEntry {
    TranscriptEntry {
        kind: TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({
            "_truncation_fallback": true,
            "raw_head_bytes": raw,
        }),
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
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
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
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
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
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
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
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
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

    // ---- token usage accumulation (B6 PR2) ----

    fn done_with_usage(input: u32, output: u32) -> ChatEventPayload {
        ChatEventPayload {
            request_id: "rid-u".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
            },
        }
    }

    #[test]
    fn buffer_sink_accumulates_token_usage_per_turn() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(100, 50));
        sink.emit_chat_event(&done_with_usage(200, 30));
        sink.emit_chat_event(&done_with_usage(50, 10));
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 350);
        assert_eq!(total.output_tokens, 90);
    }

    #[test]
    fn buffer_sink_drain_per_turn_usage_clears_buffer() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&done_with_usage(10, 5));
        let drained = sink.drain_per_turn_usage();
        assert_eq!(drained.input_tokens, 10);
        assert_eq!(drained.output_tokens, 5);
        // After drain, the cumulative is zero.
        let after = sink.cumulative_usage();
        assert_eq!(after.input_tokens, 0);
        assert_eq!(after.output_tokens, 0);
    }

    #[test]
    fn buffer_sink_done_without_usage_does_not_accumulate() {
        let sink = SubagentBufferSink::new_without_app_handle("rid".into(), "sid".into());
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".to_string(),
            event: ChatEvent::Done {
                stop_reason: Some("cancelled".to_string()),
                usage: None,
            },
        });
        let total = sink.cumulative_usage();
        assert_eq!(total.input_tokens, 0);
        assert_eq!(total.output_tokens, 0);
    }

    // ---- truncate_transcript_for_persistence (B6 PR2) ----

    fn make_entry(payload: &str) -> TranscriptEntry {
        TranscriptEntry {
            kind: TranscriptKind::ChatEvent,
            payload_json: serde_json::json!({"text": payload}),
        }
    }

    #[test]
    fn truncate_under_cap_returns_original() {
        let entries: Vec<TranscriptEntry> = (0..10)
            .map(|i| make_entry(&format!("entry-{}", i)))
            .collect();
        let (out, truncated) = truncate_transcript_for_persistence(entries.clone(), 4096);
        assert!(!truncated);
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn truncate_at_exact_cap_returns_original() {
        // Build a transcript whose JSON size is exactly the cap.
        let entries: Vec<TranscriptEntry> = (0..5)
            .map(|i| make_entry(&format!("entry-{}", i)))
            .collect();
        let json = serde_json::to_string(&entries).unwrap();
        let (out, truncated) = truncate_transcript_for_persistence(entries, json.len());
        assert!(!truncated, "size == cap should NOT truncate");
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn truncate_over_cap_marks_truncated_and_keeps_entries() {
        // Build a transcript that's ~10 KiB; cap at 1 KiB → truncated.
        let entries: Vec<TranscriptEntry> = (0..200)
            .map(|_| make_entry(&"x".repeat(40)))
            .collect();
        let json = serde_json::to_string(&entries).unwrap();
        assert!(json.len() > 1024, "test setup: should exceed 1KiB");
        let (out, truncated) = truncate_transcript_for_persistence(entries, 1024);
        assert!(truncated, "over cap must set truncated=true");
        // The truncated transcript is smaller in entry count (head+tail only).
        assert!(out.len() < 200);
        // It still parses as valid JSON (verified by re-serializing).
        let re = serde_json::to_string(&out).unwrap();
        assert!(re.len() < json.len());
    }

    #[test]
    fn truncate_empty_transcript_returns_empty() {
        let (out, truncated) = truncate_transcript_for_persistence(Vec::new(), 1024);
        assert!(out.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn truncate_uses_default_4mb_when_called_via_run_subagent_path() {
        // Sanity: the 4 MiB default is what run_subagent uses.
        // Building a > 4 MiB transcript in a unit test is expensive,
        // so we only assert the constant.
        assert_eq!(TRANSCRIPT_MAX_BYTES, 4 * 1024 * 1024);
    }

    // ---- PR2 hotfix: subagent:event IPC payload (B6 PR3, 2026-06-20) ----

    /// The `build_subagent_event_payload` helper produces the
    /// exact wire shape documented in prd.md §"PR2 hotfix":
    /// `{ runId, sessionId, kind, payload, timestamp }`. Locks the
    /// IPC contract so a drift on either side is caught at the
    /// Rust unit-test layer (the TS mirror in PR3b's
    /// `subagentRuns.ts` is the matching assertion).
    #[test]
    fn build_subagent_event_payload_matches_prd_wire_shape() {
        let payload = build_subagent_event_payload(
            "rid-x",
            "sid-y",
            TranscriptKind::ChatEvent,
            serde_json::json!({"hello": "world"}),
        );
        assert_eq!(payload["runId"], "rid-x");
        assert_eq!(payload["sessionId"], "sid-y");
        assert_eq!(payload["kind"], "chat_event");
        assert_eq!(payload["payload"]["hello"], "world");
        // Timestamp is RFC 3339 — contains "T" + a timezone offset
        // (the +00:00 form from Utc::now().to_rfc3339()).
        let ts = payload["timestamp"].as_str().expect("timestamp is string");
        assert!(ts.contains('T'), "RFC 3339 timestamp: {ts}");
    }

    /// Every `TranscriptKind` variant maps to its snake_case wire
    /// string. A drift here would silently break the frontend
    /// drawer (which switches on the kind string).
    #[test]
    fn build_subagent_event_payload_kind_strings_match_enum() {
        for (kind, expected) in [
            (TranscriptKind::ChatEvent, "chat_event"),
            (TranscriptKind::ToolCall, "tool_call"),
            (TranscriptKind::ToolResult, "tool_result"),
            (TranscriptKind::PermissionAsk, "permission_ask"),
        ] {
            let p = build_subagent_event_payload("rid", "sid", kind, serde_json::Value::Null);
            assert_eq!(p["kind"], expected, "kind={kind:?} wire form");
        }
    }

    /// `build_subagent_finished_payload` produces the one-shot
    /// terminal signal wire shape `{ runId, sessionId, status,
    /// finishedAt }`. Locks the `subagent:finished` IPC contract
    /// (the TS mirror is `SubagentFinishedPayload` in
    /// `subagentRuns.ts`). B6 PR3b hotfix (2026-06-21).
    #[test]
    fn build_subagent_finished_payload_matches_wire_shape() {
        let payload = build_subagent_finished_payload(
            "run-uuid-123",
            "sid-y",
            "completed",
            "2026-06-21T12:00:00+00:00",
        );
        assert_eq!(payload["runId"], "run-uuid-123");
        assert_eq!(payload["sessionId"], "sid-y");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["finishedAt"], "2026-06-21T12:00:00+00:00");
        // No `kind` / `payload` / `timestamp` fields — this is NOT a
        // transcript entry (distinct from subagent:event). A drift
        // here would collide with the drawer's transcript rendering.
        assert!(payload.get("kind").is_none());
        assert!(payload.get("payload").is_none());
        assert!(payload.get("timestamp").is_none());
    }

    /// Each `emit_*` method (chat_event / tool_call / tool_result /
    /// permission_ask) appends the corresponding transcript entry
    /// AND (when armed via `new_with_collector`) appends the
    /// corresponding IPC payload. The two writes are paired —
    /// every transcript entry has a matching IPC event with the
    /// same kind.
    #[test]
    fn subagent_buffer_sink_emits_ipc_event_per_emit() {
        // Reset collector.
        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
        let collector: Arc<StdMutex<Vec<serde_json::Value>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let sink = SubagentBufferSink::new_with_collector(
            "rid-pr2".into(),
            "sid-pr2".into(),
            collector.clone(),
        );

        // emit_chat_event → ChatEvent + 1 IPC payload.
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-pr2".into(),
            event: ChatEvent::Start,
        });
        // emit_tool_call → ToolCall + 1 IPC payload.
        sink.emit_tool_call(&ToolCallPayload {
            request_id: "rid-pr2".into(),
            id: "toolu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/x"}),
        });
        // emit_tool_result → ToolResult + 1 IPC payload.
        sink.emit_tool_result(&ToolResultPayload {
            request_id: "rid-pr2".into(),
            tool_use_id: "toolu_1".into(),
            content: "ok".into(),
            is_error: false,
        });
        // emit_permission_ask → PermissionAsk + 1 IPC payload.
        sink.emit_permission_ask(crate::agent::permissions::PermissionAskPayload {
            rid: "ask-rid".into(),
            session_id: "sid-pr2".into(),
            tool_use_id: "toolu_1".into(),
            tool_name: "shell".into(),
            tool_input: serde_json::json!({"command": "rm -rf /"}),
            risk: crate::agent::permissions::Risk::High,
            reason: Some("dangerous".into()),
            path: None,
        });

        // Transcript side: 4 entries, kinds match.
        let transcript = sink.transcript_snapshot();
        assert_eq!(transcript.len(), 4);
        assert_eq!(transcript[0].kind, TranscriptKind::ChatEvent);
        assert_eq!(transcript[1].kind, TranscriptKind::ToolCall);
        assert_eq!(transcript[2].kind, TranscriptKind::ToolResult);
        assert_eq!(transcript[3].kind, TranscriptKind::PermissionAsk);

        // IPC side: 4 payloads, kinds match the transcript 1:1.
        let collected = collector.lock().unwrap().clone();
        assert_eq!(collected.len(), 4, "every emit must produce 1 IPC payload");
        assert_eq!(collected[0]["kind"], "chat_event");
        assert_eq!(collected[1]["kind"], "tool_call");
        assert_eq!(collected[2]["kind"], "tool_result");
        assert_eq!(collected[3]["kind"], "permission_ask");
        // Every payload carries runId / sessionId / payload / timestamp.
        for (i, p) in collected.iter().enumerate() {
            assert_eq!(p["runId"], "rid-pr2", "payload #{i} runId");
            assert_eq!(p["sessionId"], "sid-pr2", "payload #{i} sessionId");
            assert!(p["payload"].is_object() || p["payload"].is_null(),
                    "payload #{i} shape");
            assert!(p["timestamp"].as_str().unwrap().contains('T'),
                    "payload #{i} timestamp is RFC 3339");
        }

        // Cleanup: reset the thread-local so subsequent tests don't
        // see this collector.
        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
    }

    /// `new_without_app_handle` (the default test path) does NOT
    /// emit IPC events — the collector stays empty even after
    /// emits. This locks the "emit is gated on app_handle" path
    /// so a future refactor doesn't accidentally start emitting
    /// from the test path (would crash — there's no Tauri
    /// runtime in unit tests).
    #[test]
    fn subagent_buffer_sink_without_app_handle_does_not_emit_ipc() {
        // No collector armed.
        TEST_COLLECTOR.with(|c| *c.borrow_mut() = None);
        let sink = SubagentBufferSink::new_without_app_handle(
            "rid-noop".into(),
            "sid-noop".into(),
        );
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid-noop".into(),
            event: ChatEvent::Start,
        });
        // Transcript still records the event (no functional
        // regression for the test-only path).
        assert_eq!(sink.transcript_snapshot().len(), 1);
        // But nothing leaked into the (un-armed) collector.
        TEST_COLLECTOR.with(|c| {
            assert!(c.borrow().is_none(), "no collector armed → no IPC attempted");
        });
    }
}
