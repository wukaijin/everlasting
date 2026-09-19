# ReadToolCard — read 族专属紧凑卡(glob / list_dir / read_file)

> 09-19-tool-card-compact-read 新增。三个只读检视工具在 MessageItem resolver
> 里替换通用 ToolCallCard 的专属卡(同 ShellCard / EditFileCard / SearchHistoryCard
> 先例),形态是**收起 1 行、点击展开**。改这几张卡的高度 / headline 信息 /
> 展开行为前必读。

---

## 1. 为什么(问题与实测)

三工具此前都吃通用 `ToolCallCard`:`header + ▸ input + ▸ output · N chars`
三行,实测**每张 85px**(e2e 六形态种子,1280×720)。一个回合连着七八次
查找就吃掉半屏,而那两行 summary 几乎没有用户当下要的信息 —— 更糟的是
`glob` 连 pattern 都不显示(通用卡的 chip 只认 `input.path`)。
紧凑化后同种子 **26px/张**(≈3.3× 压缩),且 pattern / 命中数 / 行范围
被提到 headline。

## 2. 文件与数据源

| 文件 | 职责 |
|---|---|
| `app/src/components/chat/ReadToolCard.vue` | 紧凑卡本体:1 行 headline + 点击展开的输出/输入 + 内联审批 + 读图缩略图 |
| `app/src/utils/toolSummary.ts` | `isReadFamilyTool` / `readToolChip`(目标)/ `readToolMeta`(规模)三个纯函数 —— headline 的全部数据源 |
| `app/src/components/chat/MessageItem.vue` | resolver:`isReadFamilyTool(name)` → ReadToolCard(timeline 与 `msg__tools` 两处都要接) |
| `app/src/components/chat/ToolCallHeader.vue` | 复用共享 header;本卡经 `#title-meta` 槽塞规模标记(2026-09-19 新增的可选槽) |
| `app/src/components/chat/ToolOutputBody.vue` | 复用输出渲染;本卡用 `:collapsible="false"`(裸 `<pre>`,不套 `<details>`) |
| `app/src/utils/messageFormat.ts` | `toolAccentVar` / `toolIcon` 里 glob / list_dir 的 read 族映射 |

## 3. headline 契约

```
▸  [icon] NAME · chip                              meta            ✓ done 0.3s
```

- **chip = 目标**(`readToolChip`):`glob` → `pattern`(`input.path` 非空且非 `.`
  时 `pattern in path`);`list_dir` → `path`,缺省字面量 `cwd`;`read_file` → `path`。
- **meta = 规模**(`readToolMeta`):
  - `glob` → `N matches`(截断提示行 → `N+ matches`;`No files matched` → `no matches`);
  - `list_dir` → `N entries`(空目录 → `empty`);
  - `read_file` → `L{首}–{末}`(cat -n 真实行号,含 offset;含
    `<truncated: omitted …>` 时追加 ` · truncated`;读图 → `image W×H`;
    非行号输出兜底 `N chars`)。
  - 结果缺失(流式中)/ `isError` / 空输出 → `null`,槽位留空。
- 单复数:1 时 `1 match` / `1 entry`。

### 硬约束

- **截断提示行不进计数**:`glob` 的 `(...and N more matches…)` /
  `(showing the N most recent…)`、`list_dir` 的 `(...N more entries hidden…)`
  都以 `(` 开头,`splitItems` 剥除;存在即计数加 `+`。`+` 只声称"还有更多",
  不把被截掉的数量当精确值报出去(被截断时 N 未必是真实剩余量)。
- **不能对 read_file 输出用 `String.trim()`**:首行是 `\t1\t<正文>`,trim 会
  吃掉前导制表符让行号正则失配,范围从第二行报起(实证 bug `L2–4`)。
  用 `trimOuterWhitespace`(只去首部空行 + 尾部空白)。
- **meta 不报错误**:失败没有"规模"可言,错误由红卡 + ✗ 承载,展开看原文。

## 4. 交互与布局

- 收起态 = 1 行;整行是开关(`role="button"` + `tabindex=0` + Enter/Space,
  照 dispatch_subagent 先例;不用原生 `<button>` —— 里面装着 header 的 div)。
  `aria-expanded` 反映状态。
- **收起态输出不进 DOM**(`v-if`):长输出既不占高也不参与渲染。
- 展开区 = `<ToolOutputBody :collapsible="false">`(裸 pre:解 envelope →
  500 字截断 → linkify 路径 → 点击委托,与统一链路同源)+ `<ToolInputBody>`
  (原始 input JSON,调试面)。
- 高度预算(桌面 26px):卡片 padding 2px + 行 padding 2px + 行高 15.6px
  (`--leading-tight` × 12px)+ 边框 2px。**本卡把 `#title-meta` 所在的 title
  改成 `align-items: center`**(`:deep` 覆盖):baseline 对齐会把 14px 图标的
  底边压到文字基线上,凭空撑出 ~4px(实测 title 19.6px vs 最高子元素 15.6px);
  共享 header 的 baseline 节奏是通用卡 / 抽屉 / 权限卡共用的,不在本卡改。
- 移动端 padding 再收 1px;紧凑 chip 不适用 44px 触摸目标规则
  (`responsive-mobile.md` §6 DEC-6)。

## 5. 出口径不缩水(硬约束)

- **审批**:三工具都是权限层 Tier 4 的 **path 工具**(项目外路径 emit ask),
  命中 `pendingAsk` 时审批区**无条件渲染**,与收起/展开解耦 —— 一次误收起的
  点击不能把项目外读取的放行藏掉。接线逐行照抄 `ToolCallCard`(含"结果到达
  即 `clearPending`"的 120s 超时 toast 护栏)。
- **读图**:`result.images` + `sessionId` → `ToolResultImages` 常驻(缩略图
  就是结果本身)。
- **错误**:红左条 + `✗ error`,展开是后端错误原文。

## 6. 视觉

| 工具 | icon | accent |
|---|---|---|
| `read_file` | `document` | `--color-tool-read` |
| `glob` | `magnifying-glass` | `--color-tool-read` |
| `list_dir` | `folder` | `--color-tool-read` |

三工具同 accent(此前 glob / list_dir 吃 `--color-text-muted` 兜底,在一排
卡里读起来像"未知工具");`grep` 仍是通用卡 + muted(有意不加,PRD Non-Goals)。

## 7. 测试

| 层 | 文件 | 锁什么 |
|---|---|---|
| 单测 | `app/src/utils/toolSummary.test.ts` | chip / meta 全形态:命中、截断(+)、空、无命中、offset range、L1 制表符、图片、报错、畸形 input、裸文本兼容 |
| 单测 | `app/src/components/chat/ReadToolCard.test.ts` | headline 三工具、收起/展开/键盘、展开区解 envelope + 截断、审批三态、读图 |
| 单测 | `app/src/components/chat/ToolOutputBody.test.ts` | `collapsible=false` 变体(裸 pre / 保留链路 / 默认仍是 details) |
| e2e | `app/e2e/tool-card-compact.spec.ts` | **高度门 ≤34px**(基线 85px)+ 不出现通用 `.tool-card` + 展开/收起 + 报错展开 |

改高度相关 CSS 后必须重跑 e2e `tool-card-compact.spec.ts`(jsdom 量不出布局,
单测永远绿)。
