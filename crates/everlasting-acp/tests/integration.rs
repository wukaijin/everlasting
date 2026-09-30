//! PR4 集成测试:spawn 真 `everlasting-acp` 子进程 + 假 daemon(axum,真流式
//! SSE)+ SDK Client role 经 stdio 全链驱动 —— `tests_*` 单测覆盖纯函数面,
//! 本文件只留全链(与单测重复的用例已收敛进这里或删除)。
//!
//! 假 daemon 为什么不用 wiremock:cancel/load 用例需要**状态化** SSE
//! (按测试时序推帧、捕获请求体取动态 rid 后再回帧),wiremock 的静态
//! body 做不到 —— PR1 的「hyper/axum 手写」预案在此兑现。单测里的
//! wiremock 用例保持不变(无时序依赖)。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, EnvVariable, InitializeRequest, LoadSessionRequest,
    McpServer, McpServerStdio, NewSessionRequest, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, TextContent, ToolCallStatus,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Client, ConnectionTo, ErrorCode};
use futures_util::StreamExt;
use serde_json::{json, Value};

const SHIM_BIN: &str = env!("CARGO_BIN_EXE_everlasting-acp");
const CWD: &str = "/tmp/acp-zed";
const SESSION_ID: &str = "s-fixed-1";
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// 假 daemon:真流式 SSE(broadcast 推帧)+ POST 路由表 + 请求体捕获
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FakeDaemon {
    inner: Arc<FakeDaemonInner>,
}

struct FakeDaemonInner {
    sse_tx: tokio::sync::broadcast::Sender<String>,
    /// 精确路径 → 固定 JSON 响应体(POST)。
    routes: Mutex<Vec<(String, Value)>>,
    /// (path, body) 请求日志,断言用。
    requests: Mutex<Vec<(String, Value)>>,
    next_event_id: AtomicEventId,
}

#[derive(Default)]
struct AtomicEventId(Mutex<u64>);

impl AtomicEventId {
    fn next(&self) -> u64 {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        *n
    }
}

