//! F1 消息队列(2026-08-25)— 输入侧 per-session FIFO。
//!
//! 经典聊天 session 的 turn 进行中,新发送不再走"取消替换",而是
//! 入队等待;当前轮正常结束后由驱动器(`agent/chat.rs` 的
//! `run_queue_driver`)drain 全队、按序批量注入为下一个 turn。
//!
//! # 数据形态
//!
//! 队列本体挂在 [`crate::state::AppState::message_queues`]
//! (`Arc<Mutex<HashMap<session_id, VecDeque<QueuedMessage>>>>`)。
//! 每个排队项持有**完整的尾部 user [`ChatMessage`]**(含 B1
//! `attachments` 引用)——与 `chat_inner` 收到的请求尾条同构,
//! 注入时经 D-D 持久化点自然落库(排队项未落过库,guard 不会
//! 误判跳过)。
//!
//! # 生命周期(纯内存)
//!
//! 入队仅内存;注入时才持久化。崩溃/重启丢失 = 与 composer 未
//! 发送文本同等风险姿态(PRD D6 已决)。`delete_session` /
//! `clear_session_messages` / Stop(cancel)负责清空。
//!
//! # 上限
//!
//! `SESSION_QUEUE_MAX` 有界防连发打爆内存(PRD AC6);超限返回
//! [`QueueError::Full`],前端 toast 提示。
//!
//! # 并发约定(设计 §2 锁纪律)
//!
//! 路由判定("忙 → 仅入队 / 闲 → 入队 + 认领 slot")要求
//! `message_queues` 与 `session_active_request` 两把锁在**同一
//! 临界区**内成对获取。全仓固定加锁顺序:**先
//! `message_queues`,后 `session_active_request`**;临界区内不得
//! await(DB / 网络)。其余路径(delete_session 清队、cancel 链)
/// 只碰其中一把,无反序锁。
use crate::agent::permissions::PermissionAskPayload;
use crate::llm::types::{ChatEvent, ChatMessage};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared queue-map handle as stored on `AppState`.
pub type SharedQueues = Arc<Mutex<HashMap<String, VecDeque<QueuedMessage>>>>;

/// Per-session queue capacity (PRD AC6). Beyond this the enqueue is
/// rejected with [`QueueError::Full`] — bounded memory against a
/// runaway client.
pub const SESSION_QUEUE_MAX: usize = 20;

/// Continuation rounds per driver request (design §1). Mirrors the
/// group-chat orchestrator's `MAX_ORCHESTRATION_ROUNDS = 30` safety
/// bound; on hitting it the driver exits Done-with-retained-queue
/// (same semantics as error termination — design §4).
pub const MAX_CONTINUATION_ROUNDS: usize = 50;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct QueuedMessage {
    /// Stable uuid (NOT position — positions shift as siblings are
    /// revoked). R8 revoke/recall address entries by this id.
    pub id: String,
    /// The full tail user message as received from the frontend
    /// (text blocks + B1 attachment refs). Persisted verbatim at
    /// injection time via run_chat_loop's entry persist site.
    pub message: ChatMessage,
    /// Epoch millis, observability only (`list_queued_messages`).
    pub enqueued_at: i64,
    /// R6 placeholder — MVP scheduling is pure FIFO; this field does
    /// NOT participate in ordering. Do not branch on it without
    /// opening the priority tier (ROADMAP B 档).
    pub priority: u8,
    /// F2 定时任务(2026-08-28, `08-28-f2-scheduled-tasks` design §4.1):
    /// 消息来源标记。Additive — 仅调度器 fire 路径经
    /// [`push_with_origin`] 塞 `Some`,其余路径恒 `None`(序列化进
    /// `list_queued_messages` 排队占位 IPC,前端显示「定时」徽标;
    /// **不进** chat 事件主链)。载荷必须在队列项上:忙时 fire 的条目
    /// 由*另一个*请求的驱动器在 round>0 消费,请求级上下文(resend_seq /
    /// forced_dispatch 同款)在 round>0 一律丢弃。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<crate::scheduler::TaskOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// Session queue at `SESSION_QUEUE_MAX`.
    Full,
    /// R8 revoke/recall target not found — either already drained by
    /// the driver (injection started) or revoked by another surface.
    NotFound,
}

impl std::fmt::Display for QueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueueError::Full => write!(f, "排队已满({SESSION_QUEUE_MAX} 条上限)"),
            QueueError::NotFound => write!(f, "消息已开始处理或已被移除"),
        }
    }
}

