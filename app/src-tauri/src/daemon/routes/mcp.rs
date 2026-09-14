//! `/mcp` — MCP streamable-HTTP endpoint(GCE 收敛,任务
//! 09-14-gce-mcp-daemon-converge;roadmap §5 path③)。
//!
//! 极简无状态 profile(契约反向提取自 @modelcontextprotocol/sdk 1.30.0,
//! 见任务 research/mcp-wire-protocol.md):
//! - `POST /mcp`:JSON-RPC 2.0(单条或批);请求 → 200 + 纯 JSON 响应;
//!   纯通知 → 202 空 body;Accept/Content-Type 校验 406/415。
//! - `GET /mcp` → 405(不开服务端主动流;SDK 客户端视为预期)。
//! - `DELETE /mcp` → 200 no-op(无 session 可终结)。
//! - 不分配 `mcp-session-id`(无状态,免 404 面);`mcp-protocol-version`
//!   头 lenient 不校验只记日志(前向兼容宿主新版本)。
//! - 工具执行错误不走 JSON-RPC error:200 + `isError:true` text result
//!   (JS 壳 errorResult 同款,宿主把它呈现给模型而非判定协议故障)。
//!
//! 八工具语义 1:1 平移自已退役的 stdio 壳 `scripts/group-chat-mcp.mjs`
//! (2026-09-15 P4 随挂载切 HTTP 删除源,本文即唯一实现;映射表见任务
//! 09-14-gce-mcp-daemon-converge 的 research/daemon-converge.md §2)。
//! 编排原语全部走 daemon 内部 `*_inner`(Q0 单源),不经 HTTP 自绕。
//!
//! 安全边界:继承 daemon 全 API 零鉴权本机前提(docs/DAEMON-API.md §8)。
//! **远程暴露(remote tunnel 转发本路由)须先过安全评审**——roadmap §5
//! 的立项前置,本期明确不做。
//!
//! 内置四档预设:编译期 `include_str!` 嵌入 `scripts/group-chat-presets.json`
//! (单一事实源保持——改 JSON 需重编译 daemon 才生效,M1 CLI 读文件路径
//! 不变;两消费方共享同一文件)。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::chat::{chat_inner, ChatAcceptance, ChatEntry};
use crate::agent::group_chat_transcript::{render_scheduled_transcript, TranscriptRenderArgs};
use crate::agent::subagent::SubagentEventSink;
use crate::commands::cancel::{cancel_chat_inner, preempt_group_chat_inner};
use crate::commands::projects::{create_project_inner, list_projects_inner, ListProjectsFilter};
use crate::commands::sessions::create_session_in_pool;
use crate::daemon::sse::{HttpSseSink, HttpSseSubagentSink};
use crate::db;
use crate::error::AppCommandError;
use crate::llm::types::{ChatMessage, MessageContent, Role};
use crate::state::{AppState, ChatEventSink};

// ---------------------------------------------------------------------------
// 协议层常量与路由
// ---------------------------------------------------------------------------

/// 我方接受的协议版本集(= SDK 1.30.0 SUPPORTED_PROTOCOL_VERSIONS;
/// echo 策略:客户端请求的版本它自己必然支持,跨宿主 SDK 版本最稳)。
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = [
    "2024-10-07",
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
];
/// 请求版本不在支持集时的回退(SDK DEFAULT_NEGOTIATED_PROTOCOL_VERSION)。
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

const SERVER_NAME: &str = "everlasting-group-chat";
const SERVER_VERSION: &str = "1.0.0";

const ERR_PARSE: i64 = -32700;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;
const ERR_TRANSPORT: i64 = -32000;

/// 路由装配。绝对路径 merge(照抄 `stream.rs` 模式——不 nest 在
/// `/api/v1/{domain}` 下;MCP 约定单 endpoint 且与既有 REST API 不同域)。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/mcp", post(mcp_post).get(mcp_get).delete(mcp_delete))
        .with_state(state)
}

/// `GET /mcp` → 405:不开服务端主动流(SDK 客户端 `_startOrAuthSse` 把
/// 405 视为「服务端不支持 GET 流」的预期分支,静默跳过;其他非 2xx 反而
/// 报错,所以这里必须精确 405)。
async fn mcp_get() -> Response {
    transport_error(
        StatusCode::METHOD_NOT_ALLOWED,
        ERR_TRANSPORT,
        "Method Not Allowed: this server does not offer a standalone SSE stream",
    )
}

/// `DELETE /mcp` → 200 no-op(无状态无 session;客户端只在持有 session
/// id 时才会发 DELETE,此分支纯防御)。
async fn mcp_delete() -> Response {
    (StatusCode::OK, "").into_response()
}

/// `POST /mcp` 主 handler:校验 → 解析 → 分派。
async fn mcp_post(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    // Accept 必须同时列出 application/json 与 text/event-stream
    // (SDK 客户端恒发此组合;逗号列表子串匹配即协议语义)。
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !accept.contains("application/json") || !accept.contains("text/event-stream") {
        return transport_error(
            StatusCode::NOT_ACCEPTABLE,
            ERR_TRANSPORT,
            "Not Acceptable: Client must accept both application/json and text/event-stream",
        );
    }
    // Content-Type 取 media type essence(剥 ;charset,小写化)。
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ct_essence = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct_essence != "application/json" {
        return transport_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ERR_TRANSPORT,
            "Unsupported Media Type: Content-Type must be application/json",
        );
    }
    // lenient:协议版本头不校验只记日志(D6,防宿主升级新版自断)。
    if let Some(v) = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
    {
        tracing::debug!(version = %v, "mcp: request protocol-version header");
    }

    let raw: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return transport_error(
                StatusCode::BAD_REQUEST,
                ERR_PARSE,
                "Parse error: Invalid JSON",
            )
        }
    };
    let is_batch = raw.is_array();
    let messages: Vec<Value> = match raw {
        Value::Array(arr) => arr,
        Value::Object(_) => vec![raw],
        _ => {
            return transport_error(
                StatusCode::BAD_REQUEST,
                ERR_PARSE,
                "Parse error: Invalid JSON-RPC message",
            )
        }
    };
    // 逐条形状校验(SDK JSONRPCMessageSchema 等价面):请求(method+id)/
    // 通知(method)/客户端响应(id+result|error)三态之外 → 整包 400。
    for m in &messages {
        let ok = m.is_object()
            && (m.get("method").map(|v| v.is_string()).unwrap_or(false)
                || (m.get("id").is_some()
                    && (m.get("result").is_some() || m.get("error").is_some())));
        if !ok {
            return transport_error(
                StatusCode::BAD_REQUEST,
                ERR_PARSE,
                "Parse error: Invalid JSON-RPC message",
            );
        }
    }

    let mut responses: Vec<Value> = Vec::new();
    let mut has_requests = false;
    for m in messages {
        let is_request = m.get("method").is_some() && m.get("id").is_some();
        if !is_request {
            // 通知(含 notifications/initialized)与客户端响应:无副作用无应答。
            continue;
        }
        has_requests = true;
        let id = m.get("id").cloned().unwrap_or(Value::Null);
        responses.push(handle_request(&state, id, &m).await);
    }
    if !has_requests {
        return (StatusCode::ACCEPTED, "").into_response();
    }
    let out = if is_batch {
        Value::Array(responses)
    } else {
        responses.into_iter().next().unwrap_or(Value::Null)
    };
    (StatusCode::OK, Json(out)).into_response()
}

/// 传输级错误(HTTP 状态码 + JSON-RPC error body;对齐 SDK 服务端
/// `createJsonErrorResponse` 形态)。
fn transport_error(status: StatusCode, code: i64, message: &str) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": code, "message": message },
    });
    (status, Json(body)).into_response()
}

fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_err(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

/// 单条 JSON-RPC 请求分派。
async fn handle_request(state: &Arc<AppState>, id: Value, msg: &Value) -> Value {
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let version = negotiate_version(requested);
            rpc_ok(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                }),
            )
        }
        "ping" => rpc_ok(&id, json!({})),
        "tools/list" => rpc_ok(&id, json!({ "tools": tool_defs() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
                return rpc_err(&id, ERR_INVALID_PARAMS, "Invalid params: missing tool name");
            };
            if !tool_defs()
                .iter()
                .any(|t| t.get("name").and_then(|v| v.as_str()) == Some(name))
            {
                return rpc_err(&id, ERR_INVALID_PARAMS, &format!("Unknown tool: {name}"));
            }
            let args = match params.get("arguments") {
                Some(v) if v.is_object() => v.clone(),
                _ => json!({}),
            };
            match call_tool(state, name, &args).await {
                Ok(v) => rpc_ok(&id, text_result(v, false)),
                Err(ToolError::Semantic(message)) => {
                    rpc_ok(&id, text_result(json!({ "error": message }), true))
                }
                Err(ToolError::Infra(message)) => rpc_ok(
                    &id,
                    text_result(json!({ "error": message, "hint": HINT_DAEMON }), true),
                ),
            }
        }
        _ => rpc_err(
            &id,
            ERR_METHOD_NOT_FOUND,
            &format!("Method not found: {method}"),
        ),
    }
}

