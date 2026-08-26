#![cfg(test)]

//! tools Stub 注册(D,`08-14-c7d-tools-stub-registration`)集成测试:
//! `load_tool_schemas` 拦截 / 直呼自愈 / 粘性 / 开关关 / worker 不 stub /
//! 群聊不 stub。纯函数单测在 `tools::stub::tests`(不变量 + 静态度量)。

use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, test_messages, MockEmitter,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};
use crate::llm::ToolDef;

/// 单轮 text-only 响应脚本。
fn text_turn(text: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: text.to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

/// 单轮 tool_use 响应脚本。
fn tool_turn(name: &str, input: serde_json::Value) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ToolCall {
            id: "toolu_stub".into(),
            name: name.to_string(),
            input,
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("tool_use".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

fn is_stub(def: &ToolDef) -> bool {
    def.input_schema == serde_json::json!({"type": "object"})
}

fn find_tool<'a>(turns: &'a [Vec<ToolDef>], turn_idx: usize, name: &str) -> &'a ToolDef {
    turns[turn_idx]
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("tool {name} missing from turn {turn_idx} tools"))
}

fn has_tool(turns: &[Vec<ToolDef>], turn_idx: usize, name: &str) -> bool {
    turns[turn_idx].iter().any(|t| t.name == name)
}

async fn stub_on(h: &super::tests_common::TestHarness) {
    crate::db::config::set_config_value(&h.db, "tools_stub_enabled", "true")
        .await
        .expect("set tools_stub_enabled=true");
}

// ---------------------------------------------------------------------------
// 1) load_tool_schemas 拦截:写 registry + 返回完整 schema + 粘性
// ---------------------------------------------------------------------------

/// AC2 核心路径:模型调 `load_tool_schemas(["web_fetch"])` → tool_result
/// 返回完整 schema JSON,registry 写入;下一 turn `web_fetch` 全量下发
/// (粘性)。stub 工具真实调用跑通见 live AC2(测试不触发真实副作用)。
#[tokio::test]
async fn agent_loop_load_tool_schemas_intercepts_and_sticky() {
    let h = make_harness().await;
    stub_on(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: 模型调 load_tool_schemas。
        tool_turn(
            "load_tool_schemas",
            serde_json::json!({"tool_names": ["web_fetch"]}),
        ),
        // Turn 2: 模型收 schema 后回话收尾。
        text_turn("ok"),
    ]));

    run_chat_loop(
        chat_loop_request(
            crate::tools::builtin_tools(),
            mock.clone(),
            200_000,
            "rid-stub-load".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    // tool_result 含完整 schema JSON(web_fetch 真名在场)。envelope 是
    // JSON 序列化,内层 schema 引号被转义 — 用转义免疫的宽松匹配。
    let results = emitter.tool_results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "exactly one tool_result (load_tool_schemas)"
    );
    assert!(!results[0].is_error, "load_tool_schemas 成功非 error");
    assert!(
        results[0].content.contains("web_fetch"),
        "tool_result 应含 web_fetch 完整 def"
    );
    assert!(
        results[0].content.contains("input_schema"),
        "tool_result 应含 input_schema"
    );

    // registry 粘性写入。
    let loaded = h.stub_loaded.get(&h.session_id).await;
    assert!(
        loaded.contains("web_fetch"),
        "load_tool_schemas 后 registry 应含 web_fetch"
    );

    // turn1: web_fetch 是 stub,load_tool_schemas 在场;turn2: web_fetch
    // 全量(粘性),load_tool_schemas 仍在。
    let sent = mock.sent_tools();
    assert_eq!(sent.len(), 2, "two sends");
    assert!(
        is_stub(find_tool(&sent, 0, "web_fetch")),
        "turn1 web_fetch 应被 stub"
    );
    assert!(
        has_tool(&sent, 0, "load_tool_schemas"),
        "turn1 应有 load_tool_schemas"
    );
    assert!(
        !is_stub(find_tool(&sent, 1, "web_fetch")),
        "turn2(load 后)web_fetch 应全量 — 粘性"
    );
    assert!(
        has_tool(&sent, 1, "load_tool_schemas"),
        "turn2 load_tool_schemas 应保留"
    );

    // AC4 跨 request 粘性:第二条用户消息(新 run_chat_loop,同一 registry)
    // 后 web_fetch 仍全量下发。
    let emitter2 = Arc::new(MockEmitter::new());
    let mock2 = Arc::new(MockProvider::new(vec![text_turn("second request")]));
    run_chat_loop(
        chat_loop_request(
            crate::tools::builtin_tools(),
            mock2.clone(),
            200_000,
            "rid-stub-load2".into(),
            h.session_id.clone(),
            test_messages(),
            emitter2.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;
    let sent2 = mock2.sent_tools();
    assert_eq!(sent2.len(), 1);
    assert!(
        !is_stub(find_tool(&sent2, 0, "web_fetch")),
        "跨 request 后 web_fetch 仍全量 — AC4 粘性"
    );
}

// ---------------------------------------------------------------------------
// 2) 直呼自愈:模型未 load 直呼 stub 工具 → error + schema 回灌 + loaded
// ---------------------------------------------------------------------------

/// 模型未 load 就直呼 `web_fetch`(stub schema 下常见):serial 顶部拦截
/// 返回 is_error + 完整 schema + "schema now loaded, retry",并写
/// loaded-set;下一 turn 该工具全量下发。
#[tokio::test]
async fn agent_loop_stub_direct_call_self_heals() {
    let h = make_harness().await;
    stub_on(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_turn("web_fetch", serde_json::json!({})),
        text_turn("ok"),
    ]));

    run_chat_loop(
        chat_loop_request(
            crate::tools::builtin_tools(),
            mock.clone(),
            200_000,
            "rid-stub-selfheal".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 1);
    assert!(results[0].is_error, "直呼 stub 应返回 error");
    assert!(
        results[0].content.contains("schema now loaded"),
        "error 应含 retry 指引"
    );
    assert!(
        results[0].content.contains("web_fetch"),
        "error 应回灌完整 schema(转义免疫匹配)"
    );

    let loaded = h.stub_loaded.get(&h.session_id).await;
    assert!(loaded.contains("web_fetch"), "自愈应写 registry");

    let sent = mock.sent_tools();
    assert!(
        !is_stub(find_tool(&sent, 1, "web_fetch")),
        "自愈后下一 turn web_fetch 全量(粘性)"
    );
}

// ---------------------------------------------------------------------------
// 3) 开关关:全量直通,无 load_tool_schemas(AC5 静态面)
// ---------------------------------------------------------------------------

/// make_harness 默认置 `tools_stub_enabled=false`;开关关时第 4 环直通
/// (候选全量)+ 不 append `load_tool_schemas`。
#[tokio::test]
async fn agent_loop_stub_off_full_schemas_no_meta_tool() {
    let h = make_harness().await; // 默认 false
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![text_turn("hi")]));

    run_chat_loop(
        chat_loop_request(
            crate::tools::builtin_tools(),
            mock.clone(),
            200_000,
            "rid-stub-off".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    let sent = mock.sent_tools();
    assert_eq!(sent.len(), 1);
    assert!(
        !is_stub(find_tool(&sent, 0, "web_fetch")),
        "开关关:web_fetch 全量"
    );
    assert!(
        !has_tool(&sent, 0, "load_tool_schemas"),
        "开关关:无 load_tool_schemas"
    );
}

// ---------------------------------------------------------------------------
// 4) worker 不 stub(gate !effective_is_worker)
// ---------------------------------------------------------------------------

/// worker 路径(开关开)也不 stub:候选全量 + 无 load_tool_schemas —
/// worker 自主可靠性优先(prd Decision 3)。
#[tokio::test]
async fn agent_loop_worker_never_stubbed() {
    let h = make_harness().await;
    stub_on(&h).await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![text_turn("worker done")]));

    run_chat_loop(
        chat_loop_request(
            crate::tools::builtin_tools(),
            mock.clone(),
            200_000,
            "rid-stub-worker".into(),
            h.session_id.clone(),
            test_messages(),
            emitter.clone(),
        ),
        chat_loop_deps(&h),
        {
            let mut role = parent_role(&h);
            // worker 路径:skip_session_active=true / skip_persist=true /
            // is_worker=Some(true) / system_prompt_override=Some(worker prompt)。
            role.skip_session_active = true;
            role.skip_persist = true;
            role.is_worker = Some(true);
            role.system_prompt_override = Some("worker system prompt".to_string());
            role.worker_run_id = Some("worker-run-id".to_string());
            role
        },
    )
    .await;

    let sent = mock.sent_tools();
    assert_eq!(sent.len(), 1);
    assert!(
        !is_stub(find_tool(&sent, 0, "web_fetch")),
        "worker 不 stub:web_fetch 全量"
    );
    assert!(
        !has_tool(&sent, 0, "load_tool_schemas"),
        "worker 无 load_tool_schemas"
    );
}

