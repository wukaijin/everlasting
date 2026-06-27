# Research: 业界 agent harness 的 subagent 工作树隔离 + delegate + 改动回流模式

- **Query**: 调研 Hermes / Claude Code / OpenHands / Aider / opencode / Cline 的 subagent 工作树隔离、delegate/任务委派、改动回流模式,映射到本项目 L3b
- **Scope**: external(主流 agent harness 源码 + 官方文档)+ internal(本项目现状 spec/code)
- **Date**: 2026-06-27

---

## TL;DR(写在前面的关键结论)

1. **Hermes `delegate_task` 不用 worktree 隔离**。子 agent 复用 parent 的 cwd(同一份 checkout),只隔离 conversation / terminal session / file_state 写入追踪。改动回流 = 纯 summary 文本(自报告),parent 用 `file_state` 的 sibling-write 检测来给 stale-read 警告。本项目 task.md 把 Hermes 写成"对齐 target"指的是 **delegate 语义**(独立 context / 独立 turn 预算 / summary 回填),**不是** worktree 隔离。
2. **Claude Code 是唯一明确提供"per-subagent worktree 隔离"的主流工具**。机制:`isolation: worktree` frontmatter(per-subagent opt-in)+ 临时 worktree(`.claude/worktrees/<value>/`,分支 `worktree-<value>`,base = `origin/HEAD` 或 `worktree.baseRef: head`)+ `git worktree lock` 防并发清理 + `cleanupPeriodDays` 扫描清理 + `.worktreeinclude` 复制 gitignored 文件。worker 完成后,如果有 changes → 保留 worktree + branch(等价于"PR 式产物:独立分支上的 commits"),如果没有 changes → 自动销毁。
3. **OpenHands / Aider / opencode / Cline 都不做 per-subagent worktree 隔离**:
   - **OpenHands**:隔离在 **Workspace / Docker sandbox** 层(per-session/per-runtime),不是 per-subagent。子 agent 共享 parent 的 workspace。`sdk/git/` 只有 diff/status/changes,无 worktree。
   - **Aider**:根本没有 subagent / delegate 概念(单 loop + repomap)。无对应模式。
   - **opencode**:Task tool 创建子 **session**(`sessions.create({ parentID })`),共享 cwd,无 worktree。结果以 `<task_result>` XML 回填 parent。
   - **Cline**:subagent 是 **read-only research** agent(不能 edit/browse/MCP/nest),因此天然无 write 冲突,不需要 worktree。
4. **改动回流 3 种业界模式**:(a) **worktree + commit 到独立分支**(Claude Code,PR 式,主会话决定 merge/cherry-pick/丢弃);(b) **diff/patch 回流**(罕见,理论模式,无主流工具采用);(c) **共享 cwd 直接写 + file_state 追踪 + summary 自报告**(Hermes / opencode / 当前本项目)。**Claude Code 是 (a) 的唯一工业级范本**;Hermes 是 (c) 的最成熟范本。
5. **worktree 生命周期**:Claude Code = per-subagent-run create + 自动/手动 destroy(`cleanupPeriodDays` sweep,带 `git worktree lock` 防竞态)。**没有发现"worktree pool / 复用"模式** —— 所有调研的工具都是 per-run create+destroy。复用在工业界不是常见做法(per-run 开销主要是 checkout,可接受)。
6. **并发 worker + 各自 worktree**:Claude Code 明确支持(每个 subagent 一个临时 worktree,`git worktree lock` 互不干扰)。Hermes 用 ThreadPoolExecutor fan-out 但**共享 cwd**,靠 read-only 范围(L3a 同思路)或 file_state 警告来回避写冲突。

---

## Findings

### A. Hermes(NousResearch/hermes-agent)— `delegate_task`

**仓库**:`NousResearch/hermes-agent`(Python,~20 万 stars,Claude Code 开源对位项目)。核心文件:`tools/delegate_tool.py`(3198 行)+ `tools/async_delegation.py`(556 行)+ `agent/agent_runtime_helpers.py`。

#### A.1 隔离边界 —— 是 context/terminal/file_state,**不是** worktree

`tools/delegate_tool.py` 顶部 docstring(line 2-17)白纸黑字:

