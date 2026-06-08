//! LLM client module — multi-protocol dispatch (PR2 of multi-model).
//!
//! Module layout (post-PR2):
//! - [`provider`] — the `Provider` trait, `ProviderCapabilities`,
//!   `ProviderProtocol` re-export, and the `build_provider` factory.
//!   `provider::anthropic` holds the Anthropic Messages API
//!   implementation.
//! - [`sse`] — line-oriented SSE parser (unchanged).
//! - [`error`] — error classification (unchanged).
//! - [`types`] — request / response / event types (Anthropic-shaped,
//!   unchanged; cross-protocol types will land in PR3).
//!
//! Backward-compat re-exports: `LlmConfig` is now defined inside
//! `provider::anthropic` (the Anthropic adapter is the only consumer
//! of these fields after PR2). We re-export it at the LLM-module level
//! so `AppState::load` (which still reads env for the cold-start
//! fallback) can keep its `llm::LlmConfig` import path.

pub mod error;
pub mod provider;
pub mod sse;
pub mod types;

#[allow(unused_imports)]
pub use error::LlmError;
#[allow(unused_imports)]
pub use provider::anthropic::LlmConfig;
pub use provider::{build_provider, Provider, ProviderBuildError};
// `AnthropicProvider` and `ProviderCapabilities` are re-exported for
// `use llm::AnthropicProvider;` callers; the chat command reaches
// them via `llm::build_provider` (returning `Box<dyn Provider>`),
// so allow-dead-code on these direct exports keeps the public
// surface self-documenting without forcing every downstream user.
#[allow(unused_imports)]
pub use provider::{AnthropicProvider, ProviderCapabilities};
pub use types::{
    ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role, ToolDef,
};
