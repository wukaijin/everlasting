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
//!    `max_turns: Some(SUBAGENT_MAX_TURNS)` (= 200, see
//!    [`dispatch::SUBAGENT_MAX_TURNS`]).
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

mod cache;
mod definition;
pub(crate) mod dispatch;
mod event_sink;
mod frontmatter;
mod loader;
mod prep;
mod prompt;
mod registry;
mod resolve;
mod sink;
mod tests_dispatch;
mod tests_loader;
#[cfg(test)]
mod tests_mod;
mod tests_sink;
mod tools_filter;
mod transcript;
mod truncate_summary;
mod worktree;

// transport-abstraction 2026-07-20 (P1.3): re-export the new
// subagent event sink trait + its two impls so callers reach it
// via `crate::agent::subagent::{SubagentEventSink,
// AppHandleSubagentSink, ThreadLocalSubagentSink}`. Test-only
// helpers `arm_test_collector` / `clear_test_collector` are
// re-exported at `pub(crate)` so the `#[cfg(test)]` blocks in
// `sink.rs` can reach them via `super::super::arm_test_collector` /
// `super::super::clear_test_collector` without exposing the
// `event_sink` module itself.
//
// 2026-07-21 (P2.2 follow-up): the `pub(crate) use` is gated under
// `cfg(test)` because the only consumers are the `#[cfg(test)]`
// blocks in `sink.rs` (`subagent_buffer_sink_emits_ipc_event_per_emit`
// and friends). In production builds the import triggered an
// `unused_imports` warning.
#[cfg(test)]
pub(crate) use event_sink::{arm_test_collector, clear_test_collector};
pub use event_sink::{AppHandleSubagentSink, SubagentEventSink, ThreadLocalSubagentSink};

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
pub use cache::SubagentCache;
#[allow(unused_imports)]
pub use loader::{LoadedSubagent, SubagentSource};
pub use sink::SubagentBufferSink;
pub use transcript::TranscriptEntry;
// `TranscriptKind` + the two wire-shape builders are consumed by
// `cfg(test)` code (`db/tests.rs`, `agent/tests.rs`) AND, since P2.3
// (2026-07-21, task `07-20-remote-access-daemon-split`), by
// `daemon::sse::HttpSseSubagentSink` — the HTTP/SSE counterpart of
// `AppHandleSubagentSink`. Both sinks must emit the *same*
// `subagent:event` / `subagent:finished` JSON, so the builders stay
// the single source of truth and are re-exported crate-wide. The
// earlier `cfg(test)`-only re-export reflected a time when no non-test
// module implemented `SubagentEventSink`; P2.3 lifts that.
pub use transcript::TranscriptKind;
pub(crate) use transcript::{build_subagent_event_payload, build_subagent_finished_payload};
pub use truncate_summary::{
    format_dispatch_result_with_model, format_final_text, summarize_worker_tool_actions,
    truncate_messages_for_persistence, truncate_transcript_for_persistence, MESSAGES_MAX_BYTES,
    TRANSCRIPT_MAX_BYTES,
};
// 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 2/3):
// the `commands::subagents` IPC needs to locate the on-disk file
// for user / project agents (`set_subagent_model` routes the
// write to the frontmatter `.md` file vs. the DB override
// table) + to perform the line-level frontmatter edit
// (`write_frontmatter_model`). Both helpers live in the
// `loader` module (private to the agent subagent family) —
// re-export them here so the IPC layer can reach them via
// `crate::agent::subagent::*` without making the whole loader
// module `pub`.
pub use cache::locate_agent_file;
pub use loader::write_frontmatter_model;

// batch3 (2026-08-08): re-export the 4 inline clusters split into
// submodules so existing `crate::agent::subagent::<sym>` callers
// (chat_loop.rs, state.rs, tests_subagent.rs, etc.) keep resolving.
#[allow(unused_imports)]
pub use definition::{definition, definition_with_cache, DISPATCH_TOOL_NAME};
#[allow(unused_imports)]
pub use prompt::{assemble_subagent_prompt, build_worker_messages};
#[allow(unused_imports)]
pub use registry::{builtin_subagents, lookup_subagent, SubagentDef};
#[allow(unused_imports)]
pub use tools_filter::{filter_tools_for_subagent, filter_tools_readonly, READONLY_TOOL_ALLOWLIST};

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
    /// B6+ B (task 07-06-b6plus-b-dispatch-model-arg): optional
    /// per-dispatch model override. Parsed from an
    /// `@@agent --model=<X> <task>` prefix by the frontend; `<X>`
    /// may be a model id or display_name, and the frontend resolves
    /// it to a model **id** via `useModelsStore` before sending
    /// (the wire carries only ids — same shape as the LLM path
    /// after its display_name reverse-lookup in
    /// `resolve_model_by_name_or_id`). `None` = no per-dispatch
    /// override (worker uses its configured default: Settings
    /// per-agent override > frontmatter `model:` > parent's model).
    #[serde(default)]
    pub model_id: Option<String>,
}

/// B6+ B (task 07-06-b6plus-b-dispatch-model-arg): a minimal
/// (id, display_name) projection of a model row, used to build the
/// dynamic `model` enum on the `dispatch_subagent` tool schema. The
/// enum values are display_names (human-readable) so the LLM does
/// not have to guess UUIDs — the system prompt does not list models,
/// so the tool schema's enum is the LLM's only discovery channel.
/// Built once per `run_chat_loop` invocation from `list_models`
/// (a session-level snapshot; model CRUD during a session is
/// reflected next session and covered by the catalog-miss fallback
/// in `resolve_worker_provider`).
#[derive(Clone, Debug)]
pub struct ModelBrief {
    /// The model's catalog key (UUID). Currently only `display_name`
    /// feeds the schema enum, so non-test lib code writes but does
    /// not read `id`; it is kept on the struct so the snapshot
    /// carries the full id↔display_name pairing for tests and future
    /// callers (e.g. a debug panel listing models by id). Tests read
    /// it via the `ModelBrief` constructor.
    #[allow(dead_code)]
    pub id: String,
    pub display_name: String,
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