> Spawns child AIAgent instances with **isolated context**, restricted toolsets, and **their own terminal sessions**. ...
> Each child gets:
>   - A fresh conversation (no parent history)
>   - Its own task_id (own terminal session, **file ops cache**)
>   - A restricted toolset (configurable, with blocked tools always stripped)
>   - A focused system prompt built from the delegated goal + context

**全文件 0 处 `worktree` / `branch` / `git commit` / `merge` / `cherry-pick`**(keyword scan 确认)。`_resolve_workspace_hint` (line 742-766)把 parent 的 `TERMINAL_CWD` / `terminal_cwd` / `cwd` 作为 **prompt 提示** 传给 child —— 也就是说 child 与 parent **在同一个 checkout 里跑**,只是各自的"terminal working directory state"独立。

工具描述(line 2949)的原话:
> "Each subagent gets its own terminal session (separate working directory and state)."

这里的 "separate working directory" 指 **shell session 的 pwd state**(child 可以 `cd` 到不同子目录而不影响 parent 的 shell),**不是** git worktree 的独立 checkout。

#### A.2 改动回流 —— 纯 summary 自报告 + file_state stale-read 警告

回流契约(line 2896-2899 工具描述):
> "Only the final summary is returned -- intermediate tool results never enter your context window."

并且工具描述明确警告 summary 是 **self-report,不是 verified fact**(line 2933-2939):
> "Subagent summaries are SELF-REPORTS, not verified facts. A subagent that claims 'uploaded successfully' or 'file written' may be wrong. For operations with external side-effects ... require the subagent to return a verifiable handle (URL, ID, absolute path, HTTP status) and verify it yourself ..."

**关键机制 —— sibling-write stale-read 检测**(line 1899-1920):child 跑完后,代码用 `file_state.writes_since("", wall_start, [])` 拿到所有 child 写过的文件,跟 parent 的 `known_reads(parent_task_id)` 求交集;如果有 overlap,把 `[NOTE: subagent modified files the parent previously read — re-read before editing: <paths>]` **追加到 summary 文本**里回填给 parent LLM。

**这是 Hermes 处理"共享 cwd 写冲突"的核心方案 —— 不是隔离,而是事后警告 + 让 parent LLM 自己 re-read**。等价于本项目 `read_guard` 的思路,但更弱(只在 dispatch 结束时一次性检查,不是 per-edit 检查)。

#### A.3 生命周期 + 并发

- **生命周期**:per-run。`ThreadPoolExecutor` per child(line 1644-1652),child 跑完 `child.close()`(line 2054-2058)释放 terminal sandbox / browser daemon / httpx clients。**无 worktree pool / 复用**。
- **并发**:`delegate_task(tasks=[...])` batch 模式,line 2500-2509 走 `dispatch_async_delegation_batch`,N 个 child 各自一个 ThreadPoolExecutor worker,**共享同一个 cwd**。安全性靠 `DELEGATE_BLOCKED_TOOLS`(line 45-54:禁 `delegate_task`/`clarify`/`memory`/`send_message`/`execute_code`/`cronjob`)+ 可配置 toolset 限制 —— **没有强制 read-only**,所以 Hermes 的并发 delegate 在写场景下**理论上仍然会冲突**,靠用户/LLM 自律 + file_state 事后警告兜底。
- **嵌套**:`role: leaf`(默认,禁 `delegate_task`)vs `role: orchestrator`(保留 `delegate_task`,可嵌套到 `max_spawn_depth`)。本项目对应:`STRUCTURALLY_DISABLED` 在 `filter_tools_for_subagent` 里把 `dispatch_subagent` 从 worker 工具集剥掉,深度恒为 1(dispatch.rs 注释 line 340-345)。

#### A.4 async/background 模式 —— 复用现有 completion rail

`tools/async_delegation.py` line 9-22 的设计值得借鉴:**completion 走 parent 进程已有的 `process_registry.completion_queue`**(CLI 的 `process_loop` / gateway 的 `_run_process_watcher` 都在 drain 这个 queue),把 child 完成事件 **forge 成一个新的 user/internal turn**(而非 splice 到正在跑的 turn 中间)。理由(line 16-19):
> "completions surface as a NEW turn when the agent is idle, never spliced between a tool result and an assistant message. That keeps strict message-role alternation legal and the prompt cache intact (**hard invariant: never mutate past context**)."

**对本项目的启示**:如果未来 L3b 之后做 background dispatch,应该把 worker 完成事件做成 parent 的 **新 turn**,而非塞进当前 turn 的 tool_result —— 这跟当前 B6 的同步阻塞语义(dispatch_subagent tool_result)是两套契约。

