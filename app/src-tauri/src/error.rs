//! A5 错误处理完善:全栈错误契约的中心模块。
//!
//! 集中定义:
//! - [`ErrorCategory`] — 5 类 category(IPC 边界 + 前端路由键)
//! - [`AppError`] — 对外错误统一接口(category / user_message / retryable)
//! - [`AppCommandError`] — Tauri command 的 wire shape(替代旧 `Result<T, String>`)
//! - 10 个领域错误类型的 `impl AppError` + `From<E> for AppCommandError`
//! - `From<anyhow::Error>` 边界兜底(commands `?anyhow` 路径)
//!
//! 设计:见任务 `07-02-a5-error-handling-refine` 的 design.md §3/§4/§5。
//! spec:见 `.trellis/spec/backend/error-handling.md`。

use crate::agent::auto_reflect::ReflectError;
use crate::agent::provider::PreFlightError;
use crate::agent::question_store::QuestionStoreError;
use crate::background_shell::BackgroundShellError;
use crate::db::memories::{MemoryInsertError, StatusTransitionError};
use crate::git::error::GitError;
use crate::llm::error::LlmError;
use crate::llm::provider::ProviderBuildError;
use crate::llm::types::LlmErrorCategory;
use crate::tools::web_fetch::WebFetchError;
use serde::{Deserialize, Serialize};

/// 5 类 category。前端按其路由(Auth→Settings / RateLimit→toast /
/// InvalidRequest→inline / Server→toast+重试 / Network→toast)。
///
/// 与 [`LlmErrorCategory`] 1:1,但是独立类型(IPC command 通道,与
/// `ChatEvent::Error` 的 stream 通道分离 —— 后者继续用 `LlmErrorCategory`,
/// 本期不统一,见 error-handling.md §Overview 四层模型)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ErrorCategory {
    Auth,
    RateLimit,
    InvalidRequest,
    Server,
    Network,
}

impl From<LlmErrorCategory> for ErrorCategory {
    fn from(c: LlmErrorCategory) -> Self {
        match c {
            LlmErrorCategory::Auth => ErrorCategory::Auth,
            LlmErrorCategory::RateLimit => ErrorCategory::RateLimit,
            LlmErrorCategory::InvalidRequest => ErrorCategory::InvalidRequest,
            LlmErrorCategory::Server => ErrorCategory::Server,
            LlmErrorCategory::Network => ErrorCategory::Network,
        }
    }
}

/// 对外错误统一接口。每个冒泡到 IPC 边界的错误类型都 impl 它。
///
/// `retryable` 默认按 category 派生(Server/Network/RateLimit=true);
/// 本期零 override(见 design.md §4 / D2)。
pub trait AppError: std::error::Error {
    fn category(&self) -> ErrorCategory;
    fn user_message(&self) -> String;
    fn retryable(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::Server | ErrorCategory::Network | ErrorCategory::RateLimit
        )
    }
}

/// Tauri command 错误 wire shape。替代旧 `Result<T, String>`。
///
/// 高频 command(chat/cancel/sessions/projects/merge_worker/discard_worker)
/// 在 command 体内覆盖 `request_id`(透传前端 requestId);轻量 command 留 None。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCommandError {
    /// 前端路由键。
    pub category: ErrorCategory,
    /// 类型短名(如 "LlmError"/"GitError"),供日志诊断;variant 级信息在 message + tracing。
    pub kind: String,
    /// 中文友好消息,直接展示用户。
    pub message: String,
    /// 前端决定是否提供"重试"按钮。
    pub retryable: bool,
    /// 关联 tracing log(高频 command 透传前端 requestId,轻量 command 为 None)。
    pub request_id: Option<String>,
}

impl AppCommandError {
    /// 手动构造(command body 内 `Err(String)` 路径转用)。retryable 按 category 派生。
    pub fn new(category: ErrorCategory, msg: impl Into<String>) -> Self {
        let retryable = matches!(
            category,
            ErrorCategory::Server | ErrorCategory::Network | ErrorCategory::RateLimit
        );
        Self {
            category,
            kind: "Manual".to_string(),
            message: msg.into(),
            retryable,
            request_id: None,
        }
    }

