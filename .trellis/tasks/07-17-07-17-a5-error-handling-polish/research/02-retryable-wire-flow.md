# Research: `AppCommandError.retryable` 字段穿透链路

- **Query**: `AppCommandError.retryable` 字段当前是否已贯穿到前端?前端能否消费?chat 流错误(`ChatEvent::Error`)能否透出 retry 按钮?
- **Scope**: internal (后端 wire + 前端消费 + chat 流通道)
- **Date**: 2026-07-17

## Findings

### Files Found

| File Path | Description |
|---|---|
| `app/src-tauri/src/error.rs:68-108` | `AppCommandError` struct 定义(`retryable: bool` 字段)+ `new()` / `with_request_id()` 构造器 |
| `app/src-tauri/src/error.rs:55-66` | `AppError` trait `retryable()` 默认实现(按 category 派生) |
| `app/src-tauri/src/error.rs:380-481` | `From<LlmError>` / `From<GitError>` / ... / `From<anyhow::Error>` 11 个 `impl From<E>` for AppCommandError |
| `app/src-tauri/src/error.rs:114-123` | `build::<E: AppError>(e)` helper — 调 `e.retryable()` 写入 `retryable` |
| `app/src-tauri/src/llm/types.rs:408-412` | **`ChatEvent::Error` wire 定义**:`{ message: String, category: LlmErrorCategory }` — **没有 retryable 字段** |
| `app/src-tauri/src/llm/types.rs:540-549` | `LlmErrorCategory` enum(`#[serde(rename_all = "snake_case")]` → `auth/rate_limit/invalid_request/server/network`) |
| `app/src-tauri/src/state.rs:504-512` | `ChatEventPayload` — `#[serde(flatten)] event: ChatEvent`,`request_id: String` 外加 |
| `app/src-tauri/src/agent/chat_loop.rs:1732-1750` | **唯一的 `ChatEvent::Error` emit** (per-event arm 内 LlmError → ChatEvent::Error) |
| `app/src-tauri/src/agent/chat_loop.rs:430-510` | 3 处前端不可见的 `ChatEvent::Error` emit(`session/project not found`, InvalidRequest 兜底) |
| `app/src-tauri/src/agent/chat_loop.rs:4134-4150` | `emit_persist_failure` helper — emit Error{Server} 持久化失败 |
| `app/src-tauri/src/utils/useErrorBus.ts:36-42` | 前端 `AppCommandError` interface — `retryable: boolean` 字段已声明 |
| `app/src/utils/useErrorBus.ts:78-89` | `isAppCommandError()` — 含 `retryable === "boolean"` 校验 |
| `app/src/utils/useErrorBus.ts:98-117` | `parseAppCommandError()` — 容错解析 + string fallback 强制 `retryable: false` |
| `app/src/utils/useErrorBus.ts:138-156` | `routeByCategory()` — 5 类 stub,**没有任何读取 retryable 的代码** |
| `app/src/stores/streamController.ts:200-203` | `ChatEventPayload` interface — `category?: ErrorCategory`,**没有 retryable 字段** |
| `app/src/stores/streamController.ts:1148-1153` | `case "error":` — `last.error = { message, category }`,**不读 retryable** |
| `app/src/components/chat/MessageItemFooter.vue:62-69` | `error: { message, category? }` prop 类型 — **不含 retryable** |
| `app/src/components/chat/ToolCallCard.vue` | tool 失败状态 — 通过 `result.isError` 标记,不消费 AppCommandError.retryable |
| `app/src/components/chat/MessageItem.vue` | `onResend` / `handleResend` 路径已存在(走 `chatStore.resendMessage`),不在 retryable 链路上 |
| `app/src-tauri/src/llm/retry.rs:73-91` | `RetryPolicy::full_jitter()` + `retryable` LlmError 派生 — retry 决策在 backend |
| `app/src-tauri/src/llm/error.rs` | `LlmError` 的 inherent `is_retryable()` / `retryable()` 方法 |
| `app/src/components/chat/ChatPanel.vue:152` | `diffError.value = e instanceof Error ? e.message : extractErrorMessage(e)` — local UI,非全局 |

### Code Patterns

