//! Agent core: the agent loop, system prompt construction, and
//! the helpers used by the chat command.
//!
//! Post-PR1 of the audit task, this module owns all the
//! "what does the agent do during a chat?" logic. The thin IPC
//! shim lives in [`crate::commands`].
//!
//! Submodules:
//! - [`chat`] — the `chat` Tauri command + the spawned agent loop
//!   (max 20 turns, cancellation-aware).
//! - [`provider`] — `resolve_chat_provider` + `PreFlightError`
//!   (pre-flight catalog resolution; used by chat for the
//!   user-facing error path).
//! - [`system_prompt`] — `build_system_prompt` +
//!   `lookup_head_sha` (Step 4 follow-up Bug 3).
//! - [`thinking`] — `PendingThinking` accumulator +
//!   `flush_pending_thinking` (handles the per-turn thinking-block
//!   assembly).
//! - [`helpers`] — `tool_result_envelope` (REQ-16),
//!   `build_synthetic_tool_result_message` (BUG FIX 2013),
//!   `persist_turn_cwd`, `emit_chat_event`, and the
//!   `cancel_inflight_for_session` helper shared with the
//!   worktree commands.
//! - [`tests`] — all `#[cfg(test)] mod tests` blocks previously
//!   inlined in `lib.rs`.

pub mod chat;
pub mod helpers;
pub mod provider;
pub mod system_prompt;
pub mod tests;
pub mod thinking;

/// Maximum agent loop turns before forced stop (safety limit).
pub const MAX_TURNS: usize = 20;