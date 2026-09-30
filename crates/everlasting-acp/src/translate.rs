//! 纯函数翻译层(daemon wire → ACP 协议类型)。全部无 IO,单测主战场。
//!
//! 本模块同时承载归一层 DTO(缺口 5):chat-event / tool 事件是 snake_case,
//! permission:ask 是 camelCase —— daemon 侧两种 casing 都是有意设计
//! (`ChatEvent` serde tag + `PermissionAskPayload` rename_all),shim 侧
//! 各自锚死,casing 回归由单测锁定。

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCall, ToolCallContent, ToolCallId, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind, UsageUpdate,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// daemon 入站 DTO(casing 锚死,缺口 5)
// ---------------------------------------------------------------------------

/// `chat-event` 的 ChatEvent 子集(serde `tag = "kind"`,snake_case,
/// `llm/types/event.rs:62-63`)。shim 只消费翻译表内的变体;`#[serde(other)]`
/// 兜底未知 kind(字段演进 additively,兜底分支防解析炸帧)。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEventDto {
    /// token 级文本流 → `agent_message_chunk`。
    Delta { text: String },
    /// thinking token 流 → `agent_thought_chunk`。
    ThinkingDelta { text: String },
    /// thinking 块签名:供下一轮回传 LLM 的不透明 blob,**客户端不可显示**,
    /// ACP 无承载面 → 忽略(翻译表注释,implement.md PR2)。
    SignatureDelta { signature: String },
    /// redacted thinking:同上,不可显示载荷 → 忽略。
    RedactedThinkingDelta { data: String },
    /// 流终态(`llm/types/event.rs:114-117`)。stop_reason 值域见
    /// [`stop_reason_to_acp`] 逐行表;`usage: null` 于 cancel/error 边缘。
    Done {
        stop_reason: Option<String>,
        usage: Option<TokenUsageDto>,
    },
    /// 流错误终态(`{message, category}`,snake_case category)。
    /// daemon 侧 error 后**不再发 done**(drive.rs:2041-2044 注释),
    /// 是独立终态 → ACP Refusal。
    Error { message: String, category: String },
    /// 每轮 token 观测(`event.rs:257-268`)→ `usage_update`。
    TurnUsage {
        request_id: String,
        seq: i64,
        run_id: String,
        usage: TokenUsageDto,
        context_window: u32,
    },
    /// 未知 kind 兜底(internally-tagged `#[serde(other)]`)。
    #[serde(other)]
    Other,
}

/// `TokenUsage`(`llm/types/usage.rs:59-66`,五字段全 int)。
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TokenUsageDto {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub context_input_tokens: u32,
}

/// `tool:call` payload(`state.rs:770-776`,snake_case)。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ToolCallPayloadDto {
    pub request_id: String,
    pub session_id: String,
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// `tool:result` payload(`state.rs:786-794`,snake_case;`images` shim 不消费)。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ToolResultPayloadDto {
    pub request_id: String,
    pub session_id: String,
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// `permission:ask` payload —— **camelCase**(`agent/permissions/payload.rs:20-62`,
/// `rename_all = "camelCase"`)。PR3 反向请求环消费;PR2 仅立形。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskDto {
    pub rid: String,
    pub session_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub risk: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub worker_run_id: Option<String>,
    #[serde(default)]
    pub grant_pattern: Option<String>,
}

