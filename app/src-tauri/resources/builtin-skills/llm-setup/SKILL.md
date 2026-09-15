---
name: llm-setup
description: Guide the user through adding or fixing an LLM provider/model configuration in Settings, then verify it with a real connection test. Use when the user asks how to configure a provider or model, mentions API keys, base URLs, or provider accounts, or wants to switch providers. Never ask the user to paste an API key into the conversation.
allowed-tools: [llm_diagnostics, test_llm_connection]
---

# 配置 LLM Provider(llm-setup)

帮用户完成「provider → model → 默认模型」三步配置，并用 `test_llm_connection` 实测收尾。配置动作发生在 Settings 界面（用户操作）；你负责给路径、查知识、验证结果。需要核对当前配置态时先调 `llm_diagnostics`（只读快照，不含任何密钥）。

## 铁律：API key 永不进对话

对话内容会原文发送给 LLM 提供商，key 一旦出现在聊天里就等于泄露。**永不要求用户把 API key 粘贴进对话**；key 只能填进 Settings 的 Providers 表单（本地加密存储）。若用户已把 key 粘进对话：告知该 key 应视为泄露，建议到提供商控制台吊销重发，新 key 只填 Settings 表单。

## 配置步骤

1. Settings → Providers → 新增：选 protocol、填 base_url、display_name、API key（key 只填在这里）。
2. Settings → Models → 在该 provider 下新增模型：`model_name` 必须与提供商文档/控制台的模型 ID 逐字一致（display_name 是本地别名，随意）。
3. Settings → Models → 选默认模型。
4. 调 `test_llm_connection`（可传 model_id，缺省测默认模型）实测：成功返回延迟毫秒数；失败按错误文案定位（排障流程见 doctor skill）。

## base_url 拼接规则（最高频错误源）

- protocol = `anthropic`：POST `{base_url}/v1/messages` → base_url **不带** `/v1`（如 `https://api.anthropic.com`）。
- protocol = `openai`：POST `{base_url}/chat/completions` → base_url **必须含** `/v1`（如 `https://api.openai.com/v1`）。少写会 404；在已含 `/v1` 的地址上再补一遍会拼出 `/v1/v1/...`。

## 常见 provider 速查

| 提供商 | protocol | base_url（常用值） | 注意事项 |
|---|---|---|---|
| Anthropic | anthropic | `https://api.anthropic.com` | 模型 ID 以官方文档为准（claude-*） |
| OpenAI | openai | `https://api.openai.com/v1` | 模型 ID 以平台文档为准 |
| DeepSeek | openai | `https://api.deepseek.com/v1` | OpenAI 兼容端点 |
| 智谱 GLM | openai | `https://open.bigmodel.cn/api/paas/v4` | 模型 ID（glm-*）以 bigmodel 文档为准 |
| Kimi（Moonshot） | openai | `https://api.moonshot.cn/v1`（国内）/ `https://api.moonshot.ai/v1`（国际） | 模型 ID 以控制台为准 |
| OpenRouter | openai | `https://openrouter.ai/api/v1` | 模型 ID 带厂商前缀（如 `anthropic/claude-*`） |
| Ollama（本地） | openai | `http://localhost:11434/v1` | 模型 ID = `ollama list` 中的名字；Ollama 不校验 key，但本 app 要求 key 非空，填占位串（如 `ollama`）即可 |

模型 ID 与 URL 会随提供商版本变化：不确定时用保守表述，建议用户查提供商控制台为准。连接测试返回 HTTP 404 / `model not found` 时，优先怀疑 base_url 的 `/v1` 规则和模型名拼写。
