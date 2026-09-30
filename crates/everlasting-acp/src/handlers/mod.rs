//! ACP 方法 handler 装配 + session 表(design §3 handlers/)。
//!
//! PR2 起接入 prompt 全链:`session/prompt` 经 `ConnectionTo::spawn` 出事件
//! 循环跑完整 turn(否则串行事件循环会卡住 `session/cancel` 通知 —— PR3 的
//! cancel 必须在 turn 进行中被处理,design §4「反向请求并发」)。
//! 未知方法由 SDK 收尾自动回 JSON-RPC method-not-found,无需自写兜底。

pub mod initialize;
pub mod session;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AuthenticateRequest, CancelNotification, InitializeRequest, ListSessionsRequest,
    LoadSessionRequest, NewSessionRequest, PromptRequest, SessionMode, SessionModeState,
    SetSessionModeRequest,
};
use agent_client_protocol::{Agent, Stdio};

use crate::daemon::DaemonClient;
use crate::sse::SseHandle;

/// session/new 显式锚定的默认 mode(与 daemon 会话默认一致;PR3 的
/// set_mode/current_mode_update 才会改它)。
pub(crate) const DEFAULT_MODE: &str = "edit";

/// 三档 mode 的 ACP 声明(与 daemon `set_session_mode` 值域 edit|plan|yolo
/// 一一对应;`session/new` 与 `session/load` 共用)。
pub(crate) fn mode_state(current: &str) -> SessionModeState {
    SessionModeState::new(
        current.to_string(),
        vec![
            SessionMode::new("edit", "Edit")
                .description("Full tool access; risky tools ask via the editor"),
            SessionMode::new("plan", "Plan")
                .description("Read-only planning; write tools are blocked"),
            SessionMode::new("yolo", "Yolo")
                .description("Full tool access without permission asks"),
        ],
    )
}

/// 单个 ACP session 在 shim 侧的登记。ACP sessionId 直接复用 daemon
/// session_id(1:1,取舍 7);值目前就是「已注册」标记(prompt/cancel/set_mode
/// 的前置校验)。随行元数据(如 project_id)有真实消费者时再加回,不预留
/// 死字段。
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub daemon_session_id: String,
}

/// ACP sessionId → SessionEntry + 当前在途 rid。临界区都是纯内存读写,
/// 持锁不跨 await,std Mutex 足够(表操作是常数时间)。
#[derive(Default)]
pub struct SessionTable {
    sessions: Mutex<HashMap<String, SessionEntry>>,
    /// sid → 当前在途 turn 的 rid。prompt 受理成功后写入、终态路径清除;
    /// `session/cancel` 据此寻址 cancel_chat。
    active: Mutex<HashMap<String, String>>,
}

impl SessionTable {
    pub fn register(&self, entry: SessionEntry) {
        self.sessions
            .lock()
            .expect("session table lock poisoned")
            .insert(entry.daemon_session_id.clone(), entry);
    }

    pub fn get(&self, session_id: &str) -> Option<SessionEntry> {
        self.sessions
            .lock()
            .expect("session table lock poisoned")
            .get(session_id)
            .cloned()
    }

    /// 标记在途 turn(受理成功后调用;session 未注册则忽略 —— 注册在
    /// prompt 前置校验已保证,这里是防御)。
    pub fn set_active_request(&self, session_id: &str, rid: String) {
        self.active
            .lock()
            .expect("active map lock poisoned")
            .insert(session_id.to_string(), rid);
    }

    /// 清除在途标记(prompt 终态/错误路径统一调用)。
    pub fn clear_active_request(&self, session_id: &str) {
        self.active
            .lock()
            .expect("active map lock poisoned")
            .remove(session_id);
    }

    /// cancel 寻址:无在途 turn 返回 None(通知侧静默 no-op)。
    pub fn active_request_id(&self, session_id: &str) -> Option<String> {
        self.active
            .lock()
            .expect("active map lock poisoned")
            .get(session_id)
            .cloned()
    }
}

/// 全局 handler 状态:daemon 客户端 + session 表 + SSE 任务句柄。
pub struct App {
    pub daemon: DaemonClient,
    pub sessions: SessionTable,
    pub sse: SseHandle,
}

impl App {
    pub fn new(daemon: DaemonClient, sse: SseHandle) -> Self {
        Self {
            daemon,
            sessions: SessionTable::default(),
            sse,
        }
    }
}

