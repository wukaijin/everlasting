# A+B: worker auto-commit + isolation 改为系统层自动决定

> Parent: `06-30-subagent-worktree-smooth`

## Goal

消除 isolated worker 的 **merge 假成功 bug**（A），并把 isolation 决策从「def 默认 + 主 agent opt-in」改为「**系统层按 serial/parallel 自动决定**」——日常单 dispatch 零 merge、同 turn 多 dispatch 仍并发安全（B）。

## Requirements

### A — worker auto-commit 兜底
- 在 `run_subagent`（`agent/subagent/dispatch.rs`）中，`probe_worker_changes` 检测到 `has_changes=true` 后、**保留 worktree 之前**，对 worker worktree 执行 `git add -A` + `git commit`，把 working tree 改动固化为一个 commit。
- commit message 含 `worker/<run_id>`，便于溯源。
- 失败时（如空 identity）回退到 Everlasting 默认 signature，不阻断「保留 worktree」逻辑。
- **不依赖 sub-agent 是否主动 commit**：无论 sub-agent 是否提交过，auto-commit 兜底保证 `has_changes` 时 `worker_tip` 真领先于 `parent_tip`。

### B — isolation 改为系统层 serial/parallel 自动决定
- **serial path**（一个 turn 单个 `dispatch_subagent`）→ 默认 **shared**（共享主 agent cwd，改动立即可见，零 merge）。`general-purpose` 的 `isolation` 默认改为 `None`；`researcher` 保持 `None`。
- **parallel path**（一个 turn 多个 `dispatch_subagent`，`chat_loop.rs` 的 `FuturesUnordered` 并发分支，约 `chat_loop.rs:2095-2118`）→ **系统强制 isolated**：在该分支调用 `run_subagent` 时无视 `def.isolation` 默认，强制 `isolation=true`，保证并发 worker 各自在 `worker/<run_id>` worktree 写、不 race。**接管 L3b PR2 原本由「`general-purpose` 默认 isolated」提供的并发安全论证**（`chat_loop.rs:2106-2108`）。
- 实现落点：`resolve_isolation` 增加一个「parallel 强制」入参（或 parallel 调用点直接传 `isolation=true`），serial 路径维持 def 默认（`None`=shared）。
- **tool description 提醒主 agent 该机制**（改写 `dispatch_subagent` description，`mod.rs:119-142` + schema `mod.rs:160-169`）：说明「单 dispatch 共享主工作区、改动立即可见；同 turn 多 dispatch 系统自动隔离到各自 worktree、并发安全、鼓励独立子任务 fan-out；isolation 由系统自动决定，通常无需手动指定」。**删掉旧的「`general-purpose` defaults to isolated; pass `false` to force shared」表述**（`mod.rs:139-141, 167-168`）。

## Acceptance Criteria

- [ ] isolated worker **不主动 commit** → `merge_worker` 能真正合并其改动（不再因 `worker_tip==parent_tip` 空走 FF）；成功后 parent worktree **确实含** worker 改动（测试断言 diff 非空）
- [ ] **serial** 单 dispatch `general-purpose` → **不创建** worker worktree，改动直接落主 agent cwd（shared）
- [ ] **parallel** 同 turn 多 dispatch → 每个被**强制隔离**到各自 worker worktree；并发写不互相覆盖（测试：两 worker 改同一文件，结果各自落自己分支、不 race）
- [ ] `dispatch_subagent` tool description 不再声称「`general-purpose` defaults to isolated」；新增 serial/parallel 自动决策 + 主 agent 无需手动指定的说明
- [ ] `resolve_isolation` 现有优先级测试（frontmatter/dispatch）在 serial 语义下通过；**新增** parallel-force-isolated 测试
- [ ] `probe_worker_changes_*` / `do_merge_blocking` 相关测试不破；**新增** auto-commit 兜底测试（worker 不 commit → merge 后改动可见）

## Out of Scope

- C（注入提示）/ D（publish UI）→ child2
- push remote

## Notes

- B 的并发安全论证从「def 默认 isolated」迁移到「parallel path 系统强制 isolated」——**不靠主 agent 判断**（主 agent 不自知并发，并发由 multi-tool_use 在系统层触发）。design.md 须对照 L3a/L3b（`chat_loop.rs:2095-2118`、`dispatch.rs:246-251`）论证迁移后并发安全**等价**、且 `force_readonly` 的 serial-only 语义不变。
- tool description 的「提醒」让主 agent 敢于 fan-out 独立子任务（知道并发自动隔离安全），同时知道单 dispatch 改动立即可见——与 child2 的 C（提醒 sub-agent）**对称**。
- auto-commit **系统兜底**是实现 A 的推荐方式（不依赖 sub-agent 行为，最稳）；C 的提示仅做「知情」、不要求自提交。
