# 修复 DeepSeek-Via-Anthropic-Relay reasoning_content 回传 400

## Goal

DeepSeek-v4 (`deepseek-v4-flash`) 经中转站 `https://api.wukaijin.com`（商业 SaaS，Anthropic Messages 端点 passthrough）多轮对话触发 400：

```
{"error":{"type":"invalid_request_error","message":"Error from provider (DeepSeek): The `reasoning_content` in the thinking mode must be passed back to the API."}}
```

修复后 DeepSeek-via-wukaijin 多轮 thinking 跑通，且 Anthropic 原生 Claude 路径 + OpenAI 路径完全不受影响。

## Root Cause（已基于 DB 证据 + warn log 锁定方向）

### 现象
* 4 个 DeepSeek-via-wukaijin session 中 3 个 400 (0a8cc2f0 / 053ae61e / 11cefabc)，1 个跑通 (e9bf6c07)
* wukaijin.com 用 **UUID v4 字符串**作 thinking block signature（DB 验证）
* empty sig (`""`) 与 UUID sig 混合出现，Anthropic SSE 解析侧丢失 `signature_delta` 时存为 empty

### 根因
**wukaijin.com 中转站对 assistant thinking block 做累积状态校验**（具体 threshold 不稳定，与 thinking block 数量 / token / cache / UUID 数量综合相关），校验失败时报"reasoning_content must be passed back"。Anthropic 协议 thin passthrough 模式下，客户端仅发 Anthropic 标准 thinking block + signature，中转站无法从标准 Anthropic shape 提取 DeepSeek V4 期望的 `reasoning_content` 字段。

### 已排除的假因
* mode (yolo/edit) 不是根因（yolo / edit 都 400）
* turn 数 / message count 不是直接根因（e9bf6c07 8 turn 跑通 vs 053ae61e 4 turn 400）
* empty sig 本身不是根因（e9bf6c07 turn 0-1 empty 仍 work）

## Requirements

* **R1（方案 A）**：`AnthropicProvider` 发请求时，对**有非空 sig thinking block 的 assistant 消息**，在消息对象顶层（与 `content` 同级）**额外**加一个 `reasoning_content` 字段，值为所有非空 sig thinking block 的 `thinking` 文本（多块用 `\n` 拼接）。Anthropic 协议非标准扩展，但 wukaijin.com 中转站需要该字段才能转回 DeepSeek V4。
* **R2（方案 B）**：`AnthropicProvider` 发请求时，从 assistant 消息的 `content[]` 数组中**过滤掉 `signature: ""` 的 thinking 块**。减少 thinking block 数量 + 消除 empty/UUID sig 混合引起的状态不一致。
* **R3（不破坏现有）**：Anthropic 原生 Claude 路径（无 relay）继续走标准 thinking block + signature，不输出 `reasoning_content`（因为 Anthropic 协议无此字段）—— 通过"只在 thinking block 存在时输出 reasoning_content"实现自然兼容。
* **R4（不破坏 OpenAI 路径）**：`OpenAIProvider` 完全不动（OpenAI 协议 + 用户未报告 OpenAI 路径的同类问题）。修复**仅作用于 `AnthropicProvider`**。

## Acceptance Criteria

* [ ] 新单测 `apply_deepseek_reasoning_fix`：assistant 消息含 empty sig thinking + UUID sig thinking + text → 输出 JSON：empty sig 块被移除；UUID sig 块保留；消息顶层加 `reasoning_content` 字段（值 = UUID sig 块的 thinking 文本）。
* [ ] 新单测：assistant 消息只含 empty sig thinking + text → empty 块被移除；**不加** `reasoning_content` 字段（值为空时跳过）。
* [ ] 新单测：assistant 消息只含 UUID sig thinking + text → thinking 块保留；加 `reasoning_content` 字段。
* [ ] 新单测：user 消息**完全不动**（content 不修改，不加 reasoning_content）。
* [ ] 新单测：assistant 消息无 thinking block（纯 text + tool_use）→ 不加 `reasoning_content` 字段。
* [ ] 新单测：顶层 `thinking` 字段（adaptive summarized）保留不动（兼容 Claude extended thinking）。
* [ ] `PKG_CONFIG_PATH="..." cargo test --lib` 全绿。
* [ ] Anthropic 原生 Claude 路径单测（已有）仍通过。
* [ ] OpenAI 路径单测（已有）仍通过。

## Definition of Done

* Rust 单测覆盖 R1-R4 全部行为 + 与 OpenAI/Claude 路径不冲突。
* `trellis-update-spec` 捕获 DeepSeek-Via-Anthropic-Relay 契约到 `.trellis/spec/backend/`。
* `spec/backend/llm-contract.md` 加 RULE：DeepSeek-via-relay thinking block 处理。
* `DEBT.md` 检查并回填 commit hash。
* 四段式 commit（fix→docs→archive→journal）。

## Technical Approach

### 改动文件

**`app/src-tauri/src/llm/provider/anthropic.rs`**（唯一改动文件）

