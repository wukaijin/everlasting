## Scenario: `schedule_task` / `schedule_status` / `schedule_cancel` — LLM 调度家族(`08-29-schedule-task-tool`,2026-08-29)

> ROADMAP F1/F2 点名的 follow-up「LLM detached dispatch」:把 F2 daemon 调度器
> 暴露给 LLM。任务规划(prd/design/implement/e2e)在 `.trellis/tasks/08-29-schedule-task-tool/`;
> fire 侧语义(判定/origin 链/去重/catch-up)零改动,见 [scheduled-tasks.md](../scheduled-tasks.md)。

### 1. Scope / Trigger

- 触碰 LLM 建/查/删定时任务的任何逻辑;新增「agent 作者面」工具;
  `scheduled_tasks.created_by` 语义;C7D 预算线再校准。
- 零新表零 migration(`created_by` 列 F2 起就有;`insert_scheduled_task` 的
  `'user'` 字面量参数化是兑现 db 注释预告)。

### 2. 契约

```rust
// tools/scheduled_task_family.rs — 三件一个模块(命名家族镜像 L1a 三件套)
pub fn definition() -> ToolDef;          // "schedule_task"   create
pub fn status_definition() -> ToolDef;   // "schedule_status" list(本项目 agent 行)
pub fn cancel_definition() -> ToolDef;   // "schedule_cancel" 硬删(仅 agent 行)
pub async fn execute(input, ctx, session_id) -> (String, bool);      // create
pub async fn status_execute(ctx) -> (String, bool);
pub async fn cancel_execute(input, ctx) -> (String, bool);
```

- **plain dispatch**(创建是快 DB 写,无 chat_loop 拦截;`execute_tool_inner`
  三个普通 arm);`builtin_tools()` **尾部追加**,家族相邻、task→status→cancel
  序(prefix cache 契约)。
- **作者面分离(核心定案)**:tool 路径写/查/删全部限 `created_by='agent'`;
  用户建的任务对家族不可见不可删(ownership guard:cancel 先 get 再校验
  created_by,不匹配结构化报错);用户 UI/IPC 路径恒 `'user'` 行为零变化。
- **Q0 单源**:`create_scheduled_task_in_pool(db, …)` 是 pool 级核心(校验矩阵
  全复用),`_inner(&Arc<AppState>)` 是薄包装。**为什么**:`ToolContext` 只有
  `db` + `project_id`,没有 AppState;专用 session 分支同理走
  `create_session_in_pool`。**给未来 tool 的范式:工具需要 AppState 能力时,
  抽 pool 级核心 + 薄包装,不要往 ToolContext 塞 AppState。**
- **tool 侧双 gate(只拦 agent,不进 `_in_pool`)**:
  1. kill switch:`scheduled_tasks_enabled == "false"`(字面,与 tick 同键同
     判定,常量 `scheduler::SCHEDULED_TASKS_ENABLED_KEY` 共享)→ 拒创建;
     status/cancel 不受限(管理动作恒可用)。**不进 `_in_pool` 是有意的**:
     用户 UI 创建路径不受 kill switch 影响(行为零变化)。
  2. 反滥用上限:`count_enabled_by_creator(project, "agent") >= 20`
     (`MAX_ACTIVE_AGENT_TASKS`)→ 拒,错误信息指引先 cancel(自愈闭环)。
     COUNT→INSERT 的 TOCTOU 竞态**有意接受**(软上限语义,勿原子化)。
- **`end_date` 入参是 ISO 日期字符串**(LLM 对墙上钟可靠、对 epoch 易错),
  tool 侧转当日 23:59:59.999 本地 epoch ms(F2b「结束日当天仍触发」);
  `in_current_session: bool`(缺省 false)= target 传当前 session(工具不可
  见于群聊 + `validate_target_session` 纵深防御)。不暴露裸 `target_session_id`。
- **权限:全部 Tier 5 silent Allow**(`classify_tool` **不加 arm**,落
  `ToolKind::Other`;`classify_tool_keeps_schedule_family_silent_allow` 测试
  钉住「无 arm」—— 若未来改路由 Tier 4 即变更 Q2 定案,须重评审)。补偿控制
  = 上限 + 作者面分离 + worker/群聊隔离 + kill switch + 审计(ToolAllowed)。
- **隔离**:worker 侧进 `STRUCTURALLY_DISABLED`(编排面归 parent,镜像
  `create_task`);群聊侧**零改动** —— `group_chat_tool_defs` 穷举白名单天然
  排除新工具(08-07 黑名单改白名单教训的性质)。**禁**把家族加进
  `filter_tools_for_session_type` 剥除名单(那是从普通 chat 剥群聊专属工具的
  名单,方向相反 —— 本任务 review P1 实证过的坑)。
- **C7D**:三具入 `STUB_CANDIDATES`(11→14,`STUB_DESCRIPTIONS` 定长同步);
  预算线 3960 → 4100(实测 4031,校准注 5;政策 = 先扩候选再平移,
  [15-search-history §4](./15-search-history.md) 同款)。

### 3. Tests

- 工具内联(`tools/scheduled_task_family.rs`,13 例):AC2 短路矩阵 / AC3
  in_current_session / AC1 专用 session + 标题 + created_by / AC5 kill switch
  (create 拒、status/cancel 照常)/ AC6 上限触顶→cancel 自愈→用户路径不受限
  / AC7 cancel 越权拒 + 幂等 / status 作者面可见性 / end-of-day 转换含非法日期。
- 集成(`agent/tests_schedule_task_family.rs`,6 例):**execute_tool 真分发
  臂**(建/查/删 roundtrip、kill switch)、worker 过滤(空 + 显式 allowlist
  双态)、群聊白名单(moderator/participant 双态)、尾部追加序断言。
- 涟漪:`permissions/tests_check.rs` 钉 Tier 5;`classify_tool` 既有 dispatch
  测试不动;预算线校准注 5。
- live E2E:`e2e.sh`(任务目录)——HTTP daemon 真实 LLM 两轮:create 回复
  **原样复述服务端 task_id**(tool_result 消费实证)+ 行 `created_by='agent'`;
  status→cancel 闭环 + 行删除。**坑**:`create_project` 第二参是 name,id 必须
  用返回值(mock 种子曾因此全挂「project 不存在」)。
