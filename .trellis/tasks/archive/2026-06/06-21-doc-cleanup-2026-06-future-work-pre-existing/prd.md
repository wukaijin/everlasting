# doc-cleanup-2026-06: ⑨ 关合并 + Future Work 状态 + pre-existing 断链修复

## Goal

清理 doc-trim-2026-06 任务明确标记的 3 个 known leftover:
1. tool-contract.md ⑨ 关段 + permission-layer.md 重叠合并
2. llm-contract.md Future Work (Deferred from Step6) 状态确认(已自动删除?)
3. 5 处 pre-existing 断链修复(实际不止 4 处)

## What I already know

### Item 1: ⑨ 关段重叠 (~350L + 289L)

**tool-contract.md ⑨ 关段**:
- "Scenario: ⑨ 关 Permission Decision Layer (A2 + B7 PR1, 2026-06-13)" — line 1269-1540
- "Scenario: Path-based Permission Layer (A2+B7 re-grill, 2026-06-13)" — line 1541-1619
- 总 1269-1619 (约 350 行)

**permission-layer.md** (上次任务新建):
- 289 行
- 来源:llm-contract.md 原 388-670 行的 "Scenario: Per-Session Mode + ⑨ 关 Permission Layer (A2 + B7, 2026-06-13)"
- 头部已含 cross-ref:`⑨ 关 tool-side contract (hard kill list + IPC + path-based): [tool-contract.md](./tool-contract.md) §"Scenario: Path-based Permission Layer"`

**重叠性质**:
- tool-contract.md ⑨ 关段:tool 侧 hard kill list / IPC / path-based 决策 (lower-level)
- permission-layer.md:Per-Session Mode + ⑨ 关 Permission Layer (higher-level)
- 两份都覆盖同一概念但侧重点不同(意图 vs 实现)

### Item 2: llm-contract.md Future Work

- 上次任务前:llm-contract.md line 1381-1392 "Future Work (Deferred from Step6)"
- 上次任务后:llm-contract.md 465 行,grep "Future Work" / "Deferred" / "Step6" 只剩 line 47 的 Extended Thinking 标题
- **结论**:上次 trellis-implement 已经自动删除该段(Step6 已实施,deferred items 已全部 resolved)
- 不需要额外操作,只需在 PRD 记录"已自动清理"

### Item 3: 5 处 pre-existing 断链(已验证,非 doc-trim 引入)

1. `docs/IMPLEMENTATION.md:383` → `[../.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md]`
   - 实际位置:`.trellis/tasks/archive/2026-06/06-13-a2-b7-regrill-path-based/prd.md`
   - 修复:`../.trellis/tasks/archive/2026-06/06-13-a2-b7-regrill-path-based/prd.md`

2. `docs/IMPLEMENTATION.md:429` → `[../research/skill-system-survey.md]`
   - 实际位置:`docs/research/skill-system-survey.md`
   - 从 `docs/IMPLEMENTATION.md` 出发,正确相对路径应为 `research/skill-system-survey.md`(去掉 `../` 前缀)

3. `docs/IMPLEMENTATION.md:656` → `[../../.trellis/tasks/06-10-v2-roadmap-and-vision-consolidation/prd.md]`
   - 实际位置:`.trellis/tasks/archive/2026-06/06-10-v2-roadmap-and-vision-consolidation/prd.md`
   - 修复:`../../.trellis/tasks/archive/2026-06/06-10-v2-roadmap-and-vision-consolidation/prd.md`

4. `docs/IMPLEMENTATION.md:739-741` (3 处相同模式) → `[../_archive/2026-06-3b-1/FOLLOW-UP.md]`
   - 实际位置:`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`
   - 从 `docs/IMPLEMENTATION.md` 出发,正确相对路径应为 `_archive/2026-06-3b-1/FOLLOW-UP.md`(去掉 `../` 前缀)
   - 注意 line 739/740/741 3 处都引用同一文件,需 3 处都修

5. `.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md` line 33/35/37/39/51/52/53/57 (8 处)
   - 当前:`./ARCHITECTURE.md` / `./BACKLOG.md`
   - 实际位置:`docs/ARCHITECTURE.md` / `docs/BACKLOG.md`
   - 从 `.trellis/spec/archive/` 出发,正确相对路径应为 `../../../docs/ARCHITECTURE.md` / `../../../docs/BACKLOG.md`

**总计**: 5 文件,13 处断链需修(原 trellis-check 报告只提 4 处,漏了 line 429)

## Assumptions

