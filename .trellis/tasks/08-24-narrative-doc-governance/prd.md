# 叙事文档治理:叙事文档降维护税,契约文档当唯一 SOT

## Goal

把"每个决策写四遍"的文档税降下来:**契约文档**保留为唯一 SOT(认真维护),
**叙事文档**(ROADMAP / BACKLOG / CONTEXT / CLAUDE.md 状态段)松着维护——
去重复、恢复纪律、降低每轮注入的 token 税。方向是"收紧漏斗",不是"少写文档",
更不是降级契约文档。

## 前因(为什么开这个任务)

2026-08-24 群聊 session `702e6ec8-2475-4510-86bc-bc8d11a17f70`「讨论一下这个项目的不足」
前半段,参与者 D4F 提出本项目最重的成本是**文档税**——每个决策要写四遍:
ROADMAP 表格 / decisions.md / .trellis/spec / CLAUDE.md;M3 量化了一张 ROI 表,
其中 CLAUDE.md「当前状态」段为每轮 LLM 注入、ROI 极低("为 LLM 写但 LLM 该读 git log,
为人写但人会查 ROADMAP")。用户随后重聚焦到代码/文档本身(后半段验证出 4 条代码债,
已登记 RULE-FM-001 / TESTPOOL-001 / ARGS-001 / DOC-001)。

会话内进一步分析把"文档税"拆成两类:
- **契约文档**(`.trellis/spec/` 的 tool-contract / memory / design-tokens):实现时查、
  是真契约,ROI 高,值得认真维护。
- **叙事文档**(ROADMAP / BACKLOG / CONTEXT / decisions / journal):几乎只在写的时候
  和考古时读,税主要来自**过度维护**。

结论:减税的方向不是少写,是**让契约文档当唯一 SOT、叙事文档松着维护**。用户认可,
决定单开本任务,另开 session 执行。

## 现状与问题

- **CLAUDE.md「当前状态」段**:易过期内容(session 106 曾专门做五特性文档回归,就是它
  过期的证据),且每轮注入付 token 税。memory-gov 已把指令块 10124→2080(-79.5%),
  但治了"量"没治"结构"——状态段仍会过期、仍重复 ROADMAP。
- **ROADMAP §5 纪律跑偏**:§5 白纸黑字承诺"不列具体 commit / PR 编号、不讲技术细节、
  不做决策追溯",但执行上 B9+ 那格已塞进一整份 spec,多行带 commit hash + 测试数。
  这些细节每改一次要同步一次,是"写四遍"里最重的一份。
- **多 SOT 重复**:BACKLOG 在重复 ROADMAP 的排期、CONTEXT 在重复 DESIGN 的内容、
  CLAUDE.md 状态段重复 ROADMAP。多 SOT 的代价 = 改一处忘一处 → 文档自相矛盾 →
  下次更不信任文档。

## 分类框架(理论基础)

| 类型 | 文档 | 维护频率 | 阅读频率 | 策略 |
|---|---|---|---|---|
| 契约 | `.trellis/spec/`(tool-contract / memory / design-tokens) | 高(行为变更) | 实现时查 | **唯一 SOT,认真维护** |
| 叙事 | ROADMAP / BACKLOG / CONTEXT / decisions / journal | 高 / 中 / 低 | 写 + 考古时 | **松着维护,去重复,恢复纪律** |

## 范围(WP)

- **WP1 CLAUDE.md 瘦身**(最高收益):只留"项目是什么 + 去哪查",状态类内容
  (当前做到哪 / 排哪档)一律不进 CLAUDE.md——人查 ROADMAP、机器查 git log。
  即使状态过期,也不污染每轮请求。与 RULE-DOC-001 的 CLAUDE.md 部分联动。
- **WP2 ROADMAP 恢复纪律**(找回自己的规矩):单元格只留"做了什么 + 什么时候 +
  链接到 spec",细节(commit hash / 测试数 / 技术细节)移走。历史行保留原样,
  新行与新增编辑遵守 §5。
- **WP3 去多 SOT 重复**(成本略高,先盘点):BACKLOG×ROADMAP 排期、
  CONTEXT×DESIGN 内容、CLAUDE.md×ROADMAP 状态,每条信息只落一处,其余改链接。
  需要先盘点一遍具体重复点再动手。

## 非目标(明确不动)

- **decisions.md 不动**:决策时写一次,成本低;虽几乎不读,考古有价值。
- **journal 不动**:一次性过程记录,不是反复维护的税;"过程进 journal、结论进 spec"
  的边界保持。
- **契约文档不降级维护**:`.trellis/spec/` ROI 高,治理目标是让它们当 SOT,不是精简。

## 验收标准

- WP1:CLAUDE.md 无易过期"当前状态"类段落(状态段改为派生 / 链接 / 删除),
  每轮注入面不再含状态叙事。
- WP2:ROADMAP 新行与后续编辑遵守 §5(无 commit hash / 测试数 / 技术细节),
  §5 承诺与执行一致。
- WP3:重复点清单落地;每条信息唯一落点,其余位置改链接;文档间不再互相复述。

## 关联

- DEBT `RULE-DOC-001`:参数注释块重复 git log(双 source of truth)+
  CLAUDE.md 状态段重复 ROADMAP 且每轮付 token 税(本任务 WP1 直接关联)。
- ROADMAP §5「后续维护承诺」:文档对自身的纪律,WP2 的验收基准。
- 群聊 session `702e6ec8…`:前因来源(文档税讨论)。
- 08-15 memory-gov:已把指令块注入量 -79.5%,本任务在结构层继续收尾(状态叙事不进注入)。
