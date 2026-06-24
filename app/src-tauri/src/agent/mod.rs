//! Agent core: the agent loop, system prompt construction, and
//! the helpers used by the chat command.
//!
//! Post-PR1 of the audit task, this module owns all the
//! "what does the agent do during a chat?" logic. The thin IPC
//! shim lives in [`crate::commands`].
//!
//! Submodules:
//! - [`at_file`] — B2 PR2 `@relpath` file-content injection (text →
//!   `read_file`-format content; image/PDF/Office/binary → placeholder).
//! - [`behavior_prompt`] — `DEFAULT_BEHAVIOR_PROMPT`: the stable agent
//!   behavior layer (tone / objectivity / tool usage / code conventions
//!   / finishing / git safety / language), injected at the front of the
//!   system prompt (`behavior_prompt + mode_prefix + base_prompt`).
//! - [`chat`] — the `chat` Tauri command + the spawned agent loop
//!   (max 50 turns, cancellation-aware).
//! - [`provider`] — `resolve_chat_provider` + `PreFlightError`
//!   (pre-flight catalog resolution; used by chat for the
//!   user-facing error path).
//! - [`system_prompt`] — `build_system_prompt` +
//!   `lookup_head_sha` (Step 4 follow-up Bug 3).
//! - [`thinking`] — `PendingThinking` accumulator +
//!   `flush_pending_thinking` (handles the per-turn thinking-block
//!   assembly).
//! - [`context`] — C3 Context compression + token budget management
//!   (`estimate_messages_tokens` / `compact_messages`). Triggered
//!   before every `provider.send()` to keep the conversation under
//!   the model's `context_window`.
//! - [`helpers`] — `tool_result_envelope` (REQ-16),
//!   `build_synthetic_tool_result_message` (BUG FIX 2013),
//!   `persist_turn_cwd`, `emit_chat_event`, and the
//!   `cancel_inflight_for_session` helper shared with the
//!   worktree commands.
//! - [`loop_detection`] — C2 ⑬ loop detection (anti death-loop):
//!   two-level scheme (exact-signature hard trigger + Jaccard soft
//!   hint) that catches the model stuck repeating a tool call before
//!   the `MAX_TURNS` backstop; pure functions wired into
//!   `run_chat_loop` (PR2).
//! - test suite (split 2026-06-23 out of a single `tests.rs`):
//!   [`tests_common`] (shared `TestHarness`/`MockEmitter`/`make_harness`
//!   helpers) + domain files [`tests_cancellation`] /
//!   [`tests_envelope`] / [`tests_prompts`] / [`tests_agent_loop`]
//!   / [`tests_subagent`].

pub mod at_file;
pub mod behavior_prompt;
pub mod chat;
pub mod chat_loop;
pub mod context;
pub mod helpers;
pub mod loop_detection;
pub mod permissions;
pub mod provider;
pub mod subagent;
pub mod system_prompt;
pub mod tests_agent_loop;
pub mod tests_cancellation;
pub mod tests_common;
pub mod tests_envelope;
pub mod tests_prompts;
pub mod tests_subagent;
pub mod thinking;

/// Maximum agent loop turns before forced stop (safety limit).
///
/// C3 (2026-06-22): bumped 20 → 50 → 200. The previous 20-turn cap
/// was both the safety net AND the de-facto context-overflow guard.
/// Post-C3, [`context::compact_messages`] handles the real overflow
/// via token-budget trimming; the 200-turn cap is a pure fallback
/// for pathological loops (e.g. a model stuck in a tool-cycle).
pub const MAX_TURNS: usize = 200;