/// Append one message onto a queue VecDeque. Sync by design: the
/// chat_inner routing critical section holds the `message_queues`
/// lock across the busy-check + slot-claim sequence (module docs) and
/// calls this while still holding the guard. Returns the entry uuid;
/// position = queue length AFTER push (1-based tail).
///
/// F2 (2026-08-28): the origin param is mandatory at the single
/// production call site (chat_inner routing critical section passes the
/// entry's `ChatEntry.origin`, `None` for every user send).
pub fn push_with_origin(
    q: &mut VecDeque<QueuedMessage>,
    message: ChatMessage,
    now_ms: i64,
    origin: Option<crate::scheduler::TaskOrigin>,
) -> Result<String, QueueError> {
    if q.len() >= SESSION_QUEUE_MAX {
        return Err(QueueError::Full);
    }
    let id = uuid::Uuid::new_v4().to_string();
    q.push_back(QueuedMessage {
        id: id.clone(),
        message,
        enqueued_at: now_ms,
        // R6 placeholder — see struct doc; MVP scheduling ignores it.
        priority: 0,
        origin,
    });
    Ok(id)
}

/// Drain ALL queued messages for `session_id` in FIFO order.
/// Destructive (mirrors `drain_notifications`). Called by the driver
/// between rounds and by the cancel path (Stop clears the queue).
pub async fn drain_all(
    queues: &Arc<Mutex<HashMap<String, VecDeque<QueuedMessage>>>>,
    session_id: &str,
) -> Vec<QueuedMessage> {
    let mut map = queues.lock().await;
    match map.get_mut(session_id) {
        Some(q) => q.drain(..).collect(),
        None => Vec::new(),
    }
}

/// Clear (discard) every queued message for `session_id`; returns how
/// many were dropped so `cancel_chat` can report "已丢弃 N 条"
/// (PRD R7). Called from the cancel path and destructive commands.
pub async fn clear_session(
    queues: &Arc<Mutex<HashMap<String, VecDeque<QueuedMessage>>>>,
    session_id: &str,
) -> usize {
    let mut map = queues.lock().await;
    match map.remove(session_id) {
        Some(q) => q.len(),
        None => 0,
    }
}

/// Remove one entry by uuid (R8 撤销). Returns the removed message.
pub async fn remove_by_id(
    queues: &Arc<Mutex<HashMap<String, VecDeque<QueuedMessage>>>>,
    session_id: &str,
    id: &str,
) -> Result<QueuedMessage, QueueError> {
    let mut map = queues.lock().await;
    let q = map.get_mut(session_id).ok_or(QueueError::NotFound)?;
    let pos = q
        .iter()
        .position(|m| m.id == id)
        .ok_or(QueueError::NotFound)?;
    Ok(q.remove(pos).expect("position validated above"))
}

