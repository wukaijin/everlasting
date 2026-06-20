# route deepseek via openai protocol for native reasoning_content

## Goal
deepseek-v4-flash 改走 OpenAI 协议（wukaijin `/v1/chat/completions`），用 DeepSeek 原生 `reasoning_content` 字段，根治经 Anthropic 协议中转时 thinking/reasoning_content 翻译不可靠导致的 turn-2+ 400。

## 背景（根因，已实测确认）
deepseek 当前走 **anthropic 协议**（wukaijin 中转 Anthropic schema → DeepSeek OpenAI）。relay 的 extended thinking 翻译**不可靠**：
- V1（删空签名 thinking 块，06-20 fix）→ 400 `thinking must be passed back`
- V2（保留块 + 加 `reasoning_content` 字段，commit 55aa9f3）→ 400 `reasoning_content must be passed back`
- relay 校验**非确定**（同 payload 时好时坏，v3 实测）
- AstrBot PR 7823 证实 DeepSeek v4 要求每个 assistant 历史消息有**非空** `reasoning_content`（空时 `"none"`）——但那是 **OpenAI 协议**；Anthropic 协议下客户端加字段反而干扰 relay 自己的翻译（实测 `rc="none"` 400，不加反而 200）。

结论：根子在**协议翻译层**，不是块的保留/删除。DeepSeek 原生是 OpenAI 协议，应直接走 OpenAI。

## 方案 A（已验证可行）
wukaijin 同时支持 OpenAI schema：`POST /v1/chat/completions` + `deepseek-v4-flash` → 200，响应原生含 `reasoning_content` 字段。走 OpenAI 协议即**无翻译层**，`reasoning_content` 原生工作。

## Requirements
1. **OpenAIProvider 历史回传**：assistant 的 `Reasoning` 块 → `message.reasoning_content` **字段**（现状 openai.rs:307-315 是 prepend `[reasoning] text` 到 content 文本，要改为放回字段）。
2. **纯 text assistant**（无 reasoning，如 worker memory ack、模型纯文本回复）→ `reasoning_content="none"`（DeepSeek 要求非空，AstrBot 做法）。
3. **配置**：deepseek-v4-flash 走 openai provider（wukaijin OpenAI base_url）——配置层，见 Open Questions。
4. **SSE 解析**：已有（`delta.reasoning_content` → `thinking_delta`，openai.rs:22）。
5. **reasoning_effort**：验证 deepseek OpenAI 是否需要/接受该字段（curl 未传也有 reasoning，可能默认开）。
6. **清理**：移除 anthropic.rs 临时 DIAG warn log（06-21 诊断遗留）。
7. **apply_deepseek_reasoning_fix 去留**：deepseek 走 openai 后，该 fix 只影响真 Claude（加被忽略字段，无害）+ 其他 anthropic relay 模型。保留不删。

## Acceptance Criteria
- [ ] deepseek-v4-flash 走 OpenAI 协议，多轮（含 subagent 并行 tool）不再 400。
- [ ] thinking/reasoning 在前端正常展示（SSE thinking_delta）。
- [ ] 历史 reasoning 正确回传（`reasoning_content` 字段），纯 text assistant 有 `"none"`。
- [ ] 真 Claude（anthropic 协议）不回归。
- [ ] `cargo test --lib` 绿；DIAG log 移除。

## Open Questions
1. **配置方式**：用户手动加 openai provider（推荐，符合 DB 配置架构）vs 代码自动路由 deepseek 到 openai。
2. **reasoning_effort**：deepseek OpenAI 是否需要该参数触发 reasoning，还是默认开。

## Technical Notes
- wukaijin OpenAI 端点验证：`/v1/chat/completions` + deepseek-v4-flash → 200 + 原生 `reasoning_content`。
- OpenAIProvider 现状：`reasoning_effort` 字段（openai.rs:278）、SSE `delta.reasoning_content` 解析（openai.rs:22）、wire `Reasoning` 块（wire.rs:178）。缺口：历史 `Reasoning` → content 文本（openai.rs:307-315），应改 → `reasoning_content` 字段。
- AstrBot PR 7823：DeepSeek v4 `reasoning_content` 契约（非空，`"none"`）。
- V2 修复（commit 55aa9f3）：对真 Claude 正确保留，对 deepseek-anthropic 无效（不 revert）。