/// 版本协商(D5):请求版本 ∈ 支持集 → echo;否则回缺省协商值。
fn negotiate_version(requested: &str) -> &'static str {
    SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == requested)
        .copied()
        .unwrap_or(DEFAULT_PROTOCOL_VERSION)
}

// ---------------------------------------------------------------------------
// 工具定义(tools/list wire schema)
// ---------------------------------------------------------------------------

/// 八工具声明。description/inputSchema 逐字段平移自已退役 JS 壳的
/// `buildToolShapes` + `TOOLS` 表(源文件 2026-09-15 P4 删除;字段映射表见
/// 任务 09-14 research/daemon-converge.md §2)——宿主注入 LLM context
/// 的就是这份 wire schema,文案即产品(成本闸/不阻塞),勿重写。
fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "start_discussion",
            "description": "Convene a multi-LLM group deliberation on a topic. Costly: 5-15 min, hundreds of thousands of tokens. Returns immediately with session_id — poll discussion_status, read conclusions via discussion_result. Presets: builtin four + user presets — see list_presets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "The question; evidence-backed, do not bake the answer in" },
                    "cwd": { "type": "string", "description": "Project dir as evidence base" },
                    "preset": { "type": "string", "description": "Participant preset: builtin review/fe_review/arch/retro, or user preset key/name — see list_presets" },
                    "participants": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "model": { "type": "string", "description": "Catalog name or UUID" },
                                "persona_md": { "type": "string" }
                            },
                            "required": ["name", "model"]
                        },
                        "description": "Full roster, replaces preset roster (moderator unchanged)"
                    },
                    "token_budget": { "type": "integer", "minimum": 1, "description": "Billed-token ceiling (input+output+cache_creation+cache_read); exceeded → halts at next round head with stop_reason=budget. Omit = unlimited" }
                },
                "required": ["topic", "cwd"]
            }
        }),
        json!({
            "name": "discussion_status",
            "description": "Check a discussion: busy=true running; busy=false + stop_reason (group_chat_end|max_rounds|cancelled|error) = finished. Optional wait_seconds long-polls for progress; detail adds messages/last_speaker/tokens.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "wait_seconds": { "type": "integer", "minimum": 1, "maximum": 30, "description": "Long-poll up to N s: returns early on progress (new message / busy flip / stop_reason); else wait_timed_out:true" },
                    "detail": { "type": "boolean", "description": "Add progress fields: messages count, last_speaker, tokens so far (+ token_budget if declared)" }
                },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "discussion_result",
            "description": "Read a finished discussion's conclusion (errors while running — poll discussion_status first). Returns summary, roster, stats, transcript path.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "cancel_discussion",
            "description": "Stop a running discussion (orchestration stops, session kept).",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "interrupt_discussion",
            "description": "Gracefully stop a running discussion: in-flight speaker finishes, the moderator wraps up with a summary, stop_reason=preempted. Returns immediately; poll discussion_status (~1-3 min), then read discussion_result.",
            "inputSchema": {
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }
        }),
        json!({
            "name": "inject_message",
            "description": "Inject a user message into a RUNNING discussion; the next moderator round sees it and the discussion continues. Errors if the session is not busy — use start_discussion to convene a new one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "text": { "type": "string", "minLength": 1, "description": "User message text; lands as [用户插入] in the next moderator round" }
                },
                "required": ["session_id", "text"]
            }
        }),
        json!({
            "name": "list_models",
            "description": "List available models (name + UUID) for start_discussion participants/moderator. Cheap metadata call.",
            "inputSchema": { "type": "object" }
        }),
        json!({
            "name": "list_presets",
            "description": "List participant presets: builtin four + user presets (key = row UUID) + overrides. Cheap metadata call; degraded=true means daemon unreachable (builtin only).",
            "inputSchema": { "type": "object" }
        }),
    ]
}

/// 工具描述预算上限(JS 壳 TOOLS_BUDGET_CHARS 同源约束,升锁须过评审)。
/// 09-14 Rust 化实测 3600(serde_json 序列化口径,键序字母序与 JS 略异,
/// 数量级一致;JS 侧最后实测 4093 —— 描述文案同源,zod 附加关键字更肥)。
pub const TOOLS_BUDGET_CHARS: usize = 4200;

// ---------------------------------------------------------------------------
// 工具错误与结果包装
// ---------------------------------------------------------------------------

const HINT_DAEMON: &str = "daemon 调用链问题,先确认 daemon 在跑(scripts/daemon.sh)";

/// 工具级错误两态(对齐 JS 壳 isToolError 惯例):
/// Semantic = 调用方语义问题(参数/状态不满足)→ 只报 `{error}`;
/// Infra = daemon 内部链路故障 → `{error, hint}` 附自救指引。
#[derive(Debug)]
enum ToolError {
    Semantic(String),
    Infra(String),
}

impl ToolError {
    fn semantic<T: Into<String>>(msg: T) -> Self {
        ToolError::Semantic(msg.into())
    }
    fn infra<T: Into<String>>(msg: T) -> Self {
        ToolError::Infra(msg.into())
    }
}

impl From<AppCommandError> for ToolError {
    fn from(e: AppCommandError) -> Self {
        ToolError::infra(e.message)
    }
}