/// Snapshot for `list_queued_messages` IPC (R8 hydration SoT).
pub async fn list_session(
    queues: &Arc<Mutex<HashMap<String, VecDeque<QueuedMessage>>>>,
    session_id: &str,
) -> Vec<QueuedMessage> {
    let map = queues.lock().await;
    map.get(session_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{MessageContent, Role};

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            speaker: None,
            attachments: None,
        }
    }

    #[tokio::test]
    async fn push_rejects_beyond_capacity() {
        let mut q = VecDeque::new();
        for i in 0..SESSION_QUEUE_MAX {
            push_with_origin(&mut q, user_msg(&format!("m{i}")), 0, None)
                .expect("under capacity must succeed");
        }
        assert_eq!(q.len(), SESSION_QUEUE_MAX);
        assert_eq!(
            push_with_origin(&mut q, user_msg("overflow"), 0, None),
            Err(QueueError::Full)
        );
        // Rejected entry must NOT have been appended.
        assert_eq!(q.len(), SESSION_QUEUE_MAX);
    }

    #[tokio::test]
    async fn drain_all_is_fifo_and_destructive() {
        let queues: SharedQueues = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut map = queues.lock().await;
            let q = map.entry("s1".into()).or_default();
            push_with_origin(q, user_msg("a"), 1, None).unwrap();
            push_with_origin(q, user_msg("b"), 2, None).unwrap();
            push_with_origin(q, user_msg("c"), 3, None).unwrap();
        }
        let drained = drain_all(&queues, "s1").await;
        let texts: Vec<_> = drained
            .iter()
            .map(|m| match &m.message.content {
                MessageContent::Text(t) => t.clone(),
                _ => panic!("text expected"),
            })
            .collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
        // Destructive: second drain is empty; missing session too.
        assert!(drain_all(&queues, "s1").await.is_empty());
        assert!(drain_all(&queues, "nope").await.is_empty());
    }

    #[tokio::test]
    async fn clear_session_returns_count() {
        let queues: SharedQueues = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut map = queues.lock().await;
            let q = map.entry("s1".into()).or_default();
            push_with_origin(q, user_msg("a"), 1, None).unwrap();
            push_with_origin(q, user_msg("b"), 2, None).unwrap();
        }
        assert_eq!(clear_session(&queues, "s1").await, 2);
        assert_eq!(clear_session(&queues, "s1").await, 0);
        assert_eq!(clear_session(&queues, "nope").await, 0);
    }

    #[tokio::test]
    async fn remove_by_id_targets_uuid_not_position() {
        let queues: SharedQueues = Arc::new(Mutex::new(HashMap::new()));
        let ids: Vec<String> = {
            let mut map = queues.lock().await;
            let q = map.entry("s1".into()).or_default();
            (0..3)
                .map(|i| push_with_origin(q, user_msg(&format!("m{i}")), i, None).unwrap())
                .collect()
        };
        // Remove the MIDDLE entry by uuid.
        let mid = remove_by_id(&queues, "s1", &ids[1]).await.unwrap();
        match &mid.message.content {
            MessageContent::Text(t) => assert_eq!(t, "m1"),
            _ => panic!("text expected"),
        }
        // Remaining order preserved.
        let rest = list_session(&queues, "s1").await;
        let texts: Vec<String> = rest
            .iter()
            .map(|m| match &m.message.content {
                MessageContent::Text(t) => t.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, vec!["m0", "m2"]);
        // Unknown id / unknown session → NotFound.
        assert_eq!(
            remove_by_id(&queues, "s1", &ids[1]).await,
            Err(QueueError::NotFound)
        );
        assert_eq!(
            remove_by_id(&queues, "nope", &ids[0]).await,
            Err(QueueError::NotFound)
        );
    }
}

// ---------------------------------------------------------------------------
// DriverSink — 续轮驱动器的 sink 包装器(design §3)
// ---------------------------------------------------------------------------

/// 驱动器观测到的内层 loop 终态。
#[derive(Default, Debug, Clone)]
pub struct DriverStatus {
    /// 最近一次(被吞掉的)`Done` 的 `(stop_reason, usage)`。每轮
    /// 内层 run 恰好发一次 Done;驱动器只在**真正退出**时把它经
    /// inner sink 补发给前端 —— 中间轮不发,前端 finalize 只认
    /// Done,单 rid 因此跨多轮保活(群聊同款机制)。
    pub last_done: Option<(Option<String>, Option<crate::llm::types::TokenUsage>)>,
    /// 任一 `Error` 事件流经即置位。错误路径驱动器不补发 Done
    /// (Error 已触发前端 finalize,再补会双重终结)。
    pub errored: bool,
}

/// [`ChatEventSink`] 装饰器:转发一切事件,**吞掉 `Done`**(记录到
/// [`DriverStatus`]),`Error` 置位后照转。其余通道原样透传。
///
/// 吞 Done 的理由见 `DriverStatus::last_done`;这是"驱动器只在真
/// 结束 emit 一次 Done"(design §1)的实现点 —— 不改
/// `run_chat_loop` 的发射行为,包装发生在 sink 层。
///
/// **透传面必须覆盖全部非 chat 通道**(tool_call / tool_result /
/// permission_ask / tool_question / mode_change_request /
/// task_state_transition)。后三个是阻塞交互工具
/// (ask_user_question / request_mode_change /
/// request_task_state_transition)的卡片事件——trait 为它们提供
/// **默认静默 no-op**,装饰器漏写转发不会编译报错,只会让前端
/// 永远收不到卡片(2026-08-31 实证:三个方法缺失导致
/// dev workflow 的 ask 卡片 + 状态转换卡片实时不渲染,刷新靠
/// `get_pending_interaction` 拉取才恢复)。新增 trait 通道时,
/// AppHandleSink / HttpSseSink / DriverSink / MockEmitter /
/// RecordingSink 五个实现点必须同步。
pub struct DriverSink {
    inner: Arc<dyn crate::state::ChatEventSink>,
    pub status: Arc<std::sync::Mutex<DriverStatus>>,
}

impl DriverSink {
    pub fn new(
        inner: Arc<dyn crate::state::ChatEventSink>,
    ) -> (Self, Arc<std::sync::Mutex<DriverStatus>>) {
        let status = Arc::new(std::sync::Mutex::new(DriverStatus::default()));
        (
            Self {
                inner,
                status: status.clone(),
            },
            status,
        )
    }