    /// 链式注入 request_id(高频 command 透传前端 requestId)。
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }
}

fn kind_of<E: ?Sized>(_e: &E) -> String {
    let full = std::any::type_name::<E>();
    full.rsplit("::").next().unwrap_or(full).to_string()
}

fn build<E: AppError>(e: &E) -> AppCommandError {
    AppCommandError {
        category: e.category(),
        kind: kind_of(e),
        message: e.user_message(),
        retryable: e.retryable(),
        request_id: None,
    }
}

// =========================================================================
// impl AppError for 10 个对外错误类型(集中;新增类型只改本文件)
// =========================================================================

impl AppError for LlmError {
    fn category(&self) -> ErrorCategory {
        // LlmError 已有 inherent category() -> LlmErrorCategory,1:1 映射。
        // 重新 match 而非调 inherent,避免 trait/inherent 同名方法歧义。
        match self {
            LlmError::Auth(_) => ErrorCategory::Auth,
            LlmError::RateLimit { .. } => ErrorCategory::RateLimit,
            LlmError::InvalidRequest(_) => ErrorCategory::InvalidRequest,
            LlmError::Server { .. } => ErrorCategory::Server,
            LlmError::Network(_) => ErrorCategory::Network,
        }
    }
    fn user_message(&self) -> String {
        // 与 inherent user_message()(llm/error.rs)文案保持一致;改文案时同步两侧(测试兜底)。
        match self {
            LlmError::Auth(_) => "API key 无效或已过期,请检查 ANTHROPIC_API_KEY".to_string(),
            LlmError::RateLimit { .. } => "请求过于频繁,请稍后再试".to_string(),
            LlmError::InvalidRequest(m) => format!("请求无效: {}", m),
            LlmError::Server { status, .. } => format!("服务器错误 (HTTP {})", status),
            LlmError::Network(_) => "网络错误:无法连接到 LLM 服务".to_string(),
        }
    }
}

impl AppError for GitError {
    fn category(&self) -> ErrorCategory {
        match self {
            GitError::NotARepo { .. } | GitError::Dirty { .. } => ErrorCategory::InvalidRequest,
            GitError::Io { .. } | GitError::Git2(_) => ErrorCategory::Server,
        }
    }
    fn user_message(&self) -> String {
        match self {
            GitError::NotARepo { path } => format!("项目不是 git 仓库: {}", path),
            GitError::Io { path, .. } => format!("git IO 错误: {}", path),
            GitError::Git2(e) => format!("git 错误: {}", e),
            GitError::Dirty { path, .. } => {
                format!("工作区 {} 有未提交改动,请先提交或丢弃", path)
            }
        }
    }
}

impl AppError for MemoryInsertError {
    fn category(&self) -> ErrorCategory {
        match self {
            MemoryInsertError::Db(_) => ErrorCategory::Server,
            _ => ErrorCategory::InvalidRequest, // 9 个校验 variant
        }
    }
    fn user_message(&self) -> String {
        match self {
            MemoryInsertError::EmptyTitle => "记忆标题不能为空".to_string(),
            MemoryInsertError::EmptyContent => "记忆内容不能为空".to_string(),
            MemoryInsertError::TitleTooLong(_) => "记忆标题过长".to_string(),
            MemoryInsertError::ContentTooLong(_) => "记忆内容过长".to_string(),
            MemoryInsertError::SensitiveContent => {
                "记忆内容包含敏感信息(api_key/secret/password 等),已拒绝".to_string()
            }
            MemoryInsertError::SensitivePath(p) => format!("记忆内容引用了敏感路径: {}", p),
            MemoryInsertError::TemporaryPath(p) => format!("记忆内容引用了临时路径: {}", p),
            MemoryInsertError::ProjectScopeMissingId => {
                "scope=Project 需指定 project_id".to_string()
            }
            MemoryInsertError::UserScopeHasProjectId(id) => {
                format!("scope=User 不应携带 project_id: {}", id)
            }
            MemoryInsertError::Db(_) => "记忆数据库错误".to_string(),
        }
    }
}

