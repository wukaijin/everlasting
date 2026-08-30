# design — shell description 参数 + ShellCard 重设计

> 方向已与用户确认（2026-08-30，三问三答）：专用 ShellCard 组件 / 一体化审批 /
> 输出默认收起。本文锁定视觉契约与组件边界。

## 1. 数据流（wire 零变更）

```
LLM tool_use(input.description)
  → agent loop 原样透传（input 是不透明 JSON，无字段白名单）
  → DB 消息 payload_json 整包持久化（现状机制，无迁移）
  → 前端 ToolCallInfo.input（streamController / rehydrate 均整包）
权限链路：PermissionAsk.toolInput 同为整包（permissions.ts:69），无需后端改动。
```

后端唯一代码变更是两处 `definition()` 的静态 schema；`execute()` 不碰
`description`。权限分类（Tier 4）只读 `command` 键，`description` 天然不进分类
路径——permissions 既有测试即回归防线（AC5）。前端全部走 Vue 模板插值（非
v-html），无 XSS 新增面。

## 2. 后端设计

### 2.1 `shell.rs::definition()` / `run_background_shell.rs::definition()`

```rust
"description": {
    "type": "string",
    "description": "Optional. A short (aim for 10 words or fewer), active-voice \
                    summary of what this command does and why — e.g. \"Run unit \
                    tests for the shell tool\". Shown to the user in the tool call \
                    header and permission prompt. Do not restate the command itself."
}
```

tool description 文本末尾各追加一句填写指引；`required` 保持 `["command"]`。

## 3. 前端设计

### 3.1 视觉契约（已确认的 ASCII 基线）

完成态 / 错误态 / 待审批态（主面板 ShellCard）：

```
┌────────────────────────────────────────────┐
│ ▮ shell · 跑 shell 工具单测     ✓ done 26s │
│ ┌────────────────────────────────────────┐ │
│ │ $ cargo test -p everlasting --lib      │ │
│ │ ↳ app/src-tauri            （有 cwd 时）│ │
│ └────────────────────────────────────────┘ │
│ ▸ output · 1.2K chars                      │
└────────────────────────────────────────────┘

┌────────────────────────────────────────────┐
│ ▮ shell · 清理构建产物        ⏳ 等待审批  │
│ ┌────────────────────────────────────────┐ │
│ │ $ find . -name '*.ts' -print0 │        │ │
│ │   xargs -0 wc -l                       │ │
│ └────────────────────────────────────────┘ │
│ ● 中风险 · 命中 denylist: find -exec       │
│ [仅一次] [始终允许] [拒绝] [拒绝并说明]     │
└────────────────────────────────────────────┘
（错误态：header ✗ error + 命令块 + 红框错误输出常显）
```

### 3.2 组件边界

```
MessageItem resolver
  ├─ name ∈ {shell, run_background_shell} → ShellCard（新）
  │    ├─ ToolCallHeader（chip = toolHeaderChip()；bg pill）
  │    ├─ 命令块（自绘，纯展示）
  │    ├─ 一体化审批（pendingAsk 时）：风险条 + <PermissionActions>
  │    ├─ 错误输出块（自绘，红框常显）/ ToolOutputBody（done 折叠）
  │    └─ 降级：command 畸形 → ToolInputBody
  └─ 其他 → 通用 ToolCallCard（不动）

PermissionAskBody
  ├─ <PermissionActions>（新抽出的共享子组件：4 按钮 + 反馈 textarea
  │    + allowAlways 文案分叉 + respond 接线）
  ├─ shell 家族 ask 新增：命令行 + 意图行（drawer 独立审批卡生效）
  └─ 现有 head/reason/path/outcome 结构与行为不变

DrawerToolCallCard：chip 换 toolHeaderChip()，其余不动
```

- **PermissionActions 抽取**是行为保持重构：PermissionAskBody 的 interactive
  actions 块（`PermissionAskBody.vue:229-273`：4 按钮 + showFeedback/feedback
  状态 + submitFeedback/cancelFeedback + allowAlwaysLabel 按 `workerRunId`
  分叉）原样搬入新组件，props 为 `{ ask, onRespond, hideAllowAlways? }`。
  PermissionAskBody template 改为引用之；既有测试零改动全绿是验收线（AC4）。
- ShellCard 不用 PermissionAskBody（避免独立盒子与命令重复），直接组合
  PermissionActions + 自绘风险条。workerRunId 在主面板恒无 → "始终允许"
  文案由共享组件天然正确。
- ShellCard 的 store 接线（pendingAsk 匹配 `ask.toolUseId === call.id` +
  `permStore.respond`）照抄 EditFileCard（`EditFileCard.vue:148-163`），
  0 新 store 逻辑。

### 3.3 关键决策

**D1 — chip 数据源抽纯函数 `messageFormat.ts`**

`toolHeaderChip(name, input): string | null`：`input.path`（非 shell，现状）→
shell 家族且有 string description → shell 家族兜底 command 第一个非空行 → null。
配套 `isShellFamilyTool(name)`（switch 对齐 `toolAccentVar` 写法，builtin 名单
封闭）。ShellCard 与 DrawerToolCallCard 共用；纯函数直接 vitest。

**D2 — 命令块**

- `$` 前缀 muted 色、command primary 色；`pre-wrap` + `word-break: break-word`；
  max-height 200px + overflow-y auto（对齐 `tool-input-body__pre` 既有尺度）。
