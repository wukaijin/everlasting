# PRD — 06-08 multi-model PR5 follow-up: 重布线 Settings/Model UI

> Task: `06-09-06-09-06-08-multi-model-pr5-ux-followup-settingsbar-chatinput-model-popover`
> Status: planning
> Parent: `06-08-multi-model-llm-provider-planning` (PR1+PR2+PR3+PR4 已在 main)
> 基线分支: `main` (PR1+PR2+PR3+PR4 都已 merge)
> 创建日期: 2026-06-09
> 阻塞关系: 与 active task `06-08-fix-diff-replace-libgit2-line-stats-with-git-numstat-for-accurate-counts-in-edit-file-follow-up` (Rust 后端) 独立,可并行

## Background

PR4 (`cb00812`) 落地了 multi-model 的 UI 层,引入了一个全新组件 `app/src/components/layout/StatusBar.vue`(243 行),定位是:
- 左下角齿轮 → 打开 SettingsModal
- 右下角 `<select>` (按 provider 分组) → 切换当前 session 的 model
- 空态("(未选择模型)" 灰字) → 点击打开 Settings

并且,`ProvidersTab.vue` 有 Test 按钮,调后端 `test_provider` IPC(只测 provider 的 baseUrl+apiKey+protocol 协议可达,不传 modelName)。

用户 2026-06-09 在 dev 体验后指出 **PR4 整体 UX 错位**,不是单纯的"接线遗漏",而是 4 处方向不对:
1. Settings 入口位置错(主区底部 StatusBar 左下 → 应在 sessions sider 下面新 Bar)
2. Test 功能语义错(测 Provider 协议可达 → 应测 Model 连通性,Provider Test 完全移除)
3. 模型选择器位置错(主区底部 StatusBar 右侧 → 应在 chat-input__hint 右侧)
4. dropdown 风格错(HTML 原生 `<select>` → 应抄 worktree 按钮的手写 popover 风格)

## Goal

按用户 4 点要求重布线 multi-model UI:
- Settings 入口在 Sidebar 底部新 Bar
- Model 选择 dropdown 在 chat-input__hint 行右侧
- Test 功能移到 ModelsTab,测 model 真实连通性
- 复用 worktree 手写 popover 实现(向上弹,符合 chat input 底部位置)
- 完全删除 `StatusBar.vue` 组件(它整体是错的设计)

## Non-goals

- 不动 PR1/PR2/PR3 的后端 catalog 层(db.rs / lib.rs 的 Provider dispatch / Anthropic+OpenAI adapter)
- 不动 chat 协议栈(PR2 决议的行为完全不变约束继续生效)
- 不改 SettingsModal 的 Tabs 整体结构,只调整入口
- 不重做 worktree 按钮的 popover(抽 composable 留作 future refactor)
- 不做 i18n(中文文案写死)

## Requirements (用户 4 点决策,2026-06-09 收敛)

### R1. Settings 入口 → Sidebar 下面新 Bar

- `app/src/components/layout/Sidebar.vue` (86 行) 加 `.sidebar__footer` flex-shrink:0
  - 位置:`<SessionList />` 之后,`.sidebar` 的最后一个 child
  - 内容:一个 `<button>` 齿轮 + 文字"设置" / "Settings",点击 open SettingsModal
  - SettingsModal 状态从 `StatusBar.vue` 移过来:Sidebar.vue 内 `const settingsOpen = ref(false)`,`<SettingsModal v-model:open="settingsOpen" />`
- `app/src/components/layout/StatusBar.vue` 整个文件**删除**(243 行,完全废弃)
- `app/src/stores/streamController.ts` / `app/src/stores/config.ts` / `app/src/stores/models.ts` 中关于 StatusBar 的注释**清理**

### R2. Test 功能 → 测 Model 连通性 (Provider Test 完全移除)

