# Design: openai_responses provider 适配器(stateless 全量回放)

> 输入:prd.md(R1-R7 / AC1-AC5)+ research.md(协议差异明细 §2、设计决策 §4)。
> 本文把调研结论落成函数级实现设计;协议字段语义与生态依据不在此重复,以 research.md 为准。

## 0. 决策速览

新建 `llm/provider/responses.rs` 适配器(与 `openai.rs` 平级、同构),实现 `Provider` trait;请求走既有 `chat_request_to_wire → strip_unsupported(responses caps) → build_http_body` 纯函数管线;响应走 `SseParser` + 按 `event.type` 分支的状态机,stop_reason 自行合成;**stateless**(`store:false` + 全量 input 回放,无 `previous_response_id`)。

## 1. 架构与数据流

```
agent loop(全量 ChatRequest)
  → chat_request_to_wire(req, system)            [wire/mod.rs,原样复用]
  → strip_unsupported(wire.messages, &caps)      [caps 见 §2.2,Responses 档]
  → build_http_body(wire, config)                [responses.rs 纯函数,新写]
  → POST {base_url}/responses  (stream:true)
  → SseParser                                   [llm/sse.rs,event 行模式,原样复用]
  → 事件状态机(responses.rs stream! 循环,新写)
  → ChatEvent 流(Delta/ThinkingDelta/ToolCall/Done{stop_reason,usage})
```

与 openai.rs(944 行)的差异集中在两端:请求体形状(§2.3)与响应事件机(§2.4/§2.5);HTTP client 构造、错误分类(`classify_error_response`)、cache-miss 哨兵(`warn_on_full_prefix_cache_miss`,Done 前调用)、read/connect timeout(RULE-A-011)全部平移。

**base_url 约定**:沿用 openai 条目「base_url 含 `/v1`」语义,adapter 只追加 `/responses`(research §1)。绝不重蹈 `/v1/v1` 双拼(06-09 修过的 bug)。

## 2. 组件设计

### 2.1 协议枚举与工厂(PR1)

- `db/types.rs:35` `ProviderProtocol` 加 `OpenaiResponses`;`as_str() -> "openai_responses"`;`from_str_opt` 加同名分支(与 serde 行为对齐,参照现有两分支)。DB `providers.protocol` 是 TEXT,零迁移。注意:枚举只覆盖 serde/测试面,真实分发走 `build_provider` 的字符串 match——回滚到旧二进制后存量 `openai_responses` 行走 `UnknownProtocol` 干净报错,但 `from_str_opt` 未知值兜底 Anthropic 会让统计/展示面在回滚期显示错协议(可接受,记录在此)。
- `llm/provider/mod.rs:193` `build_provider` 加 `"openai_responses"` 分支:构造 `responses::ResponsesConfig`,兜底逻辑与 openai 分支对称(`max_tokens` 缺省 `anthropic::DEFAULT_MAX_TOKENS`,`thinking_effort` 直传 Option 不设默认)。
- `llm/provider/mod.rs` 模块声明区(449 行附近,`pub mod` 无 cfg 门)加 `pub mod responses;` 与 `pub mod tests_responses;`(照 tests_openai 声明样式)。

### 2.2 ResponsesConfig 与 WireCapabilities 档(PR1)

`ResponsesConfig` **独立定义**,不复用 `OpenAIConfig`——字段今天同构(base_url/model/api_key/max_tokens/reasoning_effort/supports_images),但两协议独立演进(Responses 未来会加 `include`/`summary` 等 store 相关参数),耦合 struct 会互相拖累。

`WireCapabilities`(wire/types.rs:29)Responses 档取值(2026-09-18 群聊评审修正:strip 行为表述):

| 字段 | 值 | 理由 |
|---|---|---|
| `supports_thinking` | `false` | wire 载荷不携带 thinking 块——MVP 不回传 reasoning item(research §4.2) |
| `supports_reasoning_effort` | `model.thinking_effort.is_some()` | 决定是否发 `reasoning` 对象 |
| `supports_thinking_signatures` | `false` | signature/redacted blob 仅 Anthropic 能往返 |
| `supports_images` | `model_row.supports_images` | B1 语义原样,strip 换文本占位 |

