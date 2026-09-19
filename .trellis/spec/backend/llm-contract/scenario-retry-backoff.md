<!-- Moved from llm-contract.md 2026-09-19 (doc-split) -->

## Scenario: LLM Retry / Backoff (A5+, 2026-07-05)

> 堵长会话 5xx / 429 / 网络断连整轮重来的体验缺口。Provider 层补网络重试 + Full Jitter 退避 + retry-after 解析,SSE 断连走"首字节前重试"安全边界。调研:`docs/research/llm-network-resilience-survey.md`;完整 PRD `.trellis/tasks/07-04-a5plus-llm-network-resilience/`。

###1. Scope / Trigger

- Trigger: `provider.send()` 在**首字节前**(stream 还没 emit 任何 `Ok(ChatEvent)`)返回 `Err(LlmError)`,且分类为可重试(`Network` / `Server` / `RateLimit`)。
- Why now: A5(07-02)错误契约已落地 5 类分类,但 provider 层无重试 — 单次 503 / 429 / 网络抖动让整轮 turn 失败,长会话体验脆。

###2. Retryable 分类 (`LlmError::is_retryable`)

| 分类 | `is_retryable` | 理由 |
|---|---|---|
| `Network` | ✅ true | 连接重置 / DNS / 超时 — 瞬态 |
| `Server` (5xx) | ✅ true | 上游临时故障 |
| `RateLimit` (429) | ✅ true | 配额窗口 + retry-after advisory |
| `Auth` (401/403) | ❌ false | 重试同结果 |
| `InvalidRequest` (4xx 非 429) | ❌ false | 请求本身错,重试无意义 |

###3. Backoff — Full Jitter + retry-after + 双向熔断

`RetryPolicy::default()`: `max_retries=3, base=0.5s, cap=30s, budget=60s, retry_after_cap=60s`。

- **Full Jitter**(AWS Architecture Blog 共识):`sleep = uniform(0, min(cap, base·2^attempt))`。纯指数让并发客户端聚集(thundering herd);Full Jitter 是推荐方案。`attempt` 从 0 起(首次重试 wait ∈ [0, 0.5s])。
- **retry-after advisory 优先**:命中覆盖 jitter(尊重服务端意图),二次封顶 `retry_after_cap=60s`(SDK parity — Anthropic/OpenAI 都 60s),更长 fallthrough 到 jitter。
- **`parse_retry_after`** 解析 5 格式按优先级:`retry-after-ms`(ms)→ `retry-after`(秒整数 / HTTP-date)→ OpenAI `x-ratelimit-reset-requests` / `-tokens`(Go duration `6m0s`/`1s`/`500ms`,自写 parser 不引 humantime)。
- **双向独立熔断**:`max_retries`(次数)与 `budget`(总 sleep 累计)任一触达即停。budget 防 OpenCode 式"session 死几小时"。
- rng 抽象为参数(`fastrand::Rng`),测试注入确定性 seed 避免 flaky。

###4. 首字节边界 (R3) — 核心安全不变量

`retry_open`(`llm/retry.rs`):

```
loop {
  if token.is_cancelled() { return Cancelled }            // R7 早响应
  stream = provider.send(...)
  first = select! { biased; cancel → Cancelled, stream.next() → item }
  match first {
    Ok(ev) => return Stream(once(ev).chain(stream)),      // ★ 首字节 OK,之后永不重试
    Err(e) if retryable && attempt<max && budget_left => {
      emit Retrying; select! { cancel→Cancelled, sleep(wait) }; continue
    }
    Err(e) => return Stream(once_err(e)),                 // 不可重试 / 熔断 → chat_loop had_error
  }
}
```

**`OpenOutcome::Stream` 一旦返回,后续 stream Err 在 chat_loop per-event loop 处理**(had_error + ERROR_MARKER + partial tool),**不再回 retry_open**。

