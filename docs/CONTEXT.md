# CONTEXT.md

> Everlasting 项目术语表(glossary)。
> 本文件是 **glossary,只定义术语**;实现决策(schema / 写入时机 / 颜色阈值等)走 `docs/IMPLEMENTATION.md §4` 决策日志,本文件不重复。

---

## 术语表

### Turn (LLM turn)
一次 LLM HTTP 请求(Anthropic Messages API / OpenAI Chat Completions 一次 stream)。
一个用户消息可能引发 N 次 turn(主调 + tool_use 回填),受 agent loop `MAX_TURNS`(50)限制。

### TokenUsage
LLM 一次响应的 token 使用四元组(Anthropic schema 视角):

- **`input_tokens`** — 当次请求中送入的 token 数,**已包含** `cache_creation_input_tokens` + `cache_read_input_tokens`(Anthropic 语义)
- **`output_tokens`** — 当次响应生成的 token 数
- **`cache_creation_input_tokens`** — 当次请求中**新创建**的 cache token(下次可命中)
- **`cache_read_input_tokens`** — 当次请求中**命中**的 cache token

OpenAI Chat Completions 的归一化映射(在 Provider 层完成,ChatEvent 出来时已统一):

- `prompt_tokens` → `input_tokens`
- `completion_tokens` → `output_tokens`
- `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
- `cache_creation_input_tokens` → `0`(OpenAI 暂无对应字段)

### Context Pressure (上下文压力)
**当前 context 窗口的占用比例**。定义为:

- 分子 = session 累计 `input_tokens`(sum over turns)
- 分母 = `ModelRow.context_window`(默认 200K)

`input_tokens` 已包含 cache_creation + cache_read,所以 cache 命中**不重复计**——使用 cache 会让压力增长更慢。`output_tokens` **不计入** context 压力(那是响应,不是 context)。

### Cache Hit (cache 命中)
LLM 一次请求中,从 prompt cache 读回的 token(`cache_read_input_tokens`)。计费按 Anthropic / OpenAI 各自规定(Anthropic `cache_read_input_tokens` 按 0.1x input 价;OpenAI `cached_tokens` 按 0.5x input 价)。

### Context Window (上下文窗口)
LLM 模型能处理的最大 input token 数(Anthropic Sonnet / Opus 默认 200K)。数据来源:`ModelRow.context_window` 列,seed 时硬编码。

### Per-session 累积 (Token 统计颗粒度)
Token 统计在 DB 层的存储颗粒度为 session 维度:`sessions` 表的 4 列(input_tokens_total / output_tokens_total / cache_creation_total / cache_read_total)。每次 LLM turn Done 时单条 SQL UPDATE 累加。

### Anthropic SSE Usage
Anthropic Messages API 的 token 用量在 SSE 流的 `message_delta` 事件中携带(`usage: { input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }`),累计语义,本 turn 累计。

### OpenAI Stream Usage
OpenAI Chat Completions 的 token 用量在流末尾携带(`usage: { prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details: { cached_tokens } }`),**仅在请求体发送 `stream_options: { include_usage: true }` 时**返回。

### Checklist (agent 自跟踪清单)

LLM 在跑复杂多步任务时维护的**结构化进度清单**——agent 自己写、改、标记完成,用于不丢失自己的计划与进度。对齐 Claude Code 的 `TaskCreate/TaskList`、opencode 的 `todowrite`、Cline 的 plan-act。

**不是什么**(本项目内这几个词都已占用,需消歧义):
- **不是** Trellis task(`.trellis/tasks/`,dev-workflow 的 PRD / 排期任务)
- **不是** plan mode(`Mode::Plan`,权限模式,拒 tool_use)
- **不是** subagent(B6,main agent 派 worker agent,独立 context + summary 回填)

典型形态:agent 在一个任务的多 turn run 中反复更新它,每轮把当前清单重新注入 context,从而"看到自己还剩什么没做"。

---

## 相关决策

- 设计决策走 [`docs/IMPLEMENTATION.md §4 决策日志`](../IMPLEMENTATION.md#4-决策日志)(本文件不重复)
- A4 Token 相关术语已落地、作为历史术语定义保留;Checklist(agent 自跟踪清单)为规划中术语,实现决策待定
- 跨层契约走 `.trellis/spec/backend/llm-contract.md` "Scenario: Token Usage Tracking" 段
