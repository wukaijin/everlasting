#![cfg(test)]
// MAX_TURNS softcap 集成测试(08-18-max-turns-softcap,design §5)。
//
// 公共口径:
// - `max_turns = Some(3)`(continue/compact-force)/ `Some(1..2)`(其余):
//   撞线在循环边界(turn = budget+1),不消耗 MockProvider 脚本;
// - 每轮 tool 调用签名互不相同(list_dir 三不同 path)——
//   loop_detection 的 hard 需要 HARD_WINDOW=3 连续**相同**签名、
//   soft 需要 Jaccard > 0.85,不同 input 使 C2/C2+ 全程静默(否则
//   C2+ 卡会先占 QuestionStore 槽位,软卡 register 撞 AlreadyPending);
// - resolver 模板抄 basic.rs:859-893 的 spawn 轮询,按
//   `get_payload().kind` 分流(InteractionKind 判等);
// - 并行串行锁:`EVERLASTING_SOFTCAP_TIMEOUT_MS` 是进程级 env,
//   timeout 测试设置期间其余 softcap 测试不得并行进入询问(否则
//   缺省 10min 超时被压成 50ms)。锁内 7 个用例串行,总耗时 < 2s。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, make_harness, parent_role, MockEmitter, TestHarness,
};
use crate::agent::chat_loop::{run_chat_loop, TURN_LIMIT_GRANT};
use crate::agent::compaction::{compaction_registry, COMPACTION_SUMMARY_KIND};
use crate::agent::question_store::{
    InteractionKind, InteractionResponse, PendingInteraction, QuestionAnswer,
};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, ChatMessage, MessageContent, Role, TokenUsage};

/// 大窗口(200k):auto 摘要触发线 0.85×window = 170k,本文件的短
/// 历史全程低于线 —— compact-force 用例的 force flag 是唯一的摘要
/// 触发源(这正是 force 的语义:绕过 token 触发线)。
const WINDOW: u32 = 200_000;

/// 软卡测试串行锁(见文件头注释)。
static SOFTCAP_ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

async fn softcap_serial() -> tokio::sync::MutexGuard<'static, ()> {
    SOFTCAP_ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// env 泄漏护栏:断言失败 early-return 也会清掉测试专用的 env(否则
/// 后续(并行)测试的软卡询问会读到畸形的超时/边界值)。
struct EnvVarGuard(&'static str);
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

/// 第 i 轮的 tool_use 响应:list_dir + 互不相同的 path(签名互异,
/// C2/C2+ 静默,见文件头)。
fn tool_use_response(i: usize) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ToolCall {
            id: format!("toolu_softcap_{i}"),
            name: "list_dir".into(),
            input: serde_json::json!({"path": format!("./dir_{i}")}),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("tool_use".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

/// 主 turn 的文本终态响应。
fn end_turn_response(text: &str) -> MockResponse {
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

/// 摘要旁路 completion 的成功脚本(compaction_summary.rs 同款形态)。
fn summary_response(body: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta {
            text: body.to_string(),
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage {
                input_tokens: 9_100,
                output_tokens: 500,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                context_input_tokens: 9_100,
            }),
        }),
    ])
}

/// 该 send 是否摘要旁路调用(首条消息是压缩 prompt)。
fn is_summary_send(msgs: &[ChatMessage]) -> bool {
    msgs.first().is_some_and(|m| {
        m.content
            .to_text()
            .contains("CONTEXT CHECKPOINT COMPACTION")
    })
}

fn user(text: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text.into()),
        speaker: None,
        attachments: None,
    }
}

fn assistant(text: impl Into<String>) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Text(text.into()),
        speaker: None,
        attachments: None,
    }
}

fn pad(n_chars: usize) -> String {
    "the quick brown fox jumps over the lazy dog. "
        .repeat(n_chars / 45 + 1)
        .chars()
        .take(n_chars)
        .collect()
}

/// 「继续」选项的精确 label(与 ask_turn_limit_softcap 的构建一致:
/// `继续(+{TURN_LIMIT_GRANT} 轮)`)。
fn continue_label() -> String {
    format!("继续(+{} 轮)", TURN_LIMIT_GRANT)
}

