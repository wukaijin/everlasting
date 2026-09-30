//! 权限桥:daemon `permission:ask`(SSE)↔ ACP `session/request_permission`
//! 反向请求环(design §4「权限环」)。
//!
//! 环路:ask → 组反向请求(三固定选项)→ `ConnectionTo::send_request` 等客户端
//! optionId → 解码回 daemon decision(**reject 档的 optionId 是 `reject_once`,
//! 应答解码回 `deny`,一个 rename 点**)→ `POST permission_response` →
//! resolved 校验(false 不重试)。
//!
//! 并发语义(任务约定,勿特判):
//! - Selected 应答**一律照发** —— 包括 cancel turn 之后的迟到应答:daemon
//!   侧 rid 失效返回 `resolved:false`,自然收敛;
//! - 客户端以 `cancelled` outcome 应答(协议规定 turn cancel 时 MUST)→ 无
//!   optionId 可解码,不发 —— daemon 侧 ask 超时收束(默认 120s,
//!   permission-layer.md §7;`ask_no_timeout` 开启时无超时,兜底转为 GUI 侧
//!   Stop / 删会话 —— 与 GUI 客户端断连后在途 ask 的处境同款);
//! - 反向请求连接错误 → 不重试,同样超时兜底。

use std::future::Future;
use std::pin::Pin;

use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ToolCall, ToolCallId, ToolCallUpdate,
};
use agent_client_protocol::{Client, ConnectionTo, Error};

use crate::daemon::DaemonClient;
use crate::translate::{tool_kind, tool_title, PermissionAskDto};

/// 反向请求出口。真实现 = `ConnectionTo<Client>`(`send_request` +
/// `block_task` —— 后者**只在 `ConnectionTo::spawn` 的任务内安全**,权限环
/// 正是从 prompt 消费循环 spawn 出去的);测试用 [`ScriptedOutbound`]。
pub(crate) trait PermissionOutbound: Send + Sync {
    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, Error>> + Send>>;
}

impl PermissionOutbound for ConnectionTo<Client> {
    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, Error>> + Send>> {
        // clone 出 owned 句柄再进 async block(ConnectionTo 廉价 clone),
        // 返回的 future 才是 'static。
        let this = self.clone();
        Box::pin(async move { this.send_request(request).block_task().await })
    }
}

/// 三固定选项。optionId 直接用 daemon decision 值(allow_once/allow_always),
/// reject 档 optionId 为 `reject_once`(daemon 值域没有 reject 持久档,固定
/// 单档;kind 用 RejectOnce 作纯 UI 提示),应答解码回 `deny` —— rename 点。
/// name 与 daemon GUI 权限卡用词一致(仅一次/始终允许/拒绝)。
pub(crate) fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption::new("allow_once", "允许一次", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            "allow_always",
            "始终允许",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("reject_once", "拒绝", PermissionOptionKind::RejectOnce),
    ]
}

/// optionId → daemon decision。未知 optionId(客户端实现漂移)一律 `deny`
/// —— deny 是安全默认,与 evl 键位语义一致(非 y/a 即 deny)。
pub(crate) fn option_id_to_decision(option_id: &str) -> &'static str {
    match option_id {
        "allow_once" => "allow_once",
        "allow_always" => "allow_always",
        _ => "deny",
    }
}

/// ask DTO → 反向请求的 tool_call 信息(daemon 无 title/kind 源,复用
/// translate 的本地合成;pending 状态由 ToolCallUpdate 默认携带)。
pub(crate) fn ask_to_tool_call_update(ask: &PermissionAskDto) -> ToolCallUpdate {
    ToolCallUpdate::from(
        ToolCall::new(
            ToolCallId::new(ask.tool_use_id.as_str()),
            tool_title(&ask.tool_name, &ask.tool_input),
        )
        .kind(tool_kind(&ask.tool_name))
        .raw_input(ask.tool_input.clone()),
    )
}

