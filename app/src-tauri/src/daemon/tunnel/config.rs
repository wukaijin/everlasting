//! remote 隧道配置的 `app_config` KV 存取 + P2-2 校验/规范化。
//!
//! **零 migration**(design §3.2):复用现有 `app_config` 表的
//! `get_config_value` / `set_config_value`(`db/config.rs`),加 key 不需
//! 建表。key 常量单源 —— `load_remote_config`(daemon main)、
//! `set_remote_config_inner`(IPC)、`node_id` fallback 都从本模块读,
//! 不各自硬编码。
//!
//! P2-2 修订:`set_remote_config` 阶段校验 scheme(`wss://`,本地调试允许
//! `ws://`)+ 去尾斜杠 + 拒绝 query/fragment,失败返 `InvalidRequest` 不写
//! 库 —— 避免错误配置后台无限重连(design §6.2 失败模式表)。

use sqlx::SqlitePool;
use uuid::Uuid;

use super::node_id::default_display_name;
use super::TunnelConfig;
use crate::db;

/// `app_config` key:remote 基地址(如 `wss://remote.example.com`)。
pub const KEY_REMOTE_URL: &str = "remote_url";
/// `app_config` key:与 remote 共享的 secret。
pub const KEY_SHARED_SECRET: &str = "shared_secret";
/// `app_config` key:持久化 fallback 的稳定 node_id(见 `node_id.rs`)。
pub const KEY_TUNNEL_NODE_ID: &str = "tunnel_node_id";
/// `app_config` key:节点显示名(默认 = hostname,前端 Settings 可改)。
pub const KEY_TUNNEL_DISPLAY_NAME: &str = "tunnel_display_name";

/// 从 `app_config` 读 remote 配置。`remote_url` key 缺失或为空 → `None`
/// (daemon 不 spawn tunnel,本地零回归)。规范化失败(库里是手工写的坏值)
/// → warn + `None`,同样不 spawn(不无限重连)。
pub async fn load_remote_config(pool: &SqlitePool) -> Result<Option<TunnelConfig>, sqlx::Error> {
    let Some(raw_url) = db::get_config_value(pool, KEY_REMOTE_URL).await? else {
        return Ok(None);
    };
    let raw_url = raw_url.trim().to_string();
    if raw_url.is_empty() {
        return Ok(None);
    }
    let remote_url = match normalize_remote_url(&raw_url) {
        Ok(url) => url,
        Err(msg) => {
            tracing::warn!(
                target: super::TUNNEL_TARGET,
                raw_url = %raw_url,
                "remote_url 配置无效,跳过 tunnel 启动: {msg}"
            );
            return Ok(None);
        }
    };
    let shared_secret = db::get_config_value(pool, KEY_SHARED_SECRET)
        .await?
        .unwrap_or_default();
    Ok(Some(
        build_tunnel_config(pool, remote_url, shared_secret).await,
    ))
}

/// 由 (remote_url, shared_secret) 拼出完整 [`TunnelConfig`](crate::daemon::tunnel::TunnelConfig):
/// node_id 派生(可能写持久化 fallback)+ display_name(默认 hostname)。
/// `load_remote_config` 与 `set_remote_config_inner` 共用,派生逻辑单源。
pub async fn build_tunnel_config(
    pool: &SqlitePool,
    remote_url: String,
    shared_secret: String,
) -> TunnelConfig {
    let node_id = super::node_id::derive_node_id(pool).await;
    let display_name = db::get_config_value(pool, KEY_TUNNEL_DISPLAY_NAME)
        .await
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_display_name(&node_id));
    TunnelConfig {
        remote_url,
        shared_secret,
        node_id,
        display_name,
    }
}

/// P2-2 校验 + 规范化:
/// - trim + 去尾斜杠
/// - scheme 必须 `wss://`(本地调试允许 `ws://`)
/// - 不允许 query / fragment / **路径**(基地址契约 —— 连接 URL 由客户端
///   拼 `{remote_url}/ws?…`;填 `wss://host/ws` 会拼出 `/ws/ws` 断链 +
///   后台无限重连,review P2-2「带路径参数」)
/// - scheme 后必须有 host
///
/// 失败返回人类可读错误(IPC 层转 `InvalidRequest` 给前端 inline 提示)。
pub fn normalize_remote_url(raw: &str) -> Result<String, String> {
    let s = raw.trim().trim_end_matches('/').to_string();
    if s.is_empty() {
        return Err("remote_url 不能为空".to_string());
    }
    let rest = s.strip_prefix("wss://").or_else(|| s.strip_prefix("ws://"));
    let Some(rest) = rest else {
        return Err("remote_url 必须以 wss:// 开头(本地调试可用 ws://)".to_string());
    };
    if rest.is_empty() {
        return Err("remote_url 缺少主机名".to_string());
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("remote_url 不能包含查询参数或 fragment(它是基地址)".to_string());
    }
    if rest.contains('/') {
        return Err(
            "remote_url 是基地址,不能包含路径(连接 URL 会自动拼 /ws,不要填 wss://host/ws)"
                .to_string(),
        );
    }
    Ok(s)
}

/// 持久化 UUID fallback 的读写(`tunnel_node_id` 用)。读已有值;没有则
/// 生成 `node-<uuid>` 写入并返回。写失败只 warn(内存值仍可用,仅重启
/// 后漂移)。
pub async fn resolve_persistent_fallback(pool: &SqlitePool, key: &str) -> String {
    if let Ok(Some(existing)) = db::get_config_value(pool, key).await {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            return existing;
        }
    }
    let id = format!("node-{}", Uuid::new_v4().simple());
    if let Err(e) = db::set_config_value(pool, key, &id).await {
        tracing::warn!(target: super::TUNNEL_TARGET, key, error = %e, "persist node_id fallback failed");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::normalize_remote_url;

    #[test]
    fn normalizes_valid_urls() {
        assert_eq!(
            normalize_remote_url("wss://remote.example.com").unwrap(),
            "wss://remote.example.com"
        );
        // 去尾斜杠
        assert_eq!(
            normalize_remote_url("wss://remote.example.com///").unwrap(),
            "wss://remote.example.com"
        );
        // 本地调试允许 ws://
        assert_eq!(
            normalize_remote_url("ws://localhost:7457/").unwrap(),
            "ws://localhost:7457"
        );
    }

    #[test]
    fn rejects_invalid_urls() {
        assert!(normalize_remote_url("").is_err());
        assert!(normalize_remote_url("   ").is_err());
        assert!(normalize_remote_url("https://remote.example.com").is_err());
        assert!(normalize_remote_url("remote.example.com").is_err());
        assert!(normalize_remote_url("wss://").is_err());
        assert!(normalize_remote_url("wss://remote.example.com?x=1").is_err());
        assert!(normalize_remote_url("wss://remote.example.com/#frag").is_err());
    }

    /// 基地址契约(design §2.1):连接 URL 由客户端拼 `/ws`。填了路径
    /// (用户容易把完整连接 URL `wss://host/ws` 直接贴进来)会拼出
    /// `/ws/ws` 断链 + 无限重连 —— 必须在配置阶段拒绝(review P2-2)。
    #[test]
    fn rejects_urls_with_path() {
        assert!(normalize_remote_url("wss://remote.example.com/ws").is_err());
        assert!(normalize_remote_url("wss://remote.example.com/ws/").is_err());
        assert!(normalize_remote_url("wss://remote.example.com/everlasting").is_err());
        assert!(normalize_remote_url("ws://localhost:7457/ws").is_err());
    }
}
