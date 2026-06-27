# L3b PR3 merge_worker / discard_worker tool + sweep

## Goal

提供 `merge_worker` + `discard_worker` 两个新 builtin tool + sweep 机制,让用户/parent agent 在 PR1 保留的 `worker/<run_id>` branch 上做合并或丢弃决定。完成 L3a → L3b 承诺的「PR 式产物」半部分(创建在 PR1,合并/丢弃在 PR3)。

**为什么**:PR1 worker 完成后,有 changes 保留 branch `worker/<run_id>` + worktree 路径写进 `subagent_runs.worktree_path`。但用户/parent 没有工具处理这些 branch,会无限堆积(泄漏磁盘 + branch list 污染)。PR3 提供 (a) `merge_worker` 合并到 parent session 分支 + (b) `discard_worker` 销毁 + (c) sweep 清理过期残留。

## What I already know

### 代码现状(PR1 已就位,commit `862caf6`)

- **`subagent_runs.worktree_path` 列**(PR1 加)+ row 在 `run_subagent` 完成后保留(有 changes)或 NULL(无 changes 已 destroy)。
- **`worker/<run_id>` branch** + worktree dir 保留(PR1)。
- **`git/worktree.rs::destroy_worker`** 已实现(PR1,含 unlock + prune + branch delete)。
- **`git/diff.rs::diff_worker_worktree`** 已实现(PR1,内部用 `worker/<run_id>` branch 比较)。
- **parent session worktree** 的 HEAD 在 `loaded_session.session.worktree_path`(`chat_loop.rs:362-366`)。
- **Tauri command 模式**:`commands/worktree.rs::attach_worktree` / `detach_worktree` / `delete_worktree` 已就位(session 级)。

### 工具集位置

- builtin tool 注册:`tools/mod.rs::builtin_tools()`,PR1 已经把 `dispatch_subagent` 加进去。
- tool 执行:每个 tool 一个 `app/src-tauri/src/tools/<name>.rs` 文件,`mod.rs` 静态分发或 `tools/definition_with_cache` 动态。
- worker 嵌套深度 1(`STRUCTURALLY_DISABLED` 锁死 `dispatch_subagent`,PR1/2 已确认)。

## Requirements

- 新增 `merge_worker(run_id: str)` tool:把 `worker/<run_id>` merge 到 parent session 分支 `session/<id>`。冲突 → fail,返 conflict 文件列表,让用户手动。
- 新增 `discard_worker(run_id: str)` tool:销毁 worker worktree + 删 `worker/<run_id>` branch。
- Tauri command `merge_worker_run` / `discard_worker_run` IPC bridge(对齐 session worktree 已有的 `attach_worktree` 等命令)。
- sweep 机制:`cleanupPeriodDays` 等价,按 mtime 扫描清理过期 `worker/*` branch + 残留 worktree。复用 PR1 self-heal。
- sweep 触发:启动时调一次(轻量,跟 PR1 `attach_worktree` 启动检查模式一致)。
- 前端 IPC handler(本 PR 不做 UI,PR4 做 SubagentDrawer 按钮)。

## Acceptance Criteria

- [ ] `merge_worker(run_id)` → `worker/<run_id>` branch commits 落到 `session/<id>` branch(快进或三向 merge)。
- [ ] `merge_worker` 冲突 → 返回 conflict 文件列表 + error tool_result,**不破坏**任何 branch。
- [ ] `merge_worker` 成功后,worker worktree + branch 销毁,`subagent_runs.worktree_path` 置 NULL。
- [ ] `discard_worker(run_id)` → 销毁 worker worktree + 删 branch + `subagent_runs.worktree_path` 置 NULL。
- [ ] sweep 机制:扫描 `<app_data_dir>/worktrees/<project_uuid>/worker/`,mtime 超过 N 天(`cleanup_period_days` env,默认 7) → 销毁。
- [ ] sweep 跑期间 `git worktree lock` 保护正在跑的 worker(worktree 被锁的不动)。
- [ ] 并发 N worker merge(parent 分支一次接一个),避免连锁 conflict —— 通过 merge_worker tool 内部互斥锁或序列化(选最简方案)。
- [ ] `merge_worker` / `discard_worker` 走 ⑨ 关 Tier 4 shell-like 权限检查(对齐 `shell` tool 权限模式)。
- [ ] 错误路径:`merge_worker` 目标 branch 不存在(parent session 未 attach worktree) → error tool_result "parent session has no worktree"。
- [ ] 幂等 follow-up:`discard_worker` 已 destroy 的 run_id → error tool_result "worker already destroyed"(MVP 不做幂等)。

## Definition of Done

- 新增 `app/src-tauri/src/tools/merge_worker.rs`(~80-120 行,libgit2 `merge_commits` + `merge` + conflict scan)+ `app/src-tauri/src/tools/discard_worker.rs`(~30-50 行,调 PR1 `destroy_worker`)。
- `tools/mod.rs::builtin_tools()` 注册两个新 tool,加入 ⑨ 关 Tier 4 权限白名单(shell-like)。
- `commands/subagent_runs.rs` 加 `merge_worker_run` / `discard_worker_run` IPC。
- `git/worktree.rs` 加 `sweep_stale_worker_worktrees(app_data_dir, project_id, cleanup_period_days)` 函数(libgit2 扫 worktrees + 过滤 mtime + 调 `destroy_worker` 跳过 locked)。
- 启动 sweep:`app.rs::setup` 或 `lib.rs` 启动路径加 sweep 调用(轻量,启动时一次)。
- 新增测试:
  - `agent::tests_subagent.rs::l3b_merge_worker_*`(happy / conflict / no parent worktree)
  - `agent::tests_subagent.rs::l3b_discard_worker_*`(happy / already destroyed)
  - `git::worktree::tests::sweep_stale_*`(过期清理 / 跳过 locked / env var 默认)