**Reasoning 块的实际裁剪路径(评审锚验,与 OR 语义对齐)**:`block_supported`(wire/to_wire.rs:652)对 `Reasoning` 是 `supports_thinking \|\| supports_reasoning_effort`——effort 已设(推理模型主场景)时 strip **保留** Reasoning 块,真正丢弃它们的是 build 层的防御性跳过(§2.3 表末行);`Signature`/`RedactedThinking` 才由 `supports_thinking_signatures:false` 经 strip 裁掉。AC4 的保障 = build 层跳过 + strip 裁 signature,两者合力,不能只靠 strip。**与 CC 路径的不对称留痕**:同为幸存的 Reasoning 块,CC 是回放(RULE-D-006,提升为顶层 `reasoning_content`,openai.rs:497-503),Responses 是丢弃——P3 做 encrypted reasoning 回传时须先消化这个差异。

caps 在 adapter `send` 内从 config 派生(与 anthropic.rs:507-520 同构),不新增工厂参数;与 `WireCapabilities::from_model_row`(dead_code 测试路径)的分叉是既有形态,不是本任务引入。thinking**接收**方向不受 caps 影响:摘要事件照样映射 `ThinkingDelta`(caps 只管出站载荷)。

### 2.3 请求构造:`build_http_body(wire: &WireRequest, config: &ResponsesConfig) -> Value`(纯函数,PR1)

顶层字段:`model` / `instructions`(= `wire.system`,None 则省略)/ `input`(item 数组,见下)/ `max_output_tokens` / `tools`(扁平,`strict: false` 显式)/ `store: false` / `stream: true`;`thinking_effort` 存在时加 `reasoning: { effort, summary: "auto" }`(summary:auto 才会下发 `reasoning_summary_text` 事件流)。

**effort 词表归一(adapter 侧防御)**:Responses 官方只认 `minimal|low|medium|high`;DB 存量与 API 建模可能带 Anthropic/DeepSeek 词表的 `xhigh|max`。归一规则:`xhigh|max → high` 并 `warn!` 一行(不静默);不在词表内的其他值按 None 处理(不发 `reasoning` 对象)并 `warn!`。表单按协议过滤是第一道防线(PR2),adapter 归一是兜底——两道都上,老行不 400。

**input item 生成规则**(wire → item,保持消息相对顺序):

| wire 形态 | Responses item |
|---|---|
| `User { speaker, text }` | `{role:"user", content:"@<speaker>: <text>"}`;speaker=None 原文(2026-09-18 评审:前缀格式对齐 Anthropic 现行 `apply_speaker_prefix` 的 `@name: `——anthropic.rs:371,继承群聊「恰好一次、无双重前缀」既有约定,避免混编群聊两种格式漂移) |
| `UserBlocks` | `{role:"user", content:[parts]}`,Text→`input_text`、Image→`{type:"input_image", image_url:"data:...", detail:"auto"}` |
| `Assistant { speaker, blocks }` 中的 Text 块 | `{role:"assistant", content:"@<speaker>: <text>" 或 parts(output_text)}` |
| `Assistant` 中的 `ToolUse` 块 | **独立** `{type:"function_call", call_id, name, arguments}` item,排在该 assistant 消息 item 之后(保序:先 text 后 function_call) |
| `Tool { tool_call_id, content }` | `{type:"function_call_output", call_id, output}` |
| `Reasoning`/`Signature`/`RedactedThinking` 块 | strip 阶段已裁(caps),build 层防御性跳过 |

tools 扁平化:`{type:"function", name, description, parameters, strict:false}`——**不**沿用 CC 的 `{function:{...}}` 嵌套。

### 2.4 流式事件机(stream! 循环,PR1)

`SseParser` 产出 `(event, data)`;data JSON 统一按 `r#type` 字段 match。需处理的事件与映射(research §2.4 全表):

