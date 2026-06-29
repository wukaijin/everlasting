# Implement: merge_worker lazy auto-attach

> 配套 PRD: `prd.md`,design:`design.md`(同目录)
> 执行计划 + review gates

---

## 1. 步骤

### Step 1: 抽 helper `git::worktree::attach_session`

**Why first**: helper 是后面所有改动的根基;`attach_worktree` 命令体立刻改成调它(契约保持),后续 merge 两个入口都调它。先把基础打稳,后面的步骤风险递减。

文件:
- `app/src-tauri/src/git/worktree.rs` —— 新增 `pub async fn attach_session(...)` + 5 个单元 test
- `app/src-tauri/src/commands/worktree.rs` —— `attach_worktree` 命令体重构为调 helper

具体改动:
1. 复制现有 `attach_worktree` 命令体的内层 `line 47-99` 块,放进 `git::worktree::attach_session`
   - 不接 `State`,只接 `db: &SqlitePool, project: &ProjectRow, session_id: &str`
   - 返回 `Result<PathBuf, GitError>`,错误信息保留原命令体的格式(`"is not a git repository"` / `"uncommitted changes"` / `"worktree creation failed: ..."`)
2. `attach_worktree` 命令体改成"加载 row + 调 attach_session + reload + return"
   - 在 helper 之外额外做 `cancel_inflight_for_session`(IPC 边界专属,见 design §6.3/6.4)
3. 加单元测试(`#[cfg(test)]` 块):
   - `attach_session_happy_path`(项目根干净 + `worktree_state=None`)
   - `attach_session_active_state_rejected`
   - `attach_session_detached_state_rejected`
   - `attach_session_non_git_project`
   - `attach_session_dirty_project`
   - 跑测试时若需要 fixture(项目根干净 / 脏),复用 `git::worktree::tests` 已有的 repo 初始化 pattern

**Review gate**:
- `cargo check` 过
- `cargo test --lib`(含 git/worktree 模块的 5 个新 test)全过
- 现有 `attach_worktree` IPC contract 不破(若有现成 test 守住):跑现有 test 验证
- 跑 `git log --diff-filter=A --name-only` 确认新增 imports 没漏链

### Step 2: 新增 helper `tools::merge_worker::ensure_parent_worktree_attached`

文件:
- `app/src-tauri/src/tools/merge_worker.rs` —— 在 merge_worker 模块顶部加 pub 函数 + 3 个单元 test

具体改动:
1. 加 `ensure_parent_worktree_attached(db: &SqlitePool, parent_session_id: &str) -> Result<bool, String>`
2. 三态分支(INV-M2/M3/M4):
   - `Active` → `Ok(false)`
   - `Detached` → `Ok(false)`
   - `None` → 调 `crate::git::worktree::attach_session(...)`,返回 `Ok(true)`
3. 加单元 test:
   - `attached_no_op_when_active`
   - `detached_no_op_skipped`
   - `lazy_attach_on_none`(调 `attach_session`,断言 DB state + system event 落表;需要 fixture)

**Review gate**:
- `cargo test --lib` 过

### Step 3: `commands::subagent_runs::merge_worker_run` 串入 helper + 改返回值

文件:
- `app/src-tauri/src/commands/subagent_runs.rs` —— 改 IPC 接口

具体改动:
1. 新增 `pub struct MergeWorkerResult { message: String, auto_attached_parent: bool }`(serde camelCase)
2. `merge_worker_run` 函数:
   - 第 140-153 行的 `parent_wt.as_deref().ok_or(...)` 整段移除(原 hard-fail 改为新的 lazy 路径)
   - 在 spawn_blocking 前插入 `ensure_parent_worktree_attached` 调用,把返回转成 `auto_attached` flag
   - 成功路径: 返回 `Ok(MergeWorkerResult { message: msg, auto_attached_parent: auto_attached })`
   - 失败路径(原 libgit2 错误):保持 `Err(String)`
   - 新错误路径(lazy attach 失败):返回 `Err(format!("merge_worker_run: cannot auto-attach parent worktree: {}", e))`
3. 不动 `discard_worker_run`

**Review gate**:
- `cargo check` 过
- 单测覆盖:`merge_worker_run::auto_attach_parent_on_merge`(IPC 端到端,有现有 fixture 复用即可;无则 skip,优先在 helper 层测)

### Step 4: `tools::merge_worker::execute()` 串入 helper

文件:
- `app/src-tauri/src/tools/merge_worker.rs`

具体改动:
1. 在 execute() spawn_blocking `do_merge_blocking` 之前(184 行前),插入同 Step 3 的 `ensure_parent_worktree_attached` 调用
2. 错误路径:`return (format!("merge_worker: cannot auto-attach parent worktree: {}", e), true, ToolContextUpdate::default(), None)`
3. **关键改动**:lazy attach 成功后,`parent_wt` 需要**重新查询**(`do_merge_blocking` 入参)。原来从 `ctx.worktree_path` 取(可能是 project root),attach 后才落到真正的 worktree path,所以 reload `db::load_session(parent_session_id).session.worktree_path`

**Review gate**:
- `cargo test --lib` 过(可能涉及 `merge_worker::do_merge_blocking` 既存 test 不回归)

### Step 5: 前端 IPC 类型更新

文件:
- `app/src/stores/subagentRuns.types.ts` —— 新增 `MergeWorkerResult` 类型
- `app/src/stores/subagentRuns.ts` —— 改 `mergeWorker` 函数

