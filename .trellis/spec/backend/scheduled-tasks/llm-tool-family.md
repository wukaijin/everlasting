<!-- Moved from scheduled-tasks.md 2026-09-28 (doc-split): LLM 调度家族 scenario -->

# Scheduled Tasks — LLM 调度家族(agent 作者面,08-29-schedule-task-tool)

> hub:[scheduled-tasks.md](../scheduled-tasks.md)(F2 调度判定与 fire 主契约)。

## Scenario: LLM 调度家族(agent 作者面,08-29-schedule-task-tool)

### 1. Scope / Trigger

- 触碰「agent 创建/查询/删除定时任务」的任何逻辑(`tools/scheduled_task_family.rs`
  三件 + `create_scheduled_task_in_pool` 链);改动 `created_by` 语义或 tool 侧 gate。
- fire 侧(hub 全部契约)**零改动且与 created_by 无关** —— agent 建的任务
  走同一 origin 链/去重/catch-up/审计。

### 2. Contracts

- **作者面分离**:`created_by ∈ {'user','agent'}`。tool 路径(`schedule_task`/
  `schedule_status`/`schedule_cancel`,见 [tool-contract 17](../tool-contract/17-schedule-task-family.md))
  写/查/删全部限 `'agent'` 行;两个 transport 包装(Tauri command + daemon route)
  恒传 `'user'` —— `create_scheduled_task_in_pool` 是 pool 级核心,`_inner` 薄包装
  (Q0 单源不变,`ToolContext` 无 AppState 故抽池级)。
- **tool 侧双 gate 不进核心**:kill switch(同键 `SCHEDULED_TASKS_ENABLED_KEY`,
  仅拦 create)与 per-project 活跃 agent 任务上限(`MAX_ACTIVE_AGENT_TASKS=20`,
  `count_enabled_by_creator`;TOCTOU 有意接受)都只存在于 tool 层 —— 用户 UI/IPC
  创建路径行为零变化,这是「gate 放 tool 不放 `_in_pool`」的定案理由。
- `ScheduledTaskPayload.created_by` 暴露到 wire(additive);前端 Settings 任务卡
  `created_by==='agent'` 渲染「agent」来源徽标,user 不渲染(缺省态零噪音)。
- 隔离:worker `STRUCTURALLY_DISABLED` 三员;群聊 `group_chat_tool_defs` 白名单
  天然排除(零改动);**禁止**反向动 `filter_tools_for_session_type`(方向相反,
  review P1 实证)。

### 3. Tests Required

见 [tool-contract 17 §3](../tool-contract/17-schedule-task-family.md);本域涟漪:
db 层 `created_by_agent_persists_and_filters_by_creator`(作者过滤正负向 +
None 不过滤)。