/// 按 kind 分流的 resolver(basic.rs:859-893 spawn 轮询模板):目标
/// kind 出现 → resolve 一个选 `label` 的单选答案;其余 kind(本文件
/// 不应出现)继续轮询。永远循环(spawn 泄漏由测试 runtime 回收,
/// 与 basic.rs 同款)。
fn spawn_resolver(
    store: crate::agent::question_store::QuestionStore,
    session_id: String,
    kind: InteractionKind,
    label: String,
) {
    tokio::spawn(async move {
        loop {
            if let Some(entry) = store.get_payload(&session_id).await {
                if entry.kind == kind {
                    // 稍等片刻让 register→emit→select 就绪(basic.rs 同款)。
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    let _ = store
                        .resolve(
                            &session_id,
                            InteractionResponse::Answered(
                                serde_json::to_value(vec![QuestionAnswer {
                                    question: String::new(),
                                    header: None,
                                    options: vec![label.clone()],
                                    multi_select: false,
                                    custom: None,
                                }])
                                .unwrap(),
                            ),
                        )
                        .await;
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    continue;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
}

/// 软卡 audit 行的 action 列表(插入序:按自增 id 排序 —— ts 只有
/// 秒级精度,同秒两行的 DESC 相对顺序不稳定)。
async fn softcap_audit_actions(h: &TestHarness) -> Vec<String> {
    let mut rows: Vec<(i64, String)> = crate::db::list_audit_events(&h.db, &h.session_id)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.kind == "turn_limit_softcap")
        .filter_map(|r| {
            serde_json::from_str::<serde_json::Value>(&r.payload_json.clone().unwrap_or_default())
                .ok()
                .and_then(|v| {
                    v.get("action")
                        .and_then(|a| a.as_str())
                        .map(|s| s.to_string())
                })
                .map(|action| (r.id, action))
        })
        .collect();
    rows.sort_by_key(|(id, _)| *id);
    rows.into_iter().map(|(_, a)| a).collect()
}

/// Done 事件的 stop_reason 序列(含 per-turn tool_use Done)。
fn done_reasons(emitter: &MockEmitter) -> Vec<String> {
    emitter
        .chat_events()
        .iter()
        .filter_map(|p| match &p.event {
            ChatEvent::Done { stop_reason, .. } => stop_reason.clone(),
            _ => None,
        })
        .collect()
}

/// 标准 run_chat_loop 调用(compaction_summary.rs 的 run_loop 模板,
/// fixture 缺省 + max_turns / token / is_worker / skip_persist 可调)。
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    h: &TestHarness,
    provider: Arc<MockProvider>,
    rid: &str,
    messages: Vec<ChatMessage>,
    emitter: Arc<MockEmitter>,
    max_turns: Option<usize>,
    token: CancellationToken,
    is_worker: bool,
    skip_persist: bool,
) {
    run_chat_loop(
        {
            let mut request = chat_loop_request(
                vec![],
                provider,
                WINDOW,
                rid.into(),
                h.session_id.clone(),
                messages,
                emitter.clone(),
            );
            request.max_turns = max_turns;
            request
        },
        {
            let mut deps = chat_loop_deps(&h);
            deps.token = token;
            deps
        },
        {
            let mut role = parent_role(&h);
            role.skip_persist = skip_persist;
            role.is_worker = Some(is_worker);
            role
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// 1) 「继续(+200 轮)」:预算加成后 loop 续跑(AC1/AC2)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_continue_extends_budget() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
        tool_use_response(2),
        // 撞线询问(不消耗脚本)后,「继续」加成 → 第 4 次 send 收尾。
        end_turn_response("continued past the cap"),
    ]));
    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        InteractionKind::TurnLimitSoftcap,
        continue_label(),
    );

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-cont",
        vec![user("hello")],
        emitter.clone(),
        Some(3),
        CancellationToken::new(),
        false,
        false,
    )
    .await;

    assert_eq!(mock.call_count(), 4, "3 轮 + 加成后的第 4 轮");
    assert_eq!(
        emitter.max_turns_done_count(),
        0,
        "继续不产生 max_turns 终态"
    );
    let dones = done_reasons(&emitter);
    assert_eq!(dones.last(), Some(&"end_turn".to_string()));
    let actions = softcap_audit_actions(&h).await;
    assert_eq!(actions, vec!["asked".to_string(), "continued".to_string()]);
}

