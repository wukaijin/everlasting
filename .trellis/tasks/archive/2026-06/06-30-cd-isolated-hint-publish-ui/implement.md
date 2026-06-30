# Implement — C+D: isolated 上下文注入 + publish session→main UI

> Task: `06-30-cd-isolated-hint-publish-ui` · C 与 D 互相独立，可分开实现/验证。

## 执行 checklist

### C — delegation_task 注入 worktree 提示
- [ ] **C1** `dispatch.rs`：加 `ISOLATION_HINT` 常量；在 `build_worker_messages` 调用前（`dispatch.rs:504`）按 `isolated` 包装 `task`（`<run_id>` 用 `worker_run_id` 插值）。
- [ ] **C2** 测试：isolated dispatch → worker messages 末尾含提示关键字（如 "ISOLATED git worktree"）；shared dispatch → 不含。

### D — publish session→main
- [ ] **D1** `tools/merge_session.rs`：`merge_session_into_main(project_path, session_id)`（FF/3-way/冲突，借鉴 `merge_worker.rs::do_merge_blocking`）；单测覆盖 FF 前进 / 干净 3-way / 冲突报错不留脏状态。
- [ ] **D2** `commands/worktree.rs`：`publish_session_to_main` command（load session+project、校验 Active+git repo、`spawn_blocking` 调 helper）。
- [ ] **D3** `lib.rs`：注册 `publish_session_to_main`。
- [ ] **D4** 前端：`stores/chat.ts` `publishSessionToMain` + chat header「Publish → main」按钮（仅 Active 显示，冲突 toast）。
- [ ] **D5** 前端测试：`WorkerMergeControls` 风格的 invoke 断言（click → invoke `publish_session_to_main`）。

## 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app && pnpm build
```

## Review Gate（start 前必过）

- C 仅 isolated 注入；shared/并发只读不变。
- D `merge_session_into_main` 三路径（FF/3-way/冲突）正确，冲突不留半合并脏状态，**全程不 push**。
- 前端按钮 + 状态刷新，不破坏现有 worktree/merge UI。

## Rollback Point

- C（C1-C2）与 D（D1-D5）互相独立：C 出问题删包装；D 出问题删 helper/command/注册/UI。互不影响，也不影响已交付的 child1（A+B）。

## 依赖

- 软依赖 child1 A（`commit_worker_changes`）—— C 提示文案提「auto-committed」，A 已落地（cda336c）。
