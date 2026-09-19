# Scenario: OpenAI Responses adapter — stateless full-replay wire (task 09-18-openai-responses-provider)

> 第三协议 `openai_responses`,与 [scenario-openai-wire](./scenario-openai-wire.md)
> (Chat Completions)同构不同形:复用同一 wire 管线与 `SseParser`,请求形状
> (item 数组)与事件机(typed SSE)是两处协议特有面。
> 实现单点:`app/src-tauri/src/llm/provider/responses.rs` +
> `streaming.rs::parse_responses_usage` + `tests_responses.rs`(28 用例)。
> 设计出处:任务目录 `design.md` §2、`research.md` §2/§4、`review.md`(2026-09-18
> 群聊评审,六项修正已全部落回实现与本文)。

## 1. Scope / Trigger

涉及以下任一时读本文:

- 改 `responses.rs` 的请求形状(`build_http_body`)或 SSE 事件机(`ResponsesStreamState`)
- 改 `WireCapabilities` 或 `strip_unsupported` 的 Reasoning/Signature 裁剪语义
- 动 `test_model` / `test_llm_connection` 的 Responses 探测分支
- 为 Responses 增加 reasoning 回传(P3 follow-up,encrypted_content)

## 2. Signatures

```rust
// app/src-tauri/src/llm/provider/responses.rs
pub struct ResponsesConfig {          // 独立 struct,故意不复用 OpenAIConfig:
    pub base_url: String,             //   base_url 含 /v1(与 openai 协议同约定)
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,              // 上 wire 变成 max_output_tokens
    pub reasoning_effort: Option<String>, // 直传 ModelRow.thinking_effort,无默认
    pub supports_images: bool,        // B1 语义,门 strip 的图片占位降级
}

impl ResponsesProvider {
    pub(crate) fn build_http_body(wire: &WireRequest, config: &ResponsesConfig) -> Value; // 纯函数
}
pub(crate) fn responses_caps(reasoning_effort: Option<&str>, supports_images: bool) -> WireCapabilities;
pub(crate) fn normalize_responses_effort(raw: Option<&str>) -> Option<String>;
// ResponsesStreamState::handle_event(&mut self, event_type: &str, data: &Value)
//   -> Vec<Result<ChatEvent, LlmError>>   // 测试直接喂合成事件,无 HTTP

// streaming.rs(与 parse_openai_usage 并排)
pub(crate) fn parse_responses_usage(v: &Value) -> Option<TokenUsage>;

// db/types.rs:ProviderProtocol::OpenaiResponses,as_str() == "openai_responses"
// provider/mod.rs:build_provider 的 "openai_responses" 分支(模块声明 pub mod 无 cfg 门)
```

工厂缺省与 openai 分支对称:`max_tokens` 缺省 `anthropic::DEFAULT_MAX_TOKENS`;
`reasoning_effort` 直传 Option 不设默认(Anthropic 分支才默认 "high")——未设 =
不发 `reasoning` 对象,非推理模型不受影响。

## 3. Contracts

### 3.1 管线(与 openai.rs 同构)

```
ChatRequest → chat_request_to_wire → strip_unsupported(responses_caps) → build_http_body
  → POST {base}/responses (stream:true, Bearer) → SseParser(typed, event: 行)
  → ResponsesStreamState::handle_event → ChatEvent 流
```

- base_url **必须已含 `/v1`**;adapter 只追加 `/responses`(06-09 `/v1/v1` 教训,
  `endpoint()` 里 trim 尾 `/`)。
- HTTP 骨架照搬 openai.rs:`read_timeout(60s)` + `connect_timeout(10s)`
  (RULE-A-011,read_timeout 而非总 deadline)、`classify_error_response`、
  D5 full-prefix cache-miss 哨兵(Done 事件**前**触发)、跨 chunk UTF-8 carry。
- Start 事件不在 HTTP 200 时立即发——等 `response.created` 到达(与 CC 的差异点)。
- 发送前有 wire 层顺序守卫(`wire::orphan_tool_call_order`,to_wire.rs:146):
  assistant(tool_calls) 未被紧邻的 tool-result 跟随时 `tracing::error!`,不 mutate。

### 3.2 Caps 档与 Reasoning 块裁剪路径(review.md 修正 1 —— 改 strip 语义前必读)

| 字段 | 值 | 说明 |
|---|---|---|
| `supports_thinking` | `false` | wire 载荷不携带 thinking 块(MVP 不回传 reasoning item) |
| `supports_reasoning_effort` | `reasoning_effort.is_some()` | 决定 `reasoning` 对象是否发送 |
| `supports_thinking_signatures` | `false` | signature/redacted blob 仅 Anthropic 能往返 |
| `supports_images` | `model_row.supports_images` | false 时 strip 换文本占位 |

