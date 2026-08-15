# 前端配色与文字对比度修复(对比度专项评审 R1)

## Goal

基于 2026-08-15 定向对比度评审(样本页 + mmx vision 定性 + WCAG 数值三方交叉,证据已拷贝到 `research/`),修复 token 体系里"彩色当文字用"的对比度 FAIL 与 hover 感知过弱问题,并沉淀对应规范。目标:评审行动清单 P1/P2 全部落地,评审复跑缺陷可度量收敛,灰阶层级与桌面观感零回归。

## Background

- 评审证据:`research/contrast-report-20260815.md`(源 `out/ui-review/20260815-contrast/`,gitignored)+ 分章截图与 mmx 原始返回。方法已固化为 `.agents/skills/ui-review/`(Flow B)。
- 三方确认的 FAIL(数值,计算口径见报告):
  - accent `#3b5bdb` 作文字:surface 3.14 / elevated 2.87 / accent-muted 2.39;
  - tool-error `#ef4444` 作文字:elevated 4.32 / accent-muted 3.60;
  - text-muted × accent-muted 底:4.41(memory 卡片被 VLM 连续两轮点名)。
- C 章新发现:hover(6% primary wash)与 default 肉眼几乎不可辨——此前"看不见 hover"被归因于静态截图盲区,样本页复现后确认叠加本身过弱。
- A 章确认:muted + 11px mono 是感知最弱组合(数值 5.3-6.3 过 AA,双 VLM 独立判读"发灰/有停顿感")。
- **Out of scope**:elevated/border 层级感知专题(报告行动清单第 6 条,D4 被 mmx 驳回为"半步方案"),另启任务;灰阶 token 值本身不动(2026-08-05 刚调过,层级锚点);warn 黄"泥土色"观感不修(8.13 数值优先,记真机观察)。

## Requirements

### WP1 accent 文字色拆分(P1)

- R1.1 `style.css` `@theme` 新增 `--color-accent-text: #7c9aff`(6.71:1 on surface,AA)。
- R1.2 全量替换"accent 作**文字色**"的 call site 为新 token(基线 grep:20+ 处,含 `MessageItem.vue:1102` markdown 链接、DiscussionSummaryCard / EmptyProjectState / WorktreeChip / HiddenProjectsMenu / BrowserHeader / RuntimeMemoryModal 等;实现时以 `grep -rn "color: var(--color-accent)"` 重扫为准);`border-color` / `background` / focus-ring 等**非文字用途不动**。
- R1.3 链接 hover 行为沿用现状(本任务不新增交互),on-accent 场景(按钮内白字)不动。

### WP2 错误文字色拆分(P1)

- R2.1 新增 `--color-tool-error-text: #f87171`(6.43:1 on surface / 5.87 on elevated,AA)。
- R2.2 错误**文案**类 call site 换新 token(错误消息文本、error badge 文字);左色条、填充底、图标底色仍用 `#ef4444`(图形 3:1 达标)。
- R2.3 mmx 提示"纯深底上略有荧光感"(非硬性):实现后真机复核,若成立微调候选值一档并同步 token 注释。

### WP3 彩底文字规则 + memory 定点修(P2)

- R3.1 立规则:彩色/低亮度底(accent-muted、tool 色块底)上文字最低 `--color-text-secondary`,禁止 muted 与 accent 同色系文字(数值依据:紫 4.98 过线仍"沉底"、蓝 2.39)。
- R3.2 memory 相关彩底组件(RuntimeMemoryModal / MemoryPreview / MemoryLayerItem)的时间戳、标签、底部说明 muted→secondary 定点修。

### WP4 hover/active 叠加提强(P2)

- R4.1 `--color-bg-hover` 6%→10%、`--color-bg-active` 10%→14%(color-mix 源头改,全端级联;selected 12% 不动)。
- R4.2 真机(devtools / 实际 GUI)确认可辨且不显脏;过强则回落 8%/12%。C 章样本页复跑辅助验证。
- R4.3 量化规范沉淀:hover/active 相对 default 的"最小可辨 ΔL"写进 design-tokens.md(VLM 建议,采纳)。

