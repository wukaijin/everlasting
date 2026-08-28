# Scheduled Tasks(F2 定时任务)— 执行契约

> 任务源:`.trellis/tasks/08-28-f2-scheduled-tasks/`(2026-08-28,经两道外部评审);
> F2b 调度模型扩展(三新档位 + 结束条件)任务源 `08-28-f2b-schedule-extension/`。
> 本文件是 F2 的 code-spec:调度判定语义、origin 载体链、wire 契约、错误矩阵。
> 实现在 `app/src-tauri/src/scheduler/` + `db/scheduled_tasks.rs` + `commands/scheduled_tasks.rs`。

---

## Scenario: F2 调度判定与 fire

### 1. Scope / Trigger

- 触发:新增/修改任何「定时触发 agent 轮次」的逻辑;新增周期性 spawn 任务;触碰 `QueuedMessage`/`ChatEntry`/`ChatLoopRequest` 字段。
- 为什么需要 code-spec 深度:跨层契约(wire + DB + metadata)+ 调度语义有两处反直觉定案(落账记 due、catch-up 与常规触发同一算法),凭直觉写必错(两道评审实证)。

### 2. Signatures

```rust
// scheduler/compute.rs — 纯函数,全 Local 时区
pub fn most_recent_due(schedule: &ScheduleSpec, now_ms: i64, not_before: i64) -> Option<i64>;
pub fn next_fire_display(schedule: &ScheduleSpec, from_ms: i64) -> i64;

// schedule JSON — internally tagged(不是 untagged!)
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ScheduleSpec {
    Daily { at: String },          // "HH:MM"
    Interval { every_min: u32 },
    Weekly { weekday: Weekday, at: String },
    // F2b 三新档(08-28-f2b-schedule-extension prd D7):
    Hourly { minute: u32 },        // 每小时第 minute 分钟(0-59)
    Weekdays { at: String },       // 每工作日(周一至五,无节假日日历)
    Monthly { day: u32, at: String }, // 每月 day 号;短月无该日 → 跳过该月
}

// origin 载体链(四点一线,全部 Option/additive)
ChatEntry.origin: Option<TaskOrigin>              // 仅 scheduler 传 Some(全库 2 处构造点)
QueuedMessage.origin: Option<TaskOrigin>          // 路由临界区 push_with_origin 拷入
ChatLoopRequest.origin: Option<TaskOrigin>        // 驱动器取 drained.last() 传入
// init.rs persist 门控:(!injections.is_empty() || has_attachments || origin.is_some())

// DB
CREATE TABLE scheduled_tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  target_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  name TEXT NOT NULL, prompt TEXT NOT NULL,
  schedule TEXT NOT NULL,                          -- ScheduleSpec JSON
  enabled INTEGER NOT NULL DEFAULT 1,
  created_by TEXT NOT NULL DEFAULT 'user',
  created_at INTEGER NOT NULL,
  last_fired_at INTEGER,                           -- NULL = 从未触发
  next_fire_at INTEGER NOT NULL,                   -- 纯 UI 展示,不参与触发判定
  run_count INTEGER NOT NULL DEFAULT 0,            -- F2b 已 fire 次数(dedup 跳过不计)
  max_runs INTEGER,                                -- F2b 次数上限;NULL = 不限
  ends_at INTEGER                                  -- F2b 结束日期 epoch ms;NULL = 不限(含当日)
);

// wire:POST /api/v1/scheduled_tasks/{list|create|update|delete}_scheduled_tasks
//   与 Tauri command 同一 *_inner(Q0 单源)
```

### 3. Contracts