**Reasoning 块是两层合力裁剪,不是 strip 单层**——`block_supported`
(wire/to_wire.rs:649,Reasoning 臂在 :652)对 Reasoning 是 **OR 语义**
(`supports_thinking || supports_reasoning_effort`):

1. effort 已设(推理模型主场景)→ strip **保留** Reasoning 块;
2. 真正丢弃它们的是 `build_http_body` 里 `assistant_blocks_to_responses_items`
   的防御性跳过;
3. `Signature` / `RedactedThinking` 才是 strip 层裁的(`supports_thinking_signatures:
   false`),build 层的对应臂只是 belt-and-suspenders。

AC4(跨协议切换不 400)的保障 = strip 裁 signature + build 层跳过 reasoning,
**两者缺一不可**;单测 `build_http_body_anthropic_history_blocks_absent_in_body_ac4`
的断言对象是 `build_http_body` 的输出 body(同时覆盖两层),不是 post-strip wire。

**与 CC 路径的不对称(留痕,rule-d-006)**:同为幸存的 Reasoning 块,Chat
Completions 是**回放**(提升为顶层 `reasoning_content`,openai.rs),Responses 是
**丢弃**。P3 做 encrypted reasoning 回传(`include: ["reasoning.encrypted_content"]`
+ Thinking 块加 blob 载荷)时必须先消化这个差异,否则两协议历史语义漂移。

### 3.3 请求形状(build_http_body)

顶层:`model` / `input`(item 数组,全量回放)/ `max_output_tokens` /
`store: false`(**恒发**——Responses 默认 store:true 且 previous_response_id 链
只在 stored 响应上可用,我们 stateless)/ `stream: true`。
`instructions`(= wire.system)仅当 system 存在;`tools` 仅当非空;
`reasoning: { effort, summary: "auto" }` 仅当 effort 归一后存在
(summary:auto 才会下发 `reasoning_summary_text` 事件流)。**不发**
`previous_response_id`。

**effort 词表归一**(`normalize_responses_effort`,adapter 侧兜底;表单过滤是
第一道防线,见 §3.7):`minimal|low|medium|high` 原样;`xhigh|max` → `high` +
`warn!`(不静默);其他未知值 → None(整个 reasoning 对象不发)+ `warn!`;
`None`/空串 → None 静默。

input item 映射(保相对顺序):

| wire 形态 | Responses item |
|---|---|
| `User { speaker, text }` | `{role:"user", content:"@<speaker>: <text>"}`;speaker None → 原文。**格式 = anthropic.rs `apply_speaker_prefix`(:377)的 `@name: `,与 CC 的 `name` 字段不同源**,混编群聊不漂移(review.md 修正 3) |
| `UserBlocks { blocks }` | `{role:"user", content:[parts]}`:Text→`input_text`(空文本跳过)、Image→`{type:"input_image", image_url:"data:<mt>;base64,…", detail:"auto"}`;`cache_control` 无 Responses 等价物,丢弃;其余块防御跳过 |
| `Assistant` 的 Text 块 | 合并为**一个** `{role:"assistant", content:"@<speaker>: <joined>"}` item(零文本纯工具轮也发,content:"" 是合法 EasyInputMessage——item 次序是上游回放预期) |
| `Assistant` 的每个 `ToolUse` | 独立 `{type:"function_call", call_id, name, arguments}` item,排在该 assistant message item 之后(先 text 后 calls;arguments 是重序列化 JSON **字符串**——wire 里是 parsed JSON) |
| `Tool { tool_call_id, content, images }` | `{type:"function_call_output", call_id, output}`;output 仅字符串,工具结果图片降级为逐图通知行前置(与 CC 同措辞);关联键是 `call_id` |
| `Reasoning` / `Signature` / `RedactedThinking` / assistant 位 `Image` | 跳过(见 §3.2 两层裁剪) |

tools 扁平化:`{type:"function", name, description(缺省空串), parameters,
strict: false}`——**无** CC 的 `function:{…}` 信封;`strict:false` 显式发
(schema 不满足 strict 前提,显式 false 不赌上游静默回退,research §4.4)。

### 3.4 事件机(typed SSE;判别字段 = data.type,event: 行仅兜底)

