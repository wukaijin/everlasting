# V2-2+ 自主记忆可观测性与管理面板

## Goal

把 V2 2 期已落地但完全黑盒的自主记忆系统(agent 自动写 / 自动召回 / 自动晋升)打开成可观测、可管理的面板:用户能看到 agent 召回了什么、为什么某条 memory 处于某状态、并能修正 agent 写错的记忆。

> ROADMAP §2 第三档 active 项。差异化能力,契合项目"学习 harness 工程"目标——透明度本身是 harness 学习教具(商业 agent 工具几乎没有召回可视化)。

## Background

V2 2 期(P1-P5,2026-06-29 落地)已建完整自主记忆闭环:`autonomous_memories` 表 + 状态机(candidate→active→verified→demoted)+ 两层召回(session-start FTS5 + pre-tool pitfall)+ 双写入面(`remember` tool + `auto_reflect`)+ P5 质量层(verified soft-intercept + 自动晋升 + 卫生 job)。但**完全黑盒**:用户看不到 recall 发生、不能修正 agent 写错的记忆、只能 delete。

现状关键(evidence,已调研):

- **数据层就绪**:`AutonomousMemory` wire DTO(`app/src/stores/memory.ts:115`)已含 `status` / `hitCount` / `lastUsedAt` / `confidence` / `demotedReason`,DB 列齐全。MemoryPreview 拿到 row 但**未渲染 hit 相关字段**。
- **后端能力已存在**:`db::memories::update_status(pool, memory_id, new_status, demoted_reason)` 带转换矩阵校验 + `StatusTransitionError::Illegal`(P5),仅缺 IPC 暴露。
- **不存在** `update_memory`(title/content 编辑)— 本任务新增,需复用 `insert_memory` 的写入安全网(500-char cap + sensitive regex)。
- **recall 接入点**:session-start FTS `agent/chat_loop.rs:1385` `build_recall_text`;pre-tool pitfall `agent/chat_loop.rs:2453` / `:3205` `permissions::recall_pitfall`。当前 `bump_hit_count` fire-and-forget,**无 event 到前端**。
- **event 机制**:4 channel(`chat-event` / `tool:call` / `tool:result` / `permission:ask`)走 sink emit。`emit_retrying`(A5+)把 notice 复用 chat-event channel 的先例可借鉴。
- **前端**:runtime section 在 `app/src/components/memory/MemoryPreview.vue`(kind="project",ProjectTabs Memory dropdown),row 渲染 title + kind/scope/status badge + 80 字 preview + delete。`memory/MemoryModal.vue` 已存在但服务于指令文件(layer),非 runtime memory。`stores/memory.ts` 有 `runtimeMemories` / `fetchMemories` / `deleteMemory`,无 update action。

**P5 转换矩阵**(合法集,后端 `update_status` 已校验):

- `candidate → {active, verified, demoted}`
- `active → {verified, demoted}`
- `verified → demoted`
- `demoted → active`

## Requirements

- **R1(状态转换)**:用户能在管理 modal 里手动转换 memory 状态,受 P5 转换矩阵约束;非法转换被 UI 禁用 + 后端拒绝(`StatusTransitionError::Illegal`)。
- **R2a(历史统计可观测)**:memory row 渲染 `hitCount` / `lastUsedAt`(渲染已有数据,纯前端,零后端改动)。
- **R2b(实时召回 event)**:对话过程中,session-start FTS + pre-tool pitfall 两路 recall 命中实时反映到前端(chat_loop 接入点 emit + 前端 listener + UI)。
- **R3(UI 落位)**:扩展 MemoryPreview runtime section + 新建 runtime-memory 管理 modal,不新增 Settings 段。
- **R4(编辑)**:允许编辑 title + content,保留 provenance(区分 agent 写 vs 用户改);走原写入安全网(500-char content cap + sensitive regex)。
- **R5(管理 modal 统一)**:点击 row 打开 modal,统一承载全文 + hit/lastUsed/confidence 统计 + 状态转换下拉 + 编辑 + 删除。

## Acceptance Criteria

- [ ] **AC1**(R2a):runtime row 显示 `hitCount` + `lastUsedAt` chip(`hitCount>0` 时)。
- [ ] **AC2**(R5):点击 row 打开管理 modal,显示 title/content 全文 + `hitCount` / `lastUsedAt` / `confidence` + 当前 status + 来源(`sourceSessionId` / `sourceRef`)。
- [ ] **AC3**(R1):状态转换下拉只列当前 status 的合法目标(转换矩阵驱动),非法目标不出现。
- [ ] **AC4**(R1):状态转换 IPC → 后端 `update_status` → 成功后 row badge + modal 同步;构造非法转换 → `StatusTransitionError::Illegal` → 前端报错(不崩溃)。
- [ ] **AC5**(R4):编辑 title/content → IPC → 后端 `update_memory`(走原安全网:500-char cap + sensitive regex)→ 成功后 row/modal 更新;provenance 标记区分用户编辑。
- [ ] **AC6**(R2b):对话命中 memory 时(session-start FTS 或 pre-tool pitfall),前端收到 recall event 并展示(命中条数 + title 列表)。
- [ ] **AC7**(R2b):worker subagent 的 recall event 不污染主 chat 视图(走 worker sink 隔离,同 L3 模式)。
- [ ] **AC8**(回归):delete 仍工作;新增管理操作不影响 P3/P4/P5 自动闭环。

## Out of Scope

- 不改 P3/P4/P5 自动闭环(召回 / 反思 / 晋升 / 卫生 job 自动行为不变)。
- 不加新 memory kind / 不删 DB 列(provenance 走新列还是复用 `source_ref`,见 design.md 决策)。
- 不做跨 project 全局聚合视图(R3 已定项目视图;Settings 全局审计留 follow-up)。
- 不做批量操作。
- 不做 recall event 的"历史回放"(本次对话命中是实时的;跨对话历史靠 `hitCount`)。
