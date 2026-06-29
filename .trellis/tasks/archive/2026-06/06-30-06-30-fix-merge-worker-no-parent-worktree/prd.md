# PRD: merge_worker 在父 session 无 worktree 时自动 lazy-attach

> Priority: **P1**(正确性 + UX 双重绊脚石,影响所有需要 merge 的 isolated sub-agent)
> 关联 ADR: 见 `design.md §5`(lazy-attach-on-merge vs dispatch-time attach vs pure UX nudge 决策)
> 关联 spec: 见 `.trellis/spec/backend/worktree-contract.md`(attach/detach/delete 状态机)、`.trellis/spec/backend/tool-contract.md`(merge_worker 工具合约)
> 关联 task: 当前 task `.trellis/tasks/06-30-06-30-fix-merge-worker-no-parent-worktree/`
> 出错位置(根因 trace):
> - UI 路径:`app/src-tauri/src/commands/subagent_runs.rs:153` `session_row.session.worktree_path.as_deref().ok_or_else(|| "parent session has no worktree".to_string())?`
> - 工具路径:`app/src-tauri/src/tools/merge_worker.rs:287-290` `repo.find_branch(&parent_branch_name, ...).map_err(|e| format!("merge_worker: parent branch '{}' not found (parent session has no worktree?): {}", parent_branch_name, e))`
> - 父 worktree 为 None 的源头:`app/src-tauri/src/db/sessions.rs:49-54` `create_session` 显式不给 session attach worktree,只走 `attach_worktree` 显式触发

---

## Goal

修复 L3b (Worker Worktree) 与 Session Worktree 两条独立轨迹的**遗漏衔接**:
isolated sub-agent 可以在父 session 完全没有 worktree 的情况下被 dispatch(`general-purpose` 默认 `isolated=true`,`agent/subagent/dispatch.rs:108-115`),派生 `worker/<run_id>` 分支;但 merge 工具链 (`merge_worker_run` IPC + `merge_worker` tool) 都要求父 session 有绑定的 `session/<id>` branch 才能落点。两端各自能跑,**端到端走不通** —— 这是设计遗漏,不是 sub-agent 逻辑错。

**修法**: 在两个 merge 入口都加一层"父 session 无 worktree → 自动 `attach_worktree`" 的 lazy guard;同时 UI 给一个清晰的"我替你 attach 了父 worktree" 提示。**不**改 session 创建时的默认 worktree 策略(保留 Step4 opt-in 设计),**不**改 dispatch 时机(避免侵入 worker spawn 路径),**不**改 attach_worktree 的既有不变式(干净项目根、state machine guard、system event injection)。

---

## Why now

1. **触发率高**: 现状下,任何走 `general-purpose` sub-agent(默认 `isolated=true`)的用户一旦到 merge 阶段必撞这个错。除非用户读完 spec 后**主动**先去 chat header 点 "attach worktree",否则永远走不到 merge。这是普通用户的"看不见的墙"。
2. **误导性强**: 错误文案 `"parent session has no worktree"` 对没读过 worktree-contract 的用户是黑话。从 UI 看只是"点了合并按钮,没反应,弹错误";从 LLM 视角是"我啥也没干"`parent branch not found`。
3. **修复面小,但契约要锁**:lazy-attach 是把"父 session 状态可被外部动作改变"这条新不变式加进系统。如果不写 spec,下个做 worktree 改动的开发者会以为父 session 永远只受用户主动 attach 影响 → 引入新 bug。

---

## What I already know

- `app/src-tauri/src/db/sessions.rs:49-54` `create_session` —— session 起始 `WorktreeState::None`,`worktree_path=NULL`(Step4 follow-up 显式决定,opt-in 而非 auto)。
- `app/src-tauri/src/commands/worktree.rs:18-112` `attach_worktree` 完整实现 —— state guard (`None`/`Detached` 可入,`Active` 拒绝)、`check_clean(project_root)`、`git::create_worktree`、`db::set_worktree_state`、`db::insert_system_event`。 **副作用标准化**,可被其他 IPC 安全复用。
- `app/src-tauri/src/agent/subagent/dispatch.rs:108-115, 1082-1125` —— `resolve_isolation` + `create_worker_worktree` 链, `general-purpose` 默认 `isolation: Some(true)`,`researcher` 默认 `None`。worker 派生**完全独立**于父 session 的 worktree 状态。
- `app/src-tauri/src/commands/subagent_runs.rs:114-196` `merge_worker_run` —— UI 入口,153 行严格 `parent_wt.as_deref().ok_or(...)`,无 fallback。
- `app/src-tauri/src/tools/merge_worker.rs:102-213` `execute()` —— LLM 入口,184 行 spawn_blocking 前只在 `ctx.worktree_path` 上工作,**没有父 session 校验**(走 `ctx.worktree_path`,可能根本是 project root 而非 worktree)。
- `app/src-tauri/src/tools/merge_worker.rs:257-501` `do_merge_blocking` —— libgit2 实际 merge 体,287 行 `repo.find_branch(&parent_branch_name)` 失败即 errors out。
- `merge_lock_for(parent_session_id)` —— per-session merge 序列化锁,**两个入口都用**(tool + IPC),本身已经显示这个 lock 是共享不变式。

