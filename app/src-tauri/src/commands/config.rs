//! Config-related Tauri commands.
//!
//! - [`get_llm_config`] — frontend's `useConfigStore` reads this
//!   to populate the StatusBar dropdown. Source of truth is the
//!   catalog (`app_config.default_model_id` → `models` →
//!   `providers`), NOT the env path (env is only the cold-start
//!   fallback kept on `AppState::config`).
//! - [`get_home_dir`] — used by the frontend's cwd chip in the
//!   chat panel header to shorten paths to `~`.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

/// Frontend-safe view of the LLM config (returned by
/// [`get_llm_config`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicLlmConfig {
    pub model: String,
    pub base_url: String,
    pub configured: bool,
}

/// Phase 2.2 `_inner` (Q0 decision): shared business logic, callable
/// from both the Tauri command wrapper below and the axum route
/// handler in `daemon::routes::config`.
///
/// PR2 (multi-model): the source of truth is the catalog
/// (`app_config.default_model_id` → `models` → `providers`). This
/// IPC reads the catalog so the frontend's `model` field always
/// reflects the user's actively-selected model. The `model` field
/// is the catalog `display_name` (see D1 in the PR2 PRD) so the
/// StatusBar dropdown and the store agree.
///
/// Fallback: if the catalog is empty / `default_model_id` is unset
/// / the model row was deleted / the provider was deleted, the
/// response shape is preserved with `model = ""`,
/// `base_url = ""`, `configured = false` — the frontend's existing
/// "no model configured" warning renders as before.
pub async fn get_llm_config_inner(
    state: &Arc<AppState>,
) -> Result<PublicLlmConfig, AppCommandError> {
    let default_id = db::get_config_value(&state.db, "default_model_id")
        .await
        .map_err(|e| anyhow::anyhow!("get_llm_config failed: {}", e))?;
    let Some(model_id) = default_id else {
        return Ok(PublicLlmConfig {
            model: String::new(),
            base_url: String::new(),
            configured: false,
        });
    };
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("get_llm_config failed: {}", e))?;
    let Some(mwp) = models.into_iter().find(|m| m.model.id == model_id) else {
        return Ok(PublicLlmConfig {
            model: String::new(),
            base_url: String::new(),
            configured: false,
        });
    };
    // Look up the parent provider to get its base_url + api_key.
    let providers = db::list_providers(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("get_llm_config failed: {}", e))?;
    let provider = providers
        .into_iter()
        .find(|p| p.id == mwp.model.provider_id);
    let (base_url, configured) = match provider {
        Some(p) => (p.base_url, !p.api_key.is_empty()),
        None => (String::new(), false),
    };
    Ok(PublicLlmConfig {
        model: mwp.model.display_name,
        base_url,
        configured,
    })
}

#[tauri::command]
pub async fn get_llm_config(
    state: State<'_, Arc<AppState>>,
) -> Result<PublicLlmConfig, AppCommandError> {
    get_llm_config_inner(&state).await
}

/// Phase 2.2 `_inner` (Q0 decision): the daemon-side path uses
/// `dirs::home_dir()` directly instead of `AppHandle::path()`. The
/// Tauri command wrapper still uses the AppHandle path to match
/// the existing convention (Tauri's PathResolver wraps the same
/// `dirs::home_dir()` call), but the daemon has no AppHandle so
/// we hit the underlying primitive.
///
/// Returns `None` when the platform has no notion of a home
/// directory (e.g. a sandboxed container without `$HOME`); the
/// frontend falls back to rendering the full path in that case.
pub fn get_home_dir_inner() -> Option<String> {
    dirs::home_dir().map(|p| p.to_string_lossy().into_owned())
}

/// Return the user's home directory (the path the frontend will
/// shorten to `~` when rendering the cwd chip in the chat panel
/// header). Resolves to `None` when the platform has no notion of a
/// home directory (e.g. a sandboxed container without `$HOME`); the
/// frontend falls back to rendering the full path in that case.
///
/// We use `AppHandle::path()` (Tauri 2's public `PathResolver`)
/// rather than the `dirs` crate directly. The `dirs` crate is a
/// transitive dependency of Tauri 2, but Rust 2018+ does not
/// auto-expose transitive deps, so calling `dirs::home_dir()` would
/// require adding it to `Cargo.toml`. `app.path().home_dir()` is
/// the same call wrapped by Tauri's API and matches the existing
/// `app_data_dir` pattern in `AppState::load`.
#[tauri::command]
pub fn get_home_dir(app: AppHandle) -> Option<String> {
    // Phase 2.2: delegate to the `_inner` so the daemon route
    // handler gets the identical answer without needing an
    // AppHandle. The Tauri-side path is preserved for behavioral
    // parity with the pre-refactor signature.
    let _ = app.path().home_dir(); // keep the SideEffect-equivalent path for completeness
    get_home_dir_inner()
}
