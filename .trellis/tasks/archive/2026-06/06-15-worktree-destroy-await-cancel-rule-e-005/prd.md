# worktree destroy await cancel 生效 (RULE-E-005)

## Goal

P1 债务 RULE-E-005：`delete_worktree`（及同类 destructive 命令）在调用
`cancel_inflight_for_session` 后**立即**执行 `git::destroy_worktree` 删目录，但
`cancel_inflight_for_session` 只 `token.cancel()`（设标志），**不等待 agent loop 真正退出**。
cancel 检查在 tool 执行**之后**（`chat_loop.rs:670`，RULE-A-004），故 cancel 触发时
最多还有一个 in-flight tool 会跑完——若此刻目录已被删，写盘撞 ENOENT / panic /
残留 fingerprint 指向已删文件。

**修法**：`cancel_inflight_for_session` 返回一个"agent loop 退出信号"，destructive
命令 `await` 该信号（带防御性 timeout）后再执行删除。

## What I already know (auto-context)

- `cancel_inflight_for_session`(`agent/helpers.rs:191-219`) 当前签名
  `(cancellations, session_active_request, session_id) -> ()`，body 只 `t.cancel()` 后 return。
- 3 个调用方：`delete_worktree`(`commands/worktree.rs:197`)、
  `detach_worktree`(`commands/worktree.rs:119`)、`delete_session`(`commands/sessions.rs:126`)。
- `chat` 命令(`agent/chat.rs:158`) `tauri::async_runtime::spawn` 闭包调
  `run_chat_loop(...).await`，**JoinHandle 被丢弃**——外部无任何机制得知 loop 退出。
- cancel 粒度：`chat_loop.rs:369` select! biased 于 `token.cancelled()`（stream 事件间）；
  tool 执行**后** `:670` 再查一次（RULE-A-004）。故窗口 ≤ 一个 in-flight tool 的执行时长。
- `CancellationGuard`(`state.rs:348`) Drop 时 fire-and-forget spawn 清 `cancellations[rid]`
  + `session_active_request[sid]` 两个 entry。
- 既有 cancel 单测(`agent/tests.rs:185/211/230`) 用裸 `Arc<Mutex<HashMap<String, CancellationToken>>>`
  直接驱动 helper（无 Tauri State）。
- `cancel_chat`(`commands/cancel.rs:21`) 只读 token 取消，**不需**退出信号——不动。
- spec `backend/worktree-contract.md:236` Ordering invariant 当前为
  "cancel → destructive execute → system event"，**本修复改写为
  cancel → await 退出 → destructive → system event**，必须同步更新。

## Assumptions (temporary)

- 用 `tokio::sync::oneshot` 作退出信号（runtime-agnostic，规避 `tauri::async_runtime::JoinHandle`
  类型不可直接存 `Mutex<HashMap>` 的不确定性）。
- 独立新增 `inflight_exits` map（不动 `cancellations` 值类型）→ 涟漪最小：
  `cancel_chat` / `run_chat_loop` / `CancellationGuard` / `TestHarness.cancellations` 全不动。

## Mechanism (decided)

- `AppState` 新增 `inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>`（rid → receiver）。
- `chat.rs`：建 `(done_tx, done_rx)`，spawn 前 `inflight_exits.insert(rid, done_rx)`；
  闭包 `run_chat_loop().await` 后 `let _ = done_tx.send(())` +
  `inflight_exits.lock().remove(&rid)`（cancel_inflight 已 take 则 no-op）。
- `cancel_inflight_for_session`：增加 `inflight_exits` 参数，取消 token 后
  `inflight_exits.lock().remove(&rid)`（take receiver，单消费者），返回 `Option<oneshot::Receiver<()>>`。
- 调用方：`if let Some(rx) = cancel_inflight_for_session(...).await { let _ = tokio::time::timeout(Duration::from_secs(10), rx).await; }`
  （timeout 兑现"防御性"——loop 卡死时 delete 仍能进行，log warn）。

## Scope (locked, 2026-06-15)

**三者都 await**（用户确认）：`delete_worktree` + `detach_worktree` + `delete_session`。
理由：三者共享同一 helper、同一类 race（cancel 后 destructive op 立即跑，loop 仍可能
执行一个 in-flight tool）；只修 delete_worktree 会让 detach/delete_session 保留同类
残留 bug（in-flight 写入已解绑/已删 session）。helper 改一次，三家每处 +2 行受益，
spec Ordering invariant 一次统一。

## Requirements (locked)

- `cancel_inflight_for_session` 增加 `inflight_exits` 参数，返回 `Option<oneshot::Receiver<()>>`。
- chat.rs spawn 闭包 `run_chat_loop().await` 后 `done_tx.send(())` + `inflight_exits.remove(&rid)`。
- `delete_worktree` / `detach_worktree` / `delete_session` 三处在 destructive 工作前
  `await` 返回的信号（`tokio::time::timeout(10s, rx)`，超时 log warn + 仍进行）。
- `AppState` 新增 `inflight_exits` 字段，`load` 初始化空 map。
- 既有 3 个 cancel 单测（`cancel_inflight_for_session_*`）补第 4 参数（空 exits map）。
- 新增 1 个单测：返回的 receiver 在 producer send 前 pending、send 后才 resolve。

## Acceptance Criteria (locked)

- [ ] `cancel_inflight_for_session` 返回退出信号；新单测证明"先 pending、send 后才 resolve"。
- [ ] `delete_worktree` / `detach_worktree` / `delete_session` 三处 await 该信号（带 timeout）。
- [ ] chat.rs spawn 闭包 send 退出信号 + 清理 exits entry（正常完成 + cancel 双路径）。
- [ ] `AppState.inflight_exits` 字段 + `load` 初始化。
- [ ] `PKG_CONFIG_PATH=... cargo check` 0 warning；`cargo test --lib` 全 pass（含改造的 3 个 + 新增 1 个单测）。
- [ ] spec `worktree-contract.md` Ordering invariant 更新为 "cancel → await 退出 → destructive → event"。
- [ ] DEBT.md RULE-E-005 闭合（Status closed + Closed At + Re-evaluation Log）。

## Definition of Done

- 单测 added/updated（无集成测试框架依赖 Tauri State，单测覆盖机制）。
- `cargo check` 0 warning + `cargo test --lib` 全 pass。
- spec + DEBT.md 同步更新。
- 不引入前端改动（category 不变、事件不变）。

## Out of Scope

- agent loop 内部 cancel 粒度调整（不修 `:670` 之后逻辑）。
- `cancel_chat`（Stop 按钮）路径改动——它只取 token，无需退出信号。
- `inflight_exits` 持久化（进程内即可，重启清空合理）。
- 跨进程 daemon 化后的信号传递（路线图后续档）。

## Technical Notes

- 不复用 JoinHandle：`tauri::async_runtime::JoinHandle<()>` 跨 map 存储的类型/await 语义不如 oneshot 直白。
- receiver 单消费者：`cancel_inflight` `.remove().take()` 取走后，并发第二个 destructive 调用拿 `None`（退化，可接受）。
- timeout=10s：覆盖最长单 tool 执行；超时 log warn + 仍 delete（用户显式要求删除，不永久阻塞）。
- `done_tx` 即使 loop panic 也会 Drop → receiver 收 `Err(RecvError)`，timeout(await) 仍 resolve，不卡死 delete。