    /// 返回值 = 是否由调用方转发给 inner sink:`false` 吞掉(`Done`),
    /// `true` 透传。**转发动作统一在 [`DriverSink::emit_chat_event`]
    /// 按 return value 执行** —— 各分支不得自行调用
    /// `self.inner.emit_chat_event`(Error 分支曾自转发,若调用方再按
    /// 返回值转发会双发;评审 Round 2 P0 修复)。
    fn forward(&self, payload: &crate::state::ChatEventPayload) -> bool {
        match &payload.event {
            ChatEvent::Done { stop_reason, usage } => {
                let mut st = self.status.lock().expect("driver status lock");
                st.last_done = Some((stop_reason.clone(), *usage));
                false
            }
            ChatEvent::Error { .. } => {
                let mut st = self.status.lock().expect("driver status lock");
                st.errored = true;
                true
            }
            _ => true,
        }
    }
}

impl crate::state::ChatEventSink for DriverSink {
    fn emit_chat_event(&self, payload: &crate::state::ChatEventPayload) {
        // P0 修复(评审 Round 2):转发决定权在 `forward` 的返回值。
        // 此前这里丢弃返回值且 `_` 分支不转发 —— 除 Error 外的
        // 全部常规事件(Start/Delta/TurnComplete/FileInjections/…)
        // 被静默吞掉,生产端流式完全不可见;单测
        // `driver_sink_forwards_regular_events` 锁死该契约。
        if self.forward(payload) {
            self.inner.emit_chat_event(payload);
        }
    }
    fn emit_tool_call(&self, payload: &crate::state::ToolCallPayload) {
        self.inner.emit_tool_call(payload);
    }
    fn emit_tool_result(&self, payload: &crate::state::ToolResultPayload) {
        self.inner.emit_tool_result(payload);
    }
    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        self.inner.emit_permission_ask(payload);
    }
    // 2026-08-31 fix: 三个阻塞交互通道此前缺失 —— trait 默认静默
    // no-op 把 ask_user_question / request_mode_change /
    // request_task_state_transition 的卡片事件吞在包装器里,前端
    // 实时永不渲染、只能刷新靠 get_pending_interaction 拉取兜底。
    fn emit_tool_question(&self, payload: &crate::agent::question_store::ToolQuestionPayload) {
        self.inner.emit_tool_question(payload);
    }
    fn emit_mode_change_request(&self, payload: &crate::agent::question_store::ModeChangePayload) {
        self.inner.emit_mode_change_request(payload);
    }
    fn emit_task_state_transition(
        &self,
        payload: &crate::agent::question_store::TaskStateTransitionPayload,
    ) {
        self.inner.emit_task_state_transition(payload);
    }
}

