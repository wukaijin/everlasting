# design — N7 DiffView 增强:行内高亮 + side-by-side

> 任务:`09-26-diffview-enhance`。需求与裁定见 [prd.md](./prd.md)。
> 纯前端任务:不改 wire/DB/IPC,回滚 = revert commit。
> 2026-09-27 群聊评审(session `c5460c4b`,14 结论全 verified)回填;结论 10 的 PRD 修正案
> (allow-split prop)经任务侧采纳(依据 = 用户 Q2 裁定前提「窄入口天然更稳」+ 缺陷实证,
> 一行可回退)。评审转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-27-评审任务规划 09-26-diffview-enhanceN7 DiffVie-c5460c4b.md`。

## 1. 模块结构

```
app/src/utils/intraLineDiff.ts        新增 — word-diff 配对 util(纯函数,双组件共享)
app/src/utils/intraLineDiff.test.ts   新增 — util 单测
app/src/components/chat/DiffView.vue  改造 — 行内高亮 + split 渲染 + 工具行切换 + allow-split prop
app/src/components/chat/DiffView.test.ts 新增 — 组件单测(此前无专属测试)
app/src/components/chat/EditFileCard.vue  改造 — 行内高亮(接 util)
app/src/components/chat/EditFileCard.test.ts 增补
app/e2e/diffview-split.spec.ts        新增 — route-mock e2e(双栏 + 行内片段 + 切换)
```

不动:`DiffModal.vue`、`DiffPrimitive.vue`、`MockPrimitive.vue`(props 零改动);`ToolCallCard.vue` **仅一行 opt-out**(结论 10,见 §3.4a)。

## 2. word-diff util(`intraLineDiff.ts`)

```ts
export interface WordSeg { text: string; changed: boolean }

/** 低层:一对字符串的 word-diff。null = 调用方应退化为整行染色。 */
export function pairWordSegments(a: string, b: string): { a: WordSeg[]; b: WordSeg[] } | null;

