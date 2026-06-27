# L3b PR4 前端 SubagentDrawer 合并/丢弃 UI

## Goal

在 `<SubagentDrawer>` 加 worker branch 可视化(branch 名 + worktree 状态:隔离中 / 已完成留 branch / 已 destroy)+ Merge / Discard 按钮,调 PR3 `merge_worker_run` / `discard_worker_run` Tauri command。完成 L3b 「PR 式产物」的可见/可控(用户在 drawer 里能 merge 或 discard,不再依赖 git CLI)。

**为什么**:PR3 提供 tool + Tauri command,但用户看不到分支状态 + 需要手动拼 Tauri invoke 命令。`<SubagentDrawer>` 是 subagent run 的中心 UI(B6 PR3b 已落地),PR4 在这里加 merge/discard 按钮让用户自然操作,跟 Claude Code 「keep-or-remove」UI 模式对齐。

## What I already know

### 前端现状

- **`<SubagentDrawer>`** 组件:`app/src/components/chat/SubagentDrawer/`(B6 PR3b 落地)。已经显示 worker transcript + status(`Completed | Cancelled | Error | Incomplete`)+ final_text。
- **`useSubagentRunsStore`** Pinia store:`app/src/stores/subagentRuns.ts` 主 store + `subagentRuns.types.ts` types + `runAccumulator.ts` accumulator。
- **IPC bridge**:`subagent:event` / `subagent:finished` 事件 + `get_subagent_run` / `list_subagent_runs_by_session` 查询命令。
- **`subagent_runs.worktree_path` 列** 已经能查询(PR1 schema,可通过 `get_subagent_run` 拿)。
- **PR3 提供 Tauri command** `merge_worker_run` / `discard_worker_run`(PR3 已落地)。

### 设计约束

- 按钮 visible 条件:**严格** `status === Completed && worktree_path !== null`(有 changes 保留 branch + worktree)。
- 按钮 disabled 条件:merge / discard 进行中(spinner 状态)。
- 冲突处理:PR3 merge 返 conflict 文件列表 → drawer 显示 conflict 列表(纯展示,提示用户去 git CLI 手动解决后重试)。
- 国际化:`app/i18n/zh-CN.json` + `en-US.json`(项目惯例)。

## Requirements

- SubagentDrawer 在 worker `status === Completed` 且 `worktree_path` 非空时,显示 branch 名 + Merge / Discard 按钮。
- Merge 按钮 → 调 `merge_worker_run` IPC,成功刷新 drawer(branch 消失,`subagent_runs.worktree_path` 变 NULL),失败显示 conflict 列表 + 错误 toast。
- Discard 按钮 → 调 `discard_worker_run` IPC,成功刷新 drawer(branch 消失)+ 成功 toast。
- 新增 worker branch 状态 badge:「隔离中(running)」/「已完成留 branch(completed + branch 保留)」/「已 destroy(cancelled/error/incomplete + worktree_path NULL)」(从 `status` + `worktree_path` 派生)。
- 并发安全:同一 run_id 的 merge / discard 按钮互斥(spinner 状态,防双击)。
- 无障碍:按钮 keyboard navigable + aria-label。

## Acceptance Criteria

- [ ] SubagentDrawer 显示 completed worker 的 branch 名(从 `subagent_runs.worktree_path` 派生,friendly name `Worker <run_id 短 hash>`)。
- [ ] 点 Merge → Tauri invoke `merge_worker_run` → 成功 drawer 刷新(worktree_path 变 NULL + 按钮消失)→ toast「已合并到 session 分支」。
- [ ] 点 Merge 冲突 → drawer 内联显示 conflict 文件列表(纯展示,无交互)+ 错误 toast「合并冲突,请到 git CLI 解决后重试」。
- [ ] 点 Discard → Tauri invoke `discard_worker_run` → drawer 刷新(worktree_path 变 NULL + 按钮消失)+ toast「已丢弃 worker branch」。
- [ ] merge / discard 进行中按钮 spinner + disabled,防双击。
- [ ] cancelled / error / incomplete worker 不显示 Merge/Discard 按钮。
- [ ] vitest 单测覆盖 button 显隐 + dispatch 逻辑 + reducer。
- [ ] `vue-tsc --noEmit` 0 err。

## Definition of Done

- `app/src/components/chat/SubagentDrawer/` 新增 / 修改组件:
  - 新增 branch 状态 badge 组件(derived from `status` + `worktree_path`)
  - 新增 Merge / Discard 按钮(visible 条件严格)
  - 冲突显示区(merge 失败时 inline 渲染 conflict 文件列表)
- `app/src/stores/subagentRuns.ts` 加 `mergeWorker(runId)` + `discardWorker(runId)` actions:
  - 内部 invoke Tauri command
  - 管理 per-run spinner 状态(避免全局 spinner 阻塞其他 drawer)
  - 成功后 dispatch store update(刷新该 run 的 `worktree_path = null`)
- `app/src/utils/messageFormat.ts` 或新 util 加 branch 名格式化(`worker/<run_id>` → friendly name)。
- 新增 vitest 测试(`app/src/components/chat/SubagentDrawer/SubagentDrawer.spec.ts` 或 .test.ts):
  - button visible when `status === Completed && worktree_path !== null`
  - button hidden when `status === Cancelled | Error | Incomplete`
  - merge success → store update + drawer refresh
  - merge conflict → conflict list display + error toast
  - merge / discard loading state → spinner + disabled