impl FakeDaemon {
    /// 起服务,返回 (句柄, base_url)。
    async fn spawn() -> (Self, String) {
        let (sse_tx, _) = tokio::sync::broadcast::channel(256);
        let inner = Arc::new(FakeDaemonInner {
            sse_tx,
            routes: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            next_event_id: AtomicEventId::default(),
        });
        let state = inner.clone();

        async fn stream(
            state: Arc<FakeDaemonInner>,
        ) -> axum::response::Sse<
            impl futures_util::Stream<
                Item = Result<axum::response::sse::Event, std::convert::Infallible>,
            >,
        > {
            use axum::response::sse::{Event, KeepAlive, Sse};
            use tokio_stream::wrappers::BroadcastStream;

            let rx = state.sse_tx.subscribe();
            let frames = BroadcastStream::new(rx).filter_map(|item| async move {
                let frame = item.ok()?;
                // 预格式化的整帧文本 → 拆回 event/data/id 三元组。
                let (event, data, id) = parse_frame(&frame);
                Some(Ok(Event::default().event(event).data(data).id(id)))
            });
            Sse::new(frames).keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(5))
                    .text("ping"),
            )
        }

        async fn fallback(
            state: Arc<FakeDaemonInner>,
            request: axum::extract::Request,
        ) -> axum::response::Response {
            use axum::http::StatusCode;
            use axum::response::IntoResponse;

            let path = request.uri().path().to_string();
            let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .unwrap_or_default();
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            state.requests.lock().unwrap().push((path.clone(), body));

            let canned = state
                .routes
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(p, _)| *p == path)
                .map(|(_, v)| v.clone());
            match canned {
                Some(v) => axum::Json(v).into_response(),
                None => (StatusCode::NOT_FOUND, "no route").into_response(),
            }
        }

        let app = axum::Router::new()
            // 健康探针(shim initialize 入口固定打一次;无状态固定形状)。
            .route(
                "/api/v1/health",
                axum::routing::get(|| async {
                    axum::Json(json!({
                        "daemonId": "fake", "daemonVersion": "0.0.0",
                        "apiVersions": ["v1"], "uptimeSeconds": 1
                    }))
                }),
            )
            .route(
                "/api/v1/stream",
                axum::routing::get({
                    let state = state.clone();
                    move |headers: axum::http::HeaderMap| {
                        // 收到重连即 200;Last-Event-ID 被 shim 回传但本假
                        // daemon 无 buffer,replay 语义不测(单测面)。
                        let _ = headers.get("last-event-id");
                        stream(state.clone())
                    }
                }),
            )
            .fallback({
                let state = state.clone();
                move |req| fallback(state.clone(), req)
            });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (FakeDaemon { inner }, uri)
    }

    /// 挂 POST 固定响应(后挂者优先;查表反向遍历)。
    fn on_post(&self, path: &str, body: Value) {
        self.inner
            .routes
            .lock()
            .unwrap()
            .push((path.to_string(), body));
    }

    /// 推一条 SSE 帧(整帧预格式化,与 daemon wire 同形)。
    fn push_sse(&self, event: &str, data: Value) {
        let id = self.inner.next_event_id.next();
        let frame = format!("id: {id}\nevent: {event}\ndata: {data}\n\n");
        let _ = self.inner.sse_tx.send(frame);
    }

    fn chat_event(&self, rid: &str, sid: &str, payload: Value) {
        let mut data = payload;
        let obj = data.as_object_mut().unwrap();
        obj.insert("request_id".into(), json!(rid));
        obj.insert("session_id".into(), json!(sid));
        self.push_sse("chat-event", data);
    }

    /// 捕获的请求体(按路径过滤,后到在前)。
    fn requests(&self, path: &str) -> Vec<Value> {
        self.inner
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }

    async fn wait_for_request(&self, path: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
        loop {
            if let Some(body) = self.requests(path).into_iter().find(|b| pred(b)) {
                return body;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "request {path} never arrived"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// 挂上 session 生命周期所需的最小路由(list 命中 → create → set_mode)。
    fn mount_session_flow(&self) {
        self.on_post(
            "/api/v1/projects/list_projects",
            json!([{ "id": "p1", "name": "zed", "path": CWD }]),
        );
        self.on_post(
            "/api/v1/sessions/create_session",
            json!({
                "id": SESSION_ID, "title": "t", "project_id": "p1",
                "current_cwd": CWD, "mode": "edit"
            }),
        );
        self.on_post(
            "/api/v1/permissions/set_session_mode",
            json!({
                "id": SESSION_ID, "title": "t", "project_id": "p1",
                "current_cwd": CWD, "mode": "edit"
            }),
        );
    }

    /// 挂 load_session 固定 messages。
    fn on_load_session(&self, messages: Value) {
        self.on_post(
            "/api/v1/sessions/load_session",
            json!({
                "session": {
                    "id": SESSION_ID, "title": "t", "project_id": "p1",
                    "current_cwd": CWD, "mode": "edit"
                },
                "messages": messages
            }),
        );
    }
}

/// 把预格式化帧拆回 (event, data, id)(只支持本文件生成的形状)。
fn parse_frame(frame: &str) -> (&str, String, String) {
    let mut event = "message";
    let mut data = String::new();
    let mut id = String::new();
    for line in frame.trim_end_matches('\n').split('\n') {
        if let Some(v) = line.strip_prefix("event: ") {
            event = v;
        } else if let Some(v) = line.strip_prefix("data: ") {
            data = v.to_string();
        } else if let Some(v) = line.strip_prefix("id: ") {
            id = v.to_string();
        }
    }
    (event, data, id)
}

// ---------------------------------------------------------------------------
// client 侧 harness
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct ClientState {
    /// 收到的 session/update 通知(到达序)。
    updates: Arc<Mutex<Vec<SessionNotification>>>,
    /// session/request_permission 的 optionId 应答脚本(空 = cancelled)。
    permission_scripts: Arc<Mutex<VecDeque<&'static str>>>,
    permission_requests: Arc<Mutex<Vec<RequestPermissionRequest>>>,
}

impl ClientState {
    fn new(scripts: &[&'static str]) -> Self {
        Self {
            updates: Arc::new(Mutex::new(Vec::new())),
            permission_scripts: Arc::new(Mutex::new(scripts.iter().copied().collect())),
            permission_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn updates(&self) -> Vec<SessionNotification> {
        self.updates.lock().unwrap().clone()
    }
}

/// spawn 真 shim 子进程并以 Client role 连接,跑 `main`。
/// 整体 60s 兜底超时:任何悬挂(该 respond 不 respond)直接红测。
async fn run_client<R>(
    daemon_uri: &str,
    state: ClientState,
    main: impl AsyncFnOnce(
        ConnectionTo<agent_client_protocol::Agent>,
    ) -> Result<R, agent_client_protocol::Error>,
) -> R {
    let agent = AcpAgent::new(McpServer::Stdio(
        McpServerStdio::new("everlasting-acp", SHIM_BIN)
            .args(vec![])
            .env(vec![
                EnvVariable::new("EVERLASTING_ACP_DAEMON_URL", daemon_uri),
                EnvVariable::new("RUST_LOG", "error"),
            ]),
    ));

    let updates = state.updates.clone();
    let scripts = state.permission_scripts.clone();
    let requests = state.permission_requests.clone();

    let run = Client
        .builder()
        .name("integration-test")
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                updates.lock().unwrap().push(notification);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _cx| {
                requests.lock().unwrap().push(request);
                let response = match scripts.lock().unwrap().pop_front() {
                    Some(option_id) => {
                        RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(option_id.to_string()),
                        ))
                    }
                    None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
                };
                responder.respond(response)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, main);

    tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("integration case timed out")
        .expect("client connection failed")
}

/// initialize + session/new(cwd 命中既有 project)。
async fn init_and_new(cx: &ConnectionTo<agent_client_protocol::Agent>) -> SessionId {
    cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await
        .expect("initialize");
    let resp = cx
        .send_request(NewSessionRequest::new(CWD))
        .block_task()
        .await
        .expect("session/new");
    resp.session_id
}

fn text_prompt(text: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text(TextContent::new(text.to_string()))]
}

fn chat_event_json(rid: &str, kind: &str, extra: Value) -> Value {
    let mut obj = json!({ "kind": kind });
    let obj = obj.as_object_mut().unwrap();
    if let Some(extra) = extra.as_object() {
        for (k, v) in extra {
            obj.insert(k.clone(), v.clone());
        }
    }
    let _ = rid;
    json!(obj)
}

// ---------------------------------------------------------------------------
// 用例组
// ---------------------------------------------------------------------------

/// happy path:多帧 delta → done(end_turn);update 先于响应到达
/// (协议时序:block_task 返回前 update 已推给客户端)。
#[tokio::test]
async fn happy_path_text_stream() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));

    let state = ClientState::new(&[]);
    let daemon2 = daemon.clone();
    run_client(&uri, state.clone(), async move |cx| {
        let sid = init_and_new(&cx).await;
        // 推帧任务:等 shim 的 agent/chat POST(拿到动态 rid)后推帧。
        // 必须与 client 的 prompt 并发 —— rid 从 POST 来,POST 由 prompt
        // 触发,串行等会自锁。
        let pusher = tokio::spawn(async move {
            let rid = daemon2
                .wait_for_request("/api/v1/agent/chat", |b| b["session_id"] == SESSION_ID)
                .await["request_id"]
                .as_str()
                .unwrap()
                .to_string();
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(&rid, "delta", json!({ "text": "He" })),
            );
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(&rid, "delta", json!({ "text": "llo" })),
            );
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(
                    &rid,
                    "done",
                    json!({ "stop_reason": "end_turn", "usage": null }),
                ),
            );
        });
        let resp = cx
            .send_request(PromptRequest::new(sid.clone(), text_prompt("hi")))
            .block_task()
            .await?;
        pusher.await.unwrap();
        Ok(resp.stop_reason)
    })
    .await;
    assert_eq!(state.updates().len(), 2, "两帧 delta 各一条 update");
    assert!(state
        .updates()
        .iter()
        .all(|u| matches!(&u.update, SessionUpdate::AgentMessageChunk(_))));
}

