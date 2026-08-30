# ShellCard — shell 家族专属卡与一体化审批

> 08-30-shell-description 新增。`shell` / `run_background_shell` 在 MessageItem
> resolver 里替换通用 ToolCallCard 的专属卡(同 EditFileCard / SearchHistoryCard
> 先例),命令块常驻 + 审批一体化。改 ShellCard / 审批按钮 / chip 兜底逻辑前必读。

---

## 1. 组件与数据源

| 文件 | 职责 |
|---|---|
| `app/src/components/chat/ShellCard.vue` | shell 家族专属卡:命令块 + 一体化审批 + 输出策略(结构对齐 EditFileCard,permission store 接线同源) |
| `app/src/components/chat/PermissionActions.vue` | 共享审批 actions(4 按钮 + 拒绝理由 textarea + allowAlwaysLabel 按 `ask.workerRunId` 分叉);PermissionAskBody 与 ShellCard 共用 |
| `app/src/utils/messageFormat.ts` | `toolHeaderChip(name, input)` chip 兜底链 + `isShellFamilyTool(name)`(封闭名单:shell / run_background_shell) |
| `app/src/components/chat/MessageItem.vue` | resolver:`shell` / `run_background_shell` → ShellCard(timeline 与 msg__tools 两处都要接) |

## 2. 不变量(硬约束)

- **`description` display-only**:LLM 填写的 tool input `description` 永不进入
  执行路径(`shell.rs` / `run_background_shell.rs` 的 execute 只读
  command/working_directory/timeout)与权限分类路径(Tier 4 只看 `command` 原文,
  permissions 模块零感知)。
- **审批界面命令原文恒渲染**:description 只作意图补充(chip / 意图行),不可
  替代命令展示。ShellCard 命令全文在卡内恰出现一次(测试锁死)。

## 3. 契约

### chip 兜底链(`toolHeaderChip`)

```
input.path(string 非空,非 shell 工具现状)           → path
shell 家族且 input.description(string)              → description
shell 家族且 input.command(string)                  → command 第一个非空行
其余(undefined / 畸形类型)                          → null(chip 隐藏)
```

### ShellCard 状态机

| 态 | header status | body |
|---|---|---|
| 等待审批 | "等待审批"(amber pulse) | 命令块 + 风险条(RISK_LABEL_CN/RISK_META + ask.reason) + `<PermissionActions>`;**无**独立"需要权限"盒 |
| running | running… | 命令块 |
| done | ✓ + 耗时 | 命令块 + `<ToolOutputBody>` 折叠 |
| error | ✗ | 命令块 + 红框 pre 常显(`extractToolResultDisplay` + `truncateOutput(,500)`) |
| 畸形降级 | — | command 缺失/非 string → 整卡退 `ToolInputBody` |

`run_background_shell` 同卡 + header background pill。命令块:`$` 前缀(muted)+
mono pre-wrap,max-height 200px 滚动;`working_directory` 存在时第二行
`↳ <path>`(muted + ellipsis + title)。

### drawer 侧

DrawerToolCallCard 只换 chip 数据源(不建 drawer 版 ShellCard——drawer 0-store
边界);worker 的 shell 审批走 PermissionAskBody,其 shell ask 自带命令行
(pre-wrap + 200px 滚动)+ 意图行(description,缺失不渲染),interactive 与
historical 同分支(按 `isShellFamilyTool(ask.toolName)` 门控,不按 mode)。
SearchModal 只读预览经共享 resolver 自然获得 ShellCard(pendingAsk 对快照数据
不可达,零审批风险)。

## 4. Good/Bad

**Good**(新增审批相关行为):只改 PermissionActions 一处,PermissionAskBody 与
ShellCard 同时生效。

```vue
<PermissionActions :ask="pendingAsk" :on-respond="respondApproval" />
```

**Bad**:在 ShellCard 里重写按钮列/理由输入,或在 PermissionAskBody 里加
shell 特例按钮——审批交互双实现必然漂移(allowAlwaysLabel 的 workerRunId
分叉已由共享组件单源化)。

```vue
<!-- Bad:复制 4 按钮进 ShellCard -->
<button @click="respond('allow_once')">仅一次</button> ...
```

## 5. 测试锚点

- `PermissionActions` 保留 `permission-ask-body__btn*` 等 DOM 类名——
  ToolCallCard / SubagentDrawer 既有测试以此选择器锚定,**改名会破坏 AC4 式
  行为保持验证线**。
- ShellCard 契约测试在 `ShellCard.test.ts`(chip 三级兜底 / 命令唯一 /
  4 按钮 + onRespond 全链 `permission_response` / 三态输出 / 降级);
  helper 矩阵在 `messageFormat.test.ts`。
