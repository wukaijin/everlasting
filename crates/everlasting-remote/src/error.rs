//! HTTP 错误契约(design §3.5,对齐 daemon `AppCommandError` 的
//! category/status 映射,但 wire shape 更简 —— 只有 `category` + `message`)。
//!
//! 所有 4xx/5xx 响应统一 shape(P3-1 修订:**5 个变体全实现**,
//! 原 design 漏 `RateLimit`,前端按 category 路由会遇 429 错乱):
//!
//! ```json
//! { "category": "Auth" | "RateLimit" | "InvalidRequest" | "Server" | "Network",
//!   "message": "..." }
//! ```
//!
//! 状态码:`Auth→401` `RateLimit→429` `InvalidRequest→400` `Server→500`
//! `Network→502`(`node_offline` 也走 502 + Network,见 Step 6 proxy)。
//! `category` 用 `#[serde(rename_all = "PascalCase")]`,与 daemon
//! `error.rs` 的 `ErrorCategory` 序列化一致。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// 5 类 category。前端按其路由(Auth→Settings / RateLimit→toast /
/// InvalidRequest→inline / Server→toast+重试 / Network→toast)。
/// 与 daemon `ErrorCategory` 1:1 同义,但是独立类型 —— remote 不依赖
/// daemon crate(design 不变量 1:零依赖 daemon 的 `everlasting_lib`)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ErrorCategory {
    Auth,
    RateLimit,
    InvalidRequest,
    Server,
    Network,
}

/// HTTP 错误响应体。`IntoResponse` 把 category 映射到状态码(下表),
/// body 即本 struct 的 JSON。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub category: ErrorCategory,
    pub message: String,
}

impl AppError {
    pub fn new(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Auth, message)
    }
    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::RateLimit, message)
    }
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::InvalidRequest, message)
    }
    pub fn server(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Server, message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Network, message)
    }
}

/// `ErrorCategory` → HTTP 状态码(design §3.5 契约表)。Step 6 proxy 的
/// `node_offline`(Network→502)也复用它。
pub fn status_for_category(category: ErrorCategory) -> StatusCode {
    match category {
        ErrorCategory::Auth => StatusCode::UNAUTHORIZED,
        ErrorCategory::RateLimit => StatusCode::TOO_MANY_REQUESTS,
        ErrorCategory::InvalidRequest => StatusCode::BAD_REQUEST,
        ErrorCategory::Server => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCategory::Network => StatusCode::BAD_GATEWAY,
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = status_for_category(self.category);
        (status, Json(self)).into_response()
    }
}

/// handler 里 `?` 冒泡边界:任何 anyhow 错误落 Server 兜底(message
/// 保留原始串,供日志/排障)。与 daemon `From<anyhow::Error> for
/// AppCommandError` 的兜底语义一致(remote 无领域错误 downcast 需求,
/// 变体错误在 Step 4-8 逐步加)。
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::server(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个 `ErrorCategory` 落到其契约状态码(design §3.5)。
    /// 前端按 status 分类(401→Settings / 429→toast / 400→inline /
    /// 500→toast+重试 / 502→toast),映射错位会路由错乱。
    #[test]
    fn status_mapping_is_stable() {
        assert_eq!(
            status_for_category(ErrorCategory::Auth),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_for_category(ErrorCategory::RateLimit),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status_for_category(ErrorCategory::InvalidRequest),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for_category(ErrorCategory::Server),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for_category(ErrorCategory::Network),
            StatusCode::BAD_GATEWAY
        );
    }

    /// wire shape:`{"category":"PascalCase","message":...}` —— 与
    /// daemon `ErrorCategory` 的序列化一致(前端 `TransportError` 解析
    /// 依赖 `category` 字段 + PascalCase 值)。
    #[test]
    fn wire_shape_serializes_pascal_case_category() {
        let err = AppError::rate_limit("太快了");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"category\":\"RateLimit\""), "got {json}");
        assert!(json.contains("\"message\":\"太快了\""), "got {json}");
    }

    /// `IntoResponse` 把 category 的状态码与 body JSON 配对。
    #[test]
    fn into_response_pairs_status_with_body() {
        let err = AppError::auth("bad token");
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 5 个构造器各归其位(RateLimit 不落别的类目 —— P3-1 修订点)。
    #[test]
    fn constructors_map_to_correct_category() {
        assert_eq!(AppError::auth("a").category, ErrorCategory::Auth);
        assert_eq!(AppError::rate_limit("r").category, ErrorCategory::RateLimit);
        assert_eq!(
            AppError::invalid_request("i").category,
            ErrorCategory::InvalidRequest
        );
        assert_eq!(AppError::server("s").category, ErrorCategory::Server);
        assert_eq!(AppError::network("n").category, ErrorCategory::Network);
    }

    /// anyhow 冒泡兜底 → Server,message 保留原始串。
    #[test]
    fn anyhow_fallback_is_server() {
        let err = AppError::from(anyhow::anyhow!("boom"));
        assert_eq!(err.category, ErrorCategory::Server);
        assert_eq!(err.message, "boom");
    }
}
