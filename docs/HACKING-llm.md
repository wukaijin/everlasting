# HACKING-llm: LLM API 兼容层差异笔记

> 当前实测环境:用 `wukaijin.com` 转发的 **GLM-4.7** 走 Anthropic 兼容协议,**不是真的 Anthropic Claude API**。SSE 协议理论上等价,但错误响应 / HTTP 状态码 / 边界行为有 3 处差异。
>
> 写给未来的自己(或者下个 session),实施 LLM 客户端时别再踩这些坑。
>
> **触发场景**:写 / 改 / 调试 LLM 流式客户端、错误处理、SSE 解析时。

---

## 现状一句话

- **当前 base URL**:`https://api.wukaijin.com/v1/messages`(从 `ANTHROPIC_BASE_URL` env 读)
- **当前 model**:`GLM-4.7`
- **当前 API key**:环境变量 `ANTHROPIC_API_KEY`(智谱风格 `sk-g4HcGHnrqbc...`,不是 Anthropic 风格)
- **协议**:Anthropic Messages API 兼容,header 用 `x-api-key` + `anthropic-version: 2023-06-01`

未来切真 Claude:改 `ANTHROPIC_BASE_URL` 空、model 改 `claude-haiku-4-5`、key 换 Anthropic 的 `sk-ant-...`。重测下面 3 处差异。

---

## GLM 兼容层 3 处差异(vs 真 Anthropic Claude)

### 差异 1:401 的 `error.type` 字段

| | GLM-4.7 (wukaijin) | Anthropic 真 Claude(预期) |
|---|---|---|
| HTTP 状态 | 401 | 401 |
| `error.type` | `new_api_error` | `authentication_error` |
| `error.message` | `Invalid token (request id: ...)` | `invalid x-api-key` 之类 |

**实际响应**:
```json
{"error":{"code":"","message":"Invalid token (request id: 202606040746161667876668268d9d6oumb0OUc)","type":"new_api_error"}}
```

**客户端对策**:**不要硬编码 `error.type` 字符串值**,用一个 enum `AuthError / RateLimitError / InvalidRequestError / ServerError`,根据 HTTP 状态码 + 内层 `error.type` 关键词(`authentication` / `rate_limit` / `invalid_request`)做归一化。

### 差异 2:400 类错误有时返 5xx

| 用例 | GLM-4.7 实际 | Anthropic 预期 |
|---|---|---|
| `content: ""`(空消息) | **HTTP 500**,error.type=`invalid_request_error` | HTTP 400,error.type=`invalid_request_error` |
| `max_tokens: 999999` | **HTTP 200**,正常 stream(不验证上限) | HTTP 400,error.type=`invalid_request_error` |

**实际响应(content 空)**:
```json
{"error":{"type":"invalid_request_error","message":"[1213][未正常接收到prompt参数。][20260604154619bcfec0cd2f094b81]"},"type":"error"}
```
(注意外层 wrapper 又包了一层 `error.type: "error"`,嵌套 2 层)

**客户端对策**:
- **不能只信 HTTP status code**——`status >= 400` 之后还要 parse body 的 `error.type`
- 4xx + 5xx 都要走"错误归一化"路径
- 嵌套 JSON 要容错:先尝试 `body.error.type`,再尝试 `body.type`,再回退到 `status as_u16()`

### 差异 3:`max_tokens` 上限不严格

GLM-4.7 在 `max_tokens: 999999` 时**正常 stream**(不报错,不截断到某个值)。Anthropic 真 Claude 应该返 400。

**客户端对策**:**实施时不要在客户端做 max_tokens 上限预检**,由 server 报(切到真 Claude 时它会报;wukaijin 不报,那就不预检,server 决定)。

---

## 额外观察:GLM 多发一个 `ping` 心跳事件

GLM 的 SSE 流在 `message_start` 之后、`content_block_start` 之前多发一个 `event: ping`(`data:` 是空或心跳数据)。Anthropic 真 Claude 我不确定有没有,但**不崩**的客户端必须 unhandled 不退出。

**实际事件顺序(GLM)**:
```
message_start
ping                              ← GLM 特有,Anthropic 可能没有
content_block_start
content_block_delta × 49
content_block_stop
message_delta
message_stop
```

**客户端对策**:SSE 解析时遇到未知 `event:` 类型,记日志 + 继续,不要 panic、不要 return error。打印一行 `▶ <event> (unhandled)` 方便调试。

---

## LLM 客户端实施 checklist(给步骤 1-2 写 Rust 客户端时)

来源:spike-002 撞到的所有坑。

- [ ] **BASE_URL 从 env 读**(`ANTHROPIC_BASE_URL`),空时 fallback 到 `https://api.anthropic.com`
- [ ] **Model 从 env 读**(`LLM_MODEL` 或类似),默认 `GLM-4.7` 兼容 wukaijin
- [ ] **API key 从 env 读**(`ANTHROPIC_API_KEY`),env 注入不落盘
- [ ] **SSE 解析**:`event:` / `data:` / 空行 三段式,buffer 累积跨 chunk
- [ ] **未知事件不崩**:unknown event type 记日志 + continue
- [ ] **错误归一化**:把 HTTP 4xx/5xx 都 parse 成内部 `enum LlmError`,基于 status + body 的 `error.type` 关键词分类(`Auth / RateLimit / InvalidRequest / Server / Network`)
- [ ] **嵌套 JSON 容错**:`body.error.type` / `body.type` / `status` 三层 fallback
- [ ] **不在客户端做 max_tokens 上限预检**(让 server 报)
- [ ] **流式断连处理**:`bytes_stream` 提前断 → 记 partial content + 报错,不要静默
- [ ] **超时**:每个请求设 timeout(connect 10s + total 60s?),超时后归类 `Network`
- [ ] **重试**:5xx + Network 类可以重试(指数退避),4xx 不重试(无效)
- [ ] **abort / cancel**:用户停生成时,通过 `tokio::sync::watch` 或类似机制 cancel reqwest future(给前端按钮"停止"用)

