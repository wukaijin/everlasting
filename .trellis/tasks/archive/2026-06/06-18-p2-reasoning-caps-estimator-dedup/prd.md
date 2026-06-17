# brainstorm: B6 前置债 — reasoning caps + estimator dedup

## Goal

修 DEBT.md 收尾路径建议第 4 条("进 B6 前抽 A-008 + 修 D-004/D-005"),为 B6 Subagent(worker agent 独立 context/token 预算)扫清 Provider/Agent 层前置障碍。三项均在 Provider/Agent 模块,打包合理。

## 范围(三项打包)

### RULE-D-005 — OpenAI supports_reasoning_effort caps hardcode true (P2, active bug)

- **File**: `app/src-tauri/src/llm/provider/openai.rs:395-399`
- **现状**: `send` 里 caps 构造 `supports_reasoning_effort: true` 硬编码
- **后果**: 用户从 Anthropic(有 extended thinking)切到 OpenAI **gpt-4o**(无 reasoning 能力)时,`strip_unsupported` 因 caps 硬说支持而**保留**历史 Reasoning 块,污染 gpt-4o 上下文
- **Fix**: caps 从 `self.config.reasoning_effort.is_some()` 派生;抽 testable 的 `openai_caps()` 函数,send 调它

### RULE-D-004 — WireRequest.reasoning_effort dead field (P2)

- **File**: `app/src-tauri/src/llm/provider/wire.rs:127-133` + `:260`
- **现状**: 字段标 `#[allow(dead_code)]`,注释撒谎"OpenAI reads it",实际 OpenAI adapter 读的是 `config.reasoning_effort`(openai.rs:278);`chat_request_to_wire` 写死 `None` 从不填
- **Fix**: **删字段**(非接通,见决策 D-004-A)

### RULE-A-008 — estimate_messages_tokens 与 _iter 版大段重复 (P2)

- **File**: `app/src-tauri/src/agent/context.rs:121-169` vs `:333-375`
- **现状**: 两函数 buf 构造逻辑(role + to_text + blocks match)100% 重复,_iter 版只多 `dropped[i]` 跳过
- **Fix**: 抽 `push_message_tokens(buf, m)` helper,两函数共用

## 关键决策

### D-005-A: caps 用 config 派生,不直接调 from_model_row

`WireCapabilities::from_model_row`(wire.rs:97)需要 `&ModelRow`,但 `Provider::send` trait 签名不带 model_row。threading model_row 进 send 是 **trait 级改动**(影响 AnthropicProvider + 所有调用点),超出本 task 范围。

而 `OpenAIProvider` 持有 `self.config.reasoning_effort: Option<String>`,它在 `build_provider`(mod.rs:181)里由 `model_row.thinking_effort.clone()` 填入 —— **这就是 from_model_row 对 OpenAI 的等价信息**。所以 caps 从 `self.config.reasoning_effort.is_some()` 派生,语义与 from_model_row 一致,改动最小。

`from_model_row` 保留(wire.rs tests 已覆盖,是 capabilities 派生的正确实现,留给未来 Provider::send thread caps 的 PR)。

### D-004-A: 删字段,非接通

reasoning_effort 是 **OpenAI-specific** 参数,不属于 provider-agnostic wire 层。真正的参数流转 `self.config.reasoning_effort → HTTP body`(openai.rs:278)已完整,wire 字段是冗余。接通它只是"为用而用",增加复杂度而无跨协议收益(Anthropic 用 thinking_effort 走 ThinkingConfig 另一条路)。删字段 = 最小改动 + 零回归面。

## 不做(范围控制)

- **不做前端 reasoning_effort 配置 UI**:已存在(ModelForm.vue:165 "Thinking Effort" 下拉)
- **不做 session 持久化 reasoning_effort**:已持久化在 models 表 catalog 层(migrations.rs:256);per-session override 是独立新功能(产品决策),不混入 bug 修复(避免范围蔓延,对齐 RULE-B-004 收尾原则)
- **不改 Provider::send trait 签名**:threading model_row 是 trait 级改动,超范围

## 现状链路(确认无需前端/DB)

```
前端 ModelForm.vue:165 "Thinking Effort" 下拉
  → commands/providers.rs create/update → models 表 thinking_effort (migrations.rs:256)
  → build_provider (mod.rs:181) model_row.thinking_effort.clone() → OpenAIConfig.reasoning_effort
  → openai send: caps 从 config 派生(D-005 修复后) + build_http_body 写 HTTP body["reasoning_effort"]
```

## 验收

- `PKG_CONFIG_PATH=... cargo test --lib` 全 pass,0 warning
- D-005 新测试:reasoning_effort=None 的 provider 构造的 caps.supports_reasoning_effort=false
- A-008: 现有 estimate_messages_tokens 测试(context.rs:983)回归通过
- DEBT.md 回填 D-004/D-005/A-008 Closed At + Re-eval Log
- ROADMAP §2 表格 D3 行划掉(文档滞后收尾)