/// 单个 ask 的完整环。**吞掉一切自身错误**(log only)——权限环挂在 spawn
/// 任务里,任何 Err 都不能炸掉连接;daemon 侧 ask 超时是最终兜底(默认
/// 120s,`ask_no_timeout` 见模块文档)。
pub(crate) async fn resolve_ask<O: PermissionOutbound>(
    ask: PermissionAskDto,
    outbound: &O,
    daemon: &DaemonClient,
) {
    let request = RequestPermissionRequest::new(
        // SessionId 的 From 只有 Arc<str>/String/&'static str,非静态 &str
        // 不行 —— 传 owned String。
        ask.session_id.clone(),
        ask_to_tool_call_update(&ask),
        permission_options(),
    );
    let response = match outbound.request_permission(request).await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                ask_rid = %ask.rid,
                tool = %ask.tool_name,
                error = %err,
                "permission reverse request failed (connection closing?); daemon ask timeout applies"
            );
            return;
        }
    };
    match response.outcome {
        // 协议规定:client 在 turn cancel 时 MUST 用 cancelled 应答在途
        // request_permission。无 optionId 可解码,不发 permission_response
        // —— turn 已取消,daemon 侧 ask 超时收束(兜底口径见模块文档;
        // implement.md PR3:客户端取消 → 不重试)。
        RequestPermissionOutcome::Cancelled => {
            tracing::info!(
                ask_rid = %ask.rid,
                "client cancelled the permission request (turn cancelled); leaving the daemon ask to its timeout"
            );
        }
        // Selected 一律照发 —— 包括 cancel 后的迟到应答:daemon 侧 rid
        // 失效返回 resolved:false,自然收敛,不特判(任务约定)。
        RequestPermissionOutcome::Selected(selected) => {
            let decision = option_id_to_decision(selected.option_id.0.as_ref());
            match daemon.permission_response(&ask.rid, decision).await {
                Ok(true) => {
                    tracing::info!(ask_rid = %ask.rid, decision, "permission resolved")
                }
                Ok(false) => tracing::warn!(
                    ask_rid = %ask.rid,
                    decision,
                    "permission_response resolved:false (rid unknown/stale); not retrying"
                ),
                Err(err) => tracing::warn!(
                    ask_rid = %ask.rid,
                    decision,
                    error = %err,
                    "permission_response post failed; daemon ask timeout applies"
                ),
            }
        }
        // non_exhaustive 枚举的防御臂:协议未来新增 outcome 按「无 optionId
        // 可解码」处理 —— 不落 permission_response,超时兜底。
        other => tracing::warn!(
            ask_rid = %ask.rid,
            outcome = ?other,
            "unknown permission outcome; not posting permission_response"
        ),
    }
}

#[cfg(test)]
use agent_client_protocol::schema::v1::SelectedPermissionOutcome;
#[cfg(test)]
use std::collections::VecDeque;
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
pub(crate) fn selected_outcome(option_id: &str) -> RequestPermissionResponse {
    // PermissionOptionId 的 From 只有 &'static str/String,运行时串传 owned。
    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
        SelectedPermissionOutcome::new(option_id.to_string()),
    ))
}

