//! Tavily 后端(keyed 主力)。契约锁定自官方 API 文档(2026-08-25
//! 调研,`research/search-backend-options.md` §3):
//!
//! - `POST {base}/search`,头 `Authorization: Bearer tvly-xxx`,JSON
//!   body:`query` / `max_results` 0-20 / `search_depth`。
//! - **显式 `search_depth: "basic"`**(1 credit)——不传时上游的
//!   `auto_parameters` 可能悄悄升 advanced(2 credits,双倍扣费)。
//! - 200 响应:`{query, results: [{title, url, content, score, ...}]}`,
//!   `content` 即清洗过的正文片段 → `SearchHit.snippet`;`score` /
//!   `raw_content` 忽略。
//! - 错误码:400 参数 / 401 key 无效 / 429 限速 / 432 免费额度尽 /
//!   433 PAYG 限额——渲染层(mod.rs)按 code 出可操作文案。
//!
//! key 解密后构造 client 时注入,不进日志(结构体字段,永不 `tracing`
//! 输出)。base_url / 超时可注入(httpmock 测试注 loopback 地址/短值)。
//! SSRF 面不存在:固定域名固定 endpoint,无用户可控 URL——不引
//! `is_blocked`,DNS 走系统(吃代理 env,与 web_fetch 行为一致)。

use std::time::Duration;

use serde::Deserialize;

use super::{SearchError, SearchHit};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.tavily.com";

const USER_AGENT: &str = concat!("Everlasting/", env!("CARGO_PKG_VERSION"));

pub(crate) struct TavilyClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

/// 手写 Debug:api_key 永不进输出(日志/测试打印安全)。
impl std::fmt::Debug for TavilyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TavilyClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl TavilyClient {
    pub(crate) fn new(api_key: &str, base_url: &str, connect_timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect_timeout)
            // 不设 per-request .timeout():整体预算在 mod.rs 的外层
            // `tokio::time::timeout` 单包(所有重试尝试共用),这里再
            // 设只会让单次尝试吃到整份预算。
            .gzip(true)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    pub(crate) async fn search(
        &self,
        query: &str,
        count: u8,
    ) -> Result<Vec<SearchHit>, SearchError> {
        let body = serde_json::json!({
            "query": query,
            "max_results": count,
            // 显式 basic:防 auto_parameters 悄悄升 advanced 烧 2 credit。
            "search_depth": "basic",
        });
        let resp = self
            .http
            .post(format!("{}/search", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(classify_reqwest_error)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(SearchError::HttpStatus(status.as_u16()));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;
        parse_response(&text)
    }
}

/// 200 响应解析。`results` 字段**必须存在**(无 serde default)——
/// 形状漂移(如错误对象)直接落 `Parse`,不静默当零结果。单条 result
/// 的字段宽松默认(缺 title 的条目保留,缺 url 的条目丢弃——无 URL
/// 的 hit 对模型不可追访)。
fn parse_response(text: &str) -> Result<Vec<SearchHit>, SearchError> {
    #[derive(Deserialize)]
    struct TavilyResponse {
        results: Vec<TavilyResult>,
    }
    #[derive(Deserialize)]
    struct TavilyResult {
        #[serde(default)]
        title: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        content: String,
    }
    let parsed: TavilyResponse = serde_json::from_str(text).map_err(|e| {
        SearchError::Parse(format!(
            "json decode failed ({e}); body: {}",
            truncate_dbg(text, 120)
        ))
    })?;
    Ok(parsed
        .results
        .into_iter()
        .filter(|r| !r.url.is_empty())
        .map(|r| SearchHit {
            title: r.title,
            url: r.url,
            snippet: r.content,
        })
        .collect())
}

/// 诊断片段:保留原始长度信息(design §8:形状漂移时能看到原文多长)。
fn truncate_dbg(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!(
        "{}… ({} chars total)",
        s.chars().take(max).collect::<String>(),
        s.chars().count()
    )
}

fn classify_reqwest_error(e: reqwest::Error) -> SearchError {
    if e.is_timeout() {
        SearchError::Timeout
    } else {
        SearchError::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp_json(results: serde_json::Value) -> String {
        serde_json::json!({ "query": "q", "results": results }).to_string()
    }

    #[test]
    fn parse_response_maps_fields_and_drops_urlless() {
        let text = resp_json(serde_json::json!([
            {"title": "T1", "url": "https://a", "content": "C1", "score": 0.9},
            {"title": "NoUrl", "content": "junk"},
            {"url": "https://c"}
        ]));
        let hits = parse_response(&text).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            (
                hits[0].title.as_str(),
                hits[0].url.as_str(),
                hits[0].snippet.as_str()
            ),
            ("T1", "https://a", "C1")
        );
        // 缺 title 的条目保留(渲染层统一处理空 title),缺 url 的丢弃。
        assert_eq!(hits[1].url, "https://c");
    }

    #[test]
    fn parse_response_missing_results_field_is_parse_error() {
        // 形状漂移:错误对象没有 results 数组 → Parse,不静默零结果。
        let err = parse_response(r#"{"detail": "not found"}"#).unwrap_err();
        assert!(matches!(err, SearchError::Parse(_)));
    }

    #[test]
    fn parse_response_keeps_length_info_on_drift() {
        let long = "x".repeat(500);
        let err = parse_response(&long).unwrap_err();
        let SearchError::Parse(msg) = err else {
            panic!("expected Parse, got {err:?}");
        };
        assert!(msg.contains("500 chars total"), "{msg}");
    }
}
