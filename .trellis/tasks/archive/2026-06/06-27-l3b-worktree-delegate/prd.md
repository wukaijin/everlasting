# L3b worktree 隔离 + 仿 Hermes delegate_task

## Goal

让 subagent worker 能在**隔离的 git worktree**(独立 checkout + 独立分支)里执行,而非当前复用 parent session 的工作区。对齐 **Claude Code `isolation: worktree`**(隔离范本) + **Hermes `delegate_task`**(delegate 语义范本:独立 context / 独立 turn 预算 / summary 回填)。

**为什么**:当前 worker 复用 parent 的 `current_ctx.worktree_path`(`dispatch.rs:144`),无隔离 —— 并发 worker 写会互相踩(L3a 只能用 `force_readonly` 把并发 worker 锁死成只读)。worktree 隔离可消除并发写冲突 → 解锁 `general-purpose` worker 的并发写能力,这是 L3a → L3b 的核心演进动机,也是 ROADMAP §1.3 标注的「旗舰级 harness 学习项」。

## What I already know

### 代码现状(已 Auto-Context 核对)

- **`git/worktree.rs` 已就位**(create/destroy/self-heal/check_clean):
  - `create(project_path, worktree_path, session_id)`:建 `session/<id>` 分支(基于 project HEAD),含 stale metadata / stale branch / orphan dir 三态 self-heal。
  - `destroy(project_path, worktree_path, session_id)`:物理删 dir + prune metadata + 删 branch(best-effort)。
  - `worktree_path(data_dir, project_id, session_id) → <app_data_dir>/worktrees/<project_uuid>/<session_uuid>`。
  - `check_clean(repo_path)`:检查无未提交改动。
  - **但现有 worktree 是 session 级**(`commands/worktree.rs::attach_worktree`,session 创建时 attach、删除时 destroy),分支前缀硬编码 `session/`。worker 需要独立 worktree,**不能复用 session worktree**。
- **`dispatch.rs::run_subagent`**:
  - worker 当前用 `current_ctx.worktree_path`(= parent session worktree,`dispatch.rs:144`),无隔离。
  - worker 经 `filter_tools_for_subagent`(SubagentDef.tools allowlist)+ 可选 `filter_tools_readonly`(force_readonly,L3a)。
  - worker 在 parent session_id 下跑(`skip_persist=true`,中间 turn 不落 messages 表)。
  - per-worker `RunGrantCache`(06-26 加,`dispatch.rs:354`)。
  - **回流只有 summary 文本**(`final_text()`,`dispatch.rs:465`),无 diff/commit 回灌。
  - 返回 `(content, is_error, cancel_parent, exit_code)`。
