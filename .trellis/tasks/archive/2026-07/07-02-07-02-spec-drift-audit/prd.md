# SPEC-DRIFT 待审 3 条 audit 收口

> 轻量 task(PRD-only)。归档于 2026-07-02。承接 [SPEC-DRIFT.md "待审"段](./../../../reviews/SPEC-DRIFT.md)3 条候选 drift,逐条查 spec / 实现,更新 SPEC-DRIFT.md,必要时微调 spec 措辞或补 1-2 行实现,1 个汇总 commit 收口。

## Goal

把 [`.trellis/reviews/SPEC-DRIFT.md`](./../../../reviews/SPEC-DRIFT.md) 的"待审"段清空:每条逐个查 spec / 实现 → 判定是真 drift 还是误报 → 在 SPEC-DRIFT.md 加新条目(真 drift)或删除待审条目(非 drift / 已审)→ 必要时微调 spec 措辞或补 1-2 行实现 → 1 个汇总 commit。

## 待审 3 条(摘自 SPEC-DRIFT.md "待审"段)

| # | 候选 drift | 初步判定方向 |
|---|---|---|
| 1 | Memory watcher "立即生效" vs 1s debounce(spec 可能也写了"概率性") | 已审:spec 已 2026-06-15 更新为 mtime fence,老 debounce 章节明标 "historical reference"(`memory.md:35-45` 顶部更新注释),需 SPEC-DRIFT 加"已审,非 drift"说明 |
| 2 | AuditKind 11 vs docstring "10" | 真 drift:代码 `audit.rs:4` "17 variants",多份 spec/文档散落写"10"(permission-layer.md:401/454, ROADMAP.md:55, DESIGN.md:69, spikes/2026-06-19-async-parallel-tool-research.md:148)和"11"(ARCHITECTURE.md:690, REVIEW-agent-loop-full-audit-2026-06-14.md:81/142),统一为代码实际数 17 |
| 3 | `cancel_session_asks` 死代码标注 | 已审:grep 8 处使用 + 2 个测试文件,活跃代码(`commands/sessions.rs:147/150/261` + `permissions/mod.rs:54/135` + `permissions/store.rs:78` 定义 + `permissions/ask.rs:32/358` + `tests_store.rs:8/20` + `tests_ask.rs:66/126/131/141`),非 drift,从待审删除 |

## Requirements

### R1. 逐条 audit(grep + Read 查 evidence)

对每条 audit:
1. grep 全代码库 + spec 目录 + docs,找所有相关引用点
2. Read 关键文件,确认 evidence(代码 vs spec vs docs 实际写什么)
3. 判定 drift 类型:
   - **真 drift(有意)**:实施主动偏离 spec,需 spec 更新或 ADR 记录
   - **真 drift(无意)**:实施未察觉 spec 要求,需补实现或更新 spec
   - **非 drift(已审)**:SPEC-DRIFT 待审描述过期,实际已对齐
   - **非 drift(误报)**:grep 证据显示活跃代码 / 一致实现,SPEC-DRIFT 待审误报

### R2. SPEC-DRIFT.md 更新

- 真 drift(有意/无意):按已有格式加新条目(`### DRIFT-NNN — <title>`,含 Type / Spec / Implementation / Impact / Resolution / Decision / Related)
- 非 drift(已审/误报):在"待审"段删除条目,替换为"已审:<简短结论 + evidence 文件:line>"行(便于后续 audit 知道查过)
- 不删除任何已 resolved 的历史条目(DRIFT-001 / DRIFT-002 保留)

### R3. Spec 措辞微调(仅 #2)

AuditKind 计数统一为代码实际 17:
- `.trellis/spec/backend/permission-layer.md`:401 "10 类 AuditKind" → "17 类 AuditKind"
- `.trellis/spec/backend/permission-layer.md`:454 "10 类 AuditKind 都 serializable" → "17 类 AuditKind 都 serializable"(如有 round-trip 测试,确认测试数 ≥17)
- `docs/ROADMAP.md`:55 "⑯ 审计日志 10 类 AuditKind" → "⑯ 审计日志 17 类 AuditKind"
- `docs/DESIGN.md`:69 "⑯ 审计日志 10 类 AuditKind" → "⑯ 审计日志 17 类 AuditKind"
- `docs/ARCHITECTURE.md`:690 "**11 类 AuditKind**" → "**17 类 AuditKind**"
- `docs/spikes/2026-06-19-async-parallel-tool-research.md`:148 "10 类 AuditKind" → "17 类 AuditKind"
- `_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`:81 "11 类 AuditKind" → "17 类 AuditKind"(review 历史文档,可改可不改;如决定不改,SPEC-DRIFT 标注"historical review,不改")

### R4. 1 个汇总 commit

commit message:`docs(spec): audit SPEC-DRIFT 待审 3 条 + 统一 AuditKind 计数到 17`

包含文件(预估):
- `.trellis/reviews/SPEC-DRIFT.md`(3 条 audit 结论)
- `.trellis/spec/backend/permission-layer.md`(2 处)
- `docs/ROADMAP.md`(1 处)
- `docs/DESIGN.md`(1 处)
- `docs/ARCHITECTURE.md`(1 处)
- `docs/spikes/2026-06-19-async-parallel-tool-research.md`(1 处,可选 — 历史 spike)
- 历史 review 文档不改(在 SPEC-DRIFT 注明)

## Acceptance Criteria

- [ ] SPEC-DRIFT.md "待审"段清空(3 条全部移到"已审"/"已 resolved"/新"drift"段)
- [ ] AuditKind 计数在所有现役 spec/docs 中统一为 17(代码 `audit.rs:4` 实际值)
- [ ] 任何修改后,`cargo check` / `vue-tsc --noEmit` 0 err(本 task 不改 Rust/Vue,但 spec 引用代码注释需保持一致)
- [ ] 1 个汇总 commit,commit message 列出 3 条 audit 结论 + 涉及的 N 个文件
- [ ] DEBT.md 不需要新增条目(本 task 只动 spec/docs,不动 Rust 代码)

## Dependencies

- 无外部依赖
- 不需要 `design.md` / `implement.md`(轻量 task)

## Out of Scope

- 不解决 DRIFT-001(二次取消语义)— 已记录 "待 V3 评估"
- 不动代码层(本 task 仅 audit + spec/docs 措辞;若发现需补实现的真 drift,留作 follow-up task)
- 不重写 AuditKind 章节结构(只改计数;章节 outline 不动)
- 不动 `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`(历史 audit,在 SPEC-DRIFT 注明即可)