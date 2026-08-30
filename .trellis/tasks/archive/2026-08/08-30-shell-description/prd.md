# shell description 参数 + Shell 卡片与审批卡重设计

## 背景

`shell` / `run_background_shell` 的 tool_use input 只有 `command` / `working_directory` /
`timeout`（`shell.rs:255-275`、`run_background_shell.rs:93-115`）。前端展示因此有
两个真实缺口：

1. **卡片不可扫读**：ToolCallCard / DrawerToolCallCard 折叠态 header 只显示
   `shell · done · 1.2s`，命令文本只有展开 input `<details>`（默认折叠）才可见
   （`ToolCallCard.vue:79-88`、`ToolInputBody.vue:18-21`）。一轮里连续多个 shell
   调用在视觉上完全同质。
2. **审批是"盲签"**：shell 的 permission ask body 只有风险行 + reason（denylist
   命中时才有）+ 按钮列（`PermissionAskBody.vue:202-273`），`path` 行对 shell 恒空，
   命令原文不可见。用户在看不到命令的情况下做允许/拒绝决策。

行业已验证 description 参数解法（Claude Code / ZCode 的 Bash tool 均带 LLM 填写的
短句描述）。用户决策（2026-08-30 评审）：**顺带做 shell 卡片与审批卡的重设计**，
三问三答已定方向：

- Shell 卡形态：**专用 ShellCard 组件**（EditFileCard / SearchHistoryCard 先例，
  MessageItem resolver 按 tool name 替换通用卡）；
- 审批呈现：**一体化审批**（命令块 + 风险条 + 按钮融为卡片的一个状态，去掉独立
  "需要权限"盒子）；
- 输出呈现：**默认收起**（错误输出红框常显）。

## Goal

`shell` / `run_background_shell` 增加可选 `description` 参数（LLM 填写，
display-only）；主面板新建 ShellCard 专属卡（命令块常驻 + 一体化审批）；
drawer 侧工具卡 header 与独立审批卡同步消费 description；旧会话零回归。

## Requirements

### R1 后端 schema（数据源）

- `shell` 与 `run_background_shell` 的 `definition()` input_schema 增加可选
  `description`（string）属性；tool description 文本同步加填写指引：短（约 ≤10 词）、
  主动语态、说明命令的作用（what/why），而非复述命令本身。
- `execute()` **完全不读取**该参数——纯展示字段，执行语义零变化。

### R2 不可信约束（安全边界，硬性）

- `description` 永远是 display-only：不参与执行路径、不参与权限分类（Tier 4
  kill-list / 前缀匹配仍只看 `command` 原文）、不参与审计语义判定。
- 审批界面：`command` 原文必须始终展示，`description` 只能作为意图补充（header
  chip / 意图行），不得替代命令原文。

### R3 主面板 ShellCard（专属卡，替换通用卡）

- MessageItem resolver 对 `shell` / `run_background_shell` 渲染新 `ShellCard`
  （替换 ToolCallCard），组件结构对齐 EditFileCard 先例。
- **Header**：ToolCallHeader；chip 槽位显示 `description`，缺失兜底 command 首行，
  再缺失（防御）chip 隐藏；run_background_shell 加 background pill（对齐
  EditFileCard 的 replace_all pill 样式）；待审批态 status 显示"等待审批"。
- **命令块**（常驻，所有状态可见）：`$` 前缀 + command 原文，mono、pre-wrap、
  超长 max-height 滚动；`working_directory` 存在时在块内加 muted 次行（ellipsis
  + title tooltip）。
- **一体化审批**（pending ask 态）：命令块下方渲染风险条（风险点 + "风险: 中" +
  reason，有则显）+ 4 按钮列（仅一次 / 始终允许 / 拒绝 / 拒绝并说明）+ 拒绝理由
  textarea 交互；不渲染独立"需要权限"容器，命令不重复出现。按钮/理由交互逻辑
  从 PermissionAskBody 抽共享子组件复用，不复制实现。
- **状态机**：running（"…"）→ done（✓ + 折叠 output）/ error（✗ + 错误输出
  红框常显，截断 500 字符）/ 等待审批（一体化审批 UI）。取消/超时标记随错误
  内容自然展示。
- **输出**：非错误默认收起（"▸ output · N chars"）；错误输出常显红框。
- **降级**：command 缺失/畸形 → 整卡降级为 ToolInputBody 兜底（EditFileCard
  同款防御）；`description` 非字符串按缺失处理。