/// thinking 流:thinking_delta → agent_thought_chunk。
#[tokio::test]
async fn thinking_stream() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));

    let state = ClientState::new(&[]);
    let daemon2 = daemon.clone();
    run_client(&uri, state.clone(), async move |cx| {
        let sid = init_and_new(&cx).await;
        let pusher = tokio::spawn(async move {
            let rid = daemon2
                .wait_for_request("/api/v1/agent/chat", |b| b["session_id"] == SESSION_ID)
                .await["request_id"]
                .as_str()
                .unwrap()
                .to_string();
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(&rid, "thinking_delta", json!({ "text": "hmm" })),
            );
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(&rid, "delta", json!({ "text": "!" })),
            );
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(
                    &rid,
                    "done",
                    json!({ "stop_reason": "end_turn", "usage": null }),
                ),
            );
        });
        let resp = cx
            .send_request(PromptRequest::new(sid, text_prompt("hi")))
            .block_task()
            .await?;
        pusher.await.unwrap();
        Ok(resp.stop_reason)
    })
    .await;
    let updates = state.updates();
    assert_eq!(updates.len(), 2);
    assert!(matches!(
        &updates[0].update,
        SessionUpdate::AgentThoughtChunk(_)
    ));
    assert!(matches!(
        &updates[1].update,
        SessionUpdate::AgentMessageChunk(_)
    ));
}

