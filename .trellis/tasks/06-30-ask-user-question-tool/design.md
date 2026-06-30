# Design: AskUserQuestion-style agent 阻塞反问 tool

## 1. Overview

实现一个 agent 可调用的阻塞反问 tool `ask_user_question`。Agent loop 在 tool_use 阶段识别该 tool 后,**挂起当前 turn**,发 IPC 事件,等待用户在 inline message card 上点选 / 取消,收到回答后格式化为 `tool_result` 回灌 LLM,继续推理。

关键架构特征(已经 brainstorm 锁定):
- **Inline message card**,非 modal(同 `ToolCallCard` 一层)
- **Session 挂起保留**——切 session 不释放 oneshot,切回可继续答
- **新 IPC channel** `tool:question` + `tool:question_resolved`,独立于 `permission:ask`
- **Worker 结构性禁用**
- **Schema 照搬 Claude Code**(snake_case 命名)
- **单 pending 互斥**
- **无 timeout / 无 auto-decide**(v1 简化)
- **无 DB 专用表**(只用 `messages` 表)

## 2. Architecture

### 2.1 模块边界

```
┌──────────────────────────────────────────────────────────────────┐
│  Frontend (Vue 3 + Pinia + reka-ui)                              │
│                                                                  │
│  app/src/components/chat/                                        │
│    ├─ AskUserQuestionCard.vue        (新)                        │
│    ├─ MessageItem.vue / MessageList.vue  (改) tool name 分发     │
│    └─ ToolCallCard.vue               (不改,作为默认分发目标)     │
│  app/src/stores/                                                  │
│    ├─ questionCards.ts                (新) pendingBySession map   │
│    └─ streamController.ts             (改) session 切换时 invoke │
│                                          get_pending_question    │
│  app/src/types/                                                   │
│    └─ toolQuestion.ts                 (新) IPC payload types     │
│                                                                  │
│  ↕  Tauri IPC                                                    │
│    emit("tool:question", payload)                                 │
│    invoke("tool:question_resolved", { session_id, answer|cancel })│
│    invoke("get_pending_question", { session_id }) → Option<...>  │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────────┐
│  Backend (Rust + Tauri 2 + tokio)                                │
│                                                                  │
│  app/src-tauri/src/                                              │
│    ├─ tools/                                                      │
│    │   ├─ ask_user_question.rs          (新) tool 定义 + 阻塞执行 │
│    │   └─ mod.rs                        (改) dispatch 分支        │
│    ├─ agent/                                                      │
│    │   ├─ question_store.rs             (新) Arc<Mutex<HashMap>>  │
│    │   ├─ chat_loop.rs                  (改) blocking tool 挂起点 │
│    │   └─ permissions/                  (改) STRUCTURALLY_DISABLED│
│    │       └─ mode.rs                                          │
│    ├─ state.rs                          (改) ChatEventSink trait  │
│    │                                              + question_store │
│    └─ commands/                                                   │
│        ├─ question.rs                   (新) resolve + get       │
│        └─ (注册到 lib.rs generate_handler!)                       │
└──────────────────────────────────────────────────────────────────┘
```

### 2.2 不依赖 `permission:ask` 系统

`PermissionStore` / `PermissionAskPayload` / `PermissionAskBody` 完全不碰。QuestionStore 是平行的独立 store,虽然形态相似(`Arc<Mutex<HashMap<session_id, oneshot>>>`),但语义不同:
- PermissionStore = security gate,on session switch **cancel all pending asks**(R14 沿用)
- QuestionStore = UX,on session switch **保留 pending**(本设计 R9–R11)

代码层面共享的是 `tokio::select!{cancel, oneshot.recv()}` 的等待 pattern;但 store / IPC / UI 组件完全独立。

## 3. Data Flow

### 3.1 正常路径(挂起 → 回答 → 续推)

