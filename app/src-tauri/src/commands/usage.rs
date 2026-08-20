//! 08-20-turn-usage-event-quota-view WP2 — 5h 滚动窗口配额 IPC。
//!
//! - [`usage_window`] — AppHeader QuotaChip/弹层的数据源(per-provider
//!   窗口聚合 + top sessions,聚合层见 `db::usage`)。
//! - [`set_quota_settings`] — config 表 `quota_window_hours` /
//!   `quota_limit_tokens` 写入(模板 `set_remote_config_inner`)。

use std::sync::Arc;

use tauri::State;

use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

pub const KEY_QUOTA_WINDOW_HOURS: &str = "quota_window_hours";
pub const KEY_QUOTA_LIMIT_TOKENS: &str = "quota_limit_tokens";
/// 套餐 5h 滚动窗口(缺省;`quota_window_hours` 可覆盖)。
pub const DEFAULT_QUOTA_WINDOW_HOURS: i64 = 5;
/// top sessions 返回条数(弹层列表,不是分页面)。
const TOP_SESSION_LIMIT: u32 = 10;

fn parse_window_hours(raw: Option<String>) -> i64 {
    raw.and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|h| (1..=168).contains(h))
        .unwrap_or(DEFAULT_QUOTA_WINDOW_HOURS)
}

fn parse_limit_tokens(raw: Option<String>) -> Option<i64> {
    raw.and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
}

/// Phase 2.2 `_inner`:共享业务逻辑,Tauri command 与 axum route 双入口。
/// `provider_filter = None` = 全部 provider。窗口/额度读 config(缺省
/// 5h / 未设额度),聚合本体见 `db::usage::usage_window`。
pub async fn usage_window_inner(
    state: &Arc<AppState>,
    provider_filter: Option<String>,
) -> Result<db::usage::UsageWindowReport, AppCommandError> {
    let window_raw = db::get_config_value(&state.db, KEY_QUOTA_WINDOW_HOURS)
        .await
        .map_err(|e| anyhow::anyhow!("read quota_window_hours failed: {}", e))?;
    let limit_raw = db::get_config_value(&state.db, KEY_QUOTA_LIMIT_TOKENS)
        .await
        .map_err(|e| anyhow::anyhow!("read quota_limit_tokens failed: {}", e))?;
    let report = db::usage::usage_window(
        &state.db,
        provider_filter.as_deref(),
        parse_window_hours(window_raw),
        parse_limit_tokens(limit_raw),
        TOP_SESSION_LIMIT,
    )
    .await
    .map_err(|e| anyhow::anyhow!("usage_window aggregation failed: {}", e))?;
    Ok(report)
}

/// Phase 2.2 `_inner`。写两把 config 键;`limit_tokens = None` 显式删除
/// 键(区别于"保持不变"—— 前端清空输入框即走删除)。
///
/// 扁平参数(非嵌套 request struct):httpTransport 把顶层参数
/// camel→snake 后作为 POST body,daemon 路由结构体是 snake_case ——
/// 嵌套结构会在 HTTP 模式下整体 miss 字段静默重置(质检 Step 5 发现,
/// house 模式见 `set_remote_config`)。
pub async fn set_quota_settings_inner(
    state: &Arc<AppState>,
    window_hours: Option<i64>,
    limit_tokens: Option<i64>,
) -> Result<(), AppCommandError> {
    let hours = window_hours
        .map(|h| h.clamp(1, 168))
        .unwrap_or(DEFAULT_QUOTA_WINDOW_HOURS);
    db::set_config_value(&state.db, KEY_QUOTA_WINDOW_HOURS, &hours.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("set quota_window_hours failed: {}", e))?;
    match limit_tokens.filter(|v| *v > 0) {
        Some(v) => {
            db::set_config_value(&state.db, KEY_QUOTA_LIMIT_TOKENS, &v.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("set quota_limit_tokens failed: {}", e))?;
        }
        None => {
            // 删除键 = 回到"只显示消耗"(AC5 未设语义)。
            db::delete_config_value(&state.db, KEY_QUOTA_LIMIT_TOKENS)
                .await
                .map_err(|e| anyhow::anyhow!("clear quota_limit_tokens failed: {}", e))?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn usage_window(
    state: State<'_, Arc<AppState>>,
    provider_id: Option<String>,
) -> Result<db::usage::UsageWindowReport, AppCommandError> {
    usage_window_inner(&state, provider_id).await
}

#[tauri::command]
pub async fn set_quota_settings(
    state: State<'_, Arc<AppState>>,
    window_hours: Option<i64>,
    limit_tokens: Option<i64>,
) -> Result<(), AppCommandError> {
    set_quota_settings_inner(&state, window_hours, limit_tokens).await
}
