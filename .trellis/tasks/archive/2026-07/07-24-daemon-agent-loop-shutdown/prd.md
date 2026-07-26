# daemon shutdown: agent loop 硬终止 drain（cancel + persist_turn 落库一致性）

## Background

`.trellis/spec/backend/daemon-server.md` 行 136-159 记录的 follow-up。前序任务
`07-24-daemon-graceful-shutdown` 闭合了「SSE 长连接挂起 graceful shutdown」,
但**只覆盖 SSE**,**不覆盖**正在跑的 agent loop task。这是已知缺口,记录于
spec 以免误判「daemon 优雅退出已完整」。

**现状**:`serve_daemon`(`daemon/server.rs:198`)收到 SIGTERM 后只调
`SseRegistry::shutdown()` 关 SSE,然后 `tokio::time::timeout(3s, serve)` 让
axum drain。`serve_daemon` 返回后 `main` 直接 `return ExitCode` → 进程退出 →
tokio runtime 销毁 → `chat.rs:327` 的 `tokio::spawn` agent loop task 被直接
丢弃。`state.cancellations`(`state.rs:105`)在 shutdown 时**没有**被遍历 cancel。

**风险**:agent loop 正处于「tool 执行完 → 还没 `persist_turn` 落库」之间被斩断,
那一轮的工具结果会丢 —— 已发 SSE 给前端,但 DB 没落,用户刷新页面后丢一轮。
daemon 作为 sidecar 被前端拉起/关停时会真实命中(用户关 app 时 agent 正在跑)。

## Goal

daemon 收到 SIGTERM/SIGINT 后,**正在跑的 agent loop 能干净退出**(走 cancel
路径 + 完成 in-flight tool 的 `persist_turn` 落库),再让进程退出 —— 而非被
runtime 销毁硬斩。闭合 spec 记录的数据一致性缺口。

## Requirements

### R1: shutdown 路径 cancel 所有活跃 agent loop

`serve_daemon` 收到信号后的顺序,在 `sse.shutdown()` 之后、进程退出之前,新增:
遍历 `state.cancellations` 对所有 `CancellationToken` 调 `.cancel()`。

- agent loop 的 `select!` cancel 臂 `biased;` 优先命中(`chat_loop.rs:1716`/
  `2401`),设 `cancelled=true` 后正常走 post-loop persist 路径退出 —— 与用户
  点 Stop 按钮走的是**同一条已验证路径**(`tests_cancellation.rs` 覆盖)。
- `run_chat_loop` / agent loop 本体 **零改动**:它已为 cancel 设计好。

### R2: 显式并发 await 所有 loop 的退出信号

cancel 后**并发** await 所有 `state.inflight_exits` 里的 `oneshot::Receiver`
(`chat.rs:314` 的 `done_tx`/`done_rx`),带一个**总 timeout** 兜底。

- **复用而非新引 task handle**:不存 spawn handle。`chat.rs:478` 的
  `done_tx.send(())` 在**所有 exit path**(normal/error/cancel/panic-drop)
  触发,且语义比 spawn handle 更准 —— 它在「in-flight tool 也完成后」才 send
  (`run_chat_loop` 返回 = loop 完全退出),正是我们要等的不变量。
- 并发而非串行:否则 N 个 loop 各拖满 timeout,shutdown 会被无限放大。
- 批量语义与 `helpers::await_inflight_exit`(单 loop,10s)一致,避免两套 drain
  逻辑漂移。

### R3: agent loop drain 总 timeout = 8s

并发 await 的总 timeout 取 **8s**。

- 对照 `await_inflight_exit` 单 loop 的 10s;并发 drain 理论上比串行快,
  8s 是纯兜底(实测路径下 loop 多在亚秒退出)。
- shutdown 总顺序:`signal → sse.shutdown() → cancel 所有 loop → 并发
  await drain(8s) → axum drain(3s SSE grace) → 进程退出`,最坏 11s。

### R4: 同步拉长 `scripts/daemon.sh` 的 SIGKILL 窗口到 15s