```
LLM response block: ToolUse(ask_user_question, { questions: [...] })
        │
        ▼
[Agent loop] chat_loop.rs
        │ 识别 tool name == "ask_user_question"
        │ 不走 execute_tool()(避免双路径冗余)
        │ 调用 ask_user_question::execute_blocking(...)
        │   (内部先 schema 校验,失败 short-circuit 返回 is_error,不挂起)
        ▼
[Tool] ask_user_question.rs
        │ 1. 校验 schema(questions 1-4 / options 2-4 / header ≤12)
        │ 2. validation 失败 → return is_error: true + 结构化错误(短路径)
        │ 3. 校验通过 → 构造 oneshot + PendingQuestion
        │ 4. QuestionStore.register(session_id, tool_use_id, oneshot, payload)
        │ 5. sink.emit_tool_question(payload)  ← IPC 推前端
        │ 6. tokio::select! { cancel_token, oneshot.recv() }
        │    ├─ cancel 触发 → return ({"cancelled_by_session": true}, is_error: true, ...)
        │    └─ oneshot 收到 → return (user_answer, is_error: false, ...)
        ▼
[Agent loop] 拿到 result,当作 tool_result 块追加到 messages
        │ 下一轮 LLM 看到答案,继续推理
```

### 3.2 Session 切换路径(不释放 oneshot)

```
[Session A active,QuestionStore: A → pending, card 在 session A 消息流]
        │
        │ 用户点击 session B tab
        ▼
[Frontend] chat.ts setCurrentSession(B)
        │ streamController 不卸载 session A 的 in-memory state(LRU 缓存)
        │ 切到 session B 消息流
        │
        │ QuestionStore 中 A → pending **不变**
        ▼
[Session B 工作...]
        │
        │ 用户切回 session A tab
        ▼
[Frontend] chat.ts setCurrentSession(A)
        │ streamController rehydrateMessages(A)
        │   → invoke("get_pending_question", { session_id: A })
        │   → Some(payload) → 注入 pending card 到 assistant ToolUse 块下方
        │   → None → 不注入
        ▼
[User 在 A 的 card 上点选项]
        │ invoke("tool:question_resolved", { session_id: A, answer: [...] })
        ▼
[Backend] commands/question.rs
        │ QuestionStore.resolve(session_id, answer)
        │   → oneshot.send(answer)
        ▼
[Agent loop] oneshot.recv() 解阻,继续 3.1 的最后一步
```

### 3.3 跳过路径(用户点 card 上的"跳过"按钮 → wire `{"cancelled": true}`)

```
[User 点 card 上 "跳过" 按钮]
        │
        │ invoke("tool:question_resolved", { session_id, cancelled: true })
        ▼
[Backend] QuestionStore.resolve(session_id, CancelledResponse)
        │ oneshot.send(CancelledResponse)
        ▼
[Tool] execute_blocking 拿到 CancelledResponse
        │ return ({"cancelled": true}, is_error: true, ...)
        ▼
[Agent loop] 短路径处理(跟普通 tool 错误一致)
        │ tool_result 块进 messages
        │ 下一轮 LLM 看到取消错误,自己决定下一步
```

### 3.4 Session cancel 路径(token.cancelled())

```
[User 点 chat 顶部的 Stop 按钮 / 关 app]
        │
        │ chat_loop.rs 的 token.cancelled() 触发
        ▼
[Tool] execute_blocking 的 tokio::select! 收到 cancel arm
        │ QuestionStore.remove(session_id) 清理
        │ return ({"cancelled_by_session": true}, is_error: true, ...)
        ▼
[Agent loop] 走现有 cancel 路径
```

## 4. Wire Protocol

### 4.1 `tool:question` IPC 事件(backend → frontend)

```typescript
// Tauri event payload
{
  session_id: string,
  tool_use_id: string,          // 对应 LLM ToolUse block.id
  questions: Array<{
    question: string,
    header?: string,            // ≤12 字符
    options: Array<{
      label: string,
      description?: string,
      preview?: string
    }>,                          // 2..=4
    multi_select: boolean       // 默认 false
  }>,                            // 1..=4
  ts: number                    // unix ms, 用于排序
}
```

