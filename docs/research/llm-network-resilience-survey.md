# Everlasting LLM 网络健壮性调研

> 调研日期:2026-07-04
> 范围:第三档 **A5+ LLM 网络健壮性(重试 / 退避 / SSE 断点续传)**(`docs/ROADMAP.md` §2 第三档)
> 目标:理清四件事 —— ① 协议层 SSE 断连到底能不能续传;② 官方 SDK 与 coding agent 业界怎么做重试 / 退避;③ "tool_use 已执行后断连"的安全边界;④ 本仓库现状与 MVP 范围。
> 方法:一手抓取 Anthropic / OpenAI 官方 SDK 源码(Stainless 同模板)+ Claude Code / OpenCode / Cline / Aider / Continue / Cursor 业界实践 + 本仓库代码现状(`llm/provider/` / `llm/sse.rs` / `agent/chat_loop.rs`)。
> 配套:本文不写实现代码,只产出协议事实、业界共识、设计结论与待决策项;落地走 Trellis 任务 `.trellis/tasks/07-04-a5plus-llm-network-resilience/`。

---

## 0. TL;DR

1. **协议硬约束**:Anthropic Messages 与 OpenAI Chat Completions 的 SSE 流**都没有 resumption token / 可续传 event-id**。中途断连后"续传"在协议层不存在,唯一做法是**整轮重发请求**。所谓"SSE 断点续传"是 misnomer,真实命题是"断连后整轮重发的安全边界"。

2. **everlasting 的 tool 执行时序给了我们比 Claude Code 更干净的边界**。代码侧调研确认(`agent/chat_loop.rs:1490-1501` + `1655-1661`):**tool 在 SSE 流结束(`Done` / `Error` break)之后才统一执行**,`tool_calls: Vec` 在流内只是累积。→ **只要在 stream 正常结束前重试,tool 必然尚未执行 = 整轮重发绝对安全**。Claude Code 用"可见输出"作分水岭是因为它流内就执行 tool;everlasting **不需要 idempotency key / dedup 表 / 可见输出判断**。

3. **官方 SDK 默认不重试流式中断**(安全考虑)。Anthropic / OpenAI Python/TS SDK 同源 Stainless 模板,默认 `max_retries=2`,退避 `min(0.5·2ⁿ, 8s) × (0.75-1.0 抖动)`,retry-after 封顶 60s,触发码 408/409/429/>=500。但 retry loop 只包裹 HTTP 请求本身,响应头 200 + SSE 开始消费后的断连由上层处理。

4. **业界标杆 = Claude Code**:`CLAUDE_CODE_MAX_RETRIES=10`,指数退避 1s→30s,流式空闲 watchdog(5 min 无事件 abort+retry),**"可见输出前断连 → 重发;可见输出后断连 → 保留 partial + 提示 continue"** 的分水岭。**反面教材**:OpenCode(无限重试无熔断,session 死几小时)、Cursor(reconnect 后 tool call 2→11 膨胀)、OpenCode issue #32286(mid-tool 流断重复重试导致 tool 重复执行)。

5. **退避共识 = Full Jitter**(AWS Architecture Blog 经典):`sleep = random(0, min(cap, base·2ⁿ))`。纯指数退避是输家(调用聚类)。OpenAI / Anthropic SDK 的"0.75-1.0 弱抖动"**不是** Full Jitter;AWS SDK 标准 / adaptive 模式才是。OpenAI 自己的 Cookbook 推荐的也是 Full Jitter 风味(`tenacity.wait_random_exponential`)。

6. **本仓库现状**:HTTP client 已配 `read_timeout 60s + connect_timeout 10s`(RULE-A-011,用 `read_timeout` 而非 `timeout` 适配 SSE —— 设计正确);`LlmError` 5 类 + `classify_error_response` 已正确归类 429/5xx/网络;A4 token 统计已是 OVERWRITE 模式(重试不重复计数);C3 压缩幂等;`AppError.retryable` 字段已留口但无人读。**唯一缺口 = 没有任何 retry loop**(无 reqwest retry、无手动 backoff、无 Retry-After 解析)。

