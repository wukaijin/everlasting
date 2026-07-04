# Design — A5+ LLM 网络健壮性

> **prd**:[`prd.md`](./prd.md) · **source**:[`docs/research/llm-network-resilience-survey.md`](../../../docs/research/llm-network-resilience-survey.md)
>
> 本文档定义技术设计:retry wrapper 落点、Full Jitter 公式、retry-after 解析、首字节边界判定、C1 取消与 sleep 交互、前端事件、测试设计、回滚 shape。执行步骤见 `implement.md`。

## 1. 落点与作用面(retry wrapper 放哪)

### 1.1 候选与决策

| 候选 | 描述 | 取舍 |
|---|---|---|
| **A. Provider trait 内** | 每个 Provider 实现自己 retry,trait 加 `send_with_retry` 默认方法 | ❌ 两个 Provider 实现重复;Provider 应专注协议转换,不感知 retry |
| **B. agent loop 调用点包裹** ✅ | `chat_loop.rs` 调 `provider.send(...)` 处包一层 retry loop | ✅ 单一 source of retry 逻辑;Provider trait 签名不动(constraint 1);retry 可见 agent loop 的 `token`(R7)与 `sink`(R8) |

**决策:候选 B**,但 retry 逻辑抽成独立模块 `llm/retry.rs`(纯函数 + 一个 `retry_send` 异步 wrapper),agent loop 调用 `retry::retry_send(provider, system, messages, tools, &policy, &token, &sink)`。这样 retry 逻辑可单测(不依赖 agent loop 上下文),agent loop 调用点保持薄。

### 1.2 改动文件清单

| 文件 | 现状 | 本任务改造 |
|---|---|---|
| `llm/retry.rs`(新) | — | `RetryPolicy` 结构 + `retry_send` wrapper + `is_retryable` 判定 + `full_jitter` + `parse_retry_after` |
| `llm/error.rs` | 5 类 LlmError + `classify_error_response` | 加 `LlmError::is_retryable(&self) -> bool`(`Network`/`Server`/`RateLimit` → true,`Auth`/`InvalidRequest` → false) |
| `llm/mod.rs` | re-exports | export `retry` 模块 |
| `agent/chat_loop.rs` | `provider.send(...)` 直接调(L1388-1392) | 改调 `retry::retry_send(...)`,传入 `token` + `sink` + 首字节追踪 flag |
| `llm/provider/mock.rs` | `MockProvider` 现有 | 扩展:支持"错误序列"(前 N 次返指定 `LlmError`,之后成功)+ 支持 header 注入(测 retry-after) |
| 前端 `streamController.ts` / `MessageList` | 处理 `chat-event` delta/start/done/error | 加 `retrying` 子类型分发(显示"↩ 重试中"行) |

**零改动**:Provider trait 签名、`sse.rs`、`error.rs` 的 5 类与 `classify`、DB schema、AuditKind、Mode、permissions。

## 2. 数据流(改造后)

```
chat_loop.rs turn 内(provider 阶段)
  └─ retry::retry_send(provider, system, messages, tools, &policy, token, sink)
        │
        ├─ attempt = 0; has_emitted = false; total_elapsed = 0
        ├─ loop {
        │     stream = provider.send(system, messages, tools)   // 原调用
        │     tokio::select! { biased;
        │       _ = token.cancelled() => return Cancelled        // R7,C1 优先
        │       item = stream.next() => match item {
        │         Ok(first @ ChatEvent::Start {..}) if !has_emitted => {
        │           has_emitted = true                            // ★ 首字节标记
        │           sink.emit(first); 透传后续 stream; return stream 终态
        │         }
        │         Ok(other) => { has_emitted = true; 透传; return stream 终态 }
        │                              // ↑ 首字节后断连 = 不重试,交回 chat_loop 现状处理
        │         Err(e) => {
        │           if !has_emitted && e.is_retryable() && attempt < max && total_elapsed < budget {
        │             wait = parse_retry_after(headers) or full_jitter(attempt)
        │             sink.emit(Retrying { attempt: attempt+1, max, wait_ms, reason });  // R8
        │             select! { _ = token.cancelled() => return Cancelled,   // R7 sleep 中取消
        │                        _ = sleep(wait) => attempt++; total_elapsed += wait; continue }
        │           } else { return Err(e) }                      // 不可重试 / 预算耗尽 / 首字节后
        │         }
        │       }
        │     }
        │  }
```