- `working_directory` 存在（string 非空）→ 块内第二行 `↳ <path>`，muted +
  ellipsis + title tooltip。
- 多行命令原样渲染，`$` 只在首行。
- timeout 参数不展示（视觉噪音，看 input details 的需求由降级兜底覆盖——不，
  ShellCard 不渲染 input details；timeout 属可牺牲信息，记入 PRD 非目标的
  理由）。

**D3 — 一体化审批**

- pendingAsk 判定与 ToolCallCard/EditFileCard 同源（`permStore.getPending(sid)`
  + `toolUseId === call.id`）。
- 风险条：风险点（`RISK_META.iconColor`）+ `风险: <label>` + reason（`ask.reason`
  有则显，红系 muted 文本）。复用 `RISK_LABEL_CN`，不复制映射。
- status 文本：hasResult 前且 pendingAsk → "等待审批"（amber，沿用 running
  pulse）；否则 running…" / done / error 照旧。
- 命令块在待审批态**不重复**——它就是卡 body 本体。
- 审批 resolved（allow 后工具真执行 / deny 后 tool_result 回）→ pendingAsk 消失，
  状态机自然流转；backend 120s 自动 deny 由 ask store 定时器处理，无新逻辑。

**D4 — 输出呈现**

- done：ToolOutputBody 折叠（"▸ output · N chars"），直接复用现组件。
- error（exit≠0 / timeout / cancel，`result.isError`）：红框 pre 常显，
  `extractToolResultDisplay` + `truncateOutput(500)`，样式对齐
  `tool-output-body__pre--error`；不再叠 `▸ output` 折叠条（内容已常显）。
- `[exit code: N]` / `[timeout after Nms]` 等标记随内容文本自然展示，不解析
  不结构化。

**D5 — drawer 侧最小改动**

- DrawerToolCallCard：仅 chip 换 `toolHeaderChip`（worker shell 调用获得
  description/兜底首行）；命令块不做（drawer 的 store 边界使 ShellCard 不可
  直接复用，worker 上下文的命令可见性由审批卡命令行 + input details 覆盖，
  记入 PRD 非目标）。
- DrawerPermissionAskCard → PermissionAskBody：shell 家族自动获得命令行 +
  意图行（3.2 图），interactive / historical 同一 v-if 分支（按 `isShellFamilyTool(ask.toolName)`，
  不按 mode）。主面板 shell ask 不再走此路径，无双份命令。

**D7 — done 态成功色（评审追加）**

ToolCallHeader 加 `isSuccess` prop → `--success` 修饰类，status（icon + 文字）
着 `--color-tool-write`（复用既有成功语义 token，0 新 token）；duration 保持
secondary。调用方传 `hasResult && !isError`；ToolCallCard 的 dispatch 分支
排除（worker 状态与 spawn 结果不同步，着绿会误读）。error 与 success 互斥
由调用方计算保证，CSS 不做 specificity 裁决。

**D6 — 明确不做**

- 不给 chip/命令块加图标区分 description 与 command 来源；`$` 前缀本身就是
  "这是命令"的语义标记。
- 不做 ANSI / 输出高亮；不做输出 diff。
- ToolCallCard / SearchModal 只读预览不为 shell 加任何分支。

## 4. 测试设计

### Rust（`tools/tests_shell.rs` + background_shell 既有测试文件）

- schema 断言：两个 definition 的 `properties.description` 存在、type=string、
  不在 `required`。
- execute 无感断言：input 带 `"description": 12345`（畸形）执行 `echo hi`
  结果与不带一致（AC1 / R1）。

### Vitest

- `messageFormat`（新增或就近）：`toolHeaderChip` 优先级矩阵（path 优先 /
  shell+description / 兜底首行 / 多行取首行 / 畸形类型 / 非 shell 无 path →
  null / input undefined → null）；`isShellFamilyTool` 两真一假。
- `ShellCard`（新测试文件）：
  - chip 三级兜底；命令块渲染 command 与 `$` 前缀；cwd 行有/无；
  - background pill（run_background_shell）；status "等待审批"（pendingAsk 注入）；
  - 一体化审批：风险条 + 4 按钮 + onRespond 接线（mock permStore，仿
    EditFileCard 测试法）；命令不重复断言（容器内 `$` 命令文本出现一次）；
  - done 折叠 output；error 红框常显；command 畸形 → ToolInputBody 降级。
- `PermissionAskBody`（既有 + 增补）：抽取后既有用例零改动全绿；增补 shell
  ask 命令行 + 意图行（双模式）、description 缺失只剩命令行、非 shell ask
  无新增行。
- `DrawerToolCallCard.test.ts`：shell call chip 分支。

## 5. 风险与回滚

| 风险 | 缓解 |
|------|------|
| PermissionActions 抽取破坏现有审批 | 行为保持重构 + 既有测试零改动全绿为验收线（AC4）；抽取是纯搬运 |
| 模型不填/乱填 description | 可选参数 + 三级兜底；最坏退化为主命令首行，不劣于现状 |
| ShellCard 漏接 permission ask 导致审批挂起无 UI | pendingAsk 接线照抄 EditFileCard 成例 + 测试注入 pendingAsk 断言按钮渲染 |
| 超长命令/输出撑破卡片或移动端 | pre-wrap + max-height 滚动（命令块 200px / 错误输出同尺度）；320px 复核 |
| 回滚 | PR 间独立：PR1 schema、PR2 抽取+helper、PR3 ShellCard 可单独 revert；wire/DB 零变更 |
