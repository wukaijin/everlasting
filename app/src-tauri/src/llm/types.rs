//! LLM request / response / event types.
//!
//! Step 2 extends the step 1 types to support Anthropic-style tool calling:
//! - `ContentBlock` for structured message content (text / tool_use / tool_result)
//! - `MessageContent` with custom Serde to accept both plain string and block array
//! - `ToolDef` for declaring tools in the request
//! - `ChatEvent` gains the `ToolCall` variant (tool results are pushed on the
//!   independent `tool:result` IPC channel via `ToolResultPayload`, not as
//!   `ChatEvent` variants — see `state::ChatEventSink::emit_tool_result`)
//!
//! Step 6 (this task) adds extended thinking support:
//! - `ContentBlock::Thinking` and `ContentBlock::RedactedThinking` (Anthropic
//!   extended-thinking content blocks).
//! - `ChatRequest::thinking` accepts an optional `ThinkingConfig` (currently
//!   the `adaptive` variant). When present, the request asks the model to
//!   think before answering.
//! - `ChatEvent::ThinkingDelta`, `ChatEvent::SignatureDelta` and
//!   `ChatEvent::RedactedThinkingDelta` are streamed to the frontend as the
//!   model emits `thinking_delta` / `signature_delta` SSE events and as
//!   `redacted_thinking` content blocks close.
//!
//! Hub for the `types/` directory module (split 2026-08-08 batch3). The type
//! definitions live in the submodules below; this hub re-exports the public
//! surface so the existing `llm/mod.rs` `pub use types::{...}` and all
//! `crate::llm::types::*` callers keep resolving unchanged.

mod chat;
mod event;
mod message;
mod request;
mod usage;

#[allow(unused_imports)]
pub use chat::{AttachmentRef, ChatMessage, ToolDef};
#[allow(unused_imports)]
pub use event::{ChatEvent, LlmErrorCategory, RecallHit};
#[allow(unused_imports)]
pub use message::{CacheControl, ContentBlock, ImageSource, MessageContent, Role};
#[allow(unused_imports)]
pub use request::{ChatRequest, ThinkingConfig};
#[allow(unused_imports)]
pub use usage::TokenUsage;