---

## 切换到真 Anthropic Claude 时,重测清单

未来 base URL 切回 `https://api.anthropic.com`,model 改 `claude-haiku-4-5`,**重跑下面 4 个用例验证**:

- [ ] 成功用例:HTTP 200,事件顺序应该没 `ping`(除非 Claude 也发),`message_start → content_block_start → ... → message_stop`
- [ ] 401 错 key:HTTP 401,`error.type` 应是 `authentication_error`(非 `new_api_error`)
- [ ] 400 `max_tokens: 999999`:HTTP 400,`error.type: invalid_request_error`(GLM 不报这个)
- [ ] 400 `content: ""`:HTTP 400(非 500),`error.type: invalid_request_error`

任何一项不符,要更新本文件"差异 N",并改实施 checklist。

---

## 关联文档

- [spike-002](./spikes/002-reqwest-anthropic-sse.md) — 这些差异的来源 spike
- [HACKING-wsl.md](./HACKING-wsl.md) — WSL 环境坑(配对文档)
- [TECH §2 rig-core](./TECH.md#2-决策rig-core-作为-llm-抽象层) — 为什么 spike-002 决定手写 reqwest 不上 rig-core
- [IMPLEMENTATION §2.1 步骤 1](./IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp) — LLM 客户端实施位置

---

## 差异 4:extended thinking 兼容层(2026-06)

**场景**:步骤 6 起 LLM 客户端总是发 `thinking: { type: "adaptive", display: "summarized", effort: <env> }`,并流式接收 `thinking_delta` / `signature_delta` / `redacted_thinking` 块。

**结论**:

- **GLM Claude-compat 端点对 thinking 字段的转译行为未官方文档化**——智谱 `docs.bigmodel.cn/cn/guide/develop/claude/*` 路径下完全没有 thinking / reasoning / extended-thinking 子页面。`reasoning_content` 是 GLM 原生 OpenAI-compat 端点 `/api/paas/v4/chat/completions` 上的字段,Anthropic 风格端点**理论上**会做转译,但**没人验证过**。
- **真机测试路径**:发一条 `thinking: { type: "enabled", budget_tokens: 2048 }` 到 GLM Claude-compat,观察三种结局:
  1. (a) 响应里有 `type: "thinking"` block + `signature` → 完美,继续走 Anthropic schema
  2. (b) 静默忽略 `thinking` 字段,响应里没 thinking 块 → 项目照常工作(只是 UI 上看不到 thinking);client 端要防御性处理"没收到 thinking_delta 不算错"
  3. (c) 4xx/5xx 拒绝 → 改用 GLM 原生 OpenAI-compat 端点 + 读 `reasoning_content`
- **fallback 路径**:如果 Claude-compat 端点不通,改 `ANTHROPIC_BASE_URL=https://open.bigmodel.cn/api/paas/v4`,然后在 `llm/client.rs` 加一个 `reasoning_content` delta 的分支(类似 thinking_delta,只是不存 signature)。**这一步当前未实施,留作真机测试出问题时的备选方案**。
- **redacted_thinking 大概率不支持**——GLM 没动机在转译层做安全过滤,只有原 Anthropic 会触发这个 block type。client 端照样发 `redacted_thinking_delta` 事件(空数据流),UI 端接到空数据就当没收到,不影响 LLM 协议(LLM 不发 redacted,自然就没 redacted)。

**客户端对策**(已实施于步骤 6):
- `LlmConfig::from_env` 读 `LLM_THINKING_EFFORT`,默认 `"high"`,可覆盖为 `low` / `medium` / `high` / `xhigh` / `max`
- `max_tokens` 默认 16384(原 1024 撞上限),`LLM_MAX_TOKENS` env 可覆盖
- `ChatRequest` 总是带 `thinking: { type: "adaptive", display: "summarized", effort: <env> }`(无 per-session 开关,见 PRD D1)
- `display: "summarized"` 显式设,保证 `thinking_delta` SSE 流到 UI(Opus 4.7+ 默认 `omitted` 会吞掉摘要文字)
- SSE parser 处理 `content_block_start` block type = `"thinking"` / `"redacted_thinking"`,`content_block_delta` delta type = `"thinking_delta"` / `"signature_delta"`,`content_block_stop` 关闭 thinking 时已经 delta 流式发完 + signature 累积
- `signature` 必须原样回传,agent loop 在 turn 边界把 thinking text + signature 装进 `ContentBlock::Thinking` 写 DB,下次 LLM 调用通过 `toPayloadContent` 带回
- `redacted_thinking.data` 同理,opaque 不解析,verbatim 回传
- 折叠状态 in-memory,刷新重置(见 PRD D2)

**out of scope**(MVP):
- per-session thinking toggle
- `LLM_THINKING=off` kill switch
- 折叠状态持久化
- GLM 原生 OpenAI-compat 端点适配(待真机测试触发)
