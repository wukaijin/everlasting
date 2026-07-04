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
//! message and decide whether retrying makes sense. `RateLimit` / `Server`
//! additionally carry an optional `retry_after` advisory (parsed from
//! response headers by [`classify_error_response`]) consumed by
//! `llm/retry.rs` to override Full Jitter when the server says how long to
//! wait.

use super::types::LlmErrorCategory;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("auth failed: {0}")]
    Auth(String),

    #[error("rate limited: {message}")]
    RateLimit {
        message: String,
        /// Server-advised retry delay, parsed from `retry-after` /
        /// `retry-after-ms` / OpenAI `x-ratelimit-reset-*` (capped at 60s
        /// inside [`classify_error_response`]). `None` = no advisory —
        /// `llm/retry.rs` falls back to Full Jitter.
        retry_after: Option<std::time::Duration>,
    },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("server error (HTTP {status}): {message}")]
    Server {
        status: u16,
        message: String,
        /// Same semantics as [`LlmError::RateLimit::retry_after`] — rare for
        /// 5xx but some proxies advise a cooldown on 503.
        retry_after: Option<std::time::Duration>,
    },

    #[error("network error: {0}")]
    Network(String),
}

impl LlmError {
    pub fn category(&self) -> LlmErrorCategory {
        match self {
            LlmError::Auth(_) => LlmErrorCategory::Auth,
            LlmError::RateLimit { .. } => LlmErrorCategory::RateLimit,
            LlmError::InvalidRequest(_) => LlmErrorCategory::InvalidRequest,
            LlmError::Server { .. } => LlmErrorCategory::Server,
            LlmError::Network(_) => LlmErrorCategory::Network,
        }
    }

    /// Short, user-facing message. Suitable for display in the chat UI.
    pub fn user_message(&self) -> String {
        match self {
            LlmError::Auth(_) => "API key 无效或已过期,请检查 ANTHROPIC_API_KEY".to_string(),
            LlmError::RateLimit { .. } => "请求过于频繁,请稍后再试".to_string(),
            LlmError::InvalidRequest(m) => format!("请求无效: {}", m),
            LlmError::Server { status, .. } => format!("服务器错误 (HTTP {})", status),
            LlmError::Network(_) => "网络错误:无法连接到 LLM 服务".to_string(),
        }
    }

    /// Whether a fresh attempt could plausibly succeed. Aligns with
    /// `AppError::retryable()`'s default category derivation
    /// (`Server` / `Network` / `RateLimit` → retryable). `Auth` /
    /// `InvalidRequest` are deterministic — retrying burns token budget.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.category(),
            LlmErrorCategory::Server | LlmErrorCategory::Network | LlmErrorCategory::RateLimit
        )
    }

    /// Server-advised retry delay, if any. `None` for non-retryable variants
    /// and when no advisory header was present. `llm/retry.rs` honors this
    /// over Full Jitter when set.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            LlmError::RateLimit { retry_after, .. }
            | LlmError::Server { retry_after, .. } => *retry_after,
            _ => None,
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

/// Default cap on a server-advised retry delay. Aligns with Anthropic / OpenAI
/// SDK `_calculate_retry_timeout` (both ignore `retry-after > 60s` and fall
/// back to exponential backoff). Exposed for tests + `llm/retry.rs`.
pub const RETRY_AFTER_CAP_SECS: u64 = 60;