事件名: `tool:question`(避开 `tool:*` 已用通道)。

### 4.2 `tool:question_resolved` IPC 调用(frontend → backend)

```typescript
// Tauri invoke payload
{
  session_id: string,
  tool_use_id: string,
  // 二选一:
  answer?: Array<{              // 正常回答(按 questions 顺序)
    question: string,
    header?: string,
    options: string[],          // 选中的 label 数组(单选 1 元素,多选 N)
    multi_select: boolean
  }>,
  cancelled?: true              // 用户跳过
}
```

后端 handler(commands/question.rs):
```rust
#[tauri::command]
pub async fn resolve_tool_question(
    state: State<'_, AppState>,
    payload: ToolQuestionResolvePayload,
) -> Result<(), String> {
    state.question_store.resolve(&payload.session_id, payload.into()).await
        .map_err(|e| e.to_string())
}
```

### 4.2a `get_pending_question` IPC 调用(frontend → backend,新增)

```typescript
// Tauri invoke payload
{ session_id: string }

// 返回
{ payload: ToolQuestionPayload | null }  // null = 没有 pending
```

后端 handler(commands/question.rs):
```rust
#[tauri::command]
pub async fn get_pending_question(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<ToolQuestionPayload>, String> {
    Ok(state.question_store.get(&session_id).await
        .map(|p| p.payload))
}
```

调用时机:`streamController` 在 session 切换 / `rehydrateMessages` 时调,拿到 payload 后注入到对应 assistant turn 的 ToolUse 块下方。

### 4.3 `tool:question` 事件 listener(前端)

`app/src/stores/streamController.ts` 注册 Tauri listener:

```typescript
listen<ToolQuestionPayload>("tool:question", (event) => {
  const { session_id, tool_use_id, questions, ts } = event.payload;
  questionCardsStore.addPending({
    sessionId: session_id,
    toolUseId: tool_use_id,
    questions,
    ts,
  });
});
```

事件分发走 `streamController`(单源),不挂在 component lifecycle 里。

### 4.4 答案回写(前端 invoke)

```typescript
// 在 AskUserQuestionCard.vue 的按钮 handler 里
async function handleAnswer(answer: AnswerPayload) {
  await invoke("tool:question_resolved", {
    sessionId: props.sessionId,
    toolUseId: props.toolUseId,
    answer,
  });
  // 不需要等响应 — QuestionStore.resolve 同步,oneshot.send 是同步非阻塞
  // Card 内部切到 "已回答" 状态(展开保留,已选项高亮)
}
```

## 5. State Management

### 5.1 Backend: QuestionStore

```rust
// app/src-tauri/src/agent/question_store.rs (新文件)

pub type QuestionOneshot = tokio::sync::oneshot::Sender<QuestionResponse>;

pub enum QuestionResponse {
    Answered(Vec<QuestionAnswer>),     // 正常回答
    Cancelled,                          // 用户取消
    SessionCancelled,                   // session cancel 中断
}

pub struct PendingQuestion {
    pub tool_use_id: String,
    pub oneshot: Option<QuestionOneshot>,  // Option 因为 resolve 后清空
    pub payload: ToolQuestionPayload,      // 给前端 reload 用
}

#[derive(Clone)]
pub struct QuestionStore {
    inner: Arc<tokio::sync::Mutex<HashMap<String /* session_id */, PendingQuestion>>>,
}

impl QuestionStore {
    pub fn new() -> Self;
    pub async fn register(
        &self,
        session_id: &str,
        tool_use_id: &str,
        payload: ToolQuestionPayload,
    ) -> Result<oneshot::Receiver<QuestionResponse>, QuestionStoreError>;
    pub async fn resolve(
        &self,
        session_id: &str,
        response: QuestionResponse,
    ) -> Result<(), QuestionStoreError>;
    pub async fn remove(&self, session_id: &str) -> Option<PendingQuestion>;
    pub async fn get(&self, session_id: &str) -> Option<PendingQuestion>;
    pub async fn list(&self) -> Vec<(String, ToolQuestionPayload)>;
}
```

