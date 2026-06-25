# SSE / Agent Loop 多维评估报告

> 评估日期：2026-06-12
> 评估基线：commit `d7f81d9` (F5 follow-up per-turn latency tracking 合并后)
> 评估范围：Rust 后端 SSE 解析 → Provider → Agent Loop 全链路，及前端消费侧 (`streamController.ts` / `chat.ts`)
> 评估维度：稳定性、正确性、并发安全、资源管理、错误处理、边界条件、架构合理性

---

## 0. 总体评价

**整体评分：★★★★½ (4.5/5)**

一句话总结：**SSE + Agent Loop 核心路径非常稳健。** 从设计到实现体现了清晰的工程素质：逐层解耦、取消安全、跨协议兼容、错误不传染。319 个单元测试全量通过。已知风险均为极端边界场景，不影响日常使用。4 条 P2–P5 优先改进建议见 §8。

---

## 1. SSE 解析器 (`llm/sse.rs`) — 稳定性 ★★★★☆

### 1.1 设计概览

`SseParser` 是一个两字段状态机 (`event_type: String` + `data_buf: String`)，`feed()` 方法按行解析任意大小的文本 chunk，在空行边界产出完整 `SseEvent`。

```rust
// 核心状态转移 (sse.rs:31-58)
for raw_line in chunk.split('\n') {
    let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
    if line.is_empty()          { yield event }
    else if "event: "           { store event_type }
    else if "data: "            { append to data_buf }
    else if "id:" | "retry:" | ":" { ignore }
    else                        { drop silently }
}
```

### 1.2 做得好的

| 能力 | 实现 | 验证 |
|------|------|------|
| `\r\n` 行尾统一 | `strip_suffix('\r')` — 一行即处理 | `handles_carriage_return` 测试 |
| 跨 chunk 缓冲 | trailing data 保留到下次 `feed()` | `buffers_across_chunks` 测试 |
| 多行 `data:` 连接 | `push('\n')` + `push_str(rest)` — 逐行 append | 标准 SSE 行为 |
| `id:` / `retry:` / 注释 | 静默忽略 | `ignores_comments` 测试 |
| 连接中断清理 | `reset()` 清空两个 buffer | — |

### 1.3 边界条件全覆盖

| 场景 | 处理 | 评分 |
|------|------|:----:|
| chunk 边界不对齐 (`"event: ping\nd"` + `"ata: x\n\n"`) | ✅ `event_type` 缓冲，下次继续 | ★ |
| 空 data 字段 (`"data:\n\n"`) | ✅ 产出空 `data` 字符串 | ★ |
| 未闭合事件尾部 | ✅ 不产出事件，等下次 chunk | ★ |
| 畸形行（无任何前缀匹配） | ✅ 静默丢弃，不 panic | ★ |
| 非 UTF-8 字节 | ✅ 上层 `anthropic.rs:284` 做 `str::from_utf8` 检查 | ★ |
| `ping` heartbeat (GLM 兼容层) | ✅ 上层 switch 中 `"ping" => ignore` | ★ |

### 1.4 已知风险

**唯一关注点：`data_buf` 无大小上限。** 若上游发送 GB 级 SSE data（恶意或上游 bug），`push_str` 会无限增长导致 OOM。实际上游 API 不会这么干，但防御性编程应加一个上限。

> **建议 (P2):** 在 `feed()` 中检查 `data_buf.len() > 1_MiB`，超限返回错误或截断。对 99.99% 真实响应，1 MiB 已是 ample。

---

## 2. Agent Loop (`agent/chat.rs`) — 正确性 ★★★★☆

### 2.1 核心流程

