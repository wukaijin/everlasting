# WP3 多 SOT 重复点盘点(2026-08-24)

> 依据 PRD WP3「先盘点一遍具体重复点再动手」。目标:每条信息只落一处,其余改链接。
> 已随本任务动手的部分见「已处置」;未处置项列「待办」,避免一次改动面过大。

## 盘点结果

| # | 重复对 | 重复内容 | 处置 |
|---|---|---|---|
| 1 | CLAUDE.md × ROADMAP | CLAUDE.md「当前状态」段(日期 / commit hash / 已完成 epic)与 ROADMAP §1 重复 | ✅ 已处置(WP1):状态段删除,CLAUDE.md 改为指向 ROADMAP(SOT) |
| 2 | ROADMAP × git log | ROADMAP §1.2 单元格塞 commit hash / 测试数,与 git log 重复 | ✅ 已处置(WP2):18 处 commit hash + 6 处测试数删除,只留"做了什么+时间+链接" |
| 3 | BACKLOG × ROADMAP | BACKLOG §1/§2/§3.1 的「已落地 / 待办」状态 + 进度段落与 ROADMAP §1 排期重复 | ⏸ 待办:删状态叙事,改为一行「排期归 ROADMAP」+ 链接 |
| 4 | CONTEXT × DESIGN | CONTEXT 的「实现状态」段落(Checklist / BackgroundShell / daemon 化 / 术语实现状态)与 DESIGN「已具备」重复 | ⏸ 待办:状态句收敛为「见 DESIGN」链接 |
| 5 | CLAUDE.md × STRUCTURE.md | CLAUDE.md §Architecture 的大段文件树与 STRUCTURE.md(756 行全景)重复 | ⏸ 待办:Architecture 段缩为「完整结构见 STRUCTURE.md」+ 关键数据流 |
| 6 | CONTEXT × IMPLEMENTATION 决策日志 | CONTEXT 多个词条(SubagentRun / AuditKind / MAX_TURNS…)与决策日志分卷重复 | ⏸ 待办:词条只留术语定义,「实现状态」收敛为决策日志链接 |

## 说明

- **非目标**(PRD 明示):decisions / journal / `.trellis/spec` 契约文档不降级、不动。
- **契约文档保持唯一 SOT**:ROADMAP / DESIGN / STRUCTURE / 决策日志分卷是各自主管域 SOT,去重方向是"叙事文档(CLAUDE.md / BACKLOG / CONTEXT)收敛为链接",不是改 SOT 内容。
- **链接校验**:改动后跑 `.trellis/tasks/archive/2026-08/08-10-docs-staleness-audit/scripts/check-links.py`(复制到临时路径跑)验证 0 死链;存量 2 死链(REMOTE-ACCESS-E2E 归档任务引用 / ROADMAP C3 死锚)已在本任务修复。

## 待办优先级(建议)

1. #3 BACKLOG 状态叙事收敛(改动小、收益大)
2. #5 CLAUDE.md Architecture 段瘦身(去文件树,token 税大头之一)
3. #4 CONTEXT 实现状态收敛(与 #5 同批)
4. #6 CONTEXT 词条实现状态收敛(与 #4 同批)