/// Normalize an HTTP error response into an [`LlmError`]. Body is the raw
/// response text (may be non-JSON — we fall back gracefully).
///
/// `headers` is the response's header map (pass `Some(&resp.headers())` from
/// the provider HTTP error branch); when `None`, no `retry_after` advisory
/// is parsed (tests use this). The `RateLimit` / `Server` variants carry the
/// parsed advisory so `llm/retry.rs` can honor it.
///
/// The keyword match looks at both `error.type` (Anthropic / GLM
/// convention) and `error.code` (OpenAI convention). The PR3 OpenAI
/// adapter's error bodies look like
/// `{"error": {"code": "invalid_api_key", "message": "..."}}` and
/// should classify as [`LlmError::Auth`]. PR1/PR2's Anthropic / GLM
/// tests use `error.type` and continue to pass.
pub fn classify_error_response(
    status: u16,
    body: &str,
    headers: Option<&reqwest::header::HeaderMap>,
) -> LlmError {
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

    // Parse the server's retry advisory once (only consumed by RateLimit /
    // Server variants). Capped at RETRY_AFTER_CAP_SECS.
    let retry_after = headers.and_then(|h| {
        parse_retry_after(h, std::time::Duration::from_secs(RETRY_AFTER_CAP_SECS))
    });

    let classified = if keyword.contains("authentication")
        || keyword.contains("new_api_error")
        || keyword.contains("invalid_api_key")
    {
        LlmError::Auth(inner_message)
    } else if keyword.contains("rate_limit") {
        LlmError::RateLimit { message: inner_message, retry_after }
    } else if keyword.contains("invalid_request") {
        LlmError::InvalidRequest(inner_message)
    } else if status >= 500 {
        LlmError::Server { status, message: inner_message, retry_after }
    } else if status >= 400 {
        // 4xx with no recognizable subtype — treat as invalid request.
        LlmError::InvalidRequest(inner_message)
    } else {
        LlmError::Server { status, message: inner_message, retry_after }
    };

    classified
}

/// Parse a server retry advisory from response headers, capped at `cap`.
///
/// Resolution order (first hit wins), mirroring Anthropic / OpenAI SDK
/// `_parse_retry_after_header`:
/// 1. `retry-after-ms` (non-standard, milliseconds)
/// 2. `retry-after` — integer seconds only. The RFC also permits an HTTP-date
///    form, but neither Anthropic nor OpenAI emits it in practice; a
///    non-numeric value falls through.
/// 3. OpenAI `x-ratelimit-reset-requests` / `x-ratelimit-reset-tokens`
///    (Go duration strings like `6m0s` / `1s` / `500ms`) — OpenAI rarely
///    emits the standard `retry-after`, so these are the real signal on the
///    OpenAI protocol path.
///
/// Values exceeding `cap` are truncated to `cap` (the SDKs cap at 60s).
/// Returns `None` when no advisory is present or unparseable.
pub fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
    cap: std::time::Duration,
) -> Option<std::time::Duration> {
    use std::time::Duration;
    let cap_min = |d: Duration| std::cmp::min(d, cap);

    // 1. retry-after-ms (non-standard, milliseconds)
    if let Some(ms) = headers
        .get("retry-after-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Some(cap_min(Duration::from_millis(ms)));
    }

    // 2. retry-after — integer seconds. Non-numeric (HTTP-date) falls through.
    if let Some(secs) = headers
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        return Some(cap_min(Duration::from_secs(secs)));
    }

    // 3. OpenAI x-ratelimit-reset-* (Go duration strings).
    for key in ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"] {
        if let Some(d) = headers
            .get(key)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_go_duration)
        {
            return Some(cap_min(d));
        }
    }

    None
}