---

### B. Claude Code(Task tool + worktree isolation)— **唯一工业级 worktree-per-subagent 范本**

**文档来源**:`https://docs.claude.com/en/docs/claude-code/worktrees` + `/sub-agents` + `/agent-view`。Claude Code 闭源,以下基于官方文档(无源码)。

#### B.1 双层模型:session-level vs subagent-level worktree

Claude Code 把"并行隔离"分成几个正交概念(`worktrees` 文档开头):
- **`--worktree` / `EnterWorktree`**:**session-level** 隔离(整个 Claude Code 会话跑在独立 worktree 里)。这是给"用户在终端 A 改 feature,终端 B 改 bug"的场景。
- **subagent `isolation: worktree`**:**subagent-level** 隔离(单个 subagent 跑在临时 worktree 里)。这是 L3b 的对位概念。
- **agent view / background sessions**:**进程级**隔离(每个 background session 是独立 Claude Code 进程,由 supervisor 托管)。

文档原话(`worktrees` 页 "Isolate subagents with worktrees" 段):
> "Subagents can run in their own worktrees so parallel edits don't conflict. Ask Claude to 'use worktrees for your agents', or set it permanently on a custom subagent by adding **`isolation: worktree`** to the frontmatter. **Each subagent gets a temporary worktree that is removed automatically when the subagent finishes without changes.** Subagent worktrees use the same base branch as `--worktree`..."

#### B.2 worktree 规格(per-subagent)

| 维度 | Claude Code 规格 | 文档依据 |
|---|---|---|
| 路径 | `.claude/worktrees/<value>/`(repo root 下,默认) | `worktrees` 页 "Start Claude in a worktree" |
| 分支名 | `worktree-<value>`(新分支) | 同上 |
| base | `origin/HEAD`(默认)/ `worktree.baseRef: "head"`(用本地 HEAD,带未推 commits)/ `#<PR>`(从 PR fetch) | "Choose the base branch" 段 |
| gitignored 文件 | `.worktreeinclude`(`.gitignore` 语法,只复制"既 match 又 gitignored"的文件,如 `.env` / `config/secrets.json`) | "Copy gitignored files into worktrees" 段;**显式覆盖 subagent worktrees** |
| 并发保护 | agent 跑期间 `git worktree lock`,结束后释放 | "Clean up worktrees" 段 |
| 自动销毁条件 | no uncommitted changes + no untracked files + no new commits → 自动删 | 同上 |
| 保留(sweep) | 有 changes 的 worktree 等 `cleanupPeriodDays` 过期后才 sweep;`--worktree` 手动建的不被 sweep | 同上 |
| 非 git VCS | `WorktreeCreate` / `WorktreeRemove` hook 替换默认 git 逻辑(SVN/Perforce/HG) | "Non-git version control" 段 |

#### B.3 改动回流 —— "PR 式产物:独立分支上的 commits"

Claude Code 的语义是(综合 `worktrees` + `sub-agents` 文档):
- subagent 在自己的 worktree 里 edit + commit(commits 落在 `worktree-<value>` 分支上);
- subagent 结束 → summary 回填给 parent(同 Hermes);
- **但 worktree + branch 作为"产物"保留**(如果有 changes),parent / 用户事后可以 `git merge` / `cherry-pick` / `cd` 进去继续 / 直接丢弃。

这是**业界唯一的 "PR-style subagent" 模型** —— worker 不只是回填文本,而是留下一个**可审查、可合并、可丢弃的 git 分支**。文档没有明确说 parent 自动 merge;从 `cleanupPeriodDays` + "Claude prompts you to keep or remove" 的描述看,**保留/合并决策权在用户/parent**,worker 只负责产出分支。

#### B.4 subagent 本身的行为契约(`sub-agents` 页)

> "Subagents work **within a single session**. ... Each subagent runs in its own context window with a custom system prompt, specific tool access, and independent permissions."

注意:**不开 `isolation: worktree` 的 subagent 默认跟 parent 共享 session 的 cwd**(对应 Hermes / 本项目当前 B6 行为);`isolation: worktree` 是 opt-in 升级。这给了我们一个清晰的"两档"参照:
- 默认档(无 isolation)= 当前本项目 B6 / Hermes;
- 隔离档(`isolation: worktree`)= L3b 目标。

