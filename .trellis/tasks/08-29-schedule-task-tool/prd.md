# LLM schedule_task tool(detached dispatch)

> 任务源:ROADMAP F1/F2 行点名的 follow-up —— 「LLM detached dispatch(`schedule_task` tool)仍开放」。
> F1 C 档(cron 消费者)已由 F2 交付(统一入口 = chat_inner「闲也入队」);本任务把 daemon 调度器
> 暴露给 LLM:agent 在对话中即可自排/查看/取消未来任务,不再只靠用户去 Settings UI 手工建。
> 规划状态:brainstorm 已收敛(2026-08-29,三个产品决策用户定案);技术设计见 `design.md`。

## Goal

给 agent 一组任务调度工具:**create / list / cancel**(用户定案 Q1),创建 F2 定时任务
(`scheduled_tasks` 行),到期由既有 scheduler tick 经 chat_inner 队列触发一轮对话。
零新表、零调度语义改动 —— 纯粹在 F2 基建之上加 LLM 入口。

用户价值:「明早 9 点提醒我 review PR」「每小时检查 CI 直到绿」这类请求,agent 一句话
自排;「把我排的任务取消掉」对话内闭环。列表/取消限 agent 自建任务 —— 用户建的任务
仍归 Settings UI 管,两个作者面互不越界。

## Background(代码取证结论,实施的事实前提)

1. **创建单源复用**:tool 调 `create_scheduled_task_inner`(`commands/scheduled_tasks.rs:122`)
   —— 校验矩阵全量白拿:name/prompt 非空、`validate_end_conditions`、`parse_schedule`
   → 规范 JSON、project 存在、target session 存在且 chat(群聊拒)+ project 归属一致。
2. **created_by 已预留**:`scheduled_tasks.created_by` 列存在,`insert_scheduled_task`
   SQL 硬编码 `'user'`,db 注释明示「F2+ agent 复用同表时改由参数区分」
   (`db/scheduled_tasks.rs:108`)。本任务把它参数化。
3. **fire 链零改动**:`TaskOrigin::Scheduled` origin 载体链 / `metadata.scheduled` 信封 /
   落款注脚 / queue-disabled gate / 去重 / catch-up 对 agent 创建的任务一律天然生效
   (spec `scheduled-tasks.md` 契约与 created_by 无关)。
4. **schedule 形状零新发明**:6 档 `ScheduleSpec`(daily/interval/weekly/hourly/weekdays/monthly)
   + F2b 结束条件(`max_runs`/`ends_at`)原样透传;LLM 填同一 JSON 形状。
5. **权限落点**:`classify_tool` 不加 arm → `ToolKind::Other` = Tier 5 silent Allow
   (既有 tool 审计链自动记 `ToolAllowed`)。**用户定案 Q2:三面全部 silent Allow**,
   理由:创建零立即副作用,真正的执行在 fire 时刻且走完整 mode/permission 链;
   任务 Settings 可见可删可禁(可逆);ask 疲劳与「顺口一句」的定位相悖。
   补偿控制 = 反滥用上限(R4)+ worker/群聊隔离 + kill switch + 审计。
6. **kill switch 现状**:`scheduler_tick` 每tick读 `scheduled_tasks_enabled`(fail-open,
   `scheduler/mod.rs:229-232`),false 时空转不 fire。**创建侧目前无检查**。
7. **worker/群聊隔离机制现成**:worker 侧 `STRUCTURALLY_DISABLED`
   (`agent/subagent/tools_filter.rs:24`,经 `filter_tools_for_subagent` :80 生效);
   群聊侧 `group_chat_tool_defs`(`agent/group_chat_prompts.rs:216`)是**穷举白名单**
   —— 新注册的 builtin 工具不会自动进入群聊(08-07 黑名单改白名单的教训),
   即群聊隔离零改动天然成立。
8. **C7D 机制现成**:`STUB_CANDIDATES`(`tools/stub.rs:34`,11 候选)原地 stub +
   `load_tool_schemas` 直呼自愈;不变量「候选 ∩ L2 并行白名单 = ∅」。

## Requirements

- **R1 工具面(三件,plain dispatch)**:创建 `schedule_task`、列表 `schedule_status`
  (列本项目 `created_by='agent'` 的任务)、取消 `schedule_cancel`(按 id 硬删,仅限
  `created_by='agent'` 行;命名家族镜像 L1a 三件套)。注册追加在 `builtin_tools()` 尾部
  (provider prefix cache 契约)。
- **R2 身份落库**:agent 创建路径落 `created_by='agent'`(db 参数化);用户 UI/IPC 路径
  恒 `'user'`,行为零变化。`ScheduledTaskPayload` 增暴露 `created_by`,前端 Settings
  任务列表加来源徽标(silent Allow 的可见性补偿)。
