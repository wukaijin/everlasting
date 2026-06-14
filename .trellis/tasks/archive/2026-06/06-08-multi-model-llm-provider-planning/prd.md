# multi-model 多 LLM provider 切换规划

> Task: 06-08-multi-model-llm-provider-planning
> Status: planning (brainstorm 阶段)
> 最后更新: 2026-06-08

## Goal

补全项目里多 model / provider 切换的规划文档(目前散落在 `IMPLEMENTATION §2.7 步骤 6` / `DESIGN §3` checkbox / `TECH.md` 一句选型 / `HACKING-llm.md` GLM 差异笔记,**无独立规划**),把 A/B/C 哪一档范围、per-session 持久化方案、跟 rig-core 迁移(3b-2 暂缓)的依赖关系、UI 切模型入口,落到可执行的 prd + 后续 PR 切片。

## What I already know(从 repo + docs 探查)

### 现状:已有的散点

- **`IMPLEMENTATION.md §2.7 步骤 6`**: "MCP 暴露 + 多 Provider [v1] 未开始",列了 "加 OpenAI provider 切换" + "加 Ollama provider 切换(纯本地,省钱)" — 范围跟 MCP 捆绑,没独立规划
- **`DESIGN.md §3`**: 一个 checkbox "多 LLM provider 切换(Anthropic / OpenAI / 本地 Ollama)"
- **`TECH.md §1`**: rig-core 0.38.1 已锁,理由含 "20+ provider,后期切 OpenAI / 本地模型无痛" — 技术选型已定但**没用上**
- **`HACKING-llm.md`**: GLM-4.7(`<your-proxy>` 转发的 Anthropic 兼容层)vs 真 Anthropic Claude 的 3 处差异(401 error.type / 400 类错返 5xx / max_tokens 不严格)+ ping 心跳 + extended thinking 兼容层 — **真踩过**的坑,价值最高
- **`BACKLOG.md §4 多角色`**: role-level model 偏好 (`role.model.preferred / fallback`),v3+ 远期
- **`ARCHITECTURE.md §2.5.7`**: LLM Provider 限流的占位描述(假设多 provider 已存在)

### 现状:代码层

- **`app/src-tauri/src/llm/client.rs`**: `LlmConfig` 只有 `base_url` / `model` / `api_key` / `max_tokens` / `thinking_effort` 5 个字段;**无 provider 枚举**;`endpoint()` 硬编码 `format!("{}/v1/messages", ...)`;headers 硬编码 `x-api-key` + `anthropic-version`;request body 整个是 Anthropic Messages schema
- **`app/src-tauri/src/db.rs:256`**: `sessions` 表**已有** `model TEXT NOT NULL` 列(`create_session` 时填 env `LLM_MODEL`);**schema 上 per-session 持久化已就绪**,只是没 UI 改
- **`app/src/stores/config.ts`**: 全局 `model` / `baseUrl`,只从 `get_llm_config` IPC 拉一次;**无 per-session 切模型入口**
- **`.trellis/spec/backend/llm-contract.md:19-20`**: 关键约束原话 — "If at any point we switch to OpenAI-compat, the `reasoning_content` field replaces the `thinking` block entirely; that change would happen here, not in the UI." — 即多 provider 协议层变化被认为是 client 内部的事,UI 不感知 provider 差异
- **工具调用 / content block 全部 Anthropic-shaped**:`ContentBlock` enum(text / tool_use / tool_result / thinking / redacted_thinking)按 Anthropic serde tag 来

### 现状:已知 "多 model" 含义的 3 档

| 档位 | 范围 | 工作量 | 现状 |
|---|---|---|---|
| **A. 同 provider 多 model** | opus/sonnet/haiku 切换 | 极小 | 几乎能跑,缺 UI 入口 + per-session 持久化 |
| **B. 多 provider 但都走 Anthropic 兼容协议** | Anthropic 官方 / `<your-proxy>` 转发 / 第三方 Anthropic-compat | 小 | 几乎能跑,缺 `LlmConfig` 加 `provider` 字段做命名区分 + per-session 选 |
| **C. 真·多 provider 协议** | Anthropic / OpenAI / Ollama / Gemini 等不同 wire format | 大 | **要重写 `client.rs` 的请求/响应/SSE/tool 块映射**,rig-core 抽象或自写 adapter 二选一 |