---

### C. OpenHands(All-Hands-AI)— 隔离在 Workspace/Docker 层,不在 subagent 层

**仓库已迁移**:`OpenHands/software-agent-sdk`(主 SDK)+ `OpenHands/agent-canvas`(UI)。`openhands-sdk/openhands/sdk/` 下有 `subagent/` + `git/` + `workspace/` 三个目录。

#### C.1 subagent 模块只是 definition/registry,不是 dispatch

`sdk/subagent/AGENTS.md` 明确(line 1-12):这个 package 只负责 **subagent discovery + registration**(从 `.agents/agents/*.md` / `.openhands/agents/*.md` / `~/.agents/agents/*.md` 加载 markdown frontmatter + body 作为 system_prompt),**dispatch loop 在 `conversation/impl/local_conversation.py`**。`AgentDefinition` 的 frontmatter keys:`name` / `description` / `tools` / `model`(默认 `inherit`)/ `color`。**没有 `isolation` / `worktree` 字段**。

#### C.2 git 模块只做 diff/status/changes,无 worktree

`sdk/git/` 下 6 个文件:`cached_repo.py` / `git_changes.py` / `git_diff.py` / `utils.py`(`run_git_command`)/ `exceptions.py` / `models.py`。**全 0 处 `worktree` keyword**。`git_changes.py` 的 docstring(line 1-3):
> "Get git changes in the current working directory relative to the remote origin if possible."

#### C.3 隔离在 Workspace 层

`sdk/workspace/base.py` 的 `BaseWorkspace`(line 23-34):
> "Workspaces provide a **sandboxed environment** where agents can execute commands, read/write files ..."

OpenHands 的隔离模型是 **per-runtime sandbox**(Docker container / local namespace),**per-session**,不是 per-subagent。子 agent(`AgentContext(system_message_suffix=...)`,AGENTS.md "Body → system prompt" 段)继承 parent 的 workspace —— subagent 是 parent conversation 里的一个**带不同 system prompt + tool subset 的角色切换**,不是独立执行环境。

**对本项目的启示**:OpenHands 走的是"重 sandbox(Docker per session)"路线,本项目走的是"轻 worktree(per session)"路线,两者都是 **session 级**隔离,subagent 都共享 parent 的执行环境。OpenHands 不提供 per-subagent 隔离的参考。

---

### D. Aider — 无 subagent 概念

`Aider-AI/aider/aider/` 目录扫描:0 个文件名匹配 `subagent` / `delegate` / `worktree` / `sub_task` / `spawn`。Aider 是**单 agent loop + repo map**(`aider/repomap.py`)+ 编辑模式(Architect/Editor/Ask),没有 task delegation。**不适用本调研**,列出仅为完整性。

---

### E. opencode(sst/opencode)— Task tool 创建子 session,共享 cwd,无 worktree

**仓库**:`sst/opencode`(TypeScript,~18 万 stars,`dev` 分支)。核心:`packages/opencode/src/tool/task.ts`(346 行)+ `packages/opencode/src/agent/subagent-permissions.ts`。

#### E.1 Task tool = 创建子 session,不是子 process / 子 worktree

`task.ts` line 142-158:
```ts
const nextSession =
  session ??
  (yield* sessions.create({
    parentID: ctx.sessionID,
    title: params.description + ` (@${next.name} subagent)`,
    agent: next.name,
    permission: [ ...childPermission, ...childToolDenies ],
  }))
```

子 agent 跑在**独立的 session**(独立 message history / 独立 context window),但**共享同一个 cwd / 同一份 checkout**。全文件 0 处 `worktree` / `git` / `branch`。

#### E.2 改动回流 —— `<task_result>` XML 注入 parent

`task.ts` line 64-79(`renderOutput`)+ line 202-229(`injectBackgroundResult`):worker 的最终 text 被包成 `<task id="..." state="completed"><task_result>...</task_result></task>` XML,**作为 synthetic user message 注入 parent session**(background 模式)或作为 tool_result 回填(foreground 模式)。注意 line 213 `synthetic: true` —— 标记为合成消息,不污染真实用户消息流。

#### E.3 权限派生 —— `deriveSubagentSessionPermission`

