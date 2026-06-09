# 06-08 multi-model PR2: Anthropic adapter

> Task: `06-09-06-08-multi-model-pr2-anthropic-adapter`
> Status: planning (brainstorm)
> Parent task: `06-08-multi-model-llm-provider-planning` (PR 切片 K1)
> 基线分支: `06-08-multi-model-llm-provider-planning-pr1-data-layer` (PR1 已 commit `f9c5648`)

## Goal

把 `app/src-tauri/src/llm/client.rs` 拆出 `Provider` trait + `AnthropicProvider` 实现,
`lib.rs` 的 `chat` 命令改走 catalog 解析 (`session.model_id` → `models` → `providers`),
PR2 全部 session 仍走 Anthropic 协议(行为与现网 100% 一致,前端零改动)。

## What I already know(从代码 + parent PRD 探查)

### 现状(代码层)

- **`app/src-tauri/src/llm/client.rs` (582 行)**: `LlmConfig` 5 字段(`base_url`/`model`/`api_key`/`max_tokens`/`thinking_effort`);`chat_stream_with_tools()` 硬编码 `POST {base_url}/v1/messages` + `x-api-key` header;`BlockState` 状态机处理 text/tool_use/thinking/redacted_thinking;`thinking_config()` 总是发 `{type: "adaptive", display: "summarized", effort}`;tests 4 个。
- **`app/src-tauri/src/llm/types.rs` (640 行)**: `ChatRequest` / `ChatEvent` / `ContentBlock` **全部 Anthropic-shaped**(`thinking` / `redacted_thinking` 块),这意味着 PR2 的 `Provider` trait 可以直接复用 `ChatRequest` 作为入参、复用 `ChatEvent` 作为流产物 — **不需要 WireMessage 中间层**(中间层是 PR3 OpenAI 才需要)。
- **`app/src-tauri/src/lib.rs:212` `get_llm_config`**: 当前从 `state.config`(`LlmConfig::from_env()` 的结果)读 `model` / `base_url` / `configured`。parent PRD §"API contracts" 写 "PR2 will replace `get_llm_config` with a catalog read"。
- **`app/src-tauri/src/lib.rs:1109-1365` `chat` 命令**:
  - 入口 pre-flight check (`config.is_unconfigured()`) → 返 `Error{Auth}` event
  - line 1360 调用 `chat_stream_with_tools(config.clone(), Some(system_prompt.clone()), messages.clone(), tool_defs.clone())` — **每 turn 重 clone config**
  - `config` 来源:`state.config.clone()` (line 1116,启动时 `LlmConfig::from_env()` 一次)
- **`app/src-tauri/src/lib.rs:395` `list_sessions`**、`app/src-tauri/src/lib.rs:1333` `build_system_prompt` 与 `lookup_head_sha` 都不依赖 provider
- **PR1 已落地的 catalog 层**:
  - `db::ProviderRow` / `db::ModelRow` / `db::ModelWithProvider`(spec 1446-1502 行)
  - `db::list_providers` / `db::list_models` / `db::get_config_value("default_model_id")` 已可用
  - seed:2 provider + 4 model + default=`claude-sonnet-4-5`
- **前端的契约**:
  - `app/src/stores/config.ts` 调 `get_llm_config` 拿 `{model, baseUrl, configured}`
  - PR2 不动 UI(PR4 才改),所以 `get_llm_config` 返回 shape 必须保持 `{model, baseUrl, configured}`

### 4 个 HACKING-llm 坑保留位置

1. **GLM 兼容层**(401 / 5xx / max_tokens 不严格)— Anthropic adapter 内部处理
2. **thinking 块签名** — BlockState 已在 client.rs,Anthropic adapter 沿用
3. **extended thinking `display: "summarized"`** — 强制设置,Anthropic adapter 内部
4. **orphan tool_use**(Bug 1 in step 4 followup)— 仍需保留行为,Anthropic adapter 内部

### 关键文件(影响面)

- `app/src-tauri/src/llm/client.rs`(全文搬移/拆解)
- `app/src-tauri/src/llm/mod.rs`(改 `pub mod provider;` + re-export)
- `app/src-tauri/src/lib.rs:1109-1365`(`chat` 命令 catalog 解析)
- `app/src-tauri/src/lib.rs:212`(`get_llm_config` catalog 化)
- `app/src-tauri/src/lib.rs:85`(`LlmConfig::from_env()` 启动路径 — 改/不改待决)
- `.trellis/spec/backend/llm-contract.md`(PR2 section 待加)

## Assumptions(临时,待验证)

