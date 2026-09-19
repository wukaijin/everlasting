# Research:OpenAI Responses API provider 接入(2026-09-18)

> 调研人:carlos + ZCode 会话。状态:**已完成,未开工**。结论供 PRD 与后续 design.md 引用。
> 方法:代码侧逐文件勘察 + OpenAI 官方文档(迁移指南 / function calling / 流式事件)+ 生态兼容性检索。

## 0. 结论速览

- **可行且成本可控**:Provider 抽象(trait + wire 中间层 + protocol 字符串分发)就是为加新协议设计的;`openai.rs`(Chat Completions,944 行)的 HTTP/SSE/错误分类/usage 骨架可大量平移。预估后端 ~1000-1500 行(含测试),前端与 spec 小改。
- **架构对齐点**:agent loop 每轮全量回放历史 ⇒ Responses 的 stateless 用法(`store:false` + 全量 input 回放),不引入 `previous_response_id`。
- **两个最大的协议差异**要新写:①工具调用是独立 item(`function_call` / `function_call_output`,靠 `call_id` 关联),不是 `tool_calls[]` 数组;②流式是类型化 SSE 事件(`event:` 行 + 按 `type` 分支),且**无现成 stop_reason,需自行合成**。

## 1. 代码侧现状:触点清单

| 触碰点 | 位置 | 现状与改动 |
|---|---|---|
| 协议枚举 | `app/src-tauri/src/db/types.rs:35` | `ProviderProtocol { Anthropic, Openai, #[cfg(test)] Mock }`;加 `OpenaiResponses` + `as_str() -> "openai_responses"` + `from_str_opt` 分支。DB `providers.protocol` 是 TEXT(前向兼容),**零 schema 迁移** |
| ProviderRow | `db/types.rs:77` | `{protocol: String, display_name, base_url, api_key(serde skip), has_key, disabled}`——协议只是字符串,行结构不动 |
| Provider trait | `llm/provider/mod.rs:118` | `send(system, messages, tools) -> Pin<Box<dyn Stream<ChatEvent>>>` + `capabilities()` + `protocol()`;object-safe,新 adapter 实现即可 |
| 工厂分发 | `llm/provider/mod.rs:193` | `build_provider` 按 `protocol.as_str()` match;加 `"openai_responses"` 分支构造 config(参照 openai 分支 219-246) |
| 现有 OpenAI 适配器 | `llm/provider/openai.rs`(944 行) | Chat Completions 全链路:`build_http_body` 纯函数(openai.rs:153)+ `stream!` 循环(send,openai.rs:578-931)。新 adapter 的模板 |
| wire 中间层 | `llm/provider/wire/`(mod/from_wire/to_wire) | `chat_request_to_wire → strip_unsupported(target_caps) → adapter 自转 HTTP body`;`WireMessage::{User, UserBlocks, Assistant, Tool}`、`WireBlock::{Text, Image, Reasoning, Signature, RedactedThinking, ToolUse}`。原样复用,给 Responses 定义 caps |
| SSE 解析 | `llm/sse.rs` | **直接复用**:parser 支持 `event:` 行(sse.rs:118,Anthropic 在用),Responses 的事件化 SSE 正好;`utf8_chunk_text` 跨 chunk UTF-8 carry 也现成 |
| 流式工具聚合 | `llm/provider/streaming.rs` | `ToolCallBuf` / `accumulate_tool_call_delta`(按 index)/ `build_tool_call_event`;Responses 需按 `item_id`/`output_index` 聚合,模式平移 |
| usage 解析 | streaming.rs `parse_openai_usage` | Chat Completions 的 `prompt_tokens` 系;新增 `parse_responses_usage`(`input_tokens` 系,字段名反而更贴内部 `TokenUsage`) |
| cache-miss 哨兵 | `llm/provider/mod.rs:73` | `warn_on_full_prefix_cache_miss` 两 adapter 对称调用,新 adapter 照抄 Done 前调用 |
| 连通探测 ×2 | `commands/providers.rs:458`(test_model)、`tools/test_llm_connection.rs` | 两处 protocol match,各加 `/responses` 分支(非流式 1-token:取 `output[].content[].text`) |
| 前端 Settings | `app/src/components/settings/ProvidersTab.vue:316-330` | 协议下拉 hardcode 两项(anthropic / openai),加第三项 `openai_responses`;`protocolBadgeClass`(202 行)加分支 |
| 前端 store | `app/src/stores/providers.ts:10` | protocol 是透传 string,注释更新即可 |
| effort 选项 | `app/src/components/settings/ModelForm.vue:198-199` | 现有 `xhigh` 等选项;Responses 只认 `minimal|low|medium|high`,按协议过滤(否则 400) |
| 测试 | `tests_openai.rs`(1110 行)、`tests_wire.rs`(1709 行) | wire JSON 样例驱动风格;新建 `tests_responses.rs` |
| spec | `.trellis/spec/backend/multi-provider-contract/` | 三个 scenario(abstraction / provider-trait-anthropic / openai-wire);新增 scenario-responses-wire |