```
chat() Tauri command (line 107)
  ├─ pre-flight: lookup_provider_for_session → 解析 Provider + Model
  │   └─ 失败 → emit ChatEvent::Error → return Ok(()) (同步返回，不 spawn)
  ├─ 注册 CancellationToken + session_active_request 映射
  └─ tauri::async_runtime::spawn(async move {
        ├─ 加载 Session / Project
        ├─ assert_within_root (worktree_path + session_cwd)
        ├─ build_system_prompt + B5 memory 注入 (synthetic user message)
        ├─ persist user message
        └─ for turn in 1..=20:
              ├─ ① provider.send(system, messages, tools) → Stream
              ├─ ② tokio::select! { biased; cancel vs stream.next() }
              ├─ ③ 累积: text_parts / tool_calls / finalized_thinking / redacted
              ├─ ④ Done/Error → break inner loop
              ├─ ⑤ build_assistant_message + persist_turn(Some(&latency))
              ├─ ⑥ emit TurnComplete (per-turn timing)
              ├─ ⑦ 如果 stop_reason == "tool_use" 且 !cancelled:
              │      ├─ execute_tool() 逐个调用
              │      ├─ emit tool:call / tool:result
              │      ├─ build tool_result message + persist_turn
              │      └─ continue (下一轮 LLM turn)
              └─ 否则: emit Done → return
     })
```

### 2.2 取消安全 — 三层检测

这是 agent loop 最复杂的部分，处理得很干净：

1. **Stream 侧 cancel** (`tokio::select! biased;`, line 488-494) — cancel arm 优先轮询，一旦触发立即 break 内循环
2. **Tool 执行中 cancel** (line 946-985) — 每个 tool 执行完后检查 `token.is_cancelled()`，若 true 则 break
3. **Tool 执行后 cancel** (line 991-1031) — 收集已执行 tool 的部分结果，persist + 合成 `<synthetic tool_result>` → emit Done

三层覆盖确保：
- 已 emit 的 `tool_use` 块永远不会沦为孤儿 —— 必定有对应 `tool_result` 块持久化
- 下次 `send()` 不会撞到 Anthropic 2013 `"tool_use blocks must be paired with tool_result"`
- 取消时附加 `[已停止]` marker 让用户看到截断点

### 2.3 并发状态管理

| 资源 | 管理方式 | 评分 |
|------|----------|:----:|
| `CancellationToken` → `request_id` | `Arc<Mutex<HashMap>>`, 注册在 spawn 前，注销由 `CancellationGuard` Drop | ★ |
| `session_id` → `request_id` | 同上 Map，同 Guard 管理 | ★ |
| 多 session 并发流 | `streamController` 的 `activeRequests` Map 独立索引 | ★ |
| Provider (`Arc<dyn Provider>`) | 构造一次，复用所有 turn，`Send + Sync` 保证线程安全 | ★ |

**RAII CancellationGuard** (line 188-193):
```rust
let _cancel_guard = CancellationGuard {
    cancellations: cancellations.clone(),
    session_active_request: session_active_request.clone(),
    request_id: rid.clone(),
    session_id: session_id.clone(),
};
// 无论正常 return / error / cancel / max_turns，Drop 都会清理 Map 条目
```

### 2.4 边界条件处理

| 场景 | 处理 | 评分 |
|------|------|:----:|
| Provider resolution 失败 | pre-flight 同步 emit Error，不 spawn | ★ |
| Session/Project 不存在 | emit Error + return | ★ |
| Session cwd 无效（目录被中期删除） | `assert_within_root` 失败 → fallback 到 worktree root (line 304-316) | ★ |
| LLM 流无事件直接结束 | `stream.next() == None` → break (line 496) | ★ |
| Thinking 无后续 text/tool_use | `flush_pending_thinking` 在分支前统一调用 (line 736) | ★ |
| 空 assistant_blocks | 跳过 `persist_turn` 和 `TurnComplete` emit (line 781 check) | ★ |
| `emit_chat_event` 失败 | 仅 log warn，不中断 agent loop (helpers.rs:150-153) | ★ |
| MAX_TURNS=20 耗尽 | emit Done(`max_turns`)，不 persist 新 turn | ★ |

### 2.5 已知风险

**中低风险：`persist_turn` 失败静默。** 当数据库写入失败时（line 811），agent loop 记录 `tracing::error!` 但继续执行，用户看不到任何错误提示。不过此场景意味着磁盘损坏或 DB 连接断开，此时整个应用大概率不可用，所以实际上是可接受的 "best-effort" 语义。

> **建议 (P3):** 在 `persist_turn` 失败时 emit 一个 warning 级别的 `ChatEvent::Error` 给前端，至少让 UI 显示 "消息保存失败" 提示。

---

