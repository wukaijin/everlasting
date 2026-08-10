# implement.md — 执行计划

## 阶段 A：审查报告落盘（先做，作为处置依据）

- [ ] A1 产出 `research/doc-audit-report.md`：48 个文档逐行判定（保持/更新/重写/移除/归档）+ 理由 + 证据 file:line（内容取自本任务 design.md 总表 + 两份 Explore 扫描）。
- [ ] A2 归档前 grep 全项目活文档，列出所有指向 5 个待归档文件的引用点（链接/提及），作为 B3 的修复清单。

## 阶段 B：处置执行（按风险从低到高）

- [ ] B1 **断链修复**：WORKFLOW-INTEGRATION/ 11 个 part 补 `../`；decisions.md :7-9 去 `IMPLEMENTATION/` 前缀；decisions-2026-06 8 处补 `../`；decisions-2026-07 :13 补 `../`；12-integration-points.md:52 死锚 `#问题-2…` 改为对应标题锚点或删除。
- [ ] B2 **全局锚点统一**：CLAUDE.md / README.md / STRUCTURE.md / CONTEXT.md / docs/README.md / ARCHITECTURE.md / BACKLOG.md 中 `IMPLEMENTATION.md#4-决策日志` / `IMPLEMENTATION.md §4` → `IMPLEMENTATION/decisions.md`（按文件调整相对路径）。
- [ ] B3 **归档 5 文档**：`git mv` 至 `docs/_archive/<YYYY-MM-DD>-<原名>`；修复全部指向旧路径的引用（docs/README.md 索引、REMOTE-ACCESS-ROADMAP.md 对 RESEARCH 的链接等）。
- [ ] B4 **内容过时更新**：
  - DESIGN.md 5 处（DB 路径 / notify / 工具数 / AuditKind 数 / B10 措辞）
  - CONTEXT.md :144 79→91；TECH.md :72 去 notify；CLAUDE.md :189 notify → mtime fence
  - A2-SHELL-CLASSIFICATION.md 头部状态；INTERLEAVED-THINKING-DESIGN.md 补已实施头
  - WORKFLOW-INTEGRATION/07-review-plugin-vision.md 状态（愿景→已开工）
  - BACKLOG.md §5.3 两条标签刷新；ROADMAP.md B9+ 重复行清理（可选）；HACKING-wsl.md 头部日期（可选）
- [ ] B5 **错卷归位**：decisions-2026-06.md 尾部 11 条 → decisions-2026-07.md / decisions-2026-08.md 按日期归位；06 卷尾部留归位备注；decisions-2026-07.md :248 重复条目修正 + 陈旧路径刷新（dev-workflow.json、task archive/ 前缀）。

## 阶段 C：索引与收尾

- [ ] C1 更新 `docs/README.md`：索引补 A2-SHELL-CLASSIFICATION / DEBUG_DB / INTERLEAVED-THINKING-DESIGN / WORKFLOW-INTEGRATION 系；已归档文档移出/改指。
- [ ] C2 全量校验（见 design.md §4 验收）：链接脚本扫描、锚点 grep 归零、错卷归零、索引一致。
- [ ] C3 git diff 审查 + 提交（Phase 3.4）。

## 验证命令

```bash
# 链接完整性脚本（B/C 阶段反复跑）
python3 .trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py   # 若需要则创建

# 锚点归零
grep -rn "IMPLEMENTATION.md#4" AGENTS.md CLAUDE.md README.md STRUCTURE.md docs/*.md docs/IMPLEMENTATION/*.md docs/WORKFLOW-INTEGRATION/*.md

# 错卷归零
grep -nE "### 2026-0[78]-" docs/IMPLEMENTATION/decisions-2026-06.md

# 索引一致（人工对照 docs/README.md 表 vs ls docs/*.md）
```

## 风险与回滚

- 错卷归位是唯一"改写历史"动作 → git 全程可回溯；06 卷留备注行。
- 归档后如有文档仍被引用 → 引用修复清单（A2）先行，修复后归档。
- 内容更新只动事实数字/路径/状态头，不动叙事结构，diff 可控。