- 假设 1:PR2 全部 session 走 Anthropic 协议(无 OpenAI),`ProviderProtocol` 解析结果目前只可能为 `Anthropic` — 仍需写解析 + 防御性 `ProviderProtocol::Openai` 返 `PreFlightError`(占位实现等 PR3)
- 假设 2:Provider dispatch 时机 = `chat` 命令开始时解析一次(同 session 内 20 turn 用同一 provider)。原因:(a) 避免 user 切换 model 造成 turn 内协议不一致;(b) parent PRD 默认"行为完全不变",env 路径是启动时一次。
- 假设 3:`get_llm_config` 替换为 catalog 读后,前端 store 拿到的 `model` 字段 → **catalog 的 `display_name`**(已决)。

## Open Questions(已收敛)

- [x] **Q1: `get_llm_config` 替换后的 `model` 字段含义** — **catalog `display_name`**(2026-06-09 决议)。理由:PR4 StatusBar dropdown 也用 display_name,前端契约一致。
- [x] **Q2: Pre-flight 错误粒度** — **按场景分 3 种文案**(2026-06-09 决议):
  - `provider.api_key` 空 → `Error{Auth, "请到 Settings 填 {provider_display_name} 的 api_key"}`
  - `model_id` 解析失败(default 未设 / catalog 查不到)→ `Error{InvalidRequest, "没有可用 model,请到 Settings 选 default model"}`
  - `provider` 找不到 → `Error{InvalidRequest, "default model 指向的 provider 已被删除,请到 Settings 重选"}`
  - PR4 modal 跳转时直接按 `category` + `message` 文案决定 toast 与跳转路径。

## Requirements(2026-06-09 收敛)

### 1. Provider trait

```rust
// app/src-tauri/src/llm/provider/mod.rs
pub trait Provider: Send + Sync {
    fn send(
        &self,
        system: Option<String>,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolDef>,
    ) -> impl Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static;

    /// Static capabilities of this provider — independent of any specific
    /// model. The model-level capabilities (e.g. supports_thinking) live
    /// on `ModelRow` and may be combined at dispatch time.
    fn capabilities(&self) -> ProviderCapabilities;

    fn protocol(&self) -> ProviderProtocol;
}

pub struct ProviderCapabilities {
    pub supports_system_prompt: bool,  // Anthropic yes / OpenAI yes
    pub supports_tools: bool,          // both yes
    pub supports_streaming: bool,      // both yes
}
```

### 2. AnthropicProvider impl

- 路径: `app/src-tauri/src/llm/provider/anthropic.rs`
- 把现有 `client.rs` 的 `chat_stream_with_tools` 整体搬过去,改名为 `AnthropicProvider::send`
- `LlmConfig` 字段作为 `AnthropicProvider::new(LlmConfig) -> Self`
- 4 个 HACKING-llm 坑保留在内部
- `tests` 模块从 `client.rs` 搬过来 + 加 1-2 个"构造 AnthropicProvider 后 send 行为一致"测试

### 3. 模块导出

- `app/src-tauri/src/llm/mod.rs` 改:`pub mod provider;` + `pub mod sse;` + `pub mod error;` + `pub mod types;`
- `client.rs` 文件**删除**(逻辑全部搬到 `provider/anthropic.rs`)
- `pub use provider::anthropic::AnthropicProvider;`
- `pub use provider::{Provider, ProviderCapabilities, ProviderProtocol};`
- `pub use types::{ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role, ToolDef};`

### 4. Provider 工厂

```rust
// app/src-tauri/src/llm/provider/mod.rs
pub fn build_provider(
    provider_row: &db::ProviderRow,
    model_row: &db::ModelRow,
) -> Result<Box<dyn Provider>, ProviderBuildError> {
    match provider_row.protocol.as_str() {
        "anthropic" => {
            let config = LlmConfig {
                base_url: provider_row.base_url.clone(),
                model: model_row.model_name.clone(),
                api_key: provider_row.api_key.clone(),
                max_tokens: model_row.max_tokens.unwrap_or(16384),
                thinking_effort: model_row.thinking_effort.clone().unwrap_or_else(|| "high".to_string()),
            };
            Ok(Box::new(AnthropicProvider::new(config)))
        }
        "openai" => Err(ProviderBuildError::NotImplemented("openai")),
        other => Err(ProviderBuildError::UnknownProtocol(other.to_string())),
    }
}
```

### 5. `chat` 命令 catalog 解析