impl AppError for StatusTransitionError {
    fn category(&self) -> ErrorCategory {
        match self {
            // ⚠️ Db 归 Server(非 InvalidRequest)—— design §5 修正点。
            StatusTransitionError::Db(_) => ErrorCategory::Server,
            StatusTransitionError::NotFound(_) | StatusTransitionError::Illegal { .. } => {
                ErrorCategory::InvalidRequest
            }
        }
    }
    fn user_message(&self) -> String {
        match self {
            StatusTransitionError::NotFound(id) => format!("记忆 {} 不存在", id),
            StatusTransitionError::Illegal { from, to } => {
                format!("非法状态转移: {} → {}", from, to)
            }
            StatusTransitionError::Db(_) => "记忆数据库错误".to_string(),
        }
    }
}

impl AppError for QuestionStoreError {
    fn category(&self) -> ErrorCategory {
        ErrorCategory::InvalidRequest // AlreadyPending / NotFound 均归 InvalidRequest
    }
    fn user_message(&self) -> String {
        match self {
            QuestionStoreError::AlreadyPending => "该会话已有待答问题,请先完成当前回答".to_string(),
            QuestionStoreError::NotFound => "该会话没有待答问题".to_string(),
        }
    }
}

impl AppError for PreFlightError {
    fn category(&self) -> ErrorCategory {
        // design D6:EmptyApiKey/DecryptFailed → Auth(DecryptFailed 从旧
        // user_message_and_category 的 InvalidRequest 提升到 Auth,与前端
        // Auth 路由"引导去 Settings"语义对齐)。
        match self {
            PreFlightError::EmptyApiKey { .. } | PreFlightError::DecryptFailed { .. } => {
                ErrorCategory::Auth
            }
            PreFlightError::NoModel
            | PreFlightError::ProviderMissing
            | PreFlightError::BuildFailed(_) => ErrorCategory::InvalidRequest,
        }
    }
    fn user_message(&self) -> String {
        // 复用 inherent user_message_and_category().0(文案不动;只有 category 按 D6 调整)。
        self.user_message_and_category().0
    }
}

impl AppError for BackgroundShellError {
    fn category(&self) -> ErrorCategory {
        match self {
            BackgroundShellError::NotFound { .. }
            | BackgroundShellError::WrongSession { .. }
            | BackgroundShellError::InvalidCwd { .. } => ErrorCategory::InvalidRequest,
            BackgroundShellError::Spawn(_) | BackgroundShellError::Poisoned(_) => {
                ErrorCategory::Server
            }
        }
    }
    fn user_message(&self) -> String {
        match self {
            BackgroundShellError::NotFound { .. } => "后台 shell 不存在".to_string(),
            BackgroundShellError::WrongSession { .. } => "后台 shell 属于其他会话".to_string(),
            BackgroundShellError::Spawn(_) => "后台 shell 启动失败".to_string(),
            BackgroundShellError::InvalidCwd { path, reason } => {
                format!("后台 shell 工作目录无效 {}: {}", path, reason)
            }
            BackgroundShellError::Poisoned(_) => "后台 shell registry 锁污染".to_string(),
        }
    }
}

impl AppError for ReflectError {
    fn category(&self) -> ErrorCategory {
        match self {
            ReflectError::Llm(_) => ErrorCategory::Server,
            ReflectError::NoText
            | ReflectError::Json(_)
            | ReflectError::MissingField(_)
            | ReflectError::Insert(_) => ErrorCategory::InvalidRequest,
        }
    }
    fn user_message(&self) -> String {
        match self {
            ReflectError::Llm(_) => "反思 LLM 调用失败".to_string(),
            ReflectError::NoText => "反思 LLM 未返回内容".to_string(),
            ReflectError::Json(_) => "反思 LLM 响应解析失败".to_string(),
            ReflectError::MissingField(f) => format!("反思 LLM 响应缺少字段: {}", f),
            ReflectError::Insert(m) => format!("反思记忆写入被拒绝: {}", m),
        }
    }
}