/// 测试用脚本化反向请求出口:按序弹出脚本(应答 / 模拟连接错误),
/// 记录收到的反向请求供断言。Clone 共享同一份脚本与记录。
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ScriptedOutbound {
    scripts: Arc<Mutex<VecDeque<Script>>>,
    calls: Arc<Mutex<Vec<RequestPermissionRequest>>>,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) enum Script {
    /// 客户端选中某 optionId。
    Select(&'static str),
    /// 客户端以 cancelled outcome 应答(turn 取消路径)。
    Cancelled,
    /// 模拟连接死亡(send_request/block_task Err)。
    Fail,
}

#[cfg(test)]
impl ScriptedOutbound {
    pub(crate) fn new(scripts: impl IntoIterator<Item = Script>) -> Self {
        Self {
            scripts: Arc::new(Mutex::new(scripts.into_iter().collect())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn calls(&self) -> Vec<RequestPermissionRequest> {
        self.calls.lock().expect("scripted calls lock").clone()
    }
}

#[cfg(test)]
impl PermissionOutbound for ScriptedOutbound {
    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, Error>> + Send>> {
        self.calls
            .lock()
            .expect("scripted calls lock")
            .push(request);
        let script = self
            .scripts
            .lock()
            .expect("scripted scripts lock")
            .pop_front();
        Box::pin(async move {
            match script {
                Some(Script::Select(option_id)) => Ok(selected_outcome(option_id)),
                Some(Script::Cancelled) => Ok(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                )),
                Some(Script::Fail) | None => {
                    Err(Error::internal_error().data("scripted connection failure"))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::PermissionAskDto;
    use std::time::Duration;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockGuard, MockServer, ResponseTemplate};

    fn ask() -> PermissionAskDto {
        serde_json::from_str(
            r#"{"rid":"r1","sessionId":"s1","toolUseId":"tu_1","toolName":"shell",
                "toolInput":{"command":"ls"},"risk":"medium"}"#,
        )
        .unwrap()
    }

    /// 挂一个 scoped permission_response mock(带可选 decision 精确匹配),
    /// 返回 guard 供命中次数断言。scoped = server drop 时未满足期望即红。
    async fn server_with_response(
        decision: Option<&str>,
        body: serde_json::Value,
    ) -> (MockServer, MockGuard) {
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

    async fn wait_for_hits(guard: &MockGuard, expected: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while guard.received_requests().await.len() < expected {
            assert!(
                tokio::time::Instant::now() < deadline,
                "permission_response not delivered in time"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// rename 点锚:optionId `reject_once` → decision `deny`;
    /// allow 两档同名直映;未知 optionId → deny(安全默认)。
    #[test]
    fn option_id_to_decision_table() {
        assert_eq!(option_id_to_decision("allow_once"), "allow_once");
        assert_eq!(option_id_to_decision("allow_always"), "allow_always");
        assert_eq!(option_id_to_decision("reject_once"), "deny");
        assert_eq!(option_id_to_decision("whatever"), "deny");
    }

    /// ask → 反向请求的 tool_call 信息:toolCallId=toolUseId;三选项固定,
    /// optionId 锚死(allow 同名直映、reject_once 是 rename 点)。
    #[test]
    fn ask_maps_to_tool_call_update_with_fixed_options() {
        let update = ask_to_tool_call_update(&ask());
        assert_eq!(update.tool_call_id.0.as_ref(), "tu_1");

        let options = permission_options();
        let ids: Vec<&str> = options.iter().map(|o| o.option_id.0.as_ref()).collect();
        assert_eq!(ids, ["allow_once", "allow_always", "reject_once"]);
    }

    /// 环路三态 × wiremock 落点:allow_once / allow_always / reject→deny,
    /// 请求体精确锚 {rid, decision},各恰好一次(不重试)。
    #[tokio::test]
    async fn bridge_posts_permission_response_for_selected_options() {
        for (option_id, expected_decision) in [
            ("allow_once", "allow_once"),
            ("allow_always", "allow_always"),
            ("reject_once", "deny"),
        ] {
            let (server, guard) =
                server_with_response(Some(expected_decision), serde_json::json!(true)).await;
            let outbound = ScriptedOutbound::new([Script::Select(option_id)]);
            resolve_ask(ask(), &outbound, &DaemonClient::new(server.uri()).unwrap()).await;

            wait_for_hits(&guard, 1).await;
            assert_eq!(outbound.calls().len(), 1, "{option_id}");
        }
    }

    /// resolved:false(rid 未知/超时)→ 只发一次,不重试。
    #[tokio::test]
    async fn bridge_does_not_retry_on_resolved_false() {
        let (server, guard) = server_with_response(None, serde_json::json!(false)).await;
        let outbound = ScriptedOutbound::new([Script::Select("allow_once")]);
        resolve_ask(ask(), &outbound, &DaemonClient::new(server.uri()).unwrap()).await;
        wait_for_hits(&guard, 1).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            guard.received_requests().await.len(),
            1,
            "resolved:false 不重试"
        );
    }

    /// 客户端 cancelled 应答(turn 取消)→ 不发 permission_response。
    #[tokio::test]
    async fn bridge_skips_post_on_client_cancelled_outcome() {
        let (server, guard) = server_with_response(None, serde_json::json!(true)).await;
        let outbound = ScriptedOutbound::new([Script::Cancelled]);
        resolve_ask(ask(), &outbound, &DaemonClient::new(server.uri()).unwrap()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(
            guard.received_requests().await.len(),
            0,
            "cancelled outcome 不落 permission_response"
        );
    }

    /// 反向请求连接错误 → 不崩、不重试、不 POST(shim 存活,daemon 超时兜底)。
    #[tokio::test]
    async fn bridge_survives_outbound_connection_failure() {
        let (server, guard) = server_with_response(None, serde_json::json!(true)).await;
        let outbound = ScriptedOutbound::new([Script::Fail]);
        resolve_ask(ask(), &outbound, &DaemonClient::new(server.uri()).unwrap()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(guard.received_requests().await.len(), 0);
        assert_eq!(outbound.calls().len(), 1, "反向请求确实发过");
    }

    /// 反向请求面锚:sessionId/toolCall/options 组装正确。
    #[tokio::test]
    async fn bridge_sends_well_formed_reverse_request() {
        let (server, guard) =
            server_with_response(Some("allow_once"), serde_json::json!(true)).await;
        let outbound = ScriptedOutbound::new([Script::Select("allow_once")]);
        resolve_ask(ask(), &outbound, &DaemonClient::new(server.uri()).unwrap()).await;
        wait_for_hits(&guard, 1).await;

        let calls = outbound.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].session_id.0.as_ref(), "s1");
        assert_eq!(calls[0].tool_call.tool_call_id.0.as_ref(), "tu_1");
        assert_eq!(calls[0].options.len(), 3);
    }
}