- `app/src/components/settings/ProvidersTab.vue` 删除整个 Test 按钮区块:
  - line 30-48 `testResult` / `testing` / `testPassed` / `canSave` (testPassed 那段)
  - line 87-119 `onFormChange` / `runTest`
  - line 297 起的 "Test result" 模板 + "Test" 按钮
  - `canSave` 简化为:协议/baseUrl/apiKey/displayName 都非空 + !saving
- `app/src/components/settings/ModelsTab.vue` 每行 model 加 Test 按钮 (右侧 actions 区)
  - 调 `invoke("test_model", { modelId: m.id })`
  - 显示测试中 / 成功(绿色 + 延迟 ms) / 失败(红色 + error)
  - 与 ProvidersTab 旧 Test 按钮的 UX 一致(loading / result state)
  - **结果保留条件 (用户 2026-06-09 决策)**: 行内 inline 展示, 保留到 (a) 用户再次点该行 Test 按钮 或 (b) 该 model 行被删除。切换 provider / 编辑 model 字段不清结果(测试的就是 model 整体连通性,不是某个字段)
- 后端新增 IPC `test_model(model_id: String) -> { success, latencyMs, error }`:
  - 路径:`app/src-tauri/src/lib.rs` (在 `test_provider` 旁边,约 line 482 后)
  - 实现:
    1. `db::get_model(model_id)` → `db::get_provider(model.provider_id)`
    2. 用 `provider.base_url` + `provider.api_key` + `provider.protocol` + `model.model_name` 跑一次最小消息(同 `test_provider` 的 anthropic/openai 分支)
    3. 关键差异:`body["model"]` 用真实 `model.model_name` 而非 hardcoded `"claude-sonnet-4-5"`
  - 注册到 `tauri::generate_handler!` (line 2373 附近)
  - `pub` 暴露给前端的 TS 类型:`invoke<{success: boolean; latencyMs: number; error: string | null}>("test_model", { modelId })`
- 旧 `test_provider` IPC **保留**(catalog 解析时仍可能用到 provider 协议可达性检查,或留 OOS),但前端不再调用 → 加 `#[allow(dead_code)]` 或保留 + 注释 "deprecated, use test_model"
  - 决策:**保留并加注释**,不动 Rust 端(后端 catalog 解析未来可能用它)

### R3. Model 选择 → chat-input__hint 右侧

- `app/src/components/chat/ChatInput.vue` line 137-139 `.chat-input__hint` 改造:
  - 模板从 `<div class="chat-input__hint">⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令</div>`
  - 改为:
    ```html
    <div class="chat-input__hint">
      <span class="chat-input__hint-text">⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令</span>
      <!-- ModelSelect 组件:复用 worktree popover 风格,向上弹 -->
      <ModelSelect />
    </div>
    ```
  - CSS: `.chat-input__hint` 改为 `display: flex; align-items: center; justify-content: space-between; gap: 8px`
  - 新组件 `app/src/components/chat/ModelSelect.vue` 负责 model 切换 popover
- 删 `app/src/components/StatusBar.vue` 右侧 model dropdown 区块 (line 102-142, after R1 整体删除已经覆盖)

### R4. Model dropdown 抄 worktree 手写 popover (向上弹)

- 抽出来的 ModelSelect 组件 (`app/src/components/chat/ModelSelect.vue`) 实现要点:
  - 跟 ChatPanel.vue 的 worktree dropdown **完全同款**风格:
    - `position: absolute; top: auto; bottom: calc(100% + 4px); right: 0;` (向上弹,符合 chat input 底部位置)
    - `background: var(--color-bg-surface); border: 1px solid var(--color-bg-border); border-radius: 6px; box-shadow: 0 4px 12px rgba(0,0,0,0.4);`
    - 关闭逻辑:`document.addEventListener("click", onDocumentClick)` + `!root.contains(target)` 关闭
    - 键盘 Esc 关闭
  - 按钮 label: 当前 model 的 `display_name`(无 model 时灰色"(未选择模型)")
  - 菜单内容: 按 `modelsStore.modelsGroupedByProvider` 分组 (跟 StatusBar 原 optgroup 一致),每组:
    - 标题行(只读):`group.provider.displayName`
    - 列表项:`m.displayName`,点击触发 `update_session_model_id(m.id)` (同 StatusBar 原 onModelChange)
  - 状态接入:
    - `currentModelId`: per-session 优先 → global default (`modelsStore.defaultModelId`)
    - `isStreaming`: 从 `useChatStore().isCurrentSessionStreaming` 拿,streaming 时 disable + 灰