**关键不变量**:`has_emitted` 一旦 true,**永不重试**——首字节后的所有错误(包括 stream 中途 Network err)直接 return 给 chat_loop 走现状(ERROR_MARKER + 执行 partial tool)。这是 R3 边界的实现核心。

## 3. 函数边界(新增,均在 `llm/retry.rs`)

### 3.1 `RetryPolicy`

```rust
pub struct RetryPolicy {
    pub max_retries: u32,        // 推荐 3(总请求 ≤ 4)
    pub base: Duration,          // 推荐 0.5s
    pub cap: Duration,           // 推荐 30s
    pub budget: Duration,        // 推荐 60s(总 sleep 上限,R6 熔断)
    pub retry_after_cap: Duration,// 推荐 60s(R5 封顶)
}
impl Default for RetryPolicy { ... }  // 上述推荐值
```

### 3.2 `LlmError::is_retryable`

在 `llm/error.rs` 加:

```rust
impl LlmError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, LlmError::Network(_) | LlmError::Server { .. } | LlmError::RateLimit(_))
    }
}
```

`Auth` / `InvalidRequest` → false(R2)。**注意**:`Server` 含 5xx;`classify_error_response` 已保证 4xx 非 429 落 `InvalidRequest`,故 `is_retryable` 不需要看 status code。

### 3.3 `full_jitter(attempt, base, cap, rng) -> Duration`

Full Jitter 公式(AWS 共识):

```rust
fn full_jitter(attempt: u32, base: Duration, cap: Duration, rng: &mut impl Rng) -> Duration {
    let upper = (base.as_millis() as u64).saturating_mul(1u64 << attempt.min(10));
    let capped = std::cmp::min(upper, cap.as_millis() as u64);
    let ms = rng.gen_range(0..=capped);   // [0, capped]
    Duration::from_millis(ms)
}
```

- `attempt` 从 0 起:首次重试 wait ∈ [0, base·1]=[0,0.5s];第 3 次 ∈ [0, min(4s, 30s)]。
- **`rng` 抽象为 trait 参数**,测试可注入确定性 seed(`StepRng` / `MockRng`),否则 `cargo test` 会 flaky。
- **不依赖 `rand` crate 之外**:用项目已有依赖;若无 `rand`,评估 `fastrand`(轻量)。**不引** `tokio-retry` / `backoff`(constraint 6)。

### 3.4 `parse_retry_after(headers, cap) -> Option<Duration>`

按优先级解析,命中即返回(封顶 `cap`):

```rust
fn parse_retry_after(headers: &HeaderMap, cap: Duration) -> Option<Duration> {
    // 1. retry-after-ms(毫秒,非标但 SDK 解析)
    if let Some(ms) = headers.get("retry-after-ms").and_then(parse_int_ms) { return Some(cap_min(ms, cap)); }
    // 2. retry-after(秒整数 或 HTTP-date)
    if let Some(v) = headers.get("retry-after").and_then(|h| h.to_str().ok()) {
        if let Ok(secs) = v.parse::<u64>() { return Some(cap_min(Duration::from_secs(secs), cap)); }
        if let Some(http_date_dur) = parse_http_date(v) { return Some(cap_min(http_date_dur, cap)); }
    }
    // 3. OpenAI x-ratelimit-reset-requests / -tokens(Go duration 字符串)
    for k in ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"] {
        if let Some(d) = headers.get(k).and_then(|h| h.to_str().ok()).and_then(parse_go_duration) {
            return Some(cap_min(d, cap));
        }
    }
    None
}
```