### R4 drawer 侧（worker transcript + 独立审批卡）

- DrawerToolCallCard：shell 家族的 header chip 同 R3 规则显示 description
  （共享 helper），其余形态不变。
- DrawerPermissionAskCard（复用 PermissionAskBody）：shell 家族 ask 增加命令行
  （`command` 原文，pre-wrap + max-height 滚动）+ 意图行（`description`，muted，
  缺失不渲染）；interactive / historical 双模式同效。主面板 shell ask 走 R3
  一体化后不再经过 PermissionAskBody，无命令重复。
- **PermissionAskBody 的 actions/feedback 交互抽为共享子组件**，PermissionAskBody
  与 ShellCard 共用；本次重构对其现有消费方（edit_file / write_file 等主面板
  inline 审批、drawer 审批卡）零行为变化。

### R5 兼容性

- 旧会话消息无 `description`：不报错、走 R3 兜底链；无 `working_directory` 不渲染
  cwd 行。
- wire 层零变更：tool_use input 整包 JSON 本就随消息持久化并透传前端；permission
  ask 的 `toolInput` 也是整包（`permissions.ts:69`），两端均无需新增字段或迁移。
- SearchModal 只读预览无独立分支：预览经共享 MessageItem resolver 自然获得
  ShellCard（与 edit_file → EditFileCard 先例同机制），预览数据下审批 UI 因
  pendingAsk 无法匹配而实际不可达，零行为风险。

### R6 done 态成功色（2026-08-30 评审追加）

- ToolCallHeader 增加 `isSuccess` 变体：done 态（has result 且非 error）的
  status icon + "done" 文本着成功色（`--color-tool-write`，与 EditFileCard
  +N 计数 / 审批 allow 徽章同语义色）；duration 保持 secondary（耗时不是
  状态）。
- 四个消费方接线：ToolCallCard（dispatch 分支**不**传——worker 状态另计）、
  DrawerToolCallCard、EditFileCard、ShellCard。

## 非目标

- 其他工具（read_file / write_file / web_search / dispatch_subagent 等）不加
  description，卡片与审批形态不变。
- 不改 permission wire payload 结构、不改 DB schema、不改审计行结构。
- 不基于 description 做任何自动判断（审批辅助展示而已）。
- shell_status / shell_kill 等衍生工具不加 description、不建专属卡。
- ToolCallCard 本体不为 shell 加特例分支（主面板 shell 全量走 ShellCard）。
- 命令输出高亮 / ANSI 渲染不做（纯文本 pre）。

## 验收标准

- [ ] **AC1**：两个 tool 的 `input_schema` 含可选 `description` string 属性，
      tool description 文本含填写指引；`execute()` 不读取该字段（传畸形值
      / 不传均不影响执行结果）。Rust 测试证明之。
- [ ] **AC2**：ShellCard——header chip 三级兜底（description → command 首行 →
      隐藏）；命令块常驻且含 `$` 前缀与 cwd 行（有 working_directory 时）；
      run_background_shell 显示 background pill；待审批态一体化审批（风险条 +
      4 按钮 + 理由交互，命令不重复）；done 态 output 折叠；error 态输出红框
      常显；command 畸形降级 ToolInputBody。vitest 覆盖上述分支。
- [ ] **AC3**：DrawerToolCallCard shell 家族 header chip 显示 description
      （缺失兜底命令首行）；DrawerPermissionAskCard 的 shell ask（双模式）
      显示命令行 + 意图行；非 shell ask 不变。vitest 覆盖。
- [ ] **AC4**：PermissionAskBody → PermissionActions 抽取行为保持——现有消费方
      （edit_file 等主面板 inline、drawer 审批卡）的既有测试零改动全绿，
      审批按钮 / 理由 / allowAlways 文案分叉逻辑不漂移。
- [ ] **AC5**：权限分类零回归——permissions 模块既有 kill-list / 前缀分类测试
      零改动全绿（分类输入仍是 command，description 不进分类路径）。
- [ ] **AC6**：`cd app && pnpm test` 全绿；`cargo test -p everlasting --lib`
      全绿；lint / type-check 通过。
- [ ] **AC7**：done 态 header status（icon + 文字）渲染 `--success` 修饰类
      （成功绿），error / running / 等待审批态不渲染；dispatch 卡不带
      success。vitest 覆盖（DrawerToolCallCard + ShellCard）。