## 3. LLM Provider 抽象层 — 合理性 ★★★★★

### 3.1 Provider Trait — 设计亮点

```rust
pub trait Provider: Send + Sync {
    fn send(&self, system, messages, tools)
        -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>>;
    fn capabilities(&self) -> ProviderCapabilities;
    fn protocol(&self) -> ProviderProtocol;
}
```

**不用 `async_trait`，而是返回 `Pin<Box<dyn Stream>>`** — 这是此文中最考究的设计决策：
- trait 是 object-safe 的（可存为 `Box<dyn Provider>`，可放进 `Arc`）
- 避免了 `async_trait` 的额外 boxing 和 `Send` 问题
- `Provider` 构造一次、复用所有 turn — 无每 turn 重建 HTTP client 的开销

**Provider factory** (`build_provider`, mod.rs:141) 仅 50 行，对 `"anthropic"` / `"openai"` / `unknown` 三条路径。添加新协议只需 20 行。

### 3.2 Wire 中间层 — 跨协议降级

`provider/wire.rs` 是整个 PR3 的核心设计。数据流：

```
ChatRequest (Anthropic-shaped)
  └─ chat_request_to_wire → WireRequest (protocol-agnostic)
       └─ strip_unsupported(target_caps) → 丢弃目标协议不支持的块
            └─ provider-wire converter → 上游 HTTP body
```

**`strip_unsupported` 的规则矩阵：**

| WireBlock | Anthropic 目标 | OpenAI 目标 (无 thinking) | OpenAI 目标 (有 reasoning) |
|-----------|:---:|:---:|:---:|
| `Text` | 保留 | 保留 | 保留 |
| `Reasoning` | → `thinking` | 丢弃 | → `reasoning_content` |
| `Signature` | 保留 | 丢弃 | 丢弃 |
| `RedactedThinking` | 保留 | 丢弃 | 丢弃 |
| `ToolUse` | 保留 | 保留 | 保留 |

**设计正确性保证：**
- 从 Claude 切换到 GPT 时，thinking/signature 块 silently dropped —— DB 中不动，只影响本次 wire payload
- `cache_control` 在 `UserBlocks` 变体中保留 (B5 refactor, wire.rs:273-320)
- 所有降级行为有 round-trip 测试覆盖 (wire.rs tests, §"round-trip" 和 §"B5 cache_control preservation")

### 3.3 Anthropic SSE 消费者 (`anthropic.rs`)

`BlockState` 枚举管理流式 content block：

```rust
enum BlockState {
    Idle,
    Text,
    ToolUse { id, name, json_buf },
    Thinking { thinking_buf, signature_buf },
    RedactedThinking { data_buf },
}
```

**关键设计点：**
- `signature_delta` 只在 `content_block_stop` 前出现一次，正确处理
- `usage` 从 `message_delta` (优先) + `message_start` (fallback) 双路径提取
- `ping` heartbeat 静默忽略 (GLM 兼容)
- 50+ 单元测试 + 实时集成测试覆盖

### 3.4 OpenAI SSE 消费者 (`openai.rs`)

OpenAI 路径比 Anthropic 复杂 —— 它需处理：

- **多个并发 `tool_calls`**：通过 `ToolCallAccumulator: HashMap<index, ToolCallBuf>` 按 index 索引
- **增量 JSON**：`function.arguments` 是流式片段，`args_buf` 逐片拼接，emit 时整段 parse
- **`reasoning_content`** (o1/o3)：映射为 `ChatEvent::ThinkingDelta`
- **`data: [DONE]` sentinel**：正确终止流

---

## 4. Error Handling — 稳定性 ★★★★☆

### 4.1 五类错误 + 中文用户消息

```rust
pub enum LlmError {
    Auth(String),            // 401 — API key 无效
    RateLimit(String),       // 429 — 频率限制
    InvalidRequest(String),  // 400 — 请求格式错误
    Server { status, msg },  // 5xx — 服务端故障
    Network(String),         // 连接失败/超时
}
```

每类错误有独立中文 `user_message()`，前端可直接渲染（error.rs:46-54）。

### 4.2 跨协议关键字匹配

`classify_error_response` 是错误分类的核心（error.rs:96-183），同时兼容：

