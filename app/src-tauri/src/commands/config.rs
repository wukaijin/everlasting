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
use crate::error::{AppCommandError, ErrorCategory};
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

// ---------------------------------------------------------------------------
// S2 remote tunnel 配置(2026-08-11, task `08-11-tunnel-client`,design §3.1)
//
// 三层模式(`_inner` + `#[tauri::command]` + daemon route):业务逻辑全在
// `_inner`,Tauri 与 axum 两条路径共用。配置存 `app_config` KV
// (零 migration,design §3.2),key 常量单源在
// `daemon::tunnel::config`(load_remote_config 等共用)。
// ---------------------------------------------------------------------------

/// `get_remote_config` 返回体(design §3.1:`{remoteUrl, sharedSecret} | null`)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfigPayload {
    pub remote_url: String,
    pub shared_secret: String,
}

/// `get_tunnel_status` 返回体(design §3.1:
/// `{connected, remoteUrl, nodeId} | null`;`lastError` 为附加诊断字段)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatusPayload {
    pub connected: bool,
    pub remote_url: String,
    pub node_id: String,
    pub last_error: Option<String>,
}

/// 读 remote 配置。`remote_url` key 缺失或为空 → `None`(未配置)。
pub async fn get_remote_config_inner(
    state: &Arc<AppState>,
) -> Result<Option<RemoteConfigPayload>, AppCommandError> {
    let Some(remote_url) =
        db::get_config_value(&state.db, crate::daemon::tunnel::config::KEY_REMOTE_URL)
            .await
            .map_err(|e| anyhow::anyhow!("get_remote_config failed: {}", e))?
    else {
        return Ok(None);
    };
    let remote_url = remote_url.trim().to_string();
    if remote_url.is_empty() {
        return Ok(None);
    }
    let shared_secret =
        db::get_config_value(&state.db, crate::daemon::tunnel::config::KEY_SHARED_SECRET)
            .await
            .map_err(|e| anyhow::anyhow!("get_remote_config failed: {}", e))?
            .unwrap_or_default();
    Ok(Some(RemoteConfigPayload {
        remote_url,
        shared_secret,
    }))
}

#[tauri::command]
pub async fn get_remote_config(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<RemoteConfigPayload>, AppCommandError> {
    get_remote_config_inner(&state).await
}

/// 写 remote 配置 + 触发 tunnel 实时重连(design §2.4)。
///
/// P2-2 校验:scheme 必须 `wss://`(本地调试允许 `ws://`)、去尾斜杠、
/// 拒绝 query/fragment;失败返 `InvalidRequest`,**不写库**。
/// `remote_url` 为空串 = 停用 tunnel(回到纯本地,design §4.1)。
pub async fn set_remote_config_inner(
    state: &Arc<AppState>,
    remote_url: String,
    shared_secret: String,
) -> Result<(), AppCommandError> {
    use crate::daemon::tunnel::config::{build_tunnel_config, normalize_remote_url};
    use crate::daemon::tunnel::TunnelConfig;

    let normalized = if remote_url.trim().is_empty() {
        String::new()
    } else {
        normalize_remote_url(&remote_url)
            .map_err(|msg| AppCommandError::new(ErrorCategory::InvalidRequest, msg))?
    };
    db::set_config_value(
        &state.db,
        crate::daemon::tunnel::config::KEY_REMOTE_URL,
        &normalized,
    )
    .await
    .map_err(|e| anyhow::anyhow!("set_remote_config failed: {}", e))?;
    db::set_config_value(
        &state.db,
        crate::daemon::tunnel::config::KEY_SHARED_SECRET,
        &shared_secret,
    )
    .await
    .map_err(|e| anyhow::anyhow!("set_remote_config failed: {}", e))?;

    let cfg: Option<TunnelConfig> = if normalized.is_empty() {
        None
    } else {
        Some(build_tunnel_config(&state.db, normalized, shared_secret).await)
    };
    state.tunnel_manager.set_config(cfg);
    Ok(())
}

#[tauri::command]
pub async fn set_remote_config(
    state: State<'_, Arc<AppState>>,
    remote_url: String,
    shared_secret: String,
) -> Result<(), AppCommandError> {
    set_remote_config_inner(&state, remote_url, shared_secret).await
}

/// tunnel 状态查询。未配置 remote → `None`;已配置 → 状态快照。
pub async fn get_tunnel_status_inner(
    state: &Arc<AppState>,
) -> Result<Option<TunnelStatusPayload>, AppCommandError> {
    if state.tunnel_manager.current_config().is_none() {
        return Ok(None);
    }
    let status = state.tunnel_manager.status();
    Ok(Some(TunnelStatusPayload {
        connected: status.connected,
        remote_url: status.remote_url.unwrap_or_default(),
        node_id: status.node_id.unwrap_or_default(),
        last_error: status.last_error,
    }))
}

#[tauri::command]
pub async fn get_tunnel_status(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<TunnelStatusPayload>, AppCommandError> {
    get_tunnel_status_inner(&state).await
}

// ---------------------------------------------------------------------------
// F4 web_search 配置(2026-08-25, task `08-25-web-search-tool` WP2)。
// 双形态三层同 tunnel config 先例:`_inner` 业务 + `#[tauri::command]`
// 包装 + daemon route。业务逻辑(三值校验 / key 三态 AEAD / masked)
// 单源在 `tools::web_search`(set_config_state / get_config_state)。
// ---------------------------------------------------------------------------

/// 读 web_search 配置。明文 key 永不出后端——只有 masked。
pub async fn get_web_search_config_inner(
    state: &Arc<AppState>,
) -> Result<crate::tools::web_search::WebSearchConfigPayload, AppCommandError> {
    let payload = crate::tools::web_search::get_config_state(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("get_web_search_config failed: {}", e))?;
    Ok(payload)
}

#[tauri::command]
pub async fn get_web_search_config(
    state: State<'_, Arc<AppState>>,
) -> Result<crate::tools::web_search::WebSearchConfigPayload, AppCommandError> {
    get_web_search_config_inner(&state).await
}

/// 写 web_search 配置。参数为**扁平标量**(IPC 形状铁律,08-21 实证:
/// 嵌套 struct 参数在 HTTP 模式静默 miss)。`tavily_api_key` 三态:
/// `Some(非空)` 重加密落盘 / `Some("")` 清除(删行)/ `None` 不动。
pub async fn set_web_search_config_inner(
    state: &Arc<AppState>,
    provider: String,
    tavily_api_key: Option<String>,
) -> Result<(), AppCommandError> {
    crate::tools::web_search::set_config_state(&state.db, &provider, tavily_api_key.as_deref())
        .await
        .map_err(|msg| AppCommandError::new(ErrorCategory::InvalidRequest, msg))
}

#[tauri::command]
pub async fn set_web_search_config(
    state: State<'_, Arc<AppState>>,
    provider: String,
    tavily_api_key: Option<String>,
) -> Result<(), AppCommandError> {
    set_web_search_config_inner(&state, provider, tavily_api_key).await
}