- 不引入 reka-ui `DropdownMenu` (用户决策,理由:跟 worktree 行为/视觉一致,无新依赖)
- 不抽 `usePopover` composable (留作 future refactor,OOS)

## Acceptance Criteria

### 视觉 / 交互

- [ ] Sidebar 底部 (`.sidebar__footer`) 出现齿轮按钮 "设置",点击打开 SettingsModal
- [ ] StatusBar 整体消失 (DOM 里没有 `.status-bar` 元素)
- [ ] ChatInput 底部 hint 行右侧出现 model 按钮 (label 是当前 model 的 display_name 或灰色"(未选择模型)")
- [ ] Model 按钮点击 → 弹出 popover,popover **向上** (弹层在按钮上方),不遮挡 chat input
- [ ] popover 内按 provider 分组显示 model 列表
- [ ] 点击 popover 内的 model → 触发 `update_session_model_id` IPC, popover 关闭, ChatInput 的按钮 label 立即更新
- [ ] streaming 时 model 按钮 disabled + tooltip "Streaming 中,无法切换模型"
- [ ] popover 外部点击 / Esc → 关闭
- [ ] ProvidersTab 不再有 Test 按钮
- [ ] ModelsTab 每行右侧有 "测试" 按钮,点击后显示 loading / 成功(绿色 + 延迟) / 失败(红色 + 错误)

### 后端

- [ ] `lib.rs` 新增 `test_model(model_id)` IPC,handler 读 catalog model+provider,跑最小消息测真实 model_name
- [ ] `test_model` 注册到 `tauri::generate_handler!`
- [ ] 旧 `test_provider` 保留 + 加 deprecation 注释
- [ ] `cargo check` + `cargo test --lib` 全 pass,0 warning

### 文件清理

- [ ] `app/src/components/layout/StatusBar.vue` 删除 (243 行)
- [ ] `app/src/components/chat/ChatWindow.vue` 不动 (R1 移走入口后,这里不需要 import StatusBar)
- [ ] `app/src/components/chat/ModelSelect.vue` 新建
- [ ] `app/src/components/layout/Sidebar.vue` 加 `.sidebar__footer`
- [ ] `app/src/components/chat/ChatInput.vue` 改 `.chat-input__hint` 为 flex + 加 ModelSelect
- [ ] `app/src/components/settings/ProvidersTab.vue` 删 Test 区块
- [ ] `app/src/components/settings/ModelsTab.vue` 加 Test 按钮
- [ ] 注释清理:`streamController.ts` / `config.ts` / `models.ts` 里关于 StatusBar 的引用

### 单元 / 集成

- [ ] 新增 ModelSelect 组件无 prop,纯从 store 读 (`config.loaded` / `modelsStore.models` / `chatStore.currentSessionId`)
- [ ] `vue-tsc --noEmit` 全 pass
- [ ] `vite build` 全 pass
- [ ] `cargo test --lib` 全 pass
- [ ] `pnpm build` (= vue-tsc + vite build) 全 pass

## Out of Scope

