//! Catalog → provider resolution for the chat command.
//!
//! PR2 (multi-model): the chat command used to call
//! `resolve_chat_provider` at the start of every chat to look up
//! the catalog → provider mapping. Post-PR1 (grill decision #3)
//! the catalog is pre-built into `AppState.catalog` and the chat
//! command does a single `catalog.get(&model_id)` lookup.
//!
//! This module still exists because:
//! - The `chat` command's pre-flight failure modes (no default
//!   model, missing provider, empty api_key, build failure) are
//!   still resolved here and surfaced as the locked-in PRD §Q2
//!   user-facing messages.
//! - The `ResolvedChatProvider` / `PreFlightError` types are
//!   shared between chat command callers and future code paths
//!   (e.g. a CLI mode that bypasses Tauri but still needs the
//!   same resolution semantics).

use sqlx::SqlitePool;

use crate::llm::{self, LlmErrorCategory, Provider};

/// PR2 (multi-model) catalog resolution result. The chat command
/// calls [`resolve_chat_provider`] at the start of every chat
/// invocation to obtain the `Box<dyn Provider>` to use for the
/// 20-turn agent loop. The three error variants map 1:1 to the
/// pre-flight error messages the PR2 PRD §Q2 locked in (see
/// `PreFlightError::auth_message` / `invalid_request_message`).
pub struct ResolvedChatProvider {
    pub provider: Box<dyn Provider>,
    pub model_display_name: String,
    pub provider_display_name: String,
}

#[derive(Debug)]
pub enum PreFlightError {
    /// The chosen default model is missing (no `default_model_id`
    /// in `app_config`, or the catalog has no matching `models`
    /// row). PRD Q2 #2: "没有可用 model,请到 Settings 选 default model".
    NoModel,
    /// The chosen default model points at a provider row that was
    /// deleted. PRD Q2 #3: "default model 指向的 provider 已被删除,
    /// 请到 Settings 重选".
    ProviderMissing,
    /// The chosen provider's `api_key` is empty. PRD Q2 #1:
    /// "请到 Settings 填 {provider_display_name} 的 api_key".
    EmptyApiKey { provider_display_name: String },
    /// The dispatch in `build_provider` refused the protocol
    /// (e.g. `openai` not implemented yet, or an unknown protocol
    /// string from a forward-compat DB write).
    BuildFailed(llm::ProviderBuildError),
}

impl PreFlightError {
    /// Return `(user_message, category)`. The user-facing message
    /// follows the locked-in PRD §Q2 copy.
    pub fn user_message_and_category(&self) -> (String, LlmErrorCategory) {
        match self {
            PreFlightError::NoModel => (
                "没有可用 model,请到 Settings 选 default model".to_string(),
                LlmErrorCategory::InvalidRequest,
            ),
            PreFlightError::ProviderMissing => (
                "default model 指向的 provider 已被删除,请到 Settings 重选".to_string(),
                LlmErrorCategory::InvalidRequest,
            ),
            PreFlightError::EmptyApiKey {
                provider_display_name,
            } => (
                format!(
                    "请到 Settings 填 {} 的 api_key",
                    provider_display_name
                ),
                LlmErrorCategory::Auth,
            ),
            PreFlightError::BuildFailed(e) => (
                format!("无法构造 LLM provider: {}", e),
                LlmErrorCategory::InvalidRequest,
            ),
        }
    }
}

/// PR2 catalog resolution. Reads the default model id from
/// `app_config`, finds the corresponding `ModelWithProvider` row,
/// looks up the parent provider's `api_key` (for the pre-flight
/// check), and constructs a `Box<dyn Provider>` via
/// [`llm::build_provider`].
///
/// Returns one of the four [`PreFlightError`] variants on
/// failure. The `chat` command turns the first three into the
/// locked-in PRD §Q2 user-facing messages; the fourth is a
/// generic dispatcher error (e.g. OpenAI not implemented yet)
/// wrapped as `InvalidRequest`.
///
/// Resolved once per `chat` invocation. All 20 turns of the
/// agent loop use the same provider instance so the wire
/// protocol is consistent within a single chat (the user
/// can't change protocol mid-loop — they have to start a new
/// chat for that).
///
/// Post-PR1: the chat command itself prefers `AppState.catalog`
/// for the dispatch (single lookup, no DB roundtrips). This
/// function remains for callers that have only a `SqlitePool`
/// (e.g. CLI tools, tests) and want the full pre-flight +
/// user-facing-error semantics.
pub async fn resolve_chat_provider(
    db: &SqlitePool,
) -> Result<ResolvedChatProvider, PreFlightError> {
    // 1. Find the default model id.
    let default_id = crate::db::get_config_value(db, "default_model_id")
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "resolve_chat_provider: get_config_value failed");
            PreFlightError::NoModel
        })?
        .ok_or(PreFlightError::NoModel)?;

    // 2. Find the matching model row.
    let models = crate::db::list_models(db).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_chat_provider: list_models failed");
        PreFlightError::NoModel
    })?;
    let mwp = models
        .into_iter()
        .find(|m| m.model.id == default_id)
        .ok_or(PreFlightError::NoModel)?;

    // 3. Find the parent provider row. The denormalized
    //    `ModelWithProvider` carries `provider_display_name` +
    //    `provider_protocol` but NOT `api_key` (the secret stays
    //    server-side). We re-read via `list_providers` to get
    //    `api_key` for the pre-flight check.
    let providers = crate::db::list_providers(db).await.map_err(|e| {
        tracing::error!(error = %e, "resolve_chat_provider: list_providers failed");
        PreFlightError::ProviderMissing
    })?;
    let provider_row = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id)
        .ok_or(PreFlightError::ProviderMissing)?;

    // 4. Pre-flight: empty api_key. The factory would still
    //    succeed (it doesn't check), but the resulting
    //    `LlmConfig` would 401 on the first request. Better to
    //    reject here with a clear message.
    if provider_row.api_key.is_empty() {
        return Err(PreFlightError::EmptyApiKey {
            provider_display_name: provider_row.display_name.clone(),
        });
    }

    // 5. Build the provider. `build_provider` itself can fail
    //    (unknown protocol), in which case we surface the typed
    //    `ProviderBuildError` for the chat command to wrap as an
    //    `InvalidRequest` IPC error.
    let provider = llm::build_provider(&provider_row, &mwp.model)
        .map_err(PreFlightError::BuildFailed)?;

    Ok(ResolvedChatProvider {
        provider,
        model_display_name: mwp.model.display_name.clone(),
        provider_display_name: provider_row.display_name.clone(),
    })
}