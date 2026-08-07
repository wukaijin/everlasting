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