/// `DaemonError` → JSON-RPC error。daemon 不可达时信息带可执行的启动指引
/// (spec RULE-ERR-SURFACE-001:错误提示只指向真实存在的动作;
/// `scripts/daemon.sh start` 是 daemon.sh:10 的真实用法)。
pub(crate) fn daemon_error_to_acp(err: crate::daemon::DaemonError) -> agent_client_protocol::Error {
    let mut e = agent_client_protocol::Error::internal_error();
    match err {
        crate::daemon::DaemonError::DaemonUnreachable { url, source } => {
            e.message = format!(
                "everlasting daemon not reachable at {url} ({source}) — start it with \
                 `./scripts/daemon.sh start` from the everlasting repo, or point \
                 EVERLASTING_ACP_DAEMON_URL at a running daemon, then reconnect this agent"
            );
        }
        crate::daemon::DaemonError::DaemonApi { status, body } => {
            e.message = format!("everlasting daemon returned HTTP {status}");
            e.data = Some(serde_json::Value::String(body));
        }
        crate::daemon::DaemonError::Protocol(source) => {
            e.message = "everlasting daemon returned a malformed payload".to_string();
            e.data = Some(serde_json::Value::String(source.to_string()));
        }
    }
    e
}

/// 装配 agent 侧 Builder 并在 stdio 上 serve(main 的唯一出口)。
///
/// handler 都在连接事件循环内串行执行(SDK ordering 语义);短生命周期调用
/// (initialize / session 管理是亚秒级 HTTP)直 await;`session/prompt` 跨
/// 整个 turn,经 `cx.spawn` 离开事件循环(responder 随行,turn 结束时回包)。
pub async fn serve(app: App) -> agent_client_protocol::Result<()> {
    let app = Arc::new(app);
    Agent
        .builder()
        .name("everlasting-acp")
        .on_receive_request(
            async |req: InitializeRequest, responder, _cx| {
                responder.respond_with_result(initialize::initialize(&app, req).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: AuthenticateRequest, responder, _cx| {
                responder.respond_with_result(initialize::authenticate(req))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: agent_client_protocol::schema::v1::LogoutRequest, responder, _cx| {
                responder.respond_with_result(initialize::logout(req))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: NewSessionRequest, responder, _cx| {
                responder.respond_with_result(session::new_session(&app, req).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: PromptRequest, responder, cx| {
                // 整个 turn 不能占用事件循环:spawn 后 cancel 等通知才可能
                // 在 turn 进行中到达(PR3 依赖此形状)。
                let app = Arc::clone(&app);
                let conn = cx.clone();
                // spawn task 返回 Err 会关停整条连接;turn 内部错误已经
                // 走 respond_with_error 落到客户端,这里恒 Ok,不让单轮
                // 失败拖垮长存连接。
                cx.spawn(async move {
                    let result = session::prompt(&app, req, &conn, &conn).await;
                    // prompt 任何路径都必须最终 respond(PR2 硬约束);
                    // respond 失败 = 连接已在死亡中,记日志即可。
                    if let Err(err) = responder.respond_with_result(result) {
                        tracing::warn!(error = %err, "failed to deliver prompt response (connection closing?)");
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: SetSessionModeRequest, responder, cx| {
                responder.respond_with_result(session::set_mode(&app, req, &cx).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async |req: CancelNotification, cx| {
                // 通知,无响应;cancel 内部吞掉一切错误(全部降级为日志),
                // 通知 handler 返回 Err 会关停整条连接,不允许。
                // rid 在通知到达时同步读表(读序 = wire 到达序,不引入任务
                // 调度竞态),HTTP POST 则 spawn 出事件循环 —— cancel_chat
                // 失败路径最长 10s 超时,handler 内直 await 会把连接的消息
                // 处理(权限应答 / prompt 等)一并停摆(SDK:handler 运行在
                // 事件循环上,停摆期间新消息一律排队)。
                let app = Arc::clone(&app);
                let rid = app.sessions.active_request_id(req.session_id.0.as_ref());
                cx.spawn(async move {
                    session::cancel_rid(&app, req, rid).await;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async |req: LoadSessionRequest, responder, cx| {
                responder.respond_with_result(session::load_session(&app, req, &cx).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |req: ListSessionsRequest, responder, _cx| {
                responder.respond_with_result(session::list_sessions(&app, req).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // connect_to = server 模式:跑到 stdin EOF(Zed 关闭 agent 即退出),
        // 未挂 handler 的请求由 SDK 回 method-not-found,通知静默忽略。
        .connect_to(Stdio::new())
        .await
}
