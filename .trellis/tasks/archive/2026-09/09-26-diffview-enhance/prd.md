# N7 DiffView 增强:行内高亮 + side-by-side

> 来源:BACKLOG 附录 B 候选 N7(P2,前端视角;2026-09-05 群聊 session `082add5c` 共识)。
> 2026-09-26 立项,brainstorm 三问收敛(决议见 Requirements 各条)。
> 原始表述「行级高亮 / side-by-side / 按文件折叠」;勘察修正后实际缺口 = 行内(word-diff)高亮 + side-by-side 双栏。

## Goal

升级 DiffView 的审阅质量面:行内 word-diff 高亮 + side-by-side 双栏视图,让「本轮 diff / session diff / edit_file 卡 / B9 diff 卡」的变更审阅达到主流 diff 工具的判读效率——行级红绿整行染色只能看出「哪行变了」,看不出「行内什么变了」。EditFileCard(edit_file 审批面)随批升级行内高亮。

## 背景与痛点

- `DiffView.vue`(B9 期 MVP,07-02)只有单栏 unified + 整行染色 + 文件折叠;共识表第三项「按文件折叠」已存在,不重做。
- N2 checkpoint/revert(09-20)交付后,DiffView 成为变更信任闭环的核心审阅面(6 个挂载点),渲染质量与功能地位不匹配。

## 勘察结论(2026-09-26 已核实)

- **挂载面 6 入口全免费受益**:`DiffModal.vue`(session diff 弹窗 + checkpoint「本轮 diff」复用,Teleport body)、`ToolCallCard.vue:687`(edit_file inline diff,在 MessageList 虚拟行内)、`primitives/DiffPrimitive.vue`(B9 use_ui diff 卡)、`primitives/MockPrimitive.vue`(测试)。
- **jsdiff v9 已在依赖**(`diff: ^9.0.0`),`diffWordsWithSpace` 可用,行内高亮零新依赖。
- **DiffView 无专属单测**;间接覆盖 = `DiffPrimitive.test.ts` raw fallback 契约(RULE-FrontDiff-001,见 spec `frontend/chat/generative-ui.md`)。
- **EditFileCard** 独立自渲染(jsdiff `diffLines` 行级、自有 `edit-diff-line` 样式、MAX_ROWS 400 帽、审批态/错误态自洽),不走 DiffView。
- **responsive-mobile spec**:单一断点 768px,PWA 窄屏真实场景——side-by-side 需降级。
- **色约束**:现有 diff 行底是 scoped rgba(16,185,129,0.12)/color-mix 12%(generative-ui spec 钉过「同色不引入新 token」);行内强调色沿同族加深(裁决见 R6)。
- **localStorage 先例**:`config.ts` LAST_ACTIVE_PROJECT 模式(同步读 + try/catch 防私隐模式);state-management spec 无 UI 偏好强制走 store 的条款。
- N4 虚拟化只管 MessageList 行;DiffModal 已 Teleport 不受影响;ToolCallCard inline diff 默认折叠,无新增风险面。

## Requirements

- **R1 行内高亮(决议①同批)**:unified 与 split 两档均生效。hunk 内相邻 del-run/add-run 配对做 word-diff(jsdiff `diffWordsWithSpace`),行内变更片段用加深 tint 标记;纯新增/删除(无配对)行保持整行染色;ctx 行不参与。护栏:超长行跳过行内计算。
- **R2 side-by-side(决议①同批)**:双栏(旧文左/新文右),行号各栏自带;ctx 行两侧对齐,del 仅左、add 仅右,行数不等的块短侧补空占位保持视觉对齐;无 +/- 前缀列(色即语义);栏内 `pre-wrap` 换行不横滚(长行不裁切)。
- **R3 档位切换(决议② + 评审结论 9/10 修正)**:DiffView 顶部工具行切换(每实例一次、非 per-file);默认 unified(与现状零现差);选择记 localStorage(key `everlasting:diffview.mode`,config.ts 防御模式),**重挂载后生效(跨入口),同屏已挂载实例不联动**;<768px 实际渲染恒 unified;工具行三缺席条件 = raw-only / 窄屏 CSS 藏 / `allow-split=false`,各配独立测试。
- **R4 raw fallback 分支不动**:LLM 风格无头片段(RULE-FrontDiff-001)保持单栏、无行内高亮、逐字节等价;切换控件不影响该分支。
- **R5 EditFileCard 升级(决议③)**:接共享 word-diff util,审批预览行内高亮;不做 split、不重构挂 DiffView;审批态/错误态/MAX_ROWS 行为零回归。
- **R6 行内强调色(自裁,勘察依据 + 评审结论 12)**:沿既有 scoped rgba 色族加深(add 0.28 / del color-mix 28%),不进全局 token 表;AC 断言只做 DOM 层(mark span 存在 + 数量),色可辨性走 screenshots-only 人眼对照,不依赖 VLM。
- **R7 接口最小增(评审结论 10 修正,原「零变更」案不成立)**:props 仅增可选 `allowSplit: boolean = true`(缺省行为不变);`DiffModal`/`DiffPrimitive`/`MockPrimitive` 零改动;`ToolCallCard` 一行显式 opt-out(`:allow-split="false"`,inline 恒 unified)——堵评审实锤的「全局单键穿透视口降级」缺口(桌面宽视口下 ~250px inline 容器不受 768px 视口降级保护)。
- **R8 测试**:DiffView 专属单测(行内配对正确性/split 对齐模型/切换交互/localStorage 记忆/窄屏降级/工具行三缺席/allow-split/raw 与折叠不回归);EditFileCard 补断言(含截断边界钉测);e2e route-mock 用例(双栏结构 + 行内片段 + 切换持久化;空 fixture 负控防恒真)。