- ❌ OpenAI / Anthropic adapter 改动 (PR2/PR3 锁定)
- ❌ Provider dispatch 行为变化
- ❌ SettingsModal 的 Tabs 整体结构
- ❌ 抽 `usePopover` composable (留 future)
- ❌ i18n (中文文案写死)
- ❌ Settings 入口的快捷键 (例如 Cmd+, 打开 Settings)
- ❌ ModelSelect 的搜索/过滤 (model 数量小,optgroup 分组足够)
- ❌ 移动 Settings 入口到 AppHeader (用户明确要 sidebar footer)

## Technical Notes

### 关键文件改动

| 文件 | 改动 |
|---|---|
| `app/src/components/layout/StatusBar.vue` | **删除** (243 行整体废弃) |
| `app/src/components/layout/Sidebar.vue` | 加 `.sidebar__footer` (齿轮 + 文字) + `settingsOpen` ref + `<SettingsModal v-model:open="settingsOpen" />` (~30 行) |
| `app/src/components/chat/ModelSelect.vue` | **新建** (~150 行,抄 worktree popover 风格) |
| `app/src/components/chat/ChatInput.vue` | `.chat-input__hint` 改 flex + 嵌入 `<ModelSelect />` (~10 行) |
| `app/src/components/settings/ProvidersTab.vue` | 删 Test 区块 (~80 行),`canSave` 简化 |
| `app/src/components/settings/ModelsTab.vue` | 每行加 Test 按钮 + test 状态 (~50 行) |
| `app/src-tauri/src/lib.rs` | 新增 `test_model` IPC handler (~70 行) + 注册到 `generate_handler!` |
| `app/src/stores/streamController.ts` | 清理 StatusBar 注释 |
| `app/src/stores/config.ts` | 清理 StatusBar 注释 |
| `app/src/stores/models.ts` | 清理 StatusBar 注释 |

### 关键复用点

- `ChatPanel.vue:127-149` worktree dropdown 的手写 popover 实现:`worktreeMenuOpen` / `worktreeMenuRoot` / `onDocumentClick` / `document.addEventListener("click", ...)` 模式 → ModelSelect.vue 抄相同结构
- `ChatPanel.vue` `.chat-panel__menu` CSS (`position: absolute; top: calc(100% + 4px); right: 0; ...`) → ModelSelect 改 `bottom: calc(100% + 4px); top: auto;` 向上弹
- `app/src/stores/models.ts` `modelsGroupedByProvider` (PR4 已建) → ModelSelect 直接用

### Anti-patterns (避免)

- ❌ 引入 reka-ui `DropdownMenu` (用户决策:抄 worktree 风格,不引入新依赖)
- ❌ 抽 `usePopover` composable (留 future,OOS)
- ❌ 改 `app/src/components/chat/ChatWindow.vue` (R1 移走入口后这里不需要改)
- ❌ 改 worktree dropdown (它的弹方向 `top: calc(100% + 4px)` 是对的,ModelSelect 反向 `bottom: calc(100% + 4px)` 是位置不同,不是 bug)
- ❌ 删旧 `test_provider` IPC (保留 + deprecate,后端 catalog 解析未来可能用)
- ❌ 在 ChatInput 里直接写 popover 逻辑 (拆 ModelSelect.vue,职责清晰)
- ❌ 把 Model 按钮放 chat-input__row 里 (那是 textarea+send 按钮区,不放)

## Definition of Done

- [ ] 所有 R1-R4 acceptance criteria 通过
- [ ] `vue-tsc --noEmit` + `vite build` + `cargo check` + `cargo test --lib` 全 pass,0 warning
- [ ] 手动验证 4 个 user flow:
  1. Sidebar 齿轮 → 打开 Settings modal → 关闭
  2. ModelsTab 加 model → 测该 model → 看到成功/失败
  3. ChatInput 右侧 model 按钮 → 弹 popover (向上) → 选 model → 按钮 label 更新
  4. streaming 时 model 按钮 disabled
- [ ] `docs/IMPLEMENTATION.md` §multi-model 段加 PR5 follow-up 状态
- [ ] trellis-check 通过
- [ ] commit message: `fix(ui): PR5 multi-model UX follow-up — sidebar settings + chat-input model popover + test_model`

