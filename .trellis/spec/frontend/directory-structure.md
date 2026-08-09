# Directory Structure

> How frontend code is organized in this project.

---

## Overview

<!--
Document your project's frontend directory structure here.

Questions to answer:
- Where do components live?
- How are features/modules organized?
- Where are shared utilities?
- How are assets organized?
-->

(To be filled by the team)

---

## Directory Layout

```
<!-- Replace with your actual structure -->
src/
├── ...
└── ...
```

---

## Module Organization

<!-- How should new features be organized? -->

(To be filled by the team)

---

## Naming Conventions

<!-- File and folder naming rules -->

(To be filled by the team)

---

## Examples

<!-- Link to well-organized modules as examples -->

(To be filled by the team)

---

## Large-File Splitting — 收官对照 (2026-08-10)

### 守则

- **行数目标**:源码文件目标 **< 1200 行**(与后端同规);超限按职责簇拆为
  内聚子模块(hub + 子文件),纯搬迁铁律——复制 + 删源 + 接线,**禁止顺手改
  逻辑**,每簇独立 commit、独立回滚(`git revert`),`pnpm test` 全绿是硬性
  验收。
- **总纲来源**:`08-07-large-file-splitting/prd.md`(统计日期 2026-08-07)
  是全仓 >1200 行文件拆分总纲;前端 `chat.ts` 原列 Out of Scope("薄门面"),
  后经五轮功能叠加长回 2156 行,由 `08-10-chat-store-split` 推翻判断落地。

### 收官状态表

| 总纲条目(拆分前行数) | 消化批次 | 现状(2026-08-10) |
|---|---|---|
| `stores/streamController.ts` 2683 | `08-07-large-file-splitting` | ✅ hub 1121 + streamEvents.ts 1200 + streamRehydrate.ts 433(事件块 factory+ctx 拆出) |
| `stores/chat.ts` 2156 | `08-10-chat-store-split` | ✅ hub 963(降 55%)+ chatSessionActions 366 + chatModeActions 421 + chatMessageActions 428 + chatSendActions 227;cancel 5 行循环枢纽留 hub |

### stores/ 目录簇文件登记

拆分契约详见 `state-management.md` §Stream Controller Pattern +
§Chat Store Action Clusters Pattern(factory + ctx 注入,函数体逐字搬迁)。

```
src/stores/
├── chat.ts                    # hub(setup store + 模块级 export + cancel/diff/toPayloadContent)
├── chat.types.ts              # 公共类型(ChatMessage / SessionSummary / ...)
├── chatSessionActions.ts      # createSessionActions(ctx) — 会话 CRUD + worktree(13 action)
├── chatModeActions.ts         # createModeActions(ctx) — mode/yolo/workflow(2 ref + 6 action)
├── chatMessageActions.ts      # createMessageActions(ctx) — edit/resend/retry(3 action)
├── chatSendActions.ts         # createSendActions(ctx) — send
├── streamController.ts        # stream store hub(state + public API + return block)
├── streamEvents.ts            # createStreamEventHandlers(ctx) — SSE 事件处理块
└── streamRehydrate.ts         # rehydrateMessages — DB 载荷 → 内存 ChatMessage
```
