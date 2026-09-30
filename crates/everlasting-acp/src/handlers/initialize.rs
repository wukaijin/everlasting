//! `initialize` / `authenticate` / `logout`(调研报告 §3 生命周期行)。
//!
//! 能力声明(R1,全是协议内合法降级):
//! - `loadSession = true`(重放序列本体是 PR4 范围,PR1 的 session/load 只做
//!   存在性校验 + 登记 + 回 modes);
//! - `promptCapabilities` 全 false(text-only,取舍 5);
//! - fs / terminal / mcp / elicitation 一律不声明 —— agent 自持工具承担,
//!   `AgentCapabilities` 默认值即关闭,零实现;
//! - `authMethods = []`(`InitializeResponse::new` 的默认),本机零鉴权(取舍 8)。

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, Implementation,
    InitializeRequest, InitializeResponse, LogoutRequest, LogoutResponse, SessionCapabilities,
    SessionListCapabilities,
};

use super::App;

/// `initialize`:健康检查(失败 → 结构化 error,不退进程)+ 能力声明。
pub async fn initialize(
    app: &App,
    req: InitializeRequest,
) -> Result<InitializeResponse, agent_client_protocol::Error> {
    // 握手时复查一次:main 里的启动期探测只管 stderr 日志,结构化错误必须落在
    // initialize 的响应上(implement.md PR1:健康失败不 panic 退出,仍进入 serve)。
    let health = app
        .daemon
        .health()
        .await
        .map_err(super::daemon_error_to_acp)?;
    tracing::info!(
        daemon_url = app.daemon.base_url(),
        daemon_id = %health.daemon_id,
        daemon_version = %health.daemon_version,
        client = ?req.client_info.as_ref().map(|c| c.name.as_str()),
        "initialize: daemon reachable"
    );

    Ok(InitializeResponse::new(req.protocol_version)
        .agent_capabilities(
            AgentCapabilities::new()
                // promptCapabilities 全 false:PromptCapabilities::default() 不动。
                .load_session(true)
                .session_capabilities(
                    SessionCapabilities::new().list(SessionListCapabilities::new()),
                ),
        )
        .agent_info(Implementation::new(
            "everlasting-acp",
            env!("CARGO_PKG_VERSION"),
        )))
}

/// `authenticate`:本机零鉴权,恒成功(客户端在 authMethods=[] 下本不应调用;
/// 恒 Ok 使其无害)。
pub fn authenticate(
    _req: AuthenticateRequest,
) -> Result<AuthenticateResponse, agent_client_protocol::Error> {
    Ok(AuthenticateResponse::new())
}

/// `logout`:无鉴权可注销,恒成功。
pub fn logout(_req: LogoutRequest) -> Result<LogoutResponse, agent_client_protocol::Error> {
    Ok(LogoutResponse::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonClient;
    use agent_client_protocol::schema::ProtocolVersion;
    use wiremock::ResponseTemplate;

    fn app_at(uri: String) -> App {
        // SSE 句柄指向死端口:本文件用例不驱动流,健康检查走 daemon 客户端。
        App::new(
            DaemonClient::new(uri).unwrap(),
            crate::sse::SseHandle::spawn(DaemonClient::new("http://127.0.0.1:1").unwrap()),
        )
    }

    /// 能力声明锚:loadSession=true、promptCapabilities 全 false、
    /// authMethods=[]、session.list 声明。
    #[tokio::test]
    async fn initialize_advertises_lifecycle_capabilities() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "daemonId": "d1",
                "daemonVersion": "0.1.0",
                "apiVersions": ["v1"],
                "uptimeSeconds": 1
            })))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        let resp = initialize(&app, InitializeRequest::new(ProtocolVersion::V1))
            .await
            .unwrap();

        assert_eq!(resp.protocol_version, ProtocolVersion::V1);
        let caps = &resp.agent_capabilities;
        assert!(caps.load_session, "loadSession must be advertised");
        // promptCapabilities 全 false(text-only 基线)。
        assert!(!caps.prompt_capabilities.image);
        assert!(!caps.prompt_capabilities.audio);
        assert!(!caps.prompt_capabilities.embedded_context);
        // fs / terminal 在 v1 AgentCapabilities 无字段;mcp 不声明 = 默认空。
        assert!(caps.session_capabilities.list.is_some(), "session/list 门");
        assert!(caps.session_capabilities.delete.is_none());
        assert!(caps.session_capabilities.close.is_none());
        // authMethods=[]。
        assert!(resp.auth_methods.is_empty());
    }

    /// daemon 不可达 → 结构化 JSON-RPC error,信息含 daemon.sh 启动指引
    /// (implement.md PR1;进程不退出,连接继续供后续重试)。
    #[tokio::test]
    async fn initialize_reports_daemon_sh_guidance_when_unreachable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let app = app_at(format!("http://127.0.0.1:{port}"));
        let err = initialize(&app, InitializeRequest::new(ProtocolVersion::V1))
            .await
            .expect_err("daemon unreachable");
        assert!(err.message.contains("daemon.sh"), "guidance missing: {err}");
    }
}