- **`chat_loop.rs` DispatchBatch**:`Serial`(单个/混合批)/ `Concurrent`(纯 dispatch 批 ≥2,`force_readonly=true`)/ `OverLimit`(超 `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 3,硬拒)。并发走 `FuturesUnordered`(`chat_loop.rs:1788`)。
- **`SubagentDef.tools: Vec<String>`**(`subagent/mod.rs:289`),`READONLY_TOOL_ALLOWLIST = [read_file, grep, glob, list_dir, web_fetch]`(`mod.rs:546`)。
- **`ToolContext`** 有 `worktree_path: PathBuf` + `cwd: PathBuf`(`tools/mod.rs:121-123`)—— worker 的 tool 通过这个字段定位执行目录,改它即可让 worker 跑在隔离树。
- **`git/diff.rs::diff_worktree`** 已就位(可复用抓 worker 改动)。

### Research 关键结论(见 Research References)

1. **Hermes `delegate_task` 不用 worktree 隔离**:child 共享 parent cwd,只隔离 conversation/terminal/file_state,回流 = 纯 summary 自报告 + sibling-write stale-read 警告。ROADMAP「对齐 Hermes delegate_task」= 对齐 **delegate 语义**,不是 worktree 隔离。
2. **Claude Code 是唯一做 per-subagent worktree 隔离的主流工具**:`isolation: worktree` frontmatter(opt-in)+ 临时 worktree(`.claude/worktrees/<value>/`,分支 `worktree-<value>`,base = `origin/HEAD` 默认 / `worktree.baseRef: head` opt-in)+ `git worktree lock` 防并发清理 + `cleanupPeriodDays` sweep + `.worktreeinclude` 复制 gitignored 文件。**有 changes → 保留 worktree+branch(PR 式产物);无 changes → 自动销毁**。
3. OpenHands(隔离在 Docker sandbox 层,非 subagent 层)/ Aider(无 subagent)/ opencode(子 session 共享 cwd)/ Cline(subagent 天然 read-only)都**不做** per-subagent worktree 隔离。
4. 改动回流 3 模式:(a) worktree+commit 留分支(Claude Code,PR 式,唯一工业级);(b) diff/patch 回流(理论模式,**0 采用**);(c) 共享 cwd 直接写 + stale-read 警告(Hermes/当前本项目)。
5. worktree 生命周期:**全部 per-run create+destroy,无 pool/复用**(业界 0 采用 pool)。本项目 `git/worktree.rs` 的 self-heal 可直接复用。

## Assumptions (temporary, 待 grill 确认)

- 走 Approach 1(Claude Code 路线):per-worker worktree + commit 留分支。
- worker worktree base 默认 = parent worktree HEAD(继承 parent WIP,语义直观)。
- 通过新 frontmatter 字段(对标 `isolation: worktree`)opt-in,非默认全隔离。
- ReadGuard 在 worker 入口 reset(worker 新 checkout,无继承的已读集合)。

## Open Questions (按依赖排序,逐个 grill)

_全部闭合,进入 final confirmation(见 §Implementation Plan)。_

### 已闭合
- ✅ **方向 = Approach 1**(per-worker worktree + commit 留分支,Claude Code 路线)— 2026-06-27 grill 确认。
- ✅ **gitignored 文件**:现状已确认 —— session/worker worktree 均**不复制** gitignored 文件(标准 `git worktree add` 行为,无 `.worktreeinclude` 等价逻辑)。MVP **不做** gitignored 复制(保持与 session worktree 一致),作 follow-up(对标 Claude Code `.worktreeinclude`)。worker 跑测试若缺 `.env` 失败,与 session worktree 同款问题,非 L3b 回归。
- ✅ **base = parent session worktree HEAD**(worker 从 parent session 分支 `session/<id>` 当前 commit 出发,parent 进度的延伸)— 2026-06-27 grill 确认。注意 base 是 commit 级,parent 未提交 WIP worker 看不到(git worktree 固有限制)。
- ✅ **改动回流 + 生命周期 = 完整 Claude Code 路线**(有 changes 保留 branch `worker/<run_id>` + diff summary 回填 tool_result;无 changes destroy;+ `cleanupPeriodDays` 等价 sweep + `merge_worker`/`discard_worker` tool + 前端 SubagentDrawer 合并/丢弃按钮)— 2026-06-27 grill 确认。**L3b 升级为多 PR 旗舰任务**(用户选完整路线,非简化 MVP)。
- ✅ **并发 = PR2 解锁并发写**(L3a concurrent 分支 `force_readonly` → 每个 worker 各自 worktree,写冲突被隔离消解,解锁 general-purpose worker 并发写,L3b 核心价值;N worker branch 回流靠 PR3 `merge_worker` tool 批量)— 2026-06-27 grill 确认。
- ✅ **隔离触发 = 双层模型**(frontmatter default + dispatch-time override)— 2026-06-27 grill 确认:
  - **frontmatter 层(per-agent default)**:`isolation: worktree`(对标 Claude Code)。builtin `general-purpose` 默认带;`researcher` 不带(只读,省 checkout 开销)。
  - **dispatch 层(per-dispatch override)**:`dispatch_subagent` tool 入参加 `isolation: Option<bool>`,主 agent LLM 派单时可覆盖。
  - **合并语义**(优先级:dispatch 入参 > frontmatter default > 不隔离):

    | frontmatter | dispatch `isolation` | 结果 |
    |---|---|---|
    | `isolation: worktree` | 未指定 | 隔离 |
    | `isolation: worktree` | `false` | 不隔离(主 agent 主动跳过) |
    | 未声明 | `true` | 隔离(主 agent 主动要求) |
    | 未声明 | 未指定 | 不隔离(当前行为) |

## Requirements (evolving)

- worker 可在隔离 git worktree(独立 checkout + 独立 `worker/<run_id>` 分支)执行,不污染 parent session 工作区。
- worker 的 `ToolContext.worktree_path` 指向 worker worktree。
- worker 完成后,改动以可审查形式回流给 parent agent(diff summary / 独立 branch)。
- ReadGuard 在 worker 入口 reset(技术约束,非可选)。
- 复用 `git/worktree.rs` 现有 create/destroy/self-heal,分支前缀改 `worker/`(不复用 `session/`)。

## Acceptance Criteria (evolving)

- [ ] worker 跑在独立 worktree,parent 工作区文件不被 worker 改动影响。
- [ ] 并发 N 个 worker 各自 worktree,写不冲突(L3a concurrent 分支解锁)。
- [ ] worker 完成后,parent agent 能看到 worker 的改动(diff 或 branch)。
- [ ] 崩溃/取消后无残留 worktree+branch(self-heal 验证)。
- [ ] worker 跑期间 worktree 被锁,防 sweep 误删。

## Definition of Done

- Rust 单元测试(git/worktree worker 分支变体 + dispatch.rs worktree 路径计算)+ 集成测试(并发 worker 各自 worktree)。
- `cargo test --lib` 全绿;`vue-tsc --noEmit` 0 err(若动前端)。
- spec 更新(`agent-loop-architecture.md` + `tool-contract.md`)。
- ROADMAP L3b 移到 §1.2 已实施;IMPLEMENTATION §4 决策日志。

## Out of Scope (explicit)

- background/async dispatch(Hermes async_delegation 的「completion 走新 turn」契约,opencode 实验特性,留 daemon 化档)。
- worktree pool / 复用(业界 0 采用)。
- worker 嵌套(深度 >1,已 STRUCTURALLY_DISABLED 锁死)。

## Decision (ADR-lite)

**Context**: L3b 要让 subagent worker 隔离执行。research 发现 Hermes delegate_task 不隔离 worktree(共享 cwd + stale-read 警告),真做 per-subagent worktree 隔离的工业级范本只有 Claude Code(`isolation: worktree`)。3 个候选:Approach 1(worktree+留分支,Claude Code)/ Approach 2(worktree+diff 回填,无工业先例)/ Approach 3(不隔离+stale 警告,Hermes)。

**Decision**: 选 **Approach 1**(per-worker worktree + commit 留分支,Claude Code 路线)。

**Consequences**:
- 对标 Claude Code,harness 学习价值最大化;消除并发写冲突 → 解锁 general-purpose worker 并发写(L3a 当前 read-only 锁死)。
- 代价:需 sweep/merge tool + 前端合并/丢弃 UI + 新 frontmatter 字段;分支 `worker/<run_id>` 会堆积(需 sweep)。
- 可分阶段:PR1 serial-path 单 worker worktree 最小闭环 → PR2 并发切换 → PR3 sweep/merge tool + 前端。
- gitignored 文件 MVP 不处理(follow-up)。

## Implementation Plan (多 PR,final confirmation 待用户 approve)

> PR1 = 最小闭环(serial 隔离),PR2-4 在其上叠加。每个 PR 独立可发布、独立测试。

### PR1 — serial-path worker worktree 隔离核心(最小闭环)

- `git/worktree.rs`:加 worker 变体 —— branch 前缀 `worker/<run_id>`(不复用 `session/`),base = parent session worktree HEAD,per-run create + destroy;复用现有 self-heal。
- `SubagentDef` 加 `isolation: Option<bool>` frontmatter 字段(`resource_loader.rs` 解析);builtin `general-purpose` 默认带 `isolation: worktree`,`researcher` 不带。
- `dispatch_subagent` tool 入参加 `isolation: Option<bool>`。
- 双层合并语义实现(dispatch 入参 > frontmatter default > 不隔离,见 §已闭合表)。
- `run_subagent`:隔离时建 worker worktree + `git worktree lock`,`ToolContext.worktree_path` 指向 worker 树,ReadGuard **reset**;非隔离时保持当前行为(parent worktree)。
- worker 完成:`git diff` 判 changes —— 有 changes 保留 branch + diff summary 回填 tool_result(告知 parent「改动在 `worker/<run_id>`」);无 changes destroy。
- `subagent_runs` 加 `worktree_path TEXT NULL` 列(destroy 后置空,保留 branch 时留路径供审查)。
- 测试:worker worktree create/destroy + 隔离 vs 非隔离路径分流 + ReadGuard reset + 有/无 changes 分支。

### PR2 — 并发解锁(L3a concurrent 分支)

- L3a concurrent 分支(`chat_loop.rs:1788`):`force_readonly=true` → 每个 worker 各自 worktree(复用 PR1 隔离逻辑)。
- 解锁 `general-purpose` worker 并发写(N worker 各自 worktree,写冲突被隔离消解)。
- 测试:并发 N worker 各自 worktree,写不冲突 + 各自 branch 独立。

### PR3 — `merge_worker` / `discard_worker` tool + sweep

- `merge_worker` tool:把 `worker/<run_id>` merge 到 parent session 分支 `session/<id>`(conflict → fail 返回 conflict 文件列表,让用户手动,MVP 不做自动 resolution)。
- `discard_worker` tool:销毁 worker worktree + 删 `worker/<run_id>` branch。
- sweep 机制:`cleanupPeriodDays` 等价,按 mtime/时间扫描清理过期 `worker/*` branch + 残留 worktree(复用 self-heal)。
- 测试:merge 正常 / conflict fail / discard / sweep 过期清理。

### PR4 — 前端 SubagentDrawer 合并/丢弃 UI

- `<SubagentDrawer>` 加 worker branch 可视化(branch 名 + worktree 状态:隔离中 / 已完成留 branch / 已 destroy)。
- Merge / Discard 按钮,调 `merge_worker` / `discard_worker` Tauri command。
- subagent store 加 worker worktree 状态字段。
- `vue-tsc --noEmit` 0 err。

## Edge Cases (默认决策,approve 时可调整)

| 场景 | 默认决策 | 理由 |
|---|---|---|
| worker 取消(user Stop) | 按正常完成处理(有 changes 保留 branch,无 destroy),用户可事后 discard | 取消≠清理,保留产物供审查 |
| worker worktree 创建失败(磁盘满/路径冲突) | **fail dispatch**(返回 error tool_result),不降级到不隔离 | 避免静默行为不一致(LLM 以为隔离实际没隔离) |
| `merge_worker` conflict | fail,返回 conflict 文件列表,用户手动解决 | MVP 不做自动 conflict resolution |
| 并发 N worker merge | 串行 merge(parent session 分支一次接一个) | 避免连锁 conflict |
| 崩溃残留 worker worktree+branch | self-heal(下次 create 清)+ sweep | 复用现有机制 |

## Research References

- [`research/subagent-worktree-isolation-patterns.md`](research/subagent-worktree-isolation-patterns.md) — Hermes/Claude Code/OpenHands/Aider/opencode/Cline 横向对比 + 改动回流 3 模式 + 3 个 L3b 候选方案;**关键 caveat:Hermes 不隔离 worktree,真隔离范本只有 Claude Code**。

## Research Notes

### 3 个候选 Approach(research 推导)

**Approach 1: per-worker worktree + commit 留分支(Claude Code 路线)⭐ research 推荐**
- How:worker worktree(branch `worker/<run_id>`,base = parent HEAD)+ lock + ToolContext 指向 worker 树;有 changes 保留 branch,无 changes destroy;复用 L3a FuturesUnordered 并发骨架。
- Pros:对标 Claude Code,harness 学习价值最大;消除并发写冲突;改动可审查/可回滚;机制已就位增量小。
- Cons:需新 frontmatter 字段;base 选择需决策;分支管理需 sweep/merge tool;gitignored 文件是 hidden cost。

**Approach 2: per-worker worktree + diff 回填(无分支残留)**
- How:同 1 的 create+lock;worker 结束 always destroy,但先 `git diff` 抓改动放 subagent_runs + tool_result;parent LLM 自己 apply。
- Cons:diff apply 可能 conflict;无工业先例(模式 b 0 采用);harness 学习价值低。

**Approach 3: 不做 worktree,强化 stale-read 警告(Hermes 路线)**
- How:不引入 worktree;在 read_guard 上加 sibling-write stale-read warn;并发继续 force_readonly。
- Cons:**不满足 ROADMAP「worktree 隔离」标题**;不解锁并发写;与「旗舰级学习项」定位不符。

### 4 个实施前必须决策的 caveat(research 标记)

- **base 选择**:parent worktree HEAD vs project main HEAD。
- **ReadGuard reset**:worker 新 checkout 不能继承 parent 已读集合(edit_file 前置 check 会误判)。
- **gitignored 文件**:当前 session worktree 是否复制 `.env`?L3b worker worktree 需等价机制,否则 worker 跑测试因缺 `.env` 失败。**task.md 没提,需 grill**。
- **merge/discard UI**:保留 branch 需给用户合并/丢弃入口(对标 Claude Code keep-or-remove + sweep),前端 SubagentDrawer 可能要加按钮 —— 隐藏前端工作量。

## Technical Notes

- 复用文件:`git/worktree.rs`(create/destroy/self-heal)、`git/diff.rs::diff_worktree`、`dispatch.rs::run_subagent`、`chat_loop.rs` DispatchBatch/Concurrent 分支、`ToolContext.worktree_path`、per-worker `RunGrantCache`。
- 不变式:worker 嵌套深度恒 1(STRUCTURALLY_DISABLED);per-worker grant 隔离已就位;worker turn 用 `skip_persist=true` 不污染 parent messages。
- WSL/git2-rs:libgit2 无 `worktree remove`,destroy 靠 `remove_dir_all` + prune(现有方案)。