- **R3 worker/群聊隔离**:worker 侧三工具进 `STRUCTURALLY_DISABLED` 名单;群聊侧
  **零改动** —— `group_chat_tool_defs` 穷举白名单天然不含新工具,AC4 补断言测试
  锁定(moderator/participant 两态)。**不得**把它们加进 `filter_tools_for_session_type`
  剥除名单(那是从普通 chat 剥群聊专属工具的名单,方向相反;评审 P1 修正)。
- **R4 反滥用上限(用户定案 Q3)**:agent 创建时,同 project 下 `enabled=1` 且
  `created_by='agent'` 的任务数 ≥ 20 → 拒绝,错误信息指引先 cancel 再建。仅约束 agent
  路径,用户 UI 不限;不限制调度频率(与 UI 对齐,F3 资源治理未来统一收)。
- **R5 kill switch(创建侧)**:`scheduled_tasks_enabled=false` 时 `schedule_task` 拒绝
  创建(中文错误明示原因,防僵尸任务);`schedule_status`/`schedule_cancel` 不受限
  (管理动作恒可用)。检查加在 tool 侧,不进 `create_scheduled_task_inner`(用户 UI
  路径行为零变化)。
- **R6 权限**:三工具均 Tier 5 silent Allow(`ToolKind::Other` fallthrough,不加
  classify_tool arm),静默审计沿用既有 `ToolAllowed` 链。
- **R7 target session 语义**:缺省 = 新建专用 session(镜像 UI 缺省,标题 = 任务名);
  schema 提供 `in_current_session: bool`(缺省 false)→ 由 tool 侧取当前 session id
  作 target(提醒落在当前对话时间线,带定时徽标)。不暴露裸 `target_session_id` ——
  理由改述(评审 P3-4):主场景 `in_current_session` 已覆盖,省一个 LLM 填不对的
  参数;并非因为 id 保密(search_history 输出行不含 session id,但保密性也不是本
  定案的依据)。
- **R8 C7D**:三工具加入 `STUB_CANDIDATES`(∩ 并行白名单 = ∅ 不变量天然满足;
  worker/群聊本就被 stub 适用 gate 排除)。

## Acceptance Criteria

- [ ] **AC1** agent 对话中创建 daily 任务成功 → `scheduled_tasks` 新行
      `created_by='agent'`;Settings 列表可见 + 来源徽标;到期 fire 落款注脚/定时徽标
      与用户创建任务不可区分(fire 链回归零 diff)。
- [ ] **AC2** schedule JSON 非法 / prompt 空 / `max_runs=0` / `ends_at` 过去 → 结构化错误
      tool_result(`is_error: true`),零库写入。(群聊 target 的拒绝属 `_inner` 既有
      校验,agent 面无 `target_session_id` 参不可触发 —— 归既有 F2 覆盖,AC 不单列;
      评审 P3-3 修正。)
- [ ] **AC3** `in_current_session=true` → 任务 target = 当前 session;fire 后消息落在
      当前 session 时间线且带 `metadata.scheduled` 信封。
- [ ] **AC4** worker toolset 与群聊 toolset 均无三工具;非 worker 普通 chat 可见。
- [ ] **AC5** kill switch 关闭:create 拒绝(错误信息含开关语义);list/cancel 照常。
- [ ] **AC6** 同 project agent 活跃任务达 20 后 create 拒绝(错误可自愈指引);第 21 个
      被拒后 cancel 一个 → create 恢复成功;用户路径同 project 创建不受影响。
- [ ] **AC7** `schedule_status` 只返回 `created_by='agent'` 的行(用户建的不可见);
      `schedule_cancel` 对用户创建的 id 拒绝(权限越界防御),对 agent id 硬删成功,
      删不存在的 id 幂等成功。
- [ ] **AC8** 三工具 silent Allow:权限层零新增询问卡;审计出现 `ToolAllowed` 行。
- [ ] **AC9** 全量回归:`cargo test -p everlasting --lib` + `pnpm test` + clippy(lib)
      + fmt 全绿;既有 F2/F2b 测试零改动通过。

## Out of Scope

- `update` 面(UI 已覆盖;双层 Option 语义对 LLM 易错)—— 需要时 follow-up。
- list/cancel 用户创建的任务(作者面分离是有意定案)。
- fs 事件 / 本地 webhook 触发源(ROADMAP F2 余项,独立任务)。
- 调度判定/队列/fire 语义的任何改动(F2/F2b/RULE-QUEUE-001 已收口,不回退)。
- 调度频率下限(与 UI 对齐不设;F3 资源治理统一评估)。

## References

- spec:`.trellis/spec/backend/scheduled-tasks.md`(F2 全契约 + origin 载体链)、
  `tool-contract.md`(工具注册/Tier 惯例)、`permission-layer.md`(Tier 5)。
- 先例工具:`tools/request_mode_change.rs`(schema 严格校验 + 审计形状)、
  `tools/search_history.rs`(plain dispatch + Tier 5)、L1a 三件套(命名家族)。
- 技术设计:`design.md`;实施序:`implement.md`。
