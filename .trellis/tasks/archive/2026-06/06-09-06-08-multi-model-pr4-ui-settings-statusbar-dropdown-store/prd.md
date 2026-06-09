# 06-08 multi-model PR4: UI Settings + StatusBar dropdown + store 重构

> Task: `06-09-06-08-multi-model-pr4-ui-settings-statusbar-dropdown-store`
> Status: planning (brainstorm → converged)
> Parent task: `06-08-multi-model-llm-provider-planning` (PR 切片 K1)
> 基线分支: `06-08-multi-model-llm-provider-planning-pr1-data-layer` (PR1 `f9c5648` + PR2 `0a787ef` + PR3 `9395418`)

## Goal

PR4 是 multi-model 多 LLM provider 切换的最后一个 PR，负责 **UI 层**：
新增 Settings modal（Providers / Models / Default 三 tab + Test 按钮），
改造 StatusBar（左下齿轮 → settings 入口，右下 model dropdown → per-session 切模型），
删除 ChatPanel header 的 model chip，
新建 `useProvidersStore` / `useModelsStore` 并重构 `useConfigStore`。

完成后，用户可以在 GUI 中管理 provider / model，切换默认模型和 per-session 模型，
无需编辑环境变量。

## What I already know

### 代码现状（从前端探索确认）

