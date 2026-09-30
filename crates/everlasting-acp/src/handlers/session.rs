//! session 生命周期 handler:`session/new` / `session/prompt`(PR2 全链)/
//! `session/load` / `session/list`(`session/cancel` / `session/set_mode`
//! 在 PR3)。
//!
//! session/new 全链 = 调研报告 §3「session/new」行 + evl `resolveProjectId`
//! 同款:cwd 词规整 → list_projects 比对 → 缺则 create_project →
//! create_session → set_session_mode 显式锚默认 edit → 回 modes 三档。
//!
//! session/prompt(evl chat.mjs 消费循环的 Rust 翻版,design §4):
//! subscribe broadcast(先于 POST,RULE-SMOKE-001)→ `agent/chat` 受理
//! (非 `started` 拒绝,取舍 3)→ 按 request_id + session_id 过滤 →
//! translate → `session/update` 通知 → done/error → respond(StopReason)。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, ContentChunk, CurrentModeUpdate, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionId, SessionInfo, SessionNotification,
    SessionUpdate, SetSessionModeRequest, SetSessionModeResponse, TextContent, ToolCall,
    ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::Error;
use tokio::sync::broadcast;

use super::{daemon_error_to_acp, mode_state, App, DEFAULT_MODE};
use crate::daemon::{
    normalize_path_lexical, CancelOutcomeDto, ChatAcceptance, DaemonClient, SessionSummary,
};
use crate::permission::{resolve_ask, PermissionOutbound};
use crate::sse::{DaemonEvent, SseHandle};
use crate::translate::{
    normalize_mode, tool_kind, tool_title, translate_chat_event, translate_tool_call,
    translate_tool_result, Translated,
};

/// prompt 等待终态期间的 recv 空闲轮询间隔:只在事件静默超 1s 时查一次流
/// 健康(健康连接静默是工具执行常态,不打扰;失联连接据此进入 stale 判定)。
const TURN_IDLE_POLL: Duration = Duration::from_secs(1);

/// 流失联后的自愈等待上限:失联持续「HEALTHY_WINDOW(45s 判失联)+
/// 本值(30s)」仍无恢复即判定 turn 已死、respond error。须 ≥ 重连退避
/// 封顶(30s)——daemon 短暂重启的场景里,重连成功会经 replay/哨兵收束,
/// 不走本臂。
const TURN_STREAM_STALE: Duration = Duration::from_secs(30);

/// prompt 请求 id 前缀 + 进程内序号(evl `evl-` 同构;daemon 按 rid 路由
/// 事件,cancel 端点 PR3 也按它寻址)。
fn make_request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("acp-{ms:x}-{n:x}")
}

/// ACP prompt ContentBlock → daemon user 消息文本。text-only 基线
/// (promptCapabilities 全 false,取舍 5):Text 直拼;ResourceLink 按
/// 「基线必须支持」消化为文本标记(agent 可见引用,不访问客户端资源);
/// Image/Audio/Resource 未声明能力,客户端不应发送,拒绝并注明。
fn prompt_text(blocks: &[ContentBlock]) -> Result<String, Error> {
    let mut text = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text(t) => text.push_str(&t.text),
            ContentBlock::ResourceLink(link) => {
                text.push_str(&format!("\n[referenced resource: {}]\n", link.uri));
            }
            _ => {
                return Err(Error::invalid_params().data(
                    "session/prompt supports text only — image/audio/embedded-context \
                     capabilities are not advertised (text-only baseline)",
                ))
            }
        }
    }
    if text.trim().is_empty() {
        return Err(Error::invalid_params().data("session/prompt prompt is empty"));
    }
    Ok(text)
}

/// chat 受理检查:非 `started` 一律 JSON-RPC error(取舍 3:不排队)。
/// `queued` = daemon busy —— ACP 场景(单人编辑器)不排队直接拒绝,提示等
/// 当前 turn 结束;`injected` = 群聊注入,ACP 会话不驱动群聊。
pub(crate) fn ensure_started(acceptance: ChatAcceptance) -> Result<(), Error> {
    match acceptance {
        ChatAcceptance::Started => Ok(()),
        ChatAcceptance::Queued { position, .. } => Err(Error::invalid_request().data(format!(
            "daemon busy: the session has queued turns ahead (position {position}); \
             everlasting does not queue ACP prompts — wait for the current turn \
             to finish, then retry"
        ))),
        ChatAcceptance::Injected => Err(Error::invalid_request().data(
            "message injected into a running group-chat discussion; ACP sessions do \
             not drive group chats",
        )),
    }
}

/// update 通知出口。`ConnectionTo<Client>` 的实现即发 `session/update`
/// 通知;测试用 [`VecSink`] 捕获,消费循环因此可脱离 ACP 传输单测。
pub(crate) trait UpdateSink: Send + Sync {
    fn send_update(&self, notification: SessionNotification) -> Result<(), Error>;
}

impl UpdateSink for agent_client_protocol::ConnectionTo<agent_client_protocol::Client> {
    fn send_update(&self, notification: SessionNotification) -> Result<(), Error> {
        // ConnectionTo 是廉价 clone 的发送句柄,send_notification 入
        // outgoing 队列即返回(不阻塞事件循环)。
        self.send_notification(notification)
    }
}

