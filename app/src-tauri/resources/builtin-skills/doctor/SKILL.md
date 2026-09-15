---
name: doctor
description: Diagnose LLM connection and configuration failures. Use when a chat turn failed with an error (auth, rate limit, network, server, invalid request), a model seems unreachable, or the user reports "cannot connect / API error / 模型不可用". Reads a redacted config snapshot and runs one real per-model connection test to localize the fault, then maps it to a concrete fix.
allowed-tools: [llm_diagnostics, test_llm_connection]
---

# LLM 诊断（doctor）

目标：把「发不出消息 / 报错」定位到具体一层（配置 → 网络 → 提供商），给出可执行修复步骤。全程只读 + 一次连接实测。

## 流程

1. **问诊**：向用户要三样信息——报错原文（聊天错误行文案）、涉及哪个会话/模型、何时开始、是否所有会话都复现。
2. **读配置态**：调 `llm_diagnostics` 拿 providers / models / default_model_id 快照。核对：有没有 provider、有没有模型、默认模型指向谁、有没有 disabled 的 provider/model、has_key 是否为 false（= 没填 key）。
3. **实测可疑模型**：调 `test_llm_connection`（传可疑 model_id；未指明则缺省测默认模型）。它对目标模型发一个 1-token 真实请求（真实计费，勿连续重试），返回 success / latencyMs / error。
4. **按分类给修复动作**（下表）。用户口述报错与实测结果矛盾时，以实测为准并说明差异。

## 错误分类 → 修复动作

| 分类 | 典型表现 | 修复动作 |
|---|---|---|
| auth | HTTP 401/403、authentication_error | key 失效或未填：Settings → Providers 更换/重填 key（has_key=false 就是没填）；最近换过机器可能是 key 解密失败，重新填一次即可 |
| rate_limit | HTTP 429/529、rate_limit / overloaded | 等待后重试；频繁触发则换默认模型，或在该提供商侧提升额度 |
| network | request failed / timeout / 无法连接 | 核对 base_url 拼写（protocol 与 `/v1` 规则见 llm-setup）、本机代理/防火墙、DNS；本地 Ollama 先确认服务进程在跑 |
| server | HTTP 5xx | 提供商侧故障：稍后重试；持续则查该提供商状态页/公告 |
| invalid_request | HTTP 400、model not found / 路径错误 | 模型名与提供商文档逐字核对（最常见拼写错）；base_url 少/多 `/v1`；protocol 值只支持 anthropic / openai |

注意：部分提供商兼容层会用 5xx 包装 4xx 类错误，不要只看状态码——结合响应体里的错误类型（authentication / rate_limit / invalid_request 关键字）判断。

## 何时转介 llm-setup

出现配置缺失（无 provider / 无模型 / 无默认模型）、需要新增 provider 或改配置、或查明是 base_url / 模型名配错时——让用户加载 `/llm-setup` 走配置向导；改完回到本流程第 3 步实测闭环。