- **触发判定(单一算法)**:每 tick 对每个 enabled 任务算 `most_recent_due(schedule, now, not_before)`,`not_before = max(created_at, last_fired_at ?? 0)`;命中即 fire 一次。**catch-up 与常规触发是同一判定**——无独立启动补偿 pass(独立 pass 会同 tick 双 fire)。
- **落账记理论到期点**:`last_fired_at = due`(非 now)。语义 =「最近一个已消费的到期点」。效果:not_before 恒在 interval 网格上,实际间隔恒等于 `every_min`(落账记 now 会让 period = N + tick 量化误差,见 §7)。
- **同 session 每 tick 至多一 fire**(`fired_sessions` 集合,顺延者**不前移** last_fired_at):消解「多任务同 tick 同 session」对 RULE-QUEUE-001(多 drain 非尾条不落库)的确定性触发。
- **去重 uuid 只在 `Queued{id}` 返回路径可得**(`ChatAcceptance::Started` 不带 id——闲时条目即时消费,无需去重)。
- **disable→enable 不补跑**:`update_scheduled_task` false→true 时 `last_fired_at = now`。语义分界:daemon 停机是**非自愿**(补跑一次);显式禁用是**用户主动**(不补)。
- **metadata.scheduled 信封**(前端「定时」标识的依据):persist 时并入 messages.metadata,三键 `{task_id, task_name, fired_at}`;与既有 `injections`/`attachments` 并列,**不放宽 FileInjections 事件门控**(纯定时轮不发空载荷事件)。
- **fire 正文**:prompt + 注脚 `（本条由定时任务「{name}」于 {YYYY-MM-DD HH:MM} 自动触发）`(全角,模型日期上下文)。
- **审计**:`AuditKind::ScheduledTaskFired`(`"scheduled_task_fired"`),payload `{task_id, task_name, action[, reason]}`,action ∈ `fired | catchup | skipped_dedup | skipped_queue_disabled | lost | error | completed`(F2b);turn_seq 传 None;`lost` 仅覆盖两处 Stop 语义清队点(驱动器 cancel break + Stop 命令),破坏性清理(delete_session 等)不审计。
- **queue-disabled gate**:`message_queue_enabled=false` 时 fire 跳过并审计(legacy 分支对忙 session 是 cancel+replace 语义,会砍在跑轮)。
- kill switch `app_config` `scheduled_tasks_enabled`:fail-open(仅字面 `"false"` 关)。

### F2b 扩展契约(08-28-f2b-schedule-extension)

**档位回看/前看窗口**(`most_recent_due` 回看 / `next_fire_display` 前看,全部经 `local_at` 的 DST 防御 —— 春令时不存在的时刻 → `None` 跳过,秋令时重复取较早):

| 档位 | 窗口 | 说明 |
|---|---|---|
| daily | 0..=1 天 | |
| weekly | 0..=7 天 | 跳过非目标 weekday |
| hourly | 0..=2 个墙上钟点 | 第 2 个是 DST 跳过余量 |
| weekdays | 0..=3 天 | 跳过 Sat/Sun(周一最远命中周五) |
| monthly | 0..=2 个月 | `from_ymd_opt` 无效(短月无 29/30/31)→ **跳过该月**(prd D7,cron 语义;day=31 在 3 月初要回看到 1 月) |

- **interval 单位是纯 UI 换算**:「每 N 小时/天/周」在前端换算成 `every_min`(时 60 / 天 1440 / 周 10080,整除无精度损失),后端与存库只有分钟粒度,网格判定零改动。

**结束条件(`max_runs` / `ends_at` 两列对所有档位通用,UI 按类型限定展示 —— 固定时间只出次数、固定频率只出日期,prd D10)**:

- **run_count 只计真正送入 chat_inner 的 fire**(Queued/Started/Error 三种落账结局 +1);**dedup 跳过不计**(prompt 未送达,仅消费 due 点)。
- **tick 四道 gate**(顺序即代码序):① `run_count >= max_runs`(若设)→ 完成;② `ends_at` 已过且窗口内无未消费 due → 完成;③ 算出 due 后 `due > ends_at`(若设)→ 不 fire 且完成 —— 反之 `due <= ends_at` 照常 fire(**含 catchup,结束日当天仍触发**,prd D9,ends_at = 指定日 23:59:59.999 本地);④ fire 落账后 `run_count+1 >= max_runs` 或 `next_fire_display(due) > ends_at` → 即时完成。
- **完成 = `mark_task_completed`(仅置 enabled=0)+ `completed` 审计**(reason = `max_runs` / `end_date`);enabled=0 即出扫描集,completed 审计天然只发一次。任务保留在列表(UI「已完成 N/M」「已结束」),不自动删除(prd D8)。
- **重新启用(enabled false→true)重置 `run_count = 0` + `last_fired_at = now`**(与 F2「重启用不补跑」同一语义:用户主动重启 = 重新计次)。
- **wire 的 update 双层 Option**:daemon DTO 用 serde double-option 反序列化(`#[serde(default)]` + `map(Some)` helper)—— 字段缺省 = 不动,显式 `null` = 清空为不限,数值 = 写入。前端 update 恒显式带 `maxRuns`/`endsAt`(null 清空),避免切换结束方式后旧值残留。
- **ends_at 判定用 due 不是 now**:停机跨 ends_at 重启后,窗口内未消费且 `<= ends_at` 的 due 仍补跑一次(Bad 案例:「用 now > ends_at 直接判死」会让 catch-up 语义失效)。
- 校验矩阵新增:`max_runs < 1` / `ends_at <= now`(create 与 update 显式写入)→ 400 中文错误。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| 目标 session 不存在 / 是群聊 | create/update 拒绝(400 InvalidRequest,中文错误) |
| target 的 project ≠ 任务的 project | create/update 拒绝(400「不属于」) |
| schedule JSON 非法 / 未知 kind | 拒绝(`parse_schedule` 错误信息透传) |
| update 目标不存在 | InvalidRequest(防编辑竞态静默) |
| delete 不存在的 id | 幂等成功(返 false) |
| fire 时队列满(QueueError::Full) | audit `action=error, reason=queue_full` |
| `scheduled_tasks_enabled=false` | tick 空转,零审计零落账 |

### 5. Good / Base / Bad Cases

- **Good**:daily@09:00 任务,daemon 08:00–09:30 停机 → 重启首 tick `most_recent_due` 命中 09:00 → fire 一次,`action=catchup`,下一到期点明天 09:00。
- **Base**:interval 30min 正常运行 → 相邻 due 间隔恒等 30min(tick 30s 量化误差不进入下一周期);同 session 双任务同 tick → 第二个顺延下 tick 独立一轮,两条 prompt 各自落库。
- **Bad**(禁止出现):catch-up 独立 pass 与常规扫描同 tick 各 fire 一次;落账记 now;dedup 前移用 now;enable 后补跑停用期;`QueuedMessage` 加非 Option 破坏性字段;**F2b:dedup 跳过也计 run_count(prompt 未送达);ends_at 用 `now > ends_at` 判死而非 `due > ends_at`(会让跨 ends_at 停机的 catch-up 补跑失效);完成审计走独立定时扫描而非 tick gate(会与 enabled=0 竞态重复发)**。

### 6. Tests Required

- 纯函数:`scheduler::compute` — daily/weekly 跨日跨周、interval 逆推、`most_recent_due` 的 not_before 约束三态、**漂移不变量**(`interval_has_no_cumulative_drift_with_tick_jitter`:连续模拟 + 抖动,断言相邻 due 恒等步长)。
- DB:`fk_cascade_on_session_and_project_delete`、false→true 重置基准(true→false 不动)。
- tick 集成(`scheduler/tests_tick.rs`):fired/catchup 动作 + 落账 ≈ due(容差断言,记 now 会偏差一个 tick)、deferral(顺延者 last_fired_at 不动)、去重/消费后放行、kill switch、queue-disabled、error reason、Started 不记 uuid。
- origin 全链(`tests_message_queue.rs` / `tests_lost.rs`):`scheduled_origin_persists_scheduled_metadata`(MockProvider 端到端断言三键 + 无 FileInjections + 对照组 metadata None)、lost 正负向、**多 drain 钉现状**(RULE-QUEUE-001 根治前防漂移)。
- parity:`daemon/routes/scheduled_tasks.rs` Router oneshot——CRUD roundtrip(含缺省新建专用 session 分支)+ 校验矩阵全臂。
- **F2b**:compute 三新档的窗口边界 + 两函数互相一致 + monthly 短月跳过(day=31 回看两月);db 的 mark_fired 计数语义(count_fire bool)/ mark_task_completed 只翻 enabled / update 双层 Option(缺省≠null)/ 重启用清零;tick 的四道 gate(max_runs 达限完成恰好审计一次、due>ends_at 不 fire、ends_at 含当日 fire 后完成、dedup 不计数);route 的 end-condition roundtrip(monthly 档过 wire、显式 null 清空、max_runs=0 与过去 ends_at 400);前端 format/store/tab(单位换算、结束条件 args 形状、完成态徽章)。

