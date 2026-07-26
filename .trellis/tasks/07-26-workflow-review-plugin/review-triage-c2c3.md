# 评审意见分类（C2↔C3 定向评审 — MiniMax-M3 + DeepSeek-v4-pro）

> 评审日期：2026-07-26。两份评审独立收敛于事件机制/原子写，但都漏了最关键的 TaskStatus enum 硬约束（我核查代码自行发现）。
> 本文档记录每条意见的独立判断：采纳 / 部分采纳 / 不采纳 + 理由。

## 🔴 最关键发现（我核查代码发现，两份评审都没点透）

### TaskStatus enum 硬编码 4 值，review 的 4 state 存不进 task.json.status

核查「transition 钩子是否存在」时顺藤摸瓜发现：
- dev plugin 的 state（planning/in_progress/done）**就是 task.json.status**（task.rs:87-94 注释明说）—— 不是两个系统，是同一个
- `TaskStatus` enum（task.rs:122）只有 `Planning/InProgress/Done/Completed`
- `from_str_opt`（task.rs:130）把**任何非已知值 fallback 到 Planning**（line 139）
- 后果：review 的 intake/reviewing/revising/reported 写进去读回来全塌成 Planning，状态机跑不起来

**两份评审的相关判断**：
- DeepSeek 说「state.rs 钩子是 task.json 状态机，与 workflow plugin session 状态机是两个系统」—— **事实错误**，它们是同一个系统
- MiniMax 说「workflow 状态机完全靠主 LLM 自觉推进，Rust 端无状态机」—— **也错**，state 存在 task.json.status，set_task_state 是写入入口
- 但两者歪打正着都得出「transition 自动钩子不存在」——这个结论部分正确（自动钩子确实没有，但 transition 触发点 `request_task_state_transition` 存在）

**处置**：新建 C0 子任务（TaskStatus 容纳自定义 state），方案分析见 `research/taskstatus-custom-state.md`。推荐方案 X（enum + Custom(String)）。**这是 review epic 的新硬前置，顺序调整为 C0 → C1 → C3 → C2。**

## ✅ 采纳的真问题（两份独立收敛）

### 1. RunStatus 枚举不一致（C3 写 3 值，C2 写 5 值）— MiniMax 抓到

- 事实：DB `subagent_runs.status` CHECK 约束已是 5 值（running/completed/cancelled/error/incomplete）
- C3 schema 注释只列 `completed/failed/timed_out`（3 值），C2 TS 列 5 值
- **采纳**：C3 扩到 5 值。**注意命名对齐**：DB 真值是 `error/incomplete`（非评审说的 failed/timed_out），implement 时以 DB 真值为准。

### 2. write_file 不原子 — 两份都抓到

- 事实：`write_file.rs:163` 是 `tokio::fs::write`，非 tmp+rename；唯一原子写是 `task.rs:373 write_task` 的 `fs::rename`
- 后果：主 LLM 写 review-state.json 中途，前端读到半截 JSON
- **采纳**：C3 design §4 改「write_file 内部保证原子」为「不原子」。采用 MiniMax 方案 iii（新建 review-only `emit_review_state_updated` 工具，内部 tmp+rename 原子写）—— 一举解决原子性 + 事件发送点。

### 3. 事件发送点：transition 钩子不存在 — 方向采纳 MiniMax 方案 iii

- 事实：`request_task_state_transition` 工具（chat_loop.rs:3540）是 transition 触发点且有 emit；但**自动钩子（transition 后回调）不存在**
- 更根本：TaskStatus enum 问题（见上）让 transition 机制本身需要先重构
- **采纳 MiniMax 方案 iii**：新建 review-only `emit_review_state_updated` 工具（仿 ask_user_question 模式，拿 ChatEventSink），解耦 transition 机制 + 自带原子写 + 零 dev 污染
- **否决** DeepSeek 方案 a（在 chat_loop transition 处发 ChatEventPayload::ReviewStateUpdated）—— 推理基于「workflow session state 独立于 task.json」的错误前提
- **否决** C2 design 原倾向的「transition 钩子」—— 不存在

### 4. task_slug 获取：新 IPC `get_current_task_slug` — 两份都推荐

- 事实：前端无 task slug 暴露，`resolve_current_task` 在后端
- DeepSeek 指出 implement.md「方案 b 从 workflow state IPC 暴露」不可行（plugin ≠ task）—— **对**
- **采纳**：新 IPC `get_current_task_slug`，返回 `{slug, id, title, status}`（DeepSeek 的轻量优化）。删除 implement.md 方案 b。

## 🟡 采纳的次要问题

### 5. useReviewStateStore 幂等 + currentSlug 守门（MiniMax §2.4）

**采纳**：start() 前先 stop() + `if (currentSlug === taskSlug && unlisten) return` 幂等，防 slug 变化时孤儿监听。比现有 subagentRuns.ts 做得更好。

### 6. PRD vs design schema 漂移（DeepSeek §1.2）

**采纳**：C3 PRD R7 还是旧版 schema（无 schema_version/finding_id/source_run_id/triage/change_log/convergence_note/status/models_present 等扩展字段），design.md §4 声称「PRD R7 已给」是虚假声明。同步 PRD R7 到扩展版。

### 7. 前端 JSON 解析容错（DeepSeek §1.3）

**部分采纳**：LLM 写的 JSON 可能有 trailing comma 等。走方案 iii（review-only 工具）后，主 LLM 调工具而非直接 write_file，但工具内部仍是主 LLM 提供的 JSON 字符串——容错仍需要。前端用 json5 或容错解析。

## 🔴 不采纳

### 8. DeepSeek「state.rs 钩子是 task.json 状态机，与 workflow plugin session 状态机是两个系统」

**不采纳，事实错误**。核查代码确认 dev 的 state == task.json.status（task.rs 注释明说）。DeepSeek 这个误判让它给出「transition 钩子方案基于错误理解」的结论——结论部分正确（自动钩子不存在）但推理前提错。

### 9. DeepSeek 方案 a（chat_loop transition 处发 ChatEventPayload::ReviewStateUpdated）

**不采纳**。方向对（在 transition 处发）但基于错误前提（误以为 workflow session state 独立于 task.json）。实际上 transition 就是改 task.json.status，方案 a 实现位置要对齐到 TaskStatus 重构之后。倾向 MiniMax 方案 iii（review-only 工具，解耦 transition 机制）。

## 处置清单（待落档）

- [ ] 新建 C0 子任务（TaskStatus 容纳自定义 state），方案分析已写 `research/taskstatus-custom-state.md`
- [ ] 父任务依赖表更新：C0 → C1 → C3 → C2
- [ ] C3 schema：RunStatus 扩 5 值（命名对齐 DB 真值 error/incomplete）
- [ ] C3 design §4：write_file 原子性假设改为「不原子」，采用 review-only 工具方案
- [ ] C3 PRD R7：同步到扩展版 schema（消除 PRD vs design 漂移）
- [ ] C2 design §2 + §10 + implement Phase 0.2：transition 钩子倾向改为方案 iii（review-only 工具）
- [ ] C2 implement Phase 0.1：task_slug 方案定为新 IPC，删除方案 b
- [ ] C2 design §6：store start() 加 currentSlug 守门 + 幂等
- [ ] C2：前端 JSON 解析加容错（json5 或手写）