- `cargo test --lib` 全绿。
- spec 更新(`tool-contract.md` 加 merge_worker / discard_worker scenario + `worktree-contract.md` 加 sweep 契约)。
- ROADMAP §1.2 L3b PR3 移到已实施;IMPLEMENTATION §4 加 ADR 决策日志。

## Out of Scope (explicit)

- 前端 SubagentDrawer merge/discard UI 按钮(PR4 做)。
- 自动 conflict resolution(冲突文件列表 → 用户手动 git resolve,MVP 简化)。
- background async dispatch(daemon 化阶段做)。
- worker 嵌套深度 > 1。
- worktree pool / 复用(业界 0 采用,见 L3b research)。
- merge_worker 幂等(重复调返 OK,需要额外 state 跟踪,MVP 简化 fail-fast)。

## Decision (ADR-lite)

**Context**: L3b PR1 保留有 changes 的 worker branch + worktree,但没有工具处理这些产物。无限堆积会泄漏磁盘 + branch list 污染。Claude Code 模式是 sweep + 用户 keep-or-remove 提示。

**Decision**: 走 `merge_worker` + `discard_worker` tool + sweep 三件套。**sweep 默认 7 天**对齐 Claude Code `cleanupPeriodDays` 默认。冲突 → fail 让用户手动(MVP 不做自动 resolution)。并发 N worker merge 串行(parent 分支一次接一个,避免连锁 conflict)。

**Consequences**:
- 跟 Claude Code 工业级范本对齐,harness 学习价值。
- sweep 加复杂度但 PR1 self-heal 已就位可复用。
- 并发 merge 串行化保证确定性。
- 冲突 fail 让用户手动是 MVP 简化(自动 resolution 是 NP-hard + 多种 strategy,不在 L3b scope)。

## Implementation Plan

### 单 PR,serialize 三块(可拆 PR3a/3b/3c,如果体积过大)

1. **PR3a(tool + IPC)**:tool 实现 + builtin 注册 + Tauri command
   - `merge_worker.rs` + `discard_worker.rs`(~150 行 total)
   - `commands/subagent_runs.rs` 两个 IPC
   - `tools/mod.rs::builtin_tools()` 注册 + ⑨ Tier 4 权限
   - 新增 3-4 个 vitest 级别 cargo 测试
2. **PR3b(sweep)**:sweep 机制
   - `git::worktree::sweep_stale_worker_worktrees` 函数
   - 启动时 sweep 调用接入
   - 2-3 个 sweep 测试(过期 / locked / env)
3. **PR3c(spec,可选单独 PR)**:tool-contract.md + worktree-contract.md 更新

> PR3a + PR3b 合并 1 PR 也行,总代码量估计 ~250-350 行,不算大。

## Edge Cases

| 场景 | 默认决策 | 理由 |
|---|---|---|
| merge 冲突 | fail,返 conflict 文件列表 | MVP 不做自动 resolution |
| 并发 N merge(同一 session 多 worker) | 串行(merge_worker tool 内部互斥,或 `session_tool_permissions` 互斥锁) | 避免连锁 conflict |
| sweep 撞上 worker 正在跑 | `git worktree lock` 跳过(worktree 被锁的不动) | PR1 lock 机制保护 |
| merge 目标 branch 不存在(parent session 未 attach worktree) | error tool_result "parent session has no worktree" | fail fast |
| discard 已 destroy 的 run_id | error tool_result "worker already destroyed" | fail fast,幂等 follow-up |
| sweep env `cleanup_period_days` 未设置 | 默认 7 天 | 对齐 Claude Code |
| sweep 跨多 project | 每个 project 独立扫(按 `project_uuid` 子目录) | 隔离清晰 |
| sweep 中途 crash | best-effort `destroy_worker`(PR1 已实现)+ `tracing::warn!` 继续 | 不阻塞其他项目清理 |

## Technical Notes

- 复用:`git::worktree::create_worker` / `destroy_worker`(PR1)/ `diff_worker_worktree`(PR1)/ `subagent_runs::get_run` / `update_run_finished`。
- libgit2 merge API:
  - `Repository::merge_commits(our_commit, their_commit, ...)` 返回 `(MergePreference, AnnotatedCommit)`
  - `Repository::merge(their_annotated, merge_opts, ...)` 应用 merge
  - 冲突检测:`MergeOptions::file_favor` + `MergeOptions::tree_favor` 配置 + 解析 merge 结果的 tree 找 conflict marker
- libgit2 sweep:`Repository::worktrees()` 拿所有 worktree metadata,过滤 name 前缀 `worker/`,读 mtime(`.git/worktrees/<name>/locked` + `.git/worktrees/<name>/commondir` 文件 mtime),过期 → 调 `destroy_worker`。
- 不变式:`destroy_worker` 已经在 PR1 包含 unlock + prune + branch delete,直接调。
- WSL/git2-rs:无 libgit2 `worktree remove`,destroy 走 `remove_dir_all` + `Worktree::prune` + `Branch::delete`(现有方案)。
- 完整原始 L3b PRD + research:`.trellis/tasks/archive/2026-06/06-27-l3b-worktree-delegate/prd.md` + `research/subagent-worktree-isolation-patterns.md`