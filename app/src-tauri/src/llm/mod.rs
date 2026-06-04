//! LLM client module — Anthropic Messages API compatible.
//!
//! Per HACKING-llm.md, we hand-write the HTTP + SSE protocol (spike-002
//! verified this is tractable and avoids the rig-core abstraction until
//! step 3). Implementation is split into four files:
//!
//! - [`client`] — HTTP request, response streaming, error normalization
//! - [`sse`] — line-oriented SSE parser
//! - [`error`] — error classification
//! - [`types`] — request / response / event types

pub mod client;
pub mod error;
pub mod sse;
pub mod types;

#[allow(unused_imports)]
pub use client::{chat_stream, chat_stream_with_tools, LlmConfig};
#[allow(unused_imports)]
pub use error::LlmError;
pub use types::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role, ToolDef,
};