- `response.created` → `ChatEvent::Start`(一次)
- `response.output_text.delta` → `Delta { text }`
- `response.reasoning_summary_text.delta` → `ThinkingDelta { text }`
- `response.output_item.added` 且 `item.type=="function_call"` → 开聚合 buffer
- `response.function_call_arguments.delta` → 追加 arguments 字符串
- `response.output_item.done` 且 function_call → flush `ToolCall { id: call_id, name, input: serde from_str(arguments) }`;**arguments 被 max_output_tokens 截断导致 JSON 不完整时**(评审补):`from_str` 失败降级为 `input = Value::String(原始字符串)` + `warn!`,不 panic 不丢调用(与 incomplete → max_tokens 的 stop_reason 语义衔接)
- `response.completed` → `Done { stop_reason: 合成(§2.5), usage: parse_responses_usage }`
- `response.incomplete` → `Done { stop_reason: 按 incomplete_details.reason 映射(max_output_tokens→"max_tokens"), usage }`
- `response.failed` / 顶层 `error` → `LlmError`(经 `classify_error_response` 分类)
- **refusal 路径**(评审补):`message` item 的 content part 可为 `{type:"refusal", refusal:"..."}`——模型拒绝时若无处理,会「零 Delta → completed → 合成 end_turn」产出**空文本 assistant 消息**(聊天 UI 空气泡、群聊空发言)。处理:`output_item.done`(message)时检查 content parts,遇 refusal part 把 `refusal` 文本作为一条 `Delta` 发出让用户可见,再走正常 completed
- 其余几十种 → 忽略(parser 已容忍未知事件)

**工具聚合键控**:CC 按 `index`,Responses 按 `output_index`(item 在 output 数组的位置)。在 streaming.rs 平移 `ToolCallBuf` 模式新建 `ResponsesFunctionCallBuf { call_id, name, arguments: String }`,以 `output_index` 为键存 map;`output_item.done` 是唯一 flush 点(arguments `.done` 事件冗余,不做双 flush)。与 `ToolCallBuf` 的关系:字段/键制不同,放 responses.rs 内部私有(不进 streaming.rs 公共面,避免为单一消费方扩 API)。

### 2.5 stop_reason 合成(PR1)

Responses 无 finish_reason 等价物。`response.completed` 到达时:

- output items 含任一 `function_call` → `"tool_use"`
- 否则 → `"end_turn"`

`response.incomplete` → `incomplete_details.reason == "max_output_tokens"` 时 `"max_tokens"`,其余归 `"end_turn"`(保守)。内部 stop_reason 维持 Anthropic 风格词表(与 openai.rs 现行归一化一致,agent loop 只认这套)。

### 2.6 usage:`parse_responses_usage(v: &Value) -> Option<TokenUsage>`(PR1)

`response.completed.usage` → `{input_tokens, input_tokens_details.cached_tokens, output_tokens}` 映射 `TokenUsage {input_tokens, cache_read_input_tokens, output_tokens}`;零值/缺失返 None(照 `parse_openai_usage` 惯例,streaming.rs:122)。`reasoning_tokens` 不单列——它是 output_tokens 子集,不重复计(research §2.5)。放 streaming.rs 与 `parse_openai_usage` 并排(`pub(crate)`,供测试与 adapter 共用)。

### 2.7 探测分支(PR2)

- `commands/providers.rs` `test_model`(~458 行 protocol match)与 `tools/test_llm_connection.rs` 各加 `"openai_responses"` 分支:非流式 `POST {base}/responses`(`max_output_tokens: 16`,`store:false`)。**成功判据 = 裸 2xx**(2026-09-18 评审修正:与现有两分支同款——anthropic/openai 探测均不解析输出,providers.rs 两处 `is_success() → (true, None)`;原设计的「且 output_text 非空」对推理模型假阴——reasoning token 计入 max_output_tokens,16 会被烧光导致 output 无文本而误报失败,doctor 流程跟着误诊)。
- 错误 hint 文案区分:404 → 「该端点未实现 /responses,确认网关支持 OpenAI Responses API(官方、vLLM、LiteLLM 等)」;401/403 → key;400 且 message 含 effort 词 → 提示词表受限。

### 2.8 前端(PR2)

