# 叙事文档治理 follow-up:清 inventory 待办 + 修复 ROADMAP §5 自矛盾

## Goal

把叙事文档治理任务(08-24-narrative-doc-governance)收尾清干净:**清掉 inventory 遗留待办**,
并**修复 ROADMAP §5 内部自矛盾**。目标是让"写四遍"的维护税真正降下来——不是再删一批存量,
而是**让文档纪律可执行、防增量**,让叙事文档(ROADMAP / BACKLOG / CONTEXT)只当索引、契约文档当 SOT。

## 前因(为什么 follow-up)

上一个任务(`.trellis/tasks/archive/2026-08/08-24-narrative-doc-governance/`)解决了**注入税的大头**
(CLAUDE.md 巨型文件树 + 「当前状态」段删除,每轮注入 token 税显著下降,这是最值钱的一处),但收尾时
**把"已盘点"当成了"已解决"**:

- **inventory 待办与落地不一致**:inventory(`duplication-inventory.md`)明列 #3(BACKLOG × ROADMAP)、
  #4/#6(CONTEXT ×DESIGN/决策日志)为**待办**,但 commit 却声明 "WP3 去多 SOT 重复"完成。
- **盘点滞后**:#5(CLAUDE.md Architecture 文件树)实际已做、inventory 却标待办。
- **§5 自矛盾未发现**:§5「改动时机」写着"移到 §1 已实施 + **加 commit hash 引用**",但同一节「不做的边界」
  又写着"不在本文件列具体 commit / PR 编号"——自相矛盾,正是 WP2 只删了存量、没修机制的表现。

用户评估后认为"代码变更不大、没多少瘦身",决定再开任务尽量解决。

## 现状核对(2026-08-24)

### 已解决(上轮)

- ✅ CLAUDE.md 巨型目录树(756 行)→ 一行"完整结构见 STRUCTURE.md(单一来源)"(inventory #5,实际已做)
- ✅ CLAUDE.md「当前状态」段删除 → 状态查 ROADMAP(inventory #1)
- ✅ ROADMAP 删 18 处 commit hash + 6 处测试数(inventory #2,部分)
- ✅ BACKLOG §3.1 memory 治理进度 5 条日志压成一行 + 链接(inventory #3 的一部分)
- ✅ CONTEXT 加防复发纪律:"词条内实现状态为历史快照,不在此追加"

### 遗留待办(本任务范围)

- ⏸ BACKLOG §1/§2/§4/§5.1 仍有进度叙事:如 §1 "已落地 (B2 @file 2026-06-17, B3 /command 2026-06-17)",
  §2 "已落地 (B4 2026-06-18)",§4 "✅ 已落地 2026-06-06, commit `ef7cea8`"(inventory #3 未清完)
- ⏸ CONTEXT BackgroundShell 词条仍是 6 行实现细节(三触发 `select!` 等),未收敛为术语定义 + 链接
  (inventory #4/#6)
- ⏸ ROADMAP §5 内部自矛盾:改「加 commit hash 引用」与「不列 commit/PR 编号」冲突
- ⏸ ROADMAP 单元格技术细节堆积(B9+ 等"一整份 spec 塞进单元格")未系统清理(WP2 只删了最冗余的 hash/测试数)

## 范围(WP)

- **WP1 ROADMAP §5 修复 + 纪律可执行化**(最高优先,防增量):删掉 §5 里"加 commit hash 引用"这条自相矛盾项,
  §5 与执行一致;补一条**新增/编辑 ROADMAP 时的自查点**(新行不带 commit hash / 测试数 / 技术细节,
  细节走 spec / BACKLOG / PRD),让纪律从"已有但不执行"变成"可执行"。
- **WP2 BACKLOG 进度叙事收敛**(inventory #3 清完):§1/§2/§4/§5.1 的"已落地 + 日期 + commit"压缩为一行
  "排期/进度归 ROADMAP §1.2" + 链接,保留技术评估与依赖分析(BACKLOG 是技术评估文档,不重复排期)。
- **WP3 CONTEXT 词条实现状态收敛**(inventory #4/#6):BackgroundShell 等剩余带实现细节的词条,
  收敛为"术语定义 + 链接到 ROADMAP / 决策日志",删重复的实现细节(三触发 select! 等)。
- **WP4 ROADMAP 单元格技术细节系统清理**(可选,量大):对 §1.2 里"一整份 spec 塞进单元格"的行
  (如 B9+)压缩为"做了什么 + 时间 + 链接到 spec/PRD"。**注意**:这些行是历史归档(07-02 等),
  压缩不丢信息(细节在 spec / PRD / 决策日志),但改动面大,可只处理最臃肿的几行。

## 非目标(明确不动)

- **decisions / journal / `.trellis/spec` 契约文档**:不降级、不动,保持 SOT 地位。
- **ROADMAP / DESIGN / STRUCTURE / 决策日志分卷**:是各自主管域 SOT,去重方向是"叙事文档收敛为链接",
  不是改 SOT 内容。
- **历史行不改写**:已落地的历史行保留"做了什么+时间",只删冗余细节(commit hash / 测试数),不重写历史。

## 验收标准

- §5 无自相矛盾(不再出现"加 commit hash"与"不列 commit/PR 编号"并存),且新增行有自查点可执行。
- BACKLOG / CONTEXT 的进度叙事与实现细节收敛为链接,无"已落地 + 日期 + commit"散落。
- 改动后链接校验 0 死链(跑 `.trellis/tasks/archive/2026-08/08-10-docs-staleness-audit/scripts/check-links.py`)。
- inventory 待办全部清空(处置列无 ⏸)。

## 关联

- 前身任务:`.trellis/tasks/archive/2026-08/08-24-narrative-doc-governance/`
- DEBT `RULE-DOC-001`:参数注释块重复 git log + CLAUDE.md 状态段重复 ROADMAP(WP1 的 CLAUDE.md 部分已由前身解决,本任务收尾 §5)
- 群聊 session `702e6ec8…`:文档税讨论来源