// ---------------------------------------------------------------------------
// 值域表 1:daemon done.stop_reason → ACP StopReason
// ---------------------------------------------------------------------------
// daemon 侧生产点全量枚举(SSE chat-event 可见终态,实读源码 2026-09-30):
//   - agent loop 终态:`agent/chat_loop.rs:676`(cancelled|end_turn)、
//     `chat_loop.rs:1049`(max_turns,worker 档 skip_persist 不上 SSE)、
//     `chat_loop/drive.rs:2033`(cancel 收尾)、`drive.rs:2399/2593`(C2+
//     干预问询的取消臂)、`drive.rs:2283/2509/2553`(loop_terminated:worker
//     直断 + 主 loop 用户「终止 loop」)、`drive.rs:2161`(透传 provider
//     bookkeeping 值,Option 可为 None);
//   - provider 归一值:openai `finish_reason` → anthropic 风格
//     (`llm/provider/openai.rs:834-839`:stop→end_turn、length→max_tokens、
//     tool_calls→tool_use、**其余原样透传** —— content_filter 等直达 shim);
//     anthropic `message_delta.stop_reason` 原样透传
//     (`anthropic/events.rs:313-315`,可见 end_turn/max_tokens/stop_sequence/
//     refusal/tool_use);
//   - `kind=error` 是独立终态,daemon 不再补 done(`drive.rs:2041-2048`
//     注释),→ ACP Refusal,不走本表。
// ACP StopReason 值域(schema v1 agent.rs:3335-3354):EndTurn / MaxTokens /
// MaxTurnRequests / Refusal / Cancelled。

/// daemon stop_reason → ACP StopReason。逐行依据见上表与行内注释;
/// 未知新值(daemon additively 演进)按 EndTurn 收束 + warn,不击穿协议。
pub fn stop_reason_to_acp(stop_reason: Option<&str>) -> StopReason {
    match stop_reason {
        // 正常结束,同义直映。
        Some("end_turn") => StopReason::EndTurn,
        // ACP 规定 client cancel 后 MUST 返回 Cancelled;daemon 用户 Stop
        // / cancel_chat 路径同义。
        Some("cancelled") => StopReason::Cancelled,
        // 轮数上限 softcap = 「maximum number of allowed agent requests
        // between user turns」的 daemon 实现,语义一一对应。
        Some("max_turns") => StopReason::MaxTurnRequests,
        // loop intervention 终态(非模型拒答,是系统停机闸):归「次数上限」
        // 语义族,不归 Refusal(那是给 error/拒答的)。
        Some("loop_terminated") => StopReason::MaxTurnRequests,
        // provider token 上限,同义直映。
        Some("max_tokens") => StopReason::MaxTokens,
        // daemon 多工具轮内层 LLM 调用的中间 finish(normalized "tool_calls")。
        // 它出现在终态说明该轮 LLM 输出以工具调用收束、turn 整体结束
        // (工具结果已落库);ACP 无 tool_use 变体,按正常收束处理。
        Some("tool_use") => StopReason::EndTurn,
        // anthropic stop_sequence:序列停止 = 正常收束。
        Some("stop_sequence") => StopReason::EndTurn,
        // openai normalize 未覆盖的 finish_reason 原样透传(`other => other`),
        // content_filter = provider 内容闸拦停输出,与 refusal 同语义族。
        Some("content_filter") => StopReason::Refusal,
        // provider 级 refusal,同义。
        Some("refusal") => StopReason::Refusal,
        // None:provider 网络断边缘无 reason(drive.rs:2161 透传 bookkeeping
        // 初值)。turn 已终止,按正常收束;真错误走 kind=error 独立终态。
        None => StopReason::EndTurn,
        // 防御臂:daemon 未来新增值不击穿 ACP,log 后按正常收束。
        Some(other) => {
            tracing::warn!(
                stop_reason = other,
                "unmapped daemon stop_reason, defaulting to EndTurn"
            );
            StopReason::EndTurn
        }
    }
}

// ---------------------------------------------------------------------------
// 值域表 2:daemon mode → ACP SessionModeId(字符串直映,三档)
// ---------------------------------------------------------------------------

/// daemon `set_session_mode` 值域(edit|plan|yolo,lenient:未知回退 edit,
/// 与 daemon `commands/permissions.rs` 行为对齐)。`session/set_mode`
/// 与 mode 声明共用。
pub fn normalize_mode(mode: &str) -> &'static str {
    match mode {
        "plan" => "plan",
        "yolo" => "yolo",
        // edit + 未知/历史脏值:daemon 侧 lenient parse 同款回退。
        _ => "edit",
    }
}

