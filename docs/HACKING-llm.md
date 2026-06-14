# HACKING-llm: LLM API 兼容层差异笔记

> 当前实测环境:用 `<your-anthropic-compat-host>` 转发的 **GLM-4.7** 走 Anthropic 兼容协议,**不是真的 Anthropic Claude API**。SSE 协议理论上等价,但错误响应 / HTTP 状态码 / 边界行为有 3 处差异。
>
> 写给未来的自己(或者下个 session),实施 LLM 客户端时别再踩这些坑。
>
> **触发场景**:写 / 改 / 调试 LLM 流式客户端、错误处理、SSE 解析时。

---

## 现状一句话

- **当前 base URL**:`https://<your-anthropic-compat-host>/v1/messages`(从 `ANTHROPIC_BASE_URL` env 读)
- **当前 model**:`GLM-4.7`
- **当前 API key**:环境变量 `ANTHROPIC_API_KEY`(智谱风格 `sk-g4HcGHnrqbc...`,不是 Anthropic 风格)
- **协议**:Anthropic Messages API 兼容,header 用 `x-api-key` + `anthropic-version: 2023-06-01`

未来切真 Claude:改 `ANTHROPIC_BASE_URL` 空、model 改 `claude-haiku-4-5`、key 换 Anthropic 的 `sk-ant-...`。重测下面 3 处差异。

---

## GLM 兼容层 3 处差异(vs 真 Anthropic Claude)

### 差异 1:401 的 `error.type` 字段

| | GLM-4.7 (`<your-anthropic-compat-host>`) | Anthropic 真 Claude(预期) |
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

**客户端对策**:**实施时不要在客户端做 max_tokens 上限预检**,由 server 报(切到真 Claude 时它会报;`<your-proxy>` 不报,那就不预检,server 决定)。

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
- [ ] **Model 从 env 读**(`LLM_MODEL` 或类似),默认 `GLM-4.7` 兼容 `<your-proxy>`
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

---

## 差异 5:OpenAI Chat Completions 协议差异(2026-06,06-08-multi-model PR3)

**场景**:PR3 起支持 OpenAI 官方 `https://api.openai.com/v1/chat/completions` 端点,以及所有 OpenAI-兼容(DeepSeek / GLM 原生 / OpenRouter 等)。协议与 Anthropic Messages API 差异较大,**单独实现 `OpenAIProvider` + WireMessage 中间层**做互转。详细 spec 见 `.trellis/spec/backend/llm-contract.md` "Scenario: OpenAI Chat Completions adapter + cross-protocol WireMessage (PR3)",本节只记实测坑点。

**关键差异速查**(Anthropic → OpenAI):

| 维度 | Anthropic | OpenAI |
|---|---|---|
| endpoint | `POST {base}/v1/messages` | `POST {base}/v1/chat/completions` |
| 鉴权 | `x-api-key: <key>` + `anthropic-version` | `Authorization: Bearer <key>` |
| system prompt | 顶层 `system` 字段 | 第一条 `role: "system"` message |
| tools 字段名 | `input_schema` | `parameters`(在 `function` 包裹里) |
| tool call 入参 | `input` 是 JSON object | `arguments` 是 JSON **string** |
| tool result | `user` message 里 `tool_result` block | 独立 `role: "tool"` message |
| 流式 event | `event: foo\ndata: {...}\n\n` 多 event | `data: {...}\n\n` 单一格式 |
| text delta | `content_block_delta.text_delta` | `choices[0].delta.content` |
| reasoning | `thinking_delta` 块 | `choices[0].delta.reasoning_content` (o1/o3) |
| finish | `message_delta.stop_reason` + `message_stop` | `choices[0].finish_reason` + `data: [DONE]` |
| thinking 顶层字段 | `thinking: {type, display, effort}` | `reasoning_effort: "low\|medium\|high"` |

**坑点 1:OpenAI tool_calls 增量 JSON 解析**

