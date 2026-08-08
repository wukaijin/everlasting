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
//! `LlmConfig` lives inside `provider::anthropic` and is constructed
//! only by the `build_provider` factory from catalog rows. It is not
//! re-exported here — there are no external consumers after the env
//! fallback path was removed.

pub mod error;
pub mod provider;
pub mod retry;
pub mod sse;
pub mod types;

#[allow(unused_imports)]
pub use error::LlmError;
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

#[cfg(test)]
mod tests_types;