`task.ts` line 125-128 + line 129-141:child session的 permission 是 **parent session permission ∩ subagent 定义 permission**,再叠 deny 规则(默认 deny `todowrite` + `task` 自身,防嵌套)。**对本项目的对应**:本项目 `filter_tools_for_subagent` + `force_readonly`(L3a)是 tool 层裁剪,opencode 是 permission rule 层裁剪,思路一致。

#### E.4 background 模式需 feature flag

`task.ts` line 98-102:`background: true` 需要 `OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS=true`,否则 fail。说明 background subagent 在 opencode 仍是实验特性 —— **与本项目的 L3b+(daemon 化)定位一致**,不是 MVP 范围。

---

### F. Cline — subagent 天然 read-only,无写冲突,不需要 worktree

**文档来源**:`https://docs.cline.bot/features/subagents`。

Cline 的 subagent 设计契约(文档原话):
> "Each subagent ... Can read files, search code, list directories, run read-only commands, and use skills. **Cannot edit files, use the browser, access MCP servers, or spawn nested subagents.** Returns a result focused on the most relevant file paths for the main agent to read next."

**因为 subagent 不能写文件,所以根本不存在 cwd 写冲突,不需要 worktree 隔离**。这跟本项目 L3a 的 `force_readonly=true` concurrent dispatch 走的是**完全相同的论证路径**(见 `.trellis/spec/backend/agent-loop-architecture.md` §"Race dissolution by scope" 表)。

**对本项目的启示**:Cline 的全 read-only 路径证明了"用 read-only 永久约束 subagent"是工业级可行方案 —— 但代价是 subagent 只能做 research,不能 implement。本项目 `general-purpose` subagent 保留写能力是差异化设计,因此才需要 L3b 的 worktree 隔离。

---

### G. 改动回流 3 模式横向对比(核心问题 4)

| 模式 | 代表工具 | 机制 | 适用场景 | 风险 / 代价 |
|---|---|---|---|---|
| **(a) worktree + commit 到独立分支**(PR 式) | **Claude Code**(`isolation: worktree`) | worker 在临时 worktree 跑,commits 落独立分支;worker 完成后 worktree+branch 作为产物保留;parent/用户决定 merge/cherry-pick/discard | **写场景**(implement / refactor / fix);需要可审查、可回滚、可并发 | worktree 创建/销毁开销(checkout);分支管理复杂度(残留分支需 sweep);base 选择敏感(origin/HEAD vs local HEAD);gitignored 文件需 `.worktreeinclude` 显式复制 |
| **(b) worker 产出 diff/patch,parent apply** | **理论模式,无主流工具采用** | worker 在隔离 cwd 写,结束后 `git diff` 产出 patch,parent 用 `git apply` 回流 | 理论上避免分支管理;parent 完全掌控 apply 与否 | patch 可能 conflict;binary 文件 / 大文件 diff 失败;apply 失败后 partial state 难恢复;**实际无工业级实现,验证成本高** |
| **(c) 共享 cwd 直接写 + file_state 追踪 + summary 自报告** | **Hermes**(sibling-write 警告)/ **opencode**(共享 session cwd)/ **本项目当前 B6** | worker 直接在 parent 的 checkout 里写;靠 `file_state` 写入追踪 + stale-read 警告 + summary 自报告兜底 | **read-heavy 或单 worker 串行写**场景;快速迭代不想要 worktree 开销;并发写**不安全** | 并发 worker 写冲突(无隔离);parent 读到 worker 写的半成品(stale-read 警告是事后诸葛,不阻止);summary 是 self-report 可能错(需 verifiable handle 二次验证) |

**模式选择的工业趋势**:写场景用 (a)(Claude Code 独家);read 场景用 (c) 的 read-only 退化版(Cline / 本项目 L3a);**没有主流工具选 (b)**。

---

### H. worktree 生命周期 + 并发(核心问题 5、6)

#### H.1 生命周期 —— 全部 per-run create+destroy,无 pool/复用

调研的所有工具(Claude Code / Hermes / opencode / 本项目 `git/worktree.rs`)**都是 per-run create + 完成后 destroy**,没有发现"worktree pool / 复用"模式。

- **Claude Code**:`cleanupPeriodDays` sweep + `git worktree lock`(agent 跑期间锁,防 sweep 误删)。"复用"只发生在用户显式 `cd` 回老 worktree 继续干(`EnterWorktree` 工具)—— 这是用户驱动,不是系统 pool。
- **Hermes**:per-run ThreadPoolExecutor,child `close()` 释放资源,无 worktree(不适用)。
- **本项目**:`git/worktree.rs::create` 已有完整的 self-heal(stale metadata / stale branch / orphan dir 三态清理,line 84-203),`destroy` best-effort prune + branch delete(line 270-362)。**这套机制可以直接复用给 per-worker worktree**。

