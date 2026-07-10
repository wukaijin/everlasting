# Implement — 前端补齐 task_state_transition 交互卡片

> 配套 `prd.md`。对标 `07-07-07-07-request-mode-change-tool` 的前端链路 1:1。

## 实施状态(2026-07-09)

| Step | 状态 | 说明 |
|---|---|---|
| Step 1 — 类型层 questionCards.types.ts | ✅ | union 第三臂 + payload/cmd 常量 |
| Step 2 — IPC wrapper toolTaskStateTransition.ts | ✅ | 新建,多 slug 参数 |
| Step 3 — Pinia action resolveTaskStateTransition | ✅ | 比 mode_change 简单(不 patch mode) |
| Step 4 — 事件监听 streamController.ts | ✅ | listener + handler + teardown |
| Step 5 — 新组件 RequestTaskStateTransitionCard.vue | ✅ | 精简掉 Yolo + 颜色映射 |
| Step 6 — MessageItem.vue dispatch | ✅ | parse envelope + resolver + 模板 |
| Step 7 — 测试 | ✅ | 18 case(含 wire payload 断言) |
| 验证 | ✅ | pnpm test 812 pass(794+18);vue-tsc clean |

提交:`969466b feat(workflow): 前端 task_state_transition 交互卡片`

## wire-shape 要点(与 mode_change 的差异)

1. **resolve IPC 多 `slug` 参数** — IPC handler 无 WorkflowCtx,要自己定位
   `<project>/.everlasting/tasks/<slug>/task.json` 读当前 `from` state。无
   `fromState`(后端读盘)。
2. **allowed envelope**: `{"allowed":true,"prev_state":"...","new_state":"..."}`
   (对应 mode_change 的 prev_mode/new_mode)。
3. **denied envelope 多 `target_state`**: `{"cancelled_by_user":true,"target_state":"..."}`
   (mode_change 无 target_mode)。
4. **store action 不 patch session mode** — workflow 状态转移不改 edit/plan/yolo。

## 7 接入点清单

| # | 文件 | 改动 |
|---|---|---|
| 1 | `stores/questionCards.types.ts` | union 第三臂 + WorkflowState 联合 + payload/cmd 常量 |
| 2 | `utils/toolTaskStateTransition.ts` | 新建,resolveTaskStateTransition wrapper |
| 3 | `stores/questionCards.ts` | resolveTaskStateTransition action |
| 4 | `stores/streamController.ts` | unlistenTST + handleTaskStateTransition + listen + teardown |
| 5 | `components/chat/RequestTaskStateTransitionCard.vue` | 新组件 |
| 6 | `components/chat/MessageItem.vue` | parse envelope + 3 resolver + 模板第四兄弟 |
| 7 | `components/chat/RequestTaskStateTransitionCard.test.ts` | 18 case |

**无需改**:`reconcilePendingInteractionFromBackend`(已处理任意 kind 的
tagged union)+ `getPendingInteraction` wrapper(返回 union,新变体自动兼容)。

## 验证

```bash
cd app
pnpm test              # 812 passed (794 + 18 new), 零回归
npx vue-tsc --noEmit   # exit 0, clean
```

## Out of Scope(留给后续)

- 手动端到端验证(需起 dev server + workflow session 触发真实状态转移)
- archive_task 的前端入口(后端 IPC 在,何时调属另一交互设计)