### base_url 约定(与现有两协议对照)

- Anthropic:`base_url + "/v1/messages"`(anthropic.rs:77,base_url **不含** `/v1`)
- OpenAI CC:base_url **含** `/v1`(如 `https://api.openai.com/v1`),adapter 只追加 `/chat/completions`(openai.rs:125,06-09 修过 `/v1/v1` 双拼 bug)
- **Responses:沿用「base_url 含 `/v1`」约定**,追加 `/responses` ⇒ `https://api.openai.com/v1/responses`。与 openai 条目同语义,用户心智不换。

## 2. Responses 协议差异明细(vs Chat Completions 适配器)

### 2.1 请求体

| 概念 | Chat Completions(现有 openai.rs) | Responses(新) |
|---|---|---|
| 端点 | `POST {base}/chat/completions` | `POST {base}/responses` |
| system | 首条 `role:"system"` message | 顶层 `instructions` 字段 |
| 消息 | `messages: [{role, content}]` | `input`:string 或 **item 数组** |
| 输出上限 | `max_tokens` / o 系 `max_completion_tokens` | `max_output_tokens` |
| 推理档位 | 顶层 `reasoning_effort: "low|medium|high"` | `reasoning: { effort: "minimal|low|medium|high", summary }` 对象 |
| 状态 | 无(天然无状态) | `store`(默认 true!)+ `previous_response_id`;**我们固定 `store:false`** |
| function tool | `{type:"function", function:{name,description,parameters}}` 嵌套 | **扁平** `{type:"function", name, description, parameters, strict}`;省略 strict 会倾向 strict(schema 须全字段 required + additionalProperties:false,不满足自动回退)——**显式 `strict:false`** |
| 图片 | `image_url: {url: data URL}` | content part `{type:"input_image", image_url: "data:...", detail}` |
| 多候选 | `n` | 已移除 |

`ModelRow.thinking_effort` → `reasoning.effort` 映射直接,但注意**词表交集**:Responses 官方接受 `minimal|low|medium|high`;DB 允许的 `xhigh|max`(Anthropic/DeepSeek 词表)会被 400——表单按协议过滤或 adapter 拦截。

### 2.2 input item 类型(全量回放要发的)

- 简单消息:`{role: "user"|"assistant"|"system"|"developer", content: string | parts[]}`(EasyInputMessage,**无 `name` 字段**)
- assistant 历史:`WireBlock::ToolUse` 不挂 message,而是独立 `{type:"function_call", call_id, name, arguments(JSON 字符串)}` item
- 工具结果:`{type:"function_call_output", call_id, output: string}`——对应 wire 的 `WireMessage::Tool {tool_call_id, content}`,**用 `call_id`(不是 item `id`)关联**
- reasoning item(跨轮保真用):`{type:"reasoning", id:"rs_...", summary[], encrypted_content?}`——**MVP 不发**(见 §4.2)

### 2.3 响应 output(非流式 / `response.completed` 里同构)

`output` 是类型化 item 数组(替代 `choices[0].message`):

```json
"output": [
  { "type": "reasoning", "summary": [] },
  { "type": "message", "role": "assistant",
    "content": [{ "type": "output_text", "text": "..." }] },
  { "type": "function_call", "call_id": "call_...", "name": "...", "arguments": "{...}" }
]
```