## Assumptions(临时假设,待验证)

- 假设 1:用户说的"多模型"主要指 A + B 档(C 档 scope 大,需要单独决策)
- 假设 2:per-session model 优先级高于全局 model(因为 schema 已经预留 `model` 列,前端用 config store 全局覆盖只是临时状态)
- 假设 3:不动 rig-core 迁移顺序(3b-2 仍暂缓),多 model 走自研 adapter 或直接硬分叉,不依赖 rig-core 上线
- 假设 4:UI 切模型入口在 session 创建/编辑时(跟 model 一起作为 session metadata),不引入"运行中切 model"(后者是更大的 UX 决策)
- 假设 5:不做"运行中切 model"——已有 thinking / tool_use 流的 session 中途切 model 会产生协议不一致,需要持久化 reset;推到未来

## Open Questions

- [x] **Q1:范围档位** — **C 档,但 MVP 只 2 个 provider**(2026-06-08 决议)
  - Anthropic Messages API(已有,refactor 适配)
  - OpenAI Chat Completions API(新加)
  - 暂不做 Ollama / Gemini / Mistral / Cohere(留给未来)
- [x] **Q2:Provider 抽象层** — **P1 自研 `Provider` trait**(2 个 provider 不值得上 rig-core)(2026-06-08 决议)
  - 现有 4 个 HACKING-llm 坑(GLM 兼容 / thinking / extended thinking / redacted_thinking)留 Anthropic adapter 内部处理
  - Provider 概念 = `{ protocol: Anthropic | OpenAI, base_url: String, api_key: String }` — 极轻
- [x] **Q3:Provider / Model 形态** — **user-managed,SQLite 存,嵌套结构**(2026-06-08 决议)
  - Providers 列表 — user 可加/改/删,**多个同 protocol 的 provider 可共存**(e.g., Anthropic 官方 + `<your-proxy>` 转发)
  - Models 列表,绑到某个 Provider(provider_id 外键)
  - Default Model(单选,全局)— 新 session 默认用
  - Session 启动:Default Model;运行中:可"自由切换"到 catalog 任何 model
  - 未来 Agent / Role 复用同一套 catalog(绑默认 model)
- [x] **Q4:Model profile 字段** — **F3 减 `supports_tools` + `context_window`+`max_tokens` 双字段**(2026-06-08 决议)
  - `model_name: String`(必填,发到 API)
  - `display_name: String`(必填,UI 显示)
  - `max_tokens: Option<u32>`(可选,fallback 全局)— 单次输出上限(协议字段)
  - `thinking_effort: Option<String>`(可选,fallback 全局)— 仅 `supports_thinking=true` 时生效
  - `capabilities: { supports_thinking: bool, context_window: u32 }` — 不含 `supports_tools`
  - `context_window` = 总容量(input + output),给 token 预算用
  - `max_tokens` = 单次输出上限,给请求用(两个并存不冗余)
- [x] **Q5:per-session 切 model 历史处理** — **H1 历史全保留 + 跨 protocol capability 自动降级**(2026-06-08 决议)
  - 切 model 只改 `sessions.model_id`,下一轮 turn 把 messages 完整发新 model
  - 跨 protocol(Anthropic ↔ OpenAI)切时,按新 model 的 `capabilities` 自动剥不兼容块:
    - 新 model `supports_thinking=false` → 剥 thinking / redacted_thinking 块(降级为 text 摘要或丢弃)
    - signature / redacted_thinking.data 这种 opaque blob 直接丢(OpenAI 协议无对应字段)
  - 不弹 dialog,静默降级(用户切 model 时本就要接受 trade-off)
- [x] **Q6:UI 结构** — **S1(modal 弹窗) + B1-mod(StatusBar 双端:左 settings / 右 model dropdown)**(2026-06-08 决议)
  - **Settings(S1)**:reka-ui Dialog 居中弹窗,tabs「Providers」/「Models」/「Default」
  - **StatusBar 改造**(截图确认布局,2026-06-08 23:21):
    - **左下(原 `(no model)` 警告位)= 齿轮图标**(settings 入口)
    - **右下(原 `ANTHROPIC_API_KEY 未设置` 警告位)= model select dropdown**(provider-grouped 选项)
    - 警告 banner 仍保留,但位置让出(具体怎么放待 PR4 实施时定,可放 modal 弹窗时一次性提示,或挪到 chat panel 顶部)
  - **ChatPanel header 改造**:
    - **删除** `ChatPanel.vue:378-381` `chat-panel__chip` model tag(header 不再显示 model 名字,避免重复)
    - 留 git branch / cwd / worktree 那些 chip
  - **理由**:左下放 settings 符合"配置类操作放输入区附近"的常规 UX(用户改完配置就能继续输入);右下放 model select 是"高频切换"放顺手位(右手位)。两边职责不重叠。