/// 工具结果 → CallToolResult(pretty-JSON text content;JS 壳 textResult 同款)。
fn text_result(payload: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

async fn call_tool(state: &Arc<AppState>, name: &str, args: &Value) -> Result<Value, ToolError> {
    match name {
        "start_discussion" => tool_start_discussion(state, args).await,
        "discussion_status" => tool_discussion_status(state, args).await,
        "discussion_result" => tool_discussion_result(state, args).await,
        "cancel_discussion" => tool_cancel_discussion(state, args).await,
        "interrupt_discussion" => tool_interrupt_discussion(state, args).await,
        "inject_message" => tool_inject_message(state, args).await,
        "list_models" => tool_list_models(state).await,
        "list_presets" => tool_list_presets(state).await,
        _ => Err(ToolError::semantic(format!("Unknown tool: {name}"))),
    }
}

// ---------------------------------------------------------------------------
// 内置预设(编译期嵌入,D1)
// ---------------------------------------------------------------------------

const PRESETS_JSON: &str = include_str!("../../../../../scripts/group-chat-presets.json");

#[derive(Debug, Deserialize)]
struct PresetsFileDef {
    #[serde(default)]
    persona_common: String,
    #[serde(default)]
    personas: HashMap<String, String>,
    presets: HashMap<String, PresetDef>,
}

#[derive(Debug, Deserialize)]
struct PresetDef {
    description: String,
    moderator_model: String,
    participants: Vec<PresetParticipantDef>,
}

#[derive(Debug, Deserialize)]
struct PresetParticipantDef {
    name: String,
    model: String,
    persona: String,
}

/// 运行时预设条目(内置展开后与 DB 行合流的同构形状)。
#[derive(Debug, Clone)]
struct EffectivePreset {
    description: String,
    moderator_model: String,
    participants: Vec<PresetParticipant>,
    /// `builtin` | `user` | `override`(展示标记,消费逻辑零依赖)。
    source: &'static str,
    /// 内置/覆盖 = key;用户行 = 管理名。
    display_name: String,
}

#[derive(Debug, Clone)]
struct PresetParticipant {
    name: String,
    model: String,
    persona_md: String,
}

/// 解析嵌入 JSON(OnceLock 单次;坏 JSON fail-loud 存 Err,首次消费时报
/// infra 错——启动期不炸 daemon,错误在调用面呈现)。
fn presets_file() -> Result<&'static PresetsFileDef, String> {
    static CACHE: OnceLock<Result<PresetsFileDef, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            serde_json::from_str(PRESETS_JSON)
                .map_err(|e| format!("group-chat-presets.json 解析失败: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// persona kind → 完整 persona_md(边界 + 空行 + 公共纪律;JS
/// composePersonaMd 同构;脏 kind fail-loud 不降级)。
fn compose_persona_md(file: &PresetsFileDef, kind: &str) -> Result<String, String> {
    let base = file
        .personas
        .get(kind)
        .ok_or_else(|| format!("group-chat-presets.json: 缺 persona \"{kind}\""))?;
    Ok(format!("{}\n\n{}", base, file.persona_common))
}

/// 内置四档展开(JS composePresets 同构;确定性——单测锁)。
fn builtin_presets() -> Result<HashMap<String, EffectivePreset>, ToolError> {
    let file = presets_file().map_err(ToolError::infra)?;
    let mut out = HashMap::new();
    for (name, preset) in &file.presets {
        let mut participants = Vec::with_capacity(preset.participants.len());
        for p in &preset.participants {
            participants.push(PresetParticipant {
                name: p.name.clone(),
                model: p.model.clone(),
                persona_md: compose_persona_md(file, &p.persona).map_err(ToolError::infra)?,
            });
        }
        out.insert(
            name.clone(),
            EffectivePreset {
                description: preset.description.clone(),
                moderator_model: preset.moderator_model.clone(),
                participants,
                source: "builtin",
                display_name: name.clone(),
            },
        );
    }
    if out.is_empty() {
        return Err(ToolError::infra(
            "group-chat-presets.json: presets 不能为空",
        ));
    }
    Ok(out)
}

/// 有效预设视图 = 内置四档 ⊕ group_chat_presets 表(merge 规则镜像 JS
/// mergePresets/前端 mergedPresets:覆盖行原位顶替内置槽(key/display_name
/// 保持内置)、用户行追加 key=row.id、脏 builtinKey(不在内置集合)跳过)。
async fn effective_presets(
    state: &Arc<AppState>,
) -> Result<HashMap<String, EffectivePreset>, ToolError> {
    let file = presets_file().map_err(ToolError::infra)?;
    let mut out = builtin_presets()?;
    let rows = db::group_chat_presets::list_group_chat_presets(&state.db)
        .await
        .map_err(|e| ToolError::infra(format!("list_group_chat_presets failed: {e}")))?;
    for row in rows {
        // persona kind 展开依赖同一 personas 表(用户行 persona 同五内置
        // kind,DB 校验保证;防御脏行 fail-loud)。
        let mut participants = Vec::with_capacity(row.participants.len());
        for p in &row.participants {
            participants.push(PresetParticipant {
                name: p.name.clone(),
                model: p.model_id.clone(),
                persona_md: compose_persona_md(file, &p.persona).map_err(ToolError::infra)?,
            });
        }
        let entry = EffectivePreset {
            description: row.description.clone(),
            moderator_model: row.moderator_model_id.clone(),
            participants,
            source: "user",
            display_name: row.name.clone(),
        };
        match row.builtin_key.as_deref() {
            Some(key) => {
                // 覆盖行:仅当 key 确在内置集合才顶替;脏 key 跳过(防未来
                // JSON 删 key 的存量行悬空成假用户档)。
                if let Some(slot) = out.get_mut(key) {
                    *slot = EffectivePreset {
                        source: "override",
                        display_name: key.to_string(),
                        ..entry
                    };
                }
            }
            None => {
                out.insert(row.id.clone(), entry);
            }
        }
    }
    Ok(out)
}

/// 预设引用三趟解析(JS lookupPreset 同构):key 直配(内置 key / 用户行
/// id)→ display_name 精确(唯一)→ 忽略大小写(唯一);歧义/miss 报可用清单。
fn lookup_preset(
    presets: &HashMap<String, EffectivePreset>,
    reference: &str,
) -> Result<EffectivePreset, ToolError> {
    if let Some(p) = presets.get(reference) {
        return Ok(p.clone());
    }
    let entries: Vec<(&String, &EffectivePreset)> = presets.iter().collect();
    let preds: [fn(&str, &str) -> bool; 2] = [
        |name, r| name == r,
        |name, r| name.to_lowercase() == r.to_lowercase(),
    ];
    for pred in preds {
        let hits: Vec<_> = entries
            .iter()
            .filter(|(_, p)| pred(&p.display_name, reference))
            .collect();
        match hits.len() {
            1 => return Ok(hits[0].1.clone()),
            n if n > 1 => {
                return Err(ToolError::semantic(format!(
                    "预设名 \"{reference}\" 歧义({n} 档);请用 key(内置 key 或用户行 id)引用"
                )))
            }
            _ => {}
        }
    }
    let listing = entries
        .iter()
        .map(|(key, p)| {
            if p.source == "user" {
                format!(
                    "{}({})",
                    p.display_name,
                    &key.chars().take(8).collect::<String>()
                )
            } else {
                key.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" / ");
    Err(ToolError::semantic(format!(
        "未知预设 \"{reference}\";可用:{listing}(查看细节:list_presets)"
    )))
}

// ---------------------------------------------------------------------------
// 模型 / 项目解析
// ---------------------------------------------------------------------------

/// 名字/UUID → model UUID(JS normalizeModelRef 同构,含大小写两趟:
/// 目录里存在「仅大小写差」的真实撞车,单趟 lowercase 命中谁取决于顺序)。
fn normalize_model_ref(
    models: &[db::types::ModelWithProvider],
    reference: &str,
) -> Result<String, ToolError> {
    if reference.is_empty() {
        return Err(ToolError::semantic("空模型引用"));
    }
    if let Some(m) = models.iter().find(|m| m.model.id == reference) {
        return Ok(m.model.id.clone());
    }
    if let Some(m) = models
        .iter()
        .find(|m| m.model.model_name == reference || m.model.display_name == reference)
    {
        return Ok(m.model.id.clone());
    }
    let lower = reference.to_lowercase();
    if let Some(m) = models.iter().find(|m| {
        m.model.model_name.to_lowercase() == lower || m.model.display_name.to_lowercase() == lower
    }) {
        return Ok(m.model.id.clone());
    }
    let names = models
        .iter()
        .map(|m| {
            if m.model.model_name.is_empty() {
                m.model.display_name.clone()
            } else {
                m.model.model_name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" / ");
    Err(ToolError::semantic(format!(
        "模型 \"{reference}\" 不在目录(现有:{names});list_models 查清单"
    )))
}

/// 模型目录的 id → 展示名映射(roster/转录展示用;拿不到目录时原样)。
async fn model_display_map(state: &Arc<AppState>) -> Result<HashMap<String, String>, ToolError> {
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| ToolError::infra(format!("list_models failed: {e}")))?;
    Ok(models
        .into_iter()
        .map(|m| {
            let name = if m.model.display_name.is_empty() {
                m.model.model_name.clone()
            } else {
                m.model.display_name.clone()
            };
            (m.model.id, name)
        })
        .collect())
}

/// 词典绝对化(JS path.resolve 语义:相对 → cwd 拼;折叠 ./..;Linux-only)。
fn lexical_absolute(input: &str) -> String {
    let p = std::path::Path::new(input);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = std::path::PathBuf::new();
    for c in abs.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().to_string()
}

/// 按物理路径解析 project;miss 则建(JS resolveProject 同构;hidden 项目
/// 也查——create 的唯一性检查查全表,列表过滤会漏配)。
async fn resolve_project_by_path(state: &Arc<AppState>, path: &str) -> Result<String, ToolError> {
    let all = list_projects_inner(state, Some(ListProjectsFilter { hidden: Some(true) })).await?;
    if let Some(hit) = all.iter().find(|p| lexical_absolute(&p.path) == path) {
        return Ok(hit.id.clone());
    }
    let created = create_project_inner(state, path.to_string()).await?;
    Ok(created.id)
}

// ---------------------------------------------------------------------------
// 工具实现
// ---------------------------------------------------------------------------

const HINT_STARTED: &str = "started — discussion_status(session_id) polls cheaply; add wait_seconds=30 to return on progress, detail=true for messages/last_speaker/tokens. After stop_reason is set, read discussion_result. Expect 5-15 min.";
const HINT_INTERRUPT: &str = "Wrap-up in progress: the in-flight speaker finishes, then the moderator rounds off (~1-3 min). Poll discussion_status; after it turns terminal (stop_reason=preempted, or group_chat_end if the discussion finished naturally in the same instant), read discussion_result.";
const HINT_INJECTED: &str = "Injected — lands as [用户插入] in the next moderator round; the discussion continues. Poll discussion_status as usual.";
const HINT_MODELS: &str = "Reference by name or UUID in start_discussion (participants[].model / preset moderator). Resolved at start time.";
const HINT_PRESETS: &str = "Reference preset by key (builtin key or user row UUID) or user name in start_discussion. Resolved at start time.";
const NOTE_CANCELLED: &str = "编排已停,session 保留(部分转录照常可导出)";

/// 长轮询上限与拍频(JS MAX_WAIT_SECONDS / waitTickMs 同源)。
const MAX_WAIT_SECONDS: i64 = 30;
const WAIT_TICK_MS: u64 = 2000;

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// busy 真相源 = `session_active_request` 内存表(`list_sessions_inner`
/// 富化的同一映射;`load_session` 的 busy 恒 false 不可信)。
async fn session_busy(state: &Arc<AppState>, session_id: &str) -> bool {
    state
        .session_active_request
        .lock()
        .await
        .contains_key(session_id)
}

/// 会话级终态(GC1/GC2):!busy 且 stop_reason 非空。
fn is_terminal(busy: bool, stop_reason: Option<&str>) -> bool {
    !busy && stop_reason.is_some()
}

/// created_at(RFC3339)→ 至今秒数;解析失败 → None(JS elapsed 派生口径)。
fn elapsed_seconds(created_at: &str) -> Option<i64> {
    let dt = chrono::DateTime::parse_from_rfc3339(created_at).ok()?;
    Some(
        (chrono::Utc::now() - dt.with_timezone(&chrono::Utc))
            .num_seconds()
            .max(0),
    )
}

/// status 长轮询信号:busy/stop_reason 翻转 + 消息数/末 seq(JS statusSignal)。
fn status_signal(busy: bool, loaded: &db::types::LoadedSession) -> String {
    let last = loaded.messages.last();
    format!(
        "{}|{}|{}|{}",
        busy,
        loaded.session.stop_reason.as_deref().unwrap_or(""),
        loaded.messages.len(),
        last.map(|m| m.seq.to_string()).unwrap_or_default()
    )
}

/// 取 metadata(session.metadata 列,Option<Value> 直读)。
fn metadata_value(session: &db::types::SessionRow) -> Value {
    session.metadata.clone().unwrap_or(Value::Null)
}

fn chat_sinks(state: &Arc<AppState>) -> (Arc<dyn ChatEventSink>, Arc<dyn SubagentEventSink>) {
    // 与 /api/v1/agent/chat 完全同款注入(JS 壳现状就是打该 HTTP 路径,
    // 收敛后直调 inner 行为等价;不用 scheduler 的 semi_sink——那是定时
    // 场半透传,MCP/交互场要完整事件流)。
    (
        Arc::new(HttpSseSink {
            registry: state.sse.clone(),
        }),
        Arc::new(HttpSseSubagentSink {
            registry: state.sse.clone(),
        }),
    )
}

fn user_text_message(text: String) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text),
        speaker: None,
        attachments: None,
    }
}

/// start_discussion:校验 → 预设(合并视图)→ 模型解析 → 项目解析 →
/// 建群(created_via=mcp)→ chat_inner 发题(fire-and-forget)。
async fn tool_start_discussion(state: &Arc<AppState>, args: &Value) -> Result<Value, ToolError> {
    let topic = arg_str(args, "topic").unwrap_or("").trim().to_string();
    if topic.is_empty() {
        return Err(ToolError::semantic(
            "缺议题:topic(议题质量直接决定产出质量,不要把答案写进问题)",
        ));
    }
    let cwd = arg_str(args, "cwd").unwrap_or("").trim().to_string();
    if cwd.is_empty() {
        return Err(ToolError::semantic("缺工作目录:cwd(讨论的证据基地)"));
    }
    let token_budget = match args.get("token_budget") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(n) if n > 0 => Some(n),
            _ => {
                return Err(ToolError::semantic(
                    "token_budget 必须是正整数(不限请省略该参数)",
                ))
            }
        },
    };

    let preset_ref = arg_str(args, "preset").unwrap_or("review").to_string();
    let eff = effective_presets(state).await?;
    // preset 恒须可解析(moderator 取自预设;participants 覆盖名单不豁免)。
    let preset_entry = lookup_preset(&eff, &preset_ref)?;
    let moderator_model = preset_entry.moderator_model.clone();

    // 名单:显式 participants 整名单替换;否则取预设名单。
    let roster: Vec<PresetParticipant> = match args.get("participants") {
        None | Some(Value::Null) => preset_entry.participants.clone(),
        Some(Value::Array(list)) => {
            let mut out = Vec::with_capacity(list.len());
            for p in list {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let model = p.get("model").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() || model.is_empty() {
                    return Err(ToolError::semantic(format!(
                        "参与者缺 name/model:{}",
                        serde_json::to_string(p).unwrap_or_default()
                    )));
                }
                out.push(PresetParticipant {
                    name: name.to_string(),
                    model: model.to_string(),
                    persona_md: p
                        .get("persona_md")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
            if out.is_empty() {
                return Err(ToolError::semantic(
                    "participants 必须是非空数组 [{name, model, persona_md?}]",
                ));
            }
            let mut names = std::collections::HashSet::new();
            for p in &out {
                if !names.insert(p.name.as_str()) {
                    return Err(ToolError::semantic(format!("参与者重名:\"{}\"", p.name)));
                }
            }
            out
        }
        Some(_) => {
            return Err(ToolError::semantic(
                "participants 必须是非空数组 [{name, model, persona_md?}]",
            ))
        }
    };

    let models = db::list_models(&state.db)
        .await
        .map_err(|e| ToolError::infra(format!("list_models failed: {e}")))?;
    let moderator_id = normalize_model_ref(&models, &moderator_model)?;
    let mut participants_json = Vec::with_capacity(roster.len());
    for p in &roster {
        let mut entry = json!({
            "name": p.name,
            "model": normalize_model_ref(&models, &p.model)?,
        });
        if !p.persona_md.is_empty() {
            entry["persona_md"] = json!(p.persona_md);
        }
        participants_json.push(entry);
    }

    let cwd_abs = lexical_absolute(&cwd);
    let project_id = resolve_project_by_path(state, &cwd_abs).await?;

    let mut metadata = json!({
        "participants": participants_json,
        "created_via": "mcp",
    });
    if let Some(b) = token_budget {
        metadata["token_budget"] = json!(b);
    }
    let session = create_session_in_pool(
        &state.db,
        project_id,
        cwd_abs,
        Some(moderator_id),
        Some("group_chat".to_string()),
        Some(metadata),
    )
    .await
    .map_err(|e| ToolError::infra(format!("create_session failed: {}", e.message)))?;

    // 发题:fire-and-forget,acceptance 不判(新场不可能 busy)。
    let request_id = format!("gcmcp-{}", uuid::Uuid::new_v4());
    let (sink, worker_event_sink) = chat_sinks(state);
    chat_inner(
        state,
        ChatEntry {
            request_id: request_id.clone(),
            session_id: session.id.clone(),
            messages: vec![user_text_message(topic)],
            sink,
            worker_catalog: Some(state.catalog.clone()),
            worker_event_sink,
            resend_seq: None,
            forced_dispatch: None,
            origin: None,
            resume_group_chat: None,
        },
    )
    .await
    .map_err(|e| ToolError::infra(format!("agent chat 受理失败: {}", e.message)))?;

    Ok(json!({
        "session_id": session.id,
        "request_id": request_id,
        "hint": HINT_STARTED,
    }))
}

/// status 单次快照构建(status/result 共用语义,wait 循环内反复调用)。
async fn build_status_snapshot(
    state: &Arc<AppState>,
    loaded: &db::types::LoadedSession,
    busy: bool,
    wait_timed_out: bool,
    tokens: Option<&db::trace::GroupChatTokenUsage>,
    want_progress: bool,
) -> Value {
    let meta = metadata_value(&loaded.session);
    let mut out = json!({
        "busy": busy,
        "stop_reason": loaded.session.stop_reason.clone(),
        "elapsed_s": elapsed_seconds(&loaded.session.created_at),
    });
    if want_progress {
        out["messages"] = json!(loaded.messages.len());
        if let Some(last) = loaded.messages.iter().rev().find(|m| m.speaker.is_some()) {
            out["last_speaker"] = json!(last.speaker);
        }
        if let Some(t) = tokens {
            out["tokens"] = json!({
                "total": t.total,
                "per_speaker": t.by_speaker.iter().map(|s| json!({
                    "speaker": s.speaker, "tokens": s.tokens,
                })).collect::<Vec<_>>(),
            });
        }
        if let Some(b) = meta.get("token_budget").and_then(|v| v.as_u64()) {
            out["token_budget"] = json!(b);
        }
    }
    if is_terminal(busy, loaded.session.stop_reason.as_deref()) {
        // 终态首次观测 → 惰性转录(status 永不因导出报错,P2-1)。
        let t = export_mcp_transcript(state, loaded).await;
        if let Some(p) = t.get("transcript_path") {
            out["transcript_path"] = p.clone();
        }
        if let Some(w) = t.get("transcript_warning") {
            out["transcript_warning"] = w.clone();
        }
    }
    if wait_timed_out {
        out["wait_timed_out"] = json!(true);
    }
    out
}

/// discussion_status:廉价轮询 + wait_seconds 有界长轮询 + detail 富化 +
/// 终态观测触发惰性转录。
async fn tool_discussion_status(state: &Arc<AppState>, args: &Value) -> Result<Value, ToolError> {
    let session_id = arg_str(args, "session_id").unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err(ToolError::semantic("缺 session_id:session_id"));
    }
    let wait_seconds = match args.get("wait_seconds") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(n) if (1..=MAX_WAIT_SECONDS).contains(&n) => Some(n),
            _ => {
                return Err(ToolError::semantic(format!(
                    "wait_seconds 必须是 1-{MAX_WAIT_SECONDS} 的整数(有界长轮询,避开宿主工具超时;不等请省略)"
                )))
            }
        },
    };
    let detail = args
        .get("detail")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // wait 隐含 detail(等到了就该报变化);detail-only = 单次富快照。
    let want_progress = detail || wait_seconds.is_some();

    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| ToolError::infra(format!("load_session failed: {e}")))?
        .ok_or_else(|| ToolError::semantic(format!("session 不存在:{session_id}")))?;
    let busy = session_busy(state, &session_id).await;

    // tokens 富化(失败整键省略,不污染既有字段)。
    let tokens = if want_progress {
        db::trace::group_chat_token_usage(&state.db, &session_id)
            .await
            .ok()
    } else {
        None
    };

    if wait_seconds.is_none() || is_terminal(busy, loaded.session.stop_reason.as_deref()) {
        return Ok(build_status_snapshot(
            state,
            &loaded,
            busy,
            false,
            tokens.as_ref(),
            want_progress,
        )
        .await);
    }

    // B1 长轮询:每拍 = load_session(消息面)+ busy 内存表;变化即返;
    // 到点前必做一次实查再报超时(边界变化不空报)。
    let baseline = status_signal(busy, &loaded);
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(wait_seconds.unwrap_or(0) as u64);
    let mut current = loaded;
    let mut current_busy = busy;
    let mut current_tokens = tokens;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(WAIT_TICK_MS)).await;
        let next = db::load_session(&state.db, &session_id)
            .await
            .map_err(|e| ToolError::infra(format!("load_session failed: {e}")))?;
        let Some(next) = next else {
            // 会话中途被删:按最后已知态收口(温和返回,不炸轮询)。
            break;
        };
        let next_busy = session_busy(state, &session_id).await;
        let next_tokens = db::trace::group_chat_token_usage(&state.db, &session_id)
            .await
            .ok()
            .or(current_tokens);
        let changed = status_signal(next_busy, &next) != baseline;
        current = next;
        current_busy = next_busy;
        current_tokens = next_tokens;
        if changed {
            return Ok(build_status_snapshot(
                state,
                &current,
                current_busy,
                false,
                current_tokens.as_ref(),
                want_progress,
            )
            .await);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    Ok(build_status_snapshot(
        state,
        &current,
        current_busy,
        true,
        current_tokens.as_ref(),
        want_progress,
    )
    .await)
}