- `ProvidersTab.vue` 协议下拉(316-330)加第三项 `{ value: "openai_responses", label: "OpenAI Responses" }`;`protocolBadgeClass`(202)加分支(第三色)。
- `ModelForm.vue` effort 选项(198-199)按所属 provider 的 protocol 过滤:Responses 只出 `minimal|low|medium|high`。ModelForm 若拿不到 protocol,经 props 从 ProvidersTab 传入或从 providers store 按 provider_id 查;编辑存量模型带 `xhigh|max` 时显示但标「将按 high 发送」(与 adapter 归一口径一致,见 §2.3)。
- `stores/providers.ts` protocol 注释补第三值。

## 3. 已锁定决策(不重开,出处 research §4)

| 决策 | 一句话 | 否决的替代 |
|---|---|---|
| stateless 全量回放 | `store:false`、无 previous_response_id,匹配 agent loop 每轮全量 | stateful(与 catalog/session 语义冲突、网关兼容面差、输入照计费) |
| 独立第三协议 | 枚举分支而非 base_url hack | 端点后缀识别(污染 protocol 字段语义,探测/统计/strip 全乱) |
| MVP 不回传 reasoning item | 不加 encrypted_content 载荷,拆 P3 follow-up | include 回传(Thinking 块加 blob 要过 DB/IPC/wire 全链,收益推迟) |
| speaker 内联 | `@Alice: ...` 进 content 文本(与 Anthropic 现行方案同款,非降级) | ——(EasyInputMessage 无 name 字段,无替代) |
| strict:false 显式 | 行为确定,不赌上游回退 | 省略(OpenAI 会尝试 strict 再回退,项目 schema 不满足 strict 前提) |

## 4. 测试设计(tests_responses.rs,PR1;参照 tests_openai.rs 样例驱动风格)

1. **build_http_body 形状组**:
   - 基本:instructions/input/max_output_tokens/store:false/stream:true/tools 扁平含 strict:false;
   - speaker 内联(User 与 Assistant 两种);
   - Assistant 含 [Text, ToolUse×2] → assistant message item + 两个 function_call item,顺序 text→call;
   - Tool → function_call_output(call_id 关联);
   - 图片块 → input_image part;**AC4 断言对象 = `build_http_body` 的输出 body**(评审修正:不能断言 post-strip wire——strip 的 `block_supported` 对 Reasoning 是 OR 语义,effort 已设用例的 Reasoning 块会幸存到 wire,真正的丢弃在 build 层;断言落在 body 上同时覆盖两层);
   - effort 映射:`high` 原样、`xhigh`/`max` → `high`、词表外 → 无 reasoning 对象。
2. **事件机组**(喂合成 SSE 行序列,断言 ChatEvent 流):
   - 纯文本:created → text.delta×3 → completed(含 usage 断言,stop_reason="end_turn");
   - 工具调用:output_item.added → arguments.delta×2 → output_item.done → completed,断言 ToolCall 的 id=call_id、input 已 parse,stop_reason="tool_use";
   - 并行双调用(两个 output_index 交错 delta)→ 两个 ToolCall,顺序按 output_index;
   - incomplete(max_output_tokens)→ stop_reason="max_tokens";
   - **arguments 截断容错**(评审补):output_item.done 的 arguments 是不完整 JSON(模拟 max_output_tokens 截断)→ ToolCall 的 input 降级为 `Value::String(原始串)`,不 panic;
   - **refusal 序列**(评审补):message content part type=refusal → 对应文本作为 Delta 发出,completed 正常收尾,不产出零文本 Done;
   - failed → LlmError;
   - 未上心事件(web_search 系等)穿插 → 无 panic、无事件。
3. **usage 组**:`parse_responses_usage` 全载/缺 details/零值三态。

## 5. 风险与回滚

- 新代码全部落在 `responses.rs` + `tests_responses.rs` + 三处既有文件的枚举/分支追加(工厂、两探测、前端),不改任何现有协议路径;回滚 = revert 单 commit,无 schema/DB 变更。
- 最大不确定面是 SSE 事件真实序列与官方文档的偏差(尤其 function_call 的 arguments.done 与 output_item.done 的到达次序)——PR1 用合成序列锁形状,PR1 末尾跑 `scripts/turn-smoke.sh` live 校准一次,发现偏差先改状态机不改契约。
- 回归面:既有 `tests_openai`/`tests_wire`/`tests_anthropic` 不受触碰,`cargo test -p everlasting --lib` 全量守门(AC5)。