---

## Requirements

### Backend (Tauri IPC `merge_worker_run`)

- **新工具函数**: `merge_worker::ensure_parent_worktree_attached(db, parent_session_id) -> Result<Option<AttachNotice>, String>`
  - 入参: `&SqlitePool` + parent session id
  - 行为:
    1. `db::load_session` 检查父 session 的 `worktree_state` 和 `worktree_path`
    2. **若 Active**:返回 `Ok(None)`(已绑,不做事)
    3. **若 Detached**:返回 `Ok(None)`(已 detach 历史,不去尝试重 attach —— 保留用户意图)
    4. **若 None**:
       - 复用 `attach_worktree` 的内部逻辑(`check_clean` + `git::create_worktree` + `db::set_worktree_state` + `db::insert_system_event`),**不直接调 IPC**(IPC 走 `State` 注入,工具函数拿不到),而是在一个新 free function `git::worktree::attach_session(db, project, session_id) -> Result<PathBuf, GitError>` 里把 `attach_worktree` 的逻辑抽出来共享。
       - 成功 → 返回 `Ok(Some(AttachNotice { attached: true, branch: "session/<id>", path: <wt_path> }))`
       - 失败(项目非 git、项目根 dirty、libgit2 失败)→ 返回 `Err(原错误)`,**原样保留** `attach_worktree` 的错误文案(`"project not a git repository"` / `"project root has uncommitted changes; commit or stash before attaching"` / `"worktree creation failed: ..."`)
  - 抽 refactor 注意点:`attach_worktree` 命令体本身也改成调 `attach_session`,保持行为不变(契约测试守住)。
- `commands/subagent_runs.rs::merge_worker_run` 在 spawn_blocking 前(`line 156` `Stage 1` 前)插入:
  ```rust
  match crate::tools::merge_worker::ensure_parent_worktree_attached(&state.db, &parent_session_id).await {
      Ok(Some(notice)) => {
          tracing::info!(parent_session_id = %parent_session_id, branch = %notice.branch,
              "merge_worker_run: auto-attached parent worktree for merge");
          // 前端用:把 notice 通过 Ok 返回值带回? 不行——返回值是 String message
          // 见 §Frontend 协议变更 段
      }
      Ok(None) => {} // 父已绑或 detached,正常路径
      Err(e) => {
          // lazy attach 失败 → 透传 attach_worktree 的错误文案
          return Err(format!("merge_worker_run: cannot auto-attach parent worktree: {}", e));
      }
  }
  ```

### Backend (LLM-tool `merge_worker`)

- `tools/merge_worker.rs::execute()` 在 spawn_blocking `do_merge_blocking` 之前(`line 184` 前)插入同样的 `ensure_parent_worktree_attached` 调用。失败 → 直接返回 `isError=true` + 中文错误消息(同 `merge_worker_run` 的格式,工具面用 `(String, true, ToolContextUpdate::default(), None)` 元组)。
- **关键差异**:tool 路径下,`ctx.worktree_path` 可能是 project root(父未绑时),这没关系 —— 我们用 parent session id 而不是 `ctx.worktree_path` 来定位父,然后 lazy attach 用 session id 锁项目 → 创建独立 worktree path。
- `do_merge_blocking` 的入参 `parent_wt` 在 tool 路径下需要**在 attach 之后**重新查询(因为原来可能是 project root,attach 后才落到真正的 worktree path)。所以 tool `execute` 改:
  ```rust
  let _ = ensure_parent_worktree_attached(...).await?;
  let parent_wt = ctx.db  // 重新 load
      .load_session(...)
      .worktree_path; // 现在 Some(<新 path>)
  ```

### Shared helper 抽离

- `app/src-tauri/src/git/worktree.rs` 新增:
  ```rust
  pub async fn attach_session(
      db: &SqlitePool,
      project: &ProjectRow,
      session_id: &str,
  ) -> Result<PathBuf, GitError>
  ```