| SSE `data.type` | 映射 |
|---|---|
| `response.created` | `ChatEvent::Start`(恰一次;saw_created 去重,重复 created 不重置 UI) |
| `response.output_text.delta` | `Delta { text }`(空 delta 忽略) |
| `response.reasoning_summary_text.delta` | `ThinkingDelta { text }`(接收方向不受 caps 影响——caps 只管出站载荷) |
| `response.output_item.added`(item.type==function_call) | 开 `FunctionCallBuf`(模块私有,以 **`output_index`** 为键——Responses 无 CC 的 `index`);call_id/name 可在 added item 上预取 |
| `response.function_call_arguments.delta` | 追加该 output_index 的 arguments 串 |
| `response.function_call_arguments.done` | **显式不 flush**(`output_item.done` 是唯一 flush 点,双 flush 会重复发 ToolCall) |
| `response.output_item.done`(function_call) | flush `ToolCall { id: call_id, name, input }`;buffer 缺字段时逐字段回退 done item(容错只发 done 的网关);name 为空 → 丢弃 + `warn!` |
| `response.output_item.done`(message) | 扫 content parts:`{type:"refusal", refusal:"…"}` → refusal 文本作为一条**可见 `Delta`** 发出(review.md 修正 5:否则零 Delta → completed → 空气泡/群聊空发言;task owner 拍板转 Delta 而非 LlmError),之后正常 completed |
| `response.completed` | `Done { stop_reason: 合成(§3.5), usage: parse_responses_usage }` |
| `response.incomplete` | `Done { stop_reason: incomplete_details.reason=="max_output_tokens" → "max_tokens",否则保守 "end_turn", usage }` |
| `response.failed` / `error` | `LlmError`,经 `classify_error_response(500, body, None)` 分类(response.error.code 同 CC 约定,rate_limit 仍归 RateLimit) |
| 其余几十种(queued / in_progress / web_search_* / audio_* …) | 忽略(SSE parser 容忍未知事件) |

附加防御:

- 网关乱入 CC 风格 `data: [DONE]` 哨兵 → 视作流结束,不喂 JSON parser;
- 无 output_index 的 function_call added → 无法组装,`warn!` 跳过;
- **arguments 截断容错**(review.md 修正 6):`output_item.done` 的 arguments 是
  不完整 JSON(max_output_tokens 烧断)时 `from_str` 失败 → 降级
  `input = Value::String(原始串)` + `warn!`,不丢调用不 panic——与
  incomplete → max_tokens 的 stop_reason 语义衔接,loop 端知道 JSON 为什么坏;
- 流 EOF 而无 terminal 事件(断网/代理剪断)→ 照 openai.rs 兜底发
  `Done { stop_reason: None, usage: None }`(usage None 跳过 SQL 写)。

### 3.5 stop_reason 合成

Responses 无 finish_reason 等价物。内部词表维持 Anthropic 风格(loop 只认这套,
同 openai.rs 归一化立场):

- `response.completed`:扫描最终 output items,任一 `function_call` →
  `"tool_use"`;否则 `"end_turn"`。
- `response.incomplete`:`incomplete_details.reason == "max_output_tokens"` →
  `"max_tokens"`;其余归 `"end_turn"`(保守)。

### 3.6 usage 映射(parse_responses_usage)

取 `response.usage`(网关拍平的顶层 `usage` 兜底):

| Responses | TokenUsage |
|---|---|
| `input_tokens` | `input_tokens`(**全长、已含 cached_tokens**,同 CC prompt_tokens 语义) |
| `input_tokens_details.cached_tokens` | `cache_read_input_tokens` |
| `output_tokens` | `output_tokens` |
| `output_tokens_details.reasoning_tokens` | **不映射**——它是 output_tokens 子集,加即重复计 |
| (无对应) | `cache_creation_input_tokens = 0` |
| (派生) | `context_input_tokens = input_tokens`(全长,再加 cache_read 即双计) |

全零/缺失 → `None`(agent loop 跳过 SQL 写;照 `parse_openai_usage` 惯例)。

### 3.7 探测(test_model,裸 2xx 判据——review.md 修正 4)

单源 `commands/providers.rs::test_model_inner` 的 `"openai_responses"` 分支;
daemon 路由(`daemon/routes/providers.rs`)与 doctor 工具
(`tools/test_llm_connection.rs`)都直接消费它,无第二实现。

- `POST {base}/responses`(非流式,**不发 `stream`**);headers = `Authorization:
  Bearer` + `content-type`。
- body:`{"model": <catalog model_name>, "input": "Reply with exactly: ok",
  "max_output_tokens": 16, "store": false}`。`max_output_tokens` 用 16 而非 1
  (reasoning token 计入该帽,1 会直接 400);**成功判据 = `status.is_success()`
  裸 2xx,不解析响应体**——与 anthropic/openai 分支同款。解析 output_text 对
  推理模型假阴:16 帽会被 reasoning 烧光导致无可见输出,doctor 跟着误诊
  (原设计「2xx 且 output_text 非空」因此作废)。
