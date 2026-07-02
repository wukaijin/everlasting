# B9 生成式 UI（parent task）

> Parent task。拥有整体需求 + 子任务地图 + 跨子任务验收 + 集成 review。
> parent 本身不是实现目标；MVP 工作拆给 3 个 child（见 Subtask Map）。

## Goal

让 agent 的回复不只是文本，而是**可交互的 UI 卡片**。落地统一的 `use_ui` tool + 前端 component registry，让 LLM 用结构化 JSON 输出 UI，前端按 `type` 渲染。

**MVP 范围（3 个 primitive）**：`selector`（复用 `ask_user_question`，零工作）+ `diff`（只读展示，复用 `DiffView`）+ `code_block`（hljs 高亮 + 复制）。覆盖"agent 询问 / 展示结果"80% 用例。`button` 及其 action 白名单推后期（见 Out of Scope）。

## Background / Confirmed Facts（代码可证）

- B9 在 ROADMAP 第三档（缓做），DESIGN.md:75 列为"输出层 4 primitives"。
- 架构文档（ARCHITECTURE.md §⑭）预留了 `ui_render` 内容类型 + `use_ui` tool + `ui:render` 低频独立事件，但**全仓零代码实现**（`grep ui_render|UiCard|use_ui|ui:render` 零命中）。承载机制从零设计。
- **selector 已有特化实现** = `ask_user_question` tool（2026-06-30）。完整链路：`tools/ask_user_question.rs`（`ToolDef` + `execute_blocking` + `validate`）→ `chat_loop.rs` 特殊 dispatch（`tool_name == "ask_user_question"` 绕过 `execute_tool` match）→ `QuestionStore`（oneshot + session-singleton mutex）→ emit `tool:question` → 前端 `questionCards` store → `<AskUserQuestionCard>` → `resolve_tool_question` IPC → oneshot 唤醒 → `tool_result` 回填。数据结构 `Question`/`QuestionOption`/`multi_select` 与通用 selector 100% 重合。
- **前端 dispatch 雏形已存在**：`MessageItem.vue` 按 `tool_name === ASK_USER_QUESTION_TOOL_NAME` 路由 `AskUserQuestionCard`，否则 `ToolCallCard`。这是 component registry 的 1-entry 雏形，B9 泛化它。
- **diff 渲染层已落地且已在 tool 卡片内嵌**：`DiffView.vue`（jsdiff `parsePatch` + 行级着色）有 3 个调用方——`DiffModal.vue`、`ChatPanel.vue`、**`ToolCallCard.vue:635`**（在 tool result 里内嵌渲染 diff，"tool 返回 diff 数据 → 前端交互式 diff"已在跑）。独立数据源是 `diff_worktree` IPC（git diff）。
- **code_block 半落地**：markdown 管线（`utils/markdown.ts` = marked + DOMPurify，`MessageItem.vue` v-html）渲染代码块，但**无语法高亮**（无 hljs/shiki/prism）。
- **button 完全没有**，且 Tauri `generate_handler!` 注册表（`lib.rs:152`，~40+ commands）里**没有现成"agent 可触发的 action"**——全是管理类 IPC（providers/sessions/permissions/audit/...）。button 白名单需从零设计（推后期）。
- **`ContentBlock` 现有变体**（`llm/types.rs:75`）：`Text` / `Thinking` / `RedactedThinking` / `ToolUse` / `ToolResult`。无 `UiRender`/`Image`。新增变体要改 Anthropic + OpenAI 两个 Provider wire。
- **持久化现状**：`db/sessions.rs:592 persist_turn` 把 message content（含 `tool_result` blocks）JSON 落库 + `has_tool_calls`/`has_tool_results` 标志。tool 模式的 UI 事件天然持久化。

## Technical Decisions（已定，2026-07-02）

- **D1 承载机制 = `use_ui` 单 tool + `primitives` 数组**。对齐 `ask_user_question` 的"单 tool 多 item"形态。复用现有 tool 管线（权限⑨ / 审计 / `persist_turn` 持久化 / 前端 `tool_name` dispatch），零 Provider wire 改动。否决 `ui_render` content block（破坏两 Provider wire + Anthropic 不原生认 + B9 吃不到"自然输出"优势）；否决"每 primitive 一个 tool"（注册表膨胀 + 与 ask_user_question 重叠）。
- **D2 执行模型 = `use_ui` non-blocking 展示**。use_ui 只承载展示型 primitive（diff / code_block），立即返回 tool_result（"已渲染"），不等用户交互。**selector 不重做，直接 = `ask_user_question`**（blocking oneshot 已验证，语义 100% 重合）。前端 component registry 统一 dispatch：`tool_name === ask_user_question` → selector 卡片；`tool_name === use_ui` → 按 primitive.type 路由。否决"use_ui 统一 blocking"（selector 与 ask_user_question 重叠）；否决"全 non-blocking + selector 进 use_ui"（新建 UI 事件→LLM 通道，复杂）。
- **D3 独立 button primitive 推后期**。MVP 不做可自定义 action 的独立 button（B9 最大安全面：action 白名单 + 高危 action 过权限⑨，DESIGN.md:70）。首批仅内嵌复制（code_block）。独立 button + action 白名单后期一起做。
- **D4 diff primitive MVP 只读展示 + 复制**。不带"应用"动作（应用推后期，随独立 button）。理由：现有 edit_file + 权限⑨ + ToolCallCard 内嵌 DiffView 已覆盖"修改确认"全流程；diff primitive MVP 聚焦展示型（对比/分析），避免与 edit_file 并存两种修改模型造成 LLM 困惑。零新增安全面。
- **D5 session 开关 MVP 不做**。默认常开，不做 `allow_generative_ui` 开关。理由：use_ui non-blocking 展示型无副作用（不像 button action 有越权风险），无滥用面；个人单用户工具；现有 Mode（edit/plan/yolo）已是更通用的控制层。
- **D6 code_block 高亮库 = hljs**。最轻、marked-highlight 集成成熟、Vite 友好、主题多，个人工具够用。否决 shiki（重，TextMate grammars 首屏慢）；否决 prism（Vue 生态集成不如 hljs）。