#### Pattern 1: `AppCommandError.retryable` 后端实现(IPC command 通道,完整链路)

`app/src-tauri/src/error.rs:68-108`:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCommandError {
    pub category: ErrorCategory,
    pub kind: String,
    pub message: String,
    pub retryable: bool,           // ← 字段存在
    pub request_id: Option<String>,
}

impl AppError {
    fn retryable(&self) -> bool {  // ← trait 默认
        matches!(
            self.category(),
            ErrorCategory::Server | ErrorCategory::Network | ErrorCategory::RateLimit
        )
    }
}

fn build<E: AppError>(e: &E) -> AppCommandError {
    AppCommandError {
        category: e.category(),
        kind: kind_of(e),
        message: e.user_message(),
        retryable: e.retryable(),   // ← 自动派生
        request_id: None,
    }
}
```

- **后端 IPC 通道**(`Result<T, AppCommandError>` 的 Tauri command):retryable 字段**已完整可用**。`From<LlmError>` / `From<anyhow>` 等 11 个 From impl 都走 `build(e)` helper(`error.rs:380-481`),**retryable 自动按 category 派生**(`Server | Network | RateLimit → true`,`Auth | InvalidRequest → false`)。
- **anyhow fallback 路径**(`error.rs:472-480`)显式 `retryable: true`,因为 "未命中已知类型归 Server" 而 Server 默认 retryable=true,等同逻辑正确。
- **`AppError::retryable` 在初版曾被设计 override**(`error.rs:108` 注释: "本期零 override (初版基于不存在的 BackgroundShellError::Timeout 的 override 设计已删除)"),即**所有 10 个 impl 都靠默认派生**,没有任何 variant 级 override。

#### Pattern 2: 前端 `AppCommandError` interface — 字段已就位但**消费 = 0**

`app/src/utils/useErrorBus.ts:36-42`:

```ts
export interface AppCommandError {
  category: ErrorCategory;
  kind: string;
  message: string;
  retryable: boolean;          // ← 字段已就位
  requestId?: string;
}
```

`app/src/utils/useErrorBus.ts:78-89`(`isAppCommandError` 校验):

```ts
return (
  typeof o.category === "string" &&
  VALID_CATEGORIES.has(o.category as ErrorCategory) &&
  typeof o.kind === "string" &&
  typeof o.message === "string" &&
  typeof o.retryable === "boolean"   // ← 校验就位
);
```

`app/src/utils/useErrorBus.ts:109-114`(string fallback):

```ts
return {
  category: "Server",
  kind: "Unknown",
  message: e,
  retryable: false,            // ← fallback 也写死 false
};
```

- **字段契约已完整**:`isAppCommandError` 校验 + `parseAppCommandError` 解析 + interface 声明 — 任何从 IPC 拿到的对象都会被解析出 `retryable: boolean`。
- **但消费端完全空**:`routeByCategory` 5 个 stub(`useErrorBus.ts:138-181`)+ `handle()`(`main.ts:18`) + 36 处 `showToast` 调用 — **没有任何代码读 `err.retryable`**。grep `retryable` 全仓只有 2 处:type 声明 + 校验。

#### Pattern 3: ChatEvent::Error wire shape — **没有 retryable**

`app/src-tauri/src/llm/types.rs:341-413`:

```rust
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    Start,
    Delta { text: String },
    ThinkingDelta { text: String },
    SignatureDelta { signature: String },
    RedactedThinkingDelta { data: String },
    ToolCall { id: String, name: String, input: serde_json::Value },
    Done { stop_reason: Option<String>, usage: Option<TokenUsage> },
    TurnComplete { seq: i64, ttfb_ms: Option<i64>, gen_ms: Option<i64>, total_ms: Option<i64>, thinking_ms: Option<i64> },
    Error {
        message: String,
        category: LlmErrorCategory,   // ← 没有 retryable
    },
    Retrying { attempt: u32, max_attempts: u32, wait_ms: u64, reason: String },
    // ...
}
```

`app/src-tauri/src/agent/chat_loop.rs:1732-1750`(per-event arm emit 位置):

```rust
Err(err) => {
    tracing::warn!(
        request_id = %rid,
        turn,
        category = ?err.category(),
        error = %err,
        "chat: LLM stream errored"
    );
    ChatEvent::Error {
        message: err.user_message(),
        category: err.category(),
        // ← retryable 字段不在 ChatEvent::Error struct 中,无法 emit
    }
}
```

**关键缺口**:后端 emit `ChatEvent::Error` 时,只携带 `category`(5 类 string),**没有 retryable 字段**。前端拿到的 event 只能从 `category` 推断 retryability(`Server/Network/RateLimit → retryable`)。
前端 streamController 的 case "error" handler:

`app/src/stores/streamController.ts:1148-1153`:

```ts
case "error":
  last.streaming = false;
  last.error = {
    message: event.message ?? "未知错误",
    category: event.category ?? "server",
    // ← 没有 retryable
  };