/// tool 两态:tool:call(pending)→ tool:result → tool_call_update(completed)。
#[tokio::test]
async fn tool_two_state_stream() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));

    let state = ClientState::new(&[]);
    let daemon2 = daemon.clone();
    run_client(&uri, state.clone(), async move |cx| {
            let sid = init_and_new(&cx).await;
            let pusher = tokio::spawn(async move {
                let rid = daemon2
                    .wait_for_request("/api/v1/agent/chat", |b| {
                        b["session_id"] == SESSION_ID
                    })
                    .await["request_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                daemon2.push_sse(
                    "tool:call",
                    json!({ "request_id": rid, "session_id": SESSION_ID, "id": "tu_1", "name": "shell", "input": { "command": "ls" } }),
                );
                daemon2.push_sse(
                    "tool:result",
                    json!({ "request_id": rid, "session_id": SESSION_ID, "tool_use_id": "tu_1", "content": "out", "is_error": false }),
                );
                daemon2.chat_event(&rid, SESSION_ID, chat_event_json(&rid, "done", json!({ "stop_reason": "end_turn", "usage": null })));
            });
            let resp = cx
                .send_request(PromptRequest::new(sid, text_prompt("hi")))
                .block_task()
                .await?;
            pusher.await.unwrap();
            Ok(resp.stop_reason)
    })
    .await;
    let updates = state.updates();
    assert_eq!(updates.len(), 2);
    assert!(matches!(&updates[0].update, SessionUpdate::ToolCall(_)));
    let SessionUpdate::ToolCallUpdate(u) = &updates[1].update else {
        panic!("expected tool_call_update");
    };
    assert_eq!(u.fields.status, Some(ToolCallStatus::Completed));
}

