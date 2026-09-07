# daemon 侧代码证据(定时审议设计输入)

> 来源:Explore 子代理 2026-09-07 调研(very thorough)。路径以仓库根为基准。
> 用途:design.md 的锚点输入。

## 1. F2 调度器 fire 链路

- 目录:`app/src-tauri/src/scheduler/` = `mod.rs`(tick + fire)/ `compute.rs`(ScheduleSpec + `most_recent_due`)/ `tests_tick.rs` / `tests_lost.rs`。
- 装配点:`daemon/server.rs:199` `spawn_task_scheduler`(detached tokio::spawn,`SCHEDULER_TICK_SECS = 30`,`scheduler/mod.rs:60`);调用方 `bins/everlasting-daemon.rs:207`。首 tick 立即 = 启动 catch-up。
- tick 主体:`scheduler/mod.rs:232` `scheduler_tick_with_fire(state, pending_by_task, fire: &TickFire)`;`mod.rs:221` 生产入口。
- fire 链:`mod.rs:255` list_enabled → `mod.rs:284` `most_recent_due`(gate 1/2/3 在 279/287/301)→ per_run `mod.rs:503` `create_run_session` → `mod.rs:412` FireContext → `mod.rs:419` `fire(state, ctx)` → 生产 fire = `mod.rs:153` `fire_via_chat_inner`(`build_injection_message` mod.rs:191,`TaskOrigin::Scheduled`)→ `agent::chat::chat_inner`。
- `FireContext`(mod.rs:122-128):`{task_id, task_name, prompt, target_session_id, now_ms}`。
- fire 结果三态:`Queued`(dedup 表)/ `Started` / `Injected`(群聊 busy 注入,mod.rs:420-433)。
- 落账:`mod.rs:539` `account()` → `mark_task_fired`;`mod.rs:475` `maybe_complete_after_fire` gate 4。
- **fire 在 tick future 内串行 await,但编排由 chat_inner spawn 后台跑**——群聊一场 5-15 分钟不会阻塞 tick,fire 只等受理返回。
- kill switch:`SCHEDULED_TASKS_ENABLED_KEY` mod.rs:69(fail-open)。audit action 常量:`mod.rs:96-105`。

## 2. 群聊建群 daemon 内部入口

- HTTP 序列(MCP/脚本):`providers/list_models` → `projects/list_projects` → `sessions/create_session`(body `{project_id, initial_cwd, model: moderatorModel, session_type: 'group_chat', metadata: {participants, created_via}}`,group-chat-run.mjs:158 `buildCreateSessionBody`)→ `agent/chat`(fire-and-forget,`{request_id, session_id, messages:[{role:'user',content:topic}]}`)。
- 建群池级核心(不经 HTTP 直调):`commands/sessions.rs:90`
  `create_session_in_pool(db, project_id, initial_cwd, model: Option<String>, session_type: Option<String>, metadata: Option<serde_json::Value>) -> Result<db::SessionRow, AppCommandError>`。
  **F2 `create_run_session`(scheduler/mod.rs:503-532)已是 tick 内直调建 session 先例。**
- 发题:直调 `crate::agent::chat::chat_inner(state, ChatEntry{..})`(chat.rs:344,pub(crate));群聊 ctx 由 chat_inner 内部自动从 metadata 解析(`agent/group_chat.rs:140` `build_group_chat_ctx` → `GroupChatCtx{participants, moderator_model_id, project_root}`,group_chat.rs:63;调用点 chat.rs:444)。**发题方只需建 session(带 metadata)再 chat_inner。**

## 3. 群聊编排器

- 入口:`agent/group_chat_loop.rs:261` `run_group_chat_loop(...)`(26 参数,fire-and-forget)。
- spawn:`agent/chat.rs:773` `tokio::spawn`;群聊分支 chat.rs:842-874(`ChatEntry.resume_group_chat` → `GroupChatResume{start_round}` chat.rs:845-846)。
- busy = 内存(`AppState.session_active_request: HashMap<session_id, rid>`;claim chat.rs:590-602;编排器退出清理 group_chat_loop.rs:1037-1038)。GC1:整场 busy 不回落。
- 终态写:`group_chat_loop.rs:950-974` → `db::finalize_group_chat_lifecycle(db, session_id, stop_reason_str, end_summary)`(`db/sessions/session_crud.rs:953`)。
- checkpoint keep-or-delete:group_chat_loop.rs:985-994(`cancelled`/`error` 保留 = 可续;终态删除);每轮头 upsert:group_chat_loop.rs:430;轮帽 `MAX_ORCHESTRATION_ROUNDS = 30`(group_chat_loop.rs:97);开新场清 lifecycle:group_chat_loop.rs:331。
- STOP_REASON 常量:group_chat_loop.rs:141-163。