- 在 spawn task 入口(`lib.rs:1158` 之后)做 catalog 解析:
  1. 读 `loaded_session.session.model_id`;若 NULL/空 → 读 `app_config.default_model_id`
  2. 用 `db::list_models` 找到 model row;若仍 NULL → 返 `Error{InvalidRequest, "no model configured, please open Settings to set a default"}`
  3. 用 `model_row.provider_id` 读 provider row(join 在 list_models 时已有)
  4. 构造 `Box<dyn Provider>`,pre-flight check `provider_row.api_key.is_empty()` → 返 `Error{Auth, "请到 Settings 填 {provider_display_name} 的 api_key"}`
- 删 line 1116 `let config = state.config.clone()` 的 LLM 用途(若 `get_llm_config` 不再走 env,`state.config` 可保留为 fallback)
- 删 line 1125-1135 的 `config.is_unconfigured()` check(被 catalog pre-flight 取代)
- line 1360 `chat_stream_with_tools` → `provider.send(...)`

### 6. `get_llm_config` catalog 化

- 走 `db::get_config_value("default_model_id")` → `db::list_models` 找 row → 取 `model_name`(或 `display_name`,**待 Q1 决**)+ `provider.base_url`
- `configured = !provider.api_key.is_empty()`
- 若 default model 找不到 / provider 找不到 → `configured = false`、`model = ""`、`base_url = ""`(前端警告文案已支持)

### 7. env fallback 是否保留?

- `LlmConfig::from_env()` 的 env 读取是否还在 PR2 保留?
- **默认保留** (但不再被 `chat` 走,只作冷启动 catalog 未 seed 时的最后兜底)
- `state.config` 字段从 `LlmConfig` 改为 `Option<LlmConfig>`,None 表示 env 没设

## Acceptance Criteria

### 行为完全不变(PR2 唯一硬约束)

- [ ] chat 命令发出的 LLM 请求 URL = `provider_row.base_url + "/v1/messages"`(Anthropic 路径)
- [ ] chat 命令发出的 headers = `x-api-key: <provider.api_key>` + `anthropic-version: 2023-06-01`
- [ ] chat 命令的 `thinking` 字段总是 `{type: "adaptive", display: "summarized", effort: <model.thinking_effort || "high">}`
- [ ] 4 个 HACKING-llm 坑行为不变(GLM 兼容 / thinking 签名 / display summarized / orphan tool_use)
- [ ] agent loop 的 20 turn 行为不变(每 turn 用同 provider 实例)
- [ ] 已有 80+ 单元测试不动(从 client.rs 搬到 anthropic.rs,test 仍 pass)

### Provider trait + 工厂

- [ ] `provider/mod.rs` 定义 `Provider` trait / `ProviderCapabilities` / `ProviderProtocol` / `build_provider` 工厂
- [ ] `provider/anthropic.rs` 实现 `Provider` for `AnthropicProvider`
- [ ] `build_provider` 接受 `anthropic` 协议 → 返 `AnthropicProvider`
- [ ] `build_provider` 接受 `openai` 协议 → 返 `ProviderBuildError::NotImplemented("openai")`(PR3 占位)
- [ ] `build_provider` 接受未知协议 → 返 `ProviderBuildError::UnknownProtocol`

### catalog 解析

- [ ] `chat` 命令入口:session.model_id → 缺则用 `default_model_id` → catalog 查 → 构造 provider
- [ ] 解析失败(无 default / model 找不到)→ `Error{InvalidRequest}` 中文文案
- [ ] pre-flight(空 api_key)→ `Error{Auth}` 中文文案(带 provider display_name)
- [ ] `get_llm_config` 走 catalog,与前端契约 shape 一致

### 模块导出

- [ ] `app/src-tauri/src/llm/client.rs` 删除
- [ ] `app/src-tauri/src/llm/mod.rs` 改导出版本
- [ ] 所有外部 import 路径不变(`llm::chat_stream_with_tools` 改为 `llm::AnthropicProvider` 等)

### 单元测试

- [ ] `AnthropicProvider` 测试继承 client.rs 原 4 个测试(全 pass)
- [ ] `build_provider` 测试:Anthropic 协议 OK / OpenAI NotImplemented / 未知协议 UnknownProtocol
- [ ] `Provider` 工厂的 max_tokens / thinking_effort fallback 逻辑(2 case:有 model 行级配置 / 无 model 行级配置)
- [ ] `cargo test --lib` 全 pass,0 warning

### spec / docs

