# PRD: 定时任务目标 session 三档(per_run)+ 目标 session UI 重设计

日期:2026-08-31 · 来源:用户直接请求(「改下定时任务,目标 session 多一档,每次执行都是新的一个 session;另外目标 session 这部分前端 UI 重新设计下」)

## 需求

### R1 目标 session 新增第三档:每次执行新建 session(per_run)

现状两档:① 指定既有 chat session;② 创建时新建专用 session(绑定一个固定 session)。
新增:**每次触发都自动创建一个全新 session**,prompt 注入该新 session,各次执行互不干扰。

### R2 目标 session 表单区 UI 重设计

现状:checkbox「新建专用 session」+ session 下拉,概念并列不清、专用档的模型选择悬在远处。
重设计为结构化的三档选择 + 就近的上下文面板。

## Acceptance Criteria

- [ ] AC1 创建任务时可选「每次新建 session」档;创建成功后任务不绑定任何固定 session。
- [ ] AC2 每次到点触发时,后端自动在同 project 下新建一个 chat session,标题 = `{任务名} {YYYY-MM-DD HH:MM}`(触发时刻,本地时区)。
- [ ] AC3 per_run 任务的每轮注入走与既有完全相同的 fire 链(origin 载体链 / 审计 / 落账);`fired`/`catchup`/`error` 审计挂在新创建的 session 上。
- [ ] AC4 per_run 任务可选绑定模型(存任务行;每次新建 session 时写入该 session 的 per-session 覆盖列);不选 = 全局默认。
- [ ] AC5 per_run 任务不受「同 session 每 tick 一 fire」与「队列去重」约束(每次目标都是全新空闲 session);`message_queue_enabled=false` 时照常 fire(legacy 分支的 cancel+replace 危害对全新 session 不存在)。
- [ ] AC6 会话创建失败 → 该 due 点消费(error 审计 reason=session_create_failed + 落账计数),不重试风暴。
- [ ] AC7 删除任意旧 run session **不得**级联删除任务(区分于 fixed 档的既有级联语义)。
- [ ] AC8 编辑任务可在「指定 session」与「每次新建」两档间切换;切到 per_run 清空固定绑定,切回 fixed 必须选择 session。
- [ ] AC9 任务列表卡对 per_run 任务显示「每次新建 session」标识;能看出最近一次运行的 session。
- [ ] AC10 LLM `schedule_task` tool 路径行为不变(恒 fixed/专用语义,不暴露 per_run)。
- [ ] AC11 表单中目标 session 区改为三选一的 radio 卡片组(创建态):「指定 session」/「新建专用 session」/「每次新建 session」,每卡带一句话说明;选中态高亮。
- [ ] AC12 编辑态提供两档(「指定 session」/「每次新建 session」),打开时按任务存量回填。
- [ ] AC13 选「新建专用 session」或「每次新建 session」时,模型选择就近出现在卡片下方(沿用既有 modelId 语义)。
- [ ] AC14 沿用项目设计 token / reka-ui 惯例(RadioGroupRoot 先例见 DefaultTab),移动端 320-430px 无溢出,触控目标合规。
- [ ] AC15 校验:选「指定 session」未选具体 session → 内联错误不提交(既有行为保持)。

## 非目标

- 不给 LLM `schedule_task` tool 暴露 per_run。
- 不做 run session 的自动清理/归档。
- 调度判定算法(due / catch-up / 结束条件)零改动。

## 关键定案(D 系)

- D1 存储:`scheduled_tasks` 加 `target_mode`('fixed'|'per_run',默认 'fixed')+ `model_id`(per_run 每次建 session 的模型)+ `last_run_session_id`(无 FK,最近一次 run session,审计锚点 + 列表展示)。`target_session_id` 改为可空(per_run 恒 NULL,不指向 run session,避免删除旧 session 级联删任务)。不变式:`target_mode='fixed'` ⇔ `target_session_id` 非空(CHECK 约束)。
- D2 既有行迁移 = table rebuild(target_session_id 去 NOT NULL 无法 ALTER;沿 `rebuild_turn_trace_with_run_id` 先例,事务内 rename→create→copy(target_mode='fixed')→drop→reindex)。
- D3 session 创建发生在调度 tick 内(fire seam 之前),`FireContext.target_session_id` 保持 String——seam 类型与全部既有测试替身零改动。
- D4 审计锚点:fixed 挂 `target_session_id`;per_run 挂 `last_run_session_id`,为 NULL(从未成功建过 session)时跳过审计写 + tracing warn。