- 失败路径在 error 串尾部内嵌协议专属 hint(两个消费方原样透传):
  - 404 →「this endpoint does not implement /responses — confirm the gateway
    supports the OpenAI Responses API (official OpenAI, vLLM, LiteLLM, etc.)」
  - 400 且 body 含 effort 字样 →「the Responses API only accepts reasoning
    effort values minimal|low|medium|high …」(防御性:探测本身不发 reasoning
    对象,但网关可能在 400 body 里回显 effort)
  - 401/403 无新增——doctor 侧 `fix_hint` 的既有 auth 条目已覆盖。
- `test_llm_connection::fix_hint` 的 unsupported-protocol 条目词表同步为
  `anthropic`/`openai`/`openai_responses` 三值。

### 3.8 前端面(消费契约)

- `ProvidersTab.vue`:协议下拉第三项 `{value:"openai_responses", label:"OpenAI
  Responses"}`;badge 第三色 `providers-tab__badge--responses`
  (`--color-tool-thinking` violet + color-mix 底,区别 anthropic 蓝 / openai 绿)。
- `ModelForm.vue`:thinking_effort 选项按所选 provider 的 protocol 过滤
  (providers prop 内按 `form.providerId` 查)——`openai_responses` 只出
  `none`/`minimal`/`low`/`medium`/`high`;存量行带 `xhigh|max` 时该值临时并入
  选项并标注「将按 high 发送」(与 §3.3 归一口径一致)。其余协议选项列表不变。
- `stores/providers.ts`:`ProviderRow.protocol` 注释三值。

## 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| Anthropic 会话切 Responses 模型,历史含 Thinking+Signature | Signature/Redacted 被 strip 裁;Reasoning 幸存 strip(effort 已设时)但被 build 层跳过;请求不 400;DB 原样(strip 仅内存) |
| 同上但模型未设 effort | `supports_reasoning_effort=false` → Reasoning 也被 strip 裁(OR 语义两侧皆 false) |
| `thinking_effort = "xhigh"`(存量行) | wire 发 `reasoning.effort:"high"` + 一行 `warn!`;不 400 |
| `thinking_effort` 词表外(如 "ultra") | 不发 reasoning 对象 + `warn!`;请求照发 |
| arguments 被 max_output_tokens 截断 | ToolCall.input = `Value::String(原始串)` + warn;调用继续流转 |
| function_call done 无 name | 丢弃该调用 + warn(不 emit) |
| 重复 `response.created` | 第二次起忽略(Start 只发一次) |
| `response.failed` 携带 `error.code:"rate_limit_exceeded"` | `LlmError::RateLimit`(keyword 匹配,合成 500 不影响分类) |
| 流被剪断无 terminal 事件 | `Done { stop_reason: None, usage: None }`,turn 记账正常收口 |
| 探测:网关无 /responses 路由 | `success:false, error:"HTTP 404: …\nhint: …does not implement /responses…"` |
| 探测:错误 key | `success:false, error:"HTTP 401: …"`(doctor 走既有 auth hint) |
| 探测:推理模型把 16 帽烧成零可见输出 | HTTP 200 → **`success:true`**(裸 2xx,不解析 body) |

## 5. Good / Base / Bad Cases

**Good** — gpt-5.x 推理模型一轮带工具调用:instructions(system)+ input items
(全量回放)+ `reasoning:{effort:"high",summary:"auto"}` + 扁平 tools;
SSE 依序 reasoning_summary_text.delta(→ThinkingDelta)→ output_item.added →
function_call_arguments.delta×N → output_item.done(→ToolCall)→ completed
(output 含 function_call → stop_reason "tool_use",usage 落账)。

**Base** — vLLM 部署未开 Responses 支持:探测 POST /responses 404,GUI 测试行
红 ✗ + 404 body 前 200 字符 + 内嵌 /responses hint;用户据此判断是网关能力
问题而非 base_url 拼写(fix_hint 的 /v1 通用提示在其后仍会追加)。

**Bad** — 只信 strip 层裁 Reasoning(改 `block_supported` 或 caps 让 strip
丢弃 Reasoning):会把 CC 路径的 RULE-D-006 回放一起改坏——`block_supported`
是三协议共享函数,动它前先读 §3.2;Responses 侧的丢弃点在 build 层,
保持两层各自语义。

**Bad** — 把探测判据改回「解析 output_text 非空」:推理模型假阴复活
(review.md 修正 4),gpt-5 系探测永远失败。