### WP5 11px mono 元数据定向提升(P2)

- R5.1 高频注视位(侧栏分组头 `SESSIONS`/`本周`/`更早`、工具卡 meta 行、消息时间戳)由 muted→secondary;一次性角标(`(edited)` 等)保留 muted。
- R5.2 实现前先 grep 列出候选清单入 implement.md 逐条勾选,**禁止 66 文件 blanket replace**(控制爆炸半径,muted 档继续服务于真正次要信息)。

### WP6 spec 沉淀(收尾)

- R6.1 `design-tokens.md` 按 Decision 段落惯例记录:两个新 token 的来源与用途边界("填充 500 档 / 文字 400 档"原则)、彩底最低 secondary、彩底禁紫字/禁 accent 蓝字、最小可辨 ΔL。
- R6.2 清理 `scripts/ui-review-specimen.mjs` 的 `CANDIDATES` 段(采纳进 token 后删除对应条目,避免 D 章变成自我对照)。

## Constraints

- 所有颜色必须走 token;新 hex 只进 `style.css`(design-tokens.md "Don't hardcode" 规则)。
- 灰阶三档 token 值不动;间距/圆角/动效不动;bg 梯度不动(专题另启)。
- 桌面零回归:改动后 `scripts/ui-review.sh` 桌面截图与基线(`out/ui-review/20260815-151036/`)对比。
- VLM 数值不可信,验收数值一律自算(ui-review skill 内单行命令)。

## Acceptance Criteria

- [x] AC1 数值复算:accent-text on surface 6.71 / elevated 6.13 / app 7.30;error-text on surface 6.43 / elevated 5.87 / accent-muted 4.90;secondary on accent-muted 5.36 —— 全部 ≥4.5(`research/verify-contrast.md`)。
- [x] AC2 样本页复跑:B 章 accent-text / tool-error-text 行与 read/write/shell 行观感持平(mmx 确认"不再明显偏暗",`research/specimen-b.vlm.json`);D 章已采纳候选删除,仅余 elevatedUp(停泊专题);muted×彩底标「禁用」。附注:muted×accent-muted 数值本身仍 4.41,修法是 usage 级规则(禁用对)而非改 token,AC 原文按此口径判定。
- [x] AC3 全量复跑(三轮对比见 `research/after-full-comparison.md`):侧栏"吞没"类与 03"解析"徽章类收敛;底部说明两轮点名后升 secondary(VLM 仍判"过暗"属其对 11px 小字的系统性过敏,数值 7.03 为准);hover 判"可辨但极弱",真机裁决(R4.2 未闭环,遗留)。
- [x] AC4 grep 复核:`color: var(--color-accent))` / `color: var(--color-tool-error)` 行首残留 0;新增 hex 仅 style.css token 定义与注释。
- [x] AC5 `pnpm test` 1037 用例全绿(65 文件,含 PermissionAskBody 拆字段断言);`pnpm build`(vue-tsc)通过。
- [x] AC6 design-tokens.md 新增 2026-08-15 Decision(500/400 拆分原则 + 彩底规则 + ΔL 规范 + 11px 定向提升),Token 表与 State Tints 表同步;specimen CANDIDATES 仅余 elevatedUp(未决)。

## Notes

- 评审裁决规则提醒:数值管亮度、VLM 管色相观感(warn"泥土色"即数值优先的反例)。
- 本任务为中等复杂度:PRD + implement.md,无独立 design.md(token 级改动,技术决策已由评审给出)。
- 2026-08-15 真机裁决收口:R4.2 hover 10%/14% 强度可辨且不显脏、R2.3 error-text
  `#f87171` 无荧光感,两项均维持现值,无需回落/微调。任务闭环。