#[cfg(test)]
mod driver_sink_tests {
    use super::*;
    use crate::agent::permissions::PermissionAskPayload;
    use crate::llm::types::ChatEvent;
    use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};
    use std::sync::{Arc, Mutex};

    /// 最小记录 sink —— 只关心 chat 事件序,不耦合 tests_common 的
    /// harness(那套构造完整 AppState,这里单测包装器本身)。
    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ChatEvent>>,
        questions: Mutex<Vec<crate::agent::question_store::ToolQuestionPayload>>,
        mode_changes: Mutex<Vec<crate::agent::question_store::ModeChangePayload>>,
        state_transitions: Mutex<Vec<crate::agent::question_store::TaskStateTransitionPayload>>,
    }

    impl ChatEventSink for RecordingSink {
        fn emit_chat_event(&self, payload: &ChatEventPayload) {
            self.events.lock().unwrap().push(payload.event.clone());
        }
        fn emit_tool_call(&self, _p: &ToolCallPayload) {}
        fn emit_tool_result(&self, _p: &ToolResultPayload) {}
        fn emit_permission_ask(&self, _p: PermissionAskPayload) {}
        fn emit_tool_question(&self, p: &crate::agent::question_store::ToolQuestionPayload) {
            self.questions.lock().unwrap().push(p.clone());
        }
        fn emit_mode_change_request(&self, p: &crate::agent::question_store::ModeChangePayload) {
            self.mode_changes.lock().unwrap().push(p.clone());
        }
        fn emit_task_state_transition(
            &self,
            p: &crate::agent::question_store::TaskStateTransitionPayload,
        ) {
            self.state_transitions.lock().unwrap().push(p.clone());
        }
    }

    fn emit(sink: &DriverSink, event: ChatEvent) {
        sink.emit_chat_event(&ChatEventPayload {
            request_id: "rid".into(),
            session_id: "sess-test".into(),
            event,
        });
    }

    /// 评审 Round 2 P0 回归锁:常规事件(Start/Delta/TurnComplete 等)
    /// 必须穿透包装器到达 inner sink —— 曾经 `_ => true` 分支不转发、
    /// 调用方又丢弃返回值,除 Error 外全部事件被吞,生产端流式不可见。
    #[test]
    fn driver_sink_forwards_regular_events() {
        let inner = Arc::new(RecordingSink::default());
        let (sink, _status) = DriverSink::new(inner.clone());

        emit(&sink, ChatEvent::TurnContinuation { count: 1 });
        emit(
            &sink,
            ChatEvent::Delta {
                text: "hello".into(),
            },
        );
        emit(
            &sink,
            ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            },
        );
        emit(
            &sink,
            ChatEvent::Delta {
                text: "tail".into(),
            },
        );

        let events = inner.events.lock().unwrap().clone();
        // Start 层事件序:TurnContinuation → Delta → (Done 被吞) → Delta。
        assert_eq!(
            events,
            vec![
                ChatEvent::TurnContinuation { count: 1 },
                ChatEvent::Delta {
                    text: "hello".into()
                },
                ChatEvent::Delta {
                    text: "tail".into()
                },
            ],
            "regular events must pass through in order; Done must be swallowed"
        );
        // 被吞的 Done 记录在 status 里,供驱动器真退出时补发。
        let st = _status.lock().unwrap();
        assert!(matches!(st.last_done.as_ref(), Some((reason, _))
            if reason.as_deref() == Some("end_turn")));
        assert!(!st.errored);
    }

    /// Error 恰好转发一次(不因"分支自转发 + 调用方按返回值转发"双发)
    /// 且置位 errored —— 驱动器据此走"不补发 Done"的错误退出路径。
    #[test]
    fn driver_sink_error_forwarded_exactly_once_and_flags() {
        let inner = Arc::new(RecordingSink::default());
        let (sink, status) = DriverSink::new(inner.clone());

        emit(
            &sink,
            ChatEvent::Error {
                message: "boom".into(),
                category: crate::llm::types::LlmErrorCategory::Server,
            },
        );

        let events = inner.events.lock().unwrap().clone();
        let error_count = events
            .iter()
            .filter(|e| matches!(e, ChatEvent::Error { .. }))
            .count();
        assert_eq!(error_count, 1, "Error must be forwarded exactly once");
        assert!(status.lock().unwrap().errored);
    }

    /// 2026-08-31 回归锁:三个阻塞交互通道(tool_question /
    /// mode_change_request / task_state_transition)必须穿透包装器。
    /// 它们在 trait 上是默认静默 no-op,装饰器漏写**不会编译报错**
    /// ——此前三个方法全部缺失,dev workflow 的 ask 卡片 +
    /// 状态转换卡片实时不渲染(仅刷新拉取可恢复)。新增
    /// `ChatEventSink` 通道时在本测试同步补一条断言。
    #[test]
    fn driver_sink_forwards_blocking_interaction_channels() {
        use crate::agent::question_store::{
            ModeChangePayload, TaskStateTransitionPayload, ToolQuestionPayload,
        };

        let inner = Arc::new(RecordingSink::default());
        let (sink, _status) = DriverSink::new(inner.clone());

        sink.emit_tool_question(&ToolQuestionPayload {
            session_id: "sess-test".into(),
            tool_use_id: "tu_q".into(),
            questions: vec![],
            ts: 0,
        });
        sink.emit_mode_change_request(&ModeChangePayload {
            session_id: "sess-test".into(),
            tool_use_id: "tu_m".into(),
            target_mode: "yolo".into(),
            current_mode: Some("edit".into()),
            reason: None,
            ts: 0,
        });
        sink.emit_task_state_transition(&TaskStateTransitionPayload {
            session_id: "sess-test".into(),
            tool_use_id: "tu_t".into(),
            target_state: "in_progress".into(),
            current_state: Some("planning".into()),
            slug: Some("08-31-docs-sync-batch".into()),
            reason: None,
            ts: 0,
        });

        assert_eq!(
            inner.questions.lock().unwrap().len(),
            1,
            "tool:question must pass through the DriverSink"
        );
        assert_eq!(
            inner.mode_changes.lock().unwrap().len(),
            1,
            "mode:change:request must pass through the DriverSink"
        );
        assert_eq!(
            inner.state_transitions.lock().unwrap().len(),
            1,
            "task:state:transition:request must pass through the DriverSink"
        );
    }
}