R3 的 8s drain + 3s SSE grace = 11s,**超过** `daemon.sh:115` 现有的 8s
SIGTERM→SIGKILL 循环。若不改 daemon.sh,SIGKILL 会在 drain 完成前抢先斩断
loop —— 等于 R1/R2/R3 白做。

- 把 `do_stop` 的循环从 `1 2 3 4 5 6 7 8`(8 次 × 1s)拉长到 15 次(15s),
  相对 11s 最坏路径留 4s 余量。
- 同步更新循环上方注释里的「8s」→「15s」。

### R5: 集成测试(核心回归守卫)

新增集成测试,复用 `tests_agent_loop.rs::agent_loop_cancel_in_turn_2_kills_loop`
的成熟模式:

- turn 1 `tool_use`(`list_dir`,Tier 5 默认放行)→ 执行 + persist tool_result
  + 重入 turn 2
- turn 2 `HangingThenCancel`(`mock.rs:327`,流永久 pending,只能被 cancel
  唤醒)→ 这是模拟「tool 执行后、下轮 persist 前」的精确窗口
- 触发 shutdown drain 路径,断言:
  - drain 在 8s 总 timeout 内返回(不挂起)
  - **turn 1 的 tool_result 已落库**(`SELECT ... FROM messages`,沿用现有
    `query_as` 惯例)—— 这正是 follow-up 要保护的「不丢一轮」不变量

`MockResponse::HangingThenCancel` 提供了精确的「在 tool 执行后挂起」语义,
无需自己造。

### R6: 更新 spec(Phase 3.3)

实施后把 `daemon-server.md` 行 136-159 的「⚠️ 已知未覆盖」段改写为
「已覆盖」+ 实际修法,并校正 spec 原文「修法方向」措辞(原写「await 对应
task handle」,实际复用了 `done_tx`/`inflight_exits` 现成信号,更准)。

## Acceptance Criteria

- [ ] `serve_daemon` shutdown 路径在 `sse.shutdown()` 之后遍历 `cancellations`
      cancel 所有活跃 loop
- [ ] cancel 后并发 await 所有 `inflight_exits`(总 timeout 8s),用 `done_tx`
      而非 spawn handle
- [ ] `run_chat_loop` / agent loop 本体零改动
- [ ] `scripts/daemon.sh` SIGKILL 窗口拉长到 15s,注释同步更新
- [ ] 新增集成测试:turn-1 tool_use + turn-2 HangingThenCancel,断言 shutdown
      drain 在 timeout 内返回 **且** turn 1 tool_result 已落库
- [ ] `cargo test -p everlasting-daemon daemon::server` + `daemon::sse`(回归)
      通过
- [ ] `cargo clippy` + `cargo fmt --check` 通过
- [ ] `daemon-server.md` follow-up 段改写为「已覆盖」

## Scope / 非目标

**改动范围**:
- `app/src-tauri/src/daemon/server.rs` —— shutdown 路径接 `AppState`,cancel + drain
- `app/src-tauri/src/agent/helpers.rs` —— 新增 `await_all_inflight_exits` 批量函数
- `app/src-tauri/src/agent/tests_*.rs` —— 新增集成测试
- `scripts/daemon.sh` —— SIGKILL 窗口 8s → 15s

**非目标**:
- 不动 `run_chat_loop` / agent loop 本体(已为 cancel 设计好)
- 不动 SSE shutdown 机制(正交,已交付)
- 不动 `chat.rs` 的 spawn 闭包(`done_tx.send` 已覆盖所有 exit path)
- 不改单 loop 的 `await_inflight_exit` 10s(destructive-command 路径仍用它,
  本任务只新增批量版本)

## 风险

低。cancel+drain 模式已被 destructive-command 路径(`delete_session` /
`detach_worktree` / `delete_worktree`)+ `tests_cancellation.rs` 充分验证;
改动集中在 shutdown 路径编排 + 一个批量 helper + 一个集成测试 + 一个 shell
超时常量。
