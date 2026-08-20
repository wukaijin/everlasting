# PRD — token-usage 用量弹层迁移(QuotaChip 并入 input hint chip)

> 2026-08-21 用户对话内直接提出(08-20-turn-usage-event-quota-view 的 UI follow-up,非 brainstorm 流)。
> 用户原话要点:panel 放到 input 下方 token-usage 的弹出 popover;变大;含上下文容量和占用、
> 平均缓存命中率、聚合展示;清晰可视化(上下文占用进度条 + 明细列表)。

## Goal

单入口用量仪表盘:点击 ChatInput hint 行的 token chip,弹出大号(420px)向上弹层,
**取代** 原来两个分散入口:

1. 原 hover Tooltip(四计数明细,reka-ui);
2. 原 AppHeader `QuotaChip` chip + 280px 弹层(08-20 WP3,顶栏常驻)。

## 落地内容

- **新组件 `app/src/components/chat/ChatInputTokenUsage.vue`**(手写 popover 模式,
  popover-pattern.md 族;向上开、`left:50% + translateX(-50%)` 居中于 chip):
  1. **上下文占用** — 进度条(usageLevel 着色 ok/warn/alert)+ 大号 % + 已用/容量 + 剩余;
  2. **上轮明细** — input / cache_read / cache_creation / output + 上轮缓存命中率
     (`cacheRatePercent`,08-10 锁定语义;SessionTokenUsage 为 last-turn 快照);
  3. **Nh 滚动窗口聚合** — 窗口总量(设额度加占比条,AC5 语义保留)+ **平均缓存命中率**
     (Σcache_read / Σ(input+cc+cr),provider 同族不混故跨 provider 求和口径仍准)+
     per-provider(主/worker 拆分 + 各自命中率 + 小时分布柱)+ top sessions(点击跳转);
  4. **设置** — 窗口小时 / 额度(校验逻辑原样迁移)。
- **ChatInputHintRow**:Tooltip 移除,中位换 `<ChatInputTokenUsage>`;S6a 手机端隐藏
  规则改 `:deep(.chat-input__token)`(子组件根)。hint 行仍 0 store import。
- **AppHeader**:QuotaChip 挂载移除,`QuotaChip.vue` + 测试删除。
- `quota.ts` store 数据面零改动;刷新链不变(mount / 弹层开 / streamEvents done)。

## 验证

- 前端 `pnpm test` 1146/1146(新 `ChatInputTokenUsage.test.ts` 7 用例:进度条宽度与色档、
  明细行、两处命中率、额度条两态、空态、设置本地校验);`vue-tsc` 0 err。
- 真机视觉:daemon serve dist + headless Chromium 点击截图 —— 弹层居中无裁切、进度条 /
  明细 / 命中率 / 小时柱 / provider 拆分均渲染;移动端 390px chip 隐藏(S6a)生效。
- 排障插曲:截图初版弹层报 `[httpTransport] 405` —— 定位为**旧 daemon 二进制缺
  usage 路由**(08-20 01:11 提交,当时在跑的 release 二进制早于它;curl 对照
  sessions/list_sessions 422 vs usage_window 405 实锤),换新编译二进制后 200,非前端问题。

## Out of Scope

- 手机端用量入口(S6a 将 token chip 归为开发者调试信息隐藏;如需移动端入口另开任务)。
- worker per-turn live 推送、$ 换算(沿 08-20 边界)。

## 增量(同日第二轮):上下文构成可视化

用户追加:占比(消息/工具/技能/系统提示词)加进弹层;嫌 TurnCard 的 hover 构成条样式不好,**重新设计、不要悬停显示**。

- 数据:traceStore 当前 session 最近一轮带 usage 的 TurnTrace(五归因切片字段全齐);
  消息 = context_input − Σ切片(残差,budget.rs 不变量);「技能」无独立列,归并在
  system 内(图例标注「system+技能」+ 底注)。
- 呈现:进度条升级为**构成堆叠条**(外层轨道 = 容量,内层宽 = 实发占比,段按值 flex
  分配、段间 2px 缝);下方**常显图例**(色点 + 标签 + token + %,四列 grid,零悬停)。
  分类配色与 TurnCard 锚点一致(tools 紫/system 琥珀/memory 蓝/@文件 绿/图片 青),
  消息残差 = 中性 slate(text-primary 45% color-mix)。
- 取数时机:弹层打开时若 traceStore 未 scoped 到当前 session(冷启动)补一次
  loadHistory;live 由 turn_usage 事件维护。
- 退化:切片全缺(旧行)→ 单色档位条 + 无图例。
- 验证:测试 +2(构成渲染/残差/退化),全量 1148 绿;vue-tsc 0;真机截图复核。
