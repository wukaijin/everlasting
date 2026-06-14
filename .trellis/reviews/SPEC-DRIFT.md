# Spec Drift Tracker — spec 与实现偏离追踪

> **目的**: 集中记录所有 spec 文档(ARCHITECTURE.md / DESIGN.md / spec/*.md)与实际实现之间的**有意偏离**和**无意遗漏**
>
> **创建**: 2026-06-14(由 `.trellis/tasks/06-14-review-debt-consolidation` 启动)
>
> **关联**: `.trellis/reviews/DEBT.md` 跟踪代码层 finding,本文件跟踪 spec/实现层 drift

---

## 分类

- **有意偏离**: 实施时主动选择与 spec 不同路径,需要 spec 后续更新或 ADR 记录
- **无意遗漏**: 实施时未察觉 spec 要求,需要补实现或更新 spec

---

## 当前 drift

### DRIFT-001 — 二次取消语义未实现

- **Type**: 有意偏离(MVP 简化)
- **Spec**: `docs/ARCHITECTURE.md §2.5.1` 要求"取消不立即终止,把'取消'作为 tool_result 回传给 LLM 一次自我收敛机会;二次取消才真终止"
- **Implementation**: `app/src-tauri/src/agent/chat.rs:934-978` 单次 cancel 即 emit Done("cancelled") 终止
- **Impact**: 影响 LLM 自我收敛能力,但 MVP 简化可接受
- **Resolution 候选**:
  - A. 实现二次取消语义(复杂,涉及 oneshot 改造)
  - B. 更新 ARCHITECTURE.md §2.5.1 标"已偏离,见 DRIFT-001",MVP 不实现
  - C. V3 路线图评估
- **Decision**: 待 V3 评估
- **Related DEBT**: RULE-A-010 (P3)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### DRIFT-002 — web_fetch redirect docstring 自相矛盾

- **Type**: 无意遗漏(spec drift)
- **Spec**: `app/src-tauri/src/tools/web_fetch.rs:17` docstring 写 "each redirect target";`§5 security notes` 写 "not implemented"
- **Implementation**: `Policy::limited(MAX_REDIRECTS)` 不重做 IP check(RULE-E-003)
- **Impact**: 内部文档矛盾,实施者/维护者困惑
- **Resolution**: 实施 RULE-E-003 时统一 docstring + security notes,删除 "not implemented" 或改为 "implemented,see `web_fetch.rs:SECTION`"
- **Decision**: 随 RULE-E-003 修复
- **Related DEBT**: RULE-E-003 (P0)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

---

## 待审(待 audit 验证)

| 候选 drift | 状态 |
|---|---|
| spec §2.5.1 二次取消语义 | ✅ DRIFT-001 已记录 |
| web_fetch redirect docstring 矛盾 | ✅ DRIFT-002 已记录 |
| Memory watcher "立即生效" vs 1s debounce | 待审(spec 可能也写了"概率性",需确认) |
| AuditKind 11 vs docstring "10" | 待审(spec 是否写"10"?) |
| cancel_session_asks 死代码标注 | 待审(`#[allow(dead_code)]` 是不是 spec 要求保留?) |

---

## 维护说明

- 每次 audit 检查 DRIFT-001 ~ DRIFT-NNN 状态更新
- 新发现 drift 加入本文件 + 关联 DEBT.md RULE
- spec 修订 PR 必须先在本文件标"resolved by PR #N",然后再 archive 到历史段

---

**最后更新**: 2026-06-14 by carlos