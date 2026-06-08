# 修复: in-memory placeholder 累积破坏 wire 序列 → 2013 + diff chip 缓存陈旧

## Goal

修 06-08 上 c35c384 之后**仍然**复现的 2013 "tool call result does not follow tool call"，并顺手修 diff chip 在 commit / edit_file 后**不**实时更新的缓存问题。两者根因都在 `streamController` 的 in-memory 状态管理——同一处修。

## Requirements

### R1: bug 1 (2013 重新出现)
- **场景**：session 第一次 send 含 tool_use，**in-memory** placeholder 累积 toolCalls + toolResults + multi-turn text。**DB** 实际是每个 turn 单独一条 assistant message。第二次 send 时 `ensureLoaded` 命中 in-memory 缓存（**不**走 rehydrate from DB）→ history 包含 assistant turn with tool_use 但**后面** user message **没有** tool_result → Anthropic API 2013。
- **根因**：`streamController.finalizeRequest` send 完成后**不** evict in-memory。placeholder 累积形态只活在当次 send 期间是 OK 的，但 send 完**没**清，下一次 send 拿到的就是错的。
- **修法**：在 `finalizeRequest` 里调 `evict(sessionId)`，下次 `ensureLoaded` 自动从 DB 重新 load（`evict` 只是清 cache + 标记 loadedFromDb false，**不**主动 reload——下次 `ensureLoaded` 走 `getMessages` 命中失败 + 走 `invoke("load_session", ...)` + 走 `rehydrateMessages`）。in-memory 跟 DB 形态永远同步，placeholder 累积形态只活在当次 send 期间。

### R2: bug 4 (diff chip 缓存陈旧)
- **场景**：图里 chip 显示 `diff (1)`，但 commit 之后 worktree 应是 clean 的，chip 应该消失（或 0）。`chat.ts:481-493` `fetchDiff` 缓存到 `diffCache` Map，**只在 `deleteSession` / `attachWorktree` / `detachWorktree` / `deleteWorktree` 时清**。`git commit` / `edit_file` / `shell` 完成后**不**触发 invalidate。
- **修法**：跟 R1 同一位置——`streamController.finalizeRequest` 里调 `chatStore.invalidateDiff(sessionId)`（新增 Pinia action），下次 chip 读 cache miss → `fetchDiff` 重新查后端。

## Acceptance Criteria

- AC-1: 复现 2013 流程（`456ca8b8` 或 `9e8a78fe` session 同款：attach worktree → 发一句 user msg → 调 1 个 tool → LLM 返回 text → 紧接着发第二句）**不再** 2013。
- AC-2: vitest 锁住不变量：`finalizeRequest` 后 in-memory 缓存**必须**清空（`messagesBySession.has(sessionId)` 为 false；`loadedFromDb.has(sessionId)` 为 false）。
- AC-3: vitest 锁住不变量：`finalizeRequest` 后 `chatStore.diffCache.has(sessionId)` 为 false。
- AC-4: manual e2e：commit 之后 1 秒内 diff chip 数字更新（chip 显示 `diff (clean)` 或消失）。
- AC-5: manual e2e：attach → cancel mid-tool_use（上次 c35c384 修的场景）**仍然**不 2013。
- AC-6: cargo test + pnpm test (vitest) + pnpm build + vue-tsc --noEmit 0 error 0 warning。
- AC-7: 写一行 LlmError invalid_params 的 2013 文本到 `docs/HACKING-llm.md` "陷阱 4"（同 陷阱 1/2/3 风格）：in-memory 累积 vs DB 拆分，附修法。
- AC-8: 单 commit squash，commit message 风格跟 c35c384 一致。

## Definition of Done

- AC-1 ~ AC-8 全部 ✓
- cargo test 全过（含 197 旧 + 新增的 2013 wire invariant test）
- pnpm test (vitest) 全过（含 52 旧 + 新增的 evict + invalidateDiff test）
- pnpm build (vue-tsc --noEmit + vite build) 全过
- 单 commit (squash)，commit message 风格：开头 `fix: 2013 reappears from in-memory placeholder accumulation (cache stale on send done)`，bullet 列表
- archive 当前 task 到 `.trellis/tasks/archive/2026-06/06-08-step-4-followup-2013-reappears-fix/`
- `.trellis/workspace/Carlos/journal-1.md` 记录本次修复

## Technical Approach

**核心改动**（一行级）在 `streamController.ts` 的 `finalizeRequest`（行 524-527）：

```ts
function finalizeRequest(requestId: string, sessionId: string, _errored: boolean): void {
  activeRequests.delete(requestId);
  pinnedSessions.delete(sessionId);
  // BUG FIX (06-08-06-08): evict in-memory cache so next ensureLoaded
  // re-loads from DB. The placeholder accumulation shape (single
  // ChatMessage with toolCalls + toolResults + multi-turn text from
  // streaming events) does not match the DB's actual shape (one
  // assistant message per turn). If we keep the cache, the next
  // `send()` builds a history where assistant(tool_use) is followed
  // by user(text) without a tool_result in between — Anthropic API
  // returns 2013. evict + invalidateDiff is the cheapest path that
  // keeps in-memory state in sync with DB.
  controller.evict(sessionId);
  chatStore.invalidateDiff(sessionId);
}
```

