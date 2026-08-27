# Pattern: 全局 loop 并发闸(F3,2026-08-27 `08-27-f6-async-agent-task`)

## Problem

detach 后多个 session 的 agent loop 并发跑,无上限时 N 个并发 LLM 流 = N 份 token 消耗 + provider 限流雪崩 + 本地工具(shell/subagent)资源争抢。需要在「handler 立返契约」与「全局背压」之间求平衡。

## Solution: AppState 静态 Semaphore,spawn 闭包头获取

容量来自 `app_config.max_concurrent_loops`(KV 表,解析失败/≤0 回落默认 4,fail-open),`load_inner` 里 `Arc<Semaphore` 随 AppState 构造,进程级永不 close。

```
chat_inner:
  路由临界区(照旧,不知道闸的存在)→ claim 即注册(busy 即亮)
  spawn 闭包头:
    acquire_loop_permit(permits, token)   // biased select: cancelled 臂先行
      Some(permit) → 正常 run(driver/loop 全程持 permit)
      None        → emit Done{cancelled} + rollback_claim_before_loop + return
```

「已接受在途」语义刻意把**等闸轮次也算 busy**:claim 在临界区、等闸在 spawn 里,前端红点/关闭确认(R4 计数)天然覆盖排队轮。

## 硬约束(违反即回归)

1. **唯一合法调用点是 spawn 闭包开头**(handler 已返回之后)。禁止在 `chat_inner` 路由临界区内 acquire——临界区持全局 `message_queues` 锁,await 信号量会队头阻塞所有发送,并与驱动器 turn 边界 `drain_all` 的队列锁构成死锁环(外模型评审 P1,已拒并以此固化)。
2. **等闸期间被取消必须完整回滚 + 补 cancelled 终态**:`cancellations` 摘除 + `session_active_request` 按 **rid 守卫** retain(无条件按 session 删会误摘并发新请求的注册)+ `done_tx.send`(放行破坏性命令等待方)+ `inflight_exits` 清理。漏任何一步 = session 被假在途请求卡死或前端 rid 悬挂。
3. **`acquire_owned()` 的 Err 不可达但必须处理**(信号量永不 close):`.expect("loop semaphore never closed")` 固化契约——若未来有人 close,这里 fail-fast 而不是静默吞。
4. **`biased` 让 cancelled 臂先行**:已取消的请求不再排队领许可(领了也得立刻还,白排一次,还扰动 FIFO)。
5. permit 生命周期 = spawn 闭包体(`_loop_permit` 绑定,loop/driver/群聊三分支全覆盖),闭包返回自动归还。禁止在 run 内部转移/提前 drop。

## Wrong vs Correct

```rust
// Wrong — 在路由临界区内等闸:持 message_queues 锁 await,
// 全部发送被队头阻塞,且与驱动器 drain_all 构成 AB-BA 死锁环
let mut qmap = message_queues.lock().await;
let _permit = loop_permits.acquire().await;   // ← 死锁点
...

// Correct — 临界区只 claim,等闸移进 spawn(handler 立返契约保持)
if !busy { /* claim 即注册 */ }
tokio::spawn(async move {
    let _loop_permit = match acquire_loop_permit(&loop_permits, &token).await {
        Some(p) => p,
        None => { /* Done{cancelled} + rollback + return */ }
    };
    ...run...
});
```

## Tests

- `agent/chat.rs f3_gate_tests`:闸满时取消 → None 且无许可泄漏 / FIFO 跨等待者 / 已取消即 None 不领许可 / rollback 四件套逐项断言 / `max_concurrent_loops` 解析(合法值、非法回退 4、≤0 回退 4)
- live:turn-smoke.sh 实跑覆盖 spawn 链;cancel-race stress(c1×gap×c2 时序矩阵)验证取消/续轮/回滚互不串扰