/// 消费 broadcast 流直到 turn 终态。过滤规则(implement.md PR2):
/// chat-event 按 request_id + session_id,tool 事件两者同滤(daemon 侧
/// `ToolCallPayload` / `ToolResultPayload` 均带双 id,state.rs:770-794)。
/// 本 session 的 permission:ask → **并行 spawn 权限环**(PR3,design §4
/// 「反向请求并发」:等客户端应答是长等待,绝不能占住消费循环 —— done/tool
/// 流必须继续转发,审批与流式并行;turn 先结束无需 join,迟到应答由 daemon
/// `resolved:false` 收敛)。**任何路径都以 respond 收尾**:done/error → 终态;
/// daemon 重启哨兵 / 流失联超限 → error;SSE 通道死亡(Closed)→ error,
/// 不悬挂。
// 8 参:rx/双 id/通知出口/反向请求出口/daemon/SSE 句柄/stale 上限都是
// 显式依赖;存量同款豁免先例见 quality-guidelines(chat_inner RULE-ARGS-001)。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prompt_turn<S, O>(
    mut rx: broadcast::Receiver<DaemonEvent>,
    request_id: &str,
    session_id: &str,
    sink: &S,
    outbound: &O,
    daemon: &DaemonClient,
    sse: &SseHandle,
    stale_limit: Duration,
) -> Result<PromptResponse, Error>
where
    S: UpdateSink,
    O: PermissionOutbound + Clone + Send + Sync + 'static,
{
    loop {
        // 事件静默(工具执行期常态)也要盯流活性:recv 挂起最多 IDLE_POLL
        // 就回头查一次健康(健康连接零打扰;recv 本身 cancel-safe,超时
        // 不丢已就绪事件)。
        match tokio::time::timeout(TURN_IDLE_POLL, rx.recv()).await {
            Err(_elapsed) => {
                if sse.is_healthy() || sse.wait_healthy(stale_limit).await {
                    continue;
                }
                // 流失联持续到窗口过期 + stale_limit 仍无自愈:本机直连
                // daemon 的拓扑下等价于 daemon 宕机/重启,在途 turn 已死,
                // respond error 收束,不悬挂。
                let mut e = Error::internal_error();
                e.message = "daemon SSE stream lost while waiting for the turn to \
                             finish and did not recover — the daemon likely went \
                             down mid-turn; check it with `./scripts/daemon.sh \
                             status`, then retry the prompt"
                    .to_string();
                return Err(e);
            }
            Ok(item) => match item {
                Ok(DaemonEvent::ChatEvent {
                    request_id: rid,
                    session_id: sid,
                    event,
                }) if rid == request_id && sid == session_id => {
                    match translate_chat_event(session_id, event) {
                        Translated::Updates(updates) => {
                            for update in updates {
                                sink.send_update(update)?;
                            }
                        }
                        Translated::Terminal(reason) => return Ok(PromptResponse::new(reason)),
                        Translated::Ignored => {}
                    }
                }
                Ok(DaemonEvent::ToolCall(payload))
                    if payload.request_id == request_id && payload.session_id == session_id =>
                {
                    sink.send_update(translate_tool_call(payload))?;
                }
                Ok(DaemonEvent::ToolResult(payload))
                    if payload.request_id == request_id && payload.session_id == session_id =>
                {
                    sink.send_update(translate_tool_result(payload))?;
                }
                Ok(DaemonEvent::PermissionAsk(ask)) if ask.session_id == session_id => {
                    // 权限环并行 spawn,理由见本函数 doc(挂接层决策)。
                    // resolve_ask 吞掉自身一切错误(log only),spawn 任务
                    // 恒不返回 Err,不会炸连接。
                    let daemon = daemon.clone();
                    let outbound = outbound.clone();
                    tokio::spawn(async move {
                        resolve_ask(ask, &outbound, &daemon).await;
                    });
                }
                Ok(DaemonEvent::Resync { reason }) => {
                    // daemon 进程重启(restart,及未来新增的未知 reason)
                    // = 在途 turn 随进程死亡,done/error 永不再来 —— 立即
                    // respond error,不悬挂。唯一已知的「继续等」分支是
                    // buffer_overrun(ring 淘汰,daemon 仍活着,终态随后续
                    // 流到达;spec pattern-sse-resync-and-tests.md 决策表)。
                    if reason != "buffer_overrun" {
                        tracing::warn!(reason, "daemon restarted mid-turn; failing prompt");
                        let mut e = Error::internal_error();
                        e.message = format!(
                            "the daemon restarted while the turn was in flight \
                             (stream-resync reason: {reason}); the turn state was \
                             lost — retry the prompt"
                        );
                        return Err(e);
                    }
                    tracing::warn!(
                        reason,
                        "stream resync (buffer overrun) during prompt turn; waiting for terminal"
                    );
                }
                Ok(DaemonEvent::Other { event }) => {
                    tracing::debug!(event, "ignoring daemon event during prompt");
                }
                // 其余 = rid/session 不匹配的他人事件(GUI 侧并行 turn 等),
                // 静默跳过(上面的守卫臂已按本 rid+sid 消费过一遍)。
                Ok(_) => {}
                // Lagged = 消费方短暂落后丢了 n 条(文本流断章):继续等终态,
                // 「prompt 必须最终 respond」优先于完整性,丢帧本身已不可逆。
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    tracing::warn!(dropped, "daemon event stream lagged during prompt");
                }
                // Closed = SSE 任务退出(理论上不发生:重连循环永不退)或句柄
                // 已死。转 error 响应,绝不悬挂。
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(Error::internal_error()
                        .data("daemon event stream terminated before the turn completed"));
                }
            },
        }
    }
}

/// `session/prompt` 全链(SDK spawn 任务内运行,见 handlers/mod.rs 装配)。
/// `sink` 发 session/update 通知,`outbound` 发 session/request_permission
/// 反向请求;真实现两者同为 `ConnectionTo<Client>`(serve 传 &conn,&conn)。
pub async fn prompt<S, O>(
    app: &App,
    req: PromptRequest,
    sink: &S,
    outbound: &O,
) -> Result<PromptResponse, Error>
where
    S: UpdateSink,
    O: PermissionOutbound + Clone + Send + Sync + 'static,
{
    let session_id = req.session_id.0.to_string();
    // 只接受本 shim 建过/载过的 session(Zed 不会凭空 prompt)。
    if app.sessions.get(&session_id).is_none() {
        return Err(Error::invalid_params().data(format!(
            "unknown session {session_id} — create it with session/new or load it \
             with session/load first"
        )));
    }

    // ① 流健康(「启动即挂」的全局任务;窗口化判据 —— 秒级断线由 daemon
    //    Last-Event-ID replay 覆盖,长断线才拦,见 SseHandle::is_healthy)。
    if !app.sse.is_healthy() && !app.sse.wait_healthy(Duration::from_secs(5)).await {
        let mut e = Error::internal_error();
        e.message = "daemon SSE stream is unhealthy (no bytes from the daemon for \
                     a while) — the daemon may be down; check it with \
                     `./scripts/daemon.sh status`"
            .to_string();
        return Err(e);
    }
    // ② 订阅 broadcast(必须先于 POST —— 订阅只收之后的事件)。
    let rx = app.sse.subscribe();

    // ③ 受理 chat(非 started → JSON-RPC error,取舍 3)。
    let request_id = make_request_id();
    let text = prompt_text(&req.prompt)?;
    let acceptance = app
        .daemon
        .agent_chat(&request_id, &session_id, &text)
        .await
        .map_err(daemon_error_to_acp)?;
    ensure_started(acceptance)?;
    // 在途 rid 入表:session/cancel 通知据此寻址 cancel_chat。
    app.sessions
        .set_active_request(&session_id, request_id.clone());
    tracing::info!(request_id = %request_id, session_id = %session_id, "turn accepted, consuming stream");

    // ④ 消费到终态(Terminal 与 Err 都先清在途标记,再落 respond;
    // 流失联 / daemon 重启哨兵 → error respond,不悬挂)。
    let result = run_prompt_turn(
        rx,
        &request_id,
        &session_id,
        sink,
        outbound,
        &app.daemon,
        &app.sse,
        TURN_STREAM_STALE,
    )
    .await;
    app.sessions.clear_active_request(&session_id);
    result
}