- [x] **Q7:首次安装 seeding** — **D3 启动 seed 2 provider + 常见 model + default**(2026-06-08 决议)
  - `lib.rs` 启动时查 `providers` 表,**0 行就 seed**:
    - Provider 1:`display_name="Anthropic 官方"`, `protocol=Anthropic`, `base_url=https://api.anthropic.com`, `api_key=""`(留空)
    - Provider 2:`display_name="OpenAI 官方"`, `protocol=OpenAI`, `base_url=https://api.openai.com/v1`, `api_key=""`(留空)
  - 跟着 seed 几个常见 model(绑到对应 provider):
    - Anthropic:`claude-sonnet-4-5`(`context_window=200000`, `supports_thinking=true`),`claude-opus-4-7`(`context_window=200000`, `supports_thinking=true`)
    - OpenAI:`gpt-4o`(`context_window=128000`, `supports_thinking=false`),`gpt-4.1`(`context_window=1000000`, `supports_thinking=false`)
  - `default_model_id` 指向 `claude-sonnet-4-5`(兼容当前 Anthropic 用户)
  - api_key 留空 → 启动后 StatusBar 仍出 "ANTHROPIC_API_KEY 未设置" 类警告(若改字段名,记得同步警告文案)
- [x] **Q8:Provider 失败 / 切换时的行为** — **F2 + 测试按钮才能保存**(2026-06-08 决议)
  - **Pre-flight check(必做)**:发消息前查 `provider.api_key` 空 / `model_id` 链断裂 → 立即返 `PreFlightError`,UI 弹 toast「请到 Settings 填 XXX 的 api_key」+ 跳 Settings 按钮
  - 401/5xx 仍走 LLM 错误路径(沿用现有 `LlmError` 5 类)
  - **StatusBar 仍只显示二元状态**(已配置 / 未配置),不引入 per-provider 健康度(留未来)
  - **测试按钮(必做)**:Settings > Providers > Edit 表单里 `api_key` 字段右侧加「Test」按钮
    - 测试逻辑:用当前表单的 base_url + api_key 发一个最小请求(provider 自检方法,Anthropic 走 `POST /v1/messages` with `max_tokens=1` + 空消息或最小有效 messages 触发 server-side validation;OpenAI 走 `GET /v1/models` 或类似轻量 endpoint)
    - 测试通过:`Save` 按钮 enable;返回响应时间 + 协议版本
    - 测试失败:显示具体错误(401 / 网络 / 协议版本不对),`Save` 保持 disable
    - **强制**:未通过测试的 provider 不能保存(避免下次发消息才发现 api_key 是错的)
- [x] **Q9:首发 PR 切片** — **K1 4 PR,数据→协议→UI**(2026-06-08 决议)
  - **PR1 — 数据层**:`providers` / `models` / `app_config` 三表 + 迁移 + 7-8 个 CRUD IPC + 启动 seed(0 行就插 2 provider + 4 model + default)+ 重构 `sessions.model` 引用。**不接 `chat` 命令,不动 LLM 客户端,纯数据**。
  - **PR2 — Anthropic adapter**:把现有 `client.rs` 拆出 `Provider` trait,Anthropic adapter 复用现有代码(4 个 HACKING-llm 坑保留在 adapter 内部)。`chat` 命令改用 `provider.send(req)`。**OpenAI 还没接,所有 session 走 Anthropic**(行为跟现在一致)。
  - **PR3 — OpenAI adapter + 跨协议**:新增 `OpenAI` adapter + 跨 protocol 消息转换(Q5 决定的 H1 capability-aware 降级)。OpenAI Chat Completions 协议(SSE + tool_calls + reasoning_content)。
  - **PR4 — UI**:Settings modal(reka-ui Dialog,Providers / Models / Default 三 tab,带 Test 按钮)+ StatusBar model dropdown + 删 `ChatPanel` model chip + store 重构。

