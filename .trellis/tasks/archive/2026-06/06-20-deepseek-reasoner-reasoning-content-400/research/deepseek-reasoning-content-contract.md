# Research: DeepSeek API reasoning_content Multi-Turn Contract

- **Query**: DeepSeek reasoner reasoning_content 回传 400 修复所需的协议契约（`reasoning_content` 字段位置 / 是否必须回传 / reasoning_effort 等）
- **Scope**: external（DeepSeek 官方 api-docs.deepseek.com + GitHub/n8n 社区佐证）
- **Date**: 2026-06-20

> **TL;DR**: 当前任务里遇到的 400 `"The reasoning_content in the thinking mode must be passed back to the API."` 来自 DeepSeek **V4 thinking mode**（不是旧的 R1 协议）。V4 协议是**双路径契约**：
> - **无 tool_call 的中间轮** → `reasoning_content` 可省略，传回也会被忽略；
> - **有 tool_call 的中间轮** → assistant 消息**必须**把 `reasoning_content` 作为**独立顶层字段**（与 `content` 同级）原样回传，所有后续轮都要带；
> - 字段**不能**塞进 `content` 里做注释（当前 bug 的根因）。

---

## Findings

### 0. 关键前提：当前 DeepSeek API = V4，旧 `deepseek-chat` / `deepseek-reasoner` 是别名