// ---------------------------------------------------------------------------
// 值域表 3:daemon 工具名 → ACP ToolKind(图标/UI 分类;Other 兜底)
// ---------------------------------------------------------------------------

/// daemon 工具名 → ACP ToolKind。daemon 工具集硬编码表,未知工具 Other。
pub fn tool_kind(name: &str) -> ToolKind {
    match name {
        "read_file" | "list_dir" | "grep" | "glob" => ToolKind::Read,
        "write_file" | "edit_file" => ToolKind::Edit,
        "shell" => ToolKind::Execute,
        "web_fetch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

// ---------------------------------------------------------------------------
// ChatEvent → session/update 翻译
// ---------------------------------------------------------------------------

/// 单条 daemon 事件的翻译产物:发给客户端的 update 通知序列,或 turn 终态,
/// 或无需动作。终态由 prompt 消费循环 respond,不产生 update。
#[derive(Debug, PartialEq)]
pub enum Translated {
    /// 逐条发 `session/update` 通知(可能多条,当前恒 ≤1)。
    Updates(Vec<SessionNotification>),
    /// turn 终态:prompt 请求以此 StopReason respond。
    Terminal(StopReason),
    /// 忽略(signature_delta / redacted / 未知 kind 等)。
    Ignored,
}

/// 翻译一条归一后的 chat-event(session_id 由通知构造注入)。
pub fn translate_chat_event(session_id: &str, event: ChatEventDto) -> Translated {
    let update = match event {
        ChatEventDto::Delta { text } => text_chunk(SessionUpdate::AgentMessageChunk, text),
        ChatEventDto::ThinkingDelta { text } => text_chunk(SessionUpdate::AgentThoughtChunk, text),
        // 签名 / redacted thinking:回传 LLM 用的不透明 blob,ACP 无承载面,
        // 静默忽略(保留在 DTO 是为了显式枚举 daemon 值域)。
        ChatEventDto::SignatureDelta { .. } | ChatEventDto::RedactedThinkingDelta { .. } => {
            return Translated::Ignored;
        }
        ChatEventDto::Done { stop_reason, .. } => {
            return Translated::Terminal(stop_reason_to_acp(stop_reason.as_deref()));
        }
        // error 是独立终态(daemon 不再补 done):ACP 以 Refusal 收束。
        ChatEventDto::Error { message, .. } => {
            tracing::warn!(message, "daemon turn errored, responding with refusal");
            return Translated::Terminal(StopReason::Refusal);
        }
        ChatEventDto::TurnUsage {
            usage,
            context_window,
            ..
        } => SessionUpdate::UsageUpdate(UsageUpdate::new(
            u64::from(usage.context_input_tokens),
            u64::from(context_window),
        )),
        // Speaker / Start / Retrying / TurnComplete 等无 ACP 承载面的变体
        // 都落在 Other(DTO 兜底),不产 update。
        ChatEventDto::Other => return Translated::Ignored,
    };
    Translated::Updates(vec![SessionNotification::new(
        session_id.to_string(),
        update,
    )])
}

fn text_chunk(variant: impl Fn(ContentChunk) -> SessionUpdate, text: String) -> SessionUpdate {
    // ContentChunk 是 non_exhaustive,只能经 new 构造。
    variant(ContentChunk::new(ContentBlock::Text(TextContent::new(
        text,
    ))))
}

// ---------------------------------------------------------------------------
// tool:call / tool:result → ToolCall(pending) / ToolCallUpdate
// ---------------------------------------------------------------------------

/// `tool:call`(终态一次性)→ ACP `tool_call`。状态取 pending(daemon 无
/// in_progress 事件源,调研报告 §2.1 —— 两态跳变 pending→completed 是协议
/// 内合法路径,schema `ToolCallStatus` 文档明示 pending 覆盖「待审批」态);
/// rawInput 原样携带,content 留空由 tool_call_update 补。
pub fn translate_tool_call(payload: ToolCallPayloadDto) -> SessionNotification {
    SessionNotification::new(
        payload.session_id.clone(),
        SessionUpdate::ToolCall(
            ToolCall::new(
                ToolCallId::new(payload.id.as_str()),
                tool_title(&payload.name, &payload.input),
            )
            .kind(tool_kind(&payload.name))
            .raw_input(payload.input),
        ),
    )
}

/// `tool:result` → ACP `tool_call_update`:completed/failed 两态跳变;
/// content(替换语义)带结果文本,rawOutput 同文(daemon 的 content 就是
/// 文本聚合,`state.rs:786-794`;rawOutput 包成 JSON string 保形)。
pub fn translate_tool_result(payload: ToolResultPayloadDto) -> SessionNotification {
    let status = if payload.is_error {
        ToolCallStatus::Failed
    } else {
        ToolCallStatus::Completed
    };
    SessionNotification::new(
        payload.session_id,
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            ToolCallId::new(payload.tool_use_id.as_str()),
            ToolCallUpdateFields::new()
                .status(status)
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new(payload.content.clone()),
                ))])
                .raw_output(serde_json::Value::String(payload.content)),
        )),
    )
}

