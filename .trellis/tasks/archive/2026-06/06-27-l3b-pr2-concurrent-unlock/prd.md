# L3b PR2 concurrent dispatch 解锁 worker worktree

## Goal

把 L3a concurrent dispatch 的 `force_readonly=true` 闸门换成 **每个 worker 各自 worktree**(沿用 PR1 的 `worker/<run_id>` 隔离基建),让 `general-purpose` worker 并发写不冲突,真正兑现 L3a → L3b 承诺的并发写能力。

**为什么**:L3a `force_readonly` 是用「并发写消解」换「并发读」的下策 —— `general-purpose` worker 在并发批(N ≥ 2)里被锁死成只读,丧失写能力。L3b PR1 已经提供 per-worker worktree 隔离基建(L3a `force_readonly` 的根因是「没 worktree 隔离就会写冲突」),PR2 只需把并发分支接上 PR1 的隔离基建,就能解锁并发写。

## What I already know

### 代码现状(PR1 已就位,commit `862caf6`)

- **`agent/subagent/dispatch.rs::run_subagent`** 收 `force_readonly: bool`(L3a 第 19 参,`dispatch.rs:120`)。`force_readonly=true` 时,`filter_tools_readonly` 把 toolset 剥到 `READONLY_TOOL_ALLOWLIST` 5 个 read-only tool(`dispatch.rs:195`)。
- **`chat_loop.rs` concurrent 分支**(L3a 实现):`FuturesUnordered` 跑 N 个 dispatch,传 `force_readonly=true`(`chat_loop.rs:~1788`)。serial 分支(单 dispatch / 混合批)传 `false`。
- **PR1 隔离基建**:
  - `SubagentDef.isolation: Option<bool>` 字段(`mod.rs:332`)+ builtin default(general-purpose `Some(true)`,researcher `None`)。
  - `dispatch_subagent` tool `isolation: Option<bool>` 入参 + `resolve_isolation(frontmatter, dispatch) -> bool` 真值表(`dispatch.rs:108-115`)。
  - `run_subagent` 隔离分支:建 worker worktree(create_worker + lock)+ `worktree_override: Some(worker_wt)` 切 `ToolContext.worktree_path` + `ReadGuard::new()` reset。
  - `dispatch_subagent` LLM 入参里 `isolation` 字段已经存在 —— PR2 不需要再改 schema。
- **`ToolContext`**:PR1 已经在 chat_loop.rs 用 `worktree_override` 切 worktree_path,PR2 复用。

### 已知依赖

- L3a concurrent dispatch 骨架(`FuturesUnordered` + `DispatchBatch` 分类 + `DELEGATION_MAX_CONCURRENT_CHILDREN` env 硬拒)
- L3b PR1 的 worker worktree + lock + self-heal + 双层合并全部就位
- L3a race-dissolution proof 的 3 竞态点(permission:ask / token usage / cancel) —— **PR2 必须重导**(并发可写后,permission:ask Tier 4 可并发弹 N 个 banner,不再是「并发只读 → ask 不会弹」)

## Requirements

- concurrent dispatch 分支(N ≥ 2 pure dispatch batch)的每个 worker 跑在各自 worktree,不再被 `force_readonly` 剥写。
- serial 分支行为不变(单 dispatch 走 PR1 isolation 默认 + `force_readonly` 仍兼容)。
- N 个并发 worker 各写各的 checkout,commits 落在各自 `worker/<run_id>` 分支,无冲突。
- `force_readonly` 参退役(并发批不再需要传 true),但保留签名兼容(serial 路径可能仍用)。
- 不动 L2 parallel tool(read_file 并发)语义。
- 不动 L3d no-nesting gate(worker 嵌套深度恒 1)。

## Acceptance Criteria

- [ ] N=2 general-purpose worker 并发,各自 `write_file` 同一文件不同内容 → 两份独立 `worker/<run_id>` branch commits,无冲突,parent 收到 2 个 `[status: completed]` summary。
- [ ] N=3 general-purpose worker 并发,各自 `edit_file` 不同文件 → 3 个独立 `worker/<run_id>` branch commits,parent 收到 3 个 summary。
- [ ] 并发 N worker 跑期间,各自 `ToolContext.worktree_path` 不重叠(通过 `MockProvider::sent_tools` 抓 tool call 时的 ctx 上下文,或通过 `subagent_runs.worktree_path` 列验证)。
- [ ] 并发 N worker `permission:ask` Tier 4 行为:L3a 预测文档化(N 个 banner 接受现状,RULE-FrontSubagent-003 已把 worker ask 走 WorkerAskBanner)。
- [ ] N 个 worker token usage **不**再 fold 进 parent(06-26 reversal 锁定,worker 在 `subagent_runs.token_usage_json`)。
- [ ] serial 路径仍可用 `force_readonly=false`(兼容 L3a 测试),concurrent 路径不再传 `true`。

## Definition of Done

- `chat_loop.rs` concurrent 分支(`~line 1788`)改 `force_readonly=true` → 不传 `force_readonly` / 沿用 PR1 isolation 默认(general-purpose builtin `Some(true)`)。
- `dispatch.rs` `force_readonly` 参保留(L3a 兼容)+ 加 deprecated 注释 + 仅 serial 路径使用。
- 新增 `agent::tests_subagent.rs` 测试:
  - `l3b_concurrent_general_purpose_workers_complete_with_writes`(2 worker 各 `write_file`,断言 2 个独立 `worker/<run_id>` branch commits + 2 个 `subagent_runs` 行 + parent 收 2 个 summary)
  - `l3b_concurrent_workers_have_isolated_worktrees`(断言 worker 间 worktree_path 不重叠)
  - `l3b_concurrent_force_readonly_param_no_longer_set`(回归:并发路径 force_readonly 默认 false 或不再传)