DeepSeek 官方在 [pricing 页](https://api-docs.deepseek.com/quick_start/pricing) 明确：

> The model names **`deepseek-chat` and `deepseek-reasoner` will be deprecated on 2026/07/24 15:59 UTC**. For compatibility, they correspond to the **non-thinking mode** and **thinking mode** of `deepseek-v4-flash`, respectively.

即：
- 当前 API 创建 chat completion 的 `model` 只接受 `deepseek-v4-flash` / `deepseek-v4-pro`（见 [API Reference](https://api-docs.deepseek.com/api/create-chat-completion)：`model … Possible values: [deepseek-v4-flash, deepseek-v4-pro]`）。
- 旧名 `deepseek-chat` = `deepseek-v4-flash` 非思考模式；旧名 `deepseek-reasoner` = `deepseek-v4-flash` 思考模式（**即 V4 thinking mode 协议**，不是 2025 年的 R1 协议）。
- 所以 PRD 里"DeepSeek-R1 协议"的措辞**已过时**；用户配 `deepseek-reasoner` 真正命中的就是 V4 thinking mode。这解释了为什么错误文案是 V4 的 `"must be passed back to the API"`。

### 1. Round-trip 契约（核心，双路径）

来自官方 [Thinking Mode Guide](https://api-docs.deepseek.com/guides/thinking_mode) 「Input and Output Parameters」原话：

> In thinking mode, the chain-of-thought content is returned via the `reasoning_content` parameter, **at the same level as `content`**. When concatenating subsequent turns, you can **selectively** return `reasoning_content` to the API:
>
> - **Between two `user` messages, if the model did not perform a tool call**, the intermediate `assistant`'s `reasoning_content` **does not need** to participate in the context concatenation. **If passed to the API in subsequent turns, it will be ignored.** See Multi-turn Conversation for details.
> - **Between two `user` messages, if the model performed a tool call**, the intermediate `assistant`'s `reasoning_content` **must** participate in the context concatenation and **must be passed back to the API in all subsequent user interaction turns.** See Tool Calls for details.

回答任务里的问题 1：
- **不是所有历史轮都必须回传**。只在"两次 `user` 消息之间发生过 tool_call"时，那段路径上**每一个** assistant（含触发 tool_call 的那一轮）的 `reasoning_content` 必须原样回传，所有后续轮都要带。
- 纯聊天（无 tool_call）的中间轮 reasoning 可省，传回也会被 API 忽略。
- 官方 [Multi-round Conversation Sample](https://api-docs.deepseek.com/guides/thinking_mode) 注释明确写：`# The reasoning_content will be ignored by the API`，但仍然把它放在 `messages` 里一并发出（非强制，但允许）。

实际修复建议（实施侧）：
- **简单稳健策略**：对所有 thinking-mode assistant 消息，**无脑**把 `reasoning_content` 作为独立字段回传。无 tool_call 的也会被忽略，不会 400；有 tool_call 的刚好满足强制契约。这与社区 opencode PR #24150 的修法一致（"ALL assistant messages unconditionally get reasoning_content injected"）。
- **不推荐**做"是否最后一轮 / 是否有 tool_call"启发式 strip，容易漏边界（参考 opencode issue 里 fkyah3 的反复 patch）。

### 2. 字段位置：assistant 消息顶层，与 `content` 同级

[API Reference: Create Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion) 的 Assistant message schema：

```
content           string nullable   required — The contents of the assistant message.
role              string  required  Possible values: [assistant]
name              string  optional
prefix            bool    (Beta)
reasoning_content string  nullable (Beta) — Used for the thinking mode …
```

`reasoning_content` 直接挂在 assistant 消息对象上，和 `content` 是兄弟键。**不是** `content` 的子结构，**不是** `tool_calls[].function` 的子字段。

官方 [Streaming Sample](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_streaming) 给的回写形态（逐字）：

```python
messages.append({
    "role": "assistant",
    "reasoning_content": reasoning_content,   # 兄弟键，独立字段
    "content": content
})
```

官方 [Tool Call Sample](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_tool_call) 更简单：直接 `messages.append(response.choices[0].message)`（SDK 返回的 message 对象自带 `reasoning_content`，原样回传）。

### 3. content 与 reasoning_content 必须分开（当前 bug 确认错误）

- 官方 [Tool Call 示例输出](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_tool_call_output) 显示：tool_call 路径下 `content` 常常是**空串** `''`（"Turn 1.2 content='' tool_calls=[...]"），思考全在 `reasoning_content` 里，回答全在下一轮。如果按当前 bug 把 reasoning 拼进 content，会污染 content、破坏 tool_call 触发逻辑。
- 当前实现（`provider/openai.rs:301-341`）`text_parts.push(format!("[reasoning] {}", text))` 把思考塞进 content 是**错的**：DeepSeek API 在思考模式下会校验 assistant 消息结构，找不到顶层 `reasoning_content` 字段 → 400 `"must be passed back"`。
- 修复必须：保留 `content` 原值，把 reasoning 文本作为独立 `reasoning_content` 字段输出。

### 4. `reasoning_effort` 字段：接受，但取值不是 OpenAI 那套

[API Reference](https://api-docs.deepseek.com/api/create-chat-completion) + [Thinking Mode Guide](https://api-docs.deepseek.com/guides/thinking_mode)：

| 字段 | 取值 | 行为 |
|---|---|---|
| `reasoning_effort` | `high` / `max` | 顶层请求字段。`low`/`medium` → 映射为 `high`；`xhigh` → `max`（兼容映射） |
| `thinking` | `{"type":"enabled"}` / `{"type":"disabled"}` | 顶层对象，**默认 `enabled`**（即默认进思考模式） |

注意点：
- DeepSeek **接受** `reasoning_effort` 字段，不会因为传了它而 400；但取值语义不同于 OpenAI o1/o3（OpenAI 是 `low`/`medium`/`high`，DeepSeek 真实只有 `high`/`max`）。
- DeepSeek **不依赖** `reasoning_effort` 来开启思考；思考由 `thinking.type` 决定，**默认就是 enabled**。所以即使完全不传 `reasoning_effort`，V4 模型照样吐 `reasoning_content`。
- 任务问题 4 的答案：发 `reasoning_effort:"high"` 给 deepseek-reasoner → **不会 400**，等价于 `high`（实际就是默认值）。
- OpenAI SDK 用户需通过 `extra_body` 传 `thinking`（官方原话）：

  ```python
  response = client.chat.completions.create(
      model="deepseek-v4-pro",
      messages=messages,
      reasoning_effort="high",
      extra_body={"thinking": {"type": "enabled"}},
  )
  ```

### 5. deepseek-chat vs deepseek-reasoner（V4 视角）

- `deepseek-chat` = `deepseek-v4-flash` **非思考模式**（等价于 `thinking.type=disabled`）：**不吐** `reasoning_content`，**不需要**回传契约。
- `deepseek-reasoner` = `deepseek-v4-flash` **思考模式**（`thinking.type=enabled`）：吐 `reasoning_content`，遵循本契约。
- 切换由 `thinking` 参数控制，**与模型 id 正交**（同一个 `deepseek-v4-flash` 可开可关）。所以"用户把 deepseek-reasoner 配成 `supports_thinking=true`"的修复方向是对的。

### 6. Token 计费 / Context Caching 关联

[Context Caching Guide](https://api-docs.deepseek.com/guides/kv_cache)：
- DeepSeek 的 disk-based context caching **默认开启**，基于**前缀完全匹配**（"A subsequent request can only hit the cache if it fully matches a cache prefix unit"）。
- 关键含义：**如果把 `reasoning_content` 字段去掉 / 截断 / 改写，前缀就不再 byte-identical，cache 失效**，下一轮重新计费全量 input tokens。
- **所以**"是否必须回传 `reasoning_content`"的强约束来自思考模式协议本身（400），但"是否原样 byte-identical 回传"则影响 cache hit / token 成本。两者方向一致：**原样保留**。
- 当前代码 `[reasoning] {text}` 拼进 content 还会**双重破坏 cache**：(a) 字段错位让 API 直接 400；(b) 即使能过，content 被污染也让前缀对不上。
- pricing 页未单列 reasoning_content 的 token 计费规则，按常规 input/output tokens 计；具体倍率见 [pricing](https://api-docs.deepseek.com/quick_start/pricing)。

### 7. 已知坑 / Caveats

- **byte-identical**：社区 opencode issue [#24104](https://github.com/anomalyco/opencode/issues/24104) 评论（fkyah3、bilbillm）一致结论是"API 要求**所有** assistant 消息带 `reasoning_content`（无则空串），原样回传，不能改写、不能截断"。
- **历史遗留消息**：如果会话中途从别的 provider 切到 DeepSeek，旧 assistant 消息没有 `reasoning_content` 字段 → 也会 400。社区修法是注入空串 `""`（见 issue #24104 fkyah3 评论 + PR #24150）。
- **空串是合法值**：fkyah3 的 fix "now ALL assistant messages unconditionally get `reasoning_content` injected (empty string if no reasoning text)"，验证空串通过。
- **Function Calling 在 V4 已支持**：旧 R1 时代 "Not Supported: Function Calling" 已过时，V4-flash/pro 都支持 Tool Calls（见 [pricing](https://api-docs.deepseek.com/quick_start/pricing) FEATURES 行 ✓）。所以 tool_call 路径是真实存在的，本契约的强制回传分支是常态而非边缘。
- **流式 vs 非流式**：契约一致。流式下 `delta.reasoning_content` 是增量字段（[streaming sample](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_streaming) 用 `reasoning_content += chunk.choices[0].delta.reasoning_content`），客户端需自行累加成完整字符串后再回传。
- **Anthropic 兼容端点** (`https://api.deepseek.com/anthropic`)：[Anthropic API Guide](https://api-docs.deepseek.com/guides/anthropic_api) 存在但未公开 reasoning_content 字段映射细节。opencode issue #24104 评论里 hammerhoundai 用 `@ai-sdk/anthropic` 跑 V4-pro 多轮正常——推测 anthropic 端点把 reasoning 映射到 anthropic `thinking` block。**当前任务用的是 OpenAI 兼容协议，不在本路径**。
- **思考模式禁用参数**：`temperature` / `top_p` / `presence_penalty` / `frequency_penalty` 在思考模式下**不会报错但无效**；`logprobs` / `top_logprobs` **会报错**。

---

## 任务问题逐条回答

| # | 问题 | 答案 |
|---|---|---|
| 1 | 多轮必须回传 reasoning_content？所有轮还是仅中间轮？ | **双路径**：仅在"两次 user 之间发生过 tool_call"时，该路径上**每个** assistant 的 reasoning_content 必须原样回传且所有后续轮都要带；纯聊天中间轮可省（传回也会被忽略）。最稳策略是无脑全部回传。 |
| 2 | 字段位置？ | assistant 消息**顶层**，与 `content` 同级：`{"role":"assistant","content":"...","reasoning_content":"..."}` |
| 3 | content/reasoning 必须分开？当前拼接是错的？ | **必须分开**，当前 `[reasoning] {}` 拼进 content 是 bug 根因。tool_call 路径下 content 常为空串，拼接会破坏协议。 |
| 4 | 接受 reasoning_effort？发 "high" 会 400？ | **接受**，不会 400。DeepSeek 取值 `high`/`max`（`low`/`medium` 映射 `high`，`xhigh` 映射 `max`）。但思考开关由 `thinking.type`（默认 enabled）决定，**不依赖** reasoning_effort。 |
| 5 | deepseek-chat 是否吐 reasoning_content？ | 不吐（非思考模式别名），不需要回传契约。但注意：旧名 2026/07/24 弃用，新名是 `deepseek-v4-flash` + `thinking.type=disabled`。 |
| 6 | reasoning_content 与 cache 关系？ | cache 基于**前缀完全匹配**，改写/截断 reasoning_content 会让 cache 失效。原样回传既满足协议也保 cache。 |
| 7 | 已知坑？ | 必须 byte-identical；中途切 provider 的旧消息需注入空串 `""`；流式需自行累加 delta；V4 已支持 function calling（旧 R1 不支持的文档已过时）。 |

---

## External References

- [DeepSeek API Docs — Reasoning Model (deepseek-reasoner / 旧 R1 协议)](https://api-docs.deepseek.com/guides/reasoning_model) — 旧协议下"传 reasoning_content 进 input 会 400"（**与 V4 相反**），证明协议已迁移。
- [DeepSeek API Docs — Thinking Mode (V4, 当前)](https://api-docs.deepseek.com/guides/thinking_mode) — **本任务权威依据**。双路径契约 + reasoning_effort/thinking 参数语义。
- [DeepSeek API Docs — Multi-round Conversation](https://api-docs.deepseek.com/guides/multi_round_chat) — 通用多轮 stateless 说明。
- [DeepSeek API Reference — Create Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion) — assistant 消息 schema（`reasoning_content` 顶层字段定义）、`model` 取值、`thinking`/`reasoning_effort` 字段。
- [DeepSeek API Sample — thinking_mode non-streaming](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_non_streaming) — 注释 "The reasoning_content will be ignored by the API"（无 tool_call 路径）。
- [DeepSeek API Sample — thinking_mode streaming](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_streaming) — `{"role":"assistant","reasoning_content":...,"content":...}` 回写形态。
- [DeepSeek API Sample — thinking_mode tool_call](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_tool_call) — tool_call 路径 `messages.append(response.choices[0].message)` 原样回传。
- [DeepSeek API Sample — thinking_mode tool_call output](https://api-docs.deepseek.com/api_samples/thinking_mode_api_example_tool_call_output) — 展示 tool_call 路径下中间轮 `content=''` 全在 reasoning_content。
- [DeepSeek API Docs — Context Caching](https://api-docs.deepseek.com/guides/kv_cache) — disk-based，前缀完全匹配。
- [DeepSeek API Docs — Pricing / Models](https://api-docs.deepseek.com/quick_start/pricing) — `deepseek-chat`/`deepseek-reasoner` 2026/07/24 弃用声明 + V4-flash/pro 支持思考/非思考双模式 + function calling ✓。
- [DeepSeek API Docs — Anthropic API](https://api-docs.deepseek.com/guides/anthropic_api) — `https://api.deepseek.com/anthropic` 端点（与本任务 OpenAI 路径无关，仅备查）。
- [GitHub: opencode issue #24104](https://github.com/anomalyco/opencode/issues/24104) — 同款 400 的社区追踪，含 PR #24150 / #17523 / #17529 修复路径，"ALL assistant messages get reasoning_content injected (empty string if no reasoning text)" 的修法。
- [n8n community: Deepseek v4 reasoning_content fix](https://community.n8n.io/t/deepseek-v4-the-reasoning-content-in-the-thinking-mode-must-be-passed-back-to-the-api/295015) — 社区 node 修法佐证。
- [GitHub: qwen-code issue #3658](https://github.com/QwenLM/qwen-code/issues/3658) — 同款 400 在 deepseek-v4 下的复现。

---

## Caveats / Not Found

- **未实测**：本研究未发真实 HTTP 请求到 `api.deepseek.com`，所有结论来自官方文档原文 + 多个独立社区 issue 交叉佐证。修复落地后建议加一条 e2e（mock provider 回 reasoning_content，验证第二轮 request body 带 `reasoning_content` 字段）。
- **`reasoning_content` 在 API Reference 标注 `(Beta)`**：官方 schema 把它标为 Beta 字段，未来可能改名/挪位。当前（2026-06-20）仍是唯一受支持路径。
- **旧 R1 协议 (`deepseek-reasoner` 2025 版) 与 V4 协议相反**：旧 R1 文档原话"if the `reasoning_content` field is included in the sequence of input messages, the API will return a 400 error"。**不要**拿旧 R1 文档当依据——`deepseek-reasoner` 这个名字现在指向 V4 thinking mode，契约已翻转。
- **Anthropic 兼容端点的 reasoning 映射**：未在官方文档找到明文，社区 issue 显示能跑通但字段名空间（`providerOptions.anthropic.reasoning_content` vs `openaiCompatible.reasoning_content`）有不确定性。本任务不涉及该路径。
- **"所有历史轮 vs 仅 tool_call 路径"的边界**：官方文档措辞是"Between two user messages, if the model performed a tool call"——严格按文档，只有夹在两次 user 之间且发生过 tool_call 的那段 assistant 序列才必须回传。但**社区共识是无脑全传最稳**（无 tool_call 路径会被忽略，不报错）。如果实现想省 tokens 走启发式 strip，需要精确实现"找出两次 user 之间是否有 tool 消息"的判断，**不推荐**。
