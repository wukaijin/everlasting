//! LLM error normalization.
//!
//! Strategy (per HACKING-llm.md "GLM 兼容层 3 处差异"):
//! 1. Never trust HTTP status code alone — `400`-class errors can return `5xx`
//!    from the GLM compatibility layer.
//! 2. Parse the response body, look for `error.type` substring keywords
//!    (`authentication` / `rate_limit` / `invalid_request`) regardless of
//!    wrapper nesting (`body.error.type` → `body.type` → status code).
//! 3. Don't pre-validate `max_tokens` server-side limits.
//!
//! The five variants cover everything the frontend needs to display a useful
//! message and decide whether retrying makes sense.

use super::types::LlmErrorCategory;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("auth failed: {0}")]
    Auth(String),

    #[error("rate limited: {0}")]
    RateLimit(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("server error (HTTP {status}): {message}")]
    Server { status: u16, message: String },

    #[error("network error: {0}")]
    Network(String),
}

impl LlmError {
    pub fn category(&self) -> LlmErrorCategory {
        match self {
            LlmError::Auth(_) => LlmErrorCategory::Auth,
            LlmError::RateLimit(_) => LlmErrorCategory::RateLimit,
            LlmError::InvalidRequest(_) => LlmErrorCategory::InvalidRequest,
            LlmError::Server { .. } => LlmErrorCategory::Server,
            LlmError::Network(_) => LlmErrorCategory::Network,
        }
    }

    /// Short, user-facing message. Suitable for display in the chat UI.
    pub fn user_message(&self) -> String {
        match self {
            LlmError::Auth(_) => "API key 无效或已过期,请检查 ANTHROPIC_API_KEY".to_string(),
            LlmError::RateLimit(_) => "请求过于频繁,请稍后再试".to_string(),
            LlmError::InvalidRequest(m) => format!("请求无效: {}", m),
            LlmError::Server { status, .. } => format!("服务器错误 (HTTP {})", status),
            LlmError::Network(_) => "网络错误:无法连接到 LLM 服务".to_string(),
        }
    }
}

