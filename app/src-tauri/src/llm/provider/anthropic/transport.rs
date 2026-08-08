//! Anthropic HTTP 传输层(拆分自 anthropic.rs,08-08-a-class-anthropic-split)。
//!
//! `request_log_fields`(observability 字段)+ `send_request`(client 构建 +
//! 请求日志 + HTTP 发送 + 非 2xx 检查),由 `chat_stream_with_tools` 宏体调用。

#![allow(unused_imports)]
use std::time::Duration;

use crate::llm::error::{classify_error_response, LlmError};
use crate::llm::types::TokenUsage;

use super::LlmConfig;

pub(crate) fn request_log_fields(body: &serde_json::Value) -> (String, usize, bool) {
    // Pull the same observability fields the pre-fix code read off
    // `&req` — `model`, `tools_count`, `has_system` — off the JSON
    // body that the DeepSeek relay fix produced. The shape is the
    // same; the values come from the same wire payload, so log
    // content is byte-equivalent to the pre-fix logs.
    let log_model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let log_tools_count = body
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let log_has_system = body.get("system").map(|v| !v.is_null()).unwrap_or(false);
    (log_model, log_tools_count, log_has_system)
}

/// 构建 HTTP client + 发送请求 + 非 2xx 状态检查(提取自 chat_stream_with_tools
/// 阶段 A-D,08-08-a-class-anthropic-split)。RULE-A-011 注释随代码平移;
/// 错误统一返回 `Err`,宏体 yield 点收敛为 `match { Err(e) => { yield Err(e); return; } }`。
pub(crate) async fn send_request(
    config: &LlmConfig,
    url: &str,
    body: &serde_json::Value,
) -> Result<reqwest::Response, LlmError> {
    // RULE-A-011 (2026-06-19): use `read_timeout` instead of
    // `timeout` for SSE streaming. Per reqwest docs
    // (`async_impl/client.rs:1448-1459`), `.timeout()` is a
    // **total deadline** from connect to body EOF — wrong
    // for SSE where the body is unbounded and chunk rate
    // varies (extended thinking on a 3rd-party proxy can be
    // 60s+ before the first text delta). `.read_timeout()`
    // is per-read, resets on each chunk — the right tool
    // for "stalled connection when size isn't known". The
    // 60s value stays as the upper bound on silence between
    // chunks; a truly dead proxy will surface this quickly.
    // See `.trellis/spec/backend/error-handling.md` §RULE-A-011
    // and incident `mz8s3hqwx6rmqjswgte` / messages.seq=37.
    let client = match reqwest::Client::builder()
        .read_timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return Err(LlmError::Network(format!("client build: {}", e))),
    };

    let (log_model, log_tools_count, log_has_system) = request_log_fields(body);
    tracing::info!(
        url = %url,
        model = %log_model,
        tools_count = %log_tools_count,
        has_system = %log_has_system,
        "→ LLM request"
    );

    let resp = match client
        .post(url)
        .header("x-api-key", &config.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "network error before response");
            return Err(LlmError::Network(e.to_string()));
        }
    };

    let status = resp.status();
    if !status.is_success() {
        // Snapshot headers before `resp.text()` consumes the response —
        // `retry_after` advisory parsing needs them (A5+ retry support).
        let headers = resp.headers().clone();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(status = %status, body = %body, "← LLM error");
        return Err(classify_error_response(
            status.as_u16(),
            &body,
            Some(&headers),
        ));
    }
    Ok(resp)
}
