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

/// `get_remote_config` 返回体(design §3.1:`{remoteUrl, sharedSecret} | null`;
/// `nodeId` / `displayName` 为附加字段:自定义值原文,`None` = 未设置/自动)。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfigPayload {
    pub remote_url: String,
    pub shared_secret: String,
    pub node_id: Option<String>,
    pub display_name: Option<String>,
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
    // 自定义 node_id / display_name 原文回显(空串 / 全空白归一成
    // None = 自动派生 / 默认 hostname)。
    let normalize = |v: Option<String>| {
        v.and_then(|s| {
            let t = s.trim().to_string();
            (!t.is_empty()).then_some(t)
        })
    };
    let node_id = normalize(
        db::get_config_value(&state.db, crate::daemon::tunnel::config::KEY_TUNNEL_NODE_ID)
            .await
            .map_err(|e| anyhow::anyhow!("get_remote_config failed: {}", e))?,
    );
    let display_name = normalize(
        db::get_config_value(
            &state.db,
            crate::daemon::tunnel::config::KEY_TUNNEL_DISPLAY_NAME,
        )
        .await
        .map_err(|e| anyhow::anyhow!("get_remote_config failed: {}", e))?,
    );
    Ok(Some(RemoteConfigPayload {
        remote_url,
        shared_secret,
        node_id,
        display_name,
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

/// 写自定义 node_id + 按 DB 现状刷新 tunnel 配置(`TunnelConfig` 变化经
/// supervisor `cfg != current` 自动重启隧道,不另加机制)。
///
/// `node_id` 三态(照 `set_web_search_config` 的 `tavily_api_key` 先例):
/// - `Some(非空)` → trim 后须满足 `sanitize(x) == x`(小写字母/数字/连字
///   符,连字符不连续、不在首尾),失败返 `InvalidRequest` **不写库**;
/// - `Some("")` → 删 key(回到自动派生:hostname → fallback UUID);
/// - `None` → 不动 key。
///
/// 两台 hostname 相同的机器各设一个自定义 id 即可在 remote 侧消歧
/// (同 node_id 会互踢,任务 `08-26-custom-node-id`)。
pub async fn set_tunnel_node_id_inner(
    state: &Arc<AppState>,
    node_id: Option<String>,
) -> Result<(), AppCommandError> {
    use crate::daemon::tunnel::config::{
        build_tunnel_config, KEY_REMOTE_URL, KEY_SHARED_SECRET, KEY_TUNNEL_NODE_ID,
    };
    use crate::daemon::tunnel::node_id::sanitize;

    match node_id {
        // 空串 = 显式清除动作(删行,照 web_search 清 key 先例)。
        Some(v) if v.is_empty() => {
            db::delete_config_value(&state.db, KEY_TUNNEL_NODE_ID)
                .await
                .map_err(|e| anyhow::anyhow!("set_tunnel_node_id failed: {}", e))?;
        }
        Some(v) => {
            // 非空串:trim 后须非空且 sanitize 幂等(空白串视同非法值拒绝,
            // 与 Some("") 的清除语义区分)。
            let trimmed = v.trim();
            if trimmed.is_empty() || sanitize(trimmed) != trimmed {
                return Err(AppCommandError::new(
                    ErrorCategory::InvalidRequest,
                    "node_id 不能为空白,且只能包含小写字母、数字和连字符,连字符不能连续或位于首尾"
                        .to_string(),
                ));
            }
            db::set_config_value(&state.db, KEY_TUNNEL_NODE_ID, trimmed)
                .await
                .map_err(|e| anyhow::anyhow!("set_tunnel_node_id failed: {}", e))?;
        }
        None => { /* 不动 */ }
    }

    // 从 DB 重建 tunnel 配置(url 为空 → 停用;非空 → 新 node_id 经
    // derive_node_id ① 臂生效)。
    let remote_url = db::get_config_value(&state.db, KEY_REMOTE_URL)
        .await
        .map_err(|e| anyhow::anyhow!("set_tunnel_node_id failed: {}", e))?
        .unwrap_or_default();
    let cfg = if remote_url.trim().is_empty() {
        None
    } else {
        let shared_secret = db::get_config_value(&state.db, KEY_SHARED_SECRET)
            .await
            .map_err(|e| anyhow::anyhow!("set_tunnel_node_id failed: {}", e))?
            .unwrap_or_default();
        Some(build_tunnel_config(&state.db, remote_url.trim().to_string(), shared_secret).await)
    };
    state.tunnel_manager.set_config(cfg);
    Ok(())
}

#[tauri::command]
pub async fn set_tunnel_node_id(
    state: State<'_, Arc<AppState>>,
    node_id: Option<String>,
) -> Result<(), AppCommandError> {
    set_tunnel_node_id_inner(&state, node_id).await
}

/// 写自定义 display_name + 按 DB 现状刷新 tunnel 配置(镜像
/// [`set_tunnel_node_id_inner`] 的三态与重连路径)。
///
/// 显示名给人看(手机/远程节点列表),**无字符集限制**(允许中文,传输
/// 已有 percent-encode);仅约束 trim 后非空且 ≤ 64 字符。
/// - `Some(非空)` → 校验通过落 `tunnel_display_name`,重连后 remote 侧
///   经 upsert 刷新 nodes.display_name;
/// - `Some("")` → 删 key(回默认 hostname → node_id,读取逻辑在
///   `build_tunnel_config` 不动);
/// - `None` → 不动 key。
///
/// 非法(trim 空 / 超长)返 `InvalidRequest` **不写库**(08-26 增补需求 6)。
pub async fn set_tunnel_display_name_inner(
    state: &Arc<AppState>,
    display_name: Option<String>,
) -> Result<(), AppCommandError> {
    use crate::daemon::tunnel::config::{
        build_tunnel_config, KEY_REMOTE_URL, KEY_SHARED_SECRET, KEY_TUNNEL_DISPLAY_NAME,
    };

    match display_name {
        // 空串 = 显式清除动作(删行,同 node_id 清除写法)。
        Some(v) if v.is_empty() => {
            db::delete_config_value(&state.db, KEY_TUNNEL_DISPLAY_NAME)
                .await
                .map_err(|e| anyhow::anyhow!("set_tunnel_display_name failed: {}", e))?;
        }
        Some(v) => {
            // 非空串:trim 后须非空且 ≤ 64 字符(空白串视同非法值拒绝,
            // 与 Some("") 的清除语义区分)。长度按字符数计(中文不是字节)。
            let trimmed = v.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 64 {
                return Err(AppCommandError::new(
                    ErrorCategory::InvalidRequest,
                    "显示名不能为空白,且长度不能超过 64 个字符".to_string(),
                ));
            }
            db::set_config_value(&state.db, KEY_TUNNEL_DISPLAY_NAME, trimmed)
                .await
                .map_err(|e| anyhow::anyhow!("set_tunnel_display_name failed: {}", e))?;
        }
        None => { /* 不动 */ }
    }

    // 从 DB 重建 tunnel 配置(url 为空 → 停用;非空 → 新 display_name 经
    // build_tunnel_config 的 key 读取生效)。
    let remote_url = db::get_config_value(&state.db, KEY_REMOTE_URL)
        .await
        .map_err(|e| anyhow::anyhow!("set_tunnel_display_name failed: {}", e))?
        .unwrap_or_default();
    let cfg = if remote_url.trim().is_empty() {
        None
    } else {
        let shared_secret = db::get_config_value(&state.db, KEY_SHARED_SECRET)
            .await
            .map_err(|e| anyhow::anyhow!("set_tunnel_display_name failed: {}", e))?
            .unwrap_or_default();
        Some(build_tunnel_config(&state.db, remote_url.trim().to_string(), shared_secret).await)
    };
    state.tunnel_manager.set_config(cfg);
    Ok(())
}

#[tauri::command]
pub async fn set_tunnel_display_name(
    state: State<'_, Arc<AppState>>,
    display_name: Option<String>,
) -> Result<(), AppCommandError> {
    set_tunnel_display_name_inner(&state, display_name).await
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

// ---------------------------------------------------------------------------
// F6 异步 agent 任务(2026-08-27):前端可读的 app_config 开关面。
// ---------------------------------------------------------------------------

/// `get_app_config` 响应(wire: camelCase,同 `WebSearchConfigPayload`
/// 先例)。当前只暴露前端需要消费的开关;后续新开关在此 struct 加字段
/// (additive)即可,不再为新标志位单开命令。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigPayload {
    /// 跨 session 轮次完成 toast 的总开关。app_config 键
    /// `turn_complete_notify_enabled`,fail-open 缺省开——读法对齐
    /// `tools_stub_enabled`(`chat_loop.rs` 同款:仅字面 `"false"` 关)。
    pub turn_complete_notify_enabled: bool,
    /// F2 定时任务(2026-08-28, task `08-28-f2-scheduled-tasks`)全局
    /// kill switch 的读出口。app_config 键 `scheduled_tasks_enabled`,
    /// fail-open 缺省开(仅字面 `"false"` 关,读法同调度循环
    /// `scheduler/mod.rs` 的每 tick 读取)。additive 字段:供前端
    /// 「定时任务」面板展示当前开关状态(design §5)。
    pub scheduled_tasks_enabled: bool,
}

pub async fn get_app_config_inner(
    state: &Arc<AppState>,
) -> Result<AppConfigPayload, AppCommandError> {
    let on = match crate::db::config::get_config_value(&state.db, "turn_complete_notify_enabled")
        .await
    {
        Ok(Some(v)) => v != "false",
        _ => true,
    };
    // F2 kill switch:与调度循环同读法(fail-open)。
    let scheduled_on =
        match crate::db::config::get_config_value(&state.db, "scheduled_tasks_enabled").await {
            Ok(Some(v)) => v != "false",
            _ => true,
        };
    Ok(AppConfigPayload {
        turn_complete_notify_enabled: on,
        scheduled_tasks_enabled: scheduled_on,
    })
}

#[tauri::command]
pub async fn get_app_config(
    state: State<'_, Arc<AppState>>,
) -> Result<AppConfigPayload, AppCommandError> {
    get_app_config_inner(&state).await
}

// ---------------------------------------------------------------------------
// Settings「通用」开关写入口(2026-08-29, settings-shell 重构)。
// 与 `get_app_config` 组成读写对;存值语义不变(仅字面 `"false"` 关,
// fail-open 缺省开),写入即 `"true"` / `"false"` 字面量。
// ---------------------------------------------------------------------------

/// `set_app_config_flag` 允许写的 key 白名单。防呆:app_config 里还有
/// remote_url / web_search key 等敏感键(各有自己的校验命令),不能被
/// 这个通用布尔入口绕过;新增可 UI 切换的标志位时在此扩名单 +
/// `AppConfigPayload` 加读字段。
const SETTABLE_APP_FLAGS: &[&str] = &["turn_complete_notify_enabled", "scheduled_tasks_enabled"];

/// 写 app_config 布尔开关(白名单内)。key 不在白名单 → `InvalidRequest`
/// 不写库。参数为**扁平标量**(IPC 形状铁律)。
pub async fn set_app_config_flag_inner(
    state: &Arc<AppState>,
    key: String,
    value: bool,
) -> Result<(), AppCommandError> {
    if !SETTABLE_APP_FLAGS.contains(&key.as_str()) {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("unknown app_config flag: {key}"),
        ));
    }
    db::set_config_value(&state.db, &key, if value { "true" } else { "false" })
        .await
        .map_err(|e| anyhow::anyhow!("set_app_config_flag failed: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn set_app_config_flag(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: bool,
) -> Result<(), AppCommandError> {
    set_app_config_flag_inner(&state, key, value).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白名单内 key:写 `"false"` → `get_app_config_inner` 读回 false;
    /// 写 `"true"` 回 true。与读路径(fail-open 仅字面 `"false"` 关)闭环。
    #[tokio::test(flavor = "multi_thread")]
    async fn set_flag_roundtrip_through_get_app_config() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);

        // 缺省(未写)双开关为 true
        let cfg = get_app_config_inner(&state).await.unwrap();
        assert!(cfg.turn_complete_notify_enabled && cfg.scheduled_tasks_enabled);

        for key in SETTABLE_APP_FLAGS {
            set_app_config_flag_inner(&state, key.to_string(), false)
                .await
                .unwrap();
            let cfg = get_app_config_inner(&state).await.unwrap();
            let off = match *key {
                "turn_complete_notify_enabled" => cfg.turn_complete_notify_enabled,
                "scheduled_tasks_enabled" => cfg.scheduled_tasks_enabled,
                _ => unreachable!(),
            };
            assert!(!off, "{key} 写 false 后应读回 false");

            set_app_config_flag_inner(&state, key.to_string(), true)
                .await
                .unwrap();
        }
        let cfg = get_app_config_inner(&state).await.unwrap();
        assert!(cfg.turn_complete_notify_enabled && cfg.scheduled_tasks_enabled);
    }

    /// 白名单外 key 拒绝(`InvalidRequest`),且不落库。
    #[tokio::test(flavor = "multi_thread")]
    async fn set_flag_rejects_non_whitelisted_key() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);

        // default_model_id 建库即被 seed,不能当"未写入"断言对象;
        // 这几个键在新库均为空。
        for bad in [
            "remote_url",
            "shared_secret",
            "web_search.provider",
            "future_flag_x",
        ] {
            let err = set_app_config_flag_inner(&state, bad.to_string(), true)
                .await
                .unwrap_err();
            assert_eq!(err.category, ErrorCategory::InvalidRequest, "{bad}");
            assert!(
                db::get_config_value(&state.db, bad)
                    .await
                    .unwrap()
                    .is_none(),
                "拒绝的 key 不得写库: {bad}"
            );
        }
    }
}
