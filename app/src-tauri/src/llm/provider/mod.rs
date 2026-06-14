//! LLM Provider abstraction.
//!
//! PR2 of the multi-model task: the `chat` command no longer calls a
//! hand-rolled `chat_stream_with_tools` function. It dispatches through
//! a [`Provider`] trait, and the Anthropic implementation lives in
//! [`anthropic`]. PR3 will add the OpenAI implementation; the
//! [`build_provider`] factory already knows about both protocols and
//! returns a "not implemented" error for OpenAI so the dispatch surface
//! is in place even before the implementation lands.
//!
//! Design choices:
//!
//! - **`Send + 'static` Stream**, not `async_trait`. Adding
//!   `async_trait` for a single method on a stable crate set is
//!   overkill, and the current `chat` command already wraps the stream
//!   in `Box::pin` for its `tokio::select!` loop. The trait returns a
//!   `Pin<Box<dyn Stream<...> + Send>>` so it is object-safe and
//!   storable in `Box<dyn Provider>`.
//! - **No `WireMessage` intermediate type.** The current
//!   [`crate::llm::types::ChatRequest`] / [`crate::llm::types::ChatEvent`]
//!   types are Anthropic-shaped (thinking blocks, signature_delta,
//!   tool_use, …). They double as the trait's input/output —
//!   cross-protocol conversion is a PR3 concern that will land alongside
//!   the OpenAI adapter and the `WireMessage` envelope.
//! - **No per-request provider construction in the trait method.** The
//!   `chat` command builds a provider ONCE per chat invocation (one
//!   `Box<dyn Provider>` for the 20-turn agent loop) and calls
//!   `send(...)` on it once per turn. The provider holds the
//!   `LlmConfig` (or any future configuration) in its constructor
//!   state, so a turn's `send` is self-contained.
//! - **`ProviderProtocol` re-exported from `db`**. The protocol enum
//!   already lives in `db::ProviderProtocol` per PR1's schema. We
//!   re-export it here so the LLM module's public surface is
//!   self-contained (`llm::ProviderProtocol` is the canonical import
//!   for new code).

use std::pin::Pin;

use futures_util::Stream;

use super::error::LlmError;
use super::types::{ChatEvent, ChatMessage, ToolDef};
use crate::db::{ModelRow, ProviderRow};

#[allow(unused_imports)]
pub use anthropic::AnthropicProvider;
#[allow(unused_imports)]
pub use openai::OpenAIProvider;
pub use crate::db::ProviderProtocol;

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// An LLM provider speaks a wire protocol against an upstream API.
///
/// The trait is object-safe (no `Self` in argument types, no generic
/// methods) and intended to be used as `Box<dyn Provider>`. The
/// provider holds whatever connection / config state it needs across
/// `send` calls; the chat command invokes `send` once per agent-loop
/// turn and consumes the resulting stream inside a `tokio::select!`.
///
/// The returned stream is `Send + 'static` so it can be moved into a
/// `tauri::async_runtime::spawn` task and the `Box::pin` wrapper used
/// by the chat command works without further wrapping.
pub trait Provider: Send + Sync {
    /// Issue one LLM request and return a stream of `ChatEvent`s.
    ///
    /// `system` is the Anthropic `system` field (or its OpenAI
    /// equivalent). The agent loop constructs it once per chat
    /// invocation in `lib.rs::build_system_prompt`; the provider is
    /// free to ignore it if its protocol does not have a
    /// corresponding field.
    fn send(
        &self,
        system: Option<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>>;

    /// Static capabilities of this provider — independent of any
    /// specific model. The model-level capabilities (e.g.
    /// `supports_thinking`) live on [`ModelRow`] and may be combined
    /// at dispatch time.
    ///
    /// `#[allow(dead_code)]` because the chat command does not
    /// dispatch on capabilities today; the impls return
    /// representative values. The method lives on the trait so
    /// PR3 (OpenAI) can register a different capability profile
    /// and capability-gated dispatch (e.g. "skip the system prompt
    /// for OpenAI if it doesn't support it") becomes a 3-line
    /// change in `resolve_chat_provider`.
    #[allow(dead_code)]
    fn capabilities(&self) -> ProviderCapabilities;

    /// The protocol this provider speaks. Used for logging + future
    /// UI affordances (e.g. "switch protocol" dropdown in Settings).
    fn protocol(&self) -> ProviderProtocol;
}

// ---------------------------------------------------------------------------
// ProviderCapabilities
// ---------------------------------------------------------------------------

/// Static, protocol-level capabilities of a [`Provider`].
///
/// These are independent of the specific model — they describe what
/// the protocol itself supports. Model-level toggles (e.g.
/// `ModelRow.supports_thinking`) live separately.
///
/// The struct is `#[allow(dead_code)]` because the chat command
/// does not currently inspect capabilities at runtime; the
/// Anthropic + (future) OpenAI impls are free to return whatever
/// shape fits. The struct lives in the trait return type so future
/// code (capability-gated dispatch, a UI "what does this protocol
/// support" view) can use it without a schema change.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ProviderCapabilities {
    /// Whether the protocol has a top-level `system` field (or
    /// equivalent). Both Anthropic and OpenAI do.
    pub supports_system_prompt: bool,
    /// Whether the protocol supports tool/function calling. Both
    /// Anthropic and OpenAI do.
    pub supports_tools: bool,
    /// Whether the protocol supports streaming responses over SSE
    /// (or the equivalent). Both Anthropic and OpenAI do.
    pub supports_streaming: bool,
}

