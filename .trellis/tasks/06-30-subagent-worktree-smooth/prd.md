# sub-agent worktree 链路顺滑化

## Goal

让主 agent ↔ sub-agent 之间的 worktree 协作链路「顺滑」：日常派一个 sub-agent 改代码时**零 merge、零心智负担**；确实需要隔离（并发多 worker / 可丢弃改动）时，隔离链路**真正可用、可感知、可回主分支**。

## Background（现状与摩擦根因）

当前两层 worktree 嵌套：

    main（项目主仓库）
     └ session/<id> worktree   主 agent attach 后；merge 时 lazy auto-attach（06-30）
        └ worker/<run_id> worktree   isolation=true 时，每次 dispatch 一个

三个摩擦点（对照代码）：

1. **sub-agent 意识不到在 worktree** — dispatch 构造 delegation_task 时完全不注入隔离上下文（grep `worktree/branch/commit` 在 delegation 段 0 命中）。
2. **merge 假成功（最严重，bug 级）** — `probe_worker_changes`（`dispatch.rs:147`）看 **working tree**（未 commit 也能检测 → 保留 worktree）；但 `do_merge_blocking`（`merge_worker.rs:449/461`）只认 **branch tip commit**。sub-agent 不 commit → `worker_tip == parent_tip` → `is_ancestor` 命中 `==` 短路（`merge_worker.rs:651`）→ 走 FF → `return Ok("merged fast-forward")`，**改动一行也没合过去**。
3. **主 agent 汇入实际分支困难** — session worktree 是额外隔离层，worker→session→main 三层 merge。

**摩擦源**：只有 `general-purpose` 默认 `isolation: Some(true)`（`mod.rs:410`），而它正是日常「派去改代码」的 agent → 每次都进 isolated 链路 → 每次触发假成功。`researcher` 是 `None`（只读不隔离）。

## 决策（已与 Carlos 对齐）

- **A** — dispatch 结束 `probe` 检测到改动 → 系统**兜底** `git add -A && commit`（msg 含 `worker/<run_id>`），让 merge 真生效。**不依赖 sub-agent 自提交。**
- **B** — isolation 改由**系统层按 serial/parallel 自动决定**：单 dispatch（serial）默认 shared；同 turn 多 dispatch（parallel）系统强制 isolated。主 agent **无需手动指定**参数；**tool description 提醒主 agent 该机制**（鼓励独立子任务 fan-out、单 dispatch 改动立即可见）。
- **C** — dispatch 时往 delegation_task 注入 worktree 知情提示（改动会被自动 commit + merge 回主 agent），仅知情、不要求自提交。
- **D** — 新增 publish session→main（Tauri command + 前端按钮），本地 merge，冲突显式报错，**不 push remote**。

## Task Map

| Child | 范围 | 顺序 |
|---|---|---|
| `06-30-ab-autocommit-shared-default` | A（auto-commit 兜底）+ B（general-purpose 降级 shared） | 先 |
| `06-30-cd-isolated-hint-publish-ui` | C（注入提示）+ D（publish session→main UI） | 后（C 文案依赖 A） |

父任务不直接实现，只统管 source 需求、task map、cross-child AC 与最终集成验收。

## Cross-child Acceptance Criteria

- [ ] **单 dispatch（serial）** → 默认 shared，无 worker worktree 创建，改动立即可见，零 merge；**同 turn 多 dispatch（parallel）** → 系统自动隔离到各自 worktree，并发写不 race（child1 B）
- [ ] 显式 `isolation: worktree` 时，worker 改动（即使 sub-agent 未 commit）能被 `merge_worker` **真正合并**回 parent，不再假成功（child1 A）
- [ ] isolated sub-agent **知道**自己在 worktree、改动会被自动固化并 merge（child2 C）
- [ ] session worktree 的改动可通过前端**一键 publish 到 main**，冲突显式报错（child2 D）
- [ ] 现有 subagent / worktree / merge 相关测试全绿，无回归

## Out of Scope

- push remote（D 仅本地 merge）
- shared 模式下自动升级隔离（约定串行写，靠 prompt + tool description 约束）
- sub-agent 自提交语义（统一由系统兜底 auto-commit）