/// permission allow_once 全环:ask → 反向请求 → client 应答 →
/// shim POST permission_response(落点与 decision 断言)→ turn 收束。
#[tokio::test]
async fn permission_allow_once_full_round_trip() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));
    daemon.on_post("/api/v1/permissions/permission_response", json!(true));

    let state = ClientState::new(&["allow_once"]);
    let daemon2 = daemon.clone();
    let state2 = state.clone();
    run_client(&uri, state.clone(), async move |cx| {
            let sid = init_and_new(&cx).await;
            // 推帧任务(与 prompt 并发,同 happy path):ask → (client 脚本
            // 应答)→ 等 shim POST permission_response 落点 → done。
            let state_inner = state2.clone();
            let pusher = tokio::spawn(async move {
                let rid = daemon2
                    .wait_for_request("/api/v1/agent/chat", |b| {
                        b["session_id"] == SESSION_ID
                    })
                    .await["request_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                daemon2.push_sse(
                    "permission:ask",
                    json!({ "rid": "ask-1", "sessionId": SESSION_ID, "toolUseId": "tu_1",
                            "toolName": "shell", "toolInput": { "command": "curl x" }, "risk": "medium" }),
                );
                let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
                while state_inner.permission_requests.lock().unwrap().is_empty() {
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "reverse request never arrived"
                    );
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let post = daemon2
                    .wait_for_request("/api/v1/permissions/permission_response", |b| {
                        b["rid"] == "ask-1"
                    })
                    .await;
                assert_eq!(post["decision"], "allow_once");
                daemon2.chat_event(&rid, SESSION_ID, chat_event_json(&rid, "done", json!({ "stop_reason": "end_turn", "usage": null })));
            });
            let resp = cx
                .send_request(PromptRequest::new(sid, text_prompt("hi")))
                .block_task()
                .await?;
            pusher.await.unwrap();
            Ok(resp.stop_reason)
    })
    .await;

    let requests = state.permission_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].options.len(), 3);
    let kinds: Vec<_> = requests[0].options.iter().map(|o| o.kind).collect();
    assert!(kinds.contains(&PermissionOptionKind::AllowOnce));
    assert!(kinds.contains(&PermissionOptionKind::RejectOnce));
}

/// permission reject → decision `deny`(rename 点,跨进程实证)。
#[tokio::test]
async fn permission_reject_maps_to_deny() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));
    daemon.on_post("/api/v1/permissions/permission_response", json!(true));

    let state = ClientState::new(&["reject_once"]);
    let daemon2 = daemon.clone();
    run_client(&uri, state, async move |cx| {
        let sid = init_and_new(&cx).await;
        let pusher = tokio::spawn(async move {
            let rid = daemon2
                .wait_for_request("/api/v1/agent/chat", |b| b["session_id"] == SESSION_ID)
                .await["request_id"]
                .as_str()
                .unwrap()
                .to_string();
            daemon2.push_sse(
                "permission:ask",
                json!({ "rid": "ask-1", "sessionId": SESSION_ID, "toolUseId": "tu_1",
                            "toolName": "shell", "toolInput": {}, "risk": "high" }),
            );
            let post = daemon2
                .wait_for_request("/api/v1/permissions/permission_response", |b| {
                    b["rid"] == "ask-1"
                })
                .await;
            assert_eq!(post["decision"], "deny", "reject_once → deny rename 点");
            daemon2.chat_event(
                &rid,
                SESSION_ID,
                chat_event_json(
                    &rid,
                    "done",
                    json!({ "stop_reason": "end_turn", "usage": null }),
                ),
            );
        });
        let _ = cx
            .send_request(PromptRequest::new(sid, text_prompt("hi")))
            .block_task()
            .await?;
        pusher.await.unwrap();
        Ok(())
    })
    .await;
}