## Requirements(2026-06-08 部分收敛)

### 全局 Settings(新增,SQLite 新表)

- **Provider 表**:`providers(id, protocol, base_url, api_key, display_name, created_at, updated_at)` — user-managed
- **Model 表**:`models(id, provider_id FK, model_name, display_name, [可选:max_tokens, thinking_effort, capabilities], created_at, updated_at)` — user-managed
- **AppConfig 表**(已有或新):`default_model_id` 单值

### Session(已有,sessions 表 model 列扩展)

- `sessions.model_id` 由 `TEXT` 改为 `model_id TEXT REFERENCES models(id)` — per-session model 持久化已就绪
- Session 启动:从 `default_model_id` 取
- Session 切换 model:更新 `sessions.model_id`

### 未来预留

- `roles / agents` 表(尚未建,跟 BACKLOG §4 多角色衔接)bind `default_model_id`
- Protocol 枚举预留扩展(目前只 Anthropic + OpenAI)

## Acceptance Criteria(2026-06-08 收敛)

### PR1 — 数据层

- [ ] SQLite 加 3 张表 `providers` / `models` / `app_config`,迁移脚本兼容已有 `sessions.model` 列(用 `model_id` 替换,旧值转新 model 行)
- [ ] 7-8 个 CRUD IPC:`list_providers` / `add_provider` / `update_provider` / `delete_provider`(同 4 个 for models)+ `get_default_model_id` / `set_default_model_id`
- [ ] `lib.rs` 启动时:`providers` 表 0 行 → seed 2 行(Anthropic 官方 / OpenAI 官方,api_key 留空)+ 4 行 models + 1 行 `app_config` 指向 `claude-sonnet-4-5`
- [ ] `sessions.model` 列从 `TEXT` 改为 `model_id TEXT REFERENCES models(id)`,**只读路径走新表**(写入仍用 model_name string,内部 join 解析到 id)
- [ ] 单元测试:CRUD IPC 每个 1 个 happy path + 1 个 error path(共 ~16 个 test)
- [ ] `pnpm tauri build` 通过,cargo test 通过
- [ ] **不改任何 UI**,不改 LLM 客户端

### PR2 — Anthropic adapter

- [ ] 新增 `app/src-tauri/src/llm/provider/mod.rs`,定义 `trait Provider { fn send(&self, req: ChatRequest) -> Stream<ChatEvent>; fn capabilities() -> ProviderCapabilities; fn protocol() -> Protocol; }`
- [ ] 新增 `app/src-tauri/src/llm/provider/anthropic.rs`,把现有 `client.rs` 全部逻辑搬过来,作为 `AnthropicProvider` impl
- [ ] 现有 `client.rs` 删掉(或改为 `pub use provider::anthropic::*`)
- [ ] 4 个 HACKING-llm 坑(GLM 兼容 / thinking / extended thinking / redacted_thinking / orphan tool_use)保留在 Anthropic adapter 内部,行为不变
- [ ] `lib.rs` 的 `chat` 命令改用 `provider.send(req)`,**Provider 实例从 `app_config.default_model_id` 解析到 `models` 行再加载**(这一层是新加的解析逻辑)
- [ ] 单元测试:Anthropic adapter 行为跟搬之前完全一致(spike-002 的 mock 测试,搬到新位置)
- [ ] `pnpm tauri build` 通过,cargo test 通过
- [ ] **行为完全不变**,只动内部架构

### PR3 — OpenAI adapter + 跨协议

- [ ] 新增 `app/src-tauri/src/llm/provider/openai.rs`,实现 `Provider` trait
- [ ] 协议层:OpenAI Chat Completions streaming(`/v1/chat/completions`,SSE `data: {...}\n\n`,event 是 JSON 里的 `choices[].delta`)
- [ ] tool_calls 映射:OpenAI `function_call` / `tool_calls` ↔ Anthropic `tool_use` / `tool_result`(中间用 provider-agnostic `WireMessage` 表示)
- [ ] reasoning_content 映射(OpenAI `o1` / `o3` 系列):WireMessage 加 `Reasoning` block,Anthropic 不支持 → 转 Anthropic `thinking` block(若 target supports_thinking)或剥掉(否则)
- [ ] 跨协议 capability-aware 转换:发请求前查 `target_model.capabilities`,自动剥不兼容块(thinking 块 → text 摘要 / 丢 signature)
- [ ] 单元测试:OpenAI adapter mock 单测 + 跨协议转换 4-6 个 case
- [ ] `pnpm tauri build` 通过,cargo test 通过

