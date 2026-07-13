# B9+ 生成式 UI 收尾(button+action / diff 应用)

> Parent task。承接 B9(07-02,部分落地 selector/diff/code_block)推迟到后期的 D3/D4/D5。拥有整体需求 + 子任务地图 + 跨子任务验收 + 集成 review。

## Goal

让 `use_ui` primitive 从**只读展示**升级为**可交互**:用户能在卡片上对 agent 的提议(diff / button)做出动作(应用 / 拒绝 / 触发),agent 借此在受限场景(plan 模式 / human-in-the-loop)结构化提议修改并交用户拍板。

**核心命门**:`use_ui` 当前是纯展示(silent Allow,零副作用);引入"用户动作 → 后端写"后,既不能破坏 plan 模式语义(plan 约束的是 LLM 不是用户),也不能与 `edit_file` 形成"两种修改模型"冲突。定位与权限模型是本 brainstorm 要解决的根本问题。

## Background / Confirmed Facts(代码可证)

### use_ui 现状
- `tools/use_ui.rs`:non-blocking execute,Silent Allow(Tier 5)+ `Risk::Low`(纯展示无副作用,同 `remember`)。
- primitives 白名单 `KNOWN_TYPES = ["diff", "code_block"]`;`type: "button"` 被 `execute_rejects_unknown_type` 测试明确拒绝(D3 推迟注释)。
- **primitive 数据是"死"的**:只在 `tool_use.input.primitives`,前端只读,无任何回写后端的通道。这是 D3/D4 共同的核心架构缺口。
- 前端 `uiPrimitiveRegistry.ts`:`type → component` 单 entry 表,加 primitive = 加 entry;`<UiCard>` 容器,MessageItem 按 `tool_name === "use_ui"` dispatch(sibling to ToolCallCard)。
- `DiffPrimitive.vue` 已落地:`parsePatch` → `FileDiff[]` → 复用 `DiffView` 只读 + 复制;支持 unified-diff 与 LLM 式 +/- 片段双格式。

### D3/D4/D5 当初推迟理由(B9 parent prd `.trellis/tasks/archive/2026-07/07-02-b9-generative-ui/prd.md`)
- **D3 独立 button + action 白名单**:B9 最大安全面;action 白名单 + 高危 action 过权限⑨;Tauri `generate_handler!` 注册表无现成"agent 可触发 action"(全是管理类 IPC);白名单需从零设计。
- **D4 diff 应用/拒绝**:"随独立 button 后期一起做";**核心担忧 = "与 edit_file 并存两种修改模型造成 LLM 困惑"**;现有 edit_file + 权限⑨ + ToolCallCard 内嵌 DiffView 已覆盖"修改确认"全流程。
- **D5 session `allow_generative_ui` 开关**:绑定 button action 开关一起推迟。

### 权限/mode 现状(决定 D4 定位的关键)
- **Plan 模式过滤写工具**(`agent/permissions/mode.rs::filter_tools_for_mode`):`write_file` / `edit_file` / `shell` / `run_background_shell` / `merge_worker` / `discard_worker` 全部移除。**Plan 模式下 LLM 完全不能改文件**。

### 既有"前端 card / 按钮 → 后端写"参考模式(可直接复用)
- **`request_mode_change`**(07-07):inline message card,用户 allow/deny → 前端 IPC `resolve_mode_change` → 后端 `set_session_mode` 落库。**用户确认 UI 模式**(`<RequestModeChangeCard>`)。
- **`merge_worker` / `discard_worker`**(L3b PR3):SubagentDrawer 按钮 → IPC `merge_worker_run` → 后端 libgit2 写。`ToolKind::GitMutation` Tier 4 `Risk::High` + tool-level grant(WebFetch 式)。**前端按钮触发后端写模式**。
- **`ask_user_question`**:tool:question reverse-IPC,blocking oneshot,`<AskUserQuestionCard>`。

## Technical Decisions
- **D-Q1(已定 ✅)D4 定位 = 用户确认 UI**:LLM 用 diff primitive 提议修改,用户点"应用"才写文件。应用动作**用户触发**(前端 IPC,不走 LLM tool 权限链)→ 不破坏 plan 语义(plan 约束 LLM 不约束用户)+ 不与 edit_file 冲突(edit_file=LLM 自主改,diff 应用=用户拍板)。plan/edit 模式均可。复用 `request_mode_change` allow/deny card 模式。
  - **推论 1**:应用动作是独立前端 IPC(非 LLM tool),`use_ui` 本身仍可保持 Silent Allow(展示无副作用未变)。
  - **推论 2**:plan 模式天然可用 —— `filter_tools_for_mode` 只过滤 LLM 的 tool,用户触发的 IPC 不受影响。