/** 高层:相邻 del-run/add-run 配对。返回与入参逐行对应的 segments(null 行 = 整行染色)。 */
export function pairRunSegments(
  delLines: string[],
  addLines: string[],
): { del: (WordSeg[] | null)[]; add: (WordSeg[] | null)[] } | null;
```

- 算法:`diffWordsWithSpace`(jsdiff v9 已有,零新依赖)。`changed` = seg 带 `added`/`removed` 标记。
- **原子 null 契约(结论 2/3/5)**:`pairRunSegments` 对整个 run 返回 null(整 run 退化整行染色),三条触发路径:①join 后任侧长度 > `MAX_PAIR_LEN = 4000`;②`diffWordsWithSpace` 抛错(catch);③重分布后一致性校验失败(防御分支)。**不采纳**「尾部 null」部分退化案——重分布必然重建全部行,普通路径不产 null;split 行模型的 zip+补位天然表达「del3/add2 后缀形态」,该担忧转化为 del3/add2 钉死测试。
- **重分布算法(结论 2,修正原「累计字符数」方案)**:多行块 = 两侧按 `\n` join → word-diff → **按 `\n` 切分推进**逐段派发到对应行(seg 含 `\n` 就切开)。
- **util 级不变量(钉为断言)**:①两侧输出数组长度 == 入参数组长度;②每行 segments 的 text 拼接 == 归一化(`\r` 已在消费点剥除)后的该行文本。测试语料必含:空行对 / 行尾空白 / CRLF 输入 / del3+add2 不等长。
- 纯函数无状态;**CRLF 归一不放进 util**(结论 6,保契约纯净),见 §3.1/§4。
- **性能护栏补充(结论 5)**:MAX_PAIR_LEN=4000 之外,view 级新增 **run 数预算帽 ≈200**(保险丝性质,超帽 run 走同一 null 退化,防病态大 diff 逐 run 全算)。数值由实现落定时写回本文件。

## 3. DiffView 改造

### 3.1 行内高亮(unified)

`HunkLine` 增字段 `segments: WordSeg[] | null`(默认 null;仅 add/del 且配对成功非 null)。`parsedFiles` computed 内,flat 行构建后做一次 run 配对 pass:

- 极大连续 del-run 紧跟极大连续 add-run → `pairRunSegments(delTexts, addTexts)` 回填两侧行 segments;超 run 数预算帽的 run 跳过(整行染色)。
- del-run 后无 add-run / add-run 前无 del-run(纯删除/纯新增)→ 不配对,整行染色(现状)。
- ctx/hunk/noeol 行不参与。
- **CRLF 归一(结论 6)**:hunk 行文本构建点(本组件内)归一到 LF 再进配对;渲染仍用原行文本不变。

渲染:`diff-line__text` 内 `segments` 存在时逐 seg 渲染 `<span class="diff-mark diff-mark--add|del">`(changed 才加 mark 类),文本串接 == 原行文本(复制/选择语义不变);否则维持 `{{ line.text }}`。`white-space: pre` 不变。

### 3.2 split 行模型

```ts
interface SplitCell { lineNo: number | null; text: string; segments: WordSeg[] | null }
interface SplitRow { left: SplitCell | null; right: SplitCell | null; kind: "pair" | "ctx" | "hunk" | "noeol" }
```

`splitRows(hunk): SplitRow[]` 从同一 flat `HunkLine[]` 派生(与 unified 共用一次 parse,零二次解析):

- ctx → `{ kind:"ctx", left, right }` 同行号对齐。
- del-run + add-run → zip 逐行并排,**短侧补 `null` 占位**(空 tinted 格,视觉对齐;del3/add2 形态由此天然表达)。
- 纯 del-run / 纯 add-run → 单侧,对侧 null 占位。
- hunk 头 / noeol → 通栏行(kind 沿用,渲染跨全宽)。

### 3.3 split 渲染与样式

- 行网格:`grid-template-columns: 44px minmax(0,1fr) 44px minmax(0,1fr)`(各栏自带行号 gutter,无 +/- 前缀列,色即语义)。
- 栏内 `white-space: pre-wrap; overflow-wrap: anywhere`——长行 wrap 不裁切、不横滚(与 unified 的 `pre` + 行级横滚并存,两档各自策略)。
- **`align-items: start`(结论 4,必改)**:现状 `.diff-line` 是 `align-items: baseline`——wrap 多行后基线对齐会让行号 gutter 错位,split 行必须 `start`(单侧 wrap 时顶对齐;配「单侧 wrap 顶部对齐」组件测试)。
- del 侧行底 del tint、add 侧 add tint、null 占位格同样 tint(对齐线索)。
- 行内高亮 seg 在 split 内同样渲染(`diff-mark` 复用)。
- PR2 合入前 `ui-review.sh --screenshots-only` 实看 wrap 密度(结论 4)。

### 3.4 工具行切换 + 持久化 + 窄屏降级

- `files.length > 0` 且非下述缺席条件时,diff-view 根部渲染工具行(右对齐):两个 `btn btn--ghost` 小按钮「单栏」「双栏」,`aria-pressed` 表态。
- **工具行三缺席条件(结论 7),各配独立组件测试防相互遮蔽**:①raw-only(无任何 parsed 文件)不渲染;②窄屏 CSS 藏整行(走 mobile-hide 惯例);③`allow-split=false` 不渲染。
- `userMode = ref<"unified"|"split">`,初值 = localStorage(**key `everlasting:diffview.mode`**,结论 9;非法值/读失败 → unified)。
- **localStorage 三纪律(结论 9)**:①仅用户点击时写(读路径永不写回);②测试间清键;③remount 断言用新建挂载。**同屏多实例不联动为最终裁定**(架构撤回单例联动建议)——各实例挂载时读一次,后续仅自身点击可变。
- `isNarrow`:**matchMedia + change 监听选树(结论 8)**,不做 CSS 双树渲染(成本不可接受);jsdom 无 matchMedia 需 stub(ModeSelect.vue:134-138 先例)+ unmount 移除监听断言;真窄屏语义交 Playwright `setViewportSize`。responsive spec 备案例外「matchMedia 仅用于选树,不用于显隐」。
- `effectiveMode = computed(() => userMode === "split" && !isNarrow && allowSplit ? "split" : "unified")`。

### 3.4a `allow-split` prop(结论 10 + OQ1 采纳,评审实锤缺陷的修法)

**缺陷**:全局单键 + 视口降级存在穿透缺口——桌面宽视口(≥768px)下,ToolCallCard 的 inline diff 容器只有 ~250px 宽,视口级降级保护不到;用户全局切 split 后 inline 卡重挂载会把双栏灌进窄栏。

**修法**:DiffView 新增可选 prop `allowSplit: boolean = true`(缺省行为与原方案完全一致);`ToolCallCard.vue` 挂载点传 `:allow-split="false"`(inline 恒 unified,一行改动)。`false` 时:effectiveMode 恒 unified + 工具行不渲染。PRD R7/AC6 已按此修正(prd.md 2026-09-27 版)。

### 3.5 raw fallback 分支

逐字节不动(R4):raw 分支永远单栏、无行内高亮;RULE-FrontDiff-001 契约与既有锚测试不翻。

## 4. EditFileCard 改造

- `diffRows` computed 之后增 `rowsWithSegments` computed:同样 run 配对 pass(逻辑与 DiffView §3.1 同构),调 `pairRunSegments`。
- **CRLF 归一(结论 6)**:行拆分点改 `split(/\r?\n/)`(EditFileCard.vue:112 一带);**口径自查(评审 OQ3)**:`truncated` 启发式按 `split("\n")` 计行数(EditFileCard.vue:134-135),归一须同口径(归一后计数基线一致),实现时核对并写测试。
- **截断边界(结论 13,钉死测试)**:配对 pass 必须跑在**截断后**的 rows 上——截走 add-run 的 del-run 自然整行染色;钉「截断点恰在 del-run/add-run 之间」用例,防未来把配对挪到截断前。
- 渲染:文本格 segments 存在时逐 seg `<span class="edit-diff-mark edit-diff-mark--add|del">`;样式与 DiffView 同 tint 族(§5)。
- 不动:审批态/错误态/MAX_ROWS 400 帽/折叠行为/自有样式体系。

## 5. 色(R6 自裁 + 结论 12 可辨性纪律)

沿既有行底色族同 hue 加深,scoped 内联,不入全局 token 表(generative-ui spec「diff 染色同色族不立新 token」先例):

```css
.diff-mark--add   { background: rgba(16, 185, 129, 0.28); }          /* 行底 0.12 加深 */
.diff-mark--del   { background: color-mix(in srgb, var(--color-tool-error) 28%, transparent); }
```

EditFileCard 用同名 `edit-diff-mark--*`(同色值,注释标同族)。**AC1 只做 DOM 断言(mark span 存在 + 数量),不依赖 VLM 报数**;PR1 合入前 `ui-review.sh --screenshots-only` 人眼对照,不可辨则调 CSS,不阻塞结构验收。

## 6. 挂载面兼容

- `FileDiff` interface export 不变;props **仅增** `allowSplit`(默认 true,缺省行为不变);`DiffModal.vue`/`DiffPrimitive.vue`/`MockPrimitive.vue` 零改动,`ToolCallCard.vue` 一行 opt-out(§3.4a)。
- `MockPrimitive.vue` 若直接引用 DiffView 内部类名做断言需核对(预期无)。
- ToolCallCard inline(虚拟行内)与 DiffModal(Teleport)均无层级/尺寸新假设;inline 恒 unified(§3.4a)后窄入口风险封死。

## 7. 测试策略

| 层 | 文件 | 关键用例 |
|----|------|---------|
| util | `intraLineDiff.test.ts` | 单行对/多行 join+`\n` 切分推进重分布/空行对/行尾空白/CRLF 语料/del3+add2/纯增删不进/超长护栏 null/异常 catch null/**不变量断言(长度相等 + 拼接还原)** |
| 组件 | `DiffView.test.ts`(新) | unified 行内 seg 出现且文本串接=原行;切换后 split 网格行/占位格;不等长 run 对齐;localStorage 记忆(**新建挂载 remount 恢复,读路径不写回**);matchMedia stub 窄屏恒 unified + **unmount 移除监听**;raw 分支断言不变;折叠交互不回归;**工具行三缺席条件各自独立用例(raw-only / 窄屏 CSS 藏 / allow-split=false)**;**allowSplit=false 恒 unified**;**单侧 wrap 顶部对齐**(split) |
| 组件 | `EditFileCard.test.ts` | 修改行 mark span;既有审批/错误/截断用例全绿;**截断点在 del/add run 之间的形态**;无 `\r` 断言 |
| e2e | `diffview-split.spec.ts` | route-mock(无 daemon,CI 确定性):打开 diff 面 → 切双栏 → 双栏结构 + 行内 mark 断言;**不得照抄 checkpoint-revert.spec.ts:265 的 `diff_text:""` 空 fixture**(会使 DiffView 走 parsed=false 分支、断言恒真)——fixture 必含 ctx 行 + 不等长 del/add run,并做**空 fixture 负控**(先确认负控用例会红,再换回正控);真窄屏语义 `setViewportSize` |

## 8. PR 拆分

- **PR1 — 行内高亮**:`intraLineDiff.ts` + DiffView unified seg + EditFileCard + util/组件测试 + `--screenshots-only` 人眼对照。
- **PR2 — side-by-side**:split 行模型 + 渲染(`align-items:start`)+ 工具行(三缺席条件)+ localStorage + 窄屏选树 + `allow-split` prop + ToolCallCard opt-out + e2e + `--screenshots-only` wrap 密度实看。

## 9. 回滚

纯前端两个 PR,`git revert` 即回;无迁移、无 wire 变更、无配置面新增。allow-split prop 缺省 true,回退 ToolCallCard 一行即恢复旧观感。

## 10. 评审待验点 → 裁决记录(2026-09-27 回填,原五点全部定稿)

1. 多行块重分布:**改 `\n` 切分推进**(弃累计字符数),不变量钉 util 断言;边界语料入测。
2. split 换行:**采纳 pre-wrap+anywhere**,必配 `align-items:start` + 单侧 wrap 顶对齐测试 + 截图实看。
3. `pairRunSegments` null 语义:**整 run 原子 null 三路径**(超长/抛错/校验败)+ view 级 run 数预算帽 ≈200;尾部 null 案不采纳(转 del3/add2 钉死测试)。
4. localStorage:**不走 store 维持**(useTheme/config 先例),三纪律 + key 前缀 `everlasting:`;同屏不联动为最终裁定。
5. 窄屏 UX:**matchMedia 选树**(CSS 双树否决),jsdom stub + unmount 断言 + Playwright 真窄屏;spec 备案例外。

新增实锤(非原五点):**全局单键穿透视口降级**(§3.4a allow-split prop)、**e2e 空 fixture 恒真陷阱**(§7)、**EditFileCard 截断边界钉测**(§4)、**AC1 色可辨性不依赖 VLM**(§5)。