## Requirements

- **use_ui tool**（后端）：`tools/use_ui.rs` — `ToolDef` + `input_schema`（`primitives: [...]`，discriminated union by `type` ∈ {`diff`, `code_block`}）+ `execute`（non-blocking，返回 "已渲染" tool_result）。注册进 `builtin_tools()` + `execute_tool` match（普通 dispatch，**不需** ask_user_question 那样的 blocking 特殊分支）。
- **前端 component registry**：泛化 `MessageItem.vue` 的 tool_name dispatch —— `use_ui` → 按 `primitive.type` 路由到 `<DiffPrimitive>` / `<CodeBlockPrimitive>`；`ask_user_question` 保持 → `<AskUserQuestionCard>`（= selector primitive）。
- **selector primitive** = 现有 `ask_user_question` tool + `<AskUserQuestionCard>`，零代码改动（仅在 spec/文档声明它是 B9 selector primitive）。
- **code_block primitive** = hljs（marked-highlight 接现有 marked 管线）+ `<CodeBlockPrimitive>` 组件 + 复制按钮。现有 markdown 代码块顺带获得高亮。
- **diff primitive** = 复用 `DiffView.vue`（只读）+ `<DiffPrimitive>` 包装组件 + 复制按钮。use_ui diff primitive 输入 unified diff 文本 → 转 `FileDiff[]` → DiffView 渲染。

## Out of Scope（首批 / 推后期）

- **独立 button primitive + action 白名单**（D3）— action 枚举/command 白名单 + 高危 action 过权限⑨ + 审计
- **diff primitive 应用/拒绝动作**（D4）— 随独立 button 后期一起做
- **session `allow_generative_ui` 开关**（D5）— 随 button action 开关后期一起做
- **自由式 UI**（LLM 生成 HTML 沙箱渲染）— 远期
- `form` / `chart` / `table` primitive + 图表库（ECharts）/ 表格（@tanstack/vue-table）— BACKLOG §5 后期

## Subtask Map

3 个 child + parent 收尾。依赖：**Child A 是 B/C 的前提**；B、C 互相独立可并行。

| Child | 范围 | 依赖 | 主要工作 |
|---|---|---|---|
| **A. use_ui 基础设施** | 后端 `use_ui` tool + 前端 component registry + dispatch 泛化 | — | `tools/use_ui.rs`（ToolDef+schema+execute non-blocking）+ `tools/mod.rs` 注册 + `execute_tool` match；前端 MessageItem dispatch 泛化 + registry（type→组件）+ use_ui tool 卡片渲染框架（含 mock primitive 验证管线通） |
| **B. code_block primitive** | hljs 高亮 + 复制 | A | 装 hljs + marked-highlight，接 `utils/markdown.ts`；`<CodeBlockPrimitive>` 组件 + 复制按钮；现有 markdown 代码块顺带高亮（无回归） |
| **C. diff primitive** | 只读展示 + 复制 | A | `<DiffPrimitive>` 包装 `DiffView.vue` + 复制按钮；use_ui diff primitive 输入（unified diff 文本）→ `FileDiff[]` 转换 |

**Parent 收尾（§3.3 spec update）**：selector 声明（`ask_user_question` = B9 selector primitive）+ tool-contract.md / frontend/chat.md spec + ROADMAP §1.2 / §2 标记 B9 部分落地（selector/diff/code_block ✅，button 推后期）。

## Acceptance Criteria（parent 级）

- [ ] **Child A**：LLM 调 `use_ui` 返回 non-blocking tool_result（"已渲染"），tool_result 经 `persist_turn` 持久化；前端 registry 按 `primitive.type` 正确路由（mock primitive 验证管线通）
- [ ] **Child B**：`use_ui({type:"code_block"})` 渲染 hljs 高亮 + 复制按钮；现有 markdown 代码块同时获得高亮（无回归，vitest 全绿）
- [ ] **Child C**：`use_ui({type:"diff"})` 渲染只读 DiffView + 复制按钮；unified diff 文本正确转 `FileDiff[]`
- [ ] **selector**：`ask_user_question` 作为 B9 selector primitive 在 spec 中声明，零代码改动
- [ ] **跨 child 集成**：LLM 单次 `use_ui` 调用同时输出 diff + code_block，前端正确渲染两者
- [ ] **承载机制单一**：仅 `use_ui` tool 通道，未引入 `ui_render` content block
- [ ] **质量门**：`vue-tsc --noEmit` 0 err + vitest 全绿 + `cargo test` 全绿