/// discussion_result:终态 guard + 结构化结论 + roster/stats/tokens +
/// 惰性转录 + 三处降级(detail 坏 JSON / summary 缺失 / tokens 失败)。
async fn tool_discussion_result(state: &Arc<AppState>, args: &Value) -> Result<Value, ToolError> {
    let session_id = arg_str(args, "session_id").unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err(ToolError::semantic("缺 session_id:session_id"));
    }
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| ToolError::infra(format!("load_session failed: {e}")))?
        .ok_or_else(|| ToolError::semantic(format!("session 不存在:{session_id}")))?;
    let busy = session_busy(state, &session_id).await;
    if !is_terminal(busy, loaded.session.stop_reason.as_deref()) {
        return Err(ToolError::semantic(format!(
            "still running (busy={busy}, stop_reason not set) — poll discussion_status; expect 5-15 min total"
        )));
    }

    let transcript = export_mcp_transcript(state, &loaded).await;
    let names = model_display_map(state).await.unwrap_or_default();
    let disp = |id: &str| -> String {
        if id.is_empty() {
            String::new()
        } else {
            names.get(id).cloned().unwrap_or_else(|| id.to_string())
        }
    };
    let meta = metadata_value(&loaded.session);

    let mut out = json!({
        "stop_reason": loaded.session.stop_reason.clone(),
        "summary": loaded.session.discussion_summary.clone(),
        "roster": {
            "moderator": disp(&loaded.session.model),
            "participants": meta
                .get("participants")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            Some(json!(format!(
                                "{}/{}",
                                p.get("name")?.as_str()?,
                                disp(p.get("model")?.as_str()?)
                            )))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        },
        "stats": {
            "messages": loaded.messages.len(),
            "elapsed_s": elapsed_seconds(&loaded.session.created_at),
        },
    });
    // C2 证据链:坏 JSON → null + warning,不炸 result。
    if let Some(detail_json) = loaded.session.discussion_detail.as_deref() {
        match serde_json::from_str::<Value>(detail_json) {
            Ok(v) => out["detail"] = v,
            Err(_) => {
                out["detail_warning"] =
                    json!("discussion_detail 非法 JSON(列数据损坏);读转录尾段人工收束")
            }
        }
    }
    match db::trace::group_chat_token_usage(&state.db, &session_id).await {
        Ok(t) => {
            out["tokens"] = json!({
                "total": t.total,
                "per_speaker": t.by_speaker.iter().map(|s| json!({
                    "speaker": s.speaker, "tokens": s.tokens,
                })).collect::<Vec<_>>(),
            });
        }
        Err(_) => { /* 聚合失败整键省略,不污染既有 stats 字段 */ }
    }
    if let Some(p) = transcript.get("transcript_path") {
        out["transcript_path"] = p.clone();
    }
    if let Some(w) = transcript.get("transcript_warning") {
        out["transcript_warning"] = w.clone();
    }
    if loaded.session.discussion_summary.is_none() {
        out["summary_warning"] = json!(
            "正常收官但 discussion_summary 缺失(moderator 未走 end_discussion);读转录尾段人工收束"
        );
    }
    Ok(out)
}