### 2.4 流式(SSE,`event:` 行 + `data:` JSON)

需按 `event.type` 分支,关心的约 10 种:

| 事件 | 映射到 ChatEvent |
|---|---|
| `response.created` / `response.in_progress` | `Start`(created 时发) |
| `response.output_text.delta` | `Delta { text }` |
| `response.reasoning_summary_text.delta`(+ `.done` / `reasoning_summary_part.added/done`) | `ThinkingDelta { text }`(Responses 只下发推理**摘要**,无原始 CoT) |
| `response.output_item.added`(item.type=="function_call") | 开聚合 buffer(记 call_id/name) |
| `response.function_call_arguments.delta` / `.done` | 聚合 arguments 字符串(按 `output_index`/`item_id`) |
| `response.output_item.done`(function_call) | `ToolCall { id: call_id, name, input }`(可与 arguments.done 二选一做 flush 点) |
| `response.completed` | `Done { stop_reason, usage }` |
| `response.incomplete`(`incomplete_details.reason`,如 `max_output_tokens`) | `Done { stop_reason: "max_tokens" }` |
| `response.failed` / `error` | `LlmError`(`classify_error_response`) |

其余几十种(web_search 系、audio 系、`response.queued`、refusal 系等)忽略——`SseParser` 已容忍未知事件。**没有现成 stop_reason**:output 含 function_call item → `"tool_use"`,否则 `"end_turn"`(归一化成 Anthropic 风格,同 openai.rs:834 的做法)。

### 2.5 usage(`response.completed` 的 `usage` 字段)

```json
{ "input_tokens": N, "input_tokens_details": { "cached_tokens": N },
  "output_tokens": N, "output_tokens_details": { "reasoning_tokens": N },
  "total_tokens": N }
```

比 CC 的 `prompt_tokens` 系更贴内部 `TokenUsage {input_tokens, cache_read_input_tokens, output_tokens}`;`reasoning_tokens` 可并入 output 侧统计口径(与现有 Anthropic thinking token 口径对齐时注意:Anthropic 的 thinking 计在 output_tokens 内,Responses 的 reasoning_tokens 是 output_tokens 的子集,不重复计)。

### 2.6 错误体

`{error: {code, message}}` —— 与 `classify_error_response` 现有 code 提取路径(openai.rs:48-56 注释)一致,直接复用;SSE 客户端参数照搬 RULE-A-011(`read_timeout` 60s / `connect_timeout` 10s)。

## 3. 生态兼容性(2026-09 检索)

- **支持** `/v1/responses`:OpenAI 官方(主推路径,Assistants API 2026-08 已下线);**vLLM 原生兼容**(官方文档明说可用 OpenAI SDK 直连);**LiteLLM** 可作网关路由。
- **参差**:OpenRouter 及第三方部署支持不全(2025-12 仍有兼容性讨论);Scaleway 等兼容实现会静默丢不支持字段。
- **内置工具**(web_search / file_search / computer use / code_interpreter / 内置 MCP)仅 OpenAI 官方原生——本项目只用自定义 function tool,不受影响,但 UI 文案别暗示这些能力。
- 定位建议:新协议条目 = 「OpenAI 官方 + 明确支持 Responses 的网关」,不是现有 openai 条目的平替。

## 4. 设计决策与理由

### 4.1 stateless 全量回放(不 store / 不 previous_response_id)

agent loop 每轮构造全量 messages 调 `send`——这正是 Responses「手动回放」模式。不引入服务端会话状态的好处:与现有 catalog/session 语义零冲突、第三方网关兼容面最大(stateful 支持参差)、故障面小。`store:false` 还避免数据驻留(OpenAI 侧 30 天存储)。

### 4.2 MVP 不回传 reasoning item,拆后续增量(P3)

官方对 stateless 推理模型的建议:`include: ["reasoning.encrypted_content"]`,把加密 reasoning item 原样回传。这要求 `ContentBlock::Thinking` 增加加密 blob 载荷并过 DB/IPC/wire 全链——改动面大且与 Anthropic 的 signature 机制是两套东西。**不回传不报错**(行为略退化:模型丢跨轮推理上下文)。拆为独立 follow-up,等 Responses 链路稳定后再做。