**为何首字节前重试 side-effect-free**:everlasting 的 tool 执行在 stream 完成后(per-event loop 消费完才进 tool 阶段),首字节前重发 = 无 tool 副作用,不需幂等 key / 去重表(research §5.4)。对齐 Claude Code "before visible output" 规则,但因 tool 流后执行故更彻底。

###5. `LlmError` headers 字段扩展(契约变更声明)

`RateLimit` / `Server` 变体加 `headers: HeaderMap` 字段(供 `parse_retry_after` 解析 advisory):

```rust
RateLimit { message: String, headers: HeaderMap },
Server { status: u16, message: String, headers: HeaderMap },
```

`Auth` / `InvalidRequest` / `Network` **不带 header**(网络错误无 response;4xx 非 429 不重试不需 header)。**5 类名称与分类逻辑不变**(prd constraint 3 边界细化,非破坏)。headers 不参与序列化(`LlmError` 不入 DB,无 migration)。

###6. Cancellation (R7)

两个 select 都把 `token.cancelled()` 放 `biased` 第一位:
- 首字节 await race cancel
- backoff sleep race cancel — **sleep 中取消立即响应**(返回 `OpenOutcome::Cancelled`)
- `Cancelled` → chat_loop 走 C1 路径(`cancelled = true`,**不** `had_error`)

retry_open 入口的 `if token.is_cancelled()` short-circuit 让 cancel 在 send 前触发时不调 `provider.send`(cancel 早响应、不浪费请求,是预期语义)。

###7. 前端 Retrying 事件 (R8)

新 `ChatEvent::Retrying { attempt: u32, max_attempts: u32, wait_ms: u64, reason: String }`(wire `kind: "retrying"`,与 `delta`/`start`/`done`/`error` 并列)。`LlmRetrySink`(chat_loop)实现 `RetrySink` trait,通过 `emit_chat_event_via_sink` 真正 emit IPC。

前端 `streamController` `case 'retrying'` 挂到 in-flight assistant placeholder 的 `retrying` 瞬态字段(**不入 messages 数组,`rehydrateMessages` 不 copy,reload 后消失**)。`MessageItem` 在气泡上方渲染 chip:↩ 重试中 N/M,Ts 后重发…(reason)。`start`/`delta`/`done`/`error` 四个 arm 清 chip;malformed payload 防御性 drop。

###8. Tests Required

**`llm/retry.rs` 内联 `#[cfg(test)]`(35 tests)**:
- `full_jitter` 区间(注入确定性 rng)
- `parse_retry_after` 全格式(秒 / ms / HTTP-date / Go duration / 缺失 / >60s 截断)
- `retry_open`:5xx 序列成功 / 429+retry-after / 首字节前断连重试 / **首字节后断连不重试** / Auth 不重试 / max_retries 耗尽 / budget 熔断 / C1 sleep 中取消
- Step 8 边界:budget-先 / max_retries-先 / budget_remaining=0 / advisory clamp 不 overshoot

**`agent/tests_agent_loop.rs` 集成(3 tests)**:
- `a5plus_retry_does_not_double_count_token_usage`(R9 — 直查 SQL `sessions.last_input_tokens == <success_usage>` 非 N× 值;`update_last_turn_usage` 是 UPDATE OVERWRITE,只在 Done arm 调用)
- `a5plus_retry_emits_retrying_chat_events`(R8 — Retrying 字段 + rid 路由)
- `a5plus_retry_terminal_state_matches_no_retry_path`(R9 — retry 路径与无 retry 路径同位终态)

**前端 `streamController.test.ts`(6 tests)**:attach / clear-on-delta / clear-on-start / clear-on-done / clear-on-error / malformed-drop。

###9. Out of Scope

- **SSE 断点续传(message ID)**:research §5.4 证实 SSE 协议无 resumption,只能整请求重发。DESIGN §5.1 风险表"断点续传用 message ID"退路不可行,改走"首字节前重试"(本 Scenario)。
- **`Auth` / `InvalidRequest` 重试**:R2 明确不重试。
- **Retry 入审计**:transient UX,不入 AuditKind(prd grill §4 锁定,避免 AuditKind 膨胀)。

---