- 这函数做了 `commands/worktree.rs::attach_worktree` 的内层所有工作(不含 `cancel_inflight_for_session` / 不含 reload 返回 full `SessionRow` / 不含 `Result<SessionRow, String>` IPC envelope)。
- `attach_worktree` 命令体改成调 `attach_session` 然后 reload + return SessionRow,保留 IPC 合约。
- 单元测试: `git::worktree::tests::attach_session_*`(详见 Acceptance Criteria)—— 必须覆盖 attach 成功 / project non-git / dirty 项目的拒绝路径。

### Frontend (`WorkerMergeControls.vue` + `subagentRuns.ts`)

- **IPC 协议变更**: `merge_worker_run` 返回值改为结构化:
  ```rust
  #[derive(Serialize)]
  struct MergeWorkerResult {
      /// 原 libgit2 merge 结果文案("merged" / "fast-forwarded" / "merged with X commits" / "conflict: ..." 等)
      message: String,
      /// true 表示本次 merge 顺手做了父 worktree attach(给前端决定 toast 文案)
      auto_attached_parent: bool,
  }
  ```
  前端 store `mergeWorker(runId)` 把返回拆开 —— success 路径里如果 `auto_attached_parent === true`:
  - 调 `chatStore.refreshParentSession(parentSessionId)` 重新拉父 session 行,触发 chat header 的 worktree chip 变化(`worktree_state: none → active`)
  - toast 改成 `"已合并并自动绑定了父 session 工作区"`
- **冲突 / error 路径**:`mergeWorker` 已有的 `kind: 'conflict' | 'error'` 解析逻辑不变,只在 success toast 上分流。注意 libgit2 自身的 conflict 输出要走 `parseConflictFiles` 旧逻辑。
- **错误文案统一**: 父无 worktree 且 lazy attach 失败的链路下,IPC error 是 `merge_worker_run: cannot auto-attach parent worktree: project <name> is not a git repository` / `... uncommitted changes; commit or stash before attaching`。前端不在这个错误上特殊化,把原 `message` 直接 toast 出来即可(用户能看懂)。

### Spec update

- `app/src-tauri/src/.trellis/spec/backend/worktree-contract.md` 加新章节:
  - **"Pattern: Lazy auto-attach on merge"** —— 描述两个入口共享 helper、错误透传、Detached 不重 attach 的选择、SystemEvent 自动注入(已有,只需说明会被触发)。
- `app/src-tauri/src/.trellis/spec/backend/tool-contract.md` 加一行到 `merge_worker` 的合约:**"若父 session 未绑 worktree,自动 attach 一个 (session/<id>) 并注入 [worktree event] attached 系统消息"**。

---

## Acceptance Criteria

### Backend

- [ ] `cargo check` + `cargo test --lib` 全过(预计 +5 新 unit test 守住 `attach_session` 矩阵)。
- [ ] `git::worktree::attach_session::happy_path`:项目根干净 + `WorktreeState::None` → 成功创建 worktree + DB 状态变 `Active` + system event 行入表 + 返回 `PathBuf`。
- [ ] `git::worktree::attach_session::detached_path_skipped`:`WorktreeState::Detached` → 返回原 error(本函数不应被这类调用,触发即表面 bug;**强制 fail-fast**,不静默重 attach)。
- [ ] `git::worktree::attach_session::non_git_project`:`is_git_repo=false` → 错误携带 `"project not a git repository"`,DB 不变。
- [ ] `git::worktree::attach_session::dirty_project_root`:项目根有未提交变更 → 错误携带 `"uncommitted changes"`,DB 不变。
- [ ] `attach_worktree` 命令体的行为不变(契约 test):现有 call site 不动,只把内层抽到 helper。如果抽离引入回归,cargo test 守住。
- [ ] `merge_worker_run::auto_attach_parent_on_merge`:parent=`WorktreeState::None` + worker 派生完成 + 项目根干净 → 父先被 attach,do_merge_blocking 跑出 success,返回值 `MergeWorkerResult { auto_attached_parent: true }`。
- [ ] `merge_worker_run::auto_attach_skipped_when_parent_already_attached`:parent=`Active` → helper 返回 `Ok(None)`,走原 merge 路径,无额外 system event(已有的除外)。
- [ ] `merge_worker::execute::auto_attach_path`:LLM 调 `merge_worker` 同样路径触发现象同上,tool error tuple `(String, true, ToolContextUpdate::default(), None)`。
- [ ] `merge_worker::execute::auto_attach_error_propagates`:父 dirty 项目 + LLM 调 `merge_worker` → 工具返回 `is_error=true` + 中英错误文案,没有做过 attach。
- [ ] `merge_lock_for(parent_session_id)` 期间并发 merge 测试守住(已有,不变)。