具体改动:
1. `subagentRuns.types.ts`:
   ```typescript
   export type MergeWorkerResult = {
     message: string;
     autoAttachedParent: boolean;
   };

   export type MergeResult =
     | { kind: "success"; autoAttachedParent?: boolean }
     | { kind: "conflict"; files: string[] }
     | { kind: "error"; message: string };
   ```
2. `subagentRuns.ts::mergeWorker(runId, parentSessionId?)`:
   - 第二参数可选(向下兼容)
   - 接 IPC 返回 `MergeWorkerResult`,在 success 分支跑 `parseConflictFiles(result.message)` + 若 `autoAttachedParent=true` 调 `chatStore.refreshSession(parentSessionId)`
   - 改造既有 `try/catch` 结构:`try` 走 `MergeWorkerResult`,`catch` 仍走 `parseConflictFiles`

**Review gate**:
- `pnpm vue-tsc --noEmit`(等价 `pnpm build` 的 type check 阶段)过
- `pnpm test` 过(含 `subagentRuns.test.ts` 现有 + 新增分支测试)

### Step 6: 前端 UI 文案分流

文件:
- `app/src/components/chat/WorkerMergeControls.vue`

具体改动:
1. `defineProps` 加 `parentSessionId: string`(必需,因为 drawer 已经从 run 行拿得到)
2. `doMerge()`:
   ```typescript
   const result = await store.mergeWorker(props.runId, props.parentSessionId);
   if (result.kind === "success") {
     conflictFiles.value = null;
     const msg = result.autoAttachedParent
       ? "已合并到父 session 分支,并自动绑定了父工作区"
       : "已合并到 session 分支";
     projects.showToast(msg, "info");
   }
   // ... 其它分支不变
   ```
3. `SubagentDrawer.vue`(从 `WorkerMergeControls` 调用方)传 `parent-session-id` prop(从 `run.parentSessionId` 取)

**Review gate**:
- 视觉:在 dev 模式下,新项目 + dispatch general-purpose + 完成 → 点合并 → 看到 `"已合并到父 session 分支,并自动绑定了父工作区"` toast
- chat header 的 worktree chip 翻到 `Active`
- worker drawer 按钮自动消失(因 worktreePath→null)

### Step 7: spec 同步

文件:
- `.trellis/spec/backend/worktree-contract.md` —— 加新章节(从 design.md §7.1 复制)
- `.trellis/spec/backend/tool-contract.md` —— `merge_worker` 条目加一行

**Review gate**:
- 文档 grep:`grep "Lazy Auto-Attach" .trellis/spec/backend/worktree-contract.md` 命中

---

## 2. Review gates 总览(每步必过)

| Step | 必跑 | 通过标志 |
|---|---|---|
| 1 | `cargo check` + `cargo test --lib` | 5 个新 test 全过,attach_worktree IPC contract 不破 |
| 2 | `cargo test --lib` | 3 个新 test 全过 |
| 3 | `cargo check` + `cargo test --lib` | 编译过,merge_worker_run 单测(若加)过 |
| 4 | `cargo test --lib` | merge_worker 相关 test 不回归 |
| 5 | `pnpm build`(vue-tsc strict)+ `pnpm test` | 类型过,vitest 全过 |
| 6 | `pnpm build`(vue-tsc strict)+ `pnpm test` | 同上 + 视觉手测 A29-bis |
| 7 | `grep -r "Lazy Auto-Attach" .trellis/spec/` | 命中 |

---

## 3. 验证命令(汇总)

```bash
# Backend
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo check

cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib

# Frontend
cd app && pnpm build
cd app && pnpm test

# Spec 锁
grep -l "Lazy Auto-Attach" .trellis/spec/backend/*.md
```

---

## 4. 回滚点

每步独立:
- Step 1 revert: helper 删了不影响(原 attach_worktree 内联逻辑若未替换则保留)
- Step 5 revert: 前端改回 `Result<string, string>` 即可,IPC 同步回到旧版
- 整体 revert: `git revert <commit>` 一条命令,无 schema 迁移 / DB 列新增

若中途发现 cancel hook 不足(in-flight race 没防住):
- 退到 Step 4 之后追加 cancel hook;不在本 PR 范围,但留出位置(design §6.4)

---

## 5. Commit 策略

按工作流"四段式 commit"指引(memory:trellis-task-finish-commit-pattern.md):

| Commit | 范围 | message |
|---|---|---|
| 1 | Step 1: helper 抽出 + 5 unit test | `refactor(git/worktree): extract attach_session helper for tool-layer reuse` |
| 2 | Step 2: ensure_parent_worktree_attached + 3 unit test | `feat(merge_worker): ensure_parent_worktree_attached helper for lazy parent attach` |
| 3 | Step 3+4: 两个 merge 入口串入 helper + IPC 返回值变更 | `fix(merge): auto-attach parent session worktree when None to support isolated sub-agent merge` |
| 4 | Step 5+6: 前端 IPC 类型 + UI 文案 | `fix(ui): surface auto-attach parent worktree toast + refresh chat chip` |
| 5 | Step 7: spec 同步 | `docs(spec): add Pattern: Lazy Auto-Attach on Merge to worktree-contract + tool-contract` |

注:实际 commit 频率看 review 反馈,trellis-check 在每个 commit 前跑(per `trellis-check` workflow)。

---

## 6. Out of scope reminders(从 PRD 拷贝来)

- 不动 `create_session` 默认 attach 策略
- 不动 dispatch 路径
- 不加 commit 强约束
- 不改 merge conflict UI
- 不加 fallback UI 给 lazy attach 失败的场景(只 actionable error 透传)