/// `session/cancel`(通知,无响应)→ `cancel_chat {request_id}`。
/// - 寻址:session 表的在途 rid(prompt 受理时写入);无在途 turn / 未注册
///   session → 静默 no-op(daemon 对未知 rid 本就是 no-op,语义对称)。
/// - cancel 后 prompt 循环收 `done(cancelled)` → respond Cancelled(PR2
///   已落的终态路径);与在途 permission ask 并发不特判 —— 客户端若仍以
///   optionId 应答,permission_response 照发,daemon 侧 rid 失效返回
///   `resolved:false` 自然收敛(permission.rs 模块文档)。
/// - cancel_chat 失败(网络抖动等)→ warn 降级:turn 继续跑到终态,用户可
///   重发 cancel;通知 handler 不得向上传错(会关停连接)。
///
/// serve 装配(handler 侧)先同步读 rid、再 spawn [`cancel_rid`] 发 POST
/// (HTTP 不占事件循环,见 handlers/mod.rs);本入口是直调等价形状,仅测试用。
#[cfg(test)]
pub(crate) async fn cancel(app: &App, req: CancelNotification) {
    let rid = app.sessions.active_request_id(req.session_id.0.as_ref());
    cancel_rid(app, req, rid).await;
}

/// cancel 主体(rid 已在通知到达时解析)。
pub(crate) async fn cancel_rid(app: &App, req: CancelNotification, rid: Option<String>) {
    let session_id = req.session_id.0.to_string();
    let Some(rid) = rid else {
        tracing::debug!(
            session_id,
            "cancel for session without in-flight turn; no-op"
        );
        return;
    };
    match app.daemon.cancel_chat(&rid).await {
        Ok(CancelOutcomeDto {
            cancelled: true, ..
        }) => tracing::info!(session_id, request_id = %rid, "turn cancelled"),
        Ok(outcome) => tracing::debug!(
            session_id,
            request_id = %rid,
            cancelled = outcome.cancelled,
            "cancel_chat no-op (rid not in flight)"
        ),
        Err(err) => tracing::warn!(
            session_id,
            request_id = %rid,
            error = %err,
            "cancel_chat failed; turn continues, cancel can be re-sent"
        ),
    }
}

/// `session/set_mode` → daemon `set_session_mode`(值域 edit|plan|yolo)。
/// 未知值 shim 先经 [`normalize_mode`] 回退 edit —— 与 daemon lenient parse
/// 行为对齐(未知值 daemon 侧同样写 edit),双侧不会分叉。成功后向客户端发
/// `current_mode_update` 通知(Zed 据此刷新 mode 徽标),响应回 daemon 落库
/// 的实际 mode(权威值,防 shim/daemon 漂移)。
pub(crate) async fn set_mode(
    app: &App,
    req: SetSessionModeRequest,
    conn: &impl UpdateSink,
) -> Result<SetSessionModeResponse, Error> {
    let session_id = req.session_id.0.to_string();
    if app.sessions.get(&session_id).is_none() {
        return Err(Error::invalid_params().data(format!(
            "unknown session {session_id} — create or load it first"
        )));
    }
    let mode = normalize_mode(req.mode_id.0.as_ref());
    let row = app
        .daemon
        .set_session_mode(&session_id, mode)
        .await
        .map_err(daemon_error_to_acp)?;
    let actual = row.mode.clone().unwrap_or_else(|| mode.to_string());
    conn.send_update(SessionNotification::new(
        session_id,
        SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(actual)),
    ))?;
    Ok(SetSessionModeResponse::new())
}

/// 测试用通知捕集器:run_prompt_turn 的消费序列断言面。
#[cfg(test)]
pub(crate) struct VecSink(std::sync::Mutex<Vec<SessionNotification>>);

#[cfg(test)]
impl VecSink {
    fn new() -> Self {
        Self(std::sync::Mutex::new(Vec::new()))
    }

    fn notifications(&self) -> Vec<SessionNotification> {
        self.0.lock().expect("vec sink lock").clone()
    }
}

#[cfg(test)]
impl UpdateSink for VecSink {
    fn send_update(&self, notification: SessionNotification) -> Result<(), Error> {
        self.0.lock().expect("vec sink lock").push(notification);
        Ok(())
    }
}

/// `session/new {cwd}` → daemon project + session。
///
/// `additionalDirectories` / `mcpServers` 不声明能力即忽略(取舍 6,R3);
/// worktree 恒走 daemon 默认 none 态 —— 工具 cwd 直落 project.path,
/// 即 Zed 打开的工作区目录(取舍 4,调研 §2.5)。
pub async fn new_session(app: &App, req: NewSessionRequest) -> Result<NewSessionResponse, Error> {
    let cwd = normalize_path_lexical(&req.cwd);
    if !cwd.is_absolute() {
        return Err(Error::invalid_params().data(format!(
            "session cwd must be absolute, got {}",
            req.cwd.display()
        )));
    }

    let resolution = app
        .daemon
        .resolve_project(&cwd)
        .await
        .map_err(daemon_error_to_acp)?;
    tracing::info!(
        cwd = %cwd.display(),
        project_id = %resolution.project.id,
        created = resolution.created,
        "session/new: project resolved"
    );

    let row = app
        .daemon
        .create_session(&resolution.project.id, &cwd.to_string_lossy())
        .await
        .map_err(daemon_error_to_acp)?;
    // 默认 mode 显式锚定(set_session_mode 持久写 sessions.mode,与 evl 同款;
    // daemon 建会话默认也是 edit,这里多一次幂等写换取行为不依赖隐式默认)。
    let mode_row = app
        .daemon
        .set_session_mode(&row.id, DEFAULT_MODE)
        .await
        .map_err(daemon_error_to_acp)?;
    let mode = mode_row.mode.as_deref().unwrap_or(DEFAULT_MODE);

    app.sessions.register(super::SessionEntry {
        daemon_session_id: row.id.clone(),
    });

    Ok(NewSessionResponse::new(SessionId::new(row.id.as_str())).modes(mode_state(mode)))
}