OpenAI 流式 `tool_calls[]` 每元素可能只含 `id`(第一个 chunk)或只含 `function.arguments` 增量(后续 chunk),需要按 `index` 维护 state machine(`ToolCallBuf` HashMap per `tool_call_index`)。**不能假设一个 chunk 包含完整 `tool_call`**。

**坑点 2:并行多 tool call**

OpenAI `tool_calls[0].index=0` 和 `tool_calls[1].index=1` 可在同一 chunk 里同时增量。Anthropic 协议是单 tool_use per block,无此情形。`ToolCallBuf` 必须按 `index` 独立累积。

**坑点 3:`data: [DONE]` 哨兵**

OpenAI 流末尾发 `data: [DONE]\n\n`,**不**是 JSON。`SseParser` 解析出 `data: "[DONE]"`,`OpenAIProvider` 必须识别并 emit `Done` event 后退出循环。**不能**用 serde_json 解析 `[DONE]`,会失败。

**坑点 4:错误响应 body 格式**

Anthropic / GLM:`{"type": "error", "error": {"type": "invalid_request_error", "message": "..."}}`
OpenAI:`{"error": {"message": "...", "type": "...", "code": "invalid_api_key" | "rate_limit_exceeded" | ...}}`

`classify_error_response` 扩展读 `error.code` + `error.type` 双字段(PR3 改动,见 `llm/error.rs`)。

**坑点 5:`reasoning_effort` vs `thinking` 字段互转**

OpenAI 用顶层 `reasoning_effort: "low|medium|high"`(o1/o3 系列),Anthropic 用嵌套 `thinking: {type: "adaptive", display, effort}`。`ModelRow.thinking_effort` 一个字段同时承担两侧(PR3 决议 D3):
- Anthropic 路径 → `thinking.adaptive.effort = <model.thinking_effort>`
- OpenAI 路径 → 顶层 `reasoning_effort = <model.thinking_effort>`

**坑点 6:跨协议降级(in-memory,不持久化)**

切 model 时,`provider.send()` 内部 `strip_unsupported(target_caps)` 一次,DB 不动。规则(`wire.rs::strip_unsupported`):
- `Reasoning` block → `target.supports_thinking || target.supports_reasoning_effort` 时保留,否则丢
- `Signature` / `RedactedThinking` → 只 Anthropic + supports_thinking 保留,OpenAI 丢(opaque 不可转)
- `ToolUse` / `ToolResult` / `Text` → 全部保留

**留待真机测试**(out of scope):
- `max_completion_tokens` 字段(o1+ 模型需要,`max_tokens` 不接受)— 留未来 PR
- `parallel_tool_calls: true` 显式声明(目前默认 true)— 验证
- `tracing` 显式 redact api_key — 留 future

---

## 客户端陷阱(FU-5/6 沉淀)

### 陷阱 1:`Option<T>` 字段 Tauri 2 IPC null 行为

**现象**:Rust 端 `model: Option<String>`,JS 端显式传 `null`:
```ts
invoke("create_session", { ..., model: null })
```
报错:`command create_session missing required key ` (key 名字段打印为空字符串)。Tauri 2 IPC 把 JS `null` 当 missing required 处理,**且错误打印的 key 名字段被吞掉**,所以看不到"missing model"这种明确信息。

**根因**:Tauri 2 IPC 在处理 `Option<T>` 时,JS 端传 `null` 的语义是"显式 None",但当前版本把它解析为 missing required。配合上述"key 名字段打印为空"的 bug,排查非常隐蔽。

**修法**:JS 端**省略**字段不传,让 Rust 端按 `Option::None` 走 default:
```ts
// 错误
invoke("create_session", { projectId, initialCwd, model: null })
// 正确
invoke("create_session", { projectId, initialCwd })  // 省略 model
// Rust 端兜底:
let model = model.unwrap_or_else(|| state.config.model.clone());
```

**影响范围**:本项目所有 `Option<T>` 参数 + 任何未来 Rust 命令显式接 `Option<T>` 的字段。**避免在 JS 端用 `null` 显式置空 `Option` 字段**。