## 4. P1a resume

- daemon route:`daemon/routes/agent.rs:88` `resume_group_chat`(POST `/api/v1/agent/resume_group_chat`)→ `agent/chat.rs:145` `resume_group_chat_inner(state, session_id, sink, worker_event_sink) -> Result<ChatAcceptance, AppCommandError>`。
- Tauri command:`agent/chat.rs:243`。
- 五类校验(chat.rs:131-211):① session_type == GroupChat;⑤ stop_reason 非终态三值;② 不 busy;③ checkpoint 行存在;④ `checkpoint.round < 30`。通过 → chat_inner 空 messages + `resume_group_chat: Some(round)`。
- **定时任务 fire 的 interrupted 自动续 = 直调 `resume_group_chat_inner`(带 sink)。**
- boot sweep:`db/sessions/session_crud.rs:1080` `recover_group_chat_checkpoints`(stop_reason IS NULL + checkpoint 行 → `interrupted`;orphan-heal);调用点 `state.rs:417`(load_inner)。

## 5. scheduled_tasks wire/校验

- routes:`daemon/routes/scheduled_tasks.rs`(POST `{list|create|update|delete}_scheduled_task`;CreateScheduledTaskRequest :41-53;update 双层 Option :107-123;wire 恒 created_by="user" :70)。
- commands:`commands/scheduled_tasks.rs`(:123/:360/:557/:599 薄包装)。
- 池级核心:`commands/scheduled_tasks.rs:192` `create_scheduled_task_in_pool(db, project_id, target_session_id, target_mode, name, prompt, schedule, enabled, created_by, max_runs, ends_at, model_id)`。内部:normalize_target_mode(:132,fixed|per_run 白名单)→ validate_end_conditions(:94)→ parse_schedule(:217)→ once 未来(:220)→ model 存在(:232)→ 目标解析(:255-329)→ insert。
- **群聊 400 拒绝点:`db/scheduled_tasks.rs:405` `validate_target_session`,核心行 :421-423**(`session_type != "chat"` → Err「定时任务只能绑定普通聊天 session(群聊不支持定时注入)」)。调用点 create :273 / update :478。
- `ScheduledTaskRow` db/scheduled_tasks.rs:33;target_modes 常量 :24-26;`mark_task_fired` :357;`mark_task_completed` :385。
- per_run 三绕过(spec):同 session 串行化(fired_sessions)、队列去重(pending_by_task)、queue-disabled gate。

## 6. GUI 先例

- P1a 可续跑通知:**非 SSE 驱动**,是状态驱动:`ChatPanel.vue:252` `isGroupChatResumable` computed(`!streaming + stop_reason ∈ {interrupted, cancelled, error}`);chip + 续跑按钮 ChatPanel.vue:796-816;store action `app/src/stores/chat.ts:948` `resumeGroupChat()`;终态字段合并 `streamEvents.ts:1430-1447`;HTTP transport 域映射 `transport/http.ts:54-70`。
- F2 Settings tab:`app/src/components/settings/ScheduledTasksTab.vue`(1880 行;表单 :712 起;kind 分支 :918-1020;`FormTargetMode = "existing"|"dedicated"|"per_run"` :201;目标选项只列 classic session :362)。配套 `stores/scheduledTasks.ts`、`utils/scheduledTaskFormat.ts`、入口 `SettingsModal.vue`;session header 定时徽章 ChatPanel.vue:822-829。

## 7. 审计

- `agent/permissions/audit.rs:212` `AuditKind::ScheduledTaskFired`(声明 :34;变体文档 :200-211);`as_str` :261 → `"scheduled_task_fired"`。
- 写审计:`audit.rs:668` `record_scheduled_task_audit(db, session_id, task_id, task_name, action: &str, reason: Option<&str>)`。**action 是自由字符串不是枚举**——新 action 只需 `scheduler/mod.rs:96-105` 加常量 + audit.rs 变体文档补条目。

## 8. 补充关键事实(设计直接相关)

1. **fire 打空闲群聊会开新场、毁灭上一场 summary**(M3 guard 注释 group-chat-run.mjs:181-184)——`validate_target_session` 的 400 拒绝就是为避开这个;群聊任务类型的 fire 必须绕开「注入到既有群聊」路径。
2. tick 内直调建 session + chat_inner 均有先例(create_run_session / fire_via_chat_inner)。
3. MCP 建群 metadata:`{participants, created_via:'mcp'}`;三通道归因 "mcp"/"script"/缺失=GUI(AGENTS.md)——定时任务 fire 需要第四值(如 `'scheduled'`)。