`QuestionStoreError`:
- `NotFound` (resolve 时找不到 key)
- `AlreadyResolved` (双重 resolve,oneshot 已被 take)

挂在 `AppState`:
```rust
// app/src-tauri/src/state.rs
pub struct AppState {
    // ... 现有字段
    pub question_store: QuestionStore,  // 新
}
```

### 5.2 Backend: AppHandleSink 扩展

> **实施记录(2026-06-30)**:原设计写的 `async fn emit_tool_question` **已改为 sync fn**。原因:`ChatEventSink` trait 现有所有方法都是 sync(为保持 `Arc<dyn ChatEventSink>` dyn-compatible,不引入 async-trait 依赖)。新方法跟随现有 sync 设计,它包装的 `app.emit` 本身非阻塞,async 无收益。落地签名:`fn emit_tool_question(&self, payload: &ToolQuestionPayload)`。

`AppHandleSink`(在 `state.rs` 里实现 `ChatEventSink` trait)需要新加一个 `emit_tool_question` 方法:

```rust
pub trait ChatEventSink: Send + Sync {
    fn emit_chat_event(&self, payload: &ChatEventPayload);
    fn emit_tool_call(&self, payload: &ToolCallPayload);
    fn emit_tool_result(&self, payload: &ToolResultPayload);
    fn emit_tool_question(&self, payload: &ToolQuestionPayload);  // 新(sync,对齐 trait)
}

impl ChatEventSink for AppHandleSink {
    // ... 现有方法
    fn emit_tool_question(&self, payload: &ToolQuestionPayload) {
        let _ = self.app.emit("tool:question", payload);
    }
}
```

> **注**:现有 trait 方法签名(上方示例展示为 sync)以代码为准。`emit_tool_question` 用 trait default 提供 `tracing::warn!` no-op 默认实现,`AppHandleSink` 与 `MockEmitter` 各自 override;**`SubagentBufferSink` 不 override**(用默认 no-op)——worker 路径在 `STRUCTURALLY_DISABLED` 阶段已剥离该 tool,worker 永远不会到达此方法(见 §7)。

测试用的 `MockEmitter` 同步实现(存 `Vec<ToolQuestionPayload>` 用于断言)。

### 5.3 Frontend: Pinia store

```typescript
// app/src/stores/questionCards.ts (新文件)

import { defineStore } from "pinia";

interface PendingQuestion {
  sessionId: string;
  toolUseId: string;
  questions: Question[];
  ts: number;
}

// ⚠ pendingBySession 是**缓存**;唯一 source of truth 是 backend get_pending_question。
// streamController.ensureLoaded 必须以 invoke 结果覆盖本缓存(messagesBySession 按 LRU
// 淘汰会让缓存变脏,而 pending 活在 backend QuestionStore 不受 LRU 影响)。
export const useQuestionCardsStore = defineStore("questionCards", {
  state: () => ({
    pendingBySession: new Map<string, PendingQuestion>(),
  }),
  actions: {
    addPending(p: PendingQuestion) {
      this.pendingBySession.set(p.sessionId, p);  // 覆盖语义,供 ensureLoaded 同步用
    },
    removePending(sessionId: string) {
      this.pendingBySession.delete(sessionId);
    },
    getPending(sessionId: string): PendingQuestion | undefined {
      return this.pendingBySession.get(sessionId);  // 仅缓存读;跨 session 切回前须 ensureLoaded 校正
    },
  },
});
```

### 5.4 Frontend: streamController 集成

`app/src/stores/streamController.ts` 在 `ensureLoaded` 时,**以 backend 为 source of truth** 校正缓存并合并 pending(不能只读前端缓存——LRU 淘汰后缓存会脏):