### Frontend

- [ ] `pnpm build`(vue-tsc strict)+ `pnpm test` 全过。
- [ ] vitest: `subagentRuns.mergeWorker` 对 `auto_attached_parent=true` 返回值:触发 `chatStore.refreshParentSession` + toast 用新文案。
- [ ] vitest: `subagentRuns.mergeWorker` 对 `auto_attached_parent=false` 返回值:行为完全等同当前实现。
- [ ] vitest: `subagentRuns.mergeWorker` 对 `MergeWorkerResult { message: "<libgit2 错误带 conflict files>" }` 错误形状走 `parseConflictFiles` 旧逻辑,不变。
- [ ] 视觉/手测(acceptance A29-bis):
  1. 新项目(无 .worktree 状态)→ dispatch `general-purpose` sub-agent → 完成 → 看到 Merge/Discard 按钮
  2. 点合并 → 弹原生 ConfirmDialog → 确认 → **不**弹任何前置 "先 attach" 提示,直接走
  3. 完成后 toast 出现且文案 = `"已合并并自动绑定了父 session 工作区"`
  4. 切回 chat header 看父 session 的 worktree chip,**已**是 `Active` 状态(diff 计数 0,因为 worker 改的是 worker branch 自身,合并后 worker tip = parent tip)
  5. 重新打开同一个 worker drawer → 按钮已消失(因为 worker worktree 已销毁)

### Spec

- [ ] `worktree-contract.md` 加 **Pattern: Lazy auto-attach on merge** 段(200-400 字,含 invariants、错误矩阵、为什么 Detached 不重 attach)。
- [ ] `tool-contract.md` 的 `merge_worker` 条目合约段加一行说明 auto-attach 副作用。

---

## Definition of Done

- 修改 ~6 个文件:
  - `app/src-tauri/src/git/worktree.rs`(新 `attach_session` helper + 单元测试)
  - `app/src-tauri/src/commands/worktree.rs`(`attach_worktree` 命令体改成调 helper,行为不变)
  - `app/src-tauri/src/commands/subagent_runs.rs`(`merge_worker_run` 串入 helper + 返回值改 `MergeWorkerResult`)
  - `app/src-tauri/src/tools/merge_worker.rs`(`execute` 串入 helper + 重新查 parent_wt + 新 `ensure_parent_worktree_attached` helper)
  - `app/src/stores/subagentRuns.ts`(`mergeWorker` 拆 `MergeWorkerResult` + 触发 refresh + 文案分流)
  - `app/src/components/chat/WorkerMergeControls.vue`(新增 auto-attach 文案分支)
  - spec 文档 2 份(`worktree-contract.md` + `tool-contract.md`)
- `cargo test --lib` + `pnpm build` + `pnpm test` 三过
- 走标准 Trellis 收尾: PRD → archive → journal → commit
- DEBT.md 不新增条目(不在 P0/P1 债上是新发现,因为这是已有的 worktree state machine 设计遗漏,**不该污染 DEBT.md** —— 走 spec 跟 commit message)

---

## Out of Scope

- 改 `create_session` 默认 attach(worktree contract §Scenario 1 是显式决定 opt-in,本 PR 不推翻)
- 改 dispatch 时机自动 attach(parent session 状态变更侵入 worker spawn 路径,风险面更大,见 design §Alternative 评估)
- 加 commit 强约束(`answer "merge 时仍要求父 session 有 commit" = 接受 merge 会顺手帮父 attach`,所以不要 commit 约束)
- merge 冲突 UI 改造(已经能展示 conflict files,不在本 PR 范围)
- attach 失败时的 fallback 策略(让用户手动去 attach 不在本 PR 范围;只是给 actionable 错误透传)

---

## Future Work

(v2+ 候选,不在本 PR)

1. **Smart dispatcher (attach-lite)**: 在 `dispatch_subagent` tool 的 description 里加一句 hint:"if you intend to merge worker changes back to parent, ensure parent session has attached worktree (or use `attach_worktree` first)。LLM 主动 attach 后才 dispatch,避免 merge 时 lazy-attach 这一步被打断(项目根脏的情况下用户在 merge 时才被打断,体验不好)。
2. **UI 状态预显**: chat header 的 worktree chip 旁加 hover tooltip "this session has detached/never-attached worktree; clicks Merge on a sub-agent drawer will auto-attach。把 lazy 行为透明化。
3. **Configurable attach strategy**: 用户在项目设置里可调 "auto-attach-on-merge" / "require pre-attach" / "never auto-attach"。本 PR 行为 = auto-attach-on-merge 永远开,留 toggle 给 v2。