- **D-Q3(已定 ✅)scope = D4 + D3**:本批做 D4(diff 应用)+ D3(通用 button primitive + action 枚举 + 白名单)。**不做 D5** session `allow_generative_ui` 开关(继续推迟)。
- **D-Q2a(已定 ✅)action 模型 = 预定义枚举**:`button.action ∈ 硬编码白名单`,每个 action 对应已知 handler(Tauri IPC 或纯前端)。否决"复用既有 tool 引用"(与 D-Q1"用户触发不走 LLM tool 链"矛盾)+ 否决"自定义 payload"(安全面过大,个人工具无必要)。枚举首批边界见 Q2b。
- **D-Q2b(已定 ✅)枚举首批 = `apply_diff` + 纯前端**:后端 IPC 动作仅 `apply_diff`(写文件 + 审计);`copy` / `dismiss` 纯前端无副作用(不进后端枚举)。`run_command` 等命令类动作**不在本批**(与 shell tool 重复触发路径,安全面陡增)。
- **D-Q4(已定 ✅)apply_diff 权限形态 = 不弹 modal + 校验 + 审计**:用户点"应用"= 意图,不弹权限 modal(复用 merge_worker 按钮先例);做路径校验(project boundary 越界拒绝)+ 审计落表(AuditKind 新增 apply 类)+ DiffView 已展示变更预览。apply_diff 是 Tauri IPC(用户触发),**不走 LLM tool 权限链**。

## Requirements

### 后端
- **`use_ui` schema 扩展**:primitives 支持 `button` type(`KNOWN_TYPES` + schema enum 加 `"button"`)。button primitive 字段 `{ type: "button", action: "apply_diff"|"copy"|"dismiss", label? }`。`action` ∈ 预定义枚举(D-Q2a)。
- **`use_ui` 保持 non-blocking Silent Allow**(展示无副作用不变)。button 的"动作"由前端按 `action` 分发;`execute` 不执行 action,仍返回"已渲染 N 个 primitive"。
- **新增 Tauri command `apply_ui_diff`**(用户触发 IPC,**非 LLM tool**):入参 `{ sessionId, diff_text }` → 解析 diff → 路径校验(project boundary 越界拒绝)→ 写文件 → 审计落表。**不走 LLM tool 权限链,不弹权限 modal**(D-Q4)。
- **AuditKind 新增** apply 类(如 `UiDiffApplied`),payload 含 session_id + 受影响文件列表;审计 UI 可查。
- 写文件实现复用既有写路径(edit_file 写逻辑 / project boundary 校验)——具体形式 design.md 定。

### 前端
- `uiPrimitiveRegistry` 加 `button` → `<ButtonPrimitive>`(D3)。
- `<ButtonPrimitive>`:按 `action` 分发 —— `apply_diff` → 调 `apply_ui_diff` IPC + 成功/失败反馈;`copy` → 剪贴板;`dismiss` → 本地隐藏。
- `<DiffPrimitive>` 加"应用/拒绝"按钮(D4):应用 → `apply_ui_diff` IPC;反馈成功/失败(越界 / 解析失败 / 部分应用)。
- `uiCard.types.ts` 镜像 button primitive 类型。

### Spec
- `tool-contract.md` use_ui 段补 button type + 新 `apply_ui_diff` IPC 契约。
- `frontend/chat.md` UiCard 段补 ButtonPrimitive + DiffPrimitive apply。
- 审计 AuditKind 文档更新。

## Subtask Map
- **单 task 分 3 阶段,不拆 child**(D3/D4 有依赖:D3 button 的 `apply_diff` 复用 D4 的 `apply_ui_diff` IPC,拆 child 收益低于 B9 当初 3 独立 primitive)。3 阶段详见 `implement.md`:① 后端写闭环(`apply_ui_diff` + 手写 hunk apply + 审计)② 前端 diff 应用 ③ D3 通用 button。

## Acceptance Criteria(parent 级)
- [ ] `use_ui({type:"button",action:"apply_diff",label})` 渲染按钮;点击 → `apply_ui_diff` IPC 执行;`copy`/`dismiss` 纯前端正确(剪贴板 / 本地隐藏)
- [ ] `use_ui({type:"diff",diff_text})` 渲染 DiffView + "应用/拒绝"按钮;点应用 → 文件写入 + 审计落 `UiDiffApplied`
- [ ] `apply_diff` project boundary 越界 → 拒绝 + 错误反馈,不写文件
- [ ] `apply_diff` diff 解析失败 / 空 diff → 清晰错误反馈,不写坏文件
- [ ] **plan 模式可用**:`use_ui` 不被 `filter_tools_for_mode` 过滤;`apply_ui_diff` 是用户 IPC,不受 plan tool 过滤影响
- [ ] `use_ui` 保持 Silent Allow(Tier 5)不变;`apply_ui_diff` 不进 LLM tool 权限链(无 Tier 询问 / 无 PermissionStore)
- [ ] 审计 UI(`<AuditLogModal>`)可查 apply_diff 记录(AuditKind 新增)
- [ ] **质量门**:`vue-tsc --noEmit` 0 err + vitest 全绿 + `cargo test` 全绿 + `cargo fmt --check`

## Out of Scope
- `run_command` 等命令类 action(D-Q2b,与 shell tool 重复触发路径,安全面陡增)
- D5 session `allow_generative_ui` 开关(D-Q3,继续推迟)
- 自由式 UI(HTML 沙箱)
- form / chart / table primitive

## Open Questions
- 无(产品决策树已走完;Q1-Q5 全部定案,见 Technical Decisions)。剩余实现细节走 `design.md`。