**为什么没有 pool**:worktree create 的主开销是 `git checkout`(写 working tree 文件),对小 repo < 1s,大 repo 几秒 —— 相对 LLM round-trip(秒级到十秒级)可忽略。pool 的复杂度(空闲 worktree 归还、脏 worktree 重置、跨 session 共享)远超收益。**结论:L3b 应该走 per-run create+destroy,不要做 pool**。

#### H.2 竞态 / 残留风险 + 缓解

| 风险 | 缓解(Claude Code 路径) | 本项目已有/需要的缓解 |
|---|---|---|
| sweep 误删正在跑的 worker worktree | `git worktree lock`(跑期间锁) | **本项目需要加**:worker 跑期间对 worktree 上锁 |
| 崩溃残留 worktree + branch | self-heal(下次 create 时清)+ sweep by mtime | **本项目已就位**:`git/worktree.rs::create` 三态 self-heal(line 84-203) |
| 并发 worker create 撞同名 branch | branch 名带 worker_run_id(唯一) | **本项目需要**:worker worktree branch = `worker/<run_id>`(不复用 `session/<id>`) |
| destroy 撞 worker 还在写 | parent_token cancel 传播 + destroy 在 worker join 之后 | **本项目已有**:`worker_token = parent_token.child_token()`(dispatch.rs line 232) |

#### H.3 并发 worker + 各自 worktree(核心问题 6)

**可行性**:Claude Code 明确支持(每个 subagent 一个 `worktree-<value>`)。git worktree 本身支持任意数量的 linked worktree(共享 `.git/`),并发 create 是安全的(libgit2 `Repository::worktree` 串行化 metadata 写)。

**DB / 状态隔离如何处理**(映射本项目):
- **subagent_runs 表**:已有 `id`(worker_run_id UUID,dispatch.rs line 247-266),每 worker 一行,天然隔离。L3b 只需加一列 `worktree_path TEXT NULL` 记录 worker 的 worktree 路径(destroy 后置空或保留供事后审查)。
- **permission / run_grant**:已 per-worker 实例化(dispatch.rs line 354 `RunGrantCache::new()` per worker),无共享。
- **ReadGuard**:本项目 `read_guard` 是 session 级(dispatch.rs line 91 `read_guard: &ReadGuard`,继承 parent)。**worker worktree 隔离后,worker 的 read_guard 应该 reset**(因为 worker 在新 checkout 里,没有"已读文件"继承)—— 这是要重新论证的点。
- **memory_cache**:worker 复用 parent 的 MemoryCache slot(dispatch.rs line 207-208 `build_worker_messages`),这是 prompt 层共享,与 worktree 隔离正交,无需改。

**并发的真实约束不在 worktree,而在 parent turn 阻塞语义**:本项目 L3a 已经证明,N 个 read-only worker 并发靠 `FuturesUnordered`(L3a Pattern,`agent-loop-architecture.md` §"Concurrent readonly dispatch")。L3b 把 `force_readonly=false` + worker 各自 worktree 后,**写冲突被 worktree 隔离消解**,可以复用 L3a 的 `FuturesUnordered` 并发骨架 —— 这是 L3a → L3b 的自然演进路径。

---

## 映射到本项目:3 个可行 Approach

基于以上调研,给出 L3b 的 3 个候选 approach。**推荐 Approach 1(Claude Code 路线)**,理由见末尾。

### Approach 1:per-worker worktree + commit 留分支(Claude Code 路线)⭐ 推荐

**How**:
1. `run_subagent` 入口(dispatch.rs line 85)增加 `worker_worktree_path: Option<PathBuf>` 计算:若 subagent def 带 `isolation: worktree`(新增 frontmatter 字段,对标 Claude Code),则在 `dispatch_subagent` tool_use 触发时:
   - `git::worktree::create(parent_worktree, worker_wt_path, &worker_run_id)` —— branch = `worker/<worker_run_id>`,base = parent worktree HEAD;
   - worker 的 `ToolContext.worktree_path` 指向 worker_wt_path(而非 parent 的);
   - worker 跑期间 `git worktree lock`(防止并发 destroy);
   - worker 结束后:`git diff` + `git stash list` 判断有无 changes;
     - 有 changes → 保留 worktree + branch,把 `<worker run finished; changes left on branch worker/<run_id>; diff summary: <files>>` 追加到 dispatch_subagent tool_result;destroy 推迟到 session 删除或显式 `merge_worker` / `discard_worker` tool;
     - 无 changes → `git::worktree::destroy` 立即销毁(复用现有 `destroy` 函数)。