```

`app/src/stores/streamController.ts:2185-2192`(invoke IPC 失败同步分支):

```ts
} catch (e) {
  const msgs = messagesBySession.get(args.sessionId);
  if (msgs) {
    const last = msgs[msgs.length - 1];
    if (last && last.role === "assistant") {
      last.streaming = false;
      last.error = { message: extractErrorMessage(e), category: "server" };
      // ← 这里也没有 retryable(IPC 失败 = Server,语义上 retryable=true 但字段未传)
    }
  }
  finalizeRequest(requestId, args.sessionId, true);
}
```

前端 `ChatMessage.error` interface(`chat.types.ts:179`):

```ts
error?: { message: string; category: ErrorCategory };
// ← 没有 retryable: boolean
```

#### Pattern 4: 前端错误渲染点 — 4 处消费 `last.error`,0 处消费 retryable

| 渲染点 | file:line | 消费什么 |
|---|---|---|
| `MessageItemFooter.vue:62-69` | prop `error: { message, category? }` | 仅显示 `error.message`(`MessageItemFooter.vue:136-138`) |
| `MessageItem.vue` 渲染 | `MessageItemFooter` 仅 message | 0 字段消费 |
| `ToolCallCard.vue:64-87` | `result.isError` | 走 `tool_result.is_error`(非 AppCommandError) |
| `SubagentDrawerErrorCard.vue:33-41` | `errorMessage: string` prop | 仅显示字符串 |

**`category` 字段**已经被 `MessageItemFooter` 接受但**未实际渲染**(仅 `category?: string` 类型声明,模板里没用),所以即便前端知道 category 是 Server,UI 上也看不出"可重试"信号。

#### Pattern 5: 已存在的 resend 路径(可借鉴,不直接复用)

`app/src/components/chat/MessageActionsMenu.vue:156-159`:

```ts
function onResend() {
  if (!canResend()) return;
  emit("resend", props.messageSeq);
}
```

`app/src/stores/chat.ts:1257-1341`(`resendMessage`):

```ts
async function resendMessage(sessionId, messageSeq, contentText) {
  // 1. cancel any in-flight stream on this session
  if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
    await cancel();
  } else {
    const rid = controller.currentRequestId(sessionId);
    if (rid) await controller.cancel(rid);
  }
  // 2. compute nextSeq + push placeholders + startRequest({ resendSeq: messageSeq })
  // ...
}
```

- resend 路径已实现,但**只针对 user message**(D3 PR3,2026-06-17)。Error footer 想要的"重试"是 **assistant error 的 turn-level retry**(同一 request 重发),不是 user message resend。
- 语义差异:resend 复用同一 user content + resendSeq audit;retry 应在 LlmError 后端 retry budget 耗尽后由前端触发,**重新调 `startRequest` 用同 history**(不带 resendSeq,不带 audit)。

### External References

- `.trellis/spec/backend/error-handling.md:128-152` — `AppCommandError` schema(camelCase wire);`retryable` 字段明文列出。
- `.trellis/spec/backend/error-handling.md:155-161` — `request_id` 语义:"`chat` command 透传前端 requestId;其余 None"。
- `.trellis/spec/backend/error-handling.md:165-174` — "前端路由:Auth→Settings 引导 / RateLimit→toast / InvalidRequest→inline / Server→toast+重试 / Network→toast"。**spec 期望 Server/Network 出错时给"重试"按钮**,但当前 spec 是**路由建议**而非**实现契约**(A5 R3.4 仅写了路由 key,没写 retry 按钮如何接)。
- `.trellis/spec/backend/error-handling.md:206-264` — Agent Loop Error Paths(详细描述 `ChatEvent::Error` 是 terminal;`ERROR_MARKER` = "[生成出错中断]" 文案),但**未提 retryable wire**。
- `.trellis/spec/backend/agent-loop-architecture.md` — cancel 一次即终止(RULE-A-010);retry 在 LlmError 后端 retry budget 内已经做了(`llm/retry.rs::retry_open`),前端 retry 是 budget 耗尽后的兜底。

### Related Specs

- `.trellis/spec/backend/error-handling.md` §"AppError trait" — `retryable()` 默认按 category 派生的语义。
- `.trellis/spec/backend/error-handling.md` §"API Error Responses" — retryable 字段来源契约。

## Caveats / Not Found

1. **`ChatEvent::Error` 没有 retryable 字段**(wire 层缺口),但**前端可从 `category` 推断**:`Server | Network | RateLimit` → retryable。推断逻辑跟后端 `AppError::retryable()` 默认派生**完全一致**,所以前端推断结果与后端真值必然等价,**无需后端扩 wire**。
2. **`chat` IPC 同步失败路径**(`streamController.ts:2185-2192`,在 `await invoke("chat", ...)` 抛出时)目前写死 `category: "server"`,但**实际上抛出的是 `AppCommandError` 对象**(Tauri 自动序列化为 rejection),里面已有 retryable 字段。**前端只读取了 category**,**丢了 retryable 信息**。这是一个低成本修复点:`last.error = { message, category, retryable: parsed?.retryable ?? (category === "server" || ...) }`。
3. **`MessageItemFooter.vue` 已经定义了 `category?: string` 但模板不渲染** — 提前预留的位,scope B 加 retry 按钮只需"渲染 category → 决定按钮可见性 + 调 `startRequest`"。
4. **`routeByCategory` 5 个 stub 完全不读 retryable**(`useErrorBus.ts:138-181`)— scope B 即使接 toast,也只能根据 `category` 派生 button 可见性,不需要改 wire。
5. **未找到任何 `useErrorBus().handle(...)` 在业务代码的调用** — 唯一调用点 `main.ts:18`(全局兜底)。IPC 失败 36 处全部走各自 `try/catch + showToast`,**不经过 useErrorBus**,所以"接 toast 接哪一类"这个问题在 useErrorBus 层面只影响 1 处主链路。
6. **后端 `LlmError::retryable` 已实现**(`llm/error.rs` inherent),但**没有 emit 到 ChatEvent::Error**,所以前端的 retry 决策**只能用 category 推断**,无法读 backend 的真实 retryable 判定(如 manual retry budget 耗尽 vs 自动成功)。当前 backend retry policy = Full Jitter 自动 3 次,耗尽后才 emit Error,所以"用户主动 retry"等价于"自动 retry 已耗尽 → 强制再调一次"。

## 给 design.md 的输入

**Wire 链路**:后端 `AppCommandError.retryable` 完整存在(error.rs:73),前端 interface 已对齐(useErrorBus.ts:36-42),但消费端完全空(0 处读 retryable)。**`ChatEvent::Error` 不携带 retryable**(types.rs:409-412),前端只能从 `category` 推断,而推断规则与后端 `AppError::retryable()` 默认派生完全一致(Server/Network/RateLimit → true),**不需要后端扩 wire**。scope B 推荐**纯前端改**:(1) `MessageItemFooter` 的 `error` prop 加 `retryable?: boolean`,从 category 派生默认值;(2) footer 渲染 retry 按钮(visible when retryable),点击调 `controller.startRequest({ history, ... })` 复用同一 history 重发;(3) `chat` IPC 同步失败路径补上 retryable 字段;(4) useErrorBus 5 stub 仍保持 console.warn(不进 toast,scope B 不动)。**改动量 ~50-80 行**(1 个 prop + 1 个按钮 + 1 个 click handler + 1 行 category 推断 helper),**无后端改动**,无 wire 改动。