## Acceptance Criteria

- [ ] AC1 unified 档:修改行的行内变更片段以加深 tint 渲染(`diffWordsWithSpace`);新增/删除无配对行保持整行染色无片段;多行块配对正确。
- [ ] AC2 split 档:旧文左/新文右;ctx 两侧同行号对齐;del 仅左 add 仅右;不等长块短侧空占位;行内高亮在 split 同样生效;长行 wrap 可见不裁切。
- [ ] AC3 切换:工具行可切单/双栏;默认 unified;**重挂载后(含跨入口)记忆生效,同屏已挂载实例不联动**;视口 <768px 恒渲染 unified;**inline 面(ToolCallCard)经 `allow-split=false` 恒 unified**。
- [ ] AC4 raw fallback:与现状行为等价(单栏/行首字符分类/无行内高亮),RULE-FrontDiff-001 锚测试不翻。
- [ ] AC5 EditFileCard:预览行内高亮正确;审批/错误/截断既有测试全绿;**截断点落在 del/add run 之间的形态有钉死用例**。
- [ ] AC6 接口最小增:props 仅增可选 `allowSplit`(默认 true);`DiffModal`/`DiffPrimitive`/`MockPrimitive` 零改动;`ToolCallCard` 一行 opt-out。
- [ ] AC7 测试全绿:vitest 全量 + vue-tsc 零错 + e2e(route-mock,CI 确定性)新用例过(含空 fixture 负控)。
- [ ] AC8 性能护栏:超长行(MAX_PAIR_LEN=4000)与超 run 数预算帽(≈200)均走整行染色退化;既有 max-height 480px 内滚与文件折叠行为不回归。

## Out of Scope

- 共识表「按文件折叠」(已存在)。
- diff 语法高亮(syntax highlight)。
- worker Merge 前 diff 联动(subagent-drawer.md 显式 follow-up)。
- EditFileCard 挂 DiffView 重构 / EditFileCard split。
- 大 diff 渲染 F1 基准(Q5 自裁不做;N9 方法在,后补成本低)。
- raw fallback 双栏化(无行号语义,无意义)。

## Decisions(2026-09-26 brainstorm;2026-09-27 群聊评审回填)

| # | 决议 | 裁定 |
|---|------|------|
| Q1 | 行内高亮 + side-by-side 是否同批 | **同批**(拆 2 PR 分步验收) |
| Q2 | 切换交互 | 工具行切换;默认 unified;localStorage 全局记忆;<768px 强制 unified |
| Q3 | EditFileCard | 升级行内高亮(共享 util);不挂 DiffView 不做 split |
| Q4 | 强调色路径 | 自裁:既有色族加深 scoped rgba,不立全局 token;断言只做 DOM 层 |
| Q5 | F1 bench | 自裁:不做,留 out-of-scope |
| 评审 OQ1 | allow-split prop 修正案(与原 R7/AC6「零变更」冲突) | **任务侧采纳**(2026-09-27):依据 = 用户 Q2 裁定前提「窄入口天然更稳」+ 评审实证缺陷(视口降级穿不进 inline 窄容器);缺省 true 行为不变,一行可回退 |
| 评审 OQ2 | 预算帽数值 | 采纳评审建议:run 数帽 ≈200 / MAX_PAIR_LEN 4000(实现落定时可微调并写回 design) |
| 评审 OQ3 | EditFileCard `\r` 归一口径 | 实现自查项:归一与 `truncated` 启发式计行口径一致,写进测试 |

> 评审出处:session `c5460c4b`(2026-09-27,review preset,14 结论全 verified),转录
> `~/.local/share/dev.everlasting.app/discussions/2026-09-27-评审任务规划 09-26-diffview-enhanceN7 DiffVie-c5460c4b.md`。
> 其余 11 条结论(重分布 `\n` 推进 / 原子 null 三路径 / align-items:start / CRLF 消费点归一 / 工具行三缺席 /
> matchMedia 选树 / localStorage 三纪律 / e2e 空 fixture 负控 / EditFileCard 截断钉测 / AC1 不依赖 VLM /
> spec 备案两条)已直接回填 design.md 各节,不在此重复。
