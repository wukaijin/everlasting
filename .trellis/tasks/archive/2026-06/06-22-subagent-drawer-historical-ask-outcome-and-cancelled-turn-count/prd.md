# Subagent Drawer — 历史 ask outcome + cancelled turn 数据

> **Bundle 两条 DEBT P3**(都是 subagent drawer redesign PR6 的降级 follow-up,2026-06-21/22 记录):
> - **RULE-WorkerAsk-001** — worker permission_ask 历史卡片不显示 resolve outcome
> - **RULE-FrontSubagent-004** — cancelled 终态无 turn 数据(drawer 只显 wall-clock)
>
> 共同主题:**subagent drawer 历史回放的两个缺失维度**。Session 62(commit `89e5ba1`)落地了 worker ask 的 **live 交互**审批;本 task 补 **historical 回放**的 outcome 维度 + cancelled/incomplete 的 turn 维度。

---

## Goal

让 SubagentDrawer 在 worker run **结束后重开**(historical 回放)时,完整呈现两个当前丢失的维度:

1. **worker permission_ask 的审批结果** —— 现在历史 ask 卡片只能显中性 "worker asked for X",无法回溯「已允许 / 已拒绝 / 超时 / 已取消」(live 期间交互卡正常,historical 才丢)。
2. **cancelled / incomplete 终态的实际执行 turn 数** —— 现在 drawer 只显 wall-clock "at X.Xs",PRD R23 原本要 "at turn N",因 `subagent_runs` 表无 turn 列而降级。

---

## What I already know

### DEBT 锁定的需求(已查 `.trellis/reviews/DEBT.md`)

- **RULE-WorkerAsk-001** Fix 字段自己给方案二选一:(a) `TranscriptKind` 加 `PermissionAskResolved` 条目;(b) 扩 `subagent_runs` schema 加 resolve 维度。**推荐 (a)** —— transcript 已是 worker 的权威审计记录。
- **RULE-FrontSubagent-004** Fix: `subagent_runs` 加 `turn_count` 列 + worker agent loop 持久化实际 turn;前端 cancelled chip 改读 turn_count。

### 代码调研发现(已 grep + Read)

**RULE-WorkerAsk-001**:
- `TranscriptKind` enum(`agent/subagent.rs:421`)现有 4 变体:ChatEvent / ToolCall / ToolResult / PermissionAsk。
- `SubagentBufferSink::emit_permission_ask`(`subagent.rs:1071-1123`)双发 `permission:ask` IPC + `TranscriptKind::PermissionAsk` transcript entry。**代码注释自承** resolve 不写 transcript(`:1097-1100` "The resolve side ... does NOT write a follow-up audit row")。
- worker ask 的 resolve outcome **就在同一 `ask_path` worker 分支可见**(`permissions/mod.rs:1099-1168`):`:1157` emit ask → `:1165` register_ask → `:1168+` `tokio::select!{cancel, timeout, rx}`。select 返回的 `PermissionResponse`/超时/取消臂 = outcome。**sink 就在 ask_path 参数里**,无需跨模块找句柄 → A 方案零跨模块耦合。
- 前端 `PermissionAskBody.vue:207-209` historical 分支注释自承 "The transcript does not yet persist the resolve outcome (N4 — known limitation)"。
- 前端 pairing:`pairSections`(`utils/transcriptPairing.ts`)把 `permission_ask` 作 standalone section,`PermissionAskPayload` 带 `rid` —— outcome 可按 rid 配对回 ask 卡。

**RULE-FrontSubagent-004**:
- `subagent_runs` schema(`db/migrations.rs:470-576`)有 task / final_text / status(5 值 CHECK) / token_usage_json / summary / transcript_json / transcript_truncated / started_at / finished_at,**无 turn_count**。
- `update_run_finished`(`db/subagent_runs.rs:334`)签名接受 `final_text` + `token_usage_json`,**无 turn_count 参数**。
- `SUBAGENT_MAX_TURNS = 200`(`agent/subagent.rs`)是常量,不持久化实际执行 turn。
- `SubagentDrawer.vue` `statusDisplay`(`:344-384`)用 `terminalDurMs`(wall-clock)显示 cancelled → "stopped at X.Xs" / incomplete → "incomplete at X.Xs"。

---

## Requirements

### R1 — TranscriptKind 加 PermissionAskResolved + 后端记录 outcome(RULE-WorkerAsk-001)

- `TranscriptKind` 加第 5 变体 `PermissionAskResolved`(snake_case wire = `"permission_ask_resolved"`)。
- `SubagentBufferSink` 加 `emit_permission_ask_resolved(rid, outcome)` 方法,写 `TranscriptKind::PermissionAskResolved` transcript entry(payload_json = `{ rid, outcome }`)。
  - **不双发 IPC** —— outcome 只进 transcript(前端 pairing 从 transcript 读;live 期间交互卡的消失已由 permissions store rid 移除体现)。