/// Parse a Go duration string (`6m0s`, `1s`, `500ms`, `2h30m`, `1.5s`).
///
/// Grammar: optional leading sign, then repeated `<number>[.frac]<unit>`
/// pairs where unit is one of `h` / `m` / `s` / `ms` / `us` / `ns`.
/// Hand-written — no extra dependency (`humantime` doesn't speak Go
/// duration). Returns `None` on any parse error or overflow.
fn parse_go_duration(s: &str) -> Option<std::time::Duration> {
    use std::time::Duration;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut chars = s.chars().peekable();
    // optional sign
    let neg = match chars.peek() {
        Some('-') => {
            chars.next();
            true
        }
        Some('+') => {
            chars.next();
            false
        }
        _ => false,
    };
    let mut total: Duration = Duration::ZERO;
    while chars.peek().is_some() {
        // integer part
        let mut int_part: u64 = 0;
        let mut got_digit = false;
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                int_part = int_part.saturating_mul(10).saturating_add((c as u8 - b'0') as u64);
                chars.next();
                got_digit = true;
            } else {
                break;
            }
        }
        if !got_digit {
            return None;
        }
        // optional fractional part (stored as f64 for the unit multiply)
        let mut value = int_part as f64;
        if chars.peek() == Some(&'.') {
            chars.next();
            let mut frac: f64 = 0.0;
            let mut scale: f64 = 0.1;
            let mut got_frac_digit = false;
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    frac += (c as u8 - b'0') as f64 * scale;
                    scale *= 0.1;
                    chars.next();
                    got_frac_digit = true;
                } else {
                    break;
                }
            }
            if !got_frac_digit {
                return None;
            }
            value += frac;
        }
        // unit
        let mut unit = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_alphabetic() {
                unit.push(c);
                chars.next();
            } else {
                break;
            }
        }
        let add_secs: f64 = match unit.as_str() {
            "h" => value * 3600.0,
            "m" => value * 60.0,
            "s" => value,
            "ms" => value / 1000.0,
            "us" | "µs" => value / 1_000_000.0,
            "ns" => value / 1_000_000_000.0,
            _ => return None,
        };
        if add_secs <= 0.0 && value != 0.0 {
            // underflow on tiny ns — treat as 0, don't fail
        }
        let nanos = (add_secs * 1_000_000_000.0) as u64;
        total = total.checked_add(Duration::from_nanos(nanos))?;
    }
    if neg && total > Duration::ZERO {
        // Negative durations are nonsensical for retry-after; clamp to None.
        return None;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glm_401_with_new_api_error_is_auth() {
        let body = r#"{"error":{"code":"","message":"Invalid token","type":"new_api_error"}}"#;
        let err = classify_error_response(401, body, None);
        assert!(matches!(err, LlmError::Auth(_)));
        assert_eq!(err.category(), LlmErrorCategory::Auth);
    }

    #[test]
    fn anthropic_401_with_authentication_error_is_auth() {
        let body = r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let err = classify_error_response(401, body, None);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn glm_400_returned_as_500_with_invalid_request_is_invalid() {
        // HACKING-llm "差异 2": GLM returns HTTP 500 for empty content,
        // but the body says invalid_request_error.
        let body = r#"{"error":{"type":"invalid_request_error","message":"empty prompt"}}"#;
        let err = classify_error_response(500, body, None);
        assert!(matches!(err, LlmError::InvalidRequest(_)));
    }

    #[test]
    fn rate_limit_keyword_classified() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let err = classify_error_response(429, body, None);
        assert!(matches!(err, LlmError::RateLimit { .. }));
    }

    #[test]
    fn bare_server_5xx_with_no_type_is_server() {
        let body = "internal server error";
        let err = classify_error_response(502, body, None);
        assert!(matches!(err, LlmError::Server { status: 502, .. }));
    }

    #[test]
    fn nested_wrapper_is_tolerated() {
        // Some GLM responses wrap the error object twice.
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"x"}}"#;
        let err = classify_error_response(401, body, None);
        assert!(matches!(err, LlmError::Auth(_)));
    }

    #[test]
    fn user_messages_are_chinese_friendly() {
        let auth_err = LlmError::Auth("x".into());
        assert!(auth_err.user_message().contains("API key"));
        let net_err = LlmError::Network("x".into());
        assert!(net_err.user_message().contains("网络"));
    }

    // --- A5+ retry support tests (2026-07-04) ---

    #[test]
    fn is_retryable_aligns_with_category() {
        assert!(!LlmError::Auth("x".into()).is_retryable());
        assert!(!LlmError::InvalidRequest("x".into()).is_retryable());
        assert!(LlmError::Network("x".into()).is_retryable());
        assert!(LlmError::RateLimit { message: "x".into(), retry_after: None }.is_retryable());
        assert!(LlmError::Server {
            status: 503,
            message: "x".into(),
            retry_after: None
        }
        .is_retryable());
    }

    #[test]
    fn retry_after_accessor_only_on_retryable_variants() {
        // Non-retryable variants never carry an advisory.
        assert_eq!(LlmError::Auth("x".into()).retry_after(), None);
        assert_eq!(LlmError::InvalidRequest("x".into()).retry_after(), None);
        assert_eq!(LlmError::Network("x".into()).retry_after(), None);
        // Retryable variants surface what classify parsed.
        let with_advisory = LlmError::RateLimit {
            message: "x".into(),
            retry_after: Some(std::time::Duration::from_secs(5)),
        };
        assert_eq!(with_advisory.retry_after(), Some(std::time::Duration::from_secs(5)));
        let without_advisory = LlmError::Server {
            status: 503,
            message: "x".into(),
            retry_after: None,
        };
        assert_eq!(without_advisory.retry_after(), None);
    }

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut m = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            // unwrap ok: test fixtures are valid ASCII
            m.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn parse_retry_after_integer_seconds() {
        let h = headers(&[("retry-after", "5")]);
        assert_eq!(
            parse_retry_after(&h, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn parse_retry_after_ms_takes_priority() {
        // retry-after-ms wins over retry-after per SDK order.
        let h = headers(&[("retry-after-ms", "750"), ("retry-after", "5")]);
        assert_eq!(
            parse_retry_after(&h, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_millis(750))
        );
    }

    #[test]
    fn parse_retry_after_capped_at_60s() {
        // Server asks 120s → truncated to the 60s cap (SDK parity).
        let h = headers(&[("retry-after", "120")]);
        assert_eq!(
            parse_retry_after(&h, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn parse_retry_after_openai_go_duration() {
        // OpenAI x-ratelimit-reset-requests carries Go duration strings;
        // OpenAI rarely emits the standard retry-after.
        let h = headers(&[("x-ratelimit-reset-requests", "6m0s")]);
        assert_eq!(
            parse_retry_after(&h, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_secs(60)) // 360s capped to 60
        );
        let h2 = headers(&[("x-ratelimit-reset-requests", "500ms")]);
        assert_eq!(
            parse_retry_after(&h2, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_millis(500))
        );
        let h3 = headers(&[("x-ratelimit-reset-tokens", "1s")]);
        assert_eq!(
            parse_retry_after(&h3, std::time::Duration::from_secs(60)),
            Some(std::time::Duration::from_secs(1))
        );
    }

    #[test]
    fn parse_retry_after_missing_is_none() {
        let h = headers(&[]);
        assert_eq!(parse_retry_after(&h, std::time::Duration::from_secs(60)), None);
    }

    #[test]
    fn parse_retry_after_non_numeric_falls_through() {
        // HTTP-date form is RFC-allowed but unsupported in MVP (Anthropic /
        // OpenAI use integer seconds in practice). Falls through to OpenAI
        // keys, then None.
        let h = headers(&[("retry-after", "Wed, 21 Oct 2026 07:28:00 GMT")]);
        assert_eq!(parse_retry_after(&h, std::time::Duration::from_secs(60)), None);
    }

    #[test]
    fn classify_429_with_retry_after_header_carries_advisory() {
        let body = r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let h = headers(&[("retry-after", "5")]);
        let err = classify_error_response(429, body, Some(&h));
        match err {
            LlmError::RateLimit { retry_after: Some(d), .. } => {
                assert_eq!(d, std::time::Duration::from_secs(5));
            }
            _ => panic!("expected RateLimit with advisory"),
        }
    }

    #[test]
    fn classify_5xx_without_advisory_has_none() {
        let body = "internal server error";
        let err = classify_error_response(503, body, None);
        match err {
            LlmError::Server { retry_after: None, .. } => {}
            _ => panic!("expected Server with no advisory"),
        }
    }

    #[test]
    fn parse_go_duration_examples() {
        use std::time::Duration;
        assert_eq!(parse_go_duration("1s"), Some(Duration::from_secs(1)));
        assert_eq!(parse_go_duration("6m0s"), Some(Duration::from_secs(360)));
        assert_eq!(parse_go_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_go_duration("2h30m"), Some(Duration::from_secs(9000)));
        assert_eq!(parse_go_duration("1.5s"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_go_duration("0s"), Some(Duration::ZERO));
        // malformed
        assert_eq!(parse_go_duration(""), None);
        assert_eq!(parse_go_duration("abc"), None);
        assert_eq!(parse_go_duration("10"), None); // no unit
        assert_eq!(parse_go_duration("-5s"), None); // negative rejected
    }
}