2. ReadGuard 在 worker 入口 **reset**(worker 新 checkout,无继承的已读文件)。
3. 并发场景(L3a concurrent 分支)把 `force_readonly` 闸门换成"每个 worker 各自 worktree",复用 `FuturesUnordered` 骨架。

**Pros**:
- 对标 Claude Code(ROADMAP 明确点名的工业级范本),harness 学习价值最大化;
- 真正消除并发写冲突 → 可以解锁 `general-purpose` worker 的并发写(L3a 当前被 `force_readonly` 锁死);
- worker 改动可审查、可回滚、可选择性合并 —— 跟本项目"session 级 worktree + diff"哲学一致(已有 `git/diff.rs` 复用);
- worktree 生命周期机制(self-heal / destroy)已在 `git/worktree.rs` 就位,增量小。

**Cons**:
- 需要新 frontmatter 字段 + subagent def 解析改动(`resource_loader.rs`);
- worker 的 base 选择(parent worktree HEAD vs project main HEAD)需要决策 —— 若 base = parent worktree,worker 继承 parent 的未提交改动(可能想,也可能不想);
- 分支管理复杂度:需要 sweep 机制(对标 Claude Code `cleanupPeriodDays`)或显式 merge/discard tool;残留 `worker/*` 分支会堆积;
- `.worktreeinclude` 等价物(复制 gitignored 文件如 `.env`)是 hidden cost,本项目当前 session worktree 是否处理这个要确认(见 `git/worktree.rs` —— 当前未处理 gitignored 文件复制,L3b 需补)。

### Approach 2:per-worker worktree + diff 回填 parent(无分支残留)

**How**:
1. 同 Approach 1 的 create + lock + worker 跑在隔离 worktree;
2. worker 结束后:**always destroy**,但 destroy 前 `git diff` 抓取 worker 改动,作为 structured payload(file list + unified diff,带 size cap)放进 `subagent_runs` 表 + 追加到 dispatch_subagent tool_result;
3. parent LLM 拿到 diff 后,自己决定是否用 `edit_file` / `write_file` 把改动 apply 回 parent worktree;
4. 不保留 worker branch,无 sweep 负担。

**Pros**:
- 无分支残留,无 sweep 机制需要;
- parent 完全掌控 apply 与否(对标模式 b 的理论优势);
- 复用本项目 `git/diff.rs::diff_worktree`(已就位,line 65)。

**Cons**:
- **diff apply 可能 conflict**(parent 在 worker 跑期间也改了同一文件);
- binary / 大文件 diff 处理弱;
- 把"apply 改动"的负担推给 parent LLM(可能 apply 错、apply 半截),失败模式比 Approach 1 多;
- **无工业级先例**(模式 b 在调研中 0 采用)—— harness 学习价值低,且自研风险高。

### Approach 3:保持当前共享 cwd + 强化 file_state 警告(Hermes 路线,不做 worktree)

**How**:
1. 不引入 worktree;worker 继续复用 parent worktree;
2. 在 `read_guard` 之上加一层"worker 写文件后,parent 读到 stale 时 warn"(对标 Hermes `file_state` sibling-write 检测,`delegate_tool.py` line 1899-1920);
3. 并发写场景继续靠 `force_readonly`(L3a 现状)锁死,不解锁并发写。

**Pros**:
- 增量最小(只加 stale-read 警告);
- Hermes 工业级范本(虽然 Hermes 也没完全解决并发写);
- 不碰 worktree 生命周期复杂度。

**Cons**:
- **不满足 ROADMAP L3b 标题"worktree 隔离"的明确诉求**(task 标题就是 "worktree 隔离 + 仿 Hermes delegate_task");
- 不解锁并发写(`general-purpose` worker 并发仍然 `force_readonly`);
- stale-read 警告是**事后**机制,不阻止冲突,只是告知 parent LLM "你刚读的可能被 worker 改了" —— 体验比 Approach 1 差;
- 与"旗舰级 harness 学习项"(ROADMAP §1.3 L3b 备注)定位不符。

