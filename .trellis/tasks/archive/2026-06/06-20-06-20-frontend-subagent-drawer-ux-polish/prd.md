# FT-F-004 — SubagentDrawer UX polish bundle (C1+C2+C3)

> **状态**:**planning → grill 完成(2026-06-21),待 `task.py start` 进 in_progress**
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-004
>
> **Origin**:2026-06-20 截图分析(Session 50,user 主动提供主面板 + drawer 截图)
>
> **Grill**:2026-06-21 Session 54 grill-me 拷问,5 Open Questions + 1 prd 过时点全解
>
> **不依赖**:FT-F-001 typed-cards(已 closed,`6bb5060`);不依赖后端改动;不依赖 drawer 重构
>
> **打包**:1 bundle PR(C1+C2+C3 单文件)

---

## Goal(一句话)

把 2026-06-20 截图分析暴露的 drawer UX 优化打包成单 PR,经 2026-06-21 grill 拷问后从 4 项收窄为 **3 项独立小改**(C1 宽度 / C2 时间格式化 / C3 事件计数),全部在 `SubagentDrawer.vue` 单文件,改动 < 30 行 + 1 个 formatTime helper。

---

## ⚠️ prd 校准(grill 揭示的过时点)

prd 初稿基于 **FT-F-001 typed-cards 落地前**的 `<pre>` payload 现状写。但 FT-F-001 已 closed(`6bb5060`),typed-cards stage 2 已把 `SubagentDrawer.vue` 里的 `<pre>` payload **彻底移除**(见 line 192-197 注释),`word-break: break-all` 现在活在**共享组件** `ToolInputBody.vue:73` + `ToolOutputBody.vue:127` 里。这两个共享组件被 **drawer + chat 主区 `ToolCallCard` 双方共用**,改它们 = blast radius 扩到主区。

**影响**:prd C1 原方案"改 `SubagentDrawer.vue:450` 的 pre overflow-x"已**失效**(那个 pre 不存在了)。C1 经 grill 重新定义为纯宽度改动(见下)。C2/C3/C5 不受影响(prd 仍准确)。

---

## 现状(2026-06-20 截图 + 2026-06-21 代码核对)

| ID | 问题 | 影响 | 代码位置 |
|---|---|---|---|
| **C1** | drawer `width: min(480px, 90vw)` 太窄,path 长字符串在 typed-card 的 `<pre>` 里 wrap 截断成 2-4 行 | drawer 内容行被 wrap 占,scroll 量翻倍 | `SubagentDrawer.vue:599` |
| **C2** | header meta 行 `开始/结束` 显示 **raw ISO8601** `2026-06-20T05:38:54.053+00:00`(UTC),又长又是 UTC | header meta 行被时间戳占满 | `SubagentDrawer.vue:464-470` |
| **C3** | drawer 无事件计数,打开不知道规模(10 events vs 200 events),也不知道默认藏了多少 chat 事件 | 缺规模感 + 缺"展开 chat"引导 | filter-row `SubagentDrawer.vue:478-493` |
| ~~C5~~ | ~~底部 scroll 渐变~~ | **drop(grill 决议)** | 见下"为什么 drop C5" |

---

## 3 项最终方案(grill 决议)

### C1 — drawer 宽度 480 → 640(纯 CSS,drop overflow-x)

**改法**:
- `SubagentDrawer.vue:599` `width: min(480px, 90vw)` → `width: min(640px, 90vw)`
- **1 行 CSS,blast radius = 0**(不动共享 ToolInputBody/OutputBody)
- 估算 ~1 行

**为什么 drop prd 原方案的 overflow-x**(grill 决议):
1. **prd 要改的那个 pre 已被 FT-F-001 删除** —— 改对象不存在
2. **break-all 对无空格 path 是正确的** —— CSS `break-word` 不把 `/` 当断点,path 会直接溢出;break-all 才能断 path
3. **overflow-x 打断 drawer 纵向阅读流** —— 用户本来在上下滚看 transcript,遇到 path 还得横向拖
4. **改共享组件 break-all→overflow-x 会扩 blast radius 到 chat 主区 ToolCallCard**,需主区回归
5. 加宽本身(480→640)就缓解了 wrap 行数(3-4 行 → 2 行),治本