- **Anthropic**: `{"error": {"type": "authentication_error", "message": "..."}}`
- **OpenAI**: `{"error": {"code": "invalid_api_key", "message": "..."}}`（用 `code` 而非 `type`）
- **GLM**: `{"type": "error", "error": {"type": "new_api_error", "message": "..."}}`（双重包装）

匹配链：`error.type` → `error.code` → top-level `type` → status code fallback。

**已知边缘 case** (已在 `docs/HACKING-llm.md` 记录)：GLM 对 `max_tokens` 超限可能返回 `500` 而非 `400`，且 body 若无 `invalid_request` 字样，会误分类为 `Server` (可重试) 而非 `InvalidRequest` (应修正)。

> **建议 (P4):** 加一条规则：`500 + body 含 'max_tokens' → InvalidRequest`。

### 4.3 错误传播路径

```
Provider::send() → Stream<Item = Result<ChatEvent, LlmError>>
    ↓ Err 分支
agent loop (line 501-507):
    Err(err) → ChatEvent::Error {
        message: err.user_message(),
        category: err.category()
    }
    ↓
emit_chat_event → Tauri IPC → frontend streamController.handleChatEvent
    ↓
finalizeRequest → 设 last.error + 标记 streaming = false
```

**设计正确性**：Provider 层的 `LlmError` 在 agent loop 中被转换为 `ChatEvent::Error`，实现层间解耦。Agent loop 不关心具体错误类别，只关心 "有错误 → 停止"。

---

## 5. 前端消费 (`streamController.ts`) — 架构 ★★★★☆

### 5.1 Single Source of Truth 设计

```
messagesBySession: Map<sessionId, ChatMessage[]>   // LRU 上限 20, streaming session pin 住
activeRequests:    Map<requestId, RequestState>     // in-flight streams
listener:           全局 SSE 监听器, 按 request_id 路由
```

**设计亮点：**
- **LRU + Pinning**：多 session 消息共存不被覆盖；streaming session 严防驱逐
- **Per-request 路由**：事件按 `request_id` 分发，不依赖 "当前 session" —— 切换 session 不影响在飞的流
- **Listener 去重**：`listenerWired` flag + 模块级 listener 变量，防 HMR 重复注册
- **reloadAfterFinalize**：Done 后从 DB 重载 session，保证最终一致性

### 5.2 事件路由精度

`handleChatEvent` (streamController.ts) 的 switch 覆盖所有 `ChatEvent` variant：

```
"start"        → bump currentTurnIndex
"delta"        → 追加到 last.content + 打 firstDeltaAt 时间戳
"thinking_delta" → 追加到 last.thinkingBlocks
"signature_delta" / "redacted_thinking_delta" → 注入对应字段
"done"         → finalizeRequest → reloadAfterFinalize
"turn_complete" → latencyByTurn.set(currentTurnIndex, ...) + 修改 last.latency
"error"        → last.error = { message, category } + finalizeRequest
```

### 5.3 已知注意事项

`reloadAfterFinalize` 在 `finalizeRequest` 内部调用，触发异步 DB 查询。时序保证：后端 `persist_turn` 在 `emit Done` 之前完成 (chat.rs:915-928)，所以这里的异步 read 必定读到完整数据。不存在 race condition。

---

## 6. Per-Turn Latency Tracking (F5 Follow-up) — 新特性 ★★★★☆

### 6.1 演进路径

1. **F5 初版** (commit `69be143`)：单个 `latencyPending` 只存最后一轮数据 → **"只有最后一个 assistant row 有 thinking_ms" bug**
2. **F5 Follow-up** (commit `d7f81d9`)：改为 per-turn 独立 5 个 `Instant` + `TurnComplete` per turn 事件 + 前端 `latencyByTurn: Map<number, TurnLatency>`

### 6.2 后端 per-turn 时间基线

```rust
// 每个 turn 独立初始化 (chat.rs:431-435)
let mut turn_send_at: Option<Instant> = None;
let mut turn_first_delta_at: Option<Instant> = None;
let mut turn_thinking_start: Option<Instant> = None;
let mut turn_thinking_done: Option<Instant> = None;
let mut turn_done_at: Option<Instant> = None;
```

