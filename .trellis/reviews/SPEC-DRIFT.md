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
- **Decision**: ✅ resolved — 2026-06-14 PR3 (`06-14-p0-web-fetch-redirect-ssrf`) 实施 `build_redirect_policy` 时同步修复:`web_fetch.rs:15-19` 引用 `build_redirect_policy` 和 `resolve_and_check_sync`,明确说明 "applied to the initial URL AND to every redirect target via `redirect::Policy::custom` callback"
- **Related DEBT**: RULE-E-003 (P0,closed)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

### DRIFT-003 — AuditKind 文档计数散落且过期

- **Type**: 无意遗漏(spec drift,实施侧逐步加 variant 后文档未同步)
- **Spec**:
  - `.trellis/spec/backend/permission-layer.md:401` "### 6. Audit (`session_audit_events`) — 10 类 AuditKind" + `:403` "PR1 ... 实现了 10 类事件" + `:419-430` 表格列 10 行
  - `.trellis/spec/backend/permission-layer.md:454` "10 类 AuditKind 都 serializable"
  - `docs/ROADMAP.md:55` "⑯ 审计日志 10 类 AuditKind"
  - `docs/DESIGN.md:69` "⑯ 审计日志 10 类 AuditKind"
  - `docs/ARCHITECTURE.md:690` "**11 类 AuditKind**"
  - `docs/spikes/2026-06-19-async-parallel-tool-research.md:148` "10 类 AuditKind"
- **Implementation**: `app/src-tauri/src/agent/permissions/audit.rs:4` docstring "intentionally a **single enum** (17 variants)";`audit_kind_round_trip` 测试(`tests_audit.rs:6-36`)覆盖全部 17 个 variant
- **17 个 variant 全集**(对照 audit.rs:36-108):ToolDenied / ToolAllowed / ToolPermissionAsk / ToolExecuted / ToolDeniedYolo / PermissionGranted / PermissionTimeout / RequestCancelled / ModeChanged / YoloEntered / YoloExited / EditMessage / ResendMessage / WorkerAskAllowed / WorkerAskDenied / WorkerAskTimedOut / WorkerAskCancelled
- **Impact**: 文档计数错误(10 / 11 vs 17)误导读者对 AuditKind 全集认知;permission-layer.md 表格缺 7 行(ToolExecuted + EditMessage + ResendMessage + 4 WorkerAsk*)使 spec 失去"全 variant 清单"功能
- **Resolution 候选**:
  - A. 统一现役 spec/docs 为 "17 类",permission-layer.md 表格补 7 行 ✅ 选 A(本 task 范围内)
  - B. 历史 spike/历史 review 标"不改"
- **Decision**: ✅ resolved — task `07-02-spec-drift-audit` (2026-07-02):permission-layer.md 计数 10 → 17 + 表格补 7 行;ROADMAP/DESIGN/ARCHITECTURE/spike 4 处统一为 17;历史 review `REVIEW-agent-loop-full-audit-2026-06-14.md` 不改(Snapshot at 2026-06-14,值为 11)
- **Related DEBT**: 无(Rust 代码层无 bug,纯 spec drift)
- **Discovered In**: SPEC-DRIFT.md "待审" audit (2026-07-02, task `07-02-spec-drift-audit`)

---

## 待审(待 audit 验证)

_全部清空(2026-07-02,task `07-02-spec-drift-audit`)。最近 3 条 audit 结论:_

- ✅ **已审:Memory watcher "立即生效" vs 1s debounce** — 非 drift(spec 已对齐,见 `memory.md:35-45` 顶部 2026-06-15 更新注释明确"notify-based watcher removed, mtime fence";老 debounce 章节保留作 historical reference 是有意为之)。`memory/watcher.rs` grep `debounce` 已无结果,实现确为 mtime fence。
- ✅ **已审:AuditKind 11 vs docstring "10"** — 真 drift(无意),已上升为 **DRIFT-003**(本文上一段),决议选 A:统一文档为 17 + permission-layer.md 表格补 7 行。
- ✅ **已审:`cancel_session_asks` 死代码标注** — 非 drift,误报。grep 8+ 处使用(`commands/sessions.rs:147/150/261` + `permissions/mod.rs:54/135` + `permissions/store.rs:78` 定义 + `permissions/ask.rs:32/358` + `tests_store.rs:8/20` + `tests_ask.rs:66/126/131/141`),函数无 `#[allow(dead_code)]` 标注,活跃代码,接入 `delete_session` 销毁路径(per RULE-B-002)。

---

## 维护说明

- 每次 audit 检查 DRIFT-001 ~ DRIFT-NNN 状态更新
- 新发现 drift 加入本文件 + 关联 DEBT.md RULE
- spec 修订 PR 必须先在本文件标"resolved by PR #N",然后再 archive 到历史段

---

**最后更新**: 2026-06-14 by carlos