**取舍**:
- 640px 在 1600px 屏占 40%(仍可接受,不挡太多 chat 区)
- 小屏由 `90vw` cap 自动保护(无需 JS 响应式)
- 不加到 720px+(接近 prd non-goal 的"不做全屏")

### C2 — header 时间格式化(双时刻 + 本地 HH:MM:SS + clock icon)

**改法**:
- 加 `formatTime(iso: string): string` helper:`new Date(iso)` 转**本地时区** → `HH:MM:SS`(padStart 2 位)
- header meta 行(line 464-470):`开始: {{ run.startedAt }}` → `<Icon name="clock" :size="11" /> 开始 {{ formatTime(run.startedAt) }}`,结束同理
- **保留开始 + 结束两个时刻**(grill 决议:用户要双锚点,不 drop 结束)
- 估算 ~15 行(helper + template)

**为什么本地时区不是 UTC 字符串截取**(grill 揭示的坑):
- raw ISO 带 `+00:00`(UTC),直接截字符串会显示 UTC 时间,跟用户本地差 8 小时
- 必须 `new Date(iso).getHours()`(本地)/`getMinutes()`/`getSeconds()` 转 local
- `getHours` 返回本地时区小时(WSL 环境即用户本地)

**为什么用已有 `clock` icon**(grill 决议):
- `clock`(heroicons ClockIcon)已在 Icon.vue registry(line 108),零新增
- 两时刻都用 clock,视觉统一,一眼看出是时间字段
- 不引 lucide Play/Flag(那需扩 registry +2 import +2 map)

**取舍**:
- 丢日期(同 session 内多次打开 drawer 不跨日,可接受)
- 丢毫秒(精度无意义,`05:39:05` 就够)
- 不做相对时间(`2 分钟前`)(drawer 多是历史回看,相对时间随时间推移意义递减,且与 status badge 的持续时长 suffix 概念混淆)
- 注:status badge 的 suffix(`done in 11.7s`)已表达**持续时长**,meta 双时刻补充的是**绝对时刻**,两者互补不冗余

### C3 — filter-row 事件计数(纯数字 + 修正副计数)

**改法**:
- filter-row(line 478-493)toggle 旁加 `<span>{{ visibleTranscript.length }} events</span>`
- 未勾 `Show chat events` 时追加 `· +{{ hiddenChatCount }} chat hidden`(X = `transcript.length - visibleTranscript.length`)
- 勾上后副计数消失(chat 已可见)
- 估算 ~5-8 行(template span + hiddenChatCount computed)

**为什么纯数字不是进度条**(grill 决议):
- drawer 是流式/live,总事件数 M **未知**(run 可能还在跑,事件还在涨)
- N/M 进度条会被读成"完成度",严重误导 —— 这是**累积量**不是**进度**

**为什么副计数是"未勾显 hidden"不是 prd 说的"勾时显 +chat"**(grill 揭示的逻辑反向):
- prd 原意"勾 Show chat events 时加 +N chat" —— 但**勾了之后 chat 已可见**,副计数无意义
- 正确引导:**未勾时**显示 `+X chat hidden`,告诉用户默认藏了多少,鼓励展开
- 勾上后副计数消失(已展开)

**为什么放 filter-row 不是 prd 说的 status badge 旁**(grill 决议):
- title-row 已挤满(badge + name + jump-latest + close),加计数更挤
- filter-row 有 toggle + truncated notice,计数随 toggle 变化,语义聚合

---

## 为什么 drop C5(grill 决议)

prd 原方案:`.subagent-drawer__body` 加 `mask-image: linear-gradient(...)` 上下各 8px 渐变。grill 揭示**两个技术错误 + 一个冗余**:

1. **`mask-image` 不会"自动只在 overflow 时生效"** —— prd 第 70 行声称"mask-image 自动 only-render-when-overflowed,无需 JS"是**错的**。CSS mask 只要元素渲染就一直生效。短 transcript(不溢出)时,mask 仍会把顶部/底部 8px 内容淡出,**错误淡化本就看得全的短内容边缘**。要"仅 overflow 时 mask"必须加 JS 监听 `scrollHeight > clientHeight`,scope 违背 prd "无 JS"卖点。
2. **mask 底部渐变会淡化 "↓ N new" sticky 按钮** —— drawer body 已有 `position: sticky; bottom: 8px` 的浮钮(line 552-557),正好落在底部 8px 渐变区,会被一起 mask 淡化,降可读性。
3. **drawer 已有动态滚动提示** —— `autoFollow`(新事件自动滚到底) + "↓ N new" 浮钮 + header jump-to-latest 按钮。mask 静态渐变是**冗余的次要提示**,却带上面两个副作用。

**结论**:drop C5,bundle 从 4 项降 3 项。

---

## 打包策略(grill 决议)

**1 bundle PR**(C1+C2+C3 合并):
- 3 项都在 `SubagentDrawer.vue` 单文件,逻辑同源(drawer 信息密度优化)
- 单人项目,micro-PR 的独立 revert / 独立 review 价值低
- 拆同文件多 PR 有合并冲突风险(C2 改 meta 行 / C3 改 filter-row,虽不同行但同文件)
- Trellis 单 task 单 PR 收尾流程顺

---

## Non-goals(明确不做)

- **不动 drawer 内容渲染**(FT-F-001 typed-cards 范围,本 task 只调样式/文案)
- **不动 drawer header 文字以外的内容**(summary / status badge / banner)
- **不动 drawer 行为**(race / waiting / retry polling —— FT-F-002/003 范围)
- **不做 drawer 全屏模式**(留作未来 PM 决策)
- **不做 drawer 宽度用户可调**(SettingsModal 加 slider 留独立 task)
- **不做 C5 scroll 渐变**(grill drop,见上)
- **不动共享 ToolInputBody/OutputBody**(C1 blast radius 归零的关键)

---

## 测试范围(implement/check 阶段)

- **C2 `formatTime` unit test**(新增 `SubagentDrawer.test.ts` 或抽 utils 测试):invalid iso → fallback / UTC→local 转换正确 / 缺 finishedAt(结束 span 不渲染)/ padStart 2 位
- **C3 render test**:`visibleTranscript.length` 计数正确 / 未勾显 `+X chat hidden` / 勾上副计数消失
- **C1**:纯 CSS width,无逻辑测试(vue-tsc + 既有 render test 不破即可)
- 既有 20 个 SubagentDrawer.test.ts 全过(FT-F-005 留下的基线)

---

## 启动 checklist(进入 in_progress 时)

- [x] 走 grill-me 拷问 5 Open Questions + 1 prd 过时点(2026-06-21 Session 54)
- [x] 决定 bundle vs micro-PR → 1 bundle PR
- [x] 同步更新本 prd.md 把占位段全替换为 grill 产物
- [ ] Phase 1.3 curate `implement.jsonl` + `check.jsonl`(workflow.md 要求)
- [ ] `task.py start` 进 in_progress

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-004(open)
- **截图分析**:Session 50
- **关键文件**:`app/src/components/chat/SubagentDrawer.vue`(856 行,改动 < 30 行 + formatTime helper)
- **测试文件**:`app/src/components/chat/SubagentDrawer.test.ts`(37KB,20 test 基线)
- **不依赖**:FT-F-001(closed)/ FT-F-002 / FT-F-003 / FT-F-005(独立 task)
- **共享组件(不动)**:`ToolInputBody.vue` / `ToolOutputBody.vue`(drawer + 主区 ToolCallCard 共用)
- **同源 family**:
  - FT-F-005(drawer failed state banner,closed `586d4a5`)
  - FT-F-001(typed-cards,closed `6bb5060`)
  - FT-F-002(toast fallback,planning)
  - FT-F-003(workerWaiting ref leak,closed `272fbe9`)