```typescript
async function ensureLoaded(sessionId: string) {
  // ... 现有从 DB 加载 messages

  // 新:invoke get_pending_question 拿 backend 真值,覆盖前端缓存
  const questionCardsStore = useQuestionCardsStore();
  const payload = await invoke<ToolQuestionPayload | null>(
    "get_pending_question", { sessionId }
  );
  if (payload) {
    questionCardsStore.addPending({ sessionId, ...payload });  // 覆盖缓存
    messages = injectPendingCard(messages, payload);          // 注入到对应 ToolUse 块下方
  } else {
    questionCardsStore.removePending(sessionId);              // 校正:backend 已无 → 清脏缓存
  }
}
```

切换 session 时不卸载 pending——LRU 自然保留 in-memory state,QuestionStore 不参与 LRU 淘汰(它是 backend 端的)。

### 5.5 AskUserQuestionCard 组件

`app/src/components/chat/AskUserQuestionCard.vue`:

> ⚠ **UI 硬约束(红线)**:必须是消息流 **inline card**(`ToolCallCard` 同层,DOM 是 message list 的 child)。**禁止** reka-ui `Dialog`/`Popover`/`AlertDialog`、**禁止** `<Teleport to="body">`、**禁止**浮层 / 遮罩 / mask。理由:多 session 并行 + R9–R11 session 挂起保留——modal 无法跨 session 保留 pending,且会遮挡其他 session 的工作区。违反 → AC10 失败。

- props: `sessionId`, `toolUseId`, `questions`, `state: 'pending' | 'answered' | 'cancelled'`
- 渲染:多 question 单 card,每 section 一题
  - header chip(≤12 字符)+ 题干
  - 选项:radio / checkbox(根据 multi_select)
  - description 在 label 下方
  - preview 在折叠面板
- 底部(pending 状态):
  - "提交"按钮:用户填完**所有** question 后一次性 invoke(对齐 wire §4.2 一次性 answer 数组;**禁止单选即时 resolve**)
  - "跳过"按钮:语义不回答 → wire `{"cancelled": true}`(一次性作用于整张 card)
- answered 状态:显示已选项摘要 + 仍展示全部内容(展开保留)
- cancelled 状态:显示"已跳过"提示
- 状态切换:选项点击**仅累积本地 state**(单选 radio / 多选 checkbox,不 invoke)→ 点"提交"收集所有答案一次性 `invoke("tool:question_resolved", { answer })` → ack 后切 answered;点"跳过" → `invoke(..., { cancelled: true })` → 切 cancelled

> **实施记录(2026-06-30)**:落地时两点偏离原描述(均不改 wire / 视觉契约,仅为 UX + 测试友好):
> 1. **选项 click handler 挂在 `<li>` 行(非 input `@change`)**——jsdom 友好 + 点击行内任意位置都生效。`<input>` 仍保留 `@change` 供键盘驱动,`@click.stop` 防止点 radio 圆点时双触发;`<details>` preview 也用 `@click.stop` 避免展开预览误触选择。
> 2. **`ask-card__option--selected` 反映实时选中状态(非 post-submit)**——原描述暗示该 class 仅在 submit 后出现,但用户 composing 时需要即时视觉反馈,否则"整体提交语义"看起来像没生效。class 现反映 live 选中;`--disabled` 修饰仍只 post-submit 应用(R7a)。

### 5.6 MessageItem tool dispatch(R22)

`app/src/components/chat/MessageItem.vue` / `MessageList.vue` 增加 tool name → component 分发:

```typescript
// MessageItem.vue
const toolComponent = computed(() => {
  if (toolUse.name === "ask_user_question") return AskUserQuestionCard;
  return ToolCallCard;
});
```

`<AskUserQuestionCard>` 在 `<ToolCallCard>` 紧下方插入,共享 assistant turn 上下文。`AskUserQuestionCard` 从 `questionCardsStore.pendingBySession.get(sessionId)` 拿 pending payload(若 store 没有 → 走 `get_pending_question` 命令刷新)。

