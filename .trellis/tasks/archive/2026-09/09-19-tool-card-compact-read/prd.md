# read 族工具卡片紧凑化(glob / list_dir / read_file → 1 行)

## Goal

Web 端聊天流里 `glob` / `list_dir` / `read_file` 的卡片目前**每张固定 85px**,
一个回合里连着七八次查找就能吃掉半屏。把这三张卡重做成**收起 1 行(~24px)、
点击展开**的紧凑卡:headline 保留「哪个工具 + 目标 + 规模 + 状态」,输出按需展开。

实测基线(e2e `tool-card-compact.spec.ts` 种子形态,1280×720 Chromium):
**6 张卡 = 85px × 6**。目标:同种子 ≤ 34px × 6(约 3.5× 压缩)。

## Background(已确认事实)

- 三工具当前都落到通用 `ToolCallCard.vue`,形态是「header 行 + `▸ input` 行 +
  `▸ output · N chars` 行」= 3 行 85px(实测)。
- 通用卡还有 dispatch_subagent 的整卡点击打开抽屉、edit_file 的 diff popover、
  内联审批等分支 —— 直接改通用卡会牵动全部工具。
- `glob` 的 input 里 `pattern` 今天**完全不显示**(chip 数据源 `toolHeaderChip`
  只认 `input.path`),卡片上只剩扳手图标 + "GLOB" —— 看不出搜了什么;
  `glob` / `list_dir` 也没有专属 icon 与 accent(都吃 `toolAccentVar` 的 muted 兜底)。
- 三工具都是权限层 Tier 4 的 **path 工具**(项目外路径会 emit ask),所以新卡片
  **必须**保留内联审批区,否则项目外读取的审批会静默失效(硬约束)。
- 既有先例:`SearchHistoryCard` / `EditFileCard` / `ShellCard` 都是「专属卡替换
  通用卡」(`MessageItem.vue` 的 v-else-if 链,时间轴 + `msg__tools` 两个分支各一处)。
- header 的 markup/CSS 是共享组件 `ToolCallHeader.vue`(RULE-FrontSubagent-001
  明确禁止再复制一份 header)。

## Requirements

### R1 一行 headline
- 收起态 = **1 行**:`[▸] [icon] NAME · chip ……… meta …… ✓ 状态 耗时`。
- chip(目标):`glob` → `pattern`(`input.path` 存在时 `pattern in path`);
  `list_dir` → `path`,缺省显示 `cwd`;`read_file` → `path`。
- meta(规模):`glob` → `42 matches`(截断提示 → `42+ matches`;无命中 → `no matches`);
  `list_dir` → `24 entries`(空目录 `empty`);`read_file` → `L1–120`(带 offset/limit
  时按输出真实行号;截断 → `L1–120 · truncated`;非行号输出兜底 `N chars`)。
- 报错态 meta 不显示(错误由 ✗ + 红卡承载,展开看文案)。

### R2 点击展开
- 展开区 = 输出 `<pre>`(沿用 `ToolOutputBody` 的视觉与安全链路:envelope 解包 /
  截断 / linkify 路径 / 点击委托)+ 输入 `<details>`(`ToolInputBody`)。
- **收起态输出不进 DOM**(不渲染大文本,也不占高)。
- 点击行任意处切换;行 `role="button"` + `tabindex=0` + Enter/Space 可达
  (照 dispatch_subagent 既有先例),`aria-expanded` 反映状态。

### R3 状态与安全面不缩水
- 审批:本卡 `tool_use` 命中 `pendingAsk` 时,**无条件展开**审批区
  (`PermissionAskBody`,交互态),不受收起/展开影响。
- 结果图片(`read_file` 读图)照常渲染缩略图。
- 错误态:红左条 + `✗ error`,展开后是错误原文。

### R4 视觉辨识
- `glob` → `magnifying-glass`、`list_dir` → `folder`、`read_file` → `document`;
- 三工具统一 `--color-tool-read` accent(read 族同色),两档主题都由 token 承载,
  组件零 hex。

## Non-Goals

- 不改 `grep` / `web_search` / `web_fetch` / 通用 `ToolCallCard`(另议)。
- 不改 `SubagentDrawer` 的 `DrawerToolCallCard` 形态(抽屉是次要面)。
- 不改后端工具输出格式;不做「输出预览第二行」。
- 不改移动端断点策略(紧凑 chip 不适用 44px 触摸目标规则,
  `.trellis/spec/frontend/responsive-mobile.md` §6 DEC-6)。

## Acceptance Criteria

- [ ] AC1 e2e:六形态种子下每张卡 ≤ 34px(基线 85px),且不出现通用 `.tool-card`。
- [ ] AC2 e2e:默认收起(输出不在 DOM)→ 点击行展开 → 再点收起。
- [ ] AC3 e2e:报错卡仍 1 行,展开后含后端的错误原文。
- [ ] AC4 单测:chip / meta 纯函数覆盖 成功 / 截断 / 空 / 无命中 / range /
      图片 / 报错 / 字段畸形 各形态。
- [ ] AC5 审批面:`pendingAsk` 命中时审批区无条件可见(单测,接线与
      `ToolCallCard` 同款)。
- [ ] AC6 回归:`pnpm test`(vitest)+ `pnpm build`(vue-tsc)+ `pnpm test:e2e` 全绿。
- [ ] AC7 视觉:桌面截图人工过目(1 行密度 / 展开态 / 错误态 / 两主题)。
