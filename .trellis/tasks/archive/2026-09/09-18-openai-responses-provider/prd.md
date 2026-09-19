# OpenAI Responses API provider 接入(调研立项)

## Goal

新增第三种 provider protocol `openai_responses`(与现有 `anthropic` / `openai` 同级),让 agent loop 能对 OpenAI Responses API(`POST /v1/responses`)做流式工具调用对话——覆盖 OpenAI 官方推理系模型(gpt-5.x / o 系)与明确支持 Responses 的网关(vLLM、LiteLLM 路由等)。**2026-09-18 已完成调研,本任务当前处于 planning,调研全文见 [research.md](./research.md),尚未开工。**

## Background

- 调研结论(2026-09-18,详 research.md):架构上完全可行且成本可控。`Provider` trait + wire 中间层 + protocol 字符串分发就是为加新协议设计的;现有 `openai.rs`(Chat Completions)适配器的 HTTP/SSE/错误分类/usage 骨架可大量平移。
- 最关键架构对齐点:本项目 agent loop 每轮全量回放历史,天然对应 Responses 的 **stateless 用法**(`store:false` + 全量 input 回放),不需要引入 `previous_response_id` 服务端会话状态。
- 动机:OpenAI 已把 Responses 定为主推路径(Assistants API 2026-08 下线);推理模型 + 工具调用在 Responses 上体验更好,部分新模型在 Chat Completions 上功能受限。

## Requirements

- **R1 协议枚举与分发**:`ProviderProtocol` 加 `OpenaiResponses`(`as_str = "openai_responses"`);DB `providers.protocol` 是 TEXT 列,零 schema 迁移;`build_provider` 工厂加分支。
- **R2 适配器(stateless)**:新建 `llm/provider/responses.rs`,请求形状:顶层 `instructions`(system)、`input`(item 数组,全量回放)、`max_output_tokens`、`reasoning: {effort, summary}`(映射 `ModelRow.thinking_effort`)、`store: false`、function tool 扁平结构且显式 `strict: false`。不引入 `previous_response_id`。
- **R3 wire 复用**:走既有 `chat_request_to_wire → strip_unsupported` 管线,为 Responses 定义自己的 `WireCapabilities`;Anthropic 会话切到 Responses 模型时 thinking/signature/redacted 块按 caps 静默裁剪(不 400)。
- **R4 流式事件**:复用 `SseParser`(event 行模式);处理 `response.output_item.added/done`、`response.output_text.delta/done`、`response.function_call_arguments.delta/done`、`response.reasoning_summary_text.delta`、`response.completed/incomplete/failed`;stop_reason 自行合成(含 function_call → `tool_use`,`response.incomplete` → `max_tokens`);工具调用按 `call_id` 关联、`function_call_output` 回传;`refusal` content part 转可见 Delta(不让模型拒绝产出零文本空消息,2026-09-18 评审补)。
- **R5 usage**:`response.completed` 的 `usage`(input_tokens / cached_tokens / reasoning_tokens)映射内部 `TokenUsage`,接入 full-prefix cache-miss 哨兵。
- **R6 探测与前端**:`test_model`(commands/providers.rs)与 `tools/test_llm_connection.rs` 加 `/responses` 分支(成功判据与现有两协议同款裸 2xx、不解析输出——解析 output_text 对推理模型假阴,2026-09-18 评审修正);Settings ProvidersTab 协议下拉第三项 + badge;thinking_effort 选项按协议过滤(Responses 只认 `minimal|low|medium|high`,`xhigh/max` 会 400)。
- **R7 spec 与测试**:`multi-provider-contract` 新增 scenario-responses-wire;新建 `tests_responses.rs`(参照 `tests_openai.rs` wire 样例风格)。

## Technical Notes(调研锁定的关键决策)

- **stateless 全量回放**(不 store、不 previous_response_id):匹配现有 agent loop,不引入跨轮状态;第三方网关对 stateful 支持参差。
- **MVP 不回传 reasoning item**:stateless 下推理模型跨轮保真需 `include: ["reasoning.encrypted_content"]` + 回传加密 blob,`ContentBlock::Thinking` 目前无该载荷——拆为后续增量(见 Out of Scope),官方允许不回传(行为略退化,不报错)。
- **群聊 speaker 归属(与 Anthropic 同款内联,非降级)**:Responses 的 input message 无 Chat Completions 的 `name` 字段,`WireMessage::{User,Assistant}` 的 `speaker` 以 `@name: ` 文本前缀内联进 content——与 Anthropic 现行 `apply_speaker_prefix`(anthropic.rs:371)同格式,继承群聊「恰好一次、无双重前缀」既有约定(2026-09-18 评审修正,原「降级」措辞与 `Alice: ` 格式作废)。
- 错误体 `{error:{code,message}}` 与 `classify_error_response` 现有 code 提取路径一致,直接复用;`read_timeout` SSE 客户端参数照搬 RULE-A-011。

## Acceptance Criteria

- [ ] **AC1 建单跑通**:Settings 添加 `openai_responses` provider + 模型,`scripts/turn-smoke.sh` 实跑一轮流式文本,token 核算落在 turn_trace。
- [ ] **AC2 工具调用回路**:agent loop 完整走一轮 function_call → 本地执行 → function_call_output 回传 → 收尾回答(含并行多调用)。
- [ ] **AC3 探测**:`test_model` 对 Responses provider 探测成功(判据 = 裸 2xx,与现有两协议一致);错误路径 hint 文案正确。
- [ ] **AC4 跨协议切换**:Anthropic 会话中途切到 Responses 模型,thinking/signature 块按 caps strip,请求不 400。
- [ ] **AC5 回归**:cargo `--lib` + pnpm test 全绿;multi-provider-contract spec 落账。

## Out of Scope

- encrypted reasoning 跨轮回传(`include: reasoning.encrypted_content` + Thinking block 加密载荷)——单独 follow-up 任务。
- `store:true` / `previous_response_id` / Conversations API(与全量回放架构冲突,收益不明)。
- Responses 内置工具(web_search / file_search / code_interpreter / computer use / MCP)——仅 OpenAI 官方原生,本项目只用自定义 function tool。

## 分期建议(research.md §6)

- **PR1**:枚举 + 工厂 + 适配器主体(stateless、无 reasoning 回传、strict:false)+ wire 样例单测。
- **PR2**:前端下拉/徽标/effort 过滤 + 探测分支 + spec 文档。
- **PR3(可选)**:encrypted reasoning 回传。
