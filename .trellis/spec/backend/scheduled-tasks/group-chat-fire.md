<!-- Moved from scheduled-tasks.md 2026-09-28 (doc-split): group_chat 档 fire scenario -->

# Scheduled Tasks — group_chat 档 fire(定时审议,GCE-M4a,09-07-gce-m4a-scheduled-deliberation)

> hub:[scheduled-tasks.md](../scheduled-tasks.md)(F2 调度判定与 fire 主契约)。

## Scenario: group_chat 档 fire(定时审议,GCE-M4a,09-07-gce-m4a-scheduled-deliberation)

### 1. Scope / Trigger

- 触碰 `fire_group_chat` / `route_prior_session` / `GroupChatTaskConfig` 校验 /
  `last_fire_outcome` / 定时场转录导出(`group_chat_transcript.rs`)的任何逻辑。
- 为什么需要 code-spec 深度:容错路由有四臂且有两条「反直觉不动作」臂;计数
  矩阵与 F2b 契约有交叉;凭直觉写会烧 token(评审 5 P1 实证)。

### 2. Contracts

- **四态路由显式判定**(不做「终态/无」笼统兜底):busy(内存
  `session_active_request` 命中)→ skip;interrupted + checkpoint(round<30)
  → resume(P1a 五闸全套);interrupted 无 checkpoint → 审计 error **本期不动**
  (resume 闸③必拒,降级开新场 = 双活场风险);僵尸(round≥30)与停摆
  (stop_reason NULL 且无 checkpoint)→ 补 `finalize_group_chat_lifecycle(error)`
  + 审计 `recovered` → 开新场;终态/无 → 开新场。防御臂:旧场已删 / 非群聊行
  / DB 读失败 → OpenNew(且先排 busy,无在跑场被并开)。
- **计数矩阵**(F2b「只计真正送入 chat_inner 的 fire」的 group_chat 对齐):
  全臂消费 due(`last_fired_at` 记 due);`run_count` 只计 started / resumed /
  开新场 Err(含建场失败,per_run 先例);skip / resume 拒绝 / precheck 不过
  **不计**——否则 weekly+max_runs 撞 N 次 busy 提前烧完预算。
- **catalog 预检先于建场**:moderator + 全部 participants 查 models 表,缺失
  → audit error(`model_missing`)不建场(否则 model 被删后每周期落空壳
  session 循环)。TOCTOU 窗口接受(chat_inner Err 有审计兜底)。
- **半透传 sink**(不是 NullSink):`ScheduledGroupChatSink` 只透传
  `ChatEvent::Done`(收官 toast 唯一事件源)+ `permission:ask` +
  `has_live_observer()` 透传 registry —— 恒 false 会废掉 GC3 8s 快拒并掐死
  盯场观察者的弹窗。
- **转录导出守卫挂 `GroupChatCtx.created_via`**(additive 载体,metadata
  `created_via=="scheduled"` 读入):终态块全通道共享,守卫缺失会把 GUI/MCP/
  script 场也导出。导出挂 checkpoint keep-or-delete **之前**(读 started_at);
  失败仅 warn 不影响终态落库。文件名 sanitize:whitelist(控制符与路径分隔
  剥离、连续点折叠防 `..`、空白折一、CJK 保留、截 40)。
- **CHECK 不变式**:`target_mode='group_chat' ⇒ target_session_id IS NULL AND
  group_chat_config IS NOT NULL`(table rebuild 沿 per_run 五步舞;幂等断言)。
  `validate_target_session` 对 group_chat 目标的 400 拒绝**保持不动**(fire 进
  空闲群聊会抹旧场 summary——M3 实证)。
- 主题原样发题(F2 注脚不进群聊议题);归因走建群 metadata 三键
  (`created_via="scheduled"` + `scheduled_task_id` + `scheduled_task_name`)。

### 3. Wrong vs Correct

- ❌ busy 跳过计 `run_count` → max_runs 预算被空转烧完;✅ skip 消费 due 不计数。
- ❌ resume 拒绝时「兜底开新场」→ 与僵死旧场双活双烧;✅ 本期不动,下周期重路由。
- ❌ NullSink 省事 → GC3 快拒失效 + toast 无事件源;✅ 半透传。
- ❌ 在编排器终态块里按 session_type 挂导出 → GUI 群聊全部被导出;✅ created_via 守卫。

### 4. Tests Required

五组(`scheduler/tests_tick.rs` + `agent/tests_group_chat.rs` +
`group_chat_transcript.rs` + `db/migrations_tests.rs`):tick 四臂路由 /
计数矩阵(嵌入各臂断言)/ resume 兜底(dispatch seam 注入拒绝 + 无 checkpoint
构造;**直调 fire 的用例 attach 后必须 `task_row` 重载**——fixture 裸 SQL 改行,
内存结构体是陈旧的)/ 迁移 rebuild 幂等 + CHECK / 对照组(fixed/per_run 既有
用例零改动 + kill-switch 同效 + 非 scheduled 场不导出)。