- `parse_go_duration(s)`:解析 `6m0s` / `1s` / `500ms` / `2h30m`。手写小解析器(Go duration grammar 简单:符号 + 数字 + 单位序列)。不引 `humantime`(_constraint 6,且 Go duration 不是 ISO 8601)。
- `>cap` 的值:`cap_min` 截断到 `cap`(R5 封顶 60s)。
- `parse_http_date`:RFC 7231 IMF-fixdate(`Wed, 21 Oct 2026 07:28:00 GMT`)→ 相对当前时间的正 Duration。**注意**:脚本环境 `Date::now()` 在 workflow 不可用,但**这是运行时代码不是 workflow 脚本**,可用 `SystemTime::now()`。若解析失败(过去时间 / 格式错)→ None。

### 3.5 `retry_send`(核心 wrapper)

签名:

```rust
pub async fn retry_send(
    provider: &dyn Provider,
    system: Option<String>,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDef>,
    policy: &RetryPolicy,
    token: &CancellationToken,       // R7
    sink: &impl ChatEventSink,       // R8,emit retrying 事件 + 透传 ChatEvent
    rng: &mut impl Rng,
) -> Result<TurnFinalState, RetryOutcome>
```

`TurnFinalState` = 成功时的(usage / stop_reason / 已透传的 stream 终态);`RetryOutcome` = `Err(LlmError)` / `Cancelled`。

**实现要点**:
- provider.send 返回的 `Stream` 不能跨 attempt 复用——每次 attempt 重新 `provider.send(...)`。
- 首字节追踪:`has_emitted: bool`,第一个 `Ok(ChatEvent)` 即设 true 并透传,**之后所有错误直接 return**(R3 首字节后不重试)。
- 首字节前的 `Err` 才进 retry 判定(`is_retryable` + 预算 + 次数)。
- retrying 事件在 sleep 前 emit;sleep 用 `tokio::select!` 配 `token.cancelled()`(R7)。
- `total_elapsed` 累加每次 wait,触达 `budget` 即停(R6)。

**Provider trait 不动**:`retry_send` 接收 `&dyn Provider`,调 `provider.send(...)`。trait 签名零改动(constraint 1)。

### 3.6 retry-after header 的来源(关键实现细节)

`LlmError::Server` / `RateLimit` 当前**不带 header**(`classify_error_response` 只解析 body,research §5.5)。要让 `parse_retry_after` 工作,需要:

- **方案**:在 Provider 的 HTTP 错误分支(`anthropic.rs:269-271` / `openai.rs:600-603` 处理非 200 response 时)**保留 response headers**,塞进 `LlmError`。
- **改 `LlmError` 变体携带 headers**:
  ```rust
  RateLimit { message: String, headers: HeaderMap },        // 原 RateLimit(String)
  Server { status: u16, message: String, headers: HeaderMap },
  ```
  **这是 prd constraint 3"LlmError 5 类不动"的边界——分类不变,但变体加 headers 字段**。design 在此声明并回写 prd Notes:5 类**名称与分类逻辑**不变,仅 `RateLimit` / `Server` 加 `headers` 字段(供 retry 解析)。`Auth` / `InvalidRequest` / `Network` 不带 header(网络错误无 response,4xx 非 429 不重试不需 header)。
- 兼容性:所有构造 `RateLimit(_)` / `Server {..}` 的点改成带 `headers`;`From<LlmError> for AppError` 等边界更新。测试需同步改断言。
- **回滚 shape**:headers 字段加 `#[serde(skip)]` 或不参与序列化(DB 不存 LlmError),无 migration。

## 4. C1 取消与重试的交互(R7)

- agent loop 现有 `token.cancelled()` 在 biased select 第一位(`chat_loop.rs:1417-1421`)—— retry_send 内部两个 select(消费 stream / sleep)都把 `token.cancelled()` 放 biased 第一位。
- 取消发生在:① stream 消费中(已有行为,不变);② **重试 sleep 中(新)**——立即返回 `RetryOutcome::Cancelled`,agent loop 走 C1 路径(`cancelled = true` + CANCELLED_MARKER,不触发 `had_error`)。
- **不变**:取消不污染 retrying 事件流(前端已显示的"重试中"行可保留或淡化,design 不强制)。

## 5. 前端 retrying 事件(R8)

