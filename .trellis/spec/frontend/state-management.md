# State Management

> How state is managed in this project.

---

## Overview

State management is Pinia-based (`defineStore` in `app/src/stores/`). The two
core stores are `chat.ts` (thin facade over the controller) and
`streamController.ts` (owns in-memory message buffers + SSE event handling).

---

## Stream Controller Pattern (2026-08-08, 08-07-large-file-splitting 沉淀)

`streamController.ts` is the single source of truth for in-memory messages
(reactive `Map<sessionId, ChatMessage[]>`), in-flight request state
(`activeRequests` / `completedRequests`), and the SSE event pipeline.

**Module layout** (拆分后):

- `streamController.ts` — store: state + internal helpers + public API + return block
- `streamEvents.ts` — event handling block (`createStreamEventHandlers(ctx)` factory)
- `streamRehydrate.ts` — `rehydrateMessages` 纯函数(DB 载荷 → 内存 ChatMessage)

**事件块拆分契约(必须遵守)**:事件处理函数与 store 状态/公共 API
互相引用(`handleChatEvent` → `start`/`ensureLoaded`,`refresh` →
`finalizeRequest`),因此:

- 事件块以 **工厂 + ctx 注入** 拆出:`createStreamEventHandlers(ctx)` 顶部
  一次解构 ctx,函数体原样保留(零逐函数改动)。
- ctx 在 store 的 helpers 之后**一次填充**:公共 API 全是提升的
  `function` 声明,可在定义前引用;state/helpers 是 const,须先定义。
- **return 块导出约束**:`handleChatEvent` / `handleToolCall` /
  `finalizeRequest` / `putMessages` / `pinnedSessions` / `loadedFromDb`
  是测试直调入口(`streamController.test.ts` /
  `streamController.review.test.ts`),拆模块后必须经 return 块
  re-export,不允许测试改从事件模块直接 import 内部符号。
- store 内调用事件函数一律走 `events.X`,return 块用
  `finalizeRequest: events.finalizeRequest` 形式保留原导出名。

---

## Chat Store Action Clusters Pattern (2026-08-10, 08-10-chat-store-split 沉淀)

`chat.ts` 是 Pinia **setup store**(`defineStore("chat", () => {...})`),
所有 action 是闭包内 `function` 声明、共享同一份 state(refs / computeds /
controller)。当 store 体量超 1200 行时,按职责簇拆为 `createXxxActions(ctx)`
工厂文件,**复用 streamEvents 的 factory + ctx 注入契约**(见上节)。

**Module layout** (拆分后,2026-08-10):

- `chat.ts` (~960 行) — hub:模块级 export(`resolveModelInput` /
  `parseForcedDispatchPrefix` / `thinkingBlocksToText`,被测试直接 import)
  + setup 头(state refs / computeds / project-change watcher)+
  `onProjectChange` + diff 簇 + `toPayloadContent` + `cancel` +
  4 个 `createXxx(ctx)` 调用 + return 块
- `chatSessionActions.ts` — 会话 CRUD + worktree(13 个 action,
  `createSessionActions(ctx)`)
- `chatModeActions.ts` — mode / yolo / workflow(2 个 ref + 6 个 action,
  `createModeActions(ctx)`)
- `chatMessageActions.ts` — edit / resend / retry(3 个 action,
  `createMessageActions(ctx)`)
- `chatSendActions.ts` — send(`createSendActions(ctx)`)

**拆分契约(必须遵守)**——与 streamEvents 同构,补充 chat 特有约束:

- **factory + ctx 注入**:每个簇 `createXxxActions(ctx)`,顶部一次解构 ctx,
  函数体原样保留(**零逐函数改动**,含 JSDoc)。
- **ctx 在 hub 的 state/helpers 之后一次填充**:action 全是提升的
  `function` 声明;state/helpers 是 const,须先定义。
- **循环依赖消除**:跨簇 action 经 ctx 互注(sessions 簇的
  `createNewSession` 注入 send 簇;send 簇留 hub 的 `cancel` 注入
  sessions/message/send 三簇)。`cancel`(5 行循环枢纽)留 hub——
  搬走会造成工厂初始化顺序的死锁。
- **模块级共享 helper 留 hub**:`toPayloadContent`(send/message 共用)+
  `genId` / `ContentBlockPayload` / `ChatMessagePayload` 加 `export`
  供簇 `import`(对标 streamController.ts `export genId` 先例)。
- **watch immediate 与工厂初始化时机**:hub 的 `watch(currentProjectId,
  ..., { immediate: true })` 在定义时同步触发回调,回调调工厂产物
  (`loadSessions`)。因此 sessions 工厂调用须在 watch 定义之前——
  `diffCache` 声明上移到 watch 前(它无依赖),让 sessions 工厂的 ctx
  在 watch 触发时已就绪。
- **return 块导出约束**:store proxy 入口(send/cancel/editMessage/
  resendMessage/retryChat/会话 CRUD/mode-workflow 全套)必须经 return 块
  re-export;三个 chat 测试文件(chat.test.ts / chatMode.test.ts /
  chatSend.test.ts)全经 `useChatStore().X` 调用,**拆分后零改动**。

---

## State Categories

<!-- Local state, global state, server state, URL state -->

(To be filled by the team)

---

## When to Use Global State

<!-- Criteria for promoting state to global -->

(To be filled by the team)

---

## Server State

<!-- How server data is cached and synchronized -->

(To be filled by the team)

---

## Common Mistakes

<!-- State management mistakes your team has made -->

(To be filled by the team)
