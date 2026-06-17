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
//! - [`tests`] — all `#[cfg(test)] mod tests` blocks previously
//!   inlined in `lib.rs`.

pub mod at_file;
pub mod chat;
pub mod chat_loop;
pub mod context;
pub mod helpers;
pub mod permissions;
pub mod provider;
pub mod system_prompt;
pub mod tests;
pub mod thinking;

/// Maximum agent loop turns before forced stop (safety limit).
///
/// C3 (2026-06-12): bumped from 20 → 50. The previous 20-turn cap
/// was both the safety net AND the de-facto context-overflow guard.
/// Post-C3, [`context::compact_messages`] handles the real overflow
/// via token-budget trimming; the 50-turn cap is a pure fallback
/// for pathological loops (e.g. a model stuck in a tool-cycle).
pub const MAX_TURNS: usize = 50;