### PR4 — UI

- [ ] 新增 `app/src/components/SettingsModal.vue`,reka-ui Dialog,tabs「Providers」/「Models」/「Default」
- [ ] Providers tab:list + Add / Edit / Delete + Test 按钮 + Save 按钮(未 Test 通过 disable)
- [ ] Models tab:list(按 provider 分组)+ Add / Edit / Delete,字段包含 `model_name` / `display_name` / `max_tokens` / `thinking_effort` / `context_window` / `supports_thinking`
- [ ] Default tab:radio 单选,从 models 列表选
- [ ] 改 `StatusBar.vue`:**左下**放齿轮图标(settings 入口,点开 Settings modal);**右下**放 model select dropdown(provider-grouped 选项,选中即切 `sessions.model_id`)
- [ ] 删除 `ChatPanel.vue:378-381` model chip
- [ ] 新建 / 迁移 `useProvidersStore` / `useModelsStore`,`useConfigStore` 重构保留 `default_model_id`
- [ ] vitest:SettingsModal 行为测试 + StatusBar dropdown 测试 + store 迁移测试
- [ ] `pnpm tauri build` 通过

### Definition of Done(团队质量基线)

- [ ] Tests added/updated(单元/集成)
- [ ] Lint / typecheck / CI green
- [ ] 文档/笔记更新:`docs/IMPLEMENTATION.md`(路线图把多 provider 从 §2.7 拆出,新加步骤)/ `docs/HACKING-llm.md`(加 OpenAI 差异章节)/ `docs/BACKLOG.md`(§4 v3+ 多角色 bind default model 改成引用本任务)/ `.trellis/spec/backend/llm-contract.md`(多 provider 协议)
- [ ] Rollout/rollback 风险评估(写到 `prd.md` "Rollout" 节)

## Out of Scope(2026-06-08 部分收敛)

- Ollama / Gemini / Mistral / Cohere 等其他 provider(留未来)
- Provider API 自动发现 `/v1/models`(不接 catalog)
- 自动按 cost / latency 选 provider(纯手动)
- 多 agent 编排(留 BACKLOG §4)

## Technical Notes

### 关键文件

- `app/src-tauri/src/llm/client.rs` — 当前 Anthropic-shaped 客户端
- `app/src-tauri/src/llm/types.rs` — `LlmConfig` / `ChatRequest` / `ContentBlock` / `ChatEvent`
- `app/src-tauri/src/llm/sse.rs` — SSE 解析
- `app/src-tauri/src/llm/error.rs` — 5 类错误归一化
- `app/src-tauri/src/db.rs:256,791,808,886,904` — `sessions.model` 列 + 读写
- `app/src/stores/config.ts` — 全局 `model` / `baseUrl`
- `app/src/stores/chat.ts:333` — 引用 `model: string` 字段
- `.trellis/spec/backend/llm-contract.md` — LLM API 契约(Anthropic-shaped,1397 行)
- `docs/HACKING-llm.md` — 3 处 GLM 差异 + extended thinking 兼容层

### 关联决策

- 步骤 3b-2 暂缓里提到 "完整三栏 UI + rig-core 迁移" — rig-core 跟多 provider 抽象有重叠(它自带 20+ provider adapter)
- 步骤 6 (IMPLEMENTATION §2.7) 把多 Provider 跟 MCP 捆绑,本任务若做出拆分,需要更新路线图

## Decision(ADR-lite)

### D1:C 档 MVP 只做 2 个 provider(Anthropic + OpenAI)

**Context**:用户最初说"多模型",范围可大可小(A/B/C 三档)。C 档(真·多 provider 协议)涉及不同 wire format 适配,触及 `client.rs` / `types.rs` / `sse.rs`。

**Decision**:做 C 档,但 MVP 只 2 个 provider(Anthropic 已有,OpenAI 新加),其他(Ollama / Gemini / Mistral / Cohere)留未来。

