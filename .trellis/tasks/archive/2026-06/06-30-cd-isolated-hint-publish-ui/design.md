# Design — C+D: isolated 上下文注入 + publish session→main UI

> Task: `06-30-cd-isolated-hint-publish-ui` · Parent: `06-30-subagent-worktree-smooth`
> Depends on child1 (A+B): C 的提示文案提到「auto-commit」，依赖 A 的 `commit_worker_changes` 已落地。

## 1. 范围与边界

**改动文件**
- `agent/subagent/dispatch.rs` — C（`ISOLATION_HINT` 常量 + 包装 `task`）
- `tools/merge_session.rs`（新）— D（`merge_session_into_main` git helper，借鉴 `tools/merge_worker.rs::do_merge_blocking`）
- `commands/worktree.rs` — D（`publish_session_to_main` Tauri command）
- `lib.rs` — D（注册 command）
- 前端 `stores/chat.ts` + chat header 组件 — D（`publishSessionToMain` + 按钮）

**不改**
- `build_worker_messages`（C 只在其调用点包装 `task` 字符串，不动函数）
- `do_merge_blocking`（child1 已验证；D 写独立 helper，不泛化它以免回归）

## 2. C — delegation_task 注入 worktree 知情提示

**注入点**：`dispatch.rs:504` `build_worker_messages(memory_cache, &project_id, &project_path, task)` 之前。

**实现**：
```rust
const ISOLATION_HINT: &str = "\
[environment] You are running in an ISOLATED git worktree on branch \
`worker/<run_id>`. Your file edits land on that branch, are \
auto-committed by the system when you finish, and the parent agent \
will merge them back. You do NOT need to run `git commit` yourself — \
focus on the task.";

let final_task: String = if isolated {
    format!("{task}\n\n---\n{ISOLATION_HINT}")
} else {
    task.to_string()
};
let worker_messages =
    build_worker_messages(memory_cache, &project_id, &project_path, &final_task).await;
```

- **仅 `isolated=true`** 时追加；shared（含并发只读 worker）不注入（无需知情）。
- 提示**不要求** worker 自己 commit（commit 由 A 的 `commit_worker_changes` 兜底）。
- `<run_id>` 用实际值替换（或保留占位，worker 不需要精确 id）——实现时用 `worker_run_id` 插值。

## 3. D — publish session→main

**3.1 git helper**（`tools/merge_session.rs`）
```rust
pub fn merge_session_into_main(project_path: &Path, session_id: &str) -> Result<String, String>
```
- `Repository::open(project_path)`（project main repo，`.git/` 所在）。
- resolve `main` tip（`refs/heads/main`；项目默认分支）+ `session/<session_id>` tip（`git::worktree::branch_name`）。
- **FF**：`main` 是 `session/<id>` 祖先 → 移 `main` ref 到 session tip（`repo.reference(..., force=true)`），不写 merge commit。
- **3-way**：`repo.merge(&[session_annotated], ...)` → 冲突则 `has_conflicts()` 报错（含冲突文件列表）+ reset 到干净 HEAD（**不留半合并脏状态**，借鉴 `merge_worker.rs:555-602`）→ 无冲突则写 merge commit on `main`。
- 借鉴 `do_merge_blocking` 的 FF/3-way/冲突处理结构，但 branch ref 是 `main` + `session/<id>`（不是 `session/<parent>` + `worker/<run_id>`）。
- **不 push。** 仅本地 `main` 前进；session worktree 不动（用户可继续在 session 工作）。

**3.2 Tauri command**（`commands/worktree.rs`）
```rust
#[tauri::command]
pub async fn publish_session_to_main(state, session_id) -> Result<String, String>
```
- load session + project；校验 `worktree_state == Active`（无 worktree → 报错提示先 attach）。
- 校验 project main path 是 git repo。
- `spawn_blocking` 调 `merge_session_into_main(project_path, session_id)`。
- 返回 merge 结果字符串（FF/3-way/冲突）。

**3.3 前端**
- `stores/chat.ts`：`publishSessionToMain(sessionId)` → `invoke("publish_session_to_main", { sessionId })`，成功后 toast + 刷新 chat chip（对齐 `attachWorktree` 的 post-action 模式）。
- chat header worktree 控制区（`ChatPanel.vue` / `WorkerMergeControls` 附近）加「Publish → main」按钮，仅 `worktree_state == Active` 时显示；冲突时 toast 显示冲突文件。

## 4. 兼容性 / Rollout / Rollback

- **C** 只在 isolated 路径加 prompt 段，shared/并发只读路径完全不变。
- **D** 纯新增（helper + command + 注册 + UI），不改任何现有行为。
- **Rollback**：C 删 `ISOLATION_HINT` + 包装；D 删 helper/command/注册/UI。
- **不 push remote**（D 仅本地 merge）。

## 5. 验证 / Review Gate

- C：isolated worker 收到的 task 含提示（测试断言）；shared 不含。
- D：`merge_session_into_main` 单测（FF 前进 / 3-way 干净合并 / 冲突报错且不留脏状态）；不触发 push。
- `cargo test --lib` 全绿；`pnpm build` 前端 type-check。