/// cancel_discussion:rid 从 `session_active_request` 内存表取(D3:daemon
/// 即编排宿主,讨论活着 rid 就在,无跨进程记账);非 busy → 幂等收口。
async fn tool_cancel_discussion(state: &Arc<AppState>, args: &Value) -> Result<Value, ToolError> {
    let session_id = arg_str(args, "session_id").unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err(ToolError::semantic("缺 session_id:session_id"));
    }
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| ToolError::infra(format!("load_session failed: {e}")))?;
    let Some(loaded) = loaded else {
        return Err(ToolError::semantic(format!("session 不存在:{session_id}")));
    };
    let busy = session_busy(state, &session_id).await;
    let rid = if busy {
        state
            .session_active_request
            .lock()
            .await
            .get(&session_id)
            .cloned()
    } else {
        None
    };
    match rid {
        Some(rid) => {
            cancel_chat_inner(state, rid)
                .await
                .map_err(|e| ToolError::infra(format!("cancel_chat failed: {}", e.message)))?;
            Ok(json!({
                "cancelled": true,
                "session_id": session_id,
                "note": NOTE_CANCELLED,
            }))
        }
        None => Ok(json!({
            "already_finished": true,
            "stop_reason": loaded.session.stop_reason.clone(),
            "session_id": session_id,
        })),
    }
}