// ---------------------------------------------------------------------------
// 5) 群聊不 stub(gate !is_group_chat,评审 P1-1 回归锚)
// ---------------------------------------------------------------------------

/// 群聊(开关开)也不 stub:候选全量 + 无 load_tool_schemas — 群聊复用
/// 同一 `run_chat_loop`,白名单含候选 `web_fetch`;gate 是唯一防线
/// (`loaded_session.session_type == GroupChat`),stubify/append 均跳过。
#[tokio::test]
async fn agent_loop_group_chat_never_stubbed() {
    let h = make_harness().await;
    stub_on(&h).await;
    // 建一个 GroupChat session(与 harness 默认 classic session 区分)。
    let gc_sid = format!("gc-{}", uuid::Uuid::new_v4());
    let loaded = crate::db::create_session(
        &h.db,
        &gc_sid,
        &h.project_id,
        h.project_path.to_str().unwrap(),
        "mock-model",
        None,
        Some("group_chat"),
        Some(r#"{"participants":[{"name":"m1"}]}"#),
    )
    .await
    .expect("create group_chat session");

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![text_turn("moderator done")]));
    let turn_state: crate::tools::nominate_speaker::SharedTurnState = Arc::new(
        tokio::sync::Mutex::new(crate::tools::nominate_speaker::GroupChatTurnState::default()),
    );

    run_chat_loop(
        {
            let mut request = chat_loop_request(
                crate::tools::builtin_tools(),
                mock.clone(),
                200_000,
                "rid-stub-group".into(),
                gc_sid.clone(),
                test_messages(),
                emitter.clone(),
            );
            request.max_turns = Some(1);
            request.group_chat_state = Some(turn_state);
            request.current_speaker = Some("moderator".to_string());
            request
        },
        chat_loop_deps(&h),
        {
            let mut role = parent_role(&h);
            role.system_prompt_override = Some("moderator prompt".to_string());
            role
        },
    )
    .await;

    let sent = mock.sent_tools();
    assert_eq!(sent.len(), 1);
    assert!(
        !is_stub(find_tool(&sent, 0, "web_fetch")),
        "群聊不 stub:web_fetch 全量"
    );
    assert!(
        !has_tool(&sent, 0, "load_tool_schemas"),
        "群聊无 load_tool_schemas"
    );
    // 群聊 registry 不被读写(空)。
    assert!(
        h.stub_loaded.get(&gc_sid).await.is_empty(),
        "群聊 loaded-set 不写入"
    );
    let _ = loaded;
}