- [ ] `.trellis/spec/backend/llm-contract.md` 加 PR2 section(Scenario: Provider trait + Anthropic dispatch)
- [ ] `docs/HACKING-llm.md` 不动(4 个坑保留在 adapter 内部,文档位置不变)
- [ ] `docs/IMPLEMENTATION.md` §2.7 步骤 6 状态更新(PR1/PR2 完成,PR3 跟进)

## Definition of Done

- [ ] Tests added/updated(单元,继承 client.rs 4 个 + 新加 4-5 个)
- [ ] `cargo check` + `cargo test --lib` + `pnpm build`(vue-tsc + vite)全 pass,0 warning
- [ ] spec 加 PR2 section
- [ ] trellis-check 通过
- [ ] commit message:`feat(llm): PR2 Anthropic adapter (Provider trait + catalog dispatch)`

## Decision(ADR-lite)

### D1: `get_llm_config.model` 返 catalog `display_name`(2026-06-09)

**Context**: PR2 把 `get_llm_config` 从 env 路径切到 catalog 路径后,`model` 字段含义待定 — 返 `model_name` (发给 API 的字符串) 还是 `display_name` (UI 展示名)?

**Decision**: 返 `ModelRow.display_name`(如 "Claude Sonnet 4.5")。

**Consequences**:
- ✅ 跟 PR4 StatusBar dropdown 用 `display_name` 一致
- ✅ 前端 `useConfigStore.model` 直接作"已配置"显示,语义清楚
- ⚠️ 跟 env `LLM_MODEL` 行为略有差异(原本 "claude-sonnet-4-5",现在 "Claude Sonnet 4.5"),但前端 store 用途是"显示"非"调用",影响小
- ⚠️ PR4 StatusBar 实现时 dropdown 选项文本直接用 catalog 数据,不再依赖 env

### D2: Pre-flight 错误按场景分 3 种文案(2026-06-09)

**Context**: PR2 的 `chat` 命令要做 catalog-based pre-flight,可能失败 3 种情形(provider.api_key 空 / default 未设 / provider 不存在)。错误粒度决定前端 toast 文案 + PR4 modal 跳转路径。

**Decision**: 3 种场景各 1 句明确中文文案,带 `provider.display_name` / `model.display_name` 占位。

**Consequences**:
- ✅ PR4 Settings modal 的"跳到 Settings"按钮可直接根据 `category` + `message` 决定跳哪个 tab
- ✅ User 看到具体问题(填 api_key vs 选 default),不用猜
- ⚠️ 文案写死在 `lib.rs::chat` 里,未来 i18n 要提取(留 OOS)

## Out of Scope(明确不做)

- ❌ OpenAI adapter(PR3)
- ❌ 跨 protocol 消息转换(PR3)
- ❌ 跨 protocol capability-aware 自动降级(PR3)
- ❌ UI 改 model(PR4)
- ❌ 删 `state.config` 字段(保留 env fallback,即使 chat 不用)
- ❌ Pre-flight 跳 Settings modal 按钮(留 PR4)
- ❌ rig-core 迁移(3b-2 仍暂缓)

## Technical Notes

### 关键文件

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/llm/client.rs` | **删除**(内容搬到 anthropic.rs) |
| `app/src-tauri/src/llm/mod.rs` | 改 `pub mod` + re-export |
| `app/src-tauri/src/llm/provider/mod.rs` | 新建:Provider trait + 工厂 |
| `app/src-tauri/src/llm/provider/anthropic.rs` | 新建:AnthropicProvider impl(从 client.rs 搬) |
| `app/src-tauri/src/lib.rs:212` | `get_llm_config` 走 catalog |
| `app/src-tauri/src/lib.rs:1109-1365` | `chat` 命令 catalog 解析 + provider.send |
| `.trellis/spec/backend/llm-contract.md` | 加 PR2 section |
| `docs/IMPLEMENTATION.md` §2.7 | 状态更新 |

### 关联决策

- 引用 parent PRD §D2(自研 Provider trait,不上 rig-core)
- 引用 parent PRD §D3(Provider / Model 走 user-managed,SQLite 存)
- 引用 parent PRD §"PR2 — Anthropic adapter" acceptance criteria

### Anti-patterns(避免)

- ❌ 引入 rig-core(已决定自研)
- ❌ 引入 WireMessage 中间层(Anthropic-shaped types 已够用,中间层是 PR3 才需要)
- ❌ 改 `ChatRequest` / `ChatEvent` / `ContentBlock` 的 schema(向后兼容)
- ❌ 动前端(PR2 行为不变,前端零改动)
- ❌ 删 env 兜底(LlmConfig::from_env 仍保留作冷启动 fallback)