7. **MVP 建议**(§6):provider 层外包 `send_with_retry` wrapper;可重试 `Network` / `Server(5xx)` / `RateLimit(429)` + connect 阶段 + **SSE 首字节前**断连;Full Jitter 退避(base 0.5s,cap 8-30s,max_retries 3-5);retry-after 优先(Anthropic 秒整数 / OpenAI `x-ratelimit-reset-*` Go duration,封顶 60s);硬上限熔断;重试时 emit 事件给前端。**不做(follow-up)**:SSE 流中途(首字节后)断连自动重试、真正断点续传(协议不支持)、idempotency key(tool 时序分离不需要)、per-provider 自定义策略。

---

## 1. 背景与四个待解问题

第三档 A5+ 要解决(`docs/ROADMAP.md` §2 第三档):

> provider 层补网络重试,堵长会话 5xx / rate-limit / 流断连整轮重来的体验缺口(DESIGN §5.1 已留口)。A5 错误契约已落地(07-02)。

本调研回答四个设计问题:

| # | 问题 | 章节 |
|---|------|------|
| Q1 | SSE 断连协议层能否续传? | §2 |
| Q2 | 官方 SDK 与业界 coding agent 怎么做重试 / 退避? | §3 + §4 |
| Q3 | "tool_use 已执行后断连"的安全边界? | §4 + §5.4 |
| Q4 | 本仓库现状与 MVP 范围? | §5 + §6 |

---

## 2. 协议事实:Anthropic / OpenAI 流式 API 的断连 resumption

### 2.1 Anthropic Messages streaming — 无任何 resumption 机制