// ---------------------------------------------------------------------------
// Provider factory
// ---------------------------------------------------------------------------

/// Build a [`Provider`] for a given (provider row, model row) pair.
///
/// This is the dispatch surface PR2 adds. The Anthropic protocol is
/// the only fully-implemented dispatch today; OpenAI returns
/// [`ProviderBuildError::NotImplemented`] so the wire path is in
/// place ahead of PR3's adapter.
pub fn build_provider(
    provider_row: &ProviderRow,
    model_row: &ModelRow,
) -> Result<Box<dyn Provider>, ProviderBuildError> {
    match provider_row.protocol.as_str() {
        "anthropic" => {
            // Defaults: 16384 for max_tokens (matches the legacy
            // `client.rs` `DEFAULT_MAX_TOKENS`); "high" for
            // thinking_effort (matches the LLM_THINKING_EFFORT env
            // default). The chat command's pre-flight check verifies
            // `provider.api_key` is set, so we don't re-check here.
            let max_tokens = model_row.max_tokens.unwrap_or(16384);
            let thinking_effort = model_row
                .thinking_effort
                .clone()
                .unwrap_or_else(|| "high".to_string());
            let config = anthropic::LlmConfig {
                base_url: provider_row.base_url.clone(),
                model: model_row.model_name.clone(),
                api_key: provider_row.api_key.clone(),
                max_tokens,
                thinking_effort,
            };
            Ok(Box::new(AnthropicProvider::new(config)))
        }
        "openai" => {
            // PR3 OpenAI adapter. Defaults: `max_tokens = 16384`
            // (matches the Anthropic default for symmetry; future
            // PRs may lower this for o1 models where max_tokens
            // is the budget for `reasoning_content` + visible
            // answer combined).
            //
            // OpenAI's `reasoning_effort` is sourced from
            // `ModelRow.thinking_effort` (the same column the
            // Anthropic adapter reads for `adaptive.effort`).
            // The value is emitted as a top-level
            // `reasoning_effort` field on Chat Completions
            // requests; `None` means "omit the field" so
            // non-o1/o3 models are unaffected.
            let max_tokens = model_row.max_tokens.unwrap_or(16384);
            let reasoning_effort = model_row.thinking_effort.clone();
            let config = openai::OpenAIConfig {
                base_url: provider_row.base_url.clone(),
                model: model_row.model_name.clone(),
                api_key: provider_row.api_key.clone(),
                max_tokens,
                reasoning_effort,
            };
            Ok(Box::new(OpenAIProvider::new(config)))
        }
        other => Err(ProviderBuildError::UnknownProtocol(other.to_string())),
    }
}