- `cargo test --lib` 全绿;`vue-tsc --noEmit` 0 err(PR2 不动前端)。
- spec 更新(`agent-loop-architecture.md` §"Concurrent readonly dispatch" 改写为 §"Concurrent isolated dispatch" + race-dissolution proof 重导)。
- ROADMAP §1.2 L3b PR2 移到已实施;IMPLEMENTATION §4 加 ADR 决策日志。

## Out of Scope (explicit)

- worker branch merge / discard tool(PR3 做)。
- 前端 SubagentDrawer merge/discard UI(PR4 做)。
- worker 嵌套深度 > 1(STRUCTURALLY_DISABLED 锁死)。
- 并发 N worker merge 冲突的自动 resolution(MVP 让用户手动,PR3 决策)。
- background async dispatch(daemon 化阶段做)。

## Decision (ADR-lite)

**Context**: L3a concurrent dispatch 用 `force_readonly=true` 闸门消除并发写冲突,代价是 general-purpose worker 并发被锁死只读。L3b PR1 提供 per-worker worktree 隔离基建(`worker/<run_id>` branch + `worktree_override` + lock)。

**Decision**: PR2 把 L3a 的 `force_readonly=true` 替换为「强制 isolation」语义 —— 并发分支(N ≥ 2 pure dispatch batch)的每个 worker 沿用 PR1 isolation(builtin default + dispatch 入参),`force_readonly` 参退役(serial 路径保留兼容)。

**Consequences**:
- 解锁 general-purpose worker 并发写(L3a → L3b 承诺兑现)。
- 并发 N worker `permission:ask` Tier 4 现在可弹 N 个 WorkerAskBanner(L3a 预测文档化,接受现状,Workaround:用户预先在主对话 AllowAlways)。
- 不再 fold worker token 进 parent(06-26 reversal 锁定)。
- **race-dissolution proof(L3a §"Race dissolution by scope")必须重新推导**(原论据依赖「并发只读无写 → permission:ask 不会弹」,现在并发可写,permission:ask 可弹)。

## Implementation Plan

### 单 PR(可拆 PR2a/2b,如果体积过大)

- **PR2a(代码)**:chat_loop concurrent 分支切换 + 新增测试
  - `chat_loop.rs` concurrent 分支删 `force_readonly=true`(或留 `force_readonly=false` 显式)+ 加注释指 PR1 isolation 自动生效
  - `dispatch.rs` `force_readonly` 参保留 + 加 deprecated 注释
  - 新增 2-3 个 vitest-级别 cargo 测试
  - `cargo test --lib` 全绿
- **PR2b(spec,可选单独 PR)**:race-dissolution proof 重导
  - `agent-loop-architecture.md` §"Concurrent readonly dispatch" 改名为 §"Concurrent isolated dispatch"
  - race-dissolution proof 重导:permission:ask Tier 4 可并发 → N 个 WorkerAskBanner 接受现状;token usage 不 fold(06-26 锁定);cancel fan-out 不变
  - tool-contract.md §"Concurrent dispatch warning" 更新(部分缓解 → 完全缓解)

> PR2a + PR2b 合并 1 PR 即可,L3a 已示范并发骨架就位后切换是几行代码。

## Edge Cases

| 场景 | 默认决策 | 理由 |
|---|---|---|
| 并发 N worker 各自 worktree 创建失败 | fail dispatch(N 个全部 error tool_result) | 与 PR1 worker worktree 失败一致,不静默降级 |
| 并发 N worker 弹 N 个 WorkerAskBanner | 接受现状 | L3a 预测文档化,Workaround 用户预先 AllowAlways |
| 并发 N worker 各跑 token 烧得多 | 不 fold 进 parent(06-26 锁定) | worker token 在 `subagent_runs.token_usage_json` |
| 并发 N worker 完成顺序乱 | tool_result 按 tool_use 原始 index 回填(L2 模板) | L2 + L3a 已有 `result_slots[i]` |
| 并发 N worker 各自 ReadGuard reset | 每个 worker 独立 ReadGuard::new()(PR1 已实现) | 不共享 |
| 并发 N worker 取消(parent Stop) | `worker_token = parent_token.child_token()` × N → fan-out 取消所有(PR1 + L3a 已实现) | 沿用 |

## Technical Notes

- 复用文件:`chat_loop.rs` concurrent 分支(`~1788`)/ `dispatch.rs` `run_subagent` / PR1 isolation 全套(create_worker / worktree_override / ReadGuard reset)。
- 不变式:worker 嵌套深度恒 1 + per-worker grant 隔离已就位 + worker turn `skip_persist=true` 不污染 parent messages + L3d no-nesting gate 未退化。
- libgit2 merge API(PR3 也会用到):`Repository::merge_commits(our_commit, their_commit, ...)` 返回 `MergePreference` + `AnnotatedCommit`,再 `Repository::merge(their_annotated, merge_opts, ...)` 应用。如果有 conflict,merge 结果带 conflict marker,tool 可解析。
- WSL/git2-rs:create_worker 已 PR1 自-heal 复用,无需重新设计。
- 完整原始 L3b PRD + research:`.trellis/tasks/archive/2026-06/06-27-l3b-worktree-delegate/prd.md` + `research/subagent-worktree-isolation-patterns.md`