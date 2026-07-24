# daemon graceful shutdown 超时修复（SSE 长连接挂起）

## Background

daemon epic（P2.1–P2.5）代码已全部落地。2026-07-23 手动测试 Session 暴露一个未修
代码后续项（记录在 `07-20-remote-access-daemon-split/implement.md` "已知后续项" 段）：

daemon 有浏览器 SSE 长连接（`GET /api/v1/stream`）挂起时，收到 SIGTERM 后
`axum::serve(...).with_graceful_shutdown(shutdown_signal())` 会等所有 in-flight
连接"完成"再退出。但 SSE 连接是**永不自然完成的**（`ReceiverStream` 持续 pending
直到客户端断开或 daemon 主动 drop sender），于是 graceful shutdown 无限挂起，实际
靠 `scripts/daemon.sh` 的 `SIGTERM → 8s → SIGKILL` 兜底清理。

- **复现**：daemon 跑 + 浏览器连上（SSE `/api/v1/stream` 活跃）→
  `./scripts/daemon.sh stop` → 日志有 `received SIGTERM, shutting down` 但进程
  8s 内不退出。
- **影响**：不影响功能正确性（SIGKILL 兜底最终能清理），但「优雅退出」名不副实 ——
  正常关闭路径多花 8s、SIGKILL 是粗暴手段、日志噪音。

## Goal

让 daemon 收到 SIGTERM/SIGINT 后，graceful shutdown 在**亚秒级**完成，不再依赖
`daemon.sh` 的 SIGKILL 兜底。

## 范围

### R1：`SseRegistry::shutdown()` —— 主动结束所有 live SSE 流

- 新增 `pub fn shutdown(&self)`：清空 `inner.senders`（`Vec::clear()`）。
  - 所有 `mpsc::Sender` drop → 对应 `ReceiverStream` 返回 `None` →
    `stream.rs` 的 SSE 流自然 `end()` → axum 感知连接完成 → graceful shutdown
    不再阻塞。
  - 复用既有"fan-out 剔除失败订阅者"的语义（`retain` 是逐个 drop，`clear` 是
    批量 drop，效果等价）。
  - 锁逻辑跟 `broadcast` / `subscribe` 一致（`Mutex::lock` + poison 容错）。
- 不动 `broadcast` / `subscribe` / replay buffer —— 它们只读不写，shutdown 后
  再 emit 的事件静默丢弃（daemon 已在退出，无消费者）。

### R2：`serve_daemon` 接入 shutdown —— 主动关 SSE + timeout 兜底

- 收到信号（`shutdown_signal()` 返回）后、`axum::serve.await` 自然结束前，
  先调 `state.sse.shutdown()` 主动结束所有 SSE 连接。
- 实现路径：用 `tokio::select!` 在 axum serve 与 shutdown signal 之间竞速，
  signal 触发后 `drop` serve handle / 调 `sse.shutdown()` 再 await serve 完成。
- **timeout 兜底**：用 `tokio::time::timeout(SHUTDOWN_GRACE, serve_future)` 包裹，
  超时则 log warning 后直接 return（进程退出）。默认 grace = 3s（比 daemon.sh
  的 8s SIGKILL 短，留 SIGKILL 作最后一道防线）。
  - 这是 defense-in-depth：正常情况 `sse.shutdown()` 后亚秒完成；timeout 仅防
    其他未知长连接（如未来加的非 SSE streaming endpoint）卡住。

### R3：清理过时注释

- `server.rs:170-173` docstring「P2.4 will add sidecar-aware shutdown」—— P2.4
  已落地但走的是 sidecar SIGTERM（`sidecar.rs` `RunEvent::Exit`），本注释描述的
  「Tauri window close → SIGTERM」已是当前行为（非"未来"），改为现状陈述。

## 非目标（Out of Scope）

- ❌ 不改 `scripts/daemon.sh` 的 SIGKILL 兜底（保留作最后一道防线）。
- ❌ 不加"shutdown 前推送一条 shutdown 事件给前端"的协议（daemon 已退出，
  EventSource 重连失败本就是预期；前端不依赖这条信号）。
- ❌ 不改 SSE replay buffer / 重连协议（仅结束 live 连接）。
- ❌ 不动 sidecar.rs 的 `RunEvent::Exit` 回收路径（GUI 关窗走 SIGTERM，
  本修复自然覆盖）。

## Acceptance Criteria

- [ ] `SseRegistry::shutdown()` 实现 + 单测（订阅者在 shutdown 后 live channel
      返回 `None`；subscriber_count 归零）。
- [ ] `serve_daemon` 收到信号后 `sse.shutdown()` + timeout 兜底，亚秒级退出。
- [ ] `cargo test`（含 `--lib`）全绿，新增测试覆盖 shutdown 路径。
- [ ] `cargo build` 0 warning（含本改动引入的）。
- [ ] 手动验证（复现路径）：daemon + 浏览器 SSE 连上 → `daemon.sh stop` →
      日志显示 SIGTERM 后 <1s 退出（不再等 8s SIGKILL）。
- [ ] `server.rs:170` 过时注释更新。

## Notes

- 这是轻量 bugfix，PRD-only，无 `design.md` / `implement.md`。
- 架构契合点：项目已有完整的 SSE 重连/resync 机制（`Last-Event-ID` +
  `stream-resync` sentinel + snapshot 重拉），所以 daemon 退出时"硬切"客户端
  SSE 连接是安全的 —— 浏览器 `EventSource` 重连失败本就是预期行为（daemon
  已退出），**SSE 层**不存在数据一致性风险。

## 已知未覆盖项（Follow-up，独立任务）

> 本任务只修了"SSE 长连接卡住 graceful shutdown"这一症状。daemon 退出时
> **正在跑的 agent loop 会被硬终止**，这是另一个独立问题，不属本任务 scope。

**现象**：`serve_daemon` 返回后 `main` 直接 `return ExitCode` → 进程退出 →
tokio runtime 销毁 → 所有 `tokio::spawn` 的 agent loop task 被直接丢弃。
若 agent loop 正处于"tool 执行完 → 还没 `persist_turn` 落库"之间，那一轮的
工具结果会丢。

**根因**：shutdown 路径没有任何代码触发活跃 agent loop 的 `CancellationToken`
或 await 它们 drain。`state.cancellations`（`HashMap<request_id, CancellationToken>`）
在 shutdown 时没被遍历 cancel。

**为什么本任务不修**：scope 膨胀（需引入 agent loop drain 编排 + 超时策略 +
落库一致性验证），与"SSE 卡 shutdown"是正交问题。本任务先交付 SSE 修复，
agent loop drain 作为 follow-up 任务单独规划。

**修法方向**（供 follow-up 任务参考）：在 `serve_daemon` 的 shutdown 路径
（`sse.shutdown()` 之后、进程退出之前）遍历 `state.cancellations` cancel 所有
活跃 request，再 await 它们的 task handle（带超时兜底）。