- 国际化:`app/i18n/zh-CN.json` + `en-US.json` 加按钮 label + toast 文案。
- `pnpm test` 全绿;`vue-tsc --noEmit` 0 err。
- spec 更新(`spec/frontend/chat.md` 加 SubagentDrawer merge/discard UI 契约 + branch 状态 badge 派生规则)。
- ROADMAP §1.2 L3b PR4 移到已实施;IMPLEMENTATION §4 加 ADR 决策日志。

## Out of Scope (explicit)

- 自动 conflict resolution(冲突文件列表 → 用户手动 git resolve + 重试)。
- 多 worker 批量 merge 按钮(只支持单 worker)。
- worker branch diff 实时预览(PR1 已经在 `<DiffView>` 支持,但本 PR 不接 SubagentDrawer 联动)。
- 用户主动取消 sweep(自动 sweep 启动时一次,UI 不暴露)。
- 跨 session worker merge(只同 session 内 merge 到 parent session 分支)。

## Decision (ADR-lite)

**Context**: L3b PR3 提供 merge/discard tool + Tauri command,但用户看不到 + 不能在 UI 自然操作。`<SubagentDrawer>` 是 subagent run 中心 UI,自然在这里加按钮。

**Decision**: 在 `<SubagentDrawer>` `status === Completed && worktree_path !== null` 分支,显示 Merge / Discard 按钮。冲突 → drawer 内联显示 conflict 列表(纯展示,引导用户去 git resolve 后重试)。

**Consequences**:
- 跟 Claude Code 「keep-or-remove」UI 模式对齐。
- 按钮 visible 条件严格,避免误操作(cancelled/error worker 不允许 merge/discard)。
- 冲突展示只是 hint,实际解决走 git CLI(MVP 简化,自动 resolution 是 NP-hard)。
- 跟 `<DiffView>` 联动是 follow-up(用户点 Merge 前想看 diff → 跳 SubagentDrawer 当前 transcript 段 + diff)。

## Implementation Plan

### 单 PR,前端为主 + 轻微 store / util 改动

1. **store actions**:`useSubagentRunsStore` 加 `mergeWorker(runId)` + `discardWorker(runId)`:
   - 内部 invoke Tauri command(PR3 提供)
   - 管理 per-run spinner 状态(避免全局 spinner 阻塞)
   - 成功后 dispatch store update(刷新该 run 的 `worktree_path = null`)
2. **SubagentDrawer 组件**:
   - 新增 branch 状态 badge 组件(从 `status` + `worktree_path` 派生,三态:隔离中 / 已完成留 branch / 已 destroy)
   - 新增 Merge / Discard 按钮(visible 条件严格)
   - 冲突显示区(merge 失败时 inline 渲染 conflict 文件列表)
3. **i18n**:按钮 label + toast 提示中文化(双语)。
4. **vitest 测试**:button 显隐 + dispatch mock + spinner state + conflict display。
5. **spec 更新**:`spec/frontend/chat.md` 加 SubagentDrawer merge/discard UI 契约 + branch 状态 badge 派生规则。

## Edge Cases

| 场景 | 默认决策 | 理由 |
|---|---|---|
| Tauri command 返 conflict 列表 | drawer 内联显示 conflict 文件,提示用户 git resolve 后重试 | MVP 简化,不自动 |
| Tauri command 失败(网络/超时) | error toast + 按钮恢复 enabled | 标准 error handling |
| 用户连续点 Merge 两次 | 第二次点时按钮 disabled(spinner 状态),不发第二个 invoke | 防双击 |
| worktree_path 已被 PR3 sweep 清掉 | 按钮不显示(visible 条件不满足) | PR3 sweep 副作用是 branch 消失 → worktree_path NULL |
| 多个 drawer 同时打开(不同 run) | 每个 drawer 独立管理自己的 spinner,互不影响 | 局部 state |
| 抽屉打开时 status 从 Completed 变 Cancelled(parent 后续操作触发) | 按钮消失 + 状态 badge 更新 | reactive store 触发 |
| 后端 PR3 tool 不存在(merge_worker IPC 404) | error toast「工具未实现,请更新后端」 | fail fast,提示用户 |

## Technical Notes

- 复用:`useSubagentRunsStore`(B6)/ `<SubagentDrawer>`(B6)/ Tauri invoke pattern(`@tauri-apps/api/core::invoke` + 类型化)。
- 不动:`run_chat_loop` / `dispatch_subagent` / PR1/2/3 后端。
- 跟 `<DiffView>` 联动是 follow-up(本 PR 只加按钮 + 调 IPC,不接 diff 实时预览)。
- 国际化:项目惯例双语,中文优先(`zh-CN.json` + `en-US.json`),按钮 label 短词 + toast 文案完整。
- Vue 3 `<script setup>` + Pinia 风格(B6 + B12 + L3d 已示范)。
- 测试:`vitest` + `@vue/test-utils`(项目已配置)。
- 完整原始 L3b PRD + research:`.trellis/tasks/archive/2026-06/06-27-l3b-worktree-delegate/prd.md` + `research/subagent-worktree-isolation-patterns.md`