- `ask_path` worker 分支(`permissions/mod.rs:1099-1168`):`tokio::select!` 返回后,按分支确定 outcome 调 `emit_permission_ask_resolved`:
  - oneshot 臂收 `PermissionResponse::AllowOnce` / `AllowAlways` → `"allow"`(worker AllowAlways 当 AllowOnce,已在 Session 62 落地)
  - oneshot 臂收 `PermissionResponse::Deny` → `"deny"`
  - timeout 臂 → `"timeout"`
  - cancel 臂 → `"cancel"`
- outcome 枚举用字符串 wire(`"allow" | "deny" | "timeout" | "cancel"`),四态全做(DEBT 锁定)。

### R2 — 前端 historical ask 卡渲染 outcome badge(RULE-WorkerAsk-001)

- `subagentRuns.ts` `TranscriptEntryKind` union 加 `"permission_ask_resolved"`(对齐 backend wire)。
- `pairSections` / drawer pairing:把 `permission_ask_resolved` entry 按 `rid` 配对到对应 ask 卡(rid 来自 PermissionAskPayload);outcome 透传给 `PermissionAskBody` historical 渲染。
- `PermissionAskBody.vue` historical 分支:有 outcome 时显示 outcome badge(✓ 已允许 / ✗ 已拒绝 / ⏱ 已超时 / ⊘ 已取消),无 outcome(老 transcript / pre-本 task 行)降级显中性文案(向后兼容)。

### R3 — subagent_runs 加 turn_count 列 + worker loop 持久化(RULE-FrontSubagent-004)

- `subagent_runs` 加 `turn_count INTEGER` 列(nullable,向后兼容老行 NULL)。migration 用 `add_subagent_runs_column_if_missing` 既有 helper(幂等)。
- `update_run_finished`(`subagent_runs.rs:334`)加 `turn_count: Option<i64>` 参数 + 写入。
- worker agent loop(chat_loop.rs worker 路径)数 turn(每轮 turn boundary +1),终态 `update_run_finished` 传入实际执行 turn 数。

### R4 — 前端 cancelled/incomplete chip 显示 turn(RULE-FrontSubagent-004)

- `SubagentRunRow` TS type 加 `turnCount?: number | null`。
- `SubagentDrawer.vue` `statusDisplay` cancelled / incomplete 分支:`turnCount` 非空时显 "at turn N",NULL(老行)降级显 wall-clock "at X.Xs"(向后兼容)。

---

## Acceptance Criteria

### RULE-WorkerAsk-001
- [ ] worker ask 被 Allow → 重开 drawer 历史卡显「✓ 已允许」badge。
- [ ] worker ask 被 Deny → 显「✗ 已拒绝」。
- [ ] worker ask 超时(30s oneshot timeout)→ 显「⏱ 已超时」。
- [ ] worker run 被取消(parent cancel)→ 显「⊘ 已取消」。
- [ ] pre-本 task 的老 transcript(无 resolved entry)→ 降级显中性 "worker asked for X",不崩。
- [ ] live 期间交互卡行为不变(Session 62 已落地不回归)。

### RULE-FrontSubagent-004
- [ ] cancelled run 重开 drawer → statusDisplay 显 "stopped at turn N"。
- [ ] incomplete run(max_turns 终态)→ 显 "incomplete at turn N"。
- [ ] pre-本 task 老 run(无 turn_count,值 NULL)→ 降级显 wall-clock "at X.Xs"。
- [ ] completed run 的耗时显示不变(仍 wall-clock)。

### 全局
- [ ] `cargo test --lib` 全 pass(含新增 sink-level outcome 测试 + db migration 测试)。
- [ ] `vitest` 全 pass(含新增 pairing outcome 配对测试 + drawer turn 渲染测试)。
- [ ] `vue-tsc --noEmit` 0 error。
- [ ] 0 新 warning(对照 baseline:4 pre-existing streamController unhandled rejection = RULE-FrontTest-001 债)。

---

## Definition of Done

- 测试新增/更新(unit sink + db + 前端 pairing/drawer)。
- spec 同步:`.trellis/spec/backend/tool-contract.md`(transcript wire 加 resolved kind)+ `subagent-runs-schema.md`(turn_count 列)+ `agent-loop-architecture.md`(worker ask outcome 记录点)。
- DEBT.md 回填 commit hash + Status → closed。
- 四段式 commit(fix → docs(debt) → archive → journal)。

---

## Technical Approach

### 方案 A(采用)— RULE-WorkerAsk-001 outcome 走 transcript entry

- **How**: TranscriptKind 加 `PermissionAskResolved`,`ask_path` worker 分支 select 返回后调 `emit_permission_ask_resolved`,持久化进既有 `transcript_json`(4MB cap 内,每 ask 仅多一小 entry)。
- **Pros**: outcome 与 ask 同代码路径零跨模块耦合;transcript 已是权威审计记录,前端 pairing 按 rid 配对机制现成;不新增 DB 列/表。
- **Cons**: transcript_json 额外占额度(每 ask +1 entry,影响极小);前端 pairing 要按 rid 二次配对(但 rid 已在 payload)。

### 方案 B(否决)— subagent_runs schema 加 resolve 维度