### 7. Wrong vs Correct

#### Wrong(直觉写法,两道评审否决)

```rust
// 落账记实际触发时刻
state.last_fired_at = now;
// catch-up 单独一个启动 pass
spawn(async { catchup_scan().await; regular_loop().await });
```

每周期相位后移 tick 量化误差(均值 +tick/2,有界但不回正);启动 pass 与
常规扫描无共享落账,同 tick 对同一停机窗口双 fire。

#### Correct

```rust
// 落账恒记理论到期点;catch-up = 同一判定在启动首 tick 的自然结果
let due = most_recent_due(&spec, now, not_before)?; // None → 空转
fire(task, due).await;
mark_task_fired(task.id, due, next_fire_display(&spec, due));
```

`not_before` 恒在网格上;「有没有未消费的到期点」一个判定同时覆盖正常到点
与停机错过,补跑次数由「最近一个」语义天然封顶为 1。

---

## Scenario: origin 载体链(跨层契约)

### 1. Scope / Trigger

- 触发:任何「给用户消息附加来源/上下文标记」的新需求(F2+ `schedule_task` tool、未来其他自动注入方)。
- 为什么:标记必须穿越「路由临界区 → 内存队列 → 另一个请求的驱动器 → persist」,载体选错(只加在 `ChatEntry`)在忙时路径**必然失效**。

### 2. Why(关键推理,勿回退)

忙时 fire 的条目由*另一个*请求的驱动器在 round>0 消费;驱动器对 round>0
一律丢弃请求级上下文(resend_seq/forced_dispatch 同款,`chat.rs` round 分支)
——所以载体必须在 `QueuedMessage`(队列条目自有字段),不能只在
`ChatEntry`。闲时路径 round 0 也从 drained 尾条取 origin,两条路统一。

### 3. Contracts

- 链路:`ChatEntry.origin` →(路由临界区内 `push_with_origin` 纯赋值拷入)→ `QueuedMessage.origin` →(驱动器每轮取 `drained.last()`)→ `ChatLoopRequest.origin` → init.rs persist 门控 + `metadata.scheduled` 信封。
- `TaskOrigin` 是 internally-tagged enum(`#[serde(tag="kind")]`),**会随 `QueuedMessage: Serialize` 进入 `list_queued_messages` wire**(前端排队占位「定时」徽标的依据)——这是有意定案,不是泄漏;但**不进 chat 事件主链**。
- `origin: Option` 恒为 None 的路径(用户发送/群聊/worker/legacy)行为逐字节不变;多 drain 时只有尾条(被持久化那条)的 origin 生效——与「persist 只写尾条」对齐。

### 4. Wrong vs Correct

#### Wrong

```rust
// 只在 ChatEntry 加字段
pub struct ChatEntry { ..., pub origin: Option<TaskOrigin> }
// 期望忙时入队后仍能取回 → round>0 驱动器重建请求,字段丢失,标记静默消失
```

#### Correct

```rust
// ChatEntry(入口)+ QueuedMessage(载体)+ ChatLoopRequest(传递)三点齐加,
// 临界区内纯赋值拷入,驱动器尾条取值
let origin = drained.last().and_then(|qm| qm.origin.clone());
```

### 5. Tests Required

见上节 origin 全链测试;新增携带来源的场景**必须**同时有对照组断言
(无 origin 路径 metadata 恒 None),防 additive 字段向既有路径漂移。
