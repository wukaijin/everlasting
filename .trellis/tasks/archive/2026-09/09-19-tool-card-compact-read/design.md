# Design — read 族紧凑卡

## 1. 形态(ASCII 基线)

收起(1 行,桌面实测 ~24px):

```
▸  ⌕ glob · src/**/*.vue                     6 matches   ✓ 0.2s
▸  ▤ list_dir · app/src/components/chat     24 entries   ✓ 0.0s
▸  ▯ read_file · app/src/App.vue                L1–120   ✓ 0.0s
▸  ▯ read_file · src/agent/loop.rs   L320–439 · truncated ✓ 0.0s
▸  ⌕ glob · **/*.ts                          6+ matches  ✓ 0.4s
▸  ▯ read_file · app/src/DoesNotExist.vue               ✗ error 0.0s
```

展开(点行,输出 + 输入):

```
▾  ▯ read_file · app/src/App.vue                L1–120   ✓ 0.0s
   ┌─────────────────────────────────────────────────────┐
   │ <script setup lang="ts">                            │
   │ import ChatWindow from "./components/ChatWindow.vue"; │
   └─────────────────────────────────────────────────────┘   (max-height 240px 滚)
   ▸ input
```

审批命中(无条件展开,不受收起态约束):

```
▸  ▯ read_file · /etc/hosts                             ✗ error …
   ⚠ 需要权限 · 项目外读取
   [允许] [本会话始终允许] [拒绝]
```

## 2. 组件与接线

新组件 `app/src/components/chat/ReadToolCard.vue`(专属卡替换通用卡,照
`SearchHistoryCard` / `ShellCard` 先例):

```
.rocard                       ← 容器:surface 底 + 3px 左条(read 族 accent)
  .rocard__row  [role=button] ← 1 行:chevron + <ToolCallHeader> + 收起态整行可点
     chevron(Icon chevron-right/down,12px)
     ToolCallHeader  icon/name/chip(=readToolChip)/status/duration
                     + #title-meta 槽(=readToolMeta)
  .rocard__body  v-if=open    ← 输出 pre(linkify + onMarkdownClick)+ ToolInputBody
  .rocard__approval v-if=pendingAsk ← PermissionAskBody(interactive)
  ToolResultImages            ← 读图缩略图(常驻)
```

`ToolCallHeader.vue` **加一个可选具名 slot `#title-meta`**(渲染在 chip 之后、
status 之前,`flex-shrink:0`):header 的 markup/CSS 保持单一来源
(RULE-FrontSubagent-001),不复制第二份 header;对 drawer / permission 变体是
零影响(不传即不渲染)。chevron 放在 header 外、行的首位 —— 它是行的展开语义,
不是 header 内容。

`MessageItem.vue` 两处 v-else-if 链(时间轴分支 + `msg__tools` 分支)在
`ToolCallCard` 之前插入:

```ts
const READ_TOOL_NAMES = new Set(["read_file", "glob", "list_dir"]);
```

```html
<ReadToolCard
  v-else-if="READ_TOOL_NAMES.has(item.name)"
  :call="item"
  :result="getToolResult(message, item.id)"
  :session-id="chatStore.currentSessionId ?? ''"
/>
```

## 3. 高度预算(桌面)

| 项 | 值 |
|---|---|
| 卡片 padding 纵向 | 3px + 3px(原 8+8) |
| 行内容高 | 14px(icon / 文本 12px × 1.3) |
| 边框 | 1px + 1px |
| **合计** | **≈ 22–24px** |

e2e 门 `≤ 34px`:容字体/缩放抖动,同时挡回三行旧形态(实测 85px)。
移动端 padding 收到 `2px 8px`(紧凑 chip 不走 44px 触摸目标规则)。

## 4. 纯函数(chip / meta)

新文件 `app/src/utils/toolSummary.ts`(与 `messageFormat.ts` 同族,便于单测):

```ts
export function isReadFamilyTool(name: string): boolean
export function readToolChip(name, input?): string | null
export function readToolMeta(name, input?, result?: {content; isError} | null): string | null
```

- `readToolMeta` 先经 `extractToolResultDisplay` 解 `{result, cwd}` 信封;
  `isError` → `null`(错误不进 meta 槽)。
- `glob`:剥掉尾部 `(...and N more matches…)` / `(showing the N most recent…)`
  两种截断提示行;`No files matched pattern …` → `no matches`;否则
  `N matches`(有提示行 → `N+ matches`)。
- `list_dir`:`(empty directory: …)` → `empty`;否则 `N entries`
  (`(...N more entries hidden by limit…)` → `N+ entries`)。
- `read_file`:输出是 `\tN\t` 行号形态时取首末行号 → `L{first}–{last}`;
  含 `<truncated: omitted …>` 时追加 ` · truncated`;非行号输出(图片 /
  老数据)兜底 `N chars`(与 `ToolOutputBody` 的 sizeLabel 同词汇)。
- 全部防御 LLM 畸形 input(非 string / 缺失 → 该段省略,绝不抛错)。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| 项目外读取的审批被收起态吞掉 | `pendingAsk` 命中时审批区独立于 `open` 渲染(AC5);接线逐行照抄 `ToolCallCard:224-257` |
| 大输出常驻 DOM 拖慢长会话 | 展开才 `v-if` 渲染输出(AC2 断言收起态 DOM 无输出) |
| header 复制两份后漂移 | 复用 `ToolCallHeader` + 单槽扩展,不复制 CSS |
| 计数被截断提示行污染 | 提示行前缀 `(` 统一剥除;`+` 号表达「还有更多」而非虚报精确数 |
| 通用卡行为回归 | MessageItem 只加 v-else-if,不动 ToolCallCard;`ToolCallCard.test.ts` 保持通过 |
| 展开区丢 linkify / 点击委托 | 直接复用 `linkifyPlainText` + `useCodeBlockCopy().onMarkdownClick`(与 `ToolOutputBody` 同链) |