/// interrupt_discussion:session 域收束打断(preempt 端点 1:1);无进行中
/// 讨论 → 端点报错按语义错误透传(无副作用)。
async fn tool_interrupt_discussion(
    state: &Arc<AppState>,
    args: &Value,
) -> Result<Value, ToolError> {
    let session_id = arg_str(args, "session_id").unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err(ToolError::semantic("缺 session_id:session_id"));
    }
    let outcome = preempt_group_chat_inner(state, session_id.clone())
        .await
        .map_err(|e| ToolError::semantic(e.message))?;
    Ok(json!({
        "interrupted": outcome.preempted,
        "session_id": session_id,
        "hint": HINT_INTERRUPT,
    }))
}

/// inject_message:前置 busy guard(空闲/已收官群聊一旦发 chat 会重启编排
/// 器并抹旧场 summary,cancel 救不回 → 非 busy 一律不发起);受理后
/// acceptance 非 injected → 自有 rid 即时 cancel 止损 + 语义报错。
async fn tool_inject_message(state: &Arc<AppState>, args: &Value) -> Result<Value, ToolError> {
    let session_id = arg_str(args, "session_id").unwrap_or("").to_string();
    if session_id.is_empty() {
        return Err(ToolError::semantic("缺 session_id:session_id"));
    }
    let text = arg_str(args, "text").unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err(ToolError::semantic(
            "缺注入文本:text(注入只收文本;纯图片注入不支持)",
        ));
    }
    let loaded = db::load_session(&state.db, &session_id)
        .await
        .map_err(|e| ToolError::infra(format!("load_session failed: {e}")))?;
    let stop_reason = loaded.as_ref().and_then(|l| l.session.stop_reason.clone());
    let busy = session_busy(state, &session_id).await;
    if !busy {
        return Err(ToolError::semantic(format!(
            "目标不是进行中的群聊讨论(busy={}, stop_reason={});注入只对进行中的讨论有效;发起新讨论请用 start_discussion。",
            busy,
            stop_reason.as_deref().unwrap_or("null"),
        )));
    }

    let request_id = format!("gcinject-{}", uuid::Uuid::new_v4());
    let (sink, worker_event_sink) = chat_sinks(state);
    let acceptance = chat_inner(
        state,
        ChatEntry {
            request_id: request_id.clone(),
            session_id: session_id.clone(),
            messages: vec![user_text_message(text)],
            sink,
            worker_catalog: Some(state.catalog.clone()),
            worker_event_sink,
            resend_seq: None,
            forced_dispatch: None,
            origin: None,
            resume_group_chat: None,
        },
    )
    .await
    .map_err(|e| ToolError::infra(format!("agent chat 受理失败: {}", e.message)))?;

    match acceptance {
        ChatAcceptance::Injected => Ok(json!({
            "injected": true,
            "session_id": session_id,
            "hint": HINT_INJECTED,
        })),
        other => {
            // 止损尽力而为;语义错误照报。
            let _ = cancel_chat_inner(state, request_id).await;
            let status = match other {
                ChatAcceptance::Started => "started",
                ChatAcceptance::Queued { .. } => "queued",
                ChatAcceptance::Injected => unreachable!(),
            };
            Err(ToolError::semantic(format!(
                "目标不是进行中的群聊讨论(acceptance={status});已止损取消本次请求。发起新讨论请用 start_discussion。"
            )))
        }
    }
}