/// `session/load`:取 daemon 全量 messages → **先发完** update 重放通知,
/// 最后 respond modes。时序以官方为准(agentclientprotocol.com
/// protocol/session-setup):agent MUST 把整段会话以 `session/update` 通知
/// 重放,"When all the conversation entries have been streamed to the Client,
/// the Agent MUST respond to the original session/load request" —— respond
/// 在全部重放之后,客户端在此之前不得继续 prompt。
///
/// cwd 策略 = **strict 报错**:官方把 request cwd 与 session cwd 一致作为
/// load 前置;不一致时重放的历史 tool_call 路径与新工作区错位、后续 prompt
/// 也会落在旧 project 下执行,违背工作区预期 —— 报 `invalid_params` 优于
/// 放行(daemon 侧无 cwd 概念,不会替我们兜底)。
pub async fn load_session(
    app: &App,
    req: LoadSessionRequest,
    conn: &impl UpdateSink,
) -> Result<LoadSessionResponse, Error> {
    let loaded = app
        .daemon
        .load_session(req.session_id.0.as_ref())
        .await
        .map_err(daemon_error_to_acp)?;
    let Some(loaded) = loaded else {
        return Err(Error::resource_not_found(Some(req.session_id.to_string())));
    };

    let session_cwd = normalize_path_lexical(Path::new(&loaded.session.current_cwd));
    if session_cwd != normalize_path_lexical(&req.cwd) {
        return Err(Error::invalid_params().data(format!(
            "session cwd mismatch: the session lives in \"{}\" but the request \
             cwd is \"{}\" — reopen the workspace at the session's directory \
             to load it",
            session_cwd.display(),
            req.cwd.display()
        )));
    }

    let mode = loaded.session.mode.as_deref().unwrap_or(DEFAULT_MODE);
    app.sessions.register(super::SessionEntry {
        daemon_session_id: loaded.session.id.clone(),
    });

    // 重放:全部 update 发完后才 respond(官方时序,见上)。
    let session_id = loaded.session.id.clone();
    let mut replayed = 0usize;
    for message in &loaded.messages {
        for update in replay_message(message) {
            conn.send_update(SessionNotification::new(
                SessionId::new(session_id.as_str()),
                update,
            ))?;
            replayed += 1;
        }
    }
    tracing::info!(session_id = %session_id, updates = replayed, "session/load replayed");

    Ok(LoadSessionResponse::new().modes(mode_state(mode)))
}

/// 单条历史消息 → update 通知序列(纯函数,重放主战场)。daemon wire:
/// 每行 `{role, content}`,content = 裸字符串(纯文本行)或 block 数组
/// (`llm/types/message.rs` ContentBlock,serde tag "type" snake_case)。
///
/// 映射(implement.md PR4 + 官方 "replay the entire conversation"):
/// - 文本按行角色:user → `user_message_chunk`,assistant →
///   `agent_message_chunk`(implement.md 字面只写 agent chunk;按官方整段
///   会话语义补 user 侧,否则 Zed 重开后丢用户气泡);
/// - `thinking` → `agent_thought_chunk`;`signature`/`redacted_thinking`
///   忽略(回传 LLM 用,客户端不可显示,与流式同口径);
/// - `tool_use` → `tool_call`(pending)紧跟 `tool_call_update`(completed +
///   rawInput)—— 历史 tool_use 必有配对 tool_result(daemon pair
///   atomicity),两帧让客户端呈现已完成的调用;
/// - `tool_result` → `tool_call_update`(completed/failed + rawOutput);
/// - usage:messages 不携带(daemon usage 在 turn_trace),无源不发。
fn replay_message(message: &serde_json::Value) -> Vec<SessionUpdate> {
    let mut updates = Vec::new();
    let Some(role) = message.get("role").and_then(|r| r.as_str()) else {
        return updates;
    };
    let chunk_for_role = |text: String| {
        if role == "user" {
            SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            )))
        } else {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(text),
            )))
        }
    };

    match message.get("content") {
        Some(serde_json::Value::String(text)) => {
            updates.push(chunk_for_role(text.clone()));
        }
        Some(serde_json::Value::Array(blocks)) => {
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            updates.push(chunk_for_role(text.to_string()));
                        }
                    }
                    Some("thinking") => {
                        if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
                            updates.push(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(text.to_string())),
                            )));
                        }
                    }
                    // signature/redacted:回传 LLM 用,重放面不消费。
                    Some("tool_use") => {
                        let (Some(id), Some(name)) = (
                            block.get("id").and_then(|v| v.as_str()),
                            block.get("name").and_then(|v| v.as_str()),
                        ) else {
                            continue;
                        };
                        let input = block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        updates.push(SessionUpdate::ToolCall(
                            ToolCall::new(ToolCallId::new(id), tool_title(name, &input))
                                .kind(tool_kind(name))
                                .raw_input(input.clone()),
                        ));
                        updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            ToolCallId::new(id),
                            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
                        )));
                    }
                    Some("tool_result") => {
                        let Some(tool_use_id) = block.get("tool_use_id").and_then(|v| v.as_str())
                        else {
                            continue;
                        };
                        let content = block
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            ToolCallId::new(tool_use_id),
                            ToolCallUpdateFields::new()
                                .status(if is_error {
                                    ToolCallStatus::Failed
                                } else {
                                    ToolCallStatus::Completed
                                })
                                .content(vec![ToolCallContent::from(ContentBlock::Text(
                                    TextContent::new(content.clone()),
                                ))])
                                .raw_output(serde_json::Value::String(content)),
                        )));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    updates
}

/// `session/list`:cwd 过滤 = 同 project 过滤(取舍 7);cwd 缺省时聚合全部
/// project。cursor 不支持(daemon list_sessions 无分页,恒返回全量,
/// next_cursor 缺省即"没有更多")。
pub async fn list_sessions(
    app: &App,
    req: ListSessionsRequest,
) -> Result<ListSessionsResponse, Error> {
    let infos = match &req.cwd {
        Some(cwd) => {
            let want = normalize_path_lexical(cwd);
            let projects = app
                .daemon
                .list_projects()
                .await
                .map_err(daemon_error_to_acp)?;
            let Some(project) = projects
                .iter()
                .find(|p| normalize_path_lexical(Path::new(&p.path)) == want)
            else {
                // 该目录从未注册过 project:空列表,不侧建(list 不得有副作用)。
                return Ok(ListSessionsResponse::new(Vec::new()));
            };
            to_infos(
                app.daemon
                    .list_sessions(&project.id)
                    .await
                    .map_err(daemon_error_to_acp)?,
            )
        }
        None => {
            let projects = app
                .daemon
                .list_projects()
                .await
                .map_err(daemon_error_to_acp)?;
            let mut infos = Vec::new();
            for project in &projects {
                let rows = app
                    .daemon
                    .list_sessions(&project.id)
                    .await
                    .map_err(daemon_error_to_acp)?;
                infos.extend(to_infos(rows));
            }
            infos
        }
    };
    Ok(ListSessionsResponse::new(infos))
}

