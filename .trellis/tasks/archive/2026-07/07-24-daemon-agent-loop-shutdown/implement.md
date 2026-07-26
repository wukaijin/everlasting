# Implement: daemon shutdown agent loop drain

> 执行计划。配套 `prd.md`(需求) + `design.md`(技术设计)。
> 按 checklist 顺序执行,每个 `[ ]` 完成后可独立验证。

---

## 预备:读取上下文

实施前 `trellis-before-dev` 会注入 spec。关键 spec:
- `.trellis/spec/backend/daemon-server.md` —— 本任务要更新的契约(行 136-159)
- `.trellis/spec/backend/agent-loop-architecture.md` —— cancel 语义
- `.trellis/spec/backend/error-handling.md` —— RULE-A-011(persist 失败 tracing)

---

## Checklist

### Phase A: 批量 drain helper(R2)

- [ ] A1. 在 `app/src-tauri/src/agent/helpers.rs` 新增
      `cancel_and_drain_all_agent_loops`(签名见 design §2.2),紧挨
      `await_inflight_exit` 下方。
      - 锁内只 clone/take(不 await):clone 所有 `CancellationToken`、
        take 所有 `oneshot::Receiver`
      - 锁外:逐个 `.cancel()`;`futures::future::join_all` + `tokio::time::timeout`
        并发 await 所有 receiver
      - timeout 到 → log warn(参照 `await_inflight_exit` 的 warn 风格)+ return
- [ ] A2. 加 4 个单元测试(design §6.1):
      cancels_all_tokens / on_empty_is_noop / drains_done_signals /
      timeout_on_hung_sender(用 100ms 小 timeout)
- [ ] A3. **验证**:`cargo test -p everlasting --lib agent::helpers`
      (确认新函数单测绿,现有 cancel 测试未回归)

> Review gate A:helper 是后续 server.rs 改动的依赖,先独立验证再往上接。

### Phase B: server.rs shutdown 路径接 AppState(R1 + R3)

- [ ] B1. `daemon/server.rs` 新增常量 `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS: u64 = 8`
      (design §2.3 的 doc comment)
- [ ] B2. 改 `shutdown_signal` 签名:`Arc<SseRegistry>` → `Arc<AppState>`。
      内部:原 `registry.shutdown()` 改 `state.sse.shutdown()`;
      新增 `cancel_and_drain_all_agent_loops(&state.cancellations,
      &state.inflight_exits, Duration::from_secs(DAEMON_SHUTDOWN_LOOP_DRAIN_SECS)).await`
      (在 sse.shutdown 之后)。
- [ ] B3. 改 `serve_daemon` 的调用点:`with_graceful_shutdown(shutdown_signal(
      Arc::clone(&state)))`(原来是 `Arc::clone(&state.sse)`)。
- [ ] B4. 更新 `serve_daemon` + `shutdown_signal` 的 doc comment,反映新的
      4 步 shutdown 顺序(design §3)。
- [ ] B5. **验证**:`cargo build -p everlasting-daemon`(确认编译;
      此时无活跃 loop,行为退化为现状 + 空 drain)

> Review gate B:server.rs 是核心改动,编译通过 + 现有 SSE shutdown 测试不回归
> 才进 Phase C。

### Phase C: daemon.sh SIGKILL 窗口(R4)

- [ ] C1. `scripts/daemon.sh` `do_stop` 的循环 `for _ in 1 2 3 4 5 6 7 8`
      改成 15 次迭代(15s 窗口)
- [ ] C2. 同步更新循环上方注释「8s」→「15s」,并说明为何拉长
      (8s loop drain + 3s SSE grace = 11s,留 4s 余量)

### Phase D: 集成测试(R5)

- [ ] D1. 在 `daemon/server.rs` 的 `tests` mod 新增
      `serve_daemon_shutdown_drains_active_agent_loop`(design §6.2)。
      采用方案 (b):直接往 `state.cancellations` 塞挂起 token + 对应
      `inflight_exits` receiver,测 drain 机制 + shutdown 不挂。
- [ ] D2. 断言:`serve_daemon` 在 `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS*2 + 5`s
      内返回;且 token 已被 cancel。
- [ ] D3. **验证**:`cargo test -p everlasting-daemon daemon::server`
      (新测试 + `serve_daemon_shutdown_completes_with_active_sse` 都绿)
- [ ] D4. **验证回归**:`cargo test -p everlasting-daemon daemon::sse`
      (SSE shutdown 机制未破坏)

> Review gate D:集成测试是核心回归守卫。若方案 (b) 说服力不足(persist_turn
> 落库不变量没被 daemon 路径直接验证),升级到 design §6.2 方案 (a)——给
> AppState 加 test-only provider override。先跑 (b) 看 reviewer 反馈。

### Phase E: 全量验证(2.2 quality check)

- [ ] E1. `cargo test -p everlasting-daemon`(daemon crate 全测)
- [ ] E2. `cargo clippy -p everlasting-daemon -- -D warnings`
- [ ] E3. `cargo fmt --check`(若 fail 先 `cargo fmt` 再确认 diff 合理)
- [ ] E4. 手动(可选):`./scripts/daemon.sh start` + 触发一个长任务 +
      `./scripts/daemon.sh stop`,确认日志有 cancel 记录且进程在 ~11s 内退出

### Phase F: spec 更新(Phase 3.3)

- [ ] F1. `trellis-update-spec` skill:`daemon-server.md` 行 136-159 的
      「⚠️ 已知未覆盖:agent loop 硬终止(待 follow-up)」段改写为
      「已覆盖:agent loop drain」+ 实际修法 + 决策记录
- [ ] F2. 校正 spec 原文「修法方向」措辞:原写「await 对应 task handle」,
      实际复用了 `done_tx`/`inflight_exits`(design §8 决策)

### Phase G: 提交(Phase 3.4)

- [ ] G1. `git status --porcelain` 快照
- [ ] G2. 按逻辑分组提交(server.rs + helpers.rs + daemon.sh + 测试 + spec)
- [ ] G3. `/trellis:finish-work`

---

## Rollback Points

- A 失败 → helper 本身问题,只影响新函数,revert A 的 commit
- B 失败 → shutdown 路径编译/行为问题,revert B;A 的 helper 可保留(独立有价值)
- D 失败 → 集成测试构造不出来,退回 design §6.2 方案 (a) 或只留单测层覆盖
- 任何阶段 → 改动集中(4 个文件 + 测试),单 commit revert 即可回退

---

## 风险点(实施时注意)

1. **锁内不 await**:`cancellations`/`inflight_exits` 是 `tokio::Mutex`,A1
   实施时严格守「锁内 clone/take,锁外 cancel/await」纪律。
2. **`join_all` + timeout 的语义**:要对**所有** future 套总 timeout,不是
   每个 future 各自 timeout。正确写法:`timeout(总T, join_all(all_receivers))`。
   `join_all` 里某个 receiver 卡住不会拖垮 others(join_all 等所有),但总
   timeout 会兜底。
3. **panic path 的 receiver**:`done_tx` 因 panic 被 drop(未 send),receiver
   收 `Err`,`join_all` 的 `Result` 里是 `Err` —— 视为「已退出」,不 panic。
4. **daemon.sh 的 15s 是 operator 可感知变化**:改注释说明「为等 agent loop
   落库而拉长」,避免后续误改回 8s。