五个边界在事件处理中分别打点：
- `send_at` — `provider.send()` 返回后立刻 (line 464)
- `first_delta_at` — 首个 `Delta` 事件 (line 538-540)
- `thinking_start` — 首个 `ThinkingDelta` (line 567-569)
- `thinking_done` — 4 个 close boundary: `Delta` / `ToolCall` / `Done` / `Error` (line 548/601/637/677)
- `done_at` — `Done` 事件 (line 626)

### 6.3 饱和度处理

```rust
fn instant_delta_ms(start, end) -> Option<i64> {
    let d = end.saturating_duration_since(start);  // NTP step-back 安全
    Some(i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
```

- `saturating_duration_since` 防止 NTP 时钟回拨导致 panic
- `i64::try_from` + `unwrap_or(i64::MAX)` 防止超大值溢出
- 四个 ms 字段独立 `Option`：覆盖 "tool_use 无 text delta" 等边界场景

---

## 7. 测试覆盖

```text
cargo test --lib: 319 passed, 0 failed, 0 ignored
```

**模块分布：**

| 模块 | 测试数 | 覆盖内容 |
|------|:------:|----------|
| `sse` | 4 | 单事件/跨 chunk/`\r\n`/注释 |
| `error` | 7 | GLM 401/Anthropic 401/GLM 500→400/rate_limit/nested wrapper/中文消息 |
| `anthropic` | ~55 | BlockState / SSE event 分发 / parse_usage / from_env / thinking_config / factory / 1:1 round-trip |
| `openai` | ~42 | Wire ↔ Chat Completions mapping / tool_calls accumulator / error classification / stream_options / live integration |
| `wire` | ~38 | chat_request_to_wire / chat_message_to_wire / strip_unsupported / round-trip / cache_control 保留 |
| `agent` | ~15 | tool_result_envelope / synthetic_tool_result / cancel_inflight |
| `tools` | ~18 | read_file / write_file / edit_file / grep / glob / list_dir / shell / read_guard |

**缺少的测试层面：** 无 Agent Loop 集成测试 (mock HTTP server 驱动的完整 turn 流程)。当前所有 agent loop 逻辑通过手动代码审查验证。

---

## 8. 优先改进建议

| 优先级 | 建议 | 位置 | 工作量 |
|:------:|------|------|:------:|
| **P2** | SSE `data_buf` 大小上限 (1 MiB) | `llm/sse.rs:feed()` | ~5 行 |
| **P3** | `persist_turn` 失败 → emit `ChatEvent::Error` 给前端 | `agent/chat.rs:811` | ~10 行 |
| **P4** | GLM `max_tokens` 超限 500 误分类加 keyword "max_tokens" | `llm/error.rs:classify_error_response` | ~3 行 |
| **P5** | Agent Loop 集成测试 (mock HTTP server) | `agent/chat.rs` 外围 | 中等工作量 |

---

## 9. 评分矩阵

| 维度 | 评分 | 关键优势 | 已知风险 |
|------|:----:|---------|---------|
| **SSE 解析器** | ★★★★☆ | 简洁状态机，跨 chunk 正确，有 reset()，测试完整 | `data_buf` 无大小上限 |
| **Agent Loop** | ★★★★☆ | 三层 cancel 覆盖，RAII Guard 管理，边界 fallback 全面 | `persist_turn` 失败静默 |
| **Provider 抽象** | ★★★★★ | 多协议对称，wire 降级优雅，不用 async_trait，测试密集 | — |
| **Error Handling** | ★★★★☆ | 五类跨协议分类，中文用户消息，关键字多路径匹配 | GLM `max_tokens` 500/400 误分类 |
| **前端消费** | ★★★★☆ | LRU+Pin 单真理源，事件路由 correct，最终一致性 | — |
| **Per-turn Latency** | ★★★★☆ | Per-turn Instant 隔离，饱和度处理，NTP step-back 安全 | — |
| **整体架构** | ★★★★½ | 分层清晰，自研 harness 而非 SDK 包装，取消安全优秀 | 缺集成测试 |

---

*评估人：AI (Reasonix 模型) · 审查方式：全量代码阅读 + 319 单元测试运行 + 边界条件枚举分析*