/// cancel 全链:prompt 在途 → client 发 session/cancel 通知 → shim POST
/// cancel_chat(rid 精确)→ done(cancelled) → respond Cancelled。
#[tokio::test]
async fn cancel_full_chain() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post("/api/v1/agent/chat", json!({ "status": "started" }));
    daemon.on_post(
        "/api/v1/cancel/cancel_chat",
        json!({ "cancelled": true, "cleared_queued": 0 }),
    );

    let state = ClientState::new(&[]);
    let daemon2 = daemon.clone();
    run_client(&uri, state, async move |cx| {
        let sid = init_and_new(&cx).await;
        // prompt 挂后台等终态;主流程发在途 cancel 通知。
        let prompt_task = tokio::spawn({
            let cx = cx.clone();
            let sid = sid.clone();
            async move {
                cx.send_request(PromptRequest::new(sid, text_prompt("hi")))
                    .block_task()
                    .await
            }
        });
        // 先等 prompt 受理(agent/chat 落点 = rid 已入在途表),再发
        // cancel —— 否则通知早于 set_active_request,会被静默 no-op
        // (时序竞态,非被测行为)。
        let rid = daemon2
            .wait_for_request("/api/v1/agent/chat", |b| b["session_id"] == SESSION_ID)
            .await["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        cx.send_notification(CancelNotification::new(sid.clone()))?;

        // cancel_chat 落点(请求体 rid 精确;谓词命中即已 POST)。
        let posted = daemon2
            .wait_for_request("/api/v1/cancel/cancel_chat", |b| b["request_id"] == rid)
            .await;
        assert_eq!(posted["request_id"], rid);
        daemon2.chat_event(
            &rid,
            SESSION_ID,
            chat_event_json(
                &rid,
                "done",
                json!({ "stop_reason": "cancelled", "usage": null }),
            ),
        );

        let resp = prompt_task.await.unwrap()?;
        assert_eq!(
            resp.stop_reason,
            agent_client_protocol::schema::v1::StopReason::Cancelled
        );
        Ok(())
    })
    .await;
}

/// queued 受理 → prompt 请求收到 JSON-RPC error(取舍 3)。
#[tokio::test]
async fn queued_acceptance_is_rejected() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post(
        "/api/v1/agent/chat",
        json!({ "status": "queued", "id": "q1", "position": 2 }),
    );

    run_client(&uri, ClientState::new(&[]), async move |cx| {
        let sid = init_and_new(&cx).await;
        let err = cx
            .send_request(PromptRequest::new(sid, text_prompt("hi")))
            .block_task()
            .await
            .expect_err("queued must be a JSON-RPC error");
        assert_eq!(err.code, ErrorCode::InvalidRequest);
        Ok(())
    })
    .await;
}

/// daemon 不可达:initialize 收到含 daemon.sh 指引的 error(进程不退出,
/// 连接保持可重试)。
#[tokio::test]
async fn daemon_unreachable_initialize_fails_with_guidance() {
    // 死端口:bind 后 drop,保证此刻无监听。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dead = format!("http://127.0.0.1:{port}");

    let agent = AcpAgent::new(McpServer::Stdio(
        McpServerStdio::new("everlasting-acp", SHIM_BIN)
            .env(vec![EnvVariable::new("EVERLASTING_ACP_DAEMON_URL", dead)]),
    ));
    let run = Client.builder().name("unreachable-test").connect_with(
        agent,
        async move |cx: ConnectionTo<agent_client_protocol::Agent>| {
            let err = cx
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await
                .expect_err("unreachable daemon must fail initialize");
            assert_eq!(err.code, ErrorCode::InternalError);
            assert!(err.message.contains("daemon.sh"), "指引缺失: {err}");
            Ok(())
        },
    );
    tokio::time::timeout(TEST_TIMEOUT, run)
        .await
        .expect("timed out")
        .expect("connection failed");
}

