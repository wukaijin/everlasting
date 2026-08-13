//! 稳定 node_id 派生(design §3.3)。
//!
//! remote 侧 `tunnel_registry` 以 node_id 为键(重复 node_id → 新连接踢
//! 旧),所以 node_id 必须**跨重启稳定**。策略:
//!
//! 1. 优先 hostname → 只留 `[a-z0-9-]`(小写,其他字符折叠为 `-`);
//!    空结果(纯中文/纯特殊字符 hostname)落到 fallback。
//! 2. fallback:随机 UUID 持久化到 `app_config "tunnel_node_id"`(首次
//!    生成存库,之后读取)。DB 文件即身份,不随 hostname 变更漂移。
//!
//! display_name 不走这里(它是给人看的,允许中文,存
//! `app_config "tunnel_display_name"`,见 [`super::config`])。

use sqlx::SqlitePool;

use super::config::{resolve_persistent_fallback, KEY_TUNNEL_NODE_ID};

/// 派生稳定 node_id(design §3.3)。同一台机器重启不变。
pub async fn derive_node_id(pool: &SqlitePool) -> String {
    if let Some(host) = hostname::get().ok().and_then(|h| h.into_string().ok()) {
        let sanitized = sanitize(&host);
        if !sanitized.is_empty() {
            return sanitized;
        }
    }
    // hostname 不可用 / 净化后为空 → 持久化 UUID fallback。
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
    use super::sanitize;

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
}