/// Intermediate parsed shape for the Anthropic / GLM / OpenAI error JSON.
///
/// The GLM compatibility layer wraps things inconsistently — sometimes
/// `{"error": {"type": "...", "message": "..."}}`, sometimes
/// `{"type": "error", "error": {"type": "...", "message": "..."}}`. OpenAI
/// uses the same outer shape but its discriminator field is `code`
/// (e.g. `"invalid_api_key"`, `"rate_limit_exceeded"`) rather than
/// `type` (`"authentication_error"`, `"rate_limit_error"`). This
/// struct tolerates both with `Option` fields and we try multiple lookup
/// paths in [`classify_error_response`].
#[derive(Debug, Default, serde::Deserialize)]
struct RawErrorBody {
    #[serde(default)]
    error: Option<RawErrorInner>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RawErrorInner {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Normalize an HTTP error response into an [`LlmError`]. Body is the raw
/// response text (may be non-JSON — we fall back gracefully).
///
/// The keyword match looks at both `error.type` (Anthropic / GLM
/// convention) and `error.code` (OpenAI convention). The PR3 OpenAI
/// adapter's error bodies look like
/// `{"error": {"code": "invalid_api_key", "message": "..."}}` and
/// should classify as [`LlmError::Auth`]. PR1/PR2's Anthropic / GLM
/// tests use `error.type` and continue to pass.
pub fn classify_error_response(status: u16, body: &str) -> LlmError {
    let parsed: RawErrorBody = serde_json::from_str(body).unwrap_or_default();

    // The two upstream conventions are:
    // - Anthropic / GLM:  `error.type` carries the discriminator
    //   (e.g. "authentication_error", "rate_limit_error").
    // - OpenAI: `error.code` carries the discriminator
    //   (e.g. "invalid_api_key", "rate_limit_exceeded"), and
    //   `error.type` is a literal "error" that is NOT a
    //   discriminator.
    //
    // We pull both fields and use the first one whose value
    // matches a classification keyword. The fallback chain is:
    //   1. `error.type` if it contains a keyword
    //   2. `error.code` if it contains a keyword
    //   3. top-level `type` (Anthropic sometimes wraps here)
    //   4. `error.type` verbatim (no keyword match — final fallback
    //      so the caller still sees a useful string in `message`)
    let err_type = parsed
        .error
        .as_ref()
        .and_then(|e| e.r#type.clone());
    let err_code = parsed
        .error
        .as_ref()
        .and_then(|e| e.code.clone());
    let top_type = parsed.r#type.clone();

    let keyword_in = |s: &Option<String>| {
        s.as_deref()
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default()
    };
    let has_keyword = |s: &str| {
        let s = s.to_ascii_lowercase();
        s.contains("authentication")
            || s.contains("new_api_error")
            || s.contains("invalid_api_key")
            || s.contains("rate_limit")
            || s.contains("invalid_request")
    };

    let mut chosen: Option<String> = None;
    for cand in [&err_type, &err_code, &top_type] {
        let s = keyword_in(cand);
        if has_keyword(&s) {
            chosen = Some(s);
            break;
        }
    }
    // Final fallback: take any of the three verbatim so the
    // error message carries SOMETHING (e.g. OpenAI's literal
    // "error" string still surfaces).
    let keyword = chosen.unwrap_or_else(|| {
        err_type
            .or(err_code)
            .or(top_type)
            .unwrap_or_default()
            .to_ascii_lowercase()
    });

    let inner_message = parsed
        .error
        .as_ref()
        .and_then(|e| e.message.clone())
        .or(parsed.message.clone())
        .unwrap_or_else(|| body.chars().take(200).collect());

    let classified = if keyword.contains("authentication")
        || keyword.contains("new_api_error")
        || keyword.contains("invalid_api_key")
    {
        LlmError::Auth(inner_message)
    } else if keyword.contains("rate_limit") {
        LlmError::RateLimit(inner_message)
    } else if keyword.contains("invalid_request") {
        LlmError::InvalidRequest(inner_message)
    } else if status >= 500 {
        LlmError::Server { status, message: inner_message }
    } else if status >= 400 {
        // 4xx with no recognizable subtype — treat as invalid request.
        LlmError::InvalidRequest(inner_message)
    } else {
        LlmError::Server { status, message: inner_message }
    };

    classified
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glm_401_with_new_api_error_is_auth() {
        let body = r#"{"error":{"code":"","message":"Invalid token","type":"new_api_error"}}"#;
        let err = classify_error_response(401, body);
        assert!(matches!(err, LlmError::Auth(_)));
        assert_eq!(err.category(), LlmErrorCategory::Auth);
    }

    #[test]
    fn anthropic_401_with_authentication_error_is_auth() {
        let body = r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let err = classify_error_response(401, body);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn glm_400_returned_as_500_with_invalid_request_is_invalid() {
        // HACKING-llm "差异 2": GLM returns HTTP 500 for empty content,
        // but the body says invalid_request_error.
        let body = r#"{"error":{"type":"invalid_request_error","message":"empty prompt"}}"#;
        let err = classify_error_response(500, body);
        assert!(matches!(err, LlmError::InvalidRequest(_)));
    }

    #[test]
    fn rate_limit_keyword_classified() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let err = classify_error_response(429, body);
        assert!(matches!(err, LlmError::RateLimit(_)));
    }

    #[test]
    fn bare_server_5xx_with_no_type_is_server() {
        let body = "internal server error";
        let err = classify_error_response(502, body);
        assert!(matches!(err, LlmError::Server { status: 502, .. }));
    }

    #[test]
    fn nested_wrapper_is_tolerated() {
        // Some GLM responses wrap the error object twice.
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"x"}}"#;
        let err = classify_error_response(401, body);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn user_messages_are_chinese_friendly() {
        let auth_err = LlmError::Auth("x".into());
        assert!(auth_err.user_message().contains("API key"));
        let net_err = LlmError::Network("x".into());
        assert!(net_err.user_message().contains("网络"));
    }
}