## 6. Tests Required(`tests_responses.rs`,28 用例,合成样例驱动)

| 组 | Test | 锁定 |
|---|---|---|
| 形状 | `endpoint_appends_responses_only_no_double_v1` | base 只拼 /responses,无 /v1/v1 |
| 形状 | `responses_provider_reports_capabilities_and_protocol` / `…_is_send_sync` | capability 面 + Send/Sync |
| 形状 | `build_http_body_basic_shape` | instructions/input/max_output_tokens/store:false/stream:true/扁平 tools strict:false |
| 形状 | `build_http_body_no_system_omits_instructions` | system None → 字段省略 |
| 形状 | `build_http_body_speaker_prefix_inline_user_and_assistant` | `@name: ` 内联(User+Assistant) |
| 形状 | `build_http_body_assistant_text_then_function_call_items_order` | message item + function_call items,先 text 后 calls |
| 形状 | `build_http_body_tool_message_becomes_function_call_output` | call_id 关联 |
| 形状 | `build_http_body_image_block_becomes_input_image_part` | data URL + detail:auto |
| 形状 | `build_http_body_effort_vocabulary_normalization` | high 原样 / xhigh、max→high / 词表外无 reasoning 对象 |
| 形状 | `build_http_body_anthropic_history_blocks_absent_in_body_ac4` | **AC4 断言对象 = body**(§3.2) |
| 事件 | `event_stream_plain_text_end_turn_with_usage` | created→delta×N→completed,end_turn + usage |
| 事件 | `event_stream_tool_call_assembled_from_deltas` | 聚合 + parse,stop_reason tool_use |
| 事件 | `event_stream_parallel_tool_calls_interleaved_by_output_index` | output_index 键控,顺序保持 |
| 事件 | `event_stream_incomplete_max_tokens` | incomplete → max_tokens |
| 事件 | `event_stream_truncated_arguments_degrade_to_raw_string` | 截断容错 `Value::String` |
| 事件 | `event_stream_flush_prefers_delta_buffer_over_empty_done_item` | done flush 以 **envelope** `output_index` 查 buffer,先 buffer 后 item(done item 省略 arguments 时 delta 累积串是唯一数据源) |
| 事件 | `event_stream_refusal_part_becomes_delta` | refusal → Delta,不产空轮 |
| 事件 | `event_stream_failed_yields_llm_error` | failed → LlmError |
| 事件 | `event_stream_unknown_events_ignored_no_panic` | 无关心事件穿透 |
| 事件 | `event_stream_duplicate_created_emits_start_once` | Start 恰一次 |
| usage | `parse_responses_usage_full_payload` / `…_missing_details_and_top_level_fallback` / `…_zero_or_missing_returns_none` | 三态 + 拍平兜底 |
| caps | `responses_caps_derive_from_config` / `normalize_effort_empty_string_is_silent_none` | caps 派生 + 空串静默 |
| 工厂 | `build_provider_openai_responses_returns_responses_provider` / `provider_protocol_openai_responses_three_spellings_agree` | 分发 + 枚举三处拼写一致 |

前端:`ProvidersTab.test.ts` 两条(badge 类、下拉三项);`cd app && pnpm test`
全绿。探测 wire 路径无自动化测试(spec test-model-contract §6:manual smoke
是契约),`test_llm_connection` 单测覆盖 HTTP-free 臂。

## 7. Wrong vs Correct

**Wrong — 把 Reasoning 丢弃逻辑塞进 strip(或改 `block_supported` 的 OR 语义)**:
`block_supported` 是三协议共享的裁剪判据,为 Responses 单协议改它等于改 CC /
Anthropic 的历史语义(见 §3.2 Bad case)。Responses 的丢弃点在
`assistant_blocks_to_responses_items` 的 skip 臂。

**Wrong — 探测解析响应体 / max_output_tokens 设 1**:推理模型下前者假阴、
后者 400。判据 = 裸 2xx,帽 = 16。

**Wrong — store 缺省不发**:Responses 服务端默认 `store:true`(存 30 天),
不显式 `store:false` 会让 stateless 契约静默失效、第三方网关行为分叉。
`store:false` 是 load-bearing 字段,恒发。

**Wrong — 复用 `OpenAIConfig`**:两协议字段今天同构,明天分叉(Responses 会长
`include`/summary/store 旋钮);独立 `ResponsesConfig` 是锁定决策(design §3)。

**Wrong — 消费 `function_call_arguments.done` 做第二 flush 点**:
`output_item.done` 是唯一 flush,双 flush 重复 ToolCall;`.done` 事件显式忽略。