// ---------------------------------------------------------------------------
// 2) 「停止」:与今日 max_turns 终态等价(AC2)—— 恰好一次
//    Done{max_turns}、send_count = budget、persist/touch 完整。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_stop_emits_max_turns_terminal() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
        tool_use_response(2),
    ]));
    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        InteractionKind::TurnLimitSoftcap,
        "停止".to_string(),
    );

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-stop",
        vec![user("hello")],
        emitter.clone(),
        Some(3),
        CancellationToken::new(),
        false,
        false,
    )
    .await;

    assert_eq!(mock.call_count(), 3, "send_count = budget(询问不消耗脚本)");
    assert_eq!(
        emitter.max_turns_done_count(),
        1,
        "恰好一次 Done(max_turns)"
    );
    let actions = softcap_audit_actions(&h).await;
    assert_eq!(actions, vec!["asked".to_string(), "stopped".to_string()]);
    // cwd/touch 落库(skip_persist=false):DB 行 = 当前输入 + 3×
    // (assistant + tool_result) = 7;session 行 updated_at 非空。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("session loaded");
    assert_eq!(loaded.messages.len(), 7, "persist 完整(无孤儿)");
    assert!(!loaded.session.updated_at.is_empty(), "touch_session 落库");
}

// ---------------------------------------------------------------------------
// 3) 超时:询问无响应 → 停止(AC3;超时经 env 调短)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_timeout_stops() {
    let _g = softcap_serial().await;
    std::env::set_var("EVERLASTING_SOFTCAP_TIMEOUT_MS", "50");
    let _env = EnvVarGuard("EVERLASTING_SOFTCAP_TIMEOUT_MS");

    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![tool_use_response(0)]));
    // 无 resolver:询问挂起 50ms 后超时停止。

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-timeout",
        vec![user("hello")],
        emitter.clone(),
        Some(1),
        CancellationToken::new(),
        false,
        false,
    )
    .await;

    assert_eq!(mock.call_count(), 1);
    assert_eq!(emitter.max_turns_done_count(), 1, "超时 → max_turns 终态");
    let actions = softcap_audit_actions(&h).await;
    assert_eq!(
        actions,
        vec!["asked".to_string(), "timeout_stopped".to_string()]
    );
    // 槽位已清(超时臂 remove)。
    assert!(h.question_store.get_payload(&h.session_id).await.is_none());
}

// ---------------------------------------------------------------------------
// 4) pending 期间 Stop → cancelled(AC3):槽位清、Done{cancelled}。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_cancel_during_ask() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![tool_use_response(0)]));

    let token = CancellationToken::new();
    let cancel_token = token.clone();
    let store_obs = h.question_store.clone();
    let sid_obs = h.session_id.clone();
    tokio::spawn(async move {
        loop {
            if store_obs.get_payload(&sid_obs).await.is_some() {
                cancel_token.cancel();
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
    });

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-cancel",
        vec![user("hello")],
        emitter.clone(),
        Some(1),
        token,
        false,
        false,
    )
    .await;

    assert_eq!(mock.call_count(), 1);
    assert_eq!(emitter.cancel_done_count(), 1, "Done(cancelled) 一次");
    assert_eq!(emitter.max_turns_done_count(), 0);
    // 槽位已清(cancel 臂 remove)。
    assert!(h.question_store.get_payload(&h.session_id).await.is_none());
    let actions = softcap_audit_actions(&h).await;
    assert_eq!(actions, vec!["asked".to_string(), "cancelled".to_string()]);
}

