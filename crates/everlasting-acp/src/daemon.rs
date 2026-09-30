//! daemon HTTP 客户端封装 + payload 归一层(design §5,缺口 5)。
//!
//! everlasting daemon 的 wire 命名不对称(实证,勿"统一"):
//! - `GET /api/v1/health` 响应体是 **camelCase**(`app/src-tauri/src/daemon/routes/health.rs:57-73`,
//!   `#[serde(rename_all = "camelCase")]`);
//! - 域端点(projects / sessions / permissions)的请求与响应体都是 **snake_case**
//!   (`routes/projects.rs` / `routes/sessions.rs` / `routes/permissions.rs`,行类型
//!   `ProjectRow` / `SessionRow` 注释明确保留 snake_case)。
//!
//! 本模块的 DTO 就是归一层:边界两侧字段名各自锚死(单测锁 casing),
//! shim 内部一律用本模块类型,不再触碰原始 JSON。
//!
//! 请求超时:域端点 10s 短超时(design §5);SSE 长连接走同结构体的第二个
//! 免超时客户端(reqwest per-request timeout 只能设值不能取消)。

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 守护进程默认地址(与 GUI/evl 同一 daemon)。
pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:7456";

/// daemon 域端点的统一超时。chat 受理 POST 与本层其余调用同为短超时
/// (design §5);PR2 的 SSE 流不经过这里。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// shim 侧错误模型(design §6 的 PR1 子集;`SseClosed` 随 PR2 增补)。
#[derive(Debug, Error)]
pub enum DaemonError {
    /// 连不上 daemon(连接拒绝 / 超时 / TLS 初始化失败)。
    /// `initialize` 把它转成带 `daemon.sh` 启动指引的 JSON-RPC error。
    #[error("daemon unreachable at {url}: {source}")]
    DaemonUnreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// daemon 返回了非 2xx(status + 原始 body,诊断用)。
    #[error("daemon API error (HTTP {status}): {body}")]
    DaemonApi { status: u16, body: String },
    /// daemon 返回了不符合归一层 DTO 的载荷(字段缺失/类型不符)。
    #[error("malformed daemon payload: {0}")]
    Protocol(#[from] serde_json::Error),
}

/// daemon HTTP 客户端。零系统库依赖(rustls);域端点走 10s 短超时客户端,
/// SSE 长连接走独立免超时客户端(reqwest 的 per-request timeout 只能设值
/// 不能取消,双客户端是官方形状下的唯一解法)。
#[derive(Clone)]
pub struct DaemonClient {
    http: reqwest::Client,
    stream: reqwest::Client,
    base_url: String,
}

impl DaemonClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, DaemonError> {
        let base_url = base_url.into();
        // daemon 是本机零鉴权服务(取舍 8):环境代理绝不该经手。
        // 实证(shell 带 http_proxy 时):reqwest 的 NO_PROXY 匹配不认
        // `127.*` 这类通配写法,回环地址会被送进代理 → 502 空响应,
        // 症状是"daemon 明明在跑却连不上"。整客户端禁用代理,行为确定。
        let base = |timeout: Option<Duration>| {
            let mut b = reqwest::Client::builder().no_proxy();
            if let Some(t) = timeout {
                b = b.timeout(t);
            }
            b.build().map_err(|source| DaemonError::DaemonUnreachable {
                url: base_url.clone(),
                source,
            })
        };
        let http = base(Some(REQUEST_TIMEOUT))?;
        let stream = base(None)?;
        Ok(Self {
            http,
            stream,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn send<J: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<J, DaemonError> {
        let url = format!("{}{}", self.base_url, path);
        let request = match body {
            Some(body) => self.http.request(method, &url).json(body),
            None => self.http.request(method, &url),
        };
        let resp = request
            .send()
            .await
            .map_err(|source| DaemonError::DaemonUnreachable {
                url: self.base_url.clone(),
                source,
            })?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|source| DaemonError::DaemonUnreachable {
                url: self.base_url.clone(),
                source,
            })?;
        if !status.is_success() {
            return Err(DaemonError::DaemonApi {
                status: status.as_u16(),
                body: text,
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn get<J: serde::de::DeserializeOwned>(&self, path: &str) -> Result<J, DaemonError> {
        self.send(reqwest::Method::GET, path, None::<&()>).await
    }

    async fn post<J: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<J, DaemonError> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    /// `GET /api/v1/health`。失败路径(含指引)由 handler 层转 JSON-RPC error。
    pub async fn health(&self) -> Result<HealthResponse, DaemonError> {
        self.get("/api/v1/health").await
    }

    /// `GET /api/v1/stream` 请求构造器(SSE 长连接)。走免超时客户端
    /// (design §5:流无超时,daemon 30s `:ping` 天然保活);`Last-Event-ID`
    /// 头由 sse.rs 挂。域端点走另一个 10s 短超时客户端。
    pub fn stream_request(&self) -> reqwest::RequestBuilder {
        self.stream.get(format!("{}/api/v1/stream", self.base_url))
    }

    /// `POST /api/v1/agent/chat` 受理(单条 user 文本消息 —— ACP prompt 的
    /// text-only 基线,evl `cli/lib/chat.mjs:303` 同形状)。响应是 fire-and-
    /// forget 受理结果 [`ChatAcceptance`](`agent/chat.rs:296-306`,serde
    /// `tag = "status"`);turn 终态从 SSE 读,不走本响应。
    pub async fn agent_chat(
        &self,
        request_id: &str,
        session_id: &str,
        text: &str,
    ) -> Result<ChatAcceptance, DaemonError> {
        let body = AgentChatRequestDto {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            messages: vec![AgentChatMessageDto {
                role: "user",
                content: text.to_string(),
            }],
        };
        self.post("/api/v1/agent/chat", &body).await
    }

    /// `POST /api/v1/permissions/permission_response {rid, decision}`
    /// → `{resolved: bool}`(`routes/permissions.rs:230-246`)。`false` =
    /// rid 未知/已超时/重复应答 —— daemon 语义是 benign no-op,调用方记
    /// 日志**不重试**。decision 值域 `allow_once|allow_always|deny`。
    pub async fn permission_response(
        &self,
        rid: &str,
        decision: &str,
    ) -> Result<bool, DaemonError> {
        let body = PermissionResponseRequestDto {
            rid: rid.to_string(),
            decision: decision.to_string(),
        };
        self.post("/api/v1/permissions/permission_response", &body)
            .await
    }

    /// `POST /api/v1/cancel/cancel_chat {request_id}` → `{cancelled,
    /// cleared_queued}`(`routes/cancel.rs:23-31`)。rid 不存在时
    /// `cancelled: false`,daemon 侧静默 no-op —— 调用方同款 no-op。
    pub async fn cancel_chat(&self, request_id: &str) -> Result<CancelOutcomeDto, DaemonError> {
        let body = CancelChatRequestDto {
            request_id: request_id.to_string(),
        };
        self.post("/api/v1/cancel/cancel_chat", &body).await
    }

    /// `POST /api/v1/projects/list_projects`,带 `filter:{hidden:true}` 查全量
    /// (evl `cli/lib/chat.mjs:114-131` 同款:hidden 项目不在默认列表,
    /// 但 create_project 唯一性检查查全表,漏掉会撞唯一性冲突)。
    pub async fn list_projects(&self) -> Result<Vec<ProjectRow>, DaemonError> {
        let body = ListProjectsRequest {
            filter: ListProjectsFilter { hidden: true },
        };
        self.post("/api/v1/projects/list_projects", &body).await
    }

    /// `POST /api/v1/projects/create_project {path}`。
    pub async fn create_project(&self, path: &str) -> Result<ProjectRow, DaemonError> {
        let body = CreateProjectRequest {
            path: path.to_string(),
        };
        self.post("/api/v1/projects/create_project", &body).await
    }

    /// project path 解析:词规整比对(不解析符号链接,evl `pickProjectByPath`
    /// 同款),命中返回既有行;未命中建新 project(daemon 无按 path 查找端点,
    /// 调研报告 §2.5)。daemon 对重复 path 的 create_project 报唯一性错误,
    /// 因此列表必须带 hidden 过滤(见 [`Self::list_projects`])。
    pub async fn resolve_project(&self, cwd: &Path) -> Result<ProjectResolution, DaemonError> {
        let want = normalize_path_lexical(cwd);
        let projects = self.list_projects().await?;
        if let Some(project) = projects
            .iter()
            .find(|p| normalize_path_lexical(Path::new(&p.path)) == want)
        {
            return Ok(ProjectResolution {
                project: project.clone(),
                created: false,
            });
        }
        let project = self.create_project(&want.to_string_lossy()).await?;
        Ok(ProjectResolution {
            project,
            created: true,
        })
    }

    /// `POST /api/v1/sessions/list_sessions {project_id}`(按项目过滤)。
    pub async fn list_sessions(
        &self,
        project_id: &str,
    ) -> Result<Vec<SessionSummary>, DaemonError> {
        let body = ListSessionsRequest {
            project_id: project_id.to_string(),
        };
        self.post("/api/v1/sessions/list_sessions", &body).await
    }

    /// `POST /api/v1/sessions/create_session`。无 mode 字段
    /// (`routes/sessions.rs:50-60`)——默认 mode 由随后的
    /// [`Self::set_session_mode`] 显式锚定。
    pub async fn create_session(
        &self,
        project_id: &str,
        initial_cwd: &str,
    ) -> Result<SessionRow, DaemonError> {
        let body = CreateSessionRequest {
            project_id: project_id.to_string(),
            initial_cwd: initial_cwd.to_string(),
            model: None,
            session_type: None,
            metadata: None,
        };
        self.post("/api/v1/sessions/create_session", &body).await
    }

    /// `POST /api/v1/sessions/load_session {session_id}` → 会话 + 全量 messages。
    /// PR1 只用于 `session/load` 的存在性校验与登记;messages 的类型化与
    /// update 重放序列是 PR4 范围(implement.md),先以原始 JSON 占位。
    pub async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<LoadedSession>, DaemonError> {
        let body = LoadSessionRequest {
            session_id: session_id.to_string(),
        };
        self.post("/api/v1/sessions/load_session", &body).await
    }

    /// `POST /api/v1/permissions/set_session_mode {session_id, mode}`,
    /// 值域 `edit|plan|yolo`(未知值 daemon 静默回退 edit)。
    pub async fn set_session_mode(
        &self,
        session_id: &str,
        mode: &str,
    ) -> Result<SessionRow, DaemonError> {
        let body = SetSessionModeRequest {
            session_id: session_id.to_string(),
            mode: mode.to_string(),
        };
        self.post("/api/v1/permissions/set_session_mode", &body)
            .await
    }
}

/// [`DaemonClient::resolve_project`] 的结果。
#[derive(Debug, Clone)]
pub struct ProjectResolution {
    pub project: ProjectRow,
    /// true = 本次调用新建了 project(list 未命中)。
    pub created: bool,
}

// ---------------------------------------------------------------------------
// 词法规整(evl pickProjectByPath 同款:absolutize + 折叠 `.`/`..`,不解析
// 符号链接、不要求路径存在)。比对双方都过这里,`/repo/foo` 与 `/repo/foobar`
// 不会误判相等(spec backend/project-cwd-boundary.md 的前缀陷阱;shim 只做
// 相等比对不做包含判定,不引入 canonicalize——daemon 侧存的就是词法 path)。
// ---------------------------------------------------------------------------

pub fn normalize_path_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            // cwd 不可得时保持原样(比对时双方对称处理,结果一致地不等)
            Err(_) => path.to_path_buf(),
        }
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// DTO 归一层。只声明 shim 消费的字段(daemon 侧新增字段经 serde 默认忽略,
// wire 是 additive 的,见 spec daemon-server.md SessionSummary enrich 条);
// casing 各自锚死,单测锁形(缺口 5)。
// ---------------------------------------------------------------------------

/// `GET /api/v1/health` 响应 —— daemon 侧 **camelCase**(`routes/health.rs:57-73`)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub daemon_id: String,
    pub daemon_version: String,
    pub api_versions: Vec<String>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListProjectsFilter {
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListProjectsRequest {
    pub filter: ListProjectsFilter,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateProjectRequest {
    pub path: String,
}

/// `projects` 行(snake_case,`app/src-tauri/src/projects/types.rs:14-39`)。
/// 只取 resolution / 展示所需字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListSessionsRequest {
    pub project_id: String,
}

/// `sessions` 概要行(snake_case,`db::SessionSummary`,spec daemon-server.md
/// SessionSummary enrich 条:wire 是 additive,`busy` 等新字段被本 DTO 忽略)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub current_cwd: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateSessionRequest {
    pub project_id: String,
    pub initial_cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// `sessions` 行(snake_case,`db::SessionRow`,`mode` 序列化为小写字符串)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub current_cwd: String,
    /// `edit|plan|yolo`(daemon `Mode` lowercase;历史脏值 daemon 侧已回填,
    /// 这里 Option 容忍以稳)。
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadSessionRequest {
    pub session_id: String,
}

/// `load_session` 返回(`db::LoadedSession`)。messages 的类型化在 PR4
/// (implement.md PR4「session/load 重放实现」);PR1 只消费 `session`。
#[derive(Debug, Clone, Deserialize)]
pub struct LoadedSession {
    pub session: SessionRow,
    /// 逐行 `{role, content, ...}`(`db::MessageRow` 的消费子集);
    /// `session/load` 重放按 role+content 翻译,其余字段忽略。
    pub messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetSessionModeRequest {
    pub session_id: String,
    pub mode: String,
}

/// `permission_response` 请求体(snake_case,`routes/permissions.rs:236-240`;
/// `reason` shim 不带 —— 审批语义已在 optionId 里)。
#[derive(Debug, Clone, Serialize)]
pub struct PermissionResponseRequestDto {
    pub rid: String,
    pub decision: String,
}

/// `cancel_chat` 请求体(snake_case,`routes/cancel.rs:19-21`)。
#[derive(Debug, Clone, Serialize)]
pub struct CancelChatRequestDto {
    pub request_id: String,
}

/// `cancel_chat` 返回(`commands/cancel.rs:31-34`)。`cancelled: false` =
/// rid 未知/已结束,daemon 静默 no-op。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct CancelOutcomeDto {
    pub cancelled: bool,
    pub cleared_queued: u64,
}

/// `POST /api/v1/agent/chat` 请求体(`routes/agent.rs:31-42`,snake_case;
/// `messages` 由客户端构造,evl 只发单条 user)。`MessageContent::Text`
/// 序列化为裸字符串,`role` 小写 —— evl 同款 wire 形状。
#[derive(Debug, Clone, Serialize)]
pub struct AgentChatRequestDto {
    pub request_id: String,
    pub session_id: String,
    pub messages: Vec<AgentChatMessageDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentChatMessageDto {
    pub role: &'static str,
    pub content: String,
}

/// chat 受理结果(`agent/chat.rs:294-306`,serde `tag = "status"`,
/// rename_all camelCase → `{"status":"started"}` / `{"status":"queued",...}` /
/// `{"status":"injected"}`)。`queued`/`injected` 由调用方拒绝
/// (取舍 3:ACP 会话不排队、不驱动群聊)。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ChatAcceptance {
    Started,
    Queued { id: String, position: u64 },
    Injected,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// casing 锚(缺口 5):health 是 daemon wire 面上唯一的 camelCase 域。
    #[test]
    fn health_response_serializes_camel_case() {
        let resp = HealthResponse {
            daemon_id: "d1".into(),
            daemon_version: "0.1.0".into(),
            api_versions: vec!["v1".into()],
            uptime_seconds: 7,
        };
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert!(v.get("daemonId").is_some(), "{v}");
        assert!(v.get("daemonVersion").is_some(), "{v}");
        assert!(v.get("apiVersions").is_some(), "{v}");
        assert!(v.get("uptimeSeconds").is_some(), "{v}");
        assert!(v.get("daemon_id").is_none(), "snake_case 泄漏: {v}");
    }

    /// casing 锚:list_projects 请求体精确等于 evl 发的形状
    /// (`cli/lib/chat.mjs:117-121`)。
    #[test]
    fn list_projects_request_body_matches_evl_shape() {
        let body = ListProjectsRequest {
            filter: ListProjectsFilter { hidden: true },
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v, serde_json::json!({ "filter": { "hidden": true } }));
    }

    /// casing 锚:create_session 请求体 = {project_id, initial_cwd},
    /// None 字段不下线(snake_case,`routes/sessions.rs:50-60`)。
    #[test]
    fn create_session_request_body_is_snake_case_and_skips_none() {
        let body = CreateSessionRequest {
            project_id: "p1".into(),
            initial_cwd: "/tmp/x".into(),
            model: None,
            session_type: None,
            metadata: None,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "project_id": "p1", "initial_cwd": "/tmp/x" })
        );
    }

    /// 词法规整:折叠 `.`/`..`,不触碰文件系统(不要求存在)。
    #[test]
    fn normalize_path_lexical_collapses_dot_components() {
        let p = normalize_path_lexical(Path::new("/repo/./sub/../other/x"));
        assert_eq!(p, PathBuf::from("/repo/other/x"));
    }

    /// 前缀陷阱(spec project-cwd-boundary.md §2 case 4):词法整路径相等比对,
    /// `/repo/foo` 不匹配 `/repo/foobar`。
    #[test]
    fn normalize_path_lexical_keeps_foobar_distinct_from_foo() {
        assert_ne!(
            normalize_path_lexical(Path::new("/repo/foobar")),
            normalize_path_lexical(Path::new("/repo/foo"))
        );
    }

    /// 健康检查不可达路径:指向确定无人监听的端口 → DaemonUnreachable。
    #[tokio::test]
    async fn health_reports_unreachable_as_daemon_unreachable() {
        // 先 bind 后 drop,拿到一个此刻必然空闲的端口,避开写死端口的偶发占用。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let client = DaemonClient::new(format!("http://127.0.0.1:{port}")).expect("client builds");
        let err = client.health().await.expect_err("port is closed");
        assert!(
            matches!(err, DaemonError::DaemonUnreachable { .. }),
            "{err:?}"
        );
    }

    /// project 解析命中:列表里有词规整后相等的 path → 直接返回既有 id,
    /// 不触发 create_project(后者挂 500,一旦被调用本测试即失败)。
    #[tokio::test]
    async fn resolve_project_hits_existing_row_without_creating() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/projects/list_projects"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "filter": { "hidden": true }
            })))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    { "id": "p-existing", "name": "repo", "path": "/tmp/acp-repo" }
                ])),
            )
            .mount(&server)
            .await;
        // create_project 一旦被调用即 500 → resolve 失败 → 测试红。
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/projects/create_project"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = DaemonClient::new(server.uri()).unwrap();
        let hit = client
            .resolve_project(Path::new("/tmp/acp-repo/./"))
            .await
            .unwrap();
        assert!(!hit.created);
        assert_eq!(hit.project.id, "p-existing");
    }

    /// project 解析未命中:先 list(空)后 create,返回新行且 created=true。
    #[tokio::test]
    async fn resolve_project_miss_creates_project() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/projects/list_projects"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    // 故意放一个前缀陷阱邻居:词法整路径比对不得误命中
                    { "id": "p-prefix", "name": "repo-foo", "path": "/tmp/acp-repo-foo" }
                ])),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/projects/create_project"))
            .and(wiremock::matchers::body_json(
                serde_json::json!({ "path": "/tmp/acp-repo" }),
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "p-new", "name": "acp-repo", "path": "/tmp/acp-repo"
                })),
            )
            .mount(&server)
            .await;

        let client = DaemonClient::new(server.uri()).unwrap();
        let hit = client
            .resolve_project(Path::new("/tmp/acp-repo"))
            .await
            .unwrap();
        assert!(hit.created);
        assert_eq!(hit.project.id, "p-new");
    }

    /// daemon 5xx → DaemonApi(status + body 透传,design §6)。
    #[tokio::test]
    async fn non_2xx_maps_to_daemon_api_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/projects/list_projects"))
            .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let client = DaemonClient::new(server.uri()).unwrap();
        let err = client.list_projects().await.expect_err("500");
        match err {
            DaemonError::DaemonApi { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body, "boom");
            }
            other => panic!("expected DaemonApi, got {other:?}"),
        }
    }

    /// casing 锚:agent_chat 请求体精确 = evl 形状(`chat.mjs:303`:
    /// 单条 user + 裸字符串 content),受理解析三种 status。
    #[test]
    fn agent_chat_request_body_matches_evl_shape() {
        let body = AgentChatRequestDto {
            request_id: "acp-1".into(),
            session_id: "s1".into(),
            messages: vec![AgentChatMessageDto {
                role: "user",
                content: "hello".into(),
            }],
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "request_id": "acp-1",
                "session_id": "s1",
                "messages": [{ "role": "user", "content": "hello" }]
            })
        );
    }

    /// ChatAcceptance 三种 wire 形状解析(`agent/chat.rs:294-306`)。
    #[test]
    fn chat_acceptance_parses_all_three_statuses() {
        let a: ChatAcceptance = serde_json::from_str(r#"{"status":"started"}"#).unwrap();
        assert_eq!(a, ChatAcceptance::Started);
        let a: ChatAcceptance =
            serde_json::from_str(r#"{"status":"queued","id":"q1","position":3}"#).unwrap();
        assert_eq!(
            a,
            ChatAcceptance::Queued {
                id: "q1".into(),
                position: 3
            }
        );
        let a: ChatAcceptance = serde_json::from_str(r#"{"status":"injected"}"#).unwrap();
        assert_eq!(a, ChatAcceptance::Injected);
    }

    /// agent_chat 走线:wiremock 回 `{"status":"started"}` → Started。
    #[tokio::test]
    async fn agent_chat_round_trips_started_acceptance() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/v1/agent/chat"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "request_id": "acp-1",
                "session_id": "s1",
                "messages": [{ "role": "user", "content": "hi" }]
            })))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "started" })),
            )
            .mount(&server)
            .await;
        let client = DaemonClient::new(server.uri()).unwrap();
        let acceptance = client.agent_chat("acp-1", "s1", "hi").await.unwrap();
        assert_eq!(acceptance, ChatAcceptance::Started);
    }
}