/// session/load 重放:messages → update 序列(user/agent/thinking/tool 两态)
/// **全部先于** load 响应到达(官方时序);cwd 不一致 → invalid_params。
#[tokio::test]
async fn load_session_replays_updates_before_response() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_load_session(json!([
        { "role": "user", "content": "第一问" },
        { "role": "assistant", "content": [
            { "type": "thinking", "thinking": "想一下" },
            { "type": "text", "text": "答一" },
            { "type": "tool_use", "id": "tu_1", "name": "shell", "input": { "command": "ls" } }
        ]},
        { "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "tu_1", "content": "out", "is_error": false }
        ]},
        { "role": "assistant", "content": "收尾" },
        { "role": "assistant", "content": [{ "type": "tool_result", "tool_use_id": "tu_x", "content": "bad", "is_error": true }] }
    ]));

    let state = ClientState::new(&[]);
    run_client(&uri, state.clone(), async move |cx| {
        let _ = init_and_new(&cx).await;
        let resp = cx
            .send_request(LoadSessionRequest::new(SESSION_ID, CWD))
            .block_task()
            .await
            .expect("load_session");

        // 重放先于响应:响应返回时通知已全部在手(state 由通知 handler
        // 同步收集;此处直接断言数量与序列)。
        let updates = state.updates();
        assert_eq!(updates.len(), 8, "重放序列: {updates:?}");
        let variants: Vec<&str> = updates
            .iter()
            .map(|u| match &u.update {
                SessionUpdate::UserMessageChunk(_) => "user_chunk",
                SessionUpdate::AgentMessageChunk(_) => "agent_chunk",
                SessionUpdate::AgentThoughtChunk(_) => "thought",
                SessionUpdate::ToolCall(_) => "tool_call",
                SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
                _ => "other",
            })
            .collect();
        assert_eq!(
            variants,
            vec![
                "user_chunk",       // user 文本
                "thought",          // thinking
                "agent_chunk",      // text
                "tool_call",        // tool_use(pending)
                "tool_call_update", // tool_use(completed + rawInput)
                "tool_call_update", // tool_result(completed + rawOutput)
                "agent_chunk",      // 收尾文本
                "tool_call_update", // is_error → failed
            ]
        );
        let _ = resp;
        Ok(())
    })
    .await;

    // cwd 不一致 → invalid_params。
    run_client(&uri, ClientState::new(&[]), async move |cx| {
        let _ = init_and_new(&cx).await;
        let err = cx
            .send_request(LoadSessionRequest::new(SESSION_ID, "/somewhere/else"))
            .block_task()
            .await
            .expect_err("cwd mismatch must fail");
        assert_eq!(err.code, ErrorCode::InvalidParams);
        Ok(())
    })
    .await;
}

/// set_mode 往返:daemon 落点(mode 归一)→ 响应 + current_mode_update 通知。
#[tokio::test]
async fn set_mode_roundtrip() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();
    daemon.on_post(
        "/api/v1/permissions/set_session_mode",
        json!({
            "id": SESSION_ID, "title": "t", "project_id": "p1",
            "current_cwd": CWD, "mode": "plan"
        }),
    );

    let state = ClientState::new(&[]);
    run_client(&uri, state.clone(), async move |cx| {
        let sid = init_and_new(&cx).await;
        let _ = cx
            .send_request(SetSessionModeRequest::new(sid, "plan"))
            .block_task()
            .await
            .expect("set_mode");
        Ok(())
    })
    .await;

    let posted = daemon
        .requests("/api/v1/permissions/set_session_mode")
        .into_iter()
        .find(|b| b["mode"] == "plan")
        .expect("set_session_mode posted with plan");
    assert_eq!(posted["session_id"], SESSION_ID);

    let updates = state.updates();
    assert!(updates.iter().any(|u| matches!(
        &u.update,
        SessionUpdate::CurrentModeUpdate(m) if m.current_mode_id.0.as_ref() == "plan"
    )));
}

/// list_projects 请求体归一锚(hidden:true 全量,跨进程实证)。
#[tokio::test]
async fn session_new_posts_hidden_inclusive_list() {
    let (daemon, uri) = FakeDaemon::spawn().await;
    daemon.mount_session_flow();

    run_client(&uri, ClientState::new(&[]), async move |cx| {
        let _ = init_and_new(&cx).await;
        Ok(())
    })
    .await;

    let listed = daemon
        .requests("/api/v1/projects/list_projects")
        .into_iter()
        .next()
        .expect("list_projects posted");
    assert_eq!(listed["filter"]["hidden"], true, "hidden 项目必须可命中");
}
