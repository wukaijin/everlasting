# Design: `request_mode_change` tool

> 配套 PRD: [prd.md](./prd.md)。本文聚焦:技术架构、数据流、关键 contract、Cross-layer 切面、风险 / 权衡、测试策略。

## 1. 架构概览

沿用 `ask_user_question` 的**双轨 dispatch 范式** —— chat_loop 拦截 → execute_blocking → 复用 QuestionStore → 前端 inline card → 用户决策 → 工具回灌。**唯一不同**:`execute_blocking` 的成功路径**额外**调 `db::update_session_mode` 落库(等价 `set_session_mode` IPC 的副作用,但不走 IPC 回路)。

```
              LLM tool_use (request_mode_change, target_mode, reason)
                          │
                          ▼
         ┌────────────────────────────────────┐
         │ chat_loop.rs::run_chat_loop         │  ← 拦截 line 3110 风格
         │ (新增 name == "request_mode_change" │     (与 ask_user_question 并列)
         │  分支)                              │
         └────────────────────────────────────┘
                          │
                          ▼
         ┌────────────────────────────────────┐
         │ tools/request_mode_change.rs       │
         │ ::execute_blocking(input,           │
         │   session_id, tool_use_id,         │
         │   store, sink, cancel)             │
         │                                    │
         │ 1. parse + schema validate         │
         │ 2. noop? target == current → return│
         │ 3. record_audit("mode_change_req") │
         │ 4. store.register(payload)         │  ← 共用 QuestionStore
         │ 5. emit("mode:change:request")     │  ← 新 IPC event
         │ 6. tokio::select!{cancel|oneshot}  │
         │ 7a. allow → db::update_session_mode│  ← 直接落库
         │      + record_audit("mode_changed")│
         │      + record_audit("allowed")     │
         │ 7b. deny → record_audit("denied")  │
         │ 7c. cancel → store.remove          │
         │ 8. return tool_result              │
         └────────────────────────────────────┘
                          │
                          ▼
         ┌────────────────────────────────────┐
         │ ChatEventSink::emit_mode_change_   │
         │ request(&ModeChangePayload)        │  ← 新 trait method
         └────────────────────────────────────┘
                          │
                          ▼ Tauri IPC
         ┌────────────────────────────────────┐
         │ Frontend:                           │
         │  - useQuestionCardsStore.pending   │
         │    (扩展为 PendingInteraction       │
         │     kind = question|mode_change)    │
         │  - <RequestModeChangeCard>          │  ← 新组件
         │  - 允许 → resolve_interaction IPC  │
         │  - 拒绝 → resolve_interaction IPC  │
         │  - 允许 Yolo → 弹 pendingYoloConfirm│
         │    modal → 二次确认 → 落库          │
         └────────────────────────────────────┘
```

## 2. 数据流(5 个时序)

### 2.1 Happy path:LLM 申请 edit,user 允许

```
1. LLM response 包含:
   { type: "tool_use", id: "tu_X", name: "request_mode_change",
     input: { target_mode: "edit", reason: "需要落代码" } }
2. chat_loop 拦截 → execute_blocking
3. validate pass, target_mode != current_mode(假设是 plan)
4. record_audit("mode_change_requested", {target_mode:"edit", reason:"需要落代码"})
5. store.register("s1", "tu_X", { kind: ModeChange, payload })
6. sink.emit_mode_change_request(&payload) → IPC "mode:change:request"
7. 前端 <RequestModeChangeCard> 渲染
8. user 点"允许" → invoke("resolve_mode_change", { allow: true })
9. 后端 store.resolve("s1", Allow)
10. execute_blocking 拿到 Allow:
    - db::update_session_mode(pool, "s1", Edit)
    - record_audit("mode_changed", {prev:"plan", new:"edit"})
    - record_audit("mode_change_allowed", {prev:"plan", new:"edit", target:"edit"})
11. tool_result = {"allowed": true, "prev_mode": "plan", "new_mode": "edit"} (is_error: false)
12. chat_loop 推 ContentBlock::ToolResult, LLM 下一轮 system prompt 已是 Edit
```

