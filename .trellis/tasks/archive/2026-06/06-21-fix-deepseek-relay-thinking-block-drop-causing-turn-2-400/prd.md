# fix deepseek relay thinking-block drop causing turn-2 400

## Goal

修复 deepseek-v4-flash 经 wukaijin.com Anthropic 中转站在 **turn 2** 报
`400 "The content[].thinking in the thinking mode must be passed back to the API."`
的 bug。根因是上一个 task（`06-20-deepseek-reasoner-reasoning-content-400`）
加的 `apply_deepseek_reasoning_fix` **误删空签名 thinking 块**——而该中转站
实际要求 `content[].thinking` 块 **与** `reasoning_content` 字段**同时存在**，
删块即触发"thinking must be passed back"。

## 根因（已用真实中转站实验坐实）

中转站 `https://api.wukaijin.com`（Anthropic `/v1/messages` schema，上游 deepseek-v4-flash）
的 thinking 模式契约 = **两者必备**：

| 回传的 assistant shape | relay 响应 |
|---|---|
| 删掉 thinking 块（**当前现状**） | 400 `content[].thinking must be passed back` |
| 保留空签名 thinking 块，**不加** `reasoning_content` | 400 `reasoning_content must be passed back` |
| 保留空签名 thinking 块，**加** `reasoning_content` | **200 ✅** |

关键结论：
1. **空签名 thinking 块 relay 完全接受**（不密码学验证 signature）——上一次 fix
   注释里"空签名 opaque / inflate accumulated-state count"的归因**是错的**。
2. 上次 fix 的 (B) 删块**从未真正解决旧 400**——只是把错误主语从
   `reasoning_content` 换成了 `thinking`。真正的修复必须**两样都给**。
3. DB 里 `signature=""` 是因为 relay 在**流式**模式下不发 `signature_delta`
   （非流式响应里 relay 反而补占位 uuid 签名）。故持久化必然落空签名——
   修复必须按空签名处理，V2 实验证明空签名块可接受。

证据：session `863fda30-66a1-421d-bd91-0c3a6bb9b342` seq=1 assistant 的
thinking 块 `"signature": ""`；复现脚本 `/tmp/ds_probe/v{1,2,3}*.json`。

## Requirements

1. **取消 `apply_deepseek_reasoning_fix` 的删块逻辑**（anthropic.rs:700-720
   的 `arr.retain(...)`）——保留所有 thinking 块，含空签名。
2. **推广 reasoning_content lift**：reasoning_content 收集从"仅 retain 后存活的
   非空签名块"改为"**所有** thinking 块"（删掉 retain 后输入自然变成全部块，
   收集逻辑本身不变）。`if !reasoning_buf.is_empty()` 守卫保留——纯
   text+tool_use 的 assistant 消息不加空 `reasoning_content`。
3. **重写把错误契约 pin 死的两个测试**（anthropic.rs 测试模块）：
   - `deepseek_reasoning_fix_removes_empty_sig_thinking_blocks` →
     重命名为 `..._keeps_empty_sig_and_lifts_reasoning_content`，
     断言空签名块保留 + `reasoning_content` = 所有块文本 `\n` join。
   - `deepseek_reasoning_fix_omits_reasoning_content_when_all_empty` →
     改为：全空签名时块全保留 + `reasoning_content` = join 文本（不再 omit）。
4. **新增 relay 契约 pin 测试**：构造 V1/V2/V3 三种 turn-2 assistant shape，
   断言 fix 产出的是 V2（块 + reasoning_content 齐全），并在测试注释里写死
   relay 契约（两者必备、签名不验证），防止未来再被"删块"回归。
5. 非空签名 thinking 块的现有测试（`..._keeps_nonempty_sig_and_adds_reasoning_content`
   / `..._concatenates_multiple_nonempty_blocks` / `..._skips_user_messages`
   / `..._no_thinking_blocks_no_reasoning_content` / `..._preserves_top_level_thinking_field`）
   行为不变，需确认不回归。

## Acceptance Criteria

- [ ] `apply_deepseek_reasoning_fix` 不再删除任何 thinking 块。
- [ ] 任意含 thinking 块的 assistant 消息，产出同时含 `content[].thinking`
      块与顶层 `reasoning_content` 字段的 body（= V2 shape）。