/// tool 卡片标题:`name + input 摘要`(shell 取 command,路径类工具取 path,
/// 其余 JSON 截断)。daemon 无 title 源,shim 本地合成。
pub fn tool_title(name: &str, input: &serde_json::Value) -> String {
    let detail = match name {
        "shell" => input.get("command").and_then(|v| v.as_str()),
        "read_file" | "write_file" | "edit_file" | "list_dir" => {
            input.get("path").and_then(|v| v.as_str())
        }
        "grep" | "glob" => input.get("pattern").and_then(|v| v.as_str()),
        "web_fetch" => input.get("url").and_then(|v| v.as_str()),
        _ => None,
    };
    match detail {
        Some(detail) => format!("{name} {}", truncate(detail, 120)),
        None => format!("{name} {}", truncate(&input.to_string(), 120)),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::SessionUpdate;

    /// 值域表 1 全表断言:daemon 每个可见 stop_reason → ACP 变体逐行锁死。
    /// 新增 daemon 值时此表即评审清单。
    #[test]
    fn stop_reason_table_is_exhaustive() {
        use StopReason::*;
        let table: &[(&str, StopReason)] = &[
            ("end_turn", EndTurn),
            ("cancelled", Cancelled),
            ("max_turns", MaxTurnRequests),
            ("loop_terminated", MaxTurnRequests),
            ("max_tokens", MaxTokens),
            ("tool_use", EndTurn),
            ("stop_sequence", EndTurn),
            ("content_filter", Refusal),
            ("refusal", Refusal),
        ];
        for (daemon, expected) in table {
            assert_eq!(stop_reason_to_acp(Some(daemon)), *expected, "{daemon}");
        }
        assert_eq!(stop_reason_to_acp(None), EndTurn);
        // 未知新值:防御臂 → EndTurn。
        assert_eq!(stop_reason_to_acp(Some("some_future_value")), EndTurn);
    }

    /// 值域表 2:mode 三档 + lenient 回退。
    #[test]
    fn mode_table() {
        assert_eq!(normalize_mode("edit"), "edit");
        assert_eq!(normalize_mode("plan"), "plan");
        assert_eq!(normalize_mode("yolo"), "yolo");
        // 未知/历史脏值回退 edit(daemon lenient parse 同款)。
        assert_eq!(normalize_mode("chat"), "edit");
        assert_eq!(normalize_mode(""), "edit");
    }

    /// 值域表 3:工具名分类,未知兜底 Other。
    #[test]
    fn tool_kind_table() {
        assert_eq!(tool_kind("read_file"), ToolKind::Read);
        assert_eq!(tool_kind("grep"), ToolKind::Read);
        assert_eq!(tool_kind("write_file"), ToolKind::Edit);
        assert_eq!(tool_kind("edit_file"), ToolKind::Edit);
        assert_eq!(tool_kind("shell"), ToolKind::Execute);
        assert_eq!(tool_kind("web_fetch"), ToolKind::Fetch);
        assert_eq!(tool_kind("some_future_tool"), ToolKind::Other);
    }

    /// delta → agent_message_chunk,thinking_delta → agent_thought_chunk,
    /// session_id 注入正确。
    #[test]
    fn translates_delta_and_thinking_to_chunks() {
        let Translated::Updates(updates) = translate_chat_event(
            "s1",
            ChatEventDto::Delta {
                text: "你好".into(),
            },
        ) else {
            panic!("expected updates");
        };
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].session_id.0.as_ref(), "s1");
        assert!(matches!(
            &updates[0].update,
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(t),
                ..
            }) if t.text == "你好"
        ));

        let Translated::Updates(updates) = translate_chat_event(
            "s1",
            ChatEventDto::ThinkingDelta {
                text: "thinking…".into(),
            },
        ) else {
            panic!("expected updates");
        };
        assert!(matches!(
            &updates[0].update,
            SessionUpdate::AgentThoughtChunk(_)
        ));
    }

    /// signature/redacted 忽略;未知 kind 忽略。
    #[test]
    fn ignores_non_displayable_and_unknown_kinds() {
        for event in [
            ChatEventDto::SignatureDelta {
                signature: "blob".into(),
            },
            ChatEventDto::RedactedThinkingDelta {
                data: "blob".into(),
            },
            ChatEventDto::Other,
        ] {
            assert_eq!(translate_chat_event("s1", event), Translated::Ignored);
        }
    }

    /// done → Terminal 映射;error → Refusal。
    #[test]
    fn done_and_error_are_terminals() {
        assert_eq!(
            translate_chat_event(
                "s1",
                ChatEventDto::Done {
                    stop_reason: Some("cancelled".into()),
                    usage: None,
                }
            ),
            Translated::Terminal(StopReason::Cancelled)
        );
        assert_eq!(
            translate_chat_event(
                "s1",
                ChatEventDto::Error {
                    message: "boom".into(),
                    category: "network".into(),
                }
            ),
            Translated::Terminal(StopReason::Refusal)
        );
    }

    /// turn_usage → usage_update(used=context_input,size=context_window)。
    #[test]
    fn turn_usage_maps_to_usage_update() {
        let Translated::Updates(updates) = translate_chat_event(
            "s1",
            ChatEventDto::TurnUsage {
                request_id: "r1".into(),
                seq: 1,
                run_id: String::new(),
                usage: TokenUsageDto {
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                    context_input_tokens: 777,
                },
                context_window: 128_000,
            },
        ) else {
            panic!("expected updates");
        };
        let SessionUpdate::UsageUpdate(u) = &updates[0].update else {
            panic!("expected usage_update");
        };
        assert_eq!(u.used, 777);
        assert_eq!(u.size, 128_000);
    }

    /// tool:call → ToolCall(pending,kind/rawInput);title 合成截断。
    #[test]
    fn tool_call_maps_to_pending_with_kind_and_raw_input() {
        let notif = translate_tool_call(ToolCallPayloadDto {
            request_id: "r1".into(),
            session_id: "s1".into(),
            id: "tu_1".into(),
            name: "shell".into(),
            input: serde_json::json!({ "command": "cargo test" }),
        });
        let SessionUpdate::ToolCall(call) = &notif.update else {
            panic!("expected tool_call");
        };
        assert_eq!(call.tool_call_id.0.as_ref(), "tu_1");
        assert_eq!(call.kind, ToolKind::Execute);
        // pending 是序列化默认值(不上 wire),内存态仍是 Pending。
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert_eq!(
            call.raw_input.as_ref(),
            Some(&serde_json::json!({ "command": "cargo test" }))
        );
        assert_eq!(call.title, "shell cargo test");

        // 路径类工具 title 取 path;未知字段退回整 JSON 截断。
        let t = tool_title("read_file", &serde_json::json!({ "path": "/a/b.txt" }));
        assert_eq!(t, "read_file /a/b.txt");
        let t = tool_title("weird", &serde_json::json!({ "x": 1 }));
        assert!(t.starts_with("weird {"));
    }

    /// tool:result → ToolCallUpdate(completed/failed + content + rawOutput)。
    #[test]
    fn tool_result_maps_to_completed_or_failed_update() {
        let ok = translate_tool_result(ToolResultPayloadDto {
            request_id: "r1".into(),
            session_id: "s1".into(),
            tool_use_id: "tu_1".into(),
            content: "file content".into(),
            is_error: false,
        });
        let SessionUpdate::ToolCallUpdate(u) = &ok.update else {
            panic!("expected tool_call_update");
        };
        assert_eq!(u.tool_call_id.0.as_ref(), "tu_1");
        assert_eq!(u.fields.status, Some(ToolCallStatus::Completed));
        assert!(u
            .fields
            .raw_output
            .as_ref()
            .is_some_and(|v| v == "file content"));

        let failed = translate_tool_result(ToolResultPayloadDto {
            request_id: "r1".into(),
            session_id: "s1".into(),
            tool_use_id: "tu_2".into(),
            content: "exit 1".into(),
            is_error: true,
        });
        let SessionUpdate::ToolCallUpdate(u) = &failed.update else {
            panic!("expected tool_call_update");
        };
        assert_eq!(u.fields.status, Some(ToolCallStatus::Failed));
    }

    /// casing 锚(缺口 5):chat-event DTO snake_case 解析(tag 判别)。
    #[test]
    fn chat_event_dto_parses_snake_case_wire() {
        let dto: ChatEventDto = serde_json::from_str(r#"{"kind":"delta","text":"x"}"#).unwrap();
        assert_eq!(dto, ChatEventDto::Delta { text: "x".into() });

        let dto: ChatEventDto =
            serde_json::from_str(r#"{"kind":"thinking_delta","text":"t"}"#).unwrap();
        assert_eq!(dto, ChatEventDto::ThinkingDelta { text: "t".into() });

        // 未知 kind 兜底不炸(additively 演进安全)。
        let dto: ChatEventDto = serde_json::from_str(r#"{"kind":"future_kind","x":1}"#).unwrap();
        assert_eq!(dto, ChatEventDto::Other);
    }

    /// casing 锚:permission:ask 是 **camelCase**(daemon 特例,payload.rs:21)。
    #[test]
    fn permission_ask_dto_parses_camel_case_wire() {
        let dto: PermissionAskDto = serde_json::from_str(
            r#"{"rid":"r1","sessionId":"s1","toolUseId":"tu_1","toolName":"shell",
                "toolInput":{"command":"rm"},"risk":"high","workerRunId":"w1"}"#,
        )
        .unwrap();
        assert_eq!(dto.session_id, "s1");
        assert_eq!(dto.tool_use_id, "tu_1");
        assert_eq!(dto.worker_run_id.as_deref(), Some("w1"));
        assert_eq!(dto.reason, None, "缺省可选字段容错");
    }

    /// casing 锚:tool 事件 DTO snake_case(toolUseId 而非 tool_use_id 会解析失败)。
    #[test]
    fn tool_payload_dtos_parse_snake_case_wire() {
        let dto: ToolResultPayloadDto = serde_json::from_str(
            r#"{"request_id":"r1","session_id":"s1","tool_use_id":"tu_1",
                "content":"c","is_error":true}"#,
        )
        .unwrap();
        assert_eq!(dto.tool_use_id, "tu_1");
        assert!(dto.is_error);

        let dto: ToolCallPayloadDto = serde_json::from_str(
            r#"{"request_id":"r1","session_id":"s1","id":"tu_1","name":"shell","input":{}}"#,
        )
        .unwrap();
        assert_eq!(dto.name, "shell");
    }
}
