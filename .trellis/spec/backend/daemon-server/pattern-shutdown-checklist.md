<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->

## Pattern: 新增 streaming endpoint 的 shutdown 检查

**问题**:未来加任何 streaming endpoint(WebSocket、SSE、chunked),
都会重新撞上"长连接永不自然完成 → graceful shutdown 卡住"。

**检查清单**(新增 streaming endpoint 时):

- [ ] 该 endpoint 的 stream body 在 shutdown 时能否被主动结束?
- [ ] 是否在 `shutdown_signal` 里加了对应的结束调用(类比 `sse.shutdown()`)?
- [ ] 是否加了超时兜底测试(活跃连接时 shutdown 在 grace 内完成)?

若 stream body 持有 `mpsc::Receiver`,则在 `shutdown()` 里 `clear()` senders
即可(同 `SseRegistry` 模式）。若是其他载体(如 `WebSocket`),需对应的
close 路径。

---

## ✅ 已覆盖:agent loop drain(闭合硬终止缺口)

> 本契约的 graceful shutdown **同时覆盖** SSE 长连接 **和** 正在跑的
> agent loop。原「⚠️ 已知未覆盖」缺口已于 2026-07-24 闭合(task
> `07-24-daemon-agent-loop-shutdown`)。

**原缺口**:`serve_daemon` 只 `sse.shutdown()` 关 SSE,不 cancel 正在跑的
agent loop → 进程退出时 tokio runtime 销毁直接丢弃 `chat.rs` 的 spawn task,
落在「tool 执行完 → 还没 `persist_turn` 落库」窗口的那一轮结果会丢(已发
SSE 给前端,DB 没落)。

**闭合方式**:`shutdown_signal` 在 `sse.shutdown()` 之后,**复用**
destructive-command 路径(`delete_session` / `detach_worktree` /
`delete_worktree`)早为同一问题造好的 cancel+drain 基础设施,把「单 session
的 cancel+drain」搬到 shutdown 路径,粒度改成「所有 session」:

1. 遍历 `state.cancellations` 对所有 `CancellationToken` 调 `.cancel()` ——
   agent loop 的 `select!` cancel 臂 `biased;` 优先命中,走用户点 Stop 的
   **同一条已验证路径**。
2. 并发 await `state.inflight_exits` 的所有 `oneshot::Receiver`(总 timeout
   `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS = 8s` 兜底)。复用 `chat.rs` 的
   `done_tx.send(())` 信号(在 `run_chat_loop` **返回后**才触发,即 loop
   完全退出含 in-flight tool 完成 + persist_turn 落库)—— **而非新引
   spawn handle**,语义更准且复用现成设施。
3. 编排落在 `agent::helpers::cancel_and_drain_all_agent_loops`(单 loop 版
   `cancel_inflight_for_session` + `await_inflight_exit` 的批量对应),保证
   drain 语义单源、不漂移。

**shutdown 顺序(收到 SIGINT/SIGTERM 后)**:
```
signal → sse.shutdown() → cancel_and_drain_all_agent_loops(8s)
       → axum drain(SHUTDOWN_GRACE_SECS=3s) → 进程退出
```
最坏 8s + 3s = 11s,故 `scripts/daemon.sh` 的 SIGTERM→SIGKILL 窗口从 8s
拉到 **15s**(留 4s 余量),保证 SIGKILL 永远是「等不过来」的最后手段而非
抢先于 drain。

**关键不变量(本场景特有)**:
- **必须先 `sse.shutdown()` 再 cancel loop**:先断流、再停处理,语义更干净;
  且 SSE 关后前端不再收到新事件,也不会有新 chat request 到达。
- **cancel + drain 必须在同一调用内原子两步**:中间窗口里新进来的 request
  会漏 cancel。`cancel_and_drain_all_agent_loops` 封装成原子两步。
- **锁内只 clone/drain,不 await**:`cancellations` / `inflight_exits` 是
  `tokio::Mutex`,锁内若 await 会阻塞该 map 的所有其他使用者。

**测试**:
- `daemon::server::tests::serve_daemon_shutdown_drains_active_agent_loop` —
  真实 TCP + 真实 SIGTERM + 植入「卡死的活跃 loop」(永不 resolve 的
  oneshot),断言 `serve_daemon` 在 drain timeout + grace 内返回(回归守卫)
  且 token 已被 cancel。
- 「persist_turn 真的落库」不变量由 `tests_agent_loop.rs::
  agent_loop_cancel_in_turn_2_kills_loop` 在 agent loop 层单测覆盖
  (daemon 路径注入 provider 成本过高,见该任务 design.md §6.2 方案 (b))。
- 两个 SIGTERM 测试共享 `SIGNAL_TEST_MUTEX` 串行(同进程信号不可并发)。


---