impl AppError for WebFetchError {
    fn category(&self) -> ErrorCategory {
        match self {
            // HttpStatus(u16) 按 4xx/5xx 分流(design §5 唯一按数据分流的 variant)。
            WebFetchError::HttpStatus(code) => {
                if *code < 500 {
                    ErrorCategory::InvalidRequest
                } else {
                    ErrorCategory::Server
                }
            }
            WebFetchError::Timeout(_) | WebFetchError::Tls(_) | WebFetchError::Network(_) => {
                ErrorCategory::Network
            }
            WebFetchError::InvalidUrl(_)
            | WebFetchError::BlockedAddress(_)
            | WebFetchError::RedirectBlocked { .. }
            | WebFetchError::TooLarge => ErrorCategory::InvalidRequest,
        }
    }
    fn user_message(&self) -> String {
        match self {
            WebFetchError::InvalidUrl(u) => format!("URL 无效(需 http/https): {}", u),
            WebFetchError::BlockedAddress(_) => "拒绝抓取私有/回环地址(SSRF 保护)".to_string(),
            WebFetchError::RedirectBlocked { .. } => {
                "拒绝重定向到私有/回环地址(SSRF 保护)".to_string()
            }
            WebFetchError::TooLarge => "响应体超过 5 MiB 上限".to_string(),
            WebFetchError::HttpStatus(c) => format!("HTTP {}", c),
            WebFetchError::Timeout(_) => "请求超时".to_string(),
            WebFetchError::Tls(m) => format!("TLS 错误: {}", m),
            WebFetchError::Network(m) => format!("网络错误: {}", m),
        }
    }
}

impl AppError for ProviderBuildError {
    fn category(&self) -> ErrorCategory {
        ErrorCategory::InvalidRequest // NotImplemented / UnknownProtocol
    }
    fn user_message(&self) -> String {
        match self {
            ProviderBuildError::NotImplemented(p) => {
                format!("provider 协议 '{}' 尚未实现", p)
            }
            ProviderBuildError::UnknownProtocol(p) => format!("未知 provider 协议: '{}'", p),
        }
    }
}

// =========================================================================
// From<E> for AppCommandError(10 领域类型 + anyhow 边界兜底)
// =========================================================================

impl From<LlmError> for AppCommandError {
    fn from(e: LlmError) -> Self {
        build(&e)
    }
}
impl From<GitError> for AppCommandError {
    fn from(e: GitError) -> Self {
        build(&e)
    }
}
impl From<MemoryInsertError> for AppCommandError {
    fn from(e: MemoryInsertError) -> Self {
        build(&e)
    }
}
impl From<StatusTransitionError> for AppCommandError {
    fn from(e: StatusTransitionError) -> Self {
        build(&e)
    }
}
impl From<QuestionStoreError> for AppCommandError {
    fn from(e: QuestionStoreError) -> Self {
        build(&e)
    }
}
impl From<PreFlightError> for AppCommandError {
    fn from(e: PreFlightError) -> Self {
        build(&e)
    }
}
impl From<BackgroundShellError> for AppCommandError {
    fn from(e: BackgroundShellError) -> Self {
        build(&e)
    }
}
impl From<ReflectError> for AppCommandError {
    fn from(e: ReflectError) -> Self {
        build(&e)
    }
}
impl From<WebFetchError> for AppCommandError {
    fn from(e: WebFetchError) -> Self {
        build(&e)
    }
}
impl From<ProviderBuildError> for AppCommandError {
    fn from(e: ProviderBuildError) -> Self {
        build(&e)
    }
}