- 官方明确不支持 stream resumption。streaming(`stream: true`)中途断开后**没有** resumption token、无可续传的 event-id、无 streaming checkpoint。社区原话:"Claude API doesn't support automatic resumption"。(来源: <https://claudeapi.com/en/blog/dev-guides/claude-api-streaming-sse-guide/>)
- Anthropic SSE 流使用标准 `event:` 字段(`message_start` / `content_block_start` / `content_block_delta` / `content_block_stop` / `message_delta` / `message_stop` / `ping` / `error`),但 **SSE 的 `id:` 字段不携带可恢复位置信息** —— Anthropic 不读 `Last-Event-ID` header,也没有文档化的断点续传端点。(来源: <https://platform.claude.com/docs/en/build-with-claude/streaming>)
- `ping` event(约每 15-30s)只是 keep-alive 心跳,不是 checkpoint。(来源: <https://github.com/anthropics/anthropic-sdk-typescript/issues/998>)

### 2.2 OpenAI Chat Completions streaming — 同样无 resumption

- Chat Completions 的 SSE 流(`data: {"choices":[...]}` chunks + 终止 `data: [DONE]`)**没有** resumption token。社区 workaround 是"把已收到的 partial assistant 文本作为最后一条 assistant message 回填,再发一次请求让模型续写"。(来源: <https://community.openai.com/t/restarting-partially-completed-chat-completion-api-calls/649012>)
- **例外:OpenAI Responses API**(2025 推出、`store=true` 有状态响应)支持 `previous_response_id` 在服务端"续",但需要服务端已落盘该 response,**不适合** everlasting 这种纯 Chat Completions 客户端。(来源: <https://community.openai.com/t/responses-api-reconnect-to-a-streaming-response/1362217>)

### 2.3 协议层硬约束小结

| 协议特性 | Anthropic Messages | OpenAI Chat Completions |
|---|---|---|
| SSE `event:` 字段 | 有(8 类命名事件) | 无(只有 `data:` chunk) |
| SSE `id:` 字段 | 不携带可恢复位置 | 不携带 |
| `Last-Event-ID` 支持 | 无 | 无 |
| 官方 resumption 端点 | 无 | 无(Responses API 另算) |
| **断连后唯一安全做法** | **整轮重发** | **整轮重发** |

来源:<https://platform.claude.com/docs/en/build-with-claude/streaming> · <https://platform.openai.com/docs/api-reference/chat/streaming>

---

## 3. 官方 SDK 的默认 retry 策略(源码级确认)

Anthropic Python SDK 与 OpenAI Python SDK 的 `_base_client.py` 由 **Stainless** 同模板生成,重试逻辑**完全一致**。

### 3.1 精确默认值(两家相同)

`_constants.py`:

```python
DEFAULT_TIMEOUT = httpx.Timeout(timeout=10 * 60, connect=5.0)  # 10 分钟
DEFAULT_MAX_RETRIES = 2
INITIAL_RETRY_DELAY = 0.5   # 0.5s 起跳
MAX_RETRY_DELAY = 8.0       # 封顶 8s
```

- **默认重试次数**:`max_retries = 2`(即最多发 3 次请求)
- **退避公式**(`_calculate_retry_timeout`):
  ```python
  nb_retries = min(max_retries - remaining_retries, 1000)
  sleep_seconds = min(INITIAL_RETRY_DELAY * pow(2.0, nb_retries), MAX_RETRY_DELAY)
  jitter = 1 - 0.25 * random()   # 乘以 0.75-1.0 抖动因子
  timeout = sleep_seconds * jitter
  ```
  retry 1 → ~0.5s × [0.75-1.0];retry 2 → ~1.0s × [0.75-1.0];retry 3 → ~2.0s × [0.75-1.0]。
  **这不是 AWS 论文里任何一种标准 jitter(full/equal/decorrelated),而是"对指数值乘 0.75-1.0 的对称弱抖动"**。
- **Retry-After 优先且封顶 60s**:`retry-after` / `retry-after-ms` / HTTP-date 三种格式都被解析(`retry-after-ms` 优先),但**只在 `0 < retry_after <= 60` 秒区间才采纳**;超过 60s 的 retry-after 被忽略,回落指数退避。(来源: Anthropic `_base_client.py` `_parse_retry_after_header` + `_calculate_retry_timeout`,OpenAI 同源)
- **触发重试的状态码**(`_should_retry`):`x-should-retry: true/false` header 优先 → `408` / `409` / `429` / `>= 500`;`400/401/403/404/422` 等 4xx **不重试**。
- **网络错误**:`httpx.TimeoutException` 与其它裸 `Exception`(含 connection reset)在 `remaining_retries > 0` 时都重试,耗尽后包装为 `APITimeoutError` / `APIConnectionError` 抛出。

来源:<https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/_base_client.py> · <https://raw.githubusercontent.com/openai/openai-python/main/src/openai/_base_client.py> · <https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/_constants.py>

### 3.2 流式请求的中途断连:官方 SDK 默认**不**重试

这是最关键的一点,也是各家 coding agent 必须自己实现重试的根本原因:

- SDK 的 retry loop 包裹的是 `request()`(HTTP 请求本身)。响应头返回 200、SSE stream 开始消费后,**后续的连接中断发生在 stream 消费阶段,已经走出了 retry loop** —— `_streaming.py` 的 `Stream` / `AsyncStream` iterator 直接抛 `httpx.RemoteProtocolError` 给上层,SDK 不自动重连。
- 业界原话:"Anthropic SDK 的 retry logic does not apply to streaming requests once data has begun flowing"。(来源: <https://docs.portkey.ai/docs/private/catch-anthropic-errors>)
- 原因正是 §4 要讲的:tool_use 一旦下发并执行,无脑重发会导致副作用重复。

### 3.3 TypeScript SDK

参数与 Python 同源(同 Stainless 模板):`maxRetries = 2` 默认,`timeout = 600s`,同样 408/409/429/5xx 触发,同样 retry-after 封顶 60s。(来源: <https://platform.claude.com/docs/en/cli-sdks-libraries/sdks/typescript> · <https://github.com/anthropics/anthropic-sdk-typescript/issues/867>)

### 3.4 已知的"双重 retry 叠加"陷阱

由于官方 SDK 默认 `max_retries=2` 已内置重试,外层应用如果再包一层 retry,会发生 "silent 10+ retry cascades"。(来源: <https://github.com/HKUDS/nanobot/issues/2511>,标题"SDK built-in retries stack with chat_with_retry causing silent 10+ retries";<https://github.com/volcengine/OpenViking/issues/1856>)

**业界共识:自己实现重试时,如果底层用了官方 SDK,要把 SDK 的 `max_retries=0`。** everlasting 用 reqwest 裸调,不涉及官方 SDK,这条不直接适用,但设计时需意识到 everlasting 目前**完全没有任何 retry**(连 SDK 那层 2 次都没有)。

---

## 4. 主流 coding agent 如何处理流式断连 + tool_use 已执行

### 4.1 Claude Code(Anthropic 官方 CLI)—— 业界最成熟的设计

来源:官方 Error reference(<https://code.claude.com/docs/en/errors>)+ changelog(<https://code.claude.com/docs/en/changelog>)

- **重试预算**:`CLAUDE_CODE_MAX_RETRIES = 10`(默认),v2.1.186 起硬上限 15,v2.1.199 起 `CLAUDE_CODE_RETRY_WATCHDOG=1` 时提高到 300(约三小时 backoff)。
- **退避**:exponential backoff,形态 `1s → 2s → 4s → ... → 30s max`。(来源: <https://github.com/anthropics/claude-code/issues/26729>)
- **覆盖错误类**:server errors、529 overloaded、request timeouts、temporary 429 throttles、dropped connections。
- **流式空闲 watchdog**:默认开启,"aborts and retries automatically when a response stream produces no events for 5 minutes"(早期阈值 20s,2025 后放宽到 5 min)。
- **流式中途断连的边界处理(最重要的设计参考)**:

  > "As of v2.1.198, this covers connections that drop in the middle of a response **before any visible output has streamed**: Claude Code re-issues the request with the same backoff and the turn continues."

  > "As of v2.1.199, a server error that arrives **after Claude has already streamed visible output** keeps the partial response and appends an incomplete-response notice instead of retrying, **since re-running the request could execute the same tools twice**."

  Claude Code 用 **"是否已有可见输出"** 作分水岭:
  - 流未产生可见输出就断 → 重发(安全,幂等)
  - 流已产生可见输出后断 → **保留 partial、不重试**,提示用户 `continue` 让模型自续
- **TLS 证书验证失败不重试**(v2.1.199 起),避免几分钟 retry budget 浪费在不可恢复错误上。

### 4.2 OpenCode(sst/opencode)—— 重试无上限,已知 bug

- `SessionRetry.policy()` 对 429/529/"overloaded" **无限重试,无 maxRetries 上限、无熔断**,Session 会"死掉几小时"。(来源: <https://github.com/sst/opencode/issues/17648> · <https://github.com/sst/opencode/issues/21960> · <https://github.com/sst/opencode/issues/26354>)
- **Mid-tool 流断会重复重试**(issue #32286):"If the provider connection closes **after a streamed tool call has been delivered but before a clean message completion**, OpenCode repeatedly retries" —— 即 OpenCode **没有** Claude Code 那道"已下发 tool 就不重试"的保护,已知导致 tool 重复执行。(来源: <https://github.com/sst/opencode/issues/32286>)
- 社区 PR 提议:在 `opencode.json` provider config 暴露 `maxRetries` / `maxBackoffMs`,并把 OpenAI SDK 内置 `maxRetries: 0` 关掉避免双重叠加。(来源: <https://github.com/sst/opencode/issues/30510>)
- 第三方补丁 `marco-jardim/opencode-anthropic-fix` 给 Anthropic provider 加了 "529/503 重试 2 次 + exponential backoff + 每个账号最多试一次" —— 印证官方版策略缺失。

### 4.3 Cline / Roo Code(VSCode 插件)

- Cline 本身 retry 文档很少;其 fork **Roo Code** 在 nightly changelog 明确加了 "Retry API requests on stream failures instead of aborting task" —— 说明 Cline 系早期遇到 stream 中断是直接 abort 整个 task,后来才改重试。(来源: <https://open-vsx.org/extension/RooVeterinaryInc/roo-code-nightly/changes>)
- Cline 已知 bug:Plan mode 重复发 API 请求(issue #4829)。(来源: <https://github.com/cline/cline/issues/4829>)
- Cline 的 tool 执行模型是"模型决定 → Cline 跑 → 结果回填",未在文档中描述专门的 idempotency key 或 dedup 机制。

### 4.4 Aider

- Aider 通过 **litellm** 间接调用 LLM,自己不直接实现 SSE 重试。(来源: <https://aider.chat/docs/llms/other.html>) litellm 提供重试与 fallback,但对 **streaming 响应的重试一致性有已知问题**(litellm issue #8648)。(来源: <https://github.com/BerriAI/litellm/issues/8648>)
- aider issue #4659 提议加 `--num-retries` 选项控制 litellm 重试次数。(来源: <https://github.com/Aider-AI/aider/issues/4659>)
- 局限:aider 主要做"代码编辑建议",副作用是 edit 文件,执行模型是用户审批后写盘,没有"已写盘后断连重发"的 idempotency 防护讨论。

### 4.5 Continue(continue-dev)

- Continue 官方 troubleshooting 没有专门描述流式重试策略。(来源: <https://docs.continue.dev/troubleshooting>)
- 相关反模式:Hermes agent issue #31998 "Stale stream + partial-stream-stub creates unrecoverable retry loop" —— 用户说 "continue",模型重试同样的 tool call、撞同样的 stale stream、无限循环。这是 coding agent 圈子对 "continue 命令 + 流式重试" 反模式的经典反面教材。

### 4.6 Cursor(闭源)

- Cursor 论坛明确报告 "tool_call:completed event lost after connection reconnect in stream JSON mode":一个本应包含 ~2 个 tool call 的步骤,**reconnect 后膨胀到 11 个 tool call** —— orphan tool call 阻碍 step 完成。(来源: <https://forum.cursor.com/t/tool-call-completed-event-lost-after-connection-reconnect-in-stream-json-mode/157593>) 这是闭源产品里 tool-use 已执行后断连导致重复执行的最直接公开证据。

---

## 5. 本仓库现状(everlasting 代码侧)

### 5.1 HTTP client 构造与超时(✅ 已就绪)

- `AnthropicProvider` 在 `llm/provider/anthropic.rs:238-241` 构造 `Client::builder()`;`OpenAIProvider` 在 `llm/provider/openai.rs:571-574`。
- ✅ 已配置 `read_timeout(60s)` + `connect_timeout(10s)`(RULE-A-011,2026-06-19)。
- **关键设计决策(正确)**:用 `read_timeout` 而非 `timeout`。`timeout` 是连接到 EOF 的总截止时间,不适配 SSE(可能 60s 才收到首个 thinking delta);`read_timeout` 是每次读取的超时,每个 chunk 重置,适配 SSE 流式。
- ❌ 未配置 `pool_max_idle_per_host`、TCP keepalive、任何重试中间件。
- ❌ Cargo.toml 无 `retry` / `backoff` / `tokio-retry` 依赖。

### 5.2 流式请求发送路径

- 入口:AnthropicProvider::send(`anthropic.rs:789-912`)/ OpenAIProvider::send(`openai.rs:489-818`)。
- 转换链:`chat_request_to_wire()` → `strip_unsupported()` → `wire_messages_to_chat_messages()` → HTTP POST → `resp.bytes_stream()` → `SseParser::feed()` → 事件分发。
- Provider trait(`llm/provider/mod.rs:66-99`)定义:
  ```rust
  fn send(&self, system: Option<String>, messages: Vec<ChatMessage>, tools: Vec<ToolDef>)
      -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>>;
  ```

### 5.3 SSE 状态机(`llm/sse.rs`)

- 行导向状态机:`feed(chunk: &str) → Vec<SseEvent>`,缓冲 `event_type` + `data_buf`,空行 `\n\n` 触发 emit。
- 处理 `event:` / `data:` 行;`id:` / `retry:` / `:`(comment)忽略但不报错(`sse.rs:68-73`)。
- **Anthropic 事件映射**(`anthropic.rs:319-622`):`content_block_start` 切换 `BlockState`(Text/ToolUse/Thinking/RedactedThinking);`content_block_delta` 增量拼装(`input_json_delta` → `ToolUse.json_buf`);`content_block_stop` 完成块(`ToolUse` → 解析 JSON → emit `ChatEvent::ToolCall`);`message_delta` 提取 `stop_reason` + `usage`;`message_stop` 日志记录。
- **OpenAI 事件映射**(`openai.rs:656-812`):无 `event:` 行,仅 `data:`;`[DONE]` 结束 flush;`tool_calls` 累积到 `HashMap<u32, ToolCallBuf>`;`finish_reason` 时 flush。
- **流中断行为**:`content_block_stop` 未到达前若连接断开 —— 已接收的 `text_delta` / `thinking_delta` 已 emit 到前端;**未完成的 `ToolUse.json_buf` 留在 `BlockState` 中不会 emit**(即 partial tool_use 不下发)。SseParser 有 `reset()` 方法(`sse.rs:81-85`)但 marked `dead_code`,未调用。
- ❌ Anthropic `event: error` **未显式处理**(可能被下游错误分支捕获)。

### 5.4 Agent Loop 一个 turn 的时序(🎯 关键)

`agent/chat_loop.rs:1068-1702`:

1. **Pre-LLM**:C3 压缩 → B12 checklist 注入 → L1a background_shell 通知注入 → P2 memory recall。
2. **LLM 请求**(`chat_loop.rs:1388-1392`):`provider.send(system_prompt, turn_messages, turn_tool_defs)`。
3. **SSE 消费 + 事件分发**(`chat_loop.rs:1414-1596`):`tokio::select! { biased; _ = token.cancelled(), event = stream.next() }`,每个事件立即 emit 到前端。
4. **🎯 Tool 执行时机**:**不是**收到 `content_block_stop` 立刻执行,**不是**等 `message_stop` 批量执行 —— `tool_calls: Vec<(id, name, input)>` 在 SSE loop 内累积(`chat_loop.rs:1490-1501`),**LLM 流结束后(break 之后)才统一执行**(`chat_loop.rs:1596` 之后)。
5. **SSE 中途断开行为**:已收到完整 `tool_use`(id/name/args 全齐)但未到 `message_stop` 时连接断开:
   - 已 emit:`Start` / 若干 `Delta` / `ToolCall`(`chat_loop.rs:1490-1501`)
   - `stream.next()` 返回 `Err(LlmError::Network(...))` → `ChatEvent::Error` → break(`chat_loop.rs:1592-1594`)
   - **已收集的 tool_calls 会执行吗?** ✅ **会**!break 后(`chat_loop.rs:1655-1661`)仍会构造 `ContentBlock::ToolUse` 并 persist + 执行。`had_error = true`,`ERROR_MARKER` 注入 assistant text。

> **关键启示**:当前 everlasting 在 SSE 断连时**会把已收到的 tool_use 执行掉**(因为没有重试,直接当成"部分 turn"落盘)。这意味着 MVP 引入"首字节前重试"后,**断连点必须早于任何 tool_use 下发**才安全 —— 而 §5.3 已确认 partial tool_use(`content_block_stop` 未到达)**根本不会 emit**,所以"首字节前"边界天然安全;真正已 emit 完整 ToolCall 后的断连(finish 未到)是 MVP 明确**不自动重试**的区间(对齐 Claude Code "可见输出后保留 partial" 的边界)。

### 5.5 错误分类(`llm/error.rs`,✅ 已就绪)

5 类(`error.rs:16-32`):

1. `Auth(String)` — 认证失败(401/403,不可重试)
2. `RateLimit(String)` — 速率限制(429,可重试)
3. `InvalidRequest(String)` — 无效请求(4xx 非 429,不可重试)
4. `Server { status: u16, message: String }` — 服务器错误(5xx,可重试)
5. `Network(String)` — 网络错误(连接 / 超时 / SSE 断连,可重试)

`classify_error_response`(`error.rs:96-183`)统一入口:关键词匹配(`error.type` → `error.code` → `type`)+ 状态码 fallback。具体映射:
- 网络超时 / 连接错误 / DNS → reqwest `Err` → `Network(e.to_string())`(`anthropic.rs:269-271`,`openai.rs:600-603`)
- SSE 中途断连 → `stream.read()` `Err` → `Network("stream read: ...")`(`anthropic.rs:304-309`,`openai.rs:641-646`)
- 5xx → `Server`;429 → `RateLimit`;4xx 非 429 → `InvalidRequest`

错误冒泡到 agent loop:`stream.next()` match(`chat_loop.rs:1424-1457`)`Err` → `ChatEvent::Error` → 终止当前 turn(break)→ persist partial + ERROR_MARKER。**❌ 不重试**(直接进下一 turn 或终止)。

`AppError.retryable` 字段(`error.rs:60,82`)已留口:`AppError::from(LlmError::Server(...))` 时 `retryable=true`(`error.rs:447`)—— **但没人读这个字段**。

### 5.6 现有重试 / 退避逻辑(❌ 完全没有)

- ❌ 无 reqwest retry 中间件、无手动 retry loop、无指数退避、无 Retry-After 解析。
- C1 取消(`chat_loop.rs:1417-1421`)与网络错误边界清晰:取消是用户主动(`cancelled = true` + `CANCELLED_MARKER`,不触发 `had_error`);网络错误是系统失败(`had_error = true` + `ERROR_MARKER`)。

### 5.7 C3 / A4 交互(✅ 重试友好)

- **Token 统计(A4)**:2026-06-26 snapshot fix 改为 OVERWRITE 模式(`db/sessions.rs:397-419` `UPDATE ... last_input_tokens = ?`)。重发同一轮 → `update_last_turn_usage()` 再次调用覆盖前值,**不会重复计数**。
- **C3 压缩**:`compact_messages()` 仅作用于 `turn_messages`(`chat_loop.rs:1117-1169`),每次基于当前 `messages` 重算。重复压缩同一 `messages` vec 结果不变(幂等),**不会误触发**。

### 5.8 现状小结

| 维度 | 现状 |
|---|---|
| HTTP 超时 | ✅ `read_timeout 60s + connect_timeout 10s`(RULE-A-011,设计正确) |
| 错误分类 | ✅ 5 类 LlmError,429/5xx/网络已正确归类 |
| token 统计 | ✅ OVERWRITE,重试不重复计数 |
| C3 压缩 | ✅ 幂等,重试不误触发 |
| retryable 标记 | ⚠️ `AppError.retryable` 留口未用 |
| **retry loop** | ❌ **完全没有** |
| **退避策略** | ❌ 没有 |
| **Retry-After 解析** | ❌ 没有 |
| **SSE 断连处理** | ❌ 直接 ERROR_MARKER 终止 turn(且会执行已收到的 tool_use) |
| 连接池 / keepalive | ❌ 未配置 |

---

## 6. MVP 建议(给 design.md / prd.md 的输入)

### 6.1 做什么

| 组件 | 设计 |
|---|---|
| **retry wrapper** | provider 层外包 `send_with_retry`(或 agent loop 调用点包裹),不改 Provider trait 签名 |
| **可重试条件** | `Network` / `Server(5xx)` / `RateLimit(429)` + connect 阶段 + **SSE 首字节前**断连(= 还没 emit 任何 ChatEvent) |
| **不可重试** | `Auth` / `InvalidRequest`(4xx 非 429) |
| **退避** | **Full Jitter**:`sleep = random(0, min(cap, base·2ⁿ))`,base=0.5s,cap=8-30s |
| **重试上限** | `max_retries = 3-5`(个人用,平衡烧钱;对齐 SDK 默认 2 偏保守,Claude Code 10 偏激进) |
| **retry-after** | 优先解析,封顶 60s(对齐 SDK)。Anthropic `retry-after`(秒整数)+ `retry-after-ms`(毫秒);OpenAI `x-ratelimit-reset-requests`(Go duration 如 `6m0s`) |
| **熔断** | 硬上限(防 OpenCode 式无限重试):总 retry 时间预算 + 次数双限 |
| **前端可见** | 重试时 emit 一个 `chat-event`(如 `Retrying` 子类型或 text delta "↩ 重试中 2/5,2s 后重发…"),否则用户以为卡死 |

### 6.2 SSE 断连边界(MVP 选"保守")

两个候选边界,二者择一:

| 边界 | 描述 | 优点 | 缺点 | 建议 |
|---|---|---|---|---|
| **A. 首字节前重试**(保守) | 只在"连接建立失败 / 收到首个 ChatEvent 前"断连才自动重试;首字节后断连 = 保留 partial + ERROR_MARKER + 提示用户重发(等价 Claude Code "可见输出前") | 绝对安全、实现简单、零副作用风险 | 长 thinking 模型流到一半断了仍需用户手动 continue | **✅ MVP** |
| **B. 流中途也重试**(激进) | 首字节后断连也重试,**前提是还没 emit 过完整 `ChatEvent::ToolCall`**(因为 everlasting tool 执行在 stream 后,只要 stream 没 Done,tool 没跑,重发安全) | 覆盖更长尾的断连场景 | 复杂:要追踪流内状态、处理前端已渲染 text/thinking delta 的"回滚"或"提示不完整"、partial tool_use 的 json_buf 丢弃逻辑 | **follow-up** |

MVP 选 A。理由:
- A 已覆盖 80% 真实痛点(5xx / 429 / 瞬时网络抖动 / 连接被 reset 在首字节前)。
- B 的复杂度(流内状态追踪 + 前端 delta 回滚)显著高,且 everlasting 即便不做 B 也**安全**(tool 没跑,用户手动重发 = 继续即可,不会重复执行)。
- 真正的长流断连(thinking 流到一半)频率低,用户手动重发可接受。

### 6.3 明确不做(MVP out of scope)

- **SSE 真正断点续传** —— 协议不支持(§2)。
- **SSE 流中途(首字节后)断连自动重试** —— follow-up(边界 B)。
- **idempotency key / dedup 表** —— tool 执行时序分离(§5.4),不需要。
- **per-provider 自定义策略** —— MVP 全局统一够用。
- **per-turn 可观测 viewer** —— 归 E2(turn-level harness trace viewer),本任务只 emit 基本重试事件。

---

## 7. 待决策项(进 planning 后 grill)

1. **retry wrapper 落点**:Provider trait 内(每个 Provider 实现自己 retry)vs 外层 wrapper(在 agent loop 调用点统一包裹,Provider 不动)。倾向后者(单一 source of truth,Provider 专注协议)。
2. **max_retries 取 3 还是 5**:个人用,烧钱 vs 体验。倾向 3(SDK 默认 2 偏少,Claude Code 10 偏多)。
3. **退避 cap 取 8s 还是 30s**:SDK=8s,Claude Code=30s。个人用场景(长 thinking 一次请求就几十秒)倾向 30s 上限给服务端恢复时间。
4. **重试时前端 UX**:`chat-event` 新子类型 vs 纯 text delta 注入。倾向前者(结构化,前端可做"重试中"徽标)。
5. **是否解析 OpenAI `x-ratelimit-reset-requests` Go duration**:复杂度(解析 `6m0s` 字符串)vs 收益(OpenAI 499% 场景)。MVP 可只解析标准 `retry-after`,OpenAI 那个留 follow-up。
6. **熔断形态**:总时间预算(如 60s)vs 纯次数(3 次)。倾向次数为主 + 总时间兜底。

---

## 8. 来源汇总

**Anthropic / OpenAI 协议与 SDK**(源码级):
- <https://platform.claude.com/docs/en/build-with-claude/streaming>
- <https://platform.openai.com/docs/api-reference/chat/streaming>
- <https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/_base_client.py>
- <https://raw.githubusercontent.com/anthropics/anthropic-sdk-python/main/src/anthropic/_constants.py>
- <https://raw.githubusercontent.com/openai/openai-python/main/src/openai/_base_client.py>
- <https://platform.claude.com/docs/en/cli-sdks-libraries/sdks/typescript>
- <https://docs.portkey.ai/docs/private/catch-anthropic-errors>

**Claude Code(标杆)**:
- <https://code.claude.com/docs/en/errors>
- <https://code.claude.com/docs/en/changelog>
- <https://github.com/anthropics/claude-code/issues/26729>

**反面教材**:
- <https://github.com/sst/opencode/issues/17648> · <https://github.com/sst/opencode/issues/21960> · <https://github.com/sst/opencode/issues/26354>(无限重试无熔断)
- <https://github.com/sst/opencode/issues/32286>(mid-tool 流断重复重试)
- <https://forum.cursor.com/t/tool-call-completed-event-lost-after-connection-reconnect-in-stream-json-mode/157593>(tool call 2→11 膨胀)
- <https://github.com/HKUDS/nanobot/issues/2511> · <https://github.com/volcengine/OpenViking/issues/1856>(双重 retry 叠加)

**退避共识**:
- <https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/>(AWS Architecture Blog 经典,Marc Brooker)
- <https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html>
- <https://platform.openai.com/docs/guides/rate-limits>(OpenAI Cookbook 推荐 Full Jitter)

**rate-limit / retry-after 单位**:
- <https://platform.claude.com/docs/en/api/rate-limits>
- <https://www.respan.ai/articles/anthropic-api-rate-limits>
- <https://platform.openai.com/docs/guides/rate-limits>

**idempotency 模式**(follow-up 参考):
- <https://docs.aws.amazon.com/durable-execution/patterns/best-practices/idempotency/>
- <https://www.channel.tel/blog/idempotent-tool-calls-agent-retry-safety>
