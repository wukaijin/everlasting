# docs 文档过时审查 — 更新/重写/移除

## Goal

审查项目根目录 + `docs/` 下的活文档（48 个），逐一判定"保持 / 更新 / 重写 / 移除 / 归档"，产出审查报告，并对所有判定为"更新 / 归档"的文档执行处置，恢复文档与代码事实的一致性。

## Background（已确认事实）

- 范围界定：**48 个活文档** = 根目录 5（AGENTS/CLAUDE/README/STRUCTURE/THIRD_PARTY_LICENSES）+ docs/ 顶层 26 + docs/IMPLEMENTATION/ 4 + docs/WORKFLOW-INTEGRATION/ 13。历史记录目录（_archive/_deprecated/_reviews/research/spikes 共 33 个）性质为记录，不追求新鲜，**不在审查范围**（用户跳过范围问题，采用推荐方案）。
- 扫描证据（2 个 Explore agent 实测 + 主会话抽查确认）：
  - 死锚系统性问题：08-07 拆分后 `IMPLEMENTATION.md#4-决策日志` 在 CLAUDE/README/STRUCTURE/CONTEXT/docs-README/ARCHITECTURE 等 10+ 处失效。
  - 断链：WORKFLOW-INTEGRATION/ 11 个 part 缺 `../` 前缀；decisions.md 索引 3 链多一层前缀；decisions-2026-06 8 处缺 `../`。
  - 错卷：decisions-2026-06.md 尾部混入 11 条 07-02~08-04 条目。
  - 过时事实：DESIGN 5 处（DB 路径/notify/工具数/AuditKind 数/B10 措辞）、CONTEXT "79 handlers"→91、TECH :72 notify 残留、CLAUDE :189 notify、A2 状态头、INTERLEAVED-THINKING-DESIGN 无已实施标记、07-review-plugin-vision 仍标愿景、BACKLOG §5.3 标签两个月未刷。
  - 已消费的一次性文档：MANUAL-TEST-P2 / REMOTE-ACCESS-RESEARCH / WORKFLOW-INTEGRATION-REVIEW(-2) / SESSION-FIRST-MESSAGE-INTERFACE。
  - THIRD_PARTY_LICENSES 引用资产全部存在，判定保持。

## Requirements

- R1 审查报告：48 个文档每行一个判定（保持/更新/重写/移除/归档）+ 理由 + 证据 file:line，落盘 `research/doc-audit-report.md`。
- R2 断链修复：范围内全部相对链接目标可解析、无死锚。
- R3 锚点统一：`IMPLEMENTATION.md#4-决策日志` 全部改为指向 `IMPLEMENTATION/decisions.md`。
- R4 归档处置：5 个已消费文档移入 `docs/_archive/`（日期前缀命名），全项目引用同步修复。
- R5 过时事实更新：R1 判定的"更新"项全部落实到文件（数字/路径/状态头/措辞，不动叙事结构）。
- R6 错卷归位：decisions-2026-06 尾部 11 条按日期移回 07/08 分卷，06 卷留归位备注。
- R7 索引更新：docs/README.md 与实际活文档一一对应。

## Acceptance Criteria

- [ ] `research/doc-audit-report.md` 覆盖全部 48 个文档，每行含判定 + 理由 + 证据。
- [ ] 范围内所有相对链接目标存在、锚点可解析（脚本扫描 0 失败）。
- [ ] `grep "IMPLEMENTATION.md#4"` 在活文档中 0 命中。
- [ ] `grep -E "### 2026-0[78]-" docs/IMPLEMENTATION/decisions-2026-06.md` 0 命中（错卷归零）。
- [ ] 5 个归档文档已移入 `docs/_archive/`，无活文档指向旧路径。
- [ ] 全部"更新"项落地（对照 design.md 总表逐项核销）。
- [ ] docs/README.md 索引与 docs/ 实际活文档一致。

## Out of Scope

- 历史记录目录（_archive/_deprecated/_reviews/research/spikes）的内容审查。
- 文档叙事/结构重写（无重写判定；如执行中发现某文档需整体重写，单列报告交用户决策，不在本任务执行）。
- 文档内容的英文翻译、格式美化。
- `.trellis/spec/` 下的 spec 文档（不在用户指定范围）。

## Key Decisions（用户跳过澄清问题时采用的默认）

- 范围 = 活文档 48 个（推荐方案）。
- 交付 = 审查报告 + 执行处置（R1-R7 全做）。
- 归档而非删除（历史保留，git 可查）。
- 错卷归位允许编辑只追加日志（git 可回溯 + 留备注行）。