- [ ] 纯 text+tool_use（无 thinking 块）的 assistant 消息不加 `reasoning_content`。
- [ ] user / tool_result 消息完全不被触碰。
- [ ] 顶层 `thinking: adaptive` 字段不被触碰。
- [ ] 新增的 V1/V2/V3 契约 pin 测试断言 fix 产出 V2。
- [ ] `cargo test --lib`（含 `deepseek_reasoning_fix_tests::*`）全绿。
- [ ] 对真 Anthropic Claude 路径无行为回归（非空签名块仍保留 + 加
      `reasoning_content`，Anthropic 忽略未知 top-level 字段）。

## Definition of Done

- 单测覆盖修复 + relay 契约 pin。
- `cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib` 绿。
- spec 沉淀（见 Out of Scope 之后的 follow-up）：wukaijin relay thinking 契约 +
      "对外部 relay 行为归因必须实测"教训（建议 `trellis-break-loop`）。
- commit：fix →（可选 spec）→ journal。

## Technical Approach

最小改动，单文件 `app/src-tauri/src/llm/provider/anthropic.rs`：

```rust
// apply_deepseek_reasoning_fix，约 700-740
// (B) 删除：不再 retain（保留所有 thinking 块）
// (A) 推广：遍历所有 thinking 块（不再依赖 retain 后存活）lift reasoning_content
let mut reasoning_buf = String::new();
for block in arr.iter() {
    if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
        if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
            if !reasoning_buf.is_empty() { reasoning_buf.push('\n'); }
            reasoning_buf.push_str(text);
        }
    }
}
if !reasoning_buf.is_empty() {
    msg["reasoning_content"] = serde_json::Value::String(reasoning_buf);
}
```

函数仍**无条件对所有 anthropic 请求**跑（anthropic.rs:905）。对真 Claude
无害：它从无空签名块，且 Anthropic 忽略未知 top-level 字段。

## Decision (ADR-lite)

**Context**: 上次 fix（`06-20-deepseek-reasoner-reasoning-content-400`）从
"3/4 deepseek 会话 400、存活会话早期轮次有空签名没 trip"的现象**猜测**归因到
"空签名 thinking 块 inflate relay accumulated-state count"，据此加删块逻辑。
该归因未经实测，删块制造了新的 turn-2 400。

**Decision**: 用真实中转站 V1/V2/V3 对照实验**先验证归因再改**。实验证明
relay 契约是"块 + reasoning_content 两者必备、签名不验证"，正确修复 = 保留块
+ 推广 reasoning_content（V2）。

**Consequences**: 修复极小且对真 Claude 无副作用。教训——外部 relay/api 行为
的归因必须实测对照，不能从现象猜（正是 `trellis-break-loop` 要防的
fix-forget-repeat）。

## Out of Scope

- **不改流式 SSE signature 解析**：relay 流式不发 `signature_delta` 是上游行为，
  客户端落空签名 + V2 已证明空签名可接受，无需在客户端"伪造"签名。
- **不加 `thinking_config` 门控**：`supports_thinking=1` 是用户明确意图，保留。
- **不改 OpenAI provider**：OpenAI 路径不经此 fix（它走 reasoning_effort /
  reasoning_content 另一套）。
- **不改多块 join 策略**：维持现有 `\n` join。

## Technical Notes

- 关键文件 / 行：`app/src-tauri/src/llm/provider/anthropic.rs`
  - `apply_deepseek_reasoning_fix` 677-745（改 700-740）
  - 调用点 905（无条件）
  - `thinking_config()` 104-108（永开 thinking，**不动**）
  - 测试模块 1115-1400（重写 2 + 新增 1）
- thinking 块构造链：`chat_loop.rs:1032-1035` push 进 assistant_blocks →
  `messages.push(msg)` 1148 → `turn_messages = messages.clone()` →
  `provider.send` → wire → `apply_deepseek_reasoning_fix`。
- 证据 session：`863fda30-66a1-421d-bd91-0c3a6bb9b342`（DB
  `~/.local/share/dev.everlasting.app/everlasting.db`）。
- 复现脚本：`/tmp/ds_probe/v{1,2,3}*.json` + curl（base_url/token 来自 env）。
- 上一个相关 task：`.trellis/tasks/06-20-deepseek-reasoner-reasoning-content-400/`。