// ---------------------------------------------------------------------------
// 5) 「压缩后续跑」:force 摘要压缩(trigger=softcap)+ 续跑(AC1/R6)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_compact_continue_forces_compaction() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    compaction_registry().clear(&h.session_id).await;
    // 打开摘要压缩 gate(harness 默认关)。
    crate::db::config::set_config_value(&h.db, "llm_compaction_enabled", "true")
        .await
        .expect("set llm_compaction_enabled=true");

    // 预落 30 条 pad 历史(≈ 45k token,远低于 170k 触发线 —— turn1-3
    // 的 auto 路径静默;force 是唯一触发源)+ 尾部当前输入。
    let mut seed_rows: Vec<ChatMessage> = (0..30)
        .map(|i| {
            if i % 2 == 0 {
                user(format!("HIST_{:02} {}", i, pad(6_000)))
            } else {
                assistant(format!("HIST_{:02} {}", i, pad(6_000)))
            }
        })
        .collect();
    for (i, m) in seed_rows.iter().enumerate() {
        crate::db::persist_turn(
            &h.db,
            &h.session_id,
            m.role,
            &m.content,
            i as i64,
            None,
            None,
        )
        .await
        .expect("seed history row");
    }
    let mut wire = std::mem::take(&mut seed_rows);
    wire.push(user("current question"));

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
        tool_use_response(2),
        // 撞线 →「压缩后续跑」→ 下一 turn 的 drive_turn force 出一次
        // 摘要旁路 completion(消耗本条),再走主 turn。
        summary_response("SOFTCAP_FORCED_SUMMARY"),
        end_turn_response("after forced compaction"),
    ]));
    spawn_resolver(
        h.question_store.clone(),
        h.session_id.clone(),
        InteractionKind::TurnLimitSoftcap,
        "压缩后续跑".to_string(),
    );

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-compact",
        wire,
        emitter.clone(),
        Some(3),
        CancellationToken::new(),
        false,
        false,
    )
    .await;

    // 3 轮 tool + 1 次摘要旁路 + 1 轮主 turn;摘要在主 turn 之前。
    assert_eq!(mock.call_count(), 5);
    let sends = mock.sent_messages();
    assert!(is_summary_send(&sends[3]), "第 4 次 send 是 force 摘要");
    assert!(!is_summary_send(&sends[4]));
    assert_eq!(emitter.max_turns_done_count(), 0, "续跑收尾非 max_turns");
    assert_eq!(done_reasons(&emitter).last(), Some(&"end_turn".to_string()));
    let actions = softcap_audit_actions(&h).await;
    assert_eq!(
        actions,
        vec!["asked".to_string(), "compacted_continued".to_string()]
    );

    // DB:摘要行落库且 trigger 标注 softcap(与 auto 区分)。
    let loaded = crate::db::load_session(&h.db, &h.session_id)
        .await
        .unwrap()
        .expect("session loaded");
    let summary_row = loaded
        .messages
        .iter()
        .rev()
        .find(|r| {
            crate::agent::compaction::message_metadata_kind(r.metadata.as_ref())
                == Some(COMPACTION_SUMMARY_KIND)
        })
        .expect("force 摘要行已落库");
    let meta = summary_row.metadata.as_ref().unwrap();
    assert_eq!(meta["trigger"], "softcap", "观测可区分 force 路径");
    assert_eq!(summary_row.text, "SOFTCAP_FORCED_SUMMARY");

    compaction_registry().clear(&h.session_id).await;
}