**配套改动**：
- `chat.ts` 新增 `invalidateDiff(sessionId)` action：`diffCache.value.delete(sessionId)`。注：`chatStore` 在 `streamController` 里**没** import——但 streamController 的 finalizeRequest 在 `useStreamControllerStore` 闭包内，**无法**直接调 `chatStore` action。**需要**通过 listener 事件或者在 streamController 里直接 import `useChatStore` 并 `const chatStore = useChatStore()`——但这有 Pinia store 互相依赖问题。**更干净**的做法是：在 `streamController` 里直接动 `diffCache` Map（diffCache 是 `chat.ts` 的 state，**不**在 `streamController` 里）。**最优**做法：**让 `streamController` 不直接管 `diffCache`**，而是**新发**一个 Pinia event 出去，由 chat store listener 收到后 invalidate。

  **等等**——更简单：**`streamController` 已经有 `done` event handler**（行 455-475）。**在 done event handler 里**也调 `controller.evict(sessionId)` + `chatStore.invalidateDiff(sessionId)`。但 chat store 在 streamController 内部**不**直接可用——需要 import。**或者**通过事件总线 / Pinia action 触发。

  **最简方案**：`streamController.ts` 直接 import `useChatStore` 并在 `finalizeRequest` 调 `useChatStore().invalidateDiff(sessionId)`。Pinia 允许这种 import。

- `chat.ts` 新增 `invalidateDiff(sessionId: string)` action：
  ```ts
  function invalidateDiff(sessionId: string): void {
    diffCache.value.delete(sessionId);
  }
  ```

- `vitest` 测试锁住不变量：模拟 send → 等 done → 检查 in-memory 清空。

## Decision (ADR-lite)

**Context**: 06-08 上 c35c384 修的 cancel 路径"tool_use 孤儿 → 2013"是同一错误码的不同根因（cancel 路径没写 tool_result）。本次复现的 2013 **不是** cancel 路径，是正常完成路径。两次 session (`456ca8b8` 21 条 messages + `9e8a78fe` 7 条 messages) 的 DB 序列**全部** tool_use ↔ tool_result 配对正确，但 wire 上的 history 因为 in-memory 形态跟 DB 不一致**破坏**了配对。

**Decision**:
1. **bug 1 修法**：`streamController.finalizeRequest` 调 `controller.evict(sessionId)`——in-memory 跟 DB 永远同步。
2. **bug 4 修法**：同位置调 `useChatStore().invalidateDiff(sessionId)`——diff chip 在 send 完成后立即失效。
3. **commit**：单 commit squash（跟 c35c384 / 6f3d557 风格一致）。

**Consequences**:
- 2013 在**任何** multi-turn session 的第二次 send 都不再出（因为 in-memory 永远在 send 完清）
- diff chip 在 commit / edit_file / shell 完成后**立即**反映真实状态
- 性能 cost：每次 send 完成都 evict，下次 ensureLoaded 走 IPC re-load。多 1 次 DB load / 0.1-1ms IPC。**实测**：session 切换回来时本来就要 re-load（LRU eviction），现在只是更早 evict。
- bug 2 (+3/-3) **不**在本任务修——拆 follow-up task。bug 3 维持现状。
- 不再依赖"attach_worktree refresh" 这种偶发 trigger 来"碰巧"让 in-memory 同步——变成**显式** invariant。

## Out of Scope

- bug 2 (+3/-3 数字) — 拆独立 follow-up task，先看 `tools/edit_file.rs` 是否 read + write_file 整文件重写（如果是，那 line_stats 是正确的，需要改的是 edit_file 实现 / DiffView 文案）
- bug 3 (diff 按钮解耦) — 维持现状，**不**引入 "project root diff" 新概念
- 跨 session 的 tool_use id 重用（理论概率 0）
- 把 placeholder 累积形态**重构**成"每 turn 独立 ChatMessage"（架构大改，本任务**不**做）
- 给 placeholder 加一个 `multiTurnText` 字段（不优雅，污染 ChatMessage shape）

## Technical Notes

### 根因复盘

`streamController.handleToolCall` / `handleToolResult` 把 tool_call / tool_result 累积到 `last = msgs[msgs.length-1] = assistantMsg placeholder`（行 494-518）。`handleChatEvent` for `delta` 把 text 累积到**同一个** placeholder（行 440-442）。后端 agent loop 每个 turn 单独 persist 一个 assistant message 到 DB（`lib.rs:1413-1424`），**不**是累积到一条。

`toPayloadContent` for `assistant` role 按 Anthropic 协议**不**发出 `m.toolResults`（`chat.ts:519-528`）——**只**发 `tool_use` 块。所以 in-memory placeholder 累积形态的 toolResults **不上 wire**。

wire 序列：assistant(text + tool_use) → 后面 user(text) **没有** tool_result → Anthropic API 2013。

### 验证证据

- 旧 `456ca8b8` session 21 条 messages：DB 序列 22 条 tool_use ↔ tool_result 配对正确，send 5 ("再试试 git diff") 触发 2013
- 新 `9e8a78fe` session 7 条 messages：DB 序列 8 条（含 1 个 tool_use ↔ tool_result），send 3 ("帮我随便改下 README.md") 触发 2013
- 旧 session send 1 ("看一下") 之后 attach worktree 触发 `controller.refresh` → in-memory 同步 DB → send 2/3/4 期间 2013 没出

### 关键文件位置

- 修法位置：`app/src/stores/streamController.ts:524-527`（`finalizeRequest`）
- 配套新增：`app/src/stores/chat.ts`（`invalidateDiff` action）
- vitest：`app/src/stores/streamController.test.ts`（已有 8 个 rehydrateMessages 测试，新增 finalizeRequest evict 测试）
- spec / 文档：
  - `docs/HACKING-llm.md` "陷阱 4"（追加 2013 新场景，跟陷阱 1/2/3 风格）
  - `.trellis/spec/frontend/state-management.md`（追加 in-memory 形态 vs DB 形态 invariant）

### 上次 fix (c35c384) 的局限

c35c384 修的是「cancel 路径不写 tool_result」+ 「rehydrate 治历史 orphan」。**没**碰到"正常完成路径下 in-memory 累积形态"这条路径。本任务补这个洞。