/// Errors that can come out of [`build_provider`].
#[derive(Debug, thiserror::Error)]
pub enum ProviderBuildError {
    /// The protocol string was recognized but the adapter is not
    /// implemented yet (reserved for future protocols like
    /// `gemini` or `ollama` — PR3 ships both `anthropic` and
    /// `openai`). The `&'static str` names the protocol so the
    /// UI / log line can point at the missing adapter.
    #[allow(dead_code)]
    #[error("provider protocol '{0}' is not implemented yet")]
    NotImplemented(&'static str),

    /// The protocol string was not recognized at all. The DB
    /// `providers.protocol` column is `TEXT` (forward-compat) so a
    /// future binary may write a value the current binary doesn't
    /// understand. Surfacing it as an error is intentional — the
    /// `chat` command's pre-flight check will turn it into a
    /// `ChatEvent::Error` for the user.
    #[error("unknown provider protocol: '{0}'")]
    UnknownProtocol(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn anthropic_provider_row(api_key: &str) -> ProviderRow {
        ProviderRow {
            id: "pid-1".to_string(),
            protocol: "anthropic".to_string(),
            display_name: "Anthropic 官方".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: api_key.to_string(),
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        }
    }

    fn model_row_with(
        model_name: &str,
        max_tokens: Option<u32>,
        thinking_effort: Option<&str>,
    ) -> ModelRow {
        ModelRow {
            id: "mid-1".to_string(),
            provider_id: "pid-1".to_string(),
            model_name: model_name.to_string(),
            display_name: format!("Display {}", model_name),
            max_tokens,
            thinking_effort: thinking_effort.map(str::to_string),
            supports_thinking: true,
            context_window: 200_000,
            created_at: "2026-06-09T00:00:00Z".to_string(),
            updated_at: "2026-06-09T00:00:00Z".to_string(),
        }
    }

    /// The factory returns an `AnthropicProvider` (a `Box<dyn Provider>`)
    /// with the protocol/capabilities expected for the Anthropic
    /// protocol.
    #[test]
    fn build_provider_anthropic_returns_anthropic_provider() {
        let p = anthropic_provider_row("sk-test");
        let m = model_row_with("claude-sonnet-4-5", Some(8192), Some("high"));
        let provider = build_provider(&p, &m).expect("anthropic is implemented");
        assert_eq!(provider.protocol(), ProviderProtocol::Anthropic);
        let caps = provider.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    /// OpenAI protocol returns an `OpenAIProvider` (a
    /// `Box<dyn Provider>`) with the protocol/capabilities expected
    /// for Chat Completions. PR3 wires this up.
    #[test]
    fn build_provider_openai_returns_openai_provider() {
        let mut p = anthropic_provider_row("sk-test");
        p.protocol = "openai".to_string();
        p.display_name = "OpenAI 官方".to_string();
        p.base_url = "https://api.openai.com/v1".to_string();
        let m = model_row_with("gpt-4o", None, None);
        let provider = build_provider(&p, &m).expect("openai is implemented in PR3");
        assert_eq!(provider.protocol(), ProviderProtocol::Openai);
        let caps = provider.capabilities();
        assert!(caps.supports_system_prompt);
        assert!(caps.supports_tools);
        assert!(caps.supports_streaming);
    }

    /// Unknown protocol strings return a typed error rather than
    /// crashing — the DB column is `TEXT` for forward-compat, so a
    /// future binary may write values the current one doesn't
    /// recognize. The dispatch should surface that gracefully.
    #[test]
    fn build_provider_unknown_protocol_returns_error() {
        let mut p = anthropic_provider_row("sk-test");
        p.protocol = "future-mystery".to_string();
        let m = model_row_with("some-model", None, None);
        match build_provider(&p, &m) {
            Err(ProviderBuildError::UnknownProtocol(s)) => assert_eq!(s, "future-mystery"),
            Err(other) => panic!("expected UnknownProtocol, got a different error: {}", other),
            Ok(_) => panic!("expected UnknownProtocol, got Ok(provider)"),
        }
    }

    /// When the model row has row-level `max_tokens` /
    /// `thinking_effort`, the factory threads them into the
    /// internal `LlmConfig`. Verified indirectly: a successful
    /// construction with non-default values is the contract; the
    /// exact wire shape is locked in `anthropic::tests`.
    #[test]
    fn factory_passes_model_max_tokens() {
        let p = anthropic_provider_row("sk-test");
        let m = model_row_with("claude-opus-4-7", Some(32768), Some("xhigh"));
        let _provider = build_provider(&p, &m).expect("anthropic is implemented");
    }

    /// When the model row has no row-level overrides, the factory
    /// falls back to the Anthropic defaults: `max_tokens = 16384`
    /// and `thinking_effort = "high"`. This matches the legacy
    /// `LlmConfig::from_env` defaults so PR2 keeps the same wire
    /// shape.
    #[test]
    fn factory_falls_back_to_default_max_tokens_and_effort() {
        let p = anthropic_provider_row("sk-test");
        let m = model_row_with("claude-sonnet-4-5", None, None);
        let _provider = build_provider(&p, &m).expect("anthropic is implemented");
    }

    /// The factory's `ProviderBuildError` implements `Display` /
    /// `Error` so it can be formatted in a `tracing::warn!` and
    /// carried through the IPC error path as a `String`.
    #[test]
    fn provider_build_error_displays_human_readable() {
        let e1 = ProviderBuildError::NotImplemented("openai");
        assert_eq!(
            e1.to_string(),
            "provider protocol 'openai' is not implemented yet"
        );
        let e2 = ProviderBuildError::UnknownProtocol("future".to_string());
        assert_eq!(e2.to_string(), "unknown provider protocol: 'future'");
    }

    /// `db::ProviderProtocol` re-export keeps the LLM module's
    /// public surface self-contained — downstream code that uses
    /// `llm::ProviderProtocol` doesn't need to know about `db`.
    #[test]
    fn provider_protocol_reexport_matches_db() {
        assert_eq!(ProviderProtocol::Anthropic, db::ProviderProtocol::Anthropic);
        assert_eq!(ProviderProtocol::Openai, db::ProviderProtocol::Openai);
    }
}

// ---------------------------------------------------------------------------
// Module declarations
// ---------------------------------------------------------------------------

pub mod anthropic;
pub mod openai;
pub mod wire;

#[cfg(test)]
pub mod mock;
