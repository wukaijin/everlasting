# Daemon HTTP Server Contract

> axum `everlasting-daemon` 进程的运维契约:serve loop、graceful
> shutdown、SSE 长连接生命周期。对应代码 `app/src-tauri/src/daemon/`。
>
> daemon 化落地于 2026-07-20~23(remote-access epic),架构见
> [docs/ARCHITECTURE §4](../../../docs/ARCHITECTURE.md),编排放
> [docs/REMOTE-ACCESS-ROADMAP.md](../../../docs/REMOTE-ACCESS-ROADMAP.md)。

---

## Scenario: Graceful Shutdown 与 SSE 长连接

### 1. Scope / Trigger

**触发条件**:任何修改 `serve_daemon` 的 shutdown 路径,或新增
streaming endpoint(SSE / WebSocket / chunked transfer)时。

**问题根源**:axum 的 `with_graceful_shutdown` 在收到信号后,会等
**所有 in-flight 连接自然完成**才退出进程。但 SSE 长连接
(`GET /api/v1/stream`)是**永不自然完成**的 —— 它的 body 是一个
`ReceiverStream`,只要 daemon 不主动 drop sender,就永远 pending。

结果:有活跃 SSE 连接时,`with_graceful_shutdown` 无限挂起,靠
`scripts/daemon.sh` 的 `SIGTERM → 8s → SIGKILL` 兜底清理(2026-07-23
手动测试暴露,2026-07-24 修复,task `07-24-daemon-graceful-shutdown`)。

### 2. Signatures

```rust
// daemon/sse.rs
impl SseRegistry {
    /// 主动结束所有 live SSE 流。清空 senders → 每个 ReceiverStream
    /// 返回 None → stream body 自然 end() → axum 感知连接完成。
    pub fn shutdown(&self);
}

// daemon/server.rs
const SHUTDOWN_GRACE_SECS: u64 = 3;

/// signal 收到后,先调 sse.shutdown() 再让 axum drain。
async fn shutdown_signal(registry: Arc<SseRegistry>);

/// serve + with_graceful_shutdown + 外层 timeout 兜底。
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()>;
```

### 3. Contracts

shutdown 顺序(收到 SIGINT/SIGTERM 后):

1. `shutdown_signal` 的 `tokio::select!` 命中(ctrl_c / SIGTERM)
2. **`registry.shutdown()`** —— drop 所有 SSE sender,结束所有 live stream
3. `shutdown_signal` 返回 → axum `with_graceful_shutdown` 开始 drain
   此时 SSE 流已结束,只剩可快速 drain 的短请求
4. `tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve)` 兜底 ——
   正常亚秒完成;超时则 log warn 后 return(进程退出),`daemon.sh`
   SIGKILL 是最后一道防线

### 4. Validation & Error Matrix

| 条件 | 行为 |
|------|------|
| 无 SSE 连接时 SIGTERM | drain 亚秒完成(SHUTDOWN_GRACE 内) |
| 有活跃 SSE 连接时 SIGTERM | `sse.shutdown()` 结束 stream 后 drain 亚秒完成 |
| 未来加了未知长连接卡住 | SHUTDOWN_GRACE_SECS(3s) 超时后强制 return;daemon.sh 8s SIGKILL 兜底 |
| `shutdown()` 后再 `broadcast` | 静默丢弃(daemon 已退出,无消费者),不 panic |
| `shutdown()` 对空 registry | no-op,不 panic |
| `shutdown()` 重复调用 | idempotent |

### 5. 关键不变量

- **必须用 `with_graceful_shutdown`**,不能改回 `select! { serve, signal }`。
  drop `axum::serve` 的 future 会 **abort** 所有连接 task(粗暴断开),
  丢失正在处理中的请求;`with_graceful_shutdown` 才是 drain 语义。
  (2026-07-24 实现中途踩过这个坑:`select!` 未命中分支 drop serve future
  导致连接被 abort 而非 drain。)
- **`shutdown()` 必须在 `shutdown_signal` 返回前调**,在 axum 开始 drain
  之前结束 SSE,否则 drain 仍会被卡。

### 6. Tests Required

- `daemon::sse::tests::shutdown_clears_all_subscribers` — subscriber_count 归零
- `daemon::sse::tests::shutdown_ends_live_channel` — live channel 返回 None(**核心**)
- `daemon::sse::tests::shutdown_on_empty_registry_is_noop`
- `daemon::sse::tests::shutdown_is_idempotent`
- `daemon::sse::tests::shutdown_silently_drops_post_shutdown_broadcast`
- `daemon::server::tests::serve_daemon_shutdown_completes_with_active_sse` —
  **真实 TCP + 真实 SIGTERM 集成测试**:有活跃 SSE 连接时,`serve_daemon`
  必须在 `SHUTDOWN_GRACE_SECS * 2 + 2` 内返回(回归守卫,若 shutdown
  机制被破坏会超时失败)

### 7. Wrong vs Correct

#### Wrong — `select!` + drop serve future

```rust
// ❌ drop serve future 会 abort 所有连接,丢失 in-flight 请求,
//    且 SSE 连接被硬切(客户端看到连接中断而非正常结束)。
tokio::select! {
    res = axum::serve(listener, router) => return res,
    _ = shutdown_signal() => { state.sse.shutdown(); }
}
// serve future 已被 drop,无法继续 drain。
```

#### Correct — `with_graceful_shutdown` + signal 内调 shutdown

```rust
// ✅ axum 的 drain 机制正常工作;signal 内先 sse.shutdown() 让 SSE
//    流自然结束,drain 才不会被永不完成的连接卡住。
let serve = axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal(Arc::clone(&state.sse)));
tokio::time::timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), serve).await
```

---

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

## ⚠️ 已知未覆盖:agent loop 硬终止(待 follow-up)

> 本契约的 graceful shutdown **只覆盖 SSE 长连接**,**不覆盖**正在跑的
> agent loop。这是已知缺口,记录于此避免未来误判"daemon 优雅退出已完整"。

**现状**:`serve_daemon` 返回后 `main` 直接 `return ExitCode` → 进程退出 →
tokio runtime 销毁 → 所有 `tokio::spawn` 的 agent loop task(`chat.rs:327`)
被直接丢弃。`state.cancellations`(`HashMap<request_id, CancellationToken>`)
在 shutdown 时**没有**被遍历 cancel。

**风险**:若 agent loop 正处于"tool 执行完 → 还没 `persist_turn` 落库"之间
被斩断,那一轮的工具结果会丢(已发 SSE 给前端,但 DB 没落)。

**为什么现在不修**:与"SSE 卡 shutdown"是正交问题,scope 不同。SSE 修复先
交付(2026-07-24,本任务);agent loop drain 编排 + 落库一致性验证作为
独立 follow-up 任务。

**修法方向**(follow-up 任务):
1. `serve_daemon` 的 shutdown 路径,`sse.shutdown()` 之后、进程退出之前,
   遍历 `state.cancellations` cancel 所有活跃 request
2. await 对应的 task handle(带超时兜底,避免某个卡住的 agent loop 拖死
   整个 shutdown)
3. 加集成测试:活跃 agent loop(mock provider 跑长任务)时 shutdown,
   验证 `persist_turn` 在 shutdown 前完成或被正确标记为中断