/// list_models:目录只读透传(名字或 UUID 皆可作 start 引用,start 时解析)。
async fn tool_list_models(state: &Arc<AppState>) -> Result<Value, ToolError> {
    let models = db::list_models(&state.db)
        .await
        .map_err(|e| ToolError::infra(format!("list_models failed: {e}")))?;
    let models = models
        .into_iter()
        .map(|m| {
            json!({
                "id": m.model.id,
                "name": if m.model.display_name.is_empty() {
                    m.model.model_name.clone()
                } else {
                    m.model.display_name.clone()
                },
                "model_name": m.model.model_name,
                "provider": m.provider_display_name,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "models": models, "hint": HINT_MODELS }))
}

/// list_presets:合并预设视图只读内省。daemon 即数据源,degraded 恒
/// false(键保留——宿主 prompt 已习惯该形状,降级语义随收敛消失)。
async fn tool_list_presets(state: &Arc<AppState>) -> Result<Value, ToolError> {
    let eff = effective_presets(state).await?;
    let mut presets: Vec<Value> = eff
        .iter()
        .map(|(key, p)| {
            json!({
                "key": key,
                "name": p.display_name,
                "source": p.source,
                "description": p.description,
                "moderator": p.moderator_model,
                "participants": p.participants.iter().map(|x| json!({
                    "name": x.name,
                    "model": x.model,
                    "persona_chars": x.persona_md.chars().count(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    presets.sort_by(|a, b| {
        a.get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("key").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Ok(json!({
        "presets": presets,
        "degraded": false,
        "hint": HINT_PRESETS,
    }))
}

// ---------------------------------------------------------------------------
// MCP 惰性转录(D2)
// ---------------------------------------------------------------------------

/// topic slug(JS defaultTranscriptPath 同构):前 40 字符,非字母数字串
/// 折叠为 `-`,首尾 `-` 剥除,小写;空回退 `discussion`。
fn topic_slug(topic: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in topic.chars().take(40) {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "discussion".to_string()
    } else {
        trimmed
    }
}

/// MCP 转录落点:`{cwd}/out/group-chat-{slug}-{ts}.md`(转录留在证据基地,
/// 与定时场 `{data}/discussions/` 落点分叉——设计 §6 既定)。
fn transcript_target_path(session: &db::types::SessionRow, topic: &str) -> std::path::PathBuf {
    let root = if session.current_cwd.is_empty() {
        std::env::temp_dir()
    } else {
        std::path::PathBuf::from(&session.current_cwd)
    };
    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    root.join("out")
        .join(format!("group-chat-{}-{}.md", topic_slug(topic), ts))
}

/// 惰性导出(幂等性:落点含秒级 ts,重复导出 = 新文件;成本一个
/// markdown 文件,接受——ledger 记账已随收敛退役)。失败降级不抛错。
async fn export_mcp_transcript(state: &Arc<AppState>, loaded: &db::types::LoadedSession) -> Value {
    let session = &loaded.session;
    // topic = 首条 user 消息(截 40 字符口径与 slug 一致)。
    let topic: String = loaded
        .messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.text.chars().take(40).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "discussion".to_string());
    let target = transcript_target_path(session, &topic);

    let names = model_display_map(state).await.unwrap_or_default();
    let disp = |id: &str| -> String { names.get(id).cloned().unwrap_or_else(|| id.to_string()) };
    let meta = metadata_value(session);
    let participants: Vec<(String, String)> = meta
        .get("participants")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    Some((
                        p.get("name")?.as_str()?.to_string(),
                        disp(p.get("model")?.as_str()?),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let detail = session.discussion_detail.as_deref().and_then(|s| {
        serde_json::from_str::<crate::agent::discussion_detail::DiscussionDetail>(s).ok()
    });
    let now = chrono::Utc::now().to_rfc3339();
    let task_name: String = topic.chars().take(40).collect();
    let rendered = render_scheduled_transcript(&TranscriptRenderArgs {
        task_name: &task_name,
        session_id: &session.id,
        started_at: &session.created_at,
        ended_at: &now,
        moderator_label: &disp(&session.model),
        participants: &participants,
        stop_reason: session.stop_reason.as_deref().unwrap_or(""),
        messages: &loaded.messages,
        discussion_summary: session.discussion_summary.as_deref(),
        discussion_detail: detail.as_ref(),
    });

    let write = async {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&target, rendered).await
    };
    match write.await {
        Ok(()) => json!({ "transcript_path": target.to_string_lossy() }),
        Err(e) => json!({
            "transcript_path": Value::Null,
            "transcript_warning": format!("转录导出失败({e});讨论结论不受影响,可重试 discussion_result"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app(state: Arc<AppState>) -> Router {
        router(state)
    }

    async fn post_raw(
        app: &Router,
        body: &str,
        accept: &str,
        content_type: &str,
    ) -> (StatusCode, String) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("accept", accept)
                    .header("content-type", content_type)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    async fn post_rpc(app: &Router, body: &str) -> (StatusCode, Value) {
        let (status, text) = post_raw(
            app,
            body,
            "application/json, text/event-stream",
            "application/json",
        )
        .await;
        let v = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        (status, v)
    }

    /// 协议层测试用 state(tempdir keep 防自动清理;进程退出由 OS 收)。
    async fn test_state() -> Arc<AppState> {
        let path = tempfile::tempdir().unwrap().keep();
        Arc::new(AppState::load_from_dir(path).await)
    }

    /// 种子目录(四模型)的 state。
    async fn seeded_state() -> Arc<AppState> {
        let state = test_state().await;
        let provider =
            db::create_provider(&state.db, "anthropic", "测试供应商", "https://api.test", "")
                .await
                .unwrap();
        for name in ["Model-A", "Model-B", "Model-C", "Model-D"] {
            db::create_model(
                &state.db,
                &provider.id,
                name,
                name,
                None,
                None,
                true,
                false,
                128_000,
            )
            .await
            .unwrap();
        }
        state
    }

    async fn call(app: &Router, name: &str, args: Value) -> (StatusCode, Value) {
        post_rpc(
            app,
            &serde_json::to_string(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": name, "arguments": args }
            }))
            .unwrap(),
        )
        .await
    }

    fn tool_text(body: &Value) -> String {
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    // ---- 协议层 ----

    #[tokio::test(flavor = "multi_thread")]
    async fn initialize_echoes_supported_versions() {
        let app = app(test_state().await);
        for v in SUPPORTED_PROTOCOL_VERSIONS {
            let (code, body) = post_rpc(
                &app,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{v}","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#
                ),
            )
            .await;
            assert_eq!(code, StatusCode::OK, "version {v}");
            assert_eq!(body["result"]["protocolVersion"], v);
            assert!(body["result"]["capabilities"]["tools"].is_object());
            assert_eq!(body["result"]["serverInfo"]["name"], SERVER_NAME);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn initialize_unknown_version_falls_back() {
        let app = app(test_state().await);
        let (code, body) = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":"abc","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["id"], "abc"); // id 原样回显(字符串型)
        assert_eq!(body["result"]["protocolVersion"], DEFAULT_PROTOCOL_VERSION);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn notification_returns_202() {
        let app = app(test_state().await);
        let (code, text) = post_raw(
            &app,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "application/json, text/event-stream",
            "application/json",
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        assert!(text.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ping_returns_empty_result() {
        let app = app(test_state().await);
        let (code, body) = post_rpc(&app, r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["result"], json!({}));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_method_returns_minus_32601() {
        let app = app(test_state().await);
        let (_, body) = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#,
        )
        .await;
        assert_eq!(body["error"]["code"], ERR_METHOD_NOT_FOUND);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accept_header_missing_stream_half_returns_406() {
        let app = app(test_state().await);
        let (code, body) = post_raw(
            &app,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "application/json",
            "application/json",
        )
        .await;
        assert_eq!(code, StatusCode::NOT_ACCEPTABLE);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], ERR_TRANSPORT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_content_type_returns_415() {
        let app = app(test_state().await);
        let (code, _) = post_raw(
            &app,
            "{}",
            "application/json, text/event-stream",
            "text/plain",
        )
        .await;
        assert_eq!(code, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn content_type_charset_suffix_is_accepted() {
        let app = app(test_state().await);
        let (code, _) = post_raw(
            &app,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "application/json, text/event-stream",
            "application/json; charset=utf-8",
        )
        .await;
        assert_eq!(code, StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_json_returns_parse_error() {
        let app = app(test_state().await);
        let (code, body) = post_raw(
            &app,
            "{not json",
            "application/json, text/event-stream",
            "application/json",
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], ERR_PARSE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_rpc_shape_returns_parse_error() {
        let app = app(test_state().await);
        let (code, body) = post_raw(
            &app,
            r#"{"foo":1}"#,
            "application/json, text/event-stream",
            "application/json",
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], ERR_PARSE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_returns_405_and_delete_returns_200() {
        let app = app(test_state().await);
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/mcp").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_with_requests_returns_array() {
        let app = app(test_state().await);
        let (code, body) = post_rpc(
            &app,
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1); // 通知无应答
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[0]["result"], json!({}));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn client_response_message_is_ignored_as_notification() {
        let app = app(test_state().await);
        let (code, text) = post_raw(
            &app,
            r#"{"jsonrpc":"2.0","id":9,"result":{}}"#,
            "application/json, text/event-stream",
            "application/json",
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        assert!(text.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tools_call_unknown_tool_returns_minus_32602() {
        let app = app(test_state().await);
        let (_, body) = post_rpc(
            &app,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        )
        .await;
        assert_eq!(body["error"]["code"], ERR_INVALID_PARAMS);
    }

    // ---- tools/list wire schema ----

    #[tokio::test(flavor = "multi_thread")]
    async fn tools_list_has_eight_tools_with_object_schemas() {
        let app = app(test_state().await);
        let (_, body) = post_rpc(&app, r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#).await;
        let tools = body["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "start_discussion",
                "discussion_status",
                "discussion_result",
                "cancel_discussion",
                "interrupt_discussion",
                "inject_message",
                "list_models",
                "list_presets",
            ]
        );
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].is_string());
        }
    }

    /// AC4 预算锁(D7):单工具 JSON 序列化(name+description+inputSchema)
    /// 合计 ≤ 4200 字符——宿主注入 LLM context 的就是这份 wire schema
    /// (JS 壳同口径;升锁须过评审)。
    #[test]
    fn tools_list_budget_lock() {
        let total: usize = tool_defs()
            .iter()
            .map(|t| {
                serde_json::to_string(t)
                    .map(|s| s.len())
                    .unwrap_or(usize::MAX)
            })
            .sum();
        assert!(
            total <= TOOLS_BUDGET_CHARS,
            "tools/list wire 预算超限: {total} > {TOOLS_BUDGET_CHARS}"
        );
    }

    // ---- 纯逻辑:预设 ----

    #[test]
    fn builtin_presets_compose_deterministically() {
        let presets = builtin_presets().unwrap();
        let mut keys: Vec<&str> = presets.keys().map(|s| s.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["arch", "fe_review", "retro", "review"]);
        let review = &presets["review"];
        assert_eq!(review.source, "builtin");
        assert_eq!(review.display_name, "review");
        assert_eq!(review.participants.len(), 3);
        // persona 展开 = 边界 + 空行 + 公共纪律(JS composePersonaMd 同构)。
        let file = presets_file().unwrap();
        let first = &review.participants[0];
        let expected_kind = &file.presets["review"].participants[0].persona;
        assert_eq!(
            first.persona_md,
            format!(
                "{}\n\n{}",
                file.personas.get(expected_kind).unwrap(),
                file.persona_common
            )
        );
    }

    #[test]
    fn lookup_preset_three_ways() {
        let mut presets = builtin_presets().unwrap().clone();
        // 歧义:用户行 display_name 与内置忽略大小写相撞。
        presets.insert(
            "user-1".into(),
            EffectivePreset {
                description: String::new(),
                moderator_model: "m".into(),
                participants: vec![],
                source: "user",
                display_name: "REVIEW".into(),
            },
        );
        // key 直配(大小写敏感)。
        assert!(lookup_preset(&presets, "review").is_ok());
        // 忽略大小写歧义 → 报错。
        let err = lookup_preset(&presets, "Review").err().unwrap();
        match err {
            ToolError::Semantic(m) => assert!(m.contains("歧义")),
            _ => panic!("expected semantic ambiguity"),
        }
        // 无歧义时大小写不敏感命中。
        presets.remove("user-1").unwrap();
        let hit = lookup_preset(&presets, "Review").unwrap();
        assert_eq!(hit.display_name, "review");
        // miss 报可用清单(user 档带 id8 标注)。
        presets.insert(
            "11111111-2222".into(),
            EffectivePreset {
                description: String::new(),
                moderator_model: "m".into(),
                participants: vec![],
                source: "user",
                display_name: "我的评审团".into(),
            },
        );
        let err = lookup_preset(&presets, "nope").err().unwrap();
        match err {
            ToolError::Semantic(m) => {
                assert!(m.contains("未知预设"));
                assert!(m.contains("我的评审团(11111111)"));
            }
            _ => panic!("expected semantic"),
        }
    }

    #[test]
    fn normalize_model_ref_matches_id_exact_then_case_insensitive() {
        let models = vec![db::types::ModelWithProvider {
            model: db::types::ModelRow {
                id: "uuid-1".into(),
                provider_id: "p".into(),
                model_name: "GLM-5.3".into(),
                display_name: "GLM-5.3-Flash".into(),
                max_tokens: None,
                thinking_effort: None,
                supports_thinking: false,
                supports_images: false,
                context_window: 1,
                disabled: false,
                created_at: String::new(),
                updated_at: String::new(),
            },
            provider_display_name: "P".into(),
            provider_protocol: "anthropic".into(),
            provider_disabled: false,
        }];
        assert_eq!(normalize_model_ref(&models, "uuid-1").unwrap(), "uuid-1");
        assert_eq!(normalize_model_ref(&models, "GLM-5.3").unwrap(), "uuid-1");
        assert_eq!(
            normalize_model_ref(&models, "GLM-5.3-Flash").unwrap(),
            "uuid-1"
        );
        assert_eq!(normalize_model_ref(&models, "glm-5.3").unwrap(), "uuid-1");
        let err = normalize_model_ref(&models, "nope").err().unwrap();
        match err {
            ToolError::Semantic(m) => assert!(m.contains("不在目录")),
            _ => panic!("expected semantic"),
        }
    }

    #[test]
    fn lexical_absolute_folds_dots_and_trailing_slash() {
        assert_eq!(lexical_absolute("/repo/foo/"), "/repo/foo");
        assert_eq!(lexical_absolute("/repo/foo/./bar/../baz"), "/repo/foo/baz");
    }

    #[test]
    fn topic_slug_rules() {
        assert_eq!(
            topic_slug("怎么优化 LLM 内存?——一篇报告"),
            "怎么优化-llm-内存-一篇报告"
        );
        assert_eq!(topic_slug("---___"), "discussion");
        assert_eq!(topic_slug("ABC"), "abc");
        let long: String = "字".repeat(60);
        assert_eq!(topic_slug(&long).chars().count(), 40);
    }

    // ---- 工具层(错误路径,不进 LLM) ----

    #[tokio::test(flavor = "multi_thread")]
    async fn start_discussion_rejects_missing_topic_cwd_and_bad_budget() {
        let app = app(seeded_state().await);
        let (_, body) = call(&app, "start_discussion", json!({ "cwd": "/tmp" })).await;
        assert_eq!(body["result"]["isError"], true);
        assert!(tool_text(&body).contains("缺议题"));
        let (_, body) = call(&app, "start_discussion", json!({ "topic": "t" })).await;
        assert!(tool_text(&body).contains("缺工作目录"));
        let (_, body) = call(
            &app,
            "start_discussion",
            json!({ "topic": "t", "cwd": "/tmp", "token_budget": 0 }),
        )
        .await;
        assert!(tool_text(&body).contains("token_budget 必须是正整数"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn start_discussion_unknown_preset_lists_available() {
        let app = app(seeded_state().await);
        let (_, body) = call(
            &app,
            "start_discussion",
            json!({ "topic": "t", "cwd": "/tmp", "preset": "nope" }),
        )
        .await;
        let text = tool_text(&body);
        assert!(text.contains("未知预设"));
        assert!(text.contains("review"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn start_discussion_unknown_model_lists_catalog() {
        let app = app(seeded_state().await);
        let (_, body) = call(
            &app,
            "start_discussion",
            json!({ "topic": "t", "cwd": "/tmp", "preset": "review" }),
        )
        .await;
        // 内置 review 的 moderator(MiniMax-M3)不在种子目录 → 报清单。
        let text = tool_text(&body);
        assert!(text.contains("不在目录"), "got: {text}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn status_result_cancel_inject_interrupt_on_missing_or_idle_sessions() {
        let app = app(seeded_state().await);
        let (_, body) = call(&app, "discussion_status", json!({ "session_id": "ghost" })).await;
        assert!(tool_text(&body).contains("session 不存在"));
        let (_, body) = call(&app, "discussion_result", json!({ "session_id": "ghost" })).await;
        assert!(tool_text(&body).contains("session 不存在"));

        // 建一个真实但空闲的群聊 session(须先建 project——
        // create_session_in_pool 校验 project 存在)。
        let state = seeded_state().await;
        let app2 = router(state.clone());
        let proj_path = tempfile::tempdir().unwrap().keep();
        let project = create_project_inner(&state, proj_path.to_string_lossy().to_string())
            .await
            .unwrap();
        let sid = create_session_in_pool(
            &state.db,
            project.id,
            proj_path.to_string_lossy().to_string(),
            None,
            Some("group_chat".into()),
            Some(json!({ "participants": [] })),
        )
        .await
        .unwrap()
        .id;

        let (_, body) = call(&app2, "discussion_result", json!({ "session_id": sid })).await;
        assert!(tool_text(&body).contains("still running"));
        let (_, body) = call(&app2, "cancel_discussion", json!({ "session_id": sid })).await;
        let text = tool_text(&body);
        assert!(text.contains("already_finished"), "got: {text}");
        let (_, body) = call(
            &app2,
            "inject_message",
            json!({ "session_id": sid, "text": "hi" }),
        )
        .await;
        assert!(tool_text(&body).contains("目标不是进行中的群聊讨论"));
        let (_, body) = call(&app2, "interrupt_discussion", json!({ "session_id": sid })).await;
        assert!(tool_text(&body).contains("没有进行中的群聊讨论"));
        let (_, body) = call(
            &app2,
            "discussion_status",
            json!({ "session_id": sid, "wait_seconds": 0 }),
        )
        .await;
        assert!(tool_text(&body).contains("wait_seconds 必须是"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_models_and_list_presets_shapes() {
        let app = app(seeded_state().await);
        let (_, body) = call(&app, "list_models", json!({})).await;
        let parsed: Value = serde_json::from_str(&tool_text(&body)).unwrap();
        let models = parsed["models"].as_array().unwrap();
        // 首启播种默认目录,只锁种子行在场(数量随默认目录漂移)。
        let names: Vec<&str> = models.iter().filter_map(|m| m["name"].as_str()).collect();
        for seeded in ["Model-A", "Model-B", "Model-C", "Model-D"] {
            assert!(names.contains(&seeded), "missing {seeded} in {names:?}");
        }
        assert!(models[0]["id"].is_string());
        assert!(parsed["hint"].is_string());

        let (_, body) = call(&app, "list_presets", json!({})).await;
        let parsed: Value = serde_json::from_str(&tool_text(&body)).unwrap();
        assert_eq!(parsed["degraded"], false);
        let presets = parsed["presets"].as_array().unwrap();
        assert_eq!(presets.len(), 4); // 内置四档(无用户行)
        assert!(presets.iter().all(|p| p["source"] == "builtin"));
        assert!(presets
            .iter()
            .all(|p| p["participants"][0]["persona_chars"].is_u64()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn transcript_export_writes_under_cwd_out() {
        let state = test_state().await;
        let cwd = tempfile::tempdir().unwrap();
        let proj = cwd.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let loaded = db::types::LoadedSession {
            session: db::types::SessionRow {
                id: "sid-t".into(),
                title: "T".into(),
                created_at: "2026-09-14T00:00:00Z".into(),
                updated_at: "2026-09-14T00:00:00Z".into(),
                model: "m".into(),
                project_id: "p".into(),
                current_cwd: proj.to_string_lossy().to_string(),
                worktree_path: None,
                worktree_state: db::types::WorktreeState::None,
                last_worktree_path: None,
                model_id: None,
                input_tokens_total: None,
                output_tokens_total: None,
                cache_creation_total: None,
                cache_read_total: None,
                last_context_input_tokens: None,
                last_input_tokens: None,
                last_output_tokens: None,
                last_cache_creation: None,
                last_cache_read: None,
                color_tag: None,
                mode: db::types::Mode::Edit,
                workflow_enabled: false,
                plugin_name: String::new(),
                session_type: db::types::SessionType::GroupChat,
                metadata: Some(json!({ "participants": [] })),
                stop_reason: Some("group_chat_end".into()),
                discussion_summary: Some("结论".into()),
                discussion_detail: None,
            },
            messages: vec![db::types::MessageRow {
                id: 1,
                session_id: "sid-t".into(),
                role: "user".into(),
                content: json!({}),
                text: "讨论一下内存优化".into(),
                has_tool_calls: false,
                has_tool_results: false,
                created_at: String::new(),
                seq: 1,
                metadata: None,
                ttfb_ms: None,
                gen_ms: None,
                total_ms: None,
                thinking_ms: None,
                speaker: None,
                status: None,
            }],
        };
        let out = export_mcp_transcript(&state, &loaded).await;
        let path = out["transcript_path"].as_str().unwrap();
        assert!(
            path.starts_with(proj.to_string_lossy().as_ref()),
            "got {path}"
        );
        assert!(path.contains("/out/group-chat-"));
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("sid-t"));
        assert!(content.contains("结论"));
    }
}