### 2.2 Yolo 二段守门:LLM 申请 yolo

```
1-7. 同上(target_mode = "yolo")
8. user 点 card "允许" → 不直接调 resolve,而是 dispatch 到 useChatStore.requestSetMode("s1", "yolo")
9. requestSetMode 走 user 主动切 Yolo 路径:设 pendingYoloConfirm = true
10. 弹 Yolo 二次 modal(显示 "切到 Yolo 将跳过所有用户确认")
11. user 在 modal 上点"确认" → confirmYolo() 调 set_session_mode IPC
12. user 在 modal 上点"取消" → cancelYolo() → 走"拒绝"路径
    - store.resolve("s1", Deny)
    - record_audit("mode_change_denied", {target:"yolo"})
    - tool_result = {"cancelled_by_user": true, "reason": "user cancelled Yolo confirm"} (is_error: true)
13. 若 confirmYolo 成功:
    - set_session_mode IPC 落 DB + 审计 (prev:xxx, new:yolo) ← 沿用现有路径
    - 后端 IPC handler 检测:这是从 card dispatch 来的,额外 record_audit("mode_change_allowed")
    - 调 store.resolve("s1", Allow) ← 让 agent loop 的 oneshot 解除
    - execute_blocking 拿到 Allow,tool_result = {"allowed": true, ...}
```

**关键点**:第 12 步到 confirmYolo 走的是**两段 IPC**(frontend store action → set_session_mode IPC → 后端 handler → store resolve)。`store.resolve` 必须在 `db::update_session_mode` **之后**调用,否则 agent loop 会先收到 Allow,但 DB 还没落库,出现不一致。

### 2.3 Noop 路径:LLM 申请切到当前 mode

```
1. LLM tool_use: { target_mode: "plan" } (假设当前也是 plan)
2. chat_loop 拦截 → execute_blocking
3. validate pass
4. 读 current_mode (从 store / session),对比 == target
5. record_audit("mode_change_requested", {target_mode, noop: true})  // 留痕
6. 不注册 store,不发 IPC
7. 立即返回 tool_result = {"noop": true, "current_mode": "plan"} (is_error: false)
8. LLM 下一轮看到 noop,自行决定
```

### 2.4 拒绝路径

```
1-7. 同 happy path
8. user 点"拒绝" → invoke("resolve_mode_change", { allow: false })
9. 后端 store.resolve("s1", Deny)
10. record_audit("mode_change_denied", {target_mode})
11. tool_result = {"cancelled_by_user": true} (is_error: true)
12. LLM 看到,自决
```

### 2.5 Session cancel 路径

```
1-7. 同 happy path
X. session cancel (user Stop / app shutdown)
8. execute_blocking 的 cancel arm 触发
9. store.remove("s1")
10. tool_result = {"cancelled_by_session": true} (is_error: true)
11. chat_loop 推 cancelled = true,整轮终止
```

## 3. 关键 contract

### 3.1 Tool schema(LLM 入参)