### 5.1 wire 形态

新 `chat-event` 子类型(在现有 `delta` / `start` / `done` / `error` 之外):

```ts
{ type: "retrying", attempt: number, max_attempts: number, wait_ms: number, reason: string }
```

`reason` 来自 `LlmError::user_message()`(如"5xx 服务器错误" / "速率限制" / "网络断连")。

### 5.2 前端展示

- `streamController.ts`:`case 'retrying'` 分发,挂到当前 turn 的临时状态(不入 messages 数组,避免污染 DB)。
- `MessageList`:在正在生成的 assistant message 上方 / 内嵌一行 `↩ 重试中 {attempt}/{max_attempts},{wait_ms/1000}s 后重发…(reason)`。下次 ChatEvent 到来时清除该行。
- **i18n**:全中文,无新 i18n key(对齐 L3b PR4 风格)。

## 6. 测试设计

### 6.1 `MockProvider` 扩展(`llm/provider/mock.rs`)

现有 `MockProvider` 加:
- `error_sequence: VecDeque<LlmError>`——按顺序返错误,耗尽后返成功 stream。
- `header_overrides: HashMap<usize, HeaderMap>`——第 N 次请求的错误响应带指定 headers(测 retry-after)。
- `emit_then_err: Option<(usize, LlmError)>`——emit N 个 ChatEvent 后第 N+1 项返 Err(测首字节后断连)。
- `sent_tools()` / `call_count()` 已有,加 `attempt_count()`。

### 6.2 单元测试矩阵(对应 prd AC)

`llm/retry.rs` 内联 `#[cfg(test)]`:

- `full_jitter` 区间:注入 `StepRng` / `MockRng`,断言 wait ∈ [0, min(cap, base·2ⁿ)]。
- `parse_retry_after`:秒整数 / `retry-after-ms` / HTTP-date / Go duration `6m0s` / `1s` / `500ms` / 缺失 None / `>60s` 截断。
- `is_retryable`:5 类各一例。
- `retry_send`:5xx 序列成功 / 429+retry-after / OpenAI Go duration / connect err / 首字节前断连重试 / 首字节后断连不重试 / Auth 不重试 / max_retries 耗尽 / budget 熔断 / C1 sleep 中取消。

### 6.3 集成测试(agent loop)

`agent/tests_*.rs` 加:
- 重试成功后 token 统计只计一次(R9):mock 前 2 次 Server(503) + 第 3 次成功 → `sessions.last_input_tokens` 等于第 3 次的 usage。
- 重试不影响 C3 压缩触发(R9):构造接近阈值的 messages,重试成功后压缩行为不变。
- 既有 `chat_loop` 测试全绿(回归)。

## 7. 回滚 shape

- **Step 级**:每个 Step(implement.md)独立 commit,函数独立可 revert。
- **LlmError headers 字段**:若决定回退,变体改回 `String` / 无 headers,所有构造点同步——单独 commit,不影响 retry.rs(只是 retry-after 退化到只走 Full Jitter)。
- **整体**:`retry_send` 调用点改回 `provider.send(...)` 一行,retry.rs 删除,前端 retrying 分支保留(无害)。**无 DB migration、无 wire breaking、无 AuditKind 变更**——回滚成本极低。

## 8. 待 grill 的开放项(进 planning 与用户确认)

1. **`max_retries` 3 vs 5 / `cap` 8s vs 30s**:prd 推 3 + 30s;个人用烧钱 vs 体验的权衡。
2. **LlmError 变体加 headers 字段**(§3.6)是否可接受——这是 constraint 3 的边界细化(分类不变,字段扩展)。
3. **`rand` 依赖**:用 `rand` crate 还是 `fastrand`(更轻);是否项目已有。
4. **重试事件是否入审计**:prd 说"不入"(transient UX);若团队认为值得观测,可加 AuditKind(`LlmRetried`)——倾向不加(避免 AuditKind 膨胀)。
5. **前端 retrying 行的视觉**:内嵌 vs 浮层;是否带"取消重试"按钮(对齐 R7,用户可主动放弃)。