- **`config.ts`**: 扁平 `model` / `baseUrl` / `configured`，从 `get_llm_config` IPC 一次性读取；`loaded` ref 控制渲染前等待
- **`StatusBar.vue`**: 单行，左侧 `dot + model + sep + url`，`!configured` 时显示 `ANTHROPIC_API_KEY 未设置` 警告
- **`ChatPanel.vue:378-381`**: 静态 model chip（`configStore.model`），只读，无交互
- **`chat.ts`**: `create_session` 返回 `model: string`；`send()` 转发到 `streamController`，无 model 切换逻辑
- **`reka-ui`**: 已安装 `^1.0.0-alpha.10`，**源码中零使用** — 需升级到 2.x stable
- **不存在** `useProvidersStore` / `useModelsStore`
- **stores/**: 只有 `config.ts` / `chat.ts` / `projects.ts` / `streamController.ts` / `streamController.test.ts`

### 后端 IPC（PR1 已落地，10 个命令）

| IPC 命令 | Args | 返回 |
|---|---|---|
| `list_providers` | — | `Vec<ProviderRow>` |
| `add_provider` | `protocol, displayName, baseUrl, apiKey` | `ProviderRow` |
| `update_provider` | `id, protocol, displayName, baseUrl, apiKey` | `Option<ProviderRow>` |
| `delete_provider` | `id` | `bool` |
| `list_models` | — | `Vec<ModelWithProvider>` |
| `add_model` | `providerId, modelName, displayName, maxTokens?, thinkingEffort?, supportsThinking, contextWindow` | `ModelRow` |
| `update_model` | `id, providerId, modelName, displayName, maxTokens?, thinkingEffort?, supportsThinking, contextWindow` | `Option<ModelRow>` |
| `delete_model` | `id` | `bool` |
| `get_default_model` | — | `Option<ModelWithProvider>` |
| `set_default_model` | `modelId` | `()` |

### 后端 Pre-flight（PR2 已落地）

`chat` 命令入口有 3 种 pre-flight 错误：
- `api_key` 空 → Auth error
- `model` 找不到 → InvalidRequest
- `provider` 找不到 → InvalidRequest

前端已能收到这些错误（通过 `ChatEvent::Error`），PR4 需要**在 UI 入口处提前拦截**（toast + 跳 Settings）。

### Wire shape（camelCase，`#[serde(rename_all = "camelCase")]`）

```typescript
interface ProviderRow {
  id: string; protocol: string; displayName: string;
  baseUrl: string; apiKey: string;
  createdAt: string; updatedAt: string;
}

interface ModelWithProvider {
  id: string; providerId: string; modelName: string; displayName: string;
  maxTokens: number | null; thinkingEffort: string | null;
  supportsThinking: boolean; contextWindow: number;
  createdAt: string; updatedAt: string;
  providerDisplayName: string; providerProtocol: string;
}
```

### Parent PRD 已锁定的决策（K1 决议）

- **Q6**: Settings S1 modal + StatusBar B1-mod 双端布局（左下齿轮，右下 model dropdown）
- **Q7**: 启动 seed 2 provider + 4 model + default（PR1 已做）
- **Q8**: Provider Test 按钮 + Save 强制 Test 通过 + Pre-flight check
- **Q5**: 切 model 静默降级，不弹 dialog（后端 PR3 已做）
- **D4**: Pre-flight + 测试按钮，不做 per-provider 健康度

## Decisions (本轮 brainstorm 收敛)

### D1: StatusBar model dropdown — Provider 分组样式

**Context**: Parent PRD Q6 确定"右下 model dropdown"，但具体交互方式有多种。

**Decision**: `<optgroup>` 分组样式。每组是 provider display name 作为不可选标题，下面是该 provider 的 models。与 `<select>` 原生 `<optgroup>` 行为一致。

**Consequences**: 信息密度高，2 个 provider 各自分组清晰。reka-ui Select 组件支持 group。

### D2: 切 model 时机 — 选中立即生效

**Context**: 选中 model 后何时更新 `sessions.model_id`。

**Decision**: dropdown 选中后立即调 IPC `update_session` 更新 `sessions.model_id`。Streaming 时 dropdown disable。用户切完 model 立刻在 StatusBar 看到变化。

**Consequences**: 需要确认 `update_session` IPC 是否已存在（或需要用其他方式更新 model_id）。如果后端没有独立的 `update_session_model` IPC，可能需要新增一个轻量 IPC 或用 `update_session`。

### D3: API Key 安全 — 掩码 + 点击展开

**Context**: Settings 编辑表单中 api_key 的显示。

**Decision**: 默认显示 `sk-ant-****` 掩码，点击眼睛图标展开全文。IPC 返回全量 api_key（Tauri 本地沙箱保护）。

**Consequences**: 需要一个简单的 `ApiKeyInput.vue` 子组件或在表单内 inline 实现掩码逻辑。

### D4: SettingsModal 组件拆分 — 按 tab 拆子组件

**Context**: SettingsModal 代码量较大，单文件 500+ 行不利于维护。

**Decision**:
```
app/src/components/settings/
├── SettingsModal.vue    (shell + reka-ui Dialog + Tabs)
├── ProvidersTab.vue     (list + Add/Edit/Delete + Test 按钮)
├── ModelsTab.vue        (list grouped by provider + Add/Edit/Delete)
└── DefaultTab.vue       (radio select)
```

**Consequences**: 每个 tab 组件约 100-200 行，职责清晰。

### D5: useConfigStore 重构 — 删掉旧字段，统一从 catalog 派生

**Context**: `config.ts` 的 `model` / `baseUrl` / `configured` 是 env 路径遗留。

**Decision**: 删掉 `model` / `baseUrl` / `configured`。`useConfigStore` 只保留 `homeDir` / `lastActiveProjectId` / `loaded`。所有 model 信息统一从 `useModelsStore` 派生。

**Consequences**: 现有引用 `configStore.model` 的地方（StatusBar / ChatPanel）都需改为从 `modelsStore` 读取。`get_llm_config` IPC 调用可删除，替换为 `list_models` + `get_default_model`。

### D6: 空 catalog 状态 — 文案提示 + 引导去 Settings

**Context**: 没有 model 可用时 StatusBar 右侧如何显示。

**Decision**: 右侧显示 `"(未选择模型)"` 灰色文案 + 点击打开 Settings。dropdown 打开后如果 models 列表为空，显示 "请在 Settings 中添加模型"。

### D7: reka-ui 升级到 2.x stable

**Context**: 当前安装 `^1.0.0-alpha.10`，最新稳定版 `2.9.9`。API 可能有 breaking changes。

**Decision**: 开发前先升级 `reka-ui` 到最新 2.x stable，确认 Dialog / Tabs / Select / RadioGroup 组件可用。

**Consequences**: 需要检查 reka-ui 2.x 的 API 变更（import 路径、组件名称、props 等）。由于源码中零使用，升级零风险。

## Requirements

### 前置：升级 reka-ui

- `pnpm add reka-ui@latest`（从 alpha.10 → 2.x stable）
- 确认 `pnpm build` 通过（无源码引用，零影响）

### 后端新增（PR4 需要填补的空缺）

探索确认以下 3 个后端空缺需要填补：

**1. `update_session_model_id` IPC（新增）**
- `sessions.model_id` 列已存在（PR1 schema），但**没有任何 IPC 或 DB 函数可以更新它**
- 需要：`db.rs` 新增 `update_session_model_id(pool, session_id, model_id)` + `lib.rs` 注册 IPC
- 签名：`async fn update_session_model_id(state, session_id: String, model_id: String) -> Result<(), String>`
- SQL：`UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?`

**2. `test_provider` IPC（新增）**
- Provider Test 按钮需要后端验证 api_key
- 签名：`async fn test_provider(state, base_url: String, api_key: String, protocol: String) -> Result<TestResult, String>`
- `TestResult { success: bool, latency_ms: u64, error: Option<String> }`
- Anthropic: `POST /v1/messages` with `max_tokens=1` + minimal messages → 检查 200/401
- OpenAI: `GET /v1/models` with `Authorization: Bearer <api_key>` → 检查 200/401

**3. `load_session` 补充返回 `model_id`（修改）**
- 当前 `load_session` SQL 只 SELECT `model`（老字段），不包含 `model_id`
- 需要补充 SELECT `model_id` 并在返回结构中携带

### Store 层

- **新建 `app/src/stores/providers.ts`**: `useProvidersStore`
  - `providers: ref<ProviderRow[]>([])`
  - `async load()` → `invoke("list_providers")`
  - `async add(...)` → `invoke("add_provider", ...)`
  - `async update(...)` → `invoke("update_provider", ...)`
  - `async remove(id)` → `invoke("delete_provider", { id })`

- **新建 `app/src/stores/models.ts`**: `useModelsStore`
  - `models: ref<ModelWithProvider[]>([])`
  - `defaultModel: computed<ModelWithProvider | null>`
  - `async load()` → `invoke("list_models")` + `invoke("get_default_model")`
  - `async add(...)` → `invoke("add_model", ...)`
  - `async update(...)` → `invoke("update_model", ...)`
  - `async remove(id)` → `invoke("delete_model", { id })`
  - `async setDefault(modelId)` → `invoke("set_default_model", { modelId })`
  - Helper: `modelById(id)`, `modelsByProvider(providerId)`, `modelsGroupedByProvider: computed`

- **重构 `app/src/stores/config.ts`**: `useConfigStore`
  - 删除: `model`, `baseUrl`, `configured`
  - 保留: `homeDir`, `lastActiveProjectId`, `loaded`
  - `load()` 改为调用 providers + models stores 的 load，不再调 `get_llm_config`

### 组件层

- **新建 `app/src/components/settings/` 目录**:
  - `SettingsModal.vue`: reka-ui DialogRoot + DialogContent，居中弹窗，宽度 640px，高度自适应
  - `ProvidersTab.vue`: provider 列表 + Add/Edit inline form + Delete confirm + Test 按钮 + Save（Test 未通过 disable）
  - `ModelsTab.vue`: model 列表（按 provider 分组显示）+ Add/Edit form + Delete confirm
  - `DefaultTab.vue`: reka-ui RadioGroup，从 models 列表选 default model

- **改造 `app/src/components/layout/StatusBar.vue`**:
  - 左侧: 齿轮图标（点击 → emit `open-settings` 或直接 toggle SettingsModal state）
  - 右侧: reka-ui Select，provider-grouped options，选中立即更新 `sessions.model_id`
  - 空 catalog: 显示 `"(未选择模型)"` 灰色文案，点击跳 Settings
  - Streaming 时 dropdown disable

- **修改 `app/src/components/chat/ChatPanel.vue`**:
  - 删除 L378-381 model chip（`configStore.model` 相关）

- **修改 `app/src/stores/chat.ts`**:
  - 引用 `configStore.model` 的地方改为从 `modelsStore` 读取
  - 新增 per-session model 切换逻辑（dropdown → `update_session_model_id` IPC）

### SettingsModal 详情

#### Providers Tab
- 列表: provider display name + protocol badge + base_url（api_key 掩码）
- Add: 展开表单（protocol select / display_name / base_url / api_key）
- Edit: 同 Add 表单，预填当前值
- Delete: confirm dialog
- Test 按钮: 点击调 provider 的 test endpoint（目前后端无 test IPC，**需要新增** 或用轻量 API 调用模拟）
  - 测试通过: 显示 ✓ + 响应时间
  - 测试失败: 显示具体错误
  - Save 按钮: 未 Test 通过时 disable
- **注意**: PR1 后端无 `test_provider` IPC。有两种方案:
  - 方案 A: 新增 `test_provider` IPC（Rust 侧发轻量请求验证 api_key）
  - 方案 B: 前端直接调 provider base_url 的 `/v1/models`（OpenAI）或类似 endpoint 验证
  - **推荐方案 A** — 新增后端 IPC，保持前端不直接访问外部 API

#### Models Tab
- 列表: 按 provider 分组，每行显示 model display_name + model_name + capabilities tags
- Add: 表单（provider select / model_name / display_name / max_tokens / thinking_effort / supports_thinking / context_window）
- Edit: 同 Add 表单，预填当前值
- Delete: confirm dialog（提示 "sessions 中的引用将使用 default model fallback"）

#### Default Tab
- reka-ui RadioGroup
- 从 models 列表中选一个作为 default
- 选中立即调 `set_default_model` IPC

## Acceptance Criteria

### 后端新增
- [ ] `update_session_model_id` IPC 可用，前端 dropdown 选中立即写入 `sessions.model_id`
- [ ] `test_provider` IPC 可用，Anthropic + OpenAI 各走对应轻量 endpoint
- [ ] `load_session` 返回 `model_id` 字段

### 前端 Store
- [ ] reka-ui 升级到 2.x stable，`pnpm build` 通过
- [ ] `useProvidersStore` CRUD 完整，缓存可用
- [ ] `useModelsStore` CRUD + default model 完整，缓存可用
- [ ] `useConfigStore` 删掉 model/baseUrl/configured，其他功能不变

### 前端 UI
- [ ] SettingsModal 三 tab 功能完整（Providers CRUD + Test, Models CRUD, Default radio）
- [ ] StatusBar 左下齿轮打开 SettingsModal
- [ ] StatusBar 右下 dropdown 按 provider 分组，选中立即生效
- [ ] 空 catalog 时显示 "(未选择模型)" + 点击跳 Settings
- [ ] ChatPanel header model chip 已删除
- [ ] 新 session 使用 default model
- [ ] Provider Test 通过才能 Save

### 构建
- [ ] `pnpm build` 通过（vue-tsc strict）
- [ ] `pnpm tauri build` 通过

## Definition of Done

- [ ] `pnpm build` + `pnpm tauri build` 通过
- [ ] `cargo test --lib` 通过（新增 IPC 的单元测试）
- [ ] 文档更新:
  - `docs/IMPLEMENTATION.md` — 路线图更新（多 provider 从 §2.7 拆出）
  - `docs/HACKING-llm.md` — 加 OpenAI 差异章节
  - `docs/BACKLOG.md` — §4 多角色引用
  - `.trellis/spec/backend/llm-contract.md` — 多 provider 协议 + 新增 IPC
  - `.trellis/spec/frontend/state-management.md` — 新 stores
- [ ] Rollout/rollback: 前端 + 2 个新增后端 IPC（`update_session_model_id` / `test_provider`），后端已有 IPC 不变。回滚需 revert 前端 + 移除新 IPC（或保留不影响）

## Out of Scope

- Per-provider 健康度状态机
- 运行中切 model 的 UX 动画/过渡
- Ollama / Gemini / 其他 provider
- vitest 测试框架配置（项目未配置）
- Provider `/v1/models` 自动发现
- Per-session model 切换历史回放
- 主题/暗色模式

## Technical Notes

### 关键文件

| 文件 | 操作 |
|---|---|
| `app/src-tauri/src/db.rs` | **修改**（新增 `update_session_model_id` + `load_session` 补 `model_id`） |
| `app/src-tauri/src/lib.rs` | **修改**（注册 `update_session_model_id` + `test_provider` IPC） |
| `app/src/stores/providers.ts` | **新建** |
| `app/src/stores/models.ts` | **新建** |
| `app/src/stores/config.ts` | **重构**（删 model/baseUrl/configured） |
| `app/src/stores/chat.ts` | **修改**（引用改为 modelsStore） |
| `app/src/components/settings/SettingsModal.vue` | **新建** |
| `app/src/components/settings/ProvidersTab.vue` | **新建** |
| `app/src/components/settings/ModelsTab.vue` | **新建** |
| `app/src/components/settings/DefaultTab.vue` | **新建** |
| `app/src/components/layout/StatusBar.vue` | **重构**（齿轮 + dropdown） |
| `app/src/components/chat/ChatPanel.vue` | **修改**（删 model chip） |

### 关键约束

- reka-ui 需从 alpha.10 升级到 2.x stable
- 项目没有 vitest — 类型安全靠 `vue-tsc --noEmit`
- `pnpm build` = `vue-tsc --noEmit + vite build`
- Tauri 2 IPC: JS camelCase args → Rust snake_case params
- `Option<T>` IPC args: JS **omit field** for None, not `null`

### IPC Option<T> 注意（HACKING-wsl FU-1）

```typescript
// ✅ 正确：omit field for None
await invoke("add_model", {
  providerId, modelName, displayName,
  supportsThinking: true, contextWindow: 200000,
  // maxTokens omitted → None
})

// ❌ 错误：pass null
await invoke("add_model", {
  providerId, modelName, displayName,
  maxTokens: null,  // Tauri 2 treats null as missing required
})
```

### Per-session model 切换 IPC

当前 `update_session` IPC 是否支持更新 `model_id`？需确认。如果不支持，需要新增 `update_session_model` IPC（极轻量：`session_id + model_id` → UPDATE 一行）。

### Test Provider IPC

PR1 没有暴露 `test_provider` IPC。需要新增：
- Rust 侧：`async fn test_provider(base_url, api_key, protocol) -> Result<TestResult, String>`
- `TestResult { success: bool, response_time_ms: u32, error: Option<String>, protocol_version: Option<String> }`
- Anthropic: `POST /v1/messages` with `max_tokens=1` + minimal messages
- OpenAI: `GET /v1/models` with `Authorization: Bearer <api_key>`

## Research References

_暂无 research/ 产出_