```rust
// app/src-tauri/src/tools/request_mode_change.rs
pub fn definition() -> ToolDef {
    ToolDef {
        name: "request_mode_change".to_string(),
        description: Some(
            "Ask the user to switch this session's permission mode (edit / plan / \
             yolo). The user sees an inline card with the target mode and the reason \
             you provide; they choose Allow or Deny. On Allow, the mode is updated \
             and the system prompt reflects the new mode on the next turn. On Deny, \
             you get `{\"cancelled_by_user\": true}` and should adapt. If the target \
             mode is the current mode, the call returns `{\"noop\": true}` and no \
             card is shown.\n\n\
             Use this when you've completed a read-only plan and need write access \
             (request edit), or when you want to propose a change before acting on \
             it (request plan), or when the user has explicitly approved running \
             unattended (request yolo). Do not request yolo unless the task is \
             fully understood and irreversible actions are acceptable."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "target_mode": {
                    "type": "string",
                    "enum": ["edit", "plan", "yolo"],
                    "description": "The mode to request. Must be one of: edit, plan, yolo."
                },
                "reason": {
                    "type": "string",
                    "maxLength": 500,
                    "description": "Optional ≤500-char explanation shown on the card. Be specific about why this mode is needed for the next step."
                }
            },
            "required": ["target_mode"]
        }),
    }
}
```

### 3.2 Wire payload(`mode:change:request` IPC)

```rust
// app/src-tauri/src/agent/question_store.rs (扩展)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangePayload {
    pub session_id: String,
    pub tool_use_id: String,
    pub target_mode: String,           // "edit" | "plan" | "yolo"
    pub current_mode: String,          // session 当前 mode (LLM 自检用)
    pub reason: Option<String>,        // LLM 给的 reason
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingInteraction {
    Question(ToolQuestionPayload),
    ModeChange(ModeChangePayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInteractionEntry {
    pub kind: InteractionKind,         // "question" | "mode_change"
    pub payload: PendingInteraction,   // tagged enum
}
```

### 3.3 store 扩展(PendingInteraction 互斥)

```rust
// 扩展 QuestionStore::register 签名
pub async fn register(
    &self,
    session_id: &str,
    tool_use_id: &str,
    payload: PendingInteraction,       // ← 替换原 ToolQuestionPayload
) -> Result<oneshot::Receiver<InteractionResponse>, QuestionStoreError>;

// oneshot 通道的 response 类型扩展
pub enum InteractionResponse {
    Answered(serde_json::Value),       // 通用 answer(Question 用 Vec<QuestionAnswer>,ModeChange 用 true)
    Cancelled,                          // 用户拒绝 / 跳过
}
```

**互斥语义保持**:`map.contains_key(session_id)` → `AlreadyPending`(同 session 不能并发两个待决交互,无论 kind)。

### 3.4 后端 tool entry 签名

```rust
// app/src-tauri/src/tools/request_mode_change.rs
pub type BlockingToolResult = (
    String,                            // content
    bool,                              // is_error
    crate::tools::ToolContextUpdate,
    Option<i32>,                       // exit_code
);

pub async fn execute_blocking(
    input: &serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
    current_mode: db::Mode,            // ← 新增:execute_blocking 需要知道当前 mode 做 noop 判断
    pool: &sqlx::SqlitePool,           // ← 新增:execute_blocking 直接落库(不走 IPC)
    store: &QuestionStore,
    sink: &Arc<dyn ChatEventSink>,
    cancel: &CancellationToken,
) -> BlockingToolResult;
```

### 3.5 IPC surface

```rust
// 新增
#[tauri::command]
pub async fn resolve_mode_change(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    allow: bool,
) -> Result<(), AppCommandError>;

// 升级(原 get_pending_question → get_pending_interaction)
#[tauri::command]
pub async fn get_pending_interaction(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<PendingInteractionEntry>, AppCommandError>;

// 保留(向后兼容,继续路由到 QuestionStore)
#[tauri::command]
pub async fn resolve_tool_question(...);  // 不变
```

### 3.6 ChatEventSink 扩展

```rust
// app/src-tauri/src/state.rs
pub trait ChatEventSink: Send + Sync {
    // ... 现有方法 ...
    fn emit_mode_change_request(&self, payload: &ModeChangePayload);
}
```

**实现方**:
- `AppHandleSink`: `app.emit("mode:change:request", payload)`
- `MockEmitter`(tests):push 进 `Vec<ModeChangePayload>`
- `SubagentBufferSink`:no-op(worker 禁用,见 PRD R10)

### 3.7 审计 contract

```rust
// 新增 3 类 kind
pub enum AuditKind {
    // ... 现有 17 类 ...
    ModeChangeRequested,
    ModeChangeAllowed,
    ModeChangeDenied,
}

// payload
// mode_change_requested: { target_mode, reason, noop: bool }
// mode_change_allowed:   { prev_mode, new_mode, target_mode }
// mode_change_denied:    { target_mode, reason: "user denied" | "yolo_root_guard" | "yolo_cancelled_confirm" }
```

`mode_changed` audit 由 `db::update_session_mode` 自动产生(沿用 `set_session_mode` 路径),**不重复**。

### 3.8 Frontend store 扩展

```ts
// app/src/stores/questionCards.ts (扩展)
type PendingInteraction =
  | { kind: "question"; payload: ToolQuestionPayload }
  | { kind: "mode_change"; payload: ModeChangePayload };

const pendingBySession = ref<Map<string, PendingInteraction>>(new Map());
const currentModeBySession = ref<Map<string, SessionMode>>(new Map()); // ← 新增:request_mode_change 需要知道当前 mode 做 UI 反馈

// 新增 action
async function resolveModeChange(
  sessionId: string,
  toolUseId: string,
  allow: boolean,
): Promise<void>;

// 新增 IPC binding
async function getPendingInteraction(sessionId: string): Promise<PendingInteraction | null>;
```

### 3.9 Frontend card 组件

```vue
<!-- app/src/components/chat/RequestModeChangeCard.vue -->
<script setup lang="ts">
// props: { sessionId, toolUseId, targetMode, currentMode, reason?, state?, allowedMode? }
// emits: { (e: "allowed", newMode: SessionMode), (e: "denied") }
// 内部:不允许内嵌,inline 在 message 流;Yolo 二次 modal 走 useChatStore.requestSetMode
</script>
```

**视觉**:
- Header chip:目标 mode 名字 + 状态色(plan = 蓝, edit = 灰, yolo = 红)
- Reason 文本(可选,wrap,无 markdown 渲染,纯文本)
- Bottom action row(pending):"允许" + "拒绝" 两按钮(等宽,允许在左;允许按钮颜色 = 目标 mode 颜色)
- Answered 状态:`已切换到 [mode]` pill + 切换前后对比
- Cancelled 状态:`已拒绝` pill
- Noop 状态(若 user 看到 reload 后无 card):由后端不弹 card 处理(不渲染),UI 透明

## 4. 关键代码改动点(具体文件 + 区域)

### 4.1 后端新增

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/tools/request_mode_change.rs` | **新建**:`definition()` + `execute_blocking()` + `validate()` + tests(对齐 ask_user_question 结构) |
| `app/src-tauri/src/tools/mod.rs::builtin_tools()` | 在 `ask_user_question` 旁加 `request_mode_change::definition()` |
| `app/src-tauri/src/agent/chat_loop.rs` | `name == "request_mode_change"` 分支(line 3110 区域,在 ask_user_question 分支**下**新增);额外传 `current_mode` + `pool` 参数 |
| `app/src-tauri/src/agent/question_store.rs` | `register` 签名扩展(接受 `PendingInteraction`);`PendingInteraction` enum;`get_payload` 改返回 `Option<PendingInteractionEntry>`;oneshot 类型统一为 `InteractionResponse` |
| `app/src-tauri/src/state.rs::ChatEventSink` | 加 `emit_mode_change_request` 方法 |
| `app/src-tauri/src/state.rs::AppHandleSink` / `MockEmitter` / `SubagentBufferSink` | 三处实现新方法 |
| `app/src-tauri/src/db/audit.rs` | `AuditKind` 加 3 个 variant;`record_audit_event` 已支持任意 string kind,不改 |
| `app/src-tauri/src/commands/permissions.rs` | 加 `resolve_mode_change` IPC handler(调 `state.question_store.resolve` + 必要时调 `db::update_session_mode` —— **Yolo 二次 modal 确认后从 set_session_mode 路径走,tool 自身不分叉**)|

### 4.2 后端扩展(原 ask_user_question 兼容)

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/agent/question_store.rs` | `PendingQuestion` 字段保留,新增 `PendingModeChange` 结构(oneshot + payload);`register` 接口扩展;oneshot 通道类型 `QuestionResponse` → `InteractionResponse`(enum)|
| `app/src-tauri/src/commands/question.rs` | 加 `get_pending_interaction`(替代 `get_pending_question`,保留旧 IPC 软弃用);`resolve_tool_question` 不变 |

### 4.3 前端新增

| 文件 | 改动 |
|---|---|
| `app/src/components/chat/RequestModeChangeCard.vue` | **新建**:inline card,行为对齐 AskUserQuestionCard |
| `app/src/components/chat/MessageItem.vue` | `tool_name` 分发:`request_mode_change` → `<RequestModeChangeCard>`,其余不变 |
| `app/src/components/chat/MessageList.vue` | 同上分发(若 MessageItem 单独挂载,这里可能不需改)|
| `app/src/stores/questionCards.ts` | 扩展为 `pendingBySession: PendingInteraction`;新增 `resolveModeChange` action;`getPendingInteraction` IPC 绑定 |
| `app/src/stores/questionCards.types.ts` | 新增 `ModeChangePayload` interface |
| `app/src/utils/toolQuestion.ts`(对照) | 新增 `resolveModeChange` IPC binding |

### 4.4 Worker 禁用

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/agent/subagent/filter.rs` | `STRUCTURALLY_DISABLED` 列表加 `"request_mode_change"` |

### 4.5 回归点(必须测试覆盖)

| 文件 | 测试 |
|---|---|
| `app/src-tauri/src/tools/ask_user_question.rs` 既有 tests | 全绿 |
| `app/src-tauri/src/agent/permissions.rs::set_session_mode` 既有 tests | 全绿 |
| `app/src-tauri/src/agent/chat_loop.rs::is_parallel_eligible` 既有 tests | 全绿 + 加 `request_mode_change` 不在白名单 |
| `app/src-tauri/src/agent/subagent/filter.rs` 既有 tests | 全绿 + 加 worker 不可见 |
| `app/src/components/chat/AskUserQuestionCard.test.ts` | 全绿 |
| `app/src/stores/chatMode.test.ts` | 全绿(不动 useChatStore.requestSetMode 行为) |

## 5. Cross-layer 切面(必查)

### 5.1 Tool 注册 vs execute 路径

- `builtin_tools()` 加 `request_mode_change::definition()`(LLM schema 暴露)
- `execute_tool_inner` 的 `match` **不加**新分支(对齐 ask_user_question 模式)
- `chat_loop` 加 `if name == "request_mode_change" { ... }` 拦截,调 `execute_blocking`
- `is_parallel_eligible` **不加**新分支(白名单机制自动走串行)

### 5.2 QuestionStore 升级风险

- `register` 签名从 `(session_id, tool_use_id, ToolQuestionPayload)` 改为 `(session_id, tool_use_id, PendingInteraction)`,**破坏性变更**
- 缓解:`PendingInteraction::Question(ToolQuestionPayload)` 把旧 payload 装进 variant,call site 改写为 `PendingInteraction::Question(parsed)` —— **改 1 处,影响 ~2 个 caller**(`ask_user_question::execute_blocking` + 新 `request_mode_change::execute_blocking`)
- `get_payload` 改返回 `Option<PendingInteractionEntry>`,前端 `get_pending_question` IPC 软弃用,新 IPC `get_pending_interaction` 提供统一查询

### 5.3 Tool 内部调 db 的边界

- `execute_blocking` 多 2 个参数:`pool: &SqlitePool` + `current_mode: db::Mode`
- 调用点 `chat_loop` 已有 `db: SqlitePool` 和 `session_mode`(line 600),传参零成本
- 副作用(落库 + 审计)放 tool 内部,绕过 IPC,可靠性更高;但失去了"前端 refresh 触发 service"的可能性(本工具无此需要)
- **风险**:tool 内部调 `db::update_session_mode` 不走 IPC,前端 `useChatStore` 的 `currentSession.mode` 不会自动刷新 → **必须 emit `chat-event` 通知前端刷 mode**;详见 §5.4

### 5.4 前端 mode 状态同步

- 现有 `useChatStore.currentSession.mode` 是 LLM 切换后 user 看到的"权威"字段
- tool 落库后,前端不知道 mode 变了 → 必须 emit 一个事件让前端拉新
- **方案 A(简单)**:后端在 `record_audit` 后 emit `chat-event` 携带新 mode 字段,前端 listener 解析后调 `loadSession(sid)` 刷新
- **方案 B(更直接)**:`resolve_mode_change` IPC 返回新的 `SessionRow`(同 `set_session_mode` 返回),前端 invoke 拿回 row 直接更新 `currentSession`
- **决策 = B**:`resolve_mode_change` IPC 返回 `Result<SessionRow, AppCommandError>`,前端 invoke 后直接 setCurrentSession(row)。简单一致,沿用 `set_session_mode` 的 IPC 形态。
- Yolo 二段路径:`set_session_mode` IPC 自身已返回 `SessionRow`,前端 `confirmYolo` action 拿 row 后,**额外**调 `resolve_mode_change(sid, toolUseId, allow=true)` 走 store resolve 通路(此时 store 内的 tool_use_id 是前端从 card 拿的)。**两个 IPC 都返回 SessionRow,前端合并去重**。

### 5.5 Yolo 二次守门的一致性

- User 主动切 Yolo: `useChatStore.requestSetMode("yolo")` → `pendingYoloConfirm = true` → modal → `confirmYolo` → `set_session_mode` IPC
- LLM 申请 Yolo: card "允许" → `useChatStore.requestSetMode("s1", "yolo")` → `pendingYoloConfirm = true` → modal → `confirmYolo` → `set_session_mode` IPC → `resolve_mode_change` IPC
- **共同点**:`set_session_mode` IPC 是**唯一**落库入口(user 主动 / LLM 申请两路径都走);tool 内部**不**调 `db::update_session_mode`,**仅**在 store 等待 IPC handler 完成后调 `store.resolve(sid, Allow)`。
- **重大变化(对比 PRD R4 初稿)**:tool 内部不直接落库,改走"前端触发 IPC → IPC handler 落库 + resolve store"。原因:Yolo root guard / Yolo 二次 modal / audit 完整性都在 `set_session_mode` 路径,**复用即正确**;tool 单独落库会导致双路径漂移。**详见 §6 决策变更**。

## 6. 关键决策变更(对比 PRD R4 初稿)

> 迭代中识别到的设计变更,需要在 implement.md 体现。

| 维度 | PRD R4 初稿 | Design 最终版 | 理由 |
|---|---|---|---|
| **mode 应用副作用** | tool 内部直接调 `db::update_session_mode` 落库 | tool **不落库**,仅 `store.resolve`;`set_session_mode` IPC 仍为唯一落库入口 | Yolo root guard / Yolo 二次 modal / 审计完整性都集中在 `set_session_mode`;**复用 IPC 路径,避免双路径漂移**;前端 invoke `resolve_mode_change(allow=true)` 时,后端 handler 检测 `target_mode == yolo` → 转发到 `set_session_mode` IPC 内部函数(不开 IPC,直接调 `commands/permissions.rs::set_session_mode_internal` 等价的纯函数) → 落库后调 `store.resolve(Allow)` |
| **IPC 命名** | 新增 `resolve_mode_change` | 同 | 不变 |
| **前端调用顺序** | 允许 Yolo → card "允许" → `requestSetMode` → modal → `confirmYolo` → `set_session_mode` IPC | 同 | 不变;**额外**在 `confirmYolo` 成功后调 `resolve_mode_change(sid, tuid, allow=true)`,让 store oneshot 解除 |
| **tool 参数** | `pool`, `current_mode` | 移除 `pool`(tool 不落库);保留 `current_mode`(noop 判断) | 落库走 IPC,tool 不需要 pool |
| **noop 路径** | tool 内部 noop,无 IPC | 同 | 不变;`record_audit("mode_change_requested", {noop:true})` 留痕 |
| **审计 kind** | `mode_change_requested` / `allowed` / `denied` | 同 + 复用 `mode_changed`(`db::update_session_mode` 自动) | 不变 |

## 7. 风险 / 权衡

### 7.1 QuestionStore 签名升级的 blast radius

- 现状:`register` 仅 1 个 caller(`ask_user_question::execute_blocking`),签名 4 参
- 升级:签名 4 参 → 4 参但中间参数类型变(`ToolQuestionPayload` → `PendingInteraction`),1 个 caller 需改写
- 风险:低;1 个 caller 文件,1 次 sed 替换
- 测试:`ask_user_question` 既有 12+ tests 全绿 + 新 `request_mode_change` tests 6+ 即可覆盖

### 7.2 双 IPC 调用的时序(确认 Yolo 路径)

- 路径:`set_session_mode` IPC(落库) → `resolve_mode_change` IPC(让 store oneshot 解除)
- **race**:若 `set_session_mode` 失败(DB 错误 / root guard 触发),`resolve_mode_change` 是否还该调?
  - 答:**是**。`resolve_mode_change` 是工具侧契约,必须解 oneshot;失败信息通过 audit(`mode_change_denied` + `reason: "yolo_root_guard"`) + tool_result(`cancelled_by_user: true, reason`)告知 LLM。
  - 实现:`resolve_mode_change` handler 先调内部 `set_session_mode_inner`,返回 `Result<SessionRow, AppCommandError>` → 不论成败都 `store.resolve(Allow if ok else Deny)` → audit 写 `allowed` 或 `denied` → IPC 返回 `Result<SessionRow>`。

### 7.3 noop 路径的 current_mode 准确性

- `current_mode` 在 `chat_loop::run_chat_loop` turn 0 读取,后续 LLM 调用沿用
- LLM 在同一 turn 内申请切到 X,noop 判断时 `current_mode` 是 turn 0 快照
- **race**:用户在同一 LLM 响应前手动改了 mode(罕见),`current_mode` 已陈旧 → noop 误判
- 缓解:noop 误判最多产生一次"误以为切了但没切",下个 turn user 调 mode 后 LLM 看到正确;可接受
- 改进(可选 v2):每次 `execute_blocking` 实时 `load_session(...).mode` 做 noop 判断,避免陈旧

### 7.4 IPC 协议稳定性

- `get_pending_interaction` 是新 IPC,旧 `get_pending_question` 标记 deprecated 但不删
- 决策:不删旧 IPC,保留 1 个版本;前端新代码用新 IPC,旧代码路径保留兼容
- 影响:`get_pending_question` 软弃用,**下个版本(如有)再删**

## 8. 测试策略

### 8.1 后端单元测试(对齐 ask_user_question 范式)

| 测试 | 文件 |
|---|---|
| `validation_empty_target_mode_short_circuits` | `tools/request_mode_change.rs` |
| `validation_target_mode_out_of_enum_short_circuits` | 同 |
| `validation_reason_too_long_short_circuits` | 同 |
| `noop_target_equals_current_returns_noop_marker` | 同 |
| `happy_path_registers_emits_and_returns_allowed` | 同 |
| `deny_path_returns_cancelled_user_marker` | 同 |
| `cancel_arm_returns_session_cancelled_marker` | 同 |
| `already_pending_returns_structured_error` | 同(question store 互斥) |
| `audit_kind_records_requested_allowed_denied` | 同(用 CapturingSink 抓取 audit)|
| `validate_accepts_well_formed_input` | 同(纯函数) |
| `validate_rejects_*` 系列 | 同 |

### 8.2 后端集成测试

| 测试 | 文件 |
|---|---|
| Mock provider 模拟 LLM 一次响应含 `request_mode_change` + `shell` → 整批走 serial + block | `agent/tests_*.rs` |
| `is_parallel_eligible` 含 `request_mode_change` 返回 false | `agent/tests_parallel.rs` |
| `filter_tools_for_subagent` 输出不含 `request_mode_change` | `agent/subagent/tests_filter.rs` |
| Yolo root guard:以 root 跑测试,user 允许 → tool_result 走 deny 路径 | `tools/request_mode_change.rs` |
| `resolve_mode_change` IPC handler:allow / deny 路径 + audit | `commands/permissions.rs` |

### 8.3 前端 vitest

| 测试 | 文件 |
|---|---|
| `RequestModeChangeCard` 渲染 pending / allowed / cancelled 3 态 | `components/chat/RequestModeChangeCard.test.ts` |
| `useQuestionCardsStore.resolveModeChange` 调 IPC + 更新 `pendingBySession` | `stores/questionCards.test.ts` |
| `getPendingInteraction` IPC 绑定 | 同 |
| `MessageItem` tool_name 分发:`request_mode_change` → `<RequestModeChangeCard>` | `components/chat/MessageItem.test.ts` |
| `useChatStore.requestSetMode("yolo")` 二次 modal 路径(已有 chatMode.test.ts 覆盖) | 不变 |

### 8.4 端到端验证(manual)

- 启动 Tauri dev → 在 plan 模式让 LLM 申请 edit → 看 card → 允许 → ModeSelect chip 切到 edit → 后续 LLM 能写代码
- LLM 申请切到当前 mode(noop)→ tool_result 立即返回,无 card
- LLM 申请 yolo → card → 允许 → 二次 modal → 确认 → 落库
- session 切换保留 pending:session A 有 pending → 切 B → 切回 A → card 仍在
- worker 不可见:dispatch subagent,worker 的 tool list 不含 `request_mode_change`

## 9. Rollout / Rollback

### Rollout

- 单 PR,4 个 commit(后端 / IPC + audit / 前端 / 集成测试)或 1 个 squash(取决于 review)
- 顺序:后端新工具 + store 扩展 → IPC handlers → 前端 card + store → 集成测试 → 文档同步

### Rollback

- 前端:revert 前端 commit → `RequestModeChangeCard` 不渲染,`MessageItem` 分发失效 → LLM 调 `request_mode_change` 时,`chat_loop` 走默认 `execute_tool` 路径(若 dispatch table 没加 → `is_error: true` unknown tool)→ LLM 自决
- 后端:revert 后端 commit → builtin_tools() 移除 `request_mode_change` → LLM 看不到 schema → 无调用发生
- 数据:无 schema 迁移(沿用现有 `sessions.mode` 列 + `session_audit_events` 表),回滚无副作用
- 风险:**没有**用户数据迁移,纯加性变更,回滚**完全无副作用**

## 10. 文档同步清单

| 文档 | 改动 |
|---|---|
| `.trellis/spec/backend/tool-contract.md` | 加 `request_mode_change` 工具 schema section |
| `.trellis/spec/backend/permission-layer.md` | 加 "Scenario: request_mode_change 写操作" section |
| `.trellis/spec/backend/agent-loop-architecture.md` | chat_loop 拦截点列表加 1 行 |
| `.trellis/spec/frontend/chat.md` | inline card 红线重申;`RequestModeChangeCard` 红线 |
| `docs/IMPLEMENTATION.md §4` | ADR 条目:`request_mode_change` tool(Yolo 二段守门 + 共用 QuestionStore + 双 IPC 时序)|
| `docs/ROADMAP.md` | V2 路线图第二档补 1 行 |

## 11. 实施依赖(为 implement.md 准备)

- 步骤 1:扩展 `QuestionStore`(enum + register 签名 + oneshot 类型)→ **必须**先于其他步骤
- 步骤 2:新建 `tools/request_mode_change.rs`(definition + execute_blocking + tests)
- 步骤 3:`builtin_tools()` 注册
- 步骤 4:`chat_loop` 拦截分支
- 步骤 5:`ChatEventSink` 扩展 + 三处实现
- 步骤 6:新 IPC `resolve_mode_change` + `get_pending_interaction`
- 步骤 7:audit kind 扩展
- 步骤 8:Worker filter 禁用
- 步骤 9:前端 `RequestModeChangeCard` + store 扩展 + IPC binding
- 步骤 10:`MessageItem` 分发
- 步骤 11:集成测试 + manual verification
- 步骤 12:文档同步
