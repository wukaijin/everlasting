# implement — N7 DiffView 增强

> 前置:prd.md(需求/AC/决议含评审回填)、design.md(§1-§10,评审 14 结论已回填)。PR 划分见 design §8。

## PR1 — 行内高亮

- [ ] 1. `app/src/utils/intraLineDiff.ts`:`pairWordSegments` + `pairRunSegments`(join → word-diff → `\n` 切分推进重分布;原子 null 三路径;design §2)。
- [ ] 2. `app/src/utils/intraLineDiff.test.ts`:design §7 util 行全量(含空行对/行尾空白/CRLF 语料/del3+add2/**不变量断言:输出长度==入参长度 + 逐行拼接==归一后行文本**)。
- [ ] 3. `DiffView.vue`:`HunkLine.segments` + run 配对 pass(run 数预算帽 ≈200)+ CRLF 消费点归一 + `diff-mark` 渲染(design §3.1/§5);unified 其余行为零改动。
- [ ] 4. `EditFileCard.vue`:`rowsWithSegments` + `split(/\r?\n/)` 归一(与 truncated 计行口径核对,design §4)+ `edit-diff-mark` 渲染。
- [ ] 5. `DiffView.test.ts`(新建,PR1 范围:unified seg/拼接还原/raw 不回归/折叠)+ `EditFileCard.test.ts` 增补(修改行 mark span/**截断点落在 del/add run 之间钉死用例**/无 `\r` 断言)。
- [ ] 6. 验证:`cd app && pnpm test` 全绿;`pnpm build`(vue-tsc)零错;`bash scripts/ui-review.sh --screenshots-only` 人眼看 diff 面色可辨(不可辨调 CSS,不阻塞结构)。

## PR2 — side-by-side

- [ ] 7. `DiffView.vue` split 行模型 `SplitRow`/`splitRows` + split 网格渲染(`align-items: start` 必改 + pre-wrap/anywhere + 占位格,design §3.2/§3.3)。
- [ ] 8. 工具行切换 + localStorage(`everlasting:diffview.mode`,三纪律)+ `matchMedia` 选树(ModeSelect 先例 + jsdom stub + unmount 监听清理)+ effectiveMode;**`allowSplit` prop(默认 true)+ `ToolCallCard.vue` 一行 opt-out `:allow-split="false"`**(design §3.4/§3.4a)。
- [ ] 9. `DiffView.test.ts` 补:split 网格/占位对齐(含 del3+add2)/localStorage 记忆(新建挂载 remount,读路径不写回)/窄屏恒 unified/工具行三缺席条件各自独立用例/allowSplit=false 恒 unified/单侧 wrap 顶部对齐。
- [ ] 10. `app/e2e/diffview-split.spec.ts`(route-mock):**不照抄 checkpoint-revert.spec.ts:265 空 `diff_text`**(恒真陷阱);fixture 含 ctx 行 + 不等长 del/add run;**空 fixture 负控先红后绿**;`setViewportSize` 真窄屏断言;CI 确定性(无 daemon/LLM/网络)。
- [ ] 11. 验证:`pnpm test` 全绿;`pnpm test:e2e`(真实 Chromium);`pnpm build`;`ui-review.sh --screenshots-only` 实看 split wrap 密度。

## 收尾

- [ ] 12. trellis-check 全量(final pass:frontend 包 spec Quality Check)。
- [ ] 13. spec update 两条备案(评审结论 8/14):①responsive-mobile.md forbidden 清单补 matchMedia 例外(「仅用于选树,不用于显隐」,DiffView 与 ModeSelect 同例);②generative-ui.md(或新节)进 diff 渲染契约(segments 不变量/原子 null 三路径/run 预算帽/MAX_PAIR_LEN/CRLF 消费点归一)。+ ROADMAP/BACKLOG 附录 B N7 划线记账。

## 验证命令

```bash
cd app && pnpm test                                  # vitest 全量
cd app && pnpm build                                 # vue-tsc + vite build
cd app && pnpm test:e2e                              # Playwright(真实 Chromium)
bash scripts/ui-review.sh --screenshots-only         # 视觉抽查(不花 VLM quota)
```

## 风险点/回滚

- 改动全部前端、两个 PR 独立可 revert(design §9);allow-split 缺省 true,回退 ToolCallCard 一行即恢复。
- 高风险文件:`DiffView.vue`(多挂载点共底)——护栏 = R7 最小增(缺省行为不变)+ raw fallback 锚测试不翻 + DiffModal/DiffPrimitive/MockPrimitive 零改动断言。
- `EditFileCard.vue` 是审批面:既有测试全绿是硬门槛;截断边界钉测防未来回归。
- 基线(2026-09-26 实测):vitest 2056 全绿、`pnpm build` 过——新增失败即本任务引入。