> **实施记录(2026-06-30)**:实际落地的 state 解析为 **3-tier lookup + memoized helper**:
> 1. **Tier 1 — live pending**:`questionCardsStore.pendingBySession` 按 `tool_use_id` 匹配 → `state: 'pending'`
> 2. **Tier 2 — DB tool_result envelope**:解析 `parseAnswerEnvelope(content)` → `{"cancelled": true}` → `state: 'cancelled'`;否则有 answer 数组 → `state: 'answered'`(支持 reload 后只读回顾)
> 3. **Tier 3 — null(防御性)**:既无 pending 又无 tool_result(tool_use → tool_result 短暂窗口期)→ 不挂空 card(`v-if="...!==undefined"` 守卫)
>
> 另加 memoized `askCardPropsFor(tc)` helper:`v-if` + `v-bind` 每次渲染都调 parser 会重复执行,helper 短路非 `ask_user_question` 的 tool name + 单次 memo,避免每 render 重复解析。

## 6. Concurrency Model

### 6.1 单 pending 互斥

`QuestionStore.register` 时:
```rust
if inner.contains_key(session_id) {
    return Err(QuestionStoreError::AlreadyPending);
}
```

LLM 同 turn 第二次调 → `ask_user_question::execute_blocking` 在 register 阶段返回结构化错误:

```json
{"error": "已有 pending question,等当前回答完成"}
```

LLM 自然串行(下一个 turn 再调)。

### 6.2 与 permission:ask 不冲突

QuestionStore 和 PermissionStore 是两个独立 `Arc<Mutex<HashMap>>`,互不影响。同一 session 可以同时有 pending question + pending permission ask。

### 6.3 批次执行顺序与并行性排除(关键)

**`ask_user_question` 天然排除在 parallel 之外,无需改 `is_parallel_eligible`**。`chat_loop.rs::is_parallel_eligible` 是**白名单**机制:只有 `NAME_ELIGIBLE = [read_file, grep, glob, list_dir, use_skill]` 且路径在项目内的纯读批次才返回 true。`ask_user_question` 不在白名单 → 任一含它的批次返回 false → **整批自动走 L1a 串行**(跟 `dispatch_subagent` 同机制——后者也是靠不在白名单走串行,而非显式 `=> false` 分支)。

> 实现者注意:不要去 `is_parallel_eligible` 里加 `ask_user_question => false` 分支(那是黑名单心智模型,与现有白名单机制不符)。它天然返回 false。

(下面"批次内执行顺序"与"turn 计数器代价"两段不变。)

**批次内执行顺序(Claude Code 行为)**:
- 按 LLM 声明的 tool_use 顺序串行执行
- `ask_user_question` 在其位置**阻塞等待用户回答**(同 batch 其他 tool 全部暂停,即使在 question 之后的 tool 也等待)
- 之前的 tool 执行结果 + question 解决后的结果 + 之后的 tool 执行结果 → 全部进 messages → 下一轮 LLM 看到完整结果

示例:LLM batch = `[shell, ask_user_question, write_file]`
1. shell 执行 → result 进 messages
2. ask_user_question 阻塞 → 用户回答 → result 进 messages
3. write_file 执行 → result 进 messages
4. 下一轮 LLM 看到所有结果

**Turn 计数器代价**:`for turn in 1..=turn_limit` 是 Rust range iterator,blocking tool 恢复后 turn 必然递增 1 次。v1 接受这个代价(MAX_TURNS=50,blocking tool 是稀有事件);v2 可重构 while 循环实现零消耗。

## 7. Worker 禁用

> **实施记录(2026-06-30)**:原设计写的文件路径 `app/src-tauri/src/agent/permissions/mode.rs` **是错误的**。`STRUCTURALLY_DISABLED` 常量与 `filter_tools_for_subagent` 实际都定义在 `app/src-tauri/src/agent/subagent/mod.rs`(常量被该函数消费)。`permissions/mode.rs` 只含无关的 `filter_tools_for_mode`(Plan-mode write dropping)。落地加在正确位置 `subagent/mod.rs`,常量列表是唯一 source of truth。

