//! 稳定 node_id 派生(design §3.3)。
//!
//! remote 侧 `tunnel_registry` 以 node_id 为键(重复 node_id → 新连接踢
//! 旧),所以 node_id 必须**跨重启稳定**。策略(三级优先,前者命中即返回):
//!
//! 1. `app_config "tunnel_node_id"` 有值 → 直接用。**key 有值即优先**,
//!    统一覆盖用户自定义(`set_tunnel_node_id` 写入)与历史 fallback UUID
//!    —— hostname 改名不漂移,两台同 hostname 机器可各自定制消歧。
//! 2. hostname → 只留 `[a-z0-9-]`(小写,其他字符折叠为 `-`);空结果
//!    (纯中文/纯特殊字符 hostname)落到 fallback。
//! 3. fallback:随机 UUID 持久化到同一 key `tunnel_node_id`(首次生成
//!    存库,之后读取)。DB 文件即身份,不随 hostname 变更漂移。
//!
//! display_name 不走这里(它是给人看的,允许中文,存
//! `app_config "tunnel_display_name"`,见 [`super::config`])。

use sqlx::SqlitePool;

use super::config::{resolve_persistent_fallback, KEY_TUNNEL_NODE_ID};
use crate::db;

/// 派生稳定 node_id(design §3.3)。同一台机器重启不变。
pub async fn derive_node_id(pool: &SqlitePool) -> String {
    // ① key 有值(用户自定义或历史 fallback UUID)→ 直接用,hostname 不参与。
    if let Ok(Some(custom)) = db::get_config_value(pool, KEY_TUNNEL_NODE_ID).await {
        let custom = custom.trim().to_string();
        if !custom.is_empty() {
            return custom;
        }
    }
    // ② hostname 净化派生。
    if let Some(host) = hostname::get().ok().and_then(|h| h.into_string().ok()) {
        let sanitized = sanitize(&host);
        if !sanitized.is_empty() {
            return sanitized;
        }
    }
    // ③ hostname 不可用 / 净化后为空 → 持久化 UUID fallback(写同一 key)。
    resolve_persistent_fallback(pool, KEY_TUNNEL_NODE_ID).await
}

/// 只留 `[a-z0-9-]`,小写,连续/首尾的非法字符折叠成单个 `-` 后裁掉。
/// 例:`"My PC (办公)"` → `"my-pc"`;`"公司PC"` → `""`(调用方走 fallback)。
pub fn sanitize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            // 非法字符折叠为单个 '-'。
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// 默认 display_name = hostname(不净化 —— 给人看的,允许中文;传输时
/// percent-encode 即可,P2-1)。hostname 拿不到时退回 node_id。
pub fn default_display_name(node_id: &str) -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| node_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::{derive_node_id, sanitize};
    use crate::db;

    /// 每测试独立 in-memory 池(app_config 写入不能共享池)。
    async fn make_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    #[test]
    fn sanitize_keeps_ascii_lowercase_dash() {
        assert_eq!(sanitize("company-pc"), "company-pc");
        assert_eq!(sanitize("My-PC"), "my-pc");
    }

    #[test]
    fn sanitize_collapses_invalid_chars() {
        assert_eq!(sanitize("My PC (办公)"), "my-pc");
        // 纯中文 hostname → 空(调用方走持久化 UUID fallback)
        assert_eq!(sanitize("中文主机"), "");
        // 前导非法字符丢弃,后续字母数字保留
        assert_eq!(sanitize("公司PC"), "pc");
        assert_eq!(sanitize("  --a--  "), "a");
        assert_eq!(sanitize("__"), "");
    }

    /// key 有值优先于 hostname:自定义(如 `carlos-office`)和历史 fallback
    /// UUID(`node-…`)统一走这条臂 —— 同 hostname 两台机器靠它消歧。
    #[tokio::test]
    async fn key_value_wins_over_hostname() {
        let pool = make_pool().await;
        for preset in ["carlos-office", "node-a1b2c3d4e5f6"] {
            db::set_config_value(&pool, super::KEY_TUNNEL_NODE_ID, preset)
                .await
                .unwrap();
            assert_eq!(derive_node_id(&pool).await, preset);
        }
    }

    /// key 为空(空串 / 全空白)→ hostname 派生,且不写 key。
    #[tokio::test]
    async fn empty_key_falls_back_to_hostname() {
        let pool = make_pool().await;
        db::set_config_value(&pool, super::KEY_TUNNEL_NODE_ID, "   ")
            .await
            .unwrap();
        let hostname_sanitized = sanitize(
            &hostname::get()
                .unwrap()
                .into_string()
                .expect("test env has hostname"),
        );
        assert!(
            !hostname_sanitized.is_empty(),
            "CI hostname 净化后非空(否则本测试的前提不成立)"
        );
        let derived = derive_node_id(&pool).await;
        assert_eq!(derived, hostname_sanitized);
        // hostname 派生不落 key(未触发 fallback)
        let stored = db::get_config_value(&pool, super::KEY_TUNNEL_NODE_ID)
            .await
            .unwrap();
        assert_eq!(stored.as_deref(), Some("   "));
    }

    /// key 缺失 + hostname 可用 → hostname 派生(存量默认行为回归)。
    #[tokio::test]
    async fn missing_key_uses_hostname() {
        let pool = make_pool().await;
        let hostname_sanitized = sanitize(
            &hostname::get()
                .unwrap()
                .into_string()
                .expect("test env has hostname"),
        );
        assert!(!hostname_sanitized.is_empty(), "CI hostname 净化后非空");
        assert_eq!(derive_node_id(&pool).await, hostname_sanitized);
    }

    /// fallback:hostname 不可用测试环境难模拟,改为直接锁
    /// `resolve_persistent_fallback` 的持久化语义 —— 首次生成 `node-<uuid>`
    /// 写 key,二次调用读回同值(现行为回归)。
    #[tokio::test]
    async fn persistent_fallback_is_stable_across_calls() {
        let pool = make_pool().await;
        let first =
            super::super::config::resolve_persistent_fallback(&pool, super::KEY_TUNNEL_NODE_ID)
                .await;
        assert!(
            first.starts_with("node-"),
            "fallback 形如 node-<uuid>: {first}"
        );
        let stored = db::get_config_value(&pool, super::KEY_TUNNEL_NODE_ID)
            .await
            .unwrap();
        assert_eq!(stored.as_deref(), Some(first.as_str()));
        // key 已有值后,derive 优先读回同值(① 臂覆盖 ③ 的写入)
        assert_eq!(derive_node_id(&pool).await, first);
    }
}