// ---------------------------------------------------------------------------
// 6) gate 关闭:卡片只有继续/停止两支(决议 4 / AC1)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_gate_off_hides_compact_option() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    // harness 默认 llm_compaction_enabled=false → compaction_on=false。
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
    ]));

    // observer:拿到 payload 快照后回传主任务,并 resolve「停止」
    // 结束 loop(避免 10 分钟挂起)。
    let (tx, rx) = tokio::sync::oneshot::channel();
    let store_obs = h.question_store.clone();
    let sid_obs = h.session_id.clone();
    tokio::spawn(async move {
        loop {
            if let Some(entry) = store_obs.get_payload(&sid_obs).await {
                let _ = tx.send(entry);
                let _ = store_obs
                    .resolve(
                        &sid_obs,
                        InteractionResponse::Answered(
                            serde_json::to_value(vec![QuestionAnswer {
                                question: String::new(),
                                header: None,
                                options: vec!["停止".to_string()],
                                multi_select: false,
                                custom: None,
                            }])
                            .unwrap(),
                        ),
                    )
                    .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
    });

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-gateoff",
        vec![user("hello")],
        emitter.clone(),
        Some(2),
        CancellationToken::new(),
        false,
        false,
    )
    .await;

    let entry = rx.await.expect("observer captured the softcap entry");
    assert_eq!(entry.kind, InteractionKind::TurnLimitSoftcap);
    match entry.payload {
        PendingInteraction::TurnLimitSoftcap(p) => {
            assert_eq!(p.questions.len(), 1);
            let opts = &p.questions[0].options;
            assert_eq!(opts.len(), 2, "gate 关闭 → 两选项(继续/停止)");
            assert!(
                opts[0].label.starts_with("继续"),
                "首选项是继续(got {})",
                opts[0].label
            );
            assert!(
                !opts.iter().any(|o| o.label.contains("压缩")),
                "不展示压缩选项"
            );
            assert_eq!(opts[1].label, "停止");
        }
        other => panic!("expected TurnLimitSoftcap payload, got {other:?}"),
    }
    // 浮动卡 IPC 事件发出(tool_use_id 前缀 turn_limit_softcap_)。
    let questions = emitter.tool_questions_snapshot();
    assert_eq!(questions.len(), 1);
    assert!(questions[0].tool_use_id.starts_with("turn_limit_softcap_"));
}

// ---------------------------------------------------------------------------
// 7) worker 臂:撞线直接 break(AC4)—— 无询问、无 max_turns 终态
//    emit(worker skip_persist 语义与今日一致,硬卡零回归)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_worker_breaks_without_ask() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
    ]));

    run_loop(
        &h,
        mock.clone(),
        "rid-sc-worker",
        vec![user("hello")],
        emitter.clone(),
        Some(2),
        CancellationToken::new(),
        true, // is_worker → effective_is_worker=true
        true, // skip_persist(worker 语义)
    )
    .await;

    assert_eq!(mock.call_count(), 2, "worker 跑满预算");
    assert_eq!(emitter.tool_question_count(), 0, "worker 撞线不弹软卡询问");
    assert_eq!(
        emitter.max_turns_done_count(),
        0,
        "worker skip_persist 不 emit max_turns 终态(与今日一致)"
    );
    assert!(h.question_store.get_payload(&h.session_id).await.is_none());
}

// ---------------------------------------------------------------------------
// 8) 群聊臂(R4):per-speaker 段撞线直接 break —— 无询问,终态与
//    今日一致(group_chat_loop 以 max_turns=1 复用 run_chat_loop,
//    预算语义归 30 轮编排管,不归软卡管)。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn softcap_group_chat_breaks_without_ask() {
    let _g = softcap_serial().await;
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        tool_use_response(0),
        tool_use_response(1),
    ]));
    let turn_state: crate::tools::nominate_speaker::SharedTurnState = Arc::new(
        tokio::sync::Mutex::new(crate::tools::nominate_speaker::GroupChatTurnState::default()),
    );

    run_chat_loop(
        {
            let mut request = chat_loop_request(
                vec![],
                mock.clone(),
                WINDOW,
                "rid-sc-group".into(),
                h.session_id.clone(),
                vec![user("hello")],
                emitter.clone(),
            );
            request.max_turns = Some(2);
            // group_chat_state = Some → 群聊 speaker 段,软卡不触发。
            request.group_chat_state = Some(turn_state);
            request
        },
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;

    assert_eq!(mock.call_count(), 2, "speaker 段跑满预算");
    assert_eq!(
        emitter.tool_question_count(),
        0,
        "群聊 speaker 段撞线不弹软卡询问"
    );
    // 与今日一致:非 skip_persist 的群聊段撞线 emit 一次 max_turns 终态。
    assert_eq!(emitter.max_turns_done_count(), 1);
    assert!(h.question_store.get_payload(&h.session_id).await.is_none());
    assert!(
        softcap_audit_actions(&h).await.is_empty(),
        "群聊路径不落软卡 audit 行"
    );
}