`app/src-tauri/src/agent/subagent/mod.rs::filter_tools_for_subagent` 的 `STRUCTURALLY_DISABLED` 列表:

```rust
const STRUCTURALLY_DISABLED: &[&str] = &[
    "dispatch_subagent",
    "update_checklist",
    "run_background_shell",
    "shell_status",
    "shell_kill",
    "ask_user_question",  // 新
];
```

防御深度:`chat_loop.rs` 的 per-turn tool list 构建阶段(L3d 模式)对 `effective_is_worker == true` 的路径已经 gate 住 `dispatch_subagent`,同样 gate 住 `ask_user_question` 的 L3d 追加。

## 8. Session Switch Preservation

### 8.1 Backend 不释放

`QuestionStore` 在 session switch 时**不**调 `remove(session_id)`。oneshot 持续存活,agent loop 持续挂起。

只有以下情况移除:
- User 回答(resolve → oneshot.send → 清空)
- User 取消(resolve → Cancelled → oneshot.send → 清空)
- Session cancel(token.cancelled() → execute_blocking 的 cancel arm → 清空)
- App 进程死亡(整体清空,可接受)

> 即 v1 **无 timeout**:用户既不提交/跳过、也不点 Stop → 该 agent 的 tokio task 持续阻塞。可接受(单 pending 互斥 + MAX_TURNS 不受影响,阻塞不计 turn 推进)。auto-decide / 超时放 v2。

### 8.2 Frontend 合并渲染

切换 session 时:
1. `streamController` 不卸载 LRU 中的 session state
2. 切回的 session 加载 messages 时,`questionCardsStore.getPending(sessionId)` 查询
3. 有 pending → 把 `AskUserQuestionCard` 注入到对应 assistant turn 的 ToolUse 块下方
4. 无 pending → 不注入,正常渲染

### 8.3 与现有 `permission:ask` 行为对比

| 触发条件 | permission:ask | ask_user_question |
|---|---|---|
| Session 切换 | cancel all(security) | 保留 pending(UX) |
| User 关 modal/card | cancel | N/A(no modal) |
| User 点"取消" | N/A | cancel |
| Session cancel token | cancel | cancel |
| App 崩溃 | 全部丢失 | 全部丢失 |

这是**故意的不一致**——permission 是 security gate 必须立即决策;question 是 UX,可以挂起保留。

## 9. Trade-offs(关键决策回顾)

| 决策 | 选择 | 替代方案 | 选择理由 |
|---|---|---|---|
| UI 形态 | Inline card | Modal | 项目多 session 并行架构,modal 不适合跨 session 保留 |
| Session switch | 保留 pending | Cancel on switch | UX 需求:用户切走查资料,回来能继续 |
| IPC | 新 `tool:question` | 复用 `permission:ask` | 语义不同(security vs UX),schema 不同 |
| 持久化 | 只 messages 表 | 加 `tool_questions` 表 | v1 简化,messages + audit 已够 |
| 超时 | 无 | 默认 5min timeout | v1 简化,放 v2 |
| Schema | 照搬 Claude Code | 自创 | LLM 训练过,零学习成本 |
| Worker | 禁用 | 允许 | Worker 反问违反"自主执行"语义 |
| 并发 | 单 pending 互斥 | 多 question 排队 | v1 简化,LLM 自身不并发 |
| Yolo mode | v1 三档一致,均挂起 | Yolo auto-decide 默认项 | Yolo 跳过*权限*询问,不跳过*信息*询问;auto-decide 留 v2 |

## 10. Rollback Considerations

回滚成本评估(按"如果 v1 出问题要全砍掉"算):

### 简单回滚(单文件)

- 删 `app/src-tauri/src/tools/ask_user_question.rs`
- 删 `app/src-tauri/src/agent/question_store.rs`
- 删 `app/src/components/chat/AskUserQuestionCard.vue`
- 删 `app/src/stores/questionCards.ts`
- 删 `app/src-tauri/src/commands/question.rs`
- 删 `app/src-tauri/src/types/tool_question.rs`

### 需要还原的改动