/// 边界兜底:commands `?anyhow` 路径。先 downcast 到已知领域类型(命中则
/// 复用其 AppError),未命中归 Server/`kind="Anyhow"`(retryable=true)。
impl From<anyhow::Error> for AppCommandError {
    fn from(e: anyhow::Error) -> Self {
        if let Some(x) = e.downcast_ref::<LlmError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<GitError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<MemoryInsertError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<StatusTransitionError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<QuestionStoreError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<PreFlightError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<BackgroundShellError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<ReflectError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<WebFetchError>() {
            return build(x);
        }
        if let Some(x) = e.downcast_ref::<ProviderBuildError>() {
            return build(x);
        }
        AppCommandError {
            category: ErrorCategory::Server,
            kind: "Anyhow".to_string(),
            message: e.to_string(),
            retryable: true,
            request_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- LlmError(5 variant)----
    // LlmError 有同名 inherent category()/user_message()(返 LlmErrorCategory/String),
    // 测 trait 时必须 `AppError::method(&e)` 显式限定,否则 inherent 优先调到旧方法。
    #[test]
    fn llm_error_category_and_message() {
        assert_eq!(
            AppError::category(&LlmError::Auth("x".into())),
            ErrorCategory::Auth
        );
        assert_eq!(
            AppError::category(&LlmError::RateLimit {
                message: "x".into(),
                retry_after: None
            }),
            ErrorCategory::RateLimit
        );
        assert_eq!(
            AppError::category(&LlmError::InvalidRequest("x".into())),
            ErrorCategory::InvalidRequest
        );
        assert_eq!(
            AppError::category(&LlmError::Server {
                status: 502,
                message: "x".into(),
                retry_after: None
            }),
            ErrorCategory::Server
        );
        assert_eq!(
            AppError::category(&LlmError::Network("x".into())),
            ErrorCategory::Network
        );
        assert!(AppError::user_message(&LlmError::Auth("x".into())).contains("API key"));
        assert!(AppError::user_message(&LlmError::Network("x".into())).contains("网络"));
        // retryable 默认派生(LlmError 无 inherent retryable)
        assert!(AppError::retryable(&LlmError::Server {
            status: 500,
            message: "".into(),
            retry_after: None
        }));
        assert!(!AppError::retryable(&LlmError::Auth("x".into())));
    }

    // ---- GitError ----
    #[test]
    fn git_error_categories() {
        assert_eq!(
            GitError::NotARepo { path: "/x".into() }.category(),
            ErrorCategory::InvalidRequest
        );
        assert!(matches!(
            GitError::Git2(git2::Error::from_str("x")).category(),
            ErrorCategory::Server
        ));
        assert!(matches!(
            GitError::Dirty {
                path: "/x".into(),
                paths: vec![]
            }
            .category(),
            ErrorCategory::InvalidRequest
        ));
    }

    // ---- MemoryInsertError(Db → Server,其余 → InvalidRequest)----
    #[test]
    fn memory_insert_categories() {
        assert_eq!(
            MemoryInsertError::EmptyTitle.category(),
            ErrorCategory::InvalidRequest
        );
        assert_eq!(
            MemoryInsertError::SensitiveContent.category(),
            ErrorCategory::InvalidRequest
        );
    }

    // ---- StatusTransitionError(Db → Server,design §5 修正点)----
    #[test]
    fn status_transition_db_is_server() {
        // NotFound/Illegal → InvalidRequest;Db → Server(非 InvalidRequest)
        assert_eq!(
            StatusTransitionError::NotFound("m1".into()).category(),
            ErrorCategory::InvalidRequest
        );
        assert_eq!(
            StatusTransitionError::Illegal { from: "a", to: "b" }.category(),
            ErrorCategory::InvalidRequest
        );
    }

    // ---- QuestionStoreError ----
    #[test]
    fn question_store_categories() {
        assert_eq!(
            QuestionStoreError::AlreadyPending.category(),
            ErrorCategory::InvalidRequest
        );
        assert_eq!(
            QuestionStoreError::NotFound.category(),
            ErrorCategory::InvalidRequest
        );
    }

    // ---- PreFlightError(design D6:EmptyApiKey/DecryptFailed → Auth 边界)----
    #[test]
    fn preflight_auth_boundary() {
        assert_eq!(
            PreFlightError::EmptyApiKey {
                provider_display_name: "P".into()
            }
            .category(),
            ErrorCategory::Auth
        );
        // DecryptFailed 提升到 Auth(旧 user_message_and_category 是 InvalidRequest)
        assert_eq!(
            PreFlightError::DecryptFailed {
                provider_display_name: "P".into()
            }
            .category(),
            ErrorCategory::Auth
        );
        assert_eq!(
            PreFlightError::NoModel.category(),
            ErrorCategory::InvalidRequest
        );
        assert!(PreFlightError::EmptyApiKey {
            provider_display_name: "P".into()
        }
        .user_message()
        .contains("api_key"));
    }

    // ---- BackgroundShellError ----
    #[test]
    fn background_shell_categories() {
        assert_eq!(
            BackgroundShellError::NotFound {
                session_id: "s".into(),
                shell_session_id: "sh".into()
            }
            .category(),
            ErrorCategory::InvalidRequest
        );
        assert!(matches!(
            BackgroundShellError::Poisoned("x".into()).category(),
            ErrorCategory::Server
        ));
    }

    // ---- ReflectError ----
    #[test]
    fn reflect_categories() {
        assert_eq!(
            ReflectError::Llm("x".into()).category(),
            ErrorCategory::Server
        );
        assert_eq!(
            ReflectError::NoText.category(),
            ErrorCategory::InvalidRequest
        );
    }

    // ---- WebFetchError(HttpStatus 4xx/5xx 分流 + Network 边界)----
    #[test]
    fn web_fetch_http_status_split() {
        assert_eq!(
            WebFetchError::HttpStatus(404).category(),
            ErrorCategory::InvalidRequest
        );
        assert!(!WebFetchError::HttpStatus(404).retryable());
        assert_eq!(
            WebFetchError::HttpStatus(502).category(),
            ErrorCategory::Server
        );
        assert!(WebFetchError::HttpStatus(502).retryable());
        assert_eq!(
            WebFetchError::BlockedAddress("127.0.0.1".parse().unwrap()).category(),
            ErrorCategory::InvalidRequest
        );
        assert_eq!(
            WebFetchError::Timeout(60).category(),
            ErrorCategory::Network
        );
    }

    // ---- ProviderBuildError ----
    #[test]
    fn provider_build_category() {
        assert_eq!(
            ProviderBuildError::UnknownProtocol("x".into()).category(),
            ErrorCategory::InvalidRequest
        );
    }

    // ---- From<E> for AppCommandError ----
    #[test]
    fn from_llm_error_preserves_category_and_kind() {
        let err: AppCommandError = LlmError::RateLimit {
            message: "slow".into(),
            retry_after: None,
        }
        .into();
        assert_eq!(err.category, ErrorCategory::RateLimit);
        assert_eq!(err.kind, "LlmError");
        assert_eq!(err.message, "请求过于频繁,请稍后再试");
        assert!(err.retryable);
        assert!(err.request_id.is_none());
    }

    #[test]
    fn from_preflight_auth() {
        let err: AppCommandError = PreFlightError::DecryptFailed {
            provider_display_name: "P".into(),
        }
        .into();
        assert_eq!(err.category, ErrorCategory::Auth);
        assert_eq!(err.kind, "PreFlightError");
        assert!(!err.retryable);
    }

    // ---- From<anyhow::Error>:downcast 命中复用,未命中兜底 Server ----
    #[test]
    fn from_anyhow_downcasts_known_type() {
        let any: anyhow::Error = LlmError::Auth("k".into()).into();
        let err: AppCommandError = any.into();
        assert_eq!(err.category, ErrorCategory::Auth);
        assert_eq!(err.kind, "LlmError"); // 复用领域类型,非 "Anyhow"
    }

    #[test]
    fn from_anyhow_fallback_server() {
        let any: anyhow::Error = anyhow::anyhow!("任意边界错误");
        let err: AppCommandError = any.into();
        assert_eq!(err.category, ErrorCategory::Server);
        assert_eq!(err.kind, "Anyhow");
        assert!(err.retryable);
        assert!(err.message.contains("任意边界错误"));
    }

    // ---- wire shape serialize(camelCase)----
    #[test]
    fn wire_shape_serializes_camel_case() {
        let err: AppCommandError = LlmError::RateLimit {
            message: "x".into(),
            retry_after: None,
        }
        .into();
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"category\":\"RateLimit\""));
        assert!(json.contains("\"kind\":\"LlmError\""));
        assert!(json.contains("\"retryable\":true"));
        assert!(json.contains("\"requestId\":null"));
    }

    // ---- ErrorCategory ↔ LlmErrorCategory 1:1 ----
    #[test]
    fn error_category_from_llm_category() {
        assert_eq!(
            ErrorCategory::from(LlmErrorCategory::Auth),
            ErrorCategory::Auth
        );
        assert_eq!(
            ErrorCategory::from(LlmErrorCategory::Network),
            ErrorCategory::Network
        );
    }
}