**验证**:`pnpm tauri dev` 时 F12 console 看到 `missing required key` 立刻检查是否传了 `null` 给 `Option` 字段。

**经验沉淀**:3b-1 PR2 实施的 3 个 hotfix 之一(post-fixes commit `18354a0` 修法 #2)。详见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md FU-5](../_archive/2026-06-3b-1/FOLLOW-UP.md#fu-5--optiont-tauri-2-ipc-null-行为)。

---

### 陷阱 2:Anthropic tool_result 块只能在 user role

**现象**:Anthropic Messages API 严格规定 `tool_result` 块只能出现在 user role message 里。assistant role message 含 `tool_result` 块 → 2013 错误:
```
请求无效: invalid params, tool result's tool id(call_xxx) not found (2013)
```

**根因链**(本项目实际撞过):
1. UI 端需要 assistant message 1 上能查到"done / running"状态(对应 tool_result 跟没跟到)
2. `rehydrateMessages` 把 user message 2(tool_result-only "ghost")的 `toolResults` push 到上一个 assistant message 1 上做 UI grouping
3. 但**没清空** user message 2 自己的 `toolResults`
4. `toPayloadContent` 之前对 assistant / user 走同一条代码路径,把 assistant message 1 上的 `toolResults` 也喂给 LLM
5. 第二次发消息时 LLM 看到 assistant role message 含 tool_result 块,**违反协议** → 2013

**修法**:`toPayloadContent` 按 role 分发:
- assistant role: emit thinking / text / **tool_use** / redacted_thinking,**跳过 `m.toolResults`**(UI grouping 用,不上 wire)
- user role: emit text / thinking / **tool_result** / redacted_thinking

修完后 LLM 看到的 messages 顺序:`assistant(tool_use + text) → user(tool_result) → user(新消息)`,符合 Anthropic 协议。

**影响范围**:任何把 tool_result 跨 role 边界做 UI 关联的框架(我们的 rehydrate 模式、或者别的 chat framework 的 tool use 状态管理)。

**验证**:写 PR 时,在 `check.jsonl` 加"toPayloadContent / 对等函数按 role 分发 tool_result"作为硬约束。

**经验沉淀**:3b-1 PR2 实施的 3 个 hotfix 之一(post-fixes commit `18354a0` 修法 #3)。详见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md FU-6](../_archive/2026-06-3b-1/FOLLOW-UP.md#fu-6--anthropic-tool_result-块只能出现在-user-role)。

---

### 陷阱 3:cancel / 网络断留下 orphan `tool_use` → 2013 "tool call result does not follow tool call"

**现象**:Anthropic Messages API 严格要求 `tool_result` 块必须跟在对应的 `tool_use` 之后。如果 `assistant` 消息含 `tool_use` 但下一条 `user` 消息不含匹配的 `tool_result` → 2013:

```
status=400 Bad Request
body={"error":{"type":"<nil>","message":"invalid params, tool call result does not follow tool call (2013) (request id: 202606080518519376687798268d9dPCqSxy8i)"},"type":"error"}
```

**注意**:这条错误信息和陷阱 2 的"tool result's tool id not found"长得像,但**根因不一样**:
- 陷阱 2:`tool_result` 出现在 `assistant` role(协议错位)
- 陷阱 3:`tool_use` 后面**根本没有** `tool_result`(协议缺失)

**根因链**(本项目实际撞过):
1. LLM 流式输出 `tool_use(read_file)`(SSE → `ChatEvent::ToolCall`),累积到后端 `tool_calls: Vec<(id, name, input)>`
2. **在这个时点之前** cancel 触发:`Stop` 按钮 / `attach_worktree` 的 in-flight cancel / 网络断 / agent error
3. PR5 取消路径在 `lib.rs` 把 `assistant_blocks`(含 `ToolUse`)`persist_turn` 写进 DB,然后 `return`
4. **没有**走到执行 tool + 构造 `tool_result` + `persist_turn` 的步骤 — DB 里**孤儿 `tool_use`**
5. 下次 send 时,前端 `controller.refresh` 从 DB 重新 load,rehydrate 出**孤儿 `tool_use`** 推到 history
6. LLM 看到 `assistant: [tool_use(id=X)]` 后面是 `user: "新消息"`,缺 `tool_result(id=X)` → API 2013

**修法**:**B + C 双层防护** — 后端修新产生的孤儿,前端治历史孤儿。

- **B (后端)**:在 `app/src-tauri/src/lib.rs` 的 cancel 分支里,如果 `tool_calls` 非空,先构造一个 synthetic `user(tool_result)` user message,跟原 assistant 一起 `persist_turn`。synthetic 块:`role=User, is_error=true, content="Tool execution was interrupted: ... The tool <name> did not run."`。抽成 helper `build_synthetic_tool_result_message` 方便单测。详见 `tests::synthetic_tool_result_message_*` 4 个单测。
- **C (前端)**:在 `app/src/stores/streamController.ts` 的 `rehydrateMessages` 里,merge step 之后加一段"orphan tool_use repair" — 反向扫 `out` 数组,找 `assistant(toolCalls)` 但紧邻的下一条 user 消息没匹配 `tool_results` 的情况,splice 一条 synthetic user message 进去。**反向扫**避免 splice 索引错位。详见 `src/stores/streamController.test.ts` 8 个 vitest。

**为什么 B + C 都做**:
- B 单做:新 cancel 不再产生孤儿,但**用户本地 DB 里已有历史孤儿**(cancel 路径下,合成 tool_result 之前留下的旧 session 行) 仍然会让下次 send 报 2013
- C 单做:治历史孤儿,但**不防新孤儿**(如果 C 漏了某条边角 case,新产生的孤儿会再次触发 bug)
- 双做:C 治本(历史),B 治标(新);C 是兜底,C 失败 B 也能挡住,B 失败 C 也能挡住

**synthetic tool_result content 措辞**:英文 + tool name。理由:synthetic content 直接进 Anthropic 协议流,跟 LLM-compatible 提示符风格一致;带 name 让 LLM 知道是哪个工具没跑(避免 LLM 误以为 read_file 跑了但只是 result 是"interrupted")。

**为什么是 `is_error: true`**:Anthropic 协议把 `is_error` 视为 tool 失败的强信号。LLM 看到 `is_error=true` 的 tool_result 会**重发**该 tool_use 而**不是**用空 result 继续推理(参考 Anthropic tool use 文档 "Tool results and error handling" 节)。配合英文提示 "did not run",LLM 通常会决定重发 tool_use,行为符合预期。

**影响范围**:任何有"cancel / 异步中断 / 长时间工具"的 agent 框架。这是 Anthropic 协议强约束,绕不过去;只能让 message 序列自洽。

**验证**:写 PR 时,在 `check.jsonl` 加"cancel 分支必须 persist synthetic tool_result / rehydrate 必须 splice 孤儿"作为硬约束。

**经验沉淀**:Step 4 follow-up (06-08) 修法。`app/src-tauri/src/lib.rs` `build_synthetic_tool_result_message` + `app/src/stores/streamController.ts` orphan repair step。commit hash 见 `git log --oneline | head -5`。

### 陷阱 4:正常完成路径下 in-memory 累积形态 vs DB 拆分形态不一致 → 2013 "tool call result does not follow tool call" (2026-06-08 step 4 follow-up)

**现象**:跟陷阱 3 是**同一个错误码** ("tool call result does not follow tool call", 2013),但**完全不同的复现路径**。陷阱 3 修完后,**正常完成**的 multi-turn session(用户不发 `Stop`、LLM 也成功返回) 在第二次 `send()` 时仍可能 2013。复现路径:

1. 在已 attach worktree 的 session,第一次 send:用户说"确认一下当前 worktree" → LLM 调 `shell` 跑 `pwd && git rev-parse ...` → LLM 看到结果后第二次 LLM call 返回 text "当前 worktree 信息确认如下:..."
2. **正常完成**,没 cancel、没网络断。DB 序列: 2 条独立 `assistant` message(一条含 `tool_use` + `tool_result`, 一条是 text only)。
3. 第二次 send:用户说"帮我随便改下 README.md" → 前端 `ensureLoaded` 走 in-memory 缓存(不 rehydrate from DB)→ 拿到的不是 DB 的 2 条独立 assistant, 而是 1 条**累积**的 `assistantMsg` placeholder(含 `toolCalls` + `toolResults` + turn 1 + turn 2 text)
4. `toPayloadContent` for `assistant` role 按 Anthropic 协议**不**发 `m.toolResults` → wire 上 `tool_use` 后面没 `tool_result` → 2013

**注意**:这条错误信息和陷阱 3 的 `tool_use 后面没 tool_result` 长得**几乎一模一样**,但**根因完全不一样**:
- 陷阱 3:`tool_use` 后面**根本没**`tool_result`(DB 缺)
- 陷阱 4:`tool_result` 在 DB 里有,但**前端 in-memory 没正确反映 DB 拆分**;`toPayloadContent` 按协议只发 `tool_use` 不发 `tool_result`,所以 wire 上看似孤儿

**根因链**:
1. `streamController.handleToolCall` / `handleToolResult` 累积到 `last = msgs[msgs.length-1] = assistantMsg placeholder`(`app/src/stores/streamController.ts:494-518`)
2. `handleChatEvent` for `delta` 累积 text 到**同一个** placeholder(`app/src/stores/streamController.ts:440-442`)
3. 后端 agent loop 每个 turn **单独** persist 一个 assistant message 到 DB(`app/src-tauri/src/lib.rs:1413-1424`)
4. **结果**:in-memory placeholder 累积了所有 turn 的 `toolCalls` + `toolResults` + text;DB 实际是 N 条独立 assistant message
5. `toPayloadContent` for `assistant` role 按协议只发 `thinking` / `text` / **`tool_use`** / `redacted_thinking`,**跳过** `m.toolResults`(`app/src/stores/chat.ts:519-528`,陷阱 2 的设计)
6. 第二次 `send()` 时 `ensureLoaded` 走 in-memory 缓存路径(不 rehydrate from DB),placeholder 累积形态进 history
7. wire 序列: `assistant(text + tool_use)` → `user(text 新消息)`,**没有** `user(tool_result)` → 2013

**修法**:`streamController.finalizeRequest` (the function `done` / `error` / catch-error paths all route through) 配对调两个 action:
1. `evict(sessionId)` — 清空 `messagesBySession` / `loadedFromDb` / `pinnedSessions`, 下次 `ensureLoaded` 走 `invoke("load_session", ...)` + `rehydrateMessages` 拿 DB 拆分形态
2. `useChatStore().invalidateDiff(sessionId)` — 清空 diff cache, worktree chip 的 `diff (N)` 计数器重新 fetch(顺手修另一个 bug: commit 之后 chip 不消失)

```ts
// app/src/stores/streamController.ts (excerpt)
function finalizeRequest(requestId: string, sessionId: string, _errored: boolean): void {
  activeRequests.delete(requestId);
  pinnedSessions.delete(sessionId);
  evict(sessionId);
  useChatStore().invalidateDiff(sessionId);
}
```

**为什么是 finalizeRequest**: 三个 caller (`handleChatEvent.done` / `handleChatEvent.error` / `startRequest` 的 catch 块) 都调它, 改一个地方三个路径都覆盖。不能再"碰巧"靠 attach_worktree 的 `controller.refresh` 同步(陷阱 3 修完后**第一次** send 之后用户通常会 attach worktree,这条路径**碰巧**工作; 但用户也可能不 attach, 第二次 send 直接 2013)。

**为什么两个 action 必须配对**:
- `evict` 单做: 修 2013, 但 commit 之后 chip 仍显示陈旧 `diff (N)`(pre-existing 缓存 bug, 在 06-08-6px 那个 spike 没复现是因为用户没在 worktree 里 commit)
- `invalidateDiff` 单做: 修 chip 缓存, 但 2013 仍出
- 配对做: 两个 bug 一起修, 拆开任何一个都会退化一个

**性能 cost**: 每次 send 完成多 1 次 `load_session` IPC round-trip + 1 次 DB read。实测 IPC < 5ms、SQLite 7 messages < 1ms。LRU cache 已经在 `loadedFromDb` 上, evict 不影响其他 session。

**为什么不是重构 in-memory 累积形态**: 把 placeholder 拆成"每 turn 独立 ChatMessage"是更大的架构改动(需要 streamController 维护一个 `currentAssistantTurn` 状态机, `handleChatEvent.done` 之前要先 `msgs.push` 累积的 placeholder 转成 DB 形态)。本任务**不**做这个, **更小**的修法是"send 完就清", 状态机只活在当次 send 期间, 简单可验证。

**影响范围**: 任何"in-memory 累积 streaming 状态 + DB 拆分持久化"组合的 chat framework。我们项目里两个 store(`streamController` / `chat`)的状态必须配对管理。

**验证**: 写 PR 时, 在 `check.jsonl` 加 "finalizeRequest 必须配对 evict + invalidateDiff / in-memory 跟 DB 必须 evict 后 re-load 才一致"作为硬约束。`streamController.test.ts` 的 `finalizeRequest` describe block 锁住 3 个 invariant(evict 单独、invalidateDiff 单独、配对 invariant)。

**经验沉淀**: Step 4 follow-up (06-08) 修法。`app/src/stores/streamController.ts` `finalizeRequest` 改 + `app/src/stores/chat.ts` 新增 `invalidateDiff` action + `app/src/stores/streamController.test.ts` 加 3 个 vitest + `.trellis/spec/frontend/state-management.md` "Send completion invalidation" 章节 + `.trellis/spec/backend/llm-contract.md` Scenario 7 "In-memory must mirror DB on send completion" sub-section。


### 陷阱 5:OpenAI adapter `endpoint()` 重复拼 `/v1/` → 404 "path not found: /v1/v1/chat/completions" (2026-06-09 fix-session)

**现象**:用户点"新 session" → 输入消息发送 → 页面上: 用户消息 + 红色 Stop 按钮闪一下 → 立即变空 session 状态。切换 session 回来: 只有用户消息, **无任何 assistant 回复**。`test_model` 按钮显示 OK(对同一个 model 测连通性也是 OK 的)。

**根因**:`OpenAIConfig::endpoint()` 返回 `base_url + "/v1/chat/completions"`,但**真实 OpenAI 兼容 provider 的 `base_url` 已经包含 `/v1`**(PR1 seed `https://api.openai.com/v1`、用户 `https://<your-openai-compat-host>/v1`、所有 OpenAI 兼容代理都是这格式),所以拼出来是 `/v1/v1/chat/completions`,upstream 404。

**为什么 `test_model` 不出问题**:`lib.rs::test_model` 走的是**另一段代码**(`format!("{}/chat/completions", provider.base_url.trim_end_matches('/'))`,**没有** `/v1/`)。chat 走 `OpenAIProvider::endpoint()`(有 `/v1/`)。**两个地方对 OpenAI URL 的拼接方式不一致**,test 路径正确、production 路径错误。

**为什么 Anthropic 没出问题**:`AnthropicConfig::endpoint()` 也是 `base_url + "/v1/messages"`,但 Anthropic 的 PR1 seed 是 `https://api.anthropic.com`(**无** `/v1`),所以 `base_url + "/v1/messages"` 拼出来是 `https://api.anthropic.com/v1/messages` ✓。**两种 protocol 的 `base_url` 约定不对称**:Anthropic 用裸 host、OpenAI 用 `host/v1`,**恰好**让 Anthropic 那边 endpoint 重复加 `/v1/` 也能 work,OpenAI 那边就破。

**为什么是 "空 session 状态" 不是 "红色 error message"**:SSE 流从来没被打开(stream parse 阶段就 404 走了 `classify_error_response` 的 `InvalidRequest` 分支),`ChatEvent::Start` 都没 yield,前端 `handleChatEvent.done` / `.error` 都没进。**但** 8509bff 的 2013 wire invariant fix 在 `finalizeRequest` 里调了 `evict(sessionId)`,**就算** stream parser 正常跑,成功完成后也会 evict 让 cache 失效。这个 evict 在 error path 上也一样跑(三个 caller:`done` / `error` / catch),所以 SSE 404 → `ChatEvent::Error` → `finalizeRequest(_, _, true)` → `evict` → cache 清空 → 页面立刻变空状态 → 切换 session 回来 `ensureLoaded` 走 DB 只看到用户消息(assistant turn 没 persist,因为 LLM 都没成功返回)。**两个独立的 fix(evict + endpoint 修)叠在一起**才让症状是"空状态"而不是"红色 error message"。

**复现命令**:
```bash
# 直接跑 openai.rs::tests::live_openai_compat_smoke_test(默认 skip,设 4 个 env var)
EVERLASTING_RUN_LIVE_OPENAI_TEST=1 \
  EVERLASTING_LIVE_OPENAI_BASE_URL=https://api.openai.com/v1 \
  EVERLASTING_LIVE_OPENAI_API_KEY=sk-... \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib live_openai_compat_smoke_test -- --nocapture
# 修前: Err(InvalidRequest("path not found: /v1/v1/chat/completions"))
# 修后: [Start, Delta("还没"), Delta("吃呢..."), Done { stop_reason: "end_turn" }]
```

**修法**:`OpenAIConfig::endpoint()` 改成 `base_url + "/chat/completions"`(不重复加 `/v1/`),跟 `test_model` / `test_provider` 拼接方式对齐。同时更新 `.trellis/spec/backend/llm-contract.md` Protocol differences table 把 OpenAI URL 那行从 `"+ "/v1/chat/completions"` 改成 `"+ "/chat/completions"` + 新加一段 "`base_url` convention is per-protocol, NOT symmetric" 说明两种 protocol 的 seed base_url 形状。

**回归测试**:`openai::tests::endpoint_does_not_double_prefix_v1_when_base_url_includes_v1` 锁住 `base_url = "https://api.openai.com/v1"` 和 `"https://api.deepseek.com/v1"` 两种真实 base_url shape 都只拼一次 `/v1/`。同时把老的 `endpoint_trims_trailing_slash` / `endpoint_uses_provided_base_url` 测试用例的 base_url 从 `https://x.com/` / `https://x.com/openai`(无 /v1,触发旧 bug 行为)更新到 `https://x.com/v1/` / `https://x.com/openai/v1`(有 /v1,真实场景)。

**经验沉淀**:**"base_url 约定" 必须 explicit,不要从 seed 形状或单条 test 里 infer**。这次 bug 之所以 264 个 cargo test + 55 个 vitest 都没抓到,是因为:
1. `endpoint_trims_trailing_slash` / `endpoint_uses_provided_base_url` 用的是无 `/v1` 的 base_url,只测了"加 /v1 之后能 trim 尾斜杠" / "自定义 host 工作"——**没**测"base_url 已经有 /v1 时不要重复加"这个最关键的 invariance。
2. 跨模块 lint 没有: `OpenAIConfig::endpoint()` 和 `test_model` 里 `format!("{}/chat/completions", ...)` 是两段独立代码, 共享一个隐式约定但没共享一个 helper / 一个常量。
3. live test 默认 skip,本地开发时不会跑真 endpoint。

**未来防护**:
- OpenAI / Anthropic 各抽一个 `pub fn chat_completions_url(base_url: &str) -> String` / `pub fn anthropic_messages_url(base_url: &str) -> String` helper,在 `lib.rs::test_model` / `test_provider` 和 `provider::*` adapter 里都调它,保证单一来源。
- `openai::tests::live_openai_compat_smoke_test` 在 CI 上默认开(env-driven,不泄露真 api_key / 私人 endpoint;只对 staging 仓库,对 prod 关,避免烧钱)
