# C+D: isolated 上下文注入 + publish session→main UI

> Parent: `06-30-subagent-worktree-smooth`

## Goal

让「仍然选择隔离」的场景**可感知**（C：sub-agent 知道自己在 worktree）且**可回主分支**（D：session→main 一键 publish）。

## Dependencies

- 建议在 child1（A+B）**之后**做：C 的提示文案提到「改动会被自动 commit + merge 回主 agent」，依赖 A 的 auto-commit 兜底存在，否则文案失真、甚至误导。D 独立，可并行起步但归在本任务交付。

## Requirements

### C — delegation_task 注入 worktree 知情提示
- **仅当 `isolated=true`** 时，在 dispatch 构造的 delegation_task 中追加一段提示：告知 sub-agent 它运行在 `worker/<run_id>` 隔离 worktree、改动会被系统**自动固化成 commit**、主 agent 会 merge 回去；提示其聚焦任务本身、无需自行 commit。
- **非隔离（shared）不注入**（无需知情）。
- 提示**不要求** sub-agent 自己 commit（commit 由 A 的系统兜底负责）。

### D — publish session→main
- 新增 Tauri command（如 `publish_session_to_main`）：把当前 session 的 `session/<id>` 分支 merge 到 `main`（**本地**）。
- 复用 / 对齐现有 `do_merge_blocking` 风格（FF 优先，3-way 兜底，冲突显式报错、不自动合、不留半合并脏状态）。
- 前端新增入口（按钮 / 菜单），点击后调用该 command，成功后刷新状态 + toast；冲突时 toast 显示冲突文件，引导手动解决。
- **不 push remote。**

## Acceptance Criteria

- [ ] isolated sub-agent 收到的任务 prompt 中**包含** worktree 知情提示（测试断言注入）；shared sub-agent **不包含**
- [ ] 前端有 publish session→main 入口，点击后 session 分支的改动合入 main 本地工作区
- [ ] merge 冲突时显式报错（含冲突文件列表），不产生半合并脏状态
- [ ] 全程**不触发** git push
- [ ] 新增 command 有对应前端 invoke + 状态刷新；不破坏现有 worktree / merge UI

## Out of Scope

- push remote / 创建 PR
- 多 session 批量 publish
- 自动解决冲突

## Notes

- D 的 merge 目标是 `main` **本地**分支；session worktree 与 main 的关系、与 lazy auto-attach（`ensure_parent_worktree_attached`）的交互在 design.md 梳理。
