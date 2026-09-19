<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->


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
const SHUTDOWN_GRACE_SECS: u64 = 3;   // 设计参考锚点,不再用于 serve_daemon

/// signal 收到后,先调 sse.shutdown() 再 drain 活跃 agent loop,最后返回。
async fn shutdown_signal(state: Arc<AppState>);

/// serve + with_graceful_shutdown。**不能**给整个 serve future 套
/// `tokio::time::timeout` —— 会变成「无信号也 3s 自杀」(2026-07-27 修过)。
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()>;
```

### 3. Contracts

shutdown 顺序(收到 SIGINT/SIGTERM 后):

1. `shutdown_signal` 的 `tokio::select!` 命中(ctrl_c / SIGTERM)
2. **`registry.shutdown()`** —— drop 所有 SSE sender,结束所有 live stream
3. `shutdown_signal` 返回 → axum `with_graceful_shutdown` 开始 drain
   此时 SSE 流已结束,只剩可快速 drain 的短请求
4. `serve.await` 自然完成(进程随后退出)。**没有外层 timeout** ——
   drain 的硬上限由 `shutdown_signal` 内部的两步保证(见 §关键不变量);
   万一仍有未知长连接卡住,由 `daemon.sh`(standalone)/ GUI `RunEvent::Exit`
   的 SIGKILL 兜底

### 4. Validation & Error Matrix

| 条件 | 行为 |
|------|------|
| 无 SSE 连接时 SIGTERM | drain 亚秒完成 |
| 有活跃 SSE 连接时 SIGTERM | `sse.shutdown()` 结束 stream 后 drain 亚秒完成 |
| 未来加了未知长连接卡住 | `daemon.sh`(standalone)或 GUI sidecar 的 SIGKILL 兜底;**进程内不再用 timeout 提前 return**(历史教训:给整个 `serve` 套 timeout 会让 daemon 在无信号时也 N 秒自杀,见 §7) |
| **没有 SIGTERM/SIGINT** | `serve` 永久服务请求(进程不退出)—— **必须如此**,任何给 `serve` 套 deadline 的写法都是 bug |
| `shutdown()` 后再 `broadcast` | 静默丢弃(daemon 已退出,无消费者),不 panic |
| `shutdown()` 对空 registry | no-op,不 panic |
| `shutdown()` 重复调用 | idempotent |

### 5. 关键不变量

- **必须用 `with_graceful_shutdown`**,不能改回 `select! { serve, signal }`。
  drop `axum::serve` 的 future 会 **abort** 所有连接 task(粗暴断开),
  丢失正在处理中的请求;`with_graceful_shutdown` 才是 drain 语义。
  (2026-07-24 实现中途踩过这个坑:`select!` 未命中分支 drop serve future
  导致连接被 abort 而非 drain。)
- **`serve_daemon` 里禁止给 `serve` 套 `tokio::time::timeout` 或
  `tokio::select!`**。`serve` 在无信号时永久运行是**正确的**(正常服务);
  任何 deadline 都会让 daemon 在 N 秒后无信号自杀(2026-07-27 的回归就是
  `tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve)`,详见 §7「错(二)」)。
  drain 的硬上限交给 `shutdown_signal` 内部(`sse.shutdown()` +
  `cancel_and_drain_all_agent_loops`),进程级 SIGKILL 兜底。
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

#### Correct — `with_graceful_shutdown` + signal 内调 shutdown,**不**套外层 timeout

```rust
// ✅ axum 的 drain 机制正常工作;signal 内先 sse.shutdown() 让 SSE
//    流自然结束,drain 才不会被永不完成的连接卡住。
let serve = axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal(Arc::clone(&state)));
serve.await?;   // 没有 tokio::time::timeout —— 见下方「错(二)」
```

> **❌ 错(二)——给整个 `serve` 套 deadline(2026-07-27 修复的回归)**:
> 历史版本的 §7「Correct」写的是
> `tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve).await`,意图是给
> drain 阶段加兜底。但 `serve = axum::serve(...).with_graceful_shutdown(sig)`
> 在**没有 shutdown 信号时会永久跑下去**(正常服务请求),而 timeout 套的是
> **整个 serve future** —— 于是无论有没有信号,3s 后必然走 `Err` 臂 →
> `serve_daemon` 返回 `Ok(())` → bin 打印 "exited cleanly" → 进程 exit 0。
>
> 表现:daemon 每次 listen 后 ~3s 自杀(sidecar `TerminatedPayload
> {code:Some(0), signal:None}`,日志里**没有** `received SIGTERM`),
> 前端 15s health probe 永远连不上 → "daemon 不可用"。逃生 `?transport=tauri`
> 可绕过(它跳过 health probe)。
>
> 根因:`with_graceful_shutdown` 把「等信号」和「drain」捆在同一个 future,
> 无法只对 drain 加超时。drain 的硬上限改由 `shutdown_signal` 内部两步
> 保证(`sse.shutdown()` + `cancel_and_drain_all_agent_loops` 的 8s),进程
> 级 SIGKILL(`daemon.sh` / GUI sidecar)兜底。**结论:`serve_daemon` 里
> 禁止给 `serve` 套任何 `tokio::time::timeout` / `tokio::select!`。**

---