- `tools/mod.rs::execute_tool` 删 `match "ask_user_question"` 分支
- `tools/mod.rs::builtin_tools` 删 `ask_user_question::definition()`
- `agent/chat_loop.rs` 删 blocking tool 挂起点逻辑
- `agent/subagent/mod.rs::STRUCTURALLY_DISABLED` 删 `"ask_user_question"`(注:非 `permissions/mode.rs`,见 §7 实施记录)
- `state.rs` 删 `question_store` 字段 + `ChatEventSink::emit_tool_question`
- `commands` 模块注册删 `resolve_tool_question`

### Schema migration 回滚

**无 schema migration**——messages 表不动,audit 不动。如果未来要加 `tool_questions` 表再做 migration。

### 回滚演练时机

v1 落地后,如果发现:
- 性能问题(LRU + QuestionStore 内存增长)→ 单独优化,不整体回滚
- UX 问题(用户不喜欢 inline card)→ 调整 Card 组件
- LLM 行为问题(模型滥用,频繁问)→ 调整 `Risk` 评估加 Tier 4 ask

整体回滚仅在架构假设错误时(比如"inline card 跨 session 保留"被证伪)。

## 11. Test Strategy

### 11.1 单元测试(Rust)

- `app/src-tauri/src/tools/ask_user_question.rs::tests::validate_schema_*`
  - questions 空 / >4 / 各字段越界 → is_error: true
- `app/src-tauri/src/agent/question_store.rs::tests::register_resolve_*`
  - 正常 register/resolve 配对
  - 二次 register 同 session → AlreadyPending error
  - resolve 不存在的 session → NotFound
  - 双重 resolve → AlreadyResolved
- `app/src-tauri/src/agent/permissions/tests_mode.rs` 加测试
  - `filter_tools_for_subagent` 输出不含 `ask_user_question`

### 11.2 集成测试(Rust)

- `app/src-tauri/src/agent/tests_ask_user_question.rs`(新建,在 `agent/mod.rs` 注册)加 `agent_loop_ask_user_question_*` 测试
  - Mock provider 返回 ToolUse(ask_user_question) + ToolUse(shell) 序列
  - MockEmitter 捕获 `tool:question` 事件
  - 测试用 `MockQuestionStore.resolve` 模拟 user 回答
  - 验证:tool_result 正确回灌 messages,**turn 计数因 blocking tool 递增 1 次**(v1 接受,见 §6.3 / PRD R3)
  - 验证:cancel 路径正确返回结构化错误

### 11.3 单元测试(前端)

- `app/src/components/chat/AskUserQuestionCard.test.ts`
  - 渲染多 question section
  - 单选 / 多选切换
  - 答完状态切换(展开保留 + 已选项高亮)
  - 取消按钮触发 invoke
- `app/src/stores/questionCards.test.ts`
  - addPending / removePending / getPending

### 11.4 E2E / 手工测试

- 真实 chat 跑通:agent 调 `ask_user_question` → 消息流出现 inline card → 点击选项 → 下一轮 LLM 看到答案
- Session 切换保留:session A 有 pending → 切 B 工作 → 切回 A → card 仍在可答
- Session cancel:Stop 按钮 → pending 清理 + tool_result 是 cancelled_by_session 错误
- Worker 不调用:用 dispatch_subagent 派 worker → worker tool list 不含 ask_user_question

## 12. Open Items for Implementation

(实施阶段需要确认的细节,不影响本 design):

- `ContentBlock` 是否需要新增 `PendingQuestion` variant?(**不需要**——pending state 只在 `QuestionStore` 内存,messages 不存)
- `ChatEvent` 是否新增 variant?(**不需要**——走新 IPC channel `tool:question`,不走 `chat-event`)
- 前端 `vue-tsc --noEmit` 类型严格度?(沿用项目现有 strict 模式)
- Card 样式 token 用 `var(--xxx)` 还是 `bg-{color}-{shade}` reka-ui 体系?(沿用 `ToolCallCard` 现有模式)