## Decision (ADR-lite)

### D1. Settings 入口位置: Sidebar footer 而非主区底部 (2026-06-09)

**Context**: PR4 把 Settings 齿轮放在主区底部 StatusBar 左下,用户在 dev 体验后指出"找不到入口"。原因可能是:(a) 状态栏 11px 灰色文字 + 12px 齿轮图标视觉权重低,不易发现;(b) 主区底部焦点是 chat input,齿轮在那里被忽略。

**Decision**: 移到 Sidebar 底部, 跟 "会话 SESSIONS" 形成对称 (header 标题 + 底部设置),260px 侧栏内 11px 灰色齿轮 + "设置" 文字。

**Consequences**:
- ✅ 视觉对称,符合"侧栏管 meta (会话列表 + 设置)"的隐喻
- ✅ 不被 chat input 视觉抢焦点
- ⚠️ Sidebar 在空态 (`showEmptyState` 为 true) 时不渲染,空态下用户仍找不到设置入口 → 留 OOS (EmptyProjectState 内可考虑加 "先去设置" 引导, 留 future)
- ⚠️ StatusBar.vue 整体删除 (它只剩齿轮有意义, 右侧 model dropdown 移到 ChatInput)

### D2. Test 测 Model 而非 Provider (2026-06-09)

**Context**: PR4 的 `test_provider` 测的是 provider 的协议层 (`baseUrl+apiKey+protocol` 跑最小消息),但用户实际操作中想验证的是"这个 model 名能不能通"(同 provider 配的另一个 model 可能不通,例如 GLM 兼容层下某些 model_name 报 404)。

**Decision**:
- 后端新增 `test_model(model_id)` IPC, 接受 modelId → 查 catalog → 拿真实 `model.model_name` 跑最小消息
- 前端 ProvidersTab 完全删 Test 按钮 (测 provider 协议可达性对用户无意义, provider 已通过 catalog 配置能跑 `chat` 命令就证明协议通)
- ModelsTab 每行加 Test 按钮

**Consequences**:
- ✅ 测的是用户真正关心的"这个 model 能不能用"
- ✅ 失败信息能定位到具体 model_name (而非泛泛的"protocol not supported")
- ⚠️ 后端 `test_provider` 变 dead code, 但保留 + deprecate (catalog 解析未来可能用,例如检查 provider.api_key 是否被服务端拒绝)
- ⚠️ ModelsTab 表格行变宽, 移动端 / 窄窗可能挤 → 当前 sidebar 260px 不影响, ModelsTab 在 modal 内 (SettingsModal max-width 通常 600+), 留 OOS

### D3. Model dropdown 抄 worktree popover (2026-06-09)

**Context**: PR4 用 HTML 原生 `<select>`,但弹出方向不可控 (不同 OS 风格不同),且与项目内已有的 worktree dropdown 视觉不一致。reka-ui 2.9.9 提供 `DropdownMenu` / `Select` / `Popover` / `Menu` / `Combobox`,但引入新依赖路径会增加心智成本。

**Decision**: 抄 ChatPanel.vue worktree dropdown 的手写 popover 实现 (`onDocumentClick` + `!root.contains` + Esc 关闭),弹方向根据位置反向 (worktree 在 header 向下弹 → ModelSelect 在 chat input 底部向上弹)。

**Consequences**:
- ✅ 视觉/行为 100% 一致 (跟 worktree)
- ✅ 无新依赖
- ✅ 键盘可达 (Esc)
- ⚠️ 手写 popover 重复 (worktree + ModelSelect 各一份),留 OOS 抽 `usePopover` composable
- ⚠️ a11y 不如 reka-ui (无 `aria-expanded` / `aria-controls` 等) → 加最小 a11y (button 加 `aria-haspopup="menu"` + `aria-expanded`)