### 4.3 群聊 speaker 归属(与 Anthropic 同款内联;2026-09-18 评审修正)

group-chat(07-29)依赖 CC 的 `name` 字段(openai.rs:174-182 / 326-331);Responses 的 EasyInputMessage **没有 `name`**。方案:`speaker` 以 `@name: ` 文本前缀内联进 content——评审发现这不是「降级」而是与 Anthropic 现行方案同款(anthropic.rs:371 `apply_speaker_prefix` 即 `format!("@{}: {}", ...)`),沿用该格式继承群聊「恰好一次、无双重前缀」既有约定(tests_group_chat_prompts.rs:869),混编群聊两协议渲染一致;原稿的 `Alice: ` 格式与「在 instructions 里说明转录格式」作废(Anthropic 从不加说明,实测跑得通)。

### 4.4 strict:false 显式声明

项目 `input_schema` 是 Anthropic 风格自由 schema(非 strict 兼容);Responses 省略 `strict` 会尝试 strict 再自动回退。显式 `strict:false` 行为确定、不依赖上游回退逻辑。

### 4.5 不在现有 `openai` 协议里做端点 hack

(如 base_url 后缀识别)——与 protocol 字段语义冲突,探测/统计/strip 全乱。独立第三协议,成本只多在一个枚举分支。

## 5. 风险清单

| 风险 | 说明 | 缓解 |
|---|---|---|
| 兼容面窄 | 第三方网关多只有 `/chat/completions` | 定位官方+vLLM/LiteLLM;文档写明 |
| effort 词表冲突 | `xhigh|max` → 官方 400 | 表单按协议过滤选项 |
| 群聊 speaker 丢失 | 无 `name` 字段 | `@name:` 内联,与 Anthropic 同款(§4.3) |
| 流式事件面宽 | 几十种事件类型 | 只处理 ~10 种,parser 容忍未知 |
| stop_reason 需合成 | 无 `finish_reason` 等价物 | output item 类型判定 + incomplete 映射 |
| stateful 诱惑 | previous_response_id 看似省 token(输入仍计费,官方明说) | 坚持全量回放,与架构一致 |
| CC 模型行为差异 | 同模型两 API 行为可不同(LessWrong 报告) | 分开建模为不同协议条目,各自探测 |

## 6. 分期建议

- **PR1(后端主体)**:枚举 + 工厂分支 + `responses.rs` 适配器(stateless、无 reasoning 回传、strict:false、speaker 内联)+ `tests_responses.rs` wire 样例单测(请求构造 + SSE 事件序列 → ChatEvent)。
- **PR2(外围)**:前端下拉/徽标/effort 过滤 + `test_model`/`test_llm_connection` 探测分支 + multi-provider-contract spec + turn-smoke live 验证。
- **PR3(可选 follow-up)**:encrypted reasoning 跨轮回传(include 参数 + Thinking 加密载荷全链)。

工作量预估:PR1 ~800-1200 行(含测试),PR2 ~200-400 行,PR3 另立任务评估。

## 7. 参考

- [Migrate to the Responses API — OpenAI 官方迁移指南](https://developers.openai.com/api/docs/guides/migrate-to-responses)
- [Function calling — OpenAI](https://developers.openai.com/api/docs/guides/function-calling)
- [Responses streaming events — OpenAI API Reference](https://developers.openai.com)
- [vLLM Responses API 兼容性](https://docs.vllm.ai) · [LiteLLM](https://docs.litellm.ai)
- [HN: Responses vs Chat Completions 讨论](https://news.ycombinator.com) · [Sean Goedecke: stateful inference 分析](https://www.seangoedecke.com) · [LessWrong: 行为差异报告](https://www.lesswrong.com) · [社区: stateful 性能报告](https://community.openai.com) · [第三方兼容性讨论](https://github.com)
- 代码侧勘察(2026-09-18):`db/types.rs`、`llm/provider/{mod,openai,anthropic,streaming}.rs`、`llm/sse.rs`、`wire/`、`commands/providers.rs`、`tools/test_llm_connection.rs`、`app/src/components/settings/{ProvidersTab,ModelForm}.vue`、`.trellis/spec/backend/multi-provider-contract/`
