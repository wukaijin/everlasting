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