- A1: 不改文案,只修路径和合并
- A2: ⑨ 关合并是文档手术(删除 + cross-ref 调整),不动代码
- A3: 单 commit(与上次任务风格一致)
- A4: archive 子目录文件保留(archived 后仍可被查阅,只是只读)

## Final Decisions

| # | 决策点 | 选择 |
|---|---|---|
| D1 | ⑨ 关合并方向 | 删 tool-contract.md 1269-1619 (350 行),保留 permission-layer.md (289 行),tool-contract.md 头部加 cross-ref |
| D2 | Commit 粒度 | 单 commit |
| D3 | Future Work 处理 | 仅在 PRD + journal 记录"已自动清理",无文件操作 |
| D4 | 断链修复 | 全部 13 处一次性修复 (5 文件) |

## Requirements

### R1: ⑨ 关合并(删 tool-contract.md 段)

- 删 `.trellis/spec/backend/tool-contract.md` line 1269-1619 (约 350 行)
  - "Scenario: ⑨ 关 Permission Decision Layer (A2 + B7 PR1, 2026-06-13)" 整段
  - "Scenario: Path-based Permission Layer (A2+B7 re-grill, 2026-06-13)" 整段
- tool-contract.md 头部(或 ⑨ 关 section 之前)加 cross-ref:
  `> **⑨ 关 Permission Layer** 设计合约: [permission-layer.md](./permission-layer.md) (A2 + B7, 2026-06-13)`
- 主文件 net: 1964 → ~1614 行
- 保留 L1a Background Shell Tools (原 line 1620 → 新行号)

### R2: Future Work 状态记录

- 仅在 journal Session 56 记录:`llm-contract.md Future Work (Deferred from Step6) 已被 doc-trim 任务自动清理,本任务不二次操作`
- 无文件操作

### R3: 5 文件 13 处断链修复

1. `docs/IMPLEMENTATION.md:383` — `../.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md` → `../.trellis/tasks/archive/2026-06/06-13-a2-b7-regrill-path-based/prd.md`
2. `docs/IMPLEMENTATION.md:429` — `../research/skill-system-survey.md` → `research/skill-system-survey.md`
3. `docs/IMPLEMENTATION.md:656` — `../../.trellis/tasks/06-10-v2-roadmap-and-vision-consolidation/prd.md` → `../../.trellis/tasks/archive/2026-06/06-10-v2-roadmap-and-vision-consolidation/prd.md`
4. `docs/IMPLEMENTATION.md:739/740/741` (3 处) — `../_archive/2026-06-3b-1/FOLLOW-UP.md` → `_archive/2026-06-3b-1/FOLLOW-UP.md`
5. `.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md` line 33/35/37/39/51/52/53/57 (8 处) — `./ARCHITECTURE.md` → `../../../docs/ARCHITECTURE.md`,`./BACKLOG.md` → `../../../docs/BACKLOG.md`

## Acceptance Criteria

- [ ] tool-contract.md 已删 ⑨ 关段(原 line 1269-1619,350 行),头部加 cross-ref 指向 permission-layer.md
- [ ] permission-layer.md 保留原样不动
- [ ] llm-contract.md Future Work 状态在 journal 记录(无文件操作)
- [ ] 5 文件 13 处断链全修
- [ ] post-fix grep 验证:`grep -E "\(\.\./\.\./\.trellis/tasks/[0-9]" docs/IMPLEMENTATION.md` 应为空
- [ ] post-fix grep 验证:`grep -E "\(\./(ARCHITECTURE|BACKLOG)\.md\)" .trellis/spec/archive/...md` 应为空
- [ ] 不引入新断链
- [ ] 单 commit 落库,commit message:`chore(docs): cleanup ⑨ 关 overlap + 5 pre-existing broken links`
- [ ] trellis-check PASS

## Definition of Done

- ⑨ 关文档去重完成(剩 1 份 canonical + 1 处 cross-ref)
- 5 文件 13 处 pre-existing 断链全修
- 单 commit 落库
- journal Session 56 记录
- task archived

## Out of Scope

- ⑨ 关内容的实质改动(只挪位置,不改文案)
- 任何代码修改
- 其他 16 篇 markdown 文件(除 5 个被影响文件)
- llm-contract.md Future Work 物理删除(已自动完成,本任务不二次操作)

## Technical Notes

- ⑨ 关合并方向待定(Q1)
- archive 文件已 ARCHIVED 标记,断链修复保持只读性质
- commit message 格式:`chore(docs): <summary>`
- 关联:`.trellis/tasks/archive/2026-06/06-21-doc-trim-2026-06-6-600-md/`(源任务)