**Consequences**:
- ✅ 工作量收敛到 5+ PR(可管理)
- ✅ Anthropic 现有 4 个 HACKING-llm 坑可以无改动保留(adapter 内部处理)
- ✅ OpenAI 协议层一次写完,后续加协议只是新 impl
- ⚠️ 未来加 Gemini / Ollama 时,要再加一段"跨协议消息转换"的 rule(目前只覆盖 Anthropic ↔ OpenAI)
- ⚠️ 抽象层是为"2 个 provider"设计的,如果未来 provider 数 > 5,要重新评估 rig-core

### D2:自研 `Provider` trait,不上 rig-core

**Context**:TECH.md 早就锁定 rig-core 作 LLM 抽象层,但 3b-2(rig-core 迁移)暂缓,本任务又出现"多 provider 抽象"需求。

**Decision**:本任务自研 `Provider` trait。rig-core 仍是 TECH.md 锁定的远期选型,本任务不依赖。

**Consequences**:
- ✅ 跟当前 `LlmConfig` / `client.rs` 风格一致,reviewer 不用学 rig-core 概念
- ✅ 4 个 HACKING-llm 坑直接搬进 Anthropic adapter
- ✅ 2 个 provider 数量上,自研 trait 比 rig-core wrapper 简单
- ⚠️ 未来加 provider 时要写 adapter(没有 rig-core 的 20+ 现成)
- ⚠️ 跟 TECH.md 的 rig-core 锁定有表面冲突 — 需要在 TECH.md 标注"多 provider 抽象本任务先自研,rig-core 迁移待定"

### D3:Provider / Model 走 user-managed,SQLite 存

**Context**:用户希望"user 新建可以 provider,在 provider 内可以新建模型"。

**Decision**:SQLite 加 `providers` / `models` / `app_config` 三表,user 可加/改/删,**多个同 protocol 的 provider 可共存**(e.g., Anthropic 官方 + `<your-proxy>` 转发)。

**Consequences**:
- ✅ 跟"user-managed"原则一致
- ✅ 数据可移植(跟着 SQLite 走,不用单独 config 文件)
- ✅ 未来多 agent / role 复用同一套 catalog(预留 `roles.default_model_id`)
- ⚠️ 启动时要 seed(否则新 install 看不到任何 provider)
- ⚠️ 多 provider 同 protocol 共存,UI 要做区分(下拉分组)

### D4:Pre-flight check + 测试按钮(不弹 dialog)

**Context**:用户切到 api_key 为空的 model 时,UX 应该立即反馈,而不是发请求后才报 401。

**Decision**:F2(pre-flight check)+ 测试按钮(强制 Save 前 Test 通过)。不引入 per-provider 健康度(留未来)。

**Consequences**:
- ✅ 配置类错误不用 round-trip 才发现
- ✅ 用户在 Settings 填 api_key 时就能验证,避免"我以为填对了,发请求才知道是错的"
- ⚠️ Pre-flight 要在 `chat` 命令开头加一段查询,小开销(<5ms SQLite)
- ⚠️ Test 按钮的"最小请求"各家 protocol 不一样,Anthropic 走 `POST /v1/messages` + max_tokens=1,OpenAI 走 `GET /v1/models` — 这两个 endpoint 都算便宜

### D5:跨 protocol capability-aware 自动降级,不弹 dialog

**Context**:用户切到不支持 thinking 的 model(GPT-4o)时,历史 thinking 块如何处理?

**Decision**:静默降级。新 model 不支持的 block 类型按规则剥掉,不弹 dialog 询问。

**Consequences**:
- ✅ 切 model 时零摩擦
- ✅ 用户切 model 时本就要接受 trade-off,弹 dialog 反而打断
- ⚠️ "丢 signature" 是静默的,用户可能没意识到"之前的思考痕迹没了"
- ⚠️ 降级规则写死在 adapter 层,未来改规则要改代码(不是配置)

## Out of Scope(2026-06-08 收敛)

- Ollama / Gemini / Mistral / Cohere 等其他 provider(留未来)
- Provider API 自动发现 `/v1/models`(不接 catalog,user 手动加)
- 自动按 cost / latency 选 provider(纯手动)
- 多 agent 编排(留 BACKLOG §4)
- rig-core 迁移(留 3b-2 暂缓状态)
- per-provider 健康度状态机(留未来)
- Vue Router / 新增 route(本任务不引入)

## Research References

_暂无 research/ 产出_