1. 在 `send()` 入口（line 731 之前）加一个 `pub(crate) fn apply_deepseek_reasoning_fix(req: ChatRequest) -> serde_json::Value`：
   - 序列化 `ChatRequest` 为 `serde_json::Value`
   - Walk `body["messages"]` 数组，对每个 `role == "assistant"` 的 message：
     - 收集所有 `ContentBlock::Thinking` 的 `thinking` 文本（拼接 `\n`）
     - Walk `message["content"]` 数组（如果是数组），**移除** `type == "thinking"` 且 `signature == ""` 的 block
     - 若收集的 reasoning 文本非空，在 `message` 顶层加 `reasoning_content` 字段
   - 返回 `serde_json::Value`

2. 改 `send()` 函数体：line 731 `Box::pin(Self::chat_stream_with_tools(config, req))` → 把 `req` 改成序列化后的 `serde_json::Value`，传新参数。

3. 改 `chat_stream_with_tools(config, body: serde_json::Value)`：line 247 `.json(&req)` → `.header("content-type","application/json").body(body.to_string())`。

4. 保留 `tracing::info!(...)` log 的 `model / tools_count / has_system` 等字段（从 `body` 提取，不要丢失）。

5. 加 `#[cfg(test)] mod deepseek_reasoning_fix_tests` 覆盖 6 条新单测。

### 不动文件
* `provider/openai.rs` — OpenAI 路径完全不动
* `provider/wire.rs` — strip 逻辑不动（Anthropic caps 全 true，strip no-op）
* `provider/mod.rs` — `build_provider` 不动
* `agent/chat_loop.rs` — turn 边界不动
* `db/` — 不加 schema
* `types.rs` — ContentBlock schema 不动

## Decision (ADR-lite)

**Context**: 4 个 DeepSeek-via-wukaijin session 中 3 个 400，DB 揭示中转站用 UUID sig 替换 Anthropic base64 sig，且 empty/UUID 混合出现。Anthropic 标准 thinking block + signature 经 wukaijin.com passthrough 到 DeepSeek V4 后端时，无法触发 DeepSeek V4 的 `reasoning_content` 契约校验。

**Decision**:
1. **A 方案**（核心）：在 `AnthropicProvider` 请求构造时，对有非空 sig thinking block 的 assistant 消息加顶层 `reasoning_content` 字段。Anthropic 协议非标准扩展，**只** wukaijin.com 之类的中转站会消费。Anthropic 原生 API 会忽略未知字段。
2. **B 方案**（加固）：过滤 empty sig thinking 块，减少 thinking block 数量 + 消除 sig 状态不一致。
3. **不实施 D 方案**（去掉 Anthropic 顶层 `thinking` 字段）：影响 Claude extended thinking 路径太广，需更多 evidence 才能安全实施。留作 follow-up task。

**Consequences**:
* Anthropic 原生 Claude 路径：新代码路径在 assistant 消息加 `reasoning_content` 字段（值为 thinking 文本）。Anthropic 官方 API 接受未知字段（serde 默认行为），extended thinking 行为不变。
* OpenAI 路径：完全不动。
* wukaijin.com 中转站：thinking block 数量减少 + 增加 `reasoning_content` 字段 → 降低 400 触发概率。
* 风险：若某些 strict relay 拒绝 `reasoning_content` 字段（非 wukaijin.com 商业中转站）→ 需后续按 relay 配置 model 别名或加新 capability 信号。

## Out of Scope

* Anthropic 顶层 `thinking` 字段（D 方案，留作 follow-up）—— 需更深入 evidence 才能动 Claude 路径
* wukaijin.com 中转站本身的修复（不归本仓库）
* 新 ModelRow capability 字段
* DB schema 改动
* 前端思考渲染

## Follow-up Tasks（待本任务完成后开新任务）

* **FT1**: 调查 Anthropic 顶层 `thinking` 字段对 DeepSeek V4 的影响（D 方案）
* **FT2**: 调查 wukaijin.com 具体的 400 threshold（基于实测数据）
* **FT3**: 评估是否需要按 relay 自动分发 capability（heuristic 或新字段）

## Technical Notes

* 核心修复在 `anthropic.rs::send` 入口的请求构造阶段，用 `serde_json::Value` walk 处理。
* `serde_json::Value` 走 `body.to_string()` 序列化比 `req.to_string()` 多一次序列化（性能影响极小，毫秒级）。
* `apply_deepseek_reasoning_fix` 必须为 `pub(crate)` 方便单测。
* Anthropic 顶层 `thinking: Some(config.thinking_config())` 字段（line 728）保留不动。
* Wire 层（`wire.rs`）不动 — Anthropic caps 全 true，strip 是 no-op，outbound 走 `wire_messages_to_chat_messages` 重组 ChatRequest。
* 已落地研究：`research/deepseek-reasoning-content-contract.md`（V4 thinking mode 契约 + 旧 R1 协议对照 + opencode 社区修法）。
* 两次外部研究（research subagent + WebSearch）触发网关 500/400 → 已用 DB 实际证据 + 4 session 模式对比完成根因定位。
