# 前端补齐 task_state_transition 交互卡片

## Goal

补齐 `07-08-workflow-integration` Phase 3 遗留的前端缺口:后端已完整实现
workflow 状态转移的用户确认门(`request_task_state_transition` tool →
`task:state:transition:request` event → `resolve_task_state_transition` IPC),
但前端 `PendingInteraction` 类型只有 `question` / `mode_change` 两个变体,缺
第三条链路。

**后果**:agent 调 `request_task_state_transition` 时,后端 block 当前 turn
等 oneshot,但前端不渲染卡片、用户无处点允许/拒绝,turn 挂起 —— workflow
状态机的核心承诺("state 转移用户确认门,agent 不能自翻")在 UI 层失效。

## 背景

- 07-08-workflow-integration Phase 3 收官 review 发现:后端 TaskStateTransition
  链路(tool / QuestionStore 变体 / emit channel / resolve IPC / tool_result
  envelope)全部落地并测试通过,但前端 7 个接入点全部缺失。
- 对标既有 `mode_change`(request_mode_change)前端链路,二者是平行结构:
  同一个 `PendingInteraction` tagged union、同一个 `pendingBySession` map、
  同一个 `get_pending_interaction` IPC + `reconcilePendingInteractionFromBackend`
  reconciler。task_state_transition 只是 union 的第三个 arm + 各自的 per-flow
  wiring(card / resolve wrapper / store action / listener / dispatch)。

## 已确认事实(wire-shape,后端已存在)

| 事实 | 来源 |
|---|---|
| event channel = `task:state:transition:request`,payload 扁平无 kind | state.rs:706 |
| event payload 全 snake_case:`session_id/tool_use_id/target_state/current_state?/slug?/reason?/ts` | question_store.rs:220 |
| `PendingInteraction` enum 内部 tagged,`#[serde(tag="kind", rename_all="snake_case")]`,字段扁平在 kind 同级 | question_store.rs:290 |
| resolve IPC JS 传 camelCase:`{ sessionId, toolUseId, targetState, slug, allow }`,返回 SessionRow | question.rs:555 |
| resolve IPC 比 mode_change **多 `slug`** 参数(IPC handler 要定位 task.json,无 WorkflowCtx),**无 fromState**(后端读盘) | question.rs:555 |
| allowed envelope:`{"allowed":true,"prev_state":"...","new_state":"..."}`(对应 mode_change 的 prev_mode/new_mode) | request_task_state_transition.rs:598 |
| denied envelope:`{"cancelled_by_user":true,"target_state":"..."}`(比 mode_change 多 target_state) | request_task_state_transition.rs:610 |
| session-cancel envelope:`{"cancelled_by_session":true}` | request_task_state_transition.rs:578 |

## Requirements

### 功能需求

1. 类型层:`PendingInteraction` union + `PendingInteractionEntry.kind` 扩展第三
   臂 `task_state_transition`;新增 payload / cmd / event 常量
2. IPC wrapper:`resolveTaskStateTransition`(对标 `resolveModeChange`,多 slug)
3. Pinia action:`resolveTaskStateTransition`(比 mode_change 简单 — 不 patch
   session mode,只 IPC + removePending)
4. 事件监听:`task:state:transition:request` listener + handler + teardown
5. 卡片组件 `RequestTaskStateTransitionCard.vue`(对标 RequestModeChangeCard,
   精简掉 mode 专有的 Yolo 分支 / mode 颜色映射)
6. MessageItem dispatch:parse envelope + state resolver + props resolver + 模板
7. 卡片测试

### 非功能需求

- 后端零改动(已 100% 完成)
- `reconcilePendingInteractionFromBackend` / `get_pending_interaction` wrapper
  无需改(已处理任意 kind 的 tagged union)
- 中文文案:四态映射 planning=规划 / implement=实现 / check=校验 / done=完成

## Acceptance Criteria

- [ ] `PendingInteraction` union 含 `task_state_transition` arm;vue-tsc clean
- [ ] workflow session agent 调 `request_task_state_transition` → 卡片渲染
- [ ] 点允许 → resolve_task_state_transition IPC 带正确参数 → task.json 状态流转
      + turn 恢复 + 卡片转"已转换"态
- [ ] 点拒绝 → IPC allow=false → turn 按取消恢复 + 卡片转"已拒绝"态
- [ ] 跨 turn 重载(historical)→ 卡片从 tool_result envelope 还原 allowed/denied 态
- [ ] `pnpm test` 794 + 新增 case 全过,零回归

## Out of Scope

- ❌ 后端任何改动
- ❌ preflight_implement_check 真实实现(state.rs stub,Phase 4)
- ❌ workflow toggle / plugin 切换 UI(已落地)
- ❌ archive_task 的前端入口(另一交互设计)
