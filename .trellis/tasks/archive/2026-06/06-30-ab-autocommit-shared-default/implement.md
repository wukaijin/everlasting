# Implement — A+B: worker auto-commit + isolation 系统层自动决定

> Task: `06-30-ab-autocommit-shared-default` · 顺序执行；每步独立可 revert。

## 执行 checklist

### A — auto-commit 兜底
- [ ] **A1** `git/worktree.rs` 新增 `commit_worker_changes(worker_wt, run_id) -> Result<Oid, String>`（git2 `add_all`+`write`+`write_tree`+`commit`，借鉴 `merge_worker.rs:605-630`）；带单测（empty / 有改动 / untracked 文件 三种）。
- [ ] **A2** `dispatch.rs`：`probe_worker_changes` 返回 `has_changes=true` 后（`dispatch.rs:957` 附近）调 `commit_worker_changes`；失败 `warn!` + 保守保留 worktree，不阻断。
- [ ] **A3** 单测：worker 不主动 commit → A2 后 `worker_tip` 领先于 base；`merge_worker` 后 parent worktree 含改动（断言 diff 非空）。

### B — isolation 系统层自动决定（写型才隔离）
- [ ] **B1** `dispatch.rs`：新增 `worker_is_writable(def) -> bool`（toolset 空=继承含写→true；含写工具→true；纯只读→false；复用 `filter_tools_readonly` 只读集）；`run_subagent` 加 `parallel: bool` 参（`dispatch.rs:252` 附近）；决策逻辑 `dispatch.rs:346-353` 改为 `force_readonly → false / (parallel && writable) → true / else resolve_isolation(不变)`。**`resolve_isolation` 签名不动。**
- [ ] **B2** `mod.rs:410`：`general-purpose` `isolation: Some(true)` → `None`。
- [ ] **B3** `chat_loop.rs` 接线：`DispatchBatch::Concurrent` 调用点（`2216`）传 `parallel=true`；`Serial` 调用点（`2371`）传 `parallel=false`。同步修正过时注释（`2095-2118`、`2235-2247`、`2364-2369`、`2396-2401`）。
- [ ] **B4** `mod.rs`：改写 `dispatch_subagent` description（`119-142`）+ schema `isolation` 说明（`160-169`）；删旧「defaults to isolated / pass false」表述，加「单 dispatch 共享；多 dispatch 写型自动隔离；无需手动指定」。
- [ ] **B5** 测试：`resolve_isolation` truth table（`dispatch.rs:1137-1174`）**签名未变，保留**；新增 `worker_is_writable` 用例 + 决策逻辑用例（并发写型→isolated / 并发只读→shared / serial→def 默认）+ Concurrent 集成测试（两写型 worker 改同一文件 → 各落自己分支、不 race）。

## 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app && pnpm build   # tool description 改动不触及前端，保险跑一次 type-check
```

## Review Gate（start 前必过）

- design §4 效果矩阵逐条对照代码（写型隔离等价；只读共享=现状不变；`force_readonly` serial-only 不变）。
- A3 + B5 全部新增测试绿。
- 无现有 `resolve_isolation_*` / `probe_worker_changes_*` / `do_merge_blocking` / `is_parallel_eligible` / `l3a_*` 回归。

## Rollback Point

- A（A1-A3）与 B（B1-B5）互相独立：A 出问题可单独 revert A2（保留 helper A1 死代码无害）；B 出问题可 revert B1-B4 不影响 A。
- 全部 revert = 恢复 L3b PR2 现状（general-purpose 默认 isolated + 假成功 bug 仍在，但行为与改动前一致）。

## 依赖 / 出栈

- 本任务不依赖 child2（C+D）。child2 的 C 提示文案提及「auto-commit」，软依赖本任务 A 已落地——child2 implement.md 已注明。