- 否决理由:transcript 已是 worker 审计单一 source of truth;scheme 加列要改 migration + Row + get/list 查询,比 transcript entry 重;outcome 与 transcript 里的 ask 会成两处来源。

### turn_count — 直接加列

- `add_subagent_runs_column_if_missing` 既有 helper 幂等加 `turn_count INTEGER`;`update_run_finished` + worker loop 计数接线。

---

## Decision (ADR-lite)

**Context**: Session 62 落地 worker ask live 交互审批后,historical 回放仍丢 outcome;drawer cancelled/incomplete 只显 wall-clock 不显 turn 进度。两条都是 PR6 redesign 的明确降级 follow-up。

**Decision**:
- outcome 走 **方案 A**(TranscriptKind::PermissionAskResolved entry),DEBT 推荐且零跨模块耦合。
- turn_count 直接加 `subagent_runs` 列(既有 migration helper 幂等)。
- 四态 outcome 全做(allow/deny/timeout/cancel);turn_count 覆盖 cancelled + incomplete(对称 max_turns/取消两终态)。
- 向后兼容:老 transcript(无 resolved)→ 中性文案;老 run(无 turn_count)→ wall-clock。

**Consequences**: transcript_json 略增(outcome entry);前端 pairing 加 rid 配对逻辑(现成机制扩展);DB schema +1 列(幂等 migration)。未来 main-chat ask 历史回放若也要 outcome,同一 transcript entry 机制可复用。

---

## Out of Scope

- main-chat(非 worker)permission_ask 的历史 outcome 回放 —— 当前 main-chat ask 的 IPC/lifecycle 不同,不在本 task 范围(可未来复用 resolved entry 机制)。
- RULE-FrontTest-001(streamController 4 个 unhandled rejection)—— 独立测试债,不在本 task。
- transcript_json 4MB cap 提升 / outcome entry 压缩 —— 现有 cap 足够,不做。
- worker ask outcome 写 `session_audit_events`(RULE-A-016 明确不写,transcript 是 worker 审计)。

---

## Implementation Plan (small PRs)

### PR1 — RULE-WorkerAsk-001(outcome transcript entry)
- 后端:TranscriptKind 加 `PermissionAskResolved` + `transcript_kind_str` wire + `emit_permission_ask_resolved` sink 方法 + `ask_path` worker 分支 select 后接线四态。
- 前端:`subagentRuns.ts` kind union + `pairSections`/pairing 按 rid 配对 outcome + `PermissionAskBody.vue` historical badge。
- 测试:sink-level outcome 四态单测 + 前端 pairing 配对单测。

### PR2 — RULE-FrontSubagent-004(turn_count 列)
- 后端:migration 加 `turn_count` 列 + `update_run_finished` 参数 + `SubagentRunRow` 字段 + worker loop 计数传出。
- 前端:`SubagentRunRow` type + `SubagentDrawer.vue` statusDisplay cancelled/incomplete 读 turn_count(降级 wall-clock)。
- 测试:db migration 幂等 + drawer 渲染(turn 显 / NULL 降级)。

> 两条独立,PR1/PR2 顺序无关;合一个 task 走 trellis,PR1 先(纯 transcript,无 schema 改动风险更低)。

---

## Technical Notes

### 关键 file:line
- TranscriptKind enum:`app/src-tauri/src/agent/subagent.rs:421`
- emit_permission_ask:`app/src-tauri/src/agent/subagent.rs:1071-1123`
- transcript_kind_str wire:`app/src-tauri/src/agent/subagent.rs:468-473`
- ask_path worker 分支(select outcome):`app/src-tauri/src/agent/permissions/mod.rs:1099-1168`
- update_run_finished:`app/src-tauri/src/db/subagent_runs.rs:334`
- subagent_runs schema + add_subagent_runs_column_if_missing:`app/src-tauri/src/db/migrations.rs:470-576`、`:688-707`
- PermissionAskBody historical:`app/src/components/chat/PermissionAskBody.vue:207-209`
- pairSections:`app/src/utils/transcriptPairing.ts`、SubagentDrawer.vue:223
- SubagentDrawer statusDisplay:`app/src/components/chat/SubagentDrawer.vue:344-384`

### 约束
- transcript_json 4MB cap(truncate_transcript_for_persistence head+tail);outcome entry 必须在 cap 内(每 ask +1 小 entry,可接受)。
- outcome 枚举 wire 用字符串(serde rename snake_case,对齐现有 TranscriptKind 风格)。
- turn_count nullable —— pre-本 task 老行 NULL,前端必处理降级。
- worker AllowAlways 当 AllowOnce(Session 62 已落地,outcome 统一记 "allow")。

### 相关 DEBT / spec
- DEBT:`.trellis/reviews/DEBT.md` RULE-WorkerAsk-001 / RULE-FrontSubagent-004
- spec:`.trellis/spec/backend/tool-contract.md`(transcript wire)、`subagent-runs-schema.md`、`agent-loop-architecture.md`、`.trellis/spec/frontend/chat.md`
- Session 62 worker interactive 审批基线:commit `89e5ba1`