---

## 推荐 + 理由

**推荐 Approach 1(per-worker worktree + commit 留分支,Claude Code 路线)**。

理由:
1. **ROADMAP 明确点名**"worktree 隔离 + 仿 Hermes delegate_task" + "旗舰级 harness 学习项" —— Approach 3 不满足"worktree 隔离";Approach 2 无工业先例。只有 Approach 1 同时对标 Claude Code(隔离)和 Hermes(delegate 语义)。
2. **机制已就位**:`git/worktree.rs`(create/destroy/self-heal)+ `git/diff.rs`(diff_worktree)+ L3a 的 `FuturesUnordered` 并发骨架 + per-worker `RunGrantCache` 隔离 —— 拼装即可,无需从零造。
3. **解锁真实价值**:Approach 1 是唯一能让 `general-purpose` worker 并发写(L3a 当前 read-only 锁死)的方案,这是 L3a → L3b 的核心演进动机。
4. **可分阶段降风险**:PR1 先做 serial-path 单 worker worktree(最小闭环)→ PR2 加并发(L3a concurrent 分支切换)→ PR3 加 sweep / merge_worker tool。不必一次性吞下。

**关键 caveats(实施前必须决策)**:
- **base 选择**:worker worktree 的 base = parent worktree HEAD(继承 parent WIP)还是 project main HEAD(干净 base)?Claude Code 默认 `origin/HEAD`(干净),`worktree.baseRef: head` opt-in 继承。本项目建议**默认 parent worktree HEAD**(worker 看到跟 parent 一样的状态,语义最直观),但需 spec 明确。
- **ReadGuard reset**:worker 在新 checkout,不能继承 parent 的已读集合(否则 edit_file 的"前置 3 道 check"会误判)。dispatch.rs 入口需 reset。
- **gitignored 文件**:本项目当前 session worktree 是否复制 `.env` 等 gitignored 文件?L3b 的 worker worktree 需要等价机制(对标 `.worktreeinclude`),否则 worker 跑测试会因为缺 `.env` 失败。**这一点 task.md 没提,实施前要 grill**。
- **merge / discard UI**:Approach 1 保留 branch,需要给用户提供合并/丢弃入口(对标 Claude Code 的 keep-or-remove prompt + `cleanupPeriodDays` sweep)。前端 `<SubagentDrawer>` 可能要加一个 "Merge worker changes" / "Discard" 按钮 —— 这是 L3b 的隐藏前端工作量。

---

## Caveats / Not Found

- **Claude Code 源码闭源**:以上 Claude Code 行为全部基于官方文档(`docs.claude.com/en/docs/claude-code/worktrees` + `/sub-agents` + `/agent-view`)。文档没有明确说 parent 是否**自动** merge worker 分支;从描述推断是**用户/parent 决策**(保留 branch 作为产物),但具体 merge 工具(是 `git merge` 还是 `cherry-pick` 还是 `cd 进去 squash`)文档未展开。**若 L3b 要严格对位,这一点需要 follow-up grill 或实测 Claude Code**。
- **Cursor 闭源 + 无公开 agent 文档**:Cursor 的 Composer / Agent 模式是闭源 SaaS,没有公开的 subagent/worktree 隔离文档。**信息不可得**,task 列出的 Cursor 项无法调研,如实标注。Cursor Checks(背景 agent)据公开博客跑在独立 cloud container,但这是 cloud session 隔离,不是 per-subagent worktree,且细节未公开。
- **Aider 无 subagent**:不适用,列出仅为完整性。
- **"worktree pool / 复用"模式**:调研的所有工具均未采用。**这种模式在 agent harness 业界不是常见做法**,如果用户 task 设想里有 pool 概念,需要重新评估其必要性。
- **Hermes 版本**:以上基于 `NousResearch/hermes-agent` `main` 分支(2026-06-27 抓取)。Hermes 迭代快,`delegate_task` 语义未来可能变。
- **本项目当前 session worktree 是否复制 gitignored 文件**:未在 `git/worktree.rs` 看到相关逻辑(只做 create/destroy/check_clean),L3b 实施前需确认 —— 若当前 session worktree 都没解决 `.env` 复制,L3b worker worktree 也继承这个问题。