fn to_infos(rows: Vec<SessionSummary>) -> Vec<SessionInfo> {
    rows.iter()
        .map(|s| {
            SessionInfo::new(
                SessionId::new(s.id.as_str()),
                normalize_path_lexical(Path::new(&s.current_cwd)),
            )
            .title(s.title.clone())
            .updated_at(s.updated_at.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonClient;
    use crate::permission::{Script, ScriptedOutbound};
    use crate::translate::{ChatEventDto, TokenUsageDto, ToolCallPayloadDto};
    use agent_client_protocol::schema::v1::{
        ContentBlock, ImageContent, SessionModeId, SessionUpdate, StopReason, TextContent,
        ToolCallStatus,
    };
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn app_at(uri: String) -> App {
        App::new(
            DaemonClient::new(uri).unwrap(),
            crate::sse::SseHandle::spawn(DaemonClient::new("http://127.0.0.1:1").unwrap()),
        )
    }

    /// 指向死端口的 SSE 句柄(恒不健康):run_prompt_turn 直测用 —— 事件
    /// 预载进 broadcast 时 recv 永不超时,stale 守卫不触发;stale 用例则
    /// 显式收短 stale_limit。
    fn dead_sse() -> SseHandle {
        SseHandle::spawn(DaemonClient::new("http://127.0.0.1:1").unwrap())
    }

    /// 指向死端口的 daemon 客户端:不触发权限环 POST 的用例用。
    fn dead_daemon() -> DaemonClient {
        DaemonClient::new("http://127.0.0.1:1").unwrap()
    }

    /// scoped permission_response mock(带可选 decision 精确匹配)+ guard。
    async fn permission_response_server(
        decision: Option<&str>,
        body: serde_json::Value,
    ) -> (MockServer, wiremock::MockGuard) {
        let server = MockServer::start().await;
        let mut mock =
            Mock::given(method("POST")).and(path("/api/v1/permissions/permission_response"));
        if let Some(d) = decision {
            mock = mock.and(body_json(serde_json::json!({ "rid": "r1", "decision": d })));
        }
        let guard = mock
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount_as_scoped(&server)
            .await;
        (server, guard)
    }

    /// 直测用的宽松 stale 上限(远超测试时长,守卫臂不触发)。
    fn no_stale() -> Duration {
        Duration::from_secs(600)
    }

    fn chat_event(rid: &str, sid: &str, event: ChatEventDto) -> DaemonEvent {
        DaemonEvent::ChatEvent {
            request_id: rid.into(),
            session_id: sid.into(),
            event,
        }
    }

    /// 文本流多帧 delta + tool 两态 → 全部转成 update 且顺序保持;
    /// done(end_turn)→ Terminal。混入他人 rid/sid 的事件必须被滤掉。
    #[tokio::test]
    async fn prompt_loop_filters_by_rid_and_streams_updates_until_done() {
        let (tx, rx) = broadcast::channel(16);
        let sink = VecSink::new();

        let producer = tokio::spawn(async move {
            // 他人 turn 的事件:必须被滤掉。
            tx.send(chat_event(
                "other-rid",
                "s1",
                ChatEventDto::Delta {
                    text: " alien".into(),
                },
            ))
            .unwrap();
            tx.send(chat_event(
                "r1",
                "s2",
                ChatEventDto::Delta {
                    text: " alien".into(),
                },
            ))
            .unwrap();
            // 本 turn:delta ×2 → tool_call → tool_result → done。
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::Delta { text: "He".into() },
            ))
            .unwrap();
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::Delta { text: "llo".into() },
            ))
            .unwrap();
            tx.send(DaemonEvent::ToolCall(ToolCallPayloadDto {
                request_id: "r1".into(),
                session_id: "s1".into(),
                id: "tu_1".into(),
                name: "shell".into(),
                input: serde_json::json!({ "command": "ls" }),
            }))
            .unwrap();
            tx.send(DaemonEvent::ToolResult(
                crate::translate::ToolResultPayloadDto {
                    request_id: "r1".into(),
                    session_id: "s1".into(),
                    tool_use_id: "tu_1".into(),
                    content: "ok".into(),
                    is_error: false,
                },
            ))
            .unwrap();
            // usage 通知(done 之前,Zed 惯例)。
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::TurnUsage {
                    request_id: "r1".into(),
                    seq: 0,
                    run_id: String::new(),
                    usage: TokenUsageDto {
                        input_tokens: 10,
                        output_tokens: 2,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        context_input_tokens: 12,
                    },
                    context_window: 1000,
                },
            ))
            .unwrap();
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::Done {
                    stop_reason: Some("end_turn".into()),
                    usage: None,
                },
            ))
            .unwrap();
            // done 之后的事件不得被消费(循环已退出)。
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::Delta {
                    text: "late".into(),
                },
            ))
            .unwrap();
        });

        let resp = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .unwrap();
        producer.await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);

        let updates = sink.notifications();
        // delta, delta, tool_call, tool_result, usage_update = 5 条。
        assert_eq!(updates.len(), 5, "alien 事件被滤掉: {updates:?}");
        assert!(updates.iter().take(2).all(|u| matches!(
            &u.update,
            SessionUpdate::AgentMessageChunk(c) if matches!(&c.content, ContentBlock::Text(t) if t.text == "He" || t.text == "llo")
        )));
        assert!(matches!(&updates[2].update, SessionUpdate::ToolCall(_)));
        let SessionUpdate::ToolCallUpdate(u) = &updates[3].update else {
            panic!("expected tool_call_update");
        };
        assert_eq!(u.fields.status, Some(ToolCallStatus::Completed));
        assert!(matches!(&updates[4].update, SessionUpdate::UsageUpdate(_)));
    }

    /// error 事件是独立终态 → Refusal(此前收到的 delta 照常转发)。
    #[tokio::test]
    async fn prompt_loop_error_event_responds_refusal() {
        let (tx, rx) = broadcast::channel(8);
        let sink = VecSink::new();
        tx.send(chat_event(
            "r1",
            "s1",
            ChatEventDto::Delta {
                text: "partial".into(),
            },
        ))
        .unwrap();
        tx.send(chat_event(
            "r1",
            "s1",
            ChatEventDto::Error {
                message: "provider down".into(),
                category: "network".into(),
            },
        ))
        .unwrap();

        let resp = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::Refusal);
        assert_eq!(sink.notifications().len(), 1, "error 前的 delta 已送达");
    }

    /// cancel 终态(daemon done.cancelled)→ Cancelled;Lagged 不悬挂。
    #[tokio::test]
    async fn prompt_loop_cancelled_and_lagged_do_not_hang() {
        // Lagged:先把广播灌满(broadcast 容量 2)再消费,消费方应收到
        // Lagged 错误并继续等终态,而不是报错退出。
        let (tx, rx) = broadcast::channel(2);
        let sink = VecSink::new();
        for i in 0..6 {
            tx.send(chat_event(
                "r1",
                "s1",
                ChatEventDto::Delta {
                    text: format!("d{i}"),
                },
            ))
            .unwrap();
        }
        tx.send(chat_event(
            "r1",
            "s1",
            ChatEventDto::Done {
                stop_reason: Some("cancelled".into()),
                usage: None,
            },
        ))
        .unwrap();
        let resp = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::Cancelled);
        assert!(sink.notifications().len() < 6, "Lagged 丢帧但流程走到终态");
    }

    /// daemon 重启哨兵(restart)落在 turn 中间 = 在途 turn 已随进程死亡,
    /// done/error 永不再来 —— 必须 respond error 收束,不得悬挂。
    #[tokio::test]
    async fn prompt_loop_resync_restart_fails_prompt() {
        let (tx, rx) = broadcast::channel(8);
        let sink = VecSink::new();
        tx.send(DaemonEvent::Resync {
            reason: "restart".into(),
        })
        .unwrap();

        let err = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .expect_err("restart sentinel must fail the prompt");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError);
        assert!(
            err.message.contains("restarted"),
            "消息应指明 daemon 重启: {err}"
        );
        assert!(sink.notifications().is_empty());
    }

    /// buffer_overrun 哨兵(daemon 仍活着,ring 淘汰)不是终态:继续等,
    /// 随后的 done 照常收束。
    #[tokio::test]
    async fn prompt_loop_survives_buffer_overrun_resync() {
        let (tx, rx) = broadcast::channel(8);
        let sink = VecSink::new();
        tx.send(DaemonEvent::Resync {
            reason: "buffer_overrun".into(),
        })
        .unwrap();
        tx.send(chat_event(
            "r1",
            "s1",
            ChatEventDto::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            },
        ))
        .unwrap();

        let resp = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    /// 流失联(恒不健康句柄)超过 stale_limit:respond error,不悬挂。
    /// (dead_sse 永不健康;IDLE_POLL 1s 后进入守卫,stale_limit 收短到
    /// 50ms 立即判死。)
    #[tokio::test]
    async fn prompt_loop_errors_when_stream_stays_unhealthy() {
        let (_tx, rx) = broadcast::channel(8);
        let sink = VecSink::new();

        let err = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &ScriptedOutbound::default(),
            &dead_daemon(),
            &dead_sse(),
            Duration::from_millis(50),
        )
        .await
        .expect_err("permanently unhealthy stream must fail the prompt");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InternalError);
        assert!(err.message.contains("daemon.sh"), "{err}");
        assert!(!err.message.contains("restarted"), "{err}");
    }

    /// 受理检查:started 放行,queued/injected 拒绝(取舍 3),消息注明 busy。
    #[test]
    fn ensure_started_rejects_queued_and_injected() {
        assert!(ensure_started(ChatAcceptance::Started).is_ok());
        let err = ensure_started(ChatAcceptance::Queued {
            id: "q1".into(),
            position: 2,
        })
        .expect_err("queued must be rejected");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidRequest);
        let data = err.data.expect("busy detail");
        assert!(data.as_str().unwrap().contains("busy"), "{data}");
        assert!(data.as_str().unwrap().contains("position 2"), "{data}");

        let err = ensure_started(ChatAcceptance::Injected).expect_err("injected must be rejected");
        assert!(err.data.is_some());
    }

    /// prompt ContentBlock 基线:Text 直拼、ResourceLink 消化为文本标记、
    /// Image 拒绝(text-only)、空 prompt 拒绝。
    #[test]
    fn prompt_text_enforces_text_only_baseline() {
        let text = prompt_text(&[ContentBlock::Text(TextContent::new("hi "))]).unwrap();
        assert_eq!(text, "hi ");

        let with_link = prompt_text(&[
            ContentBlock::Text(TextContent::new("see ")),
            ContentBlock::ResourceLink(agent_client_protocol::schema::v1::ResourceLink::new(
                "a.rs",
                "file:///a.rs",
            )),
        ])
        .unwrap();
        assert!(with_link.contains("see "));
        assert!(
            with_link.contains("file:///a.rs"),
            "ResourceLink 消化进文本: {with_link}"
        );

        let err = prompt_text(&[ContentBlock::Image(ImageContent::new(
            "data:image/png;base64,xxx",
            "image/png",
        ))])
        .expect_err("image must be rejected");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);

        let err = prompt_text(&[]).expect_err("empty must be rejected");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    /// request_id:进程内不重复(同毫秒内靠序号区分)。
    #[test]
    fn request_ids_are_unique_within_process() {
        let a = make_request_id();
        let b = make_request_id();
        assert_ne!(a, b);
        assert!(a.starts_with("acp-"));
    }

    /// 全链(SSI 任务 → broadcast → prompt 消费):wiremock 同时扮演
    /// SSE 流与 chat 受理端,SseHandle 真连真解析,prompt 消费到终态。
    /// wiremock body 一次性下发(非逐帧 chunked)对本状态机无差 ——
    /// 解析层按 \\n\\n 切帧;PR4 集成测试沿用此形状。
    #[tokio::test]
    async fn prompt_full_chain_against_wiremock_sse_stream() {
        let server = MockServer::start().await;
        // SSE:两帧 delta + done(end_turn);ping 注释帧混入验证丢弃。
        let sse_body = concat!(
            "id: 1\nevent: chat-event\ndata: {\"request_id\":\"acp-t\",\"session_id\":\"s9\",\"kind\":\"delta\",\"text\":\"he\"}\n\n",
            ":ping\n\n",
            "id: 2\nevent: chat-event\ndata: {\"request_id\":\"acp-t\",\"session_id\":\"s9\",\"kind\":\"delta\",\"text\":\"y\"}\n\n",
            "id: 3\nevent: chat-event\ndata: {\"request_id\":\"acp-t\",\"session_id\":\"s9\",\"kind\":\"done\",\"stop_reason\":\"end_turn\",\"usage\":null}\n\n",
        );
        Mock::given(method("GET"))
            .and(path("/api/v1/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/agent/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "status": "started" })),
            )
            .mount(&server)
            .await;

        let app = App::new(
            DaemonClient::new(server.uri()).unwrap(),
            crate::sse::SseHandle::spawn(DaemonClient::new(server.uri()).unwrap()),
        );
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s9".into(),
        });

        // 预置已知 session 表项(wiremock 帧里的 session_id 写死 s9)。
        // 受理面(agent_chat → ensure_started)已在 daemon.rs 单测 +
        // queued 用例覆盖;本用例专验 SseHandle 真连真解析 → broadcast
        // 归一链,故不调 prompt(其生成 rid 与 wiremock 固定帧不匹配)。
        let mut rx = app.sse.subscribe();
        assert!(
            app.sse.wait_healthy(Duration::from_secs(5)).await,
            "wiremock SSE 应可连"
        );
        // 收齐 3 条归一事件(delta, delta, done)。
        let mut events = Vec::new();
        for _ in 0..3 {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("event within timeout")
                .expect("broadcast alive");
            events.push(ev);
        }
        assert!(
            matches!(&events[0], DaemonEvent::ChatEvent { request_id, session_id, event: ChatEventDto::Delta { text } }
                if request_id == "acp-t" && session_id == "s9" && text == "he"),
            "{events:?}"
        );
        assert!(matches!(
            &events[2],
            DaemonEvent::ChatEvent {
                event: ChatEventDto::Done { .. },
                ..
            }
        ));
    }

    /// queued 受理全链:wiremock 回 queued → prompt 返回 invalid_request,
    /// 事件流不被消费。
    #[tokio::test]
    async fn prompt_rejects_queued_acceptance_with_busy_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(""),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/agent/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({ "status": "queued", "id": "q1", "position": 1 }),
            ))
            .mount(&server)
            .await;

        let app = App::new(
            DaemonClient::new(server.uri()).unwrap(),
            crate::sse::SseHandle::spawn(DaemonClient::new(server.uri()).unwrap()),
        );
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s9".into(),
        });
        assert!(app.sse.wait_healthy(Duration::from_secs(5)).await);

        let sink = VecSink::new();
        let outbound = ScriptedOutbound::default();
        let err = prompt(
            &app,
            PromptRequest::new("s9", vec![ContentBlock::Text(TextContent::new("hi"))]),
            &sink,
            &outbound,
        )
        .await
        .expect_err("queued must be rejected");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidRequest);
        assert!(err
            .data
            .is_some_and(|d| d.as_str().unwrap().contains("busy")));
        assert!(sink.notifications().is_empty());

        // 未知 session 拒绝。
        let err = prompt(
            &app,
            PromptRequest::new("nope", vec![ContentBlock::Text(TextContent::new("hi"))]),
            &sink,
            &outbound,
        )
        .await
        .expect_err("unknown session");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);

        // text-only 门:image 内容拒绝。
        let err = prompt(
            &app,
            PromptRequest::new(
                "s9",
                vec![ContentBlock::Image(ImageContent::new(
                    "data:x",
                    "image/png",
                ))],
            ),
            &sink,
            &outbound,
        )
        .await
        .expect_err("image rejected");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    /// 权限环经 prompt 消费循环端到端:本 session 的 ask → 并行 spawn 桥 →
    /// ScriptedOutbound 应答 allow_once → wiremock 收到 permission_response;
    /// done 照常收束(turn 不被审批等待阻塞)。
    #[tokio::test]
    async fn prompt_loop_spawns_permission_bridge_in_parallel() {
        let (server, guard) =
            permission_response_server(Some("allow_once"), serde_json::json!(true)).await;
        let (tx, rx) = broadcast::channel(8);
        let sink = VecSink::new();
        let outbound = ScriptedOutbound::new([Script::Select("allow_once")]);

        tx.send(DaemonEvent::PermissionAsk(
            serde_json::from_str(
                r#"{"rid":"r1","sessionId":"s1","toolUseId":"tu_1","toolName":"shell",
                    "toolInput":{"command":"ls"},"risk":"medium"}"#,
            )
            .unwrap(),
        ))
        .unwrap();
        tx.send(chat_event(
            "r1",
            "s1",
            ChatEventDto::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            },
        ))
        .unwrap();

        let resp = run_prompt_turn(
            rx,
            "r1",
            "s1",
            &sink,
            &outbound,
            &DaemonClient::new(server.uri()).unwrap(),
            &dead_sse(),
            no_stale(),
        )
        .await
        .unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn, "审批并行,终态照常");

        // 反向请求与 permission_response 各一次(桥完成,等 guard 命中)。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while outbound.calls().is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "bridge never ran");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        while guard.received_requests().await.is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "post never landed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// cancel 全链:session 表有在途 rid → cancel 通知 → cancel_chat 落点
    /// (body 精确锚 {request_id});prompt 侧 done(cancelled) → Cancelled
    /// respond(PR2 已落的终态路径,此处钉通知→落点半程)。
    #[tokio::test]
    async fn cancel_notification_posts_cancel_chat_for_active_rid() {
        let server = MockServer::start().await;
        let guard = Mock::given(method("POST"))
            .and(path("/api/v1/cancel/cancel_chat"))
            .and(body_json(serde_json::json!({ "request_id": "r-inflight" })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "cancelled": true, "cleared_queued": 0 })),
            )
            .mount_as_scoped(&server)
            .await;

        let app = app_at(server.uri());
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s1".into(),
        });
        app.sessions.set_active_request("s1", "r-inflight".into());

        cancel(&app, CancelNotification::new("s1")).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while guard.received_requests().await.is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "cancel_chat not posted"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// cancel 无在途 turn / 未注册 session → 不发 cancel_chat(静默 no-op)。
    #[tokio::test]
    async fn cancel_without_inflight_turn_is_noop() {
        let server = MockServer::start().await;
        let guard = Mock::given(method("POST"))
            .and(path("/api/v1/cancel/cancel_chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "cancelled": false, "cleared_queued": 0 })),
            )
            .mount_as_scoped(&server)
            .await;

        let app = app_at(server.uri());
        // 已注册但无在途 turn。
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s1".into(),
        });
        cancel(&app, CancelNotification::new("s1")).await;
        // 未注册 session。
        cancel(&app, CancelNotification::new("ghost")).await;
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            guard.received_requests().await.len(),
            0,
            "无在途 rid 不得触发 cancel_chat"
        );
    }

    /// set_mode 往返:daemon 落点(body 锚)+ 响应 + current_mode_update 通知。
    #[tokio::test]
    async fn set_mode_roundtrip_posts_and_notifies_current_mode_update() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/permissions/set_session_mode"))
            .and(body_json(
                serde_json::json!({ "session_id": "s1", "mode": "plan" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "s1", "title": "t", "project_id": "p1",
                "current_cwd": "/x", "mode": "plan"
            })))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s1".into(),
        });
        let sink = VecSink::new();
        let resp = set_mode(
            &app,
            agent_client_protocol::schema::v1::SetSessionModeRequest::new("s1", "plan"),
            &sink,
        )
        .await
        .unwrap();
        assert!(resp.meta.is_none(), "空响应体");

        let updates = sink.notifications();
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0].update,
            SessionUpdate::CurrentModeUpdate(m) if m.current_mode_id.0.as_ref() == "plan"
        ));
    }

    /// set_mode 未知值:normalize 回 edit(与 daemon lenient 对齐),
    /// 落库 body 是 edit 而非原值。
    #[tokio::test]
    async fn set_mode_unknown_value_falls_back_to_edit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/permissions/set_session_mode"))
            .and(body_json(
                serde_json::json!({ "session_id": "s1", "mode": "edit" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "s1", "title": "t", "project_id": "p1",
                "current_cwd": "/x", "mode": "edit"
            })))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        app.sessions.register(super::super::SessionEntry {
            daemon_session_id: "s1".into(),
        });
        let sink = VecSink::new();
        set_mode(
            &app,
            agent_client_protocol::schema::v1::SetSessionModeRequest::new("s1", "chat"),
            &sink,
        )
        .await
        .unwrap();
        let updates = sink.notifications();
        assert!(matches!(
            &updates[0].update,
            SessionUpdate::CurrentModeUpdate(m) if m.current_mode_id.0.as_ref() == "edit"
        ));
    }

    /// session/new 全链:list(未命中)→ create_project → create_session →
    /// set_session_mode → 响应带 daemon session id + modes;请求体 casing
    /// 由 body_json 精确匹配锚死(缺口 5:匹配失败 = 404 = handler 报错 = 红测)。
    #[tokio::test]
    async fn session_new_full_chain_creates_project_and_session() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/api/v1/projects/list_projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/projects/create_project"))
            .and(body_json(serde_json::json!({ "path": "/tmp/acp-zed" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "p1", "name": "acp-zed", "path": "/tmp/acp-zed"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/sessions/create_session"))
            .and(body_json(
                serde_json::json!({ "project_id": "p1", "initial_cwd": "/tmp/acp-zed" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "s1", "title": "s1", "project_id": "p1",
                "current_cwd": "/tmp/acp-zed", "mode": "edit"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/permissions/set_session_mode"))
            .and(body_json(
                serde_json::json!({ "session_id": "s1", "mode": "edit" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "s1", "title": "s1", "project_id": "p1",
                "current_cwd": "/tmp/acp-zed", "mode": "edit"
            })))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        let resp = new_session(&app, NewSessionRequest::new("/tmp/acp-zed/./sub/.."))
            .await
            .unwrap();

        // 词规整后的 cwd 一路透传(/tmp/acp-zed/./sub/.. → /tmp/acp-zed)。
        assert_eq!(resp.session_id.0.as_ref(), "s1");
        let modes = resp.modes.expect("modes advertised");
        assert_eq!(modes.current_mode_id, SessionModeId::new("edit"));
        assert_eq!(modes.available_modes.len(), 3);
        let ids: Vec<_> = modes
            .available_modes
            .iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(ids, vec!["edit", "plan", "yolo"]);

        // session 表登记(ACP sessionId = daemon session_id,1:1)。
        assert!(app.sessions.get("s1").is_some(), "session registered");
    }

    /// session/load 最小实现:存在 → 登记 + modes;不存在 → resource_not_found。
    #[tokio::test]
    async fn session_load_registers_session_and_reports_modes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/sessions/load_session"))
            .and(body_json(serde_json::json!({ "session_id": "s9" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "session": {
                    "id": "s9", "title": "t", "project_id": "p1",
                    "current_cwd": "/tmp/acp-zed", "mode": "plan"
                },
                "messages": []
            })))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        let req = LoadSessionRequest::new("s9", "/tmp/acp-zed");
        let resp = load_session(&app, req, &VecSink::new()).await.unwrap();
        assert_eq!(
            resp.modes.expect("modes").current_mode_id,
            SessionModeId::new("plan"),
            "当前 mode 取 daemon 行值"
        );
        assert!(app.sessions.get("s9").is_some());

        // 未存在的 session → resource_not_found(daemon 返回 null)。
        Mock::given(method("POST"))
            .and(path("/api/v1/sessions/load_session"))
            .and(body_json(serde_json::json!({ "session_id": "missing" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::Value::Null))
            .mount(&server)
            .await;
        let err = load_session(
            &app,
            LoadSessionRequest::new("missing", "/tmp/acp-zed"),
            &VecSink::new(),
        )
        .await
        .expect_err("missing session");
        assert_eq!(err.code, agent_client_protocol::ErrorCode::ResourceNotFound);
    }

    /// session/list:cwd 命中 project → 该项目会话映射为 SessionInfo;
    /// cwd 未注册 → 空列表(不侧建)。
    #[tokio::test]
    async fn session_list_filters_by_project_and_maps_rows() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/projects/list_projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": "p1", "name": "zed", "path": "/tmp/acp-zed" }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/sessions/list_sessions"))
            .and(body_json(serde_json::json!({ "project_id": "p1" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "id": "s1", "title": "fix bug", "project_id": "p1",
                    "current_cwd": "/tmp/acp-zed", "updated_at": "2026-09-30T00:00:00Z"
                }
            ])))
            .mount(&server)
            .await;

        let app = app_at(server.uri());
        let resp = list_sessions(
            &app,
            ListSessionsRequest::new().cwd(std::path::PathBuf::from("/tmp/acp-zed")),
        )
        .await
        .unwrap();
        assert_eq!(resp.sessions.len(), 1);
        assert_eq!(resp.sessions[0].session_id.0.as_ref(), "s1");
        assert_eq!(
            resp.sessions[0].cwd,
            std::path::PathBuf::from("/tmp/acp-zed")
        );
        assert_eq!(resp.sessions[0].title.as_deref(), Some("fix bug"));
        assert!(resp.next_cursor.is_none(), "无分页源,恒无下一页");

        // 未注册目录 → 空,不 create。
        Mock::given(method("POST"))
            .and(path("/api/v1/projects/create_project"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let resp = list_sessions(
            &app,
            ListSessionsRequest::new().cwd(std::path::PathBuf::from("/tmp/never-registered")),
        )
        .await
        .unwrap();
        assert!(resp.sessions.is_empty());
    }
}
