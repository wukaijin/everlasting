# F2 定时任务 — 技术设计

> 前置阅读:prd.md(决策 D1–D6 + 需求 R1–R6 + 代码勘察锚点)。本文只讲实现设计,不复述需求。
> 2026-08-28 按外部评审修订:来源标记载体(P0)、触发判定单一算法(P1)、落账丢失窗口(P1)、多 drain 缺口(P1,登记 DEBT §RULE-QUEUE-001)、token 挂载点等 P2 项。

## 1. 架构与边界

```
┌─ everlasting-daemon(唯一调度主体;GUI 零 timer 硬约束不变)──────────┐
│                                                                      │
│  bin/everlasting-daemon.rs                                           │
│    └─ server::spawn_task_scheduler(&state)   ← 新增 wrapper(detached)│
│         └─ scheduler 循环:30s tick(CancellationToken 可停)           │
│              └─ 单一扫描算法(§4):重算最近到期点 → fire → 落账        │
│                     └─ fire: 构造 ChatEntry(+origin) → chat_inner    │
│                          └─ F1 队列 / F3 信号量 / F6 busy 自动生效     │
│                                                                      │
│  db/scheduled_tasks.rs(新)  CRUD + 级联(FK)                        │
│  scheduler/compute.rs(新)   纯函数:most_recent_due(§4.1)           │
│  agent/message_queue.rs      QueuedMessage 加 origin 字段(additive)  │
│  agent/chat_loop/init.rs     persist 门控放宽 + metadata 信封加键     │
└──────────────────────────────────────────────────────────────────────┘
```

- fire 是「往 chat_inner 塞一条带 origin 的 user message」,与用户发送同一条代码路径;忙时自动入队、闸满自动排队,零新增并发控制。
- **agent loop 侧改动收敛为两处小面**(评审 P0 定案):① `QueuedMessage` 加 `origin: Option<TaskOrigin>`;② init.rs persist 门控放宽 + metadata 信封加 `scheduled` 键。原因:忙时 fire 的条目由*另一个*请求的驱动器在 round>0 消费,请求级上下文(resend_seq/forced_dispatch 同款)在 round>0 一律丢弃(chat.rs:1076-1080),所以载体必须在 `QueuedMessage` 上,不能只在 `ChatEntry`。
- **调度循环放 daemon/server.rs wrapper**(沿 `spawn_backup_task`/`spawn_shell_sweeper` 先例 + daemon-server.md:255-275「AppState::load 绝不 spawn」约束);停机采用 tunnel 心跳的 `CancellationToken + select!` 样板(tunnel/client.rs:220-240)。

## 2. 数据模型

```sql
CREATE TABLE IF NOT EXISTS scheduled_tasks (
  id TEXT PRIMARY KEY,                      -- uuid
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  target_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  prompt TEXT NOT NULL,                     -- fire 时注入的 user message 正文
  schedule TEXT NOT NULL,                   -- JSON,见 §3
  enabled INTEGER NOT NULL DEFAULT 1,
  created_by TEXT NOT NULL DEFAULT 'user',  -- MVP 恒 'user';F2+ agent 复用
  created_at INTEGER NOT NULL,              -- epoch ms
  last_fired_at INTEGER,                    -- NULL = 从未触发;触发判定基准
  next_fire_at INTEGER NOT NULL             -- 纯 UI 展示;不参与触发判定
);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due
  ON scheduled_tasks(enabled, next_fire_at);
```

- 直接追加进 schema.rs(幂等重放模式,无版本机制);FK 级联删依赖 init_pool 已开的 FK pragma(有测试锁定,migrations_tests.rs:60-67)——**删 session 级联删任务**(AC6)。
- **触发判定不信任存库 `next_fire_at`**(评审 P1 定案,消除「信存列 vs 信重算」矛盾):调度器每 tick 在 Rust 侧按 schedule 纯函数重算;存库值只喂 UI。启用/编辑/停开都无需正确维护它,杜绝「存量值过期 → 永不 fire 或连环 fire」。`NOT NULL` 约束下的停用任务存「按 schedule 的下一个到期点」展示值即可,UI 停用行灰显(评审 P3-5)。

## 3. 调度计算(scheduler/compute.rs,纯函数)

schedule JSON(D2 preset 档位,**internally tagged** `#[serde(tag = "kind")]`):

```json
{ "kind": "daily",    "at": "09:00" }
{ "kind": "interval", "every_min": 30 }
{ "kind": "weekly",   "weekday": "mon", "at": "09:00" }
```

核心纯函数,全部本地时区(chrono `Local`):

- `most_recent_due(schedule, now, not_before) -> Option<i64>`:从 now 向后步进找**最近**到期点 d(daily 最多回看 1 天、weekly 7 天、interval 按 k 逆推),要求 `not_before < d <= now`;无则 None。`not_before = max(created_at, last_fired_at.unwrap(0))`。
- `next_fire_display(schedule, from) -> i64`:严格 `> from` 的下一个到期点,仅供 UI 列展示与存库。

**catch-up 与常规触发是同一算法**(评审 P1 定案):调度器每 tick 对每个 enabled 任务调 `most_recent_due`——命中即说明「存在未消费的到期点」(无论正常到点还是停机错过),fire 一次并把 `last_fired_at` 前移;未命中即空转。D4「补一次、不追多次」由「最近一个到期点」语义天然保证;同一停机窗口多次评估幂等(窗口左端随 last_fired_at 前移)。无独立 catch-up pass → 不存在同 tick 双 fire。

**interval 网格锚点 + 落账记 due(二道评审 P1 定案)**:interval 网格锚定 `not_before`(= `max(created_at, last_fired_at)`),而落账**恒记理论到期点 `last_fired_at = due`**(语义升格为「最近一个已消费的到期点」)——保证 `not_before` 恒落在网格上,实际触发间隔恒等于 `every_min`(tick 量化误差不进入下一周期)。若落账记实际触发时刻 `now`,period = N + tick 量化误差(有界但持续吃掉半拍,均值 +tick/2);dedup 跳过分支同样记 due(该 due 点标记为已消费,prompt 该轮放弃,防每 tick 重判)。daily/weekly 网格是自然时间,无锚点问题,同样记 due。

**disable→enable 不补跑存量(二道评审 P2-3,裁定取 (b))**:`update_scheduled_task` 在 enabled false→true 时把 `last_fired_at = now`——重新启用从下一个到期点开始,不补停用期。语义分界:daemon 停机是**非自愿**的(补跑,D4);显式 disable 是**用户主动**的(不补)。与评审倾向的 (a)「enable 即补跑最近一次」不同:用户禁用一周后重新打开,预期是「从下次开始」而非「立刻补一份过期报告」,一行 update 分支换语义直觉,值得。

不变量(单测锁):`most_recent_due` 结果恒 `> not_before` 且 `<= now`;`next_fire_display > from`;两函数对同一 schedule/given now 互相一致;**interval 无累积漂移——1min interval 连续模拟 fire(含 tick 量化抖动),断言相邻 due 间隔恒等于网格步长**(二道评审 P1 要求的回归锁)。

## 4. fire 路径

调度循环伪代码(每 30s tick,CancellationToken 可停):

```text
if !scheduled_tasks_enabled: continue            # kill switch,fail-open
fired_sessions = ∅                               # 本 tick 已 fire 的 target_session(见下)
for task in load_enabled_tasks():                # 按 last_fired_at NULLS FIRST 排序
    not_before = max(task.created_at, task.last_fired_at ?? 0)
    match most_recent_due(task.schedule, now, not_before):
        None => continue
        Some(due) =>
            if task.target_session_id ∈ fired_sessions:
                continue                         # 同 session 同 tick 只 fire 一个,余者下 tick
                                                 # (消掉「多任务同 tick 同 session」对
                                                 #  RULE-QUEUE-001 的确定性触发,评审 P2-2;
                                                 #  不前移 last_fired_at,下 tick 自然重判)
            if !message_queue_enabled:            # 评审 P2:legacy 分支会砍在跑轮
                audit(task, action=skipped_queue_disabled); continue
            if queue_holds_previous_entry(task):  # 去重,见 §4.2
                audit(task, action=skipped_dedup)
                last_fired_at = due               # 记 due(网格点),prompt 该轮放弃
                continue
            action = if now - due > 60s { catchup } else { fired }   # 宽限 = 2×tick
            fire(task) → 见 §4.3;Err ⇒ audit(action=error, reason=错误类别)
            last_fired_at = due                   # 记理论到期点(评审 P1 定案,防相位漂移)
            next_fire_at(展示)重算写回;fired_sessions += target_session_id
```

- `turn_seq` 传值:`record_audit_event` 签名要求的零值(实施时按签名定 0 或 None,不留 TODO,评审 P3-4)。

### 4.1 来源标记载体(评审 P0 定案)

```rust
// agent/chat.rs — ChatEntry 加一字段(仅 scheduler 传,其余调用点 None;
// 全库构造点仅 2 处:routes/agent.rs:61、chat.rs:107):
pub origin: Option<TaskOrigin>,
// scheduler/mod.rs:
#[serde(tag = "kind")]
pub enum TaskOrigin { Scheduled { task_id: String, task_name: String, fired_at: i64 } }
// agent/message_queue.rs — QueuedMessage 加字段(additive,现有 4 字段不动):
pub origin: Option<TaskOrigin>,
```

**TaskOrigin 上 wire(二道评审 P2-1 定案)**:`QueuedMessage` derive 了 `Serialize`(服务 R8 `list_queued_messages` 排队占位 hydration),origin 字段必然随之序列化——顺势采纳而非 `#[serde(skip)]`:前端排队占位可直接显示「定时」徽标,补齐「排队中 → 注入 → done 后权威行」全程可见性,成本为零。故 §6「不上 wire」限定为**不进 chat 事件主链**,排队占位 IPC 携带。

传递链:`chat_inner` 路由临界区 push 时把 `entry.origin` 拷入 `QueuedMessage.origin` → 驱动器每轮取 `drained.last()`(= 本轮被持久化的尾条)的 origin,经 `ChatLoopRequest` 新字段传入 → init.rs persist 门控从 `(!injections.is_empty() || has_attachments)` 放宽为 `… || origin.is_some()`,metadata 信封加第四键 `scheduled`(additive,不动 injections/attachments;update_message_metadata 整体覆盖写语义不变)。群聊/legacy 路径不经过此链(origin 恒 None),零影响。

### 4.2 去重与堆积

- 判定:锁 `message_queues`(只读 peek,遵守「先 queues 后 active_request、临界区内无 await」纪律),查目标 session 队列是否仍含本任务上次 fire 的条目 uuid。
- **uuid 仅 `Queued{id}` 返回路径可得**(`ChatAcceptance::Started` 不带 id——闲时条目即时被驱动器消费,本就无需去重;评审 P2 定案,实现按字面只记 Queued 分支)。
- 语义:上次注入还在队(未被消费)→ 本轮跳过。这是对「interval 任务遇慢轮」的堆积闸;对「daily 任务恰逢忙」场景,注入会正常排队等消费(见 §9 权衡)。

### 4.3 fire 动作与落账

1. **构造注入消息**:content = `prompt + "\n\n（本条由定时任务「{name}」于 {YYYY-MM-DD HH:MM} 自动触发）"`(注脚给模型日期上下文);origin = `TaskOrigin::Scheduled{...}`。
2. **调 chat_inner**:新 uuid request_id + `HttpSseSink{registry: state.sse.clone()}`(与 routes/agent.rs:47 同构造,事件进 SseRegistry → GUI/PWA 订阅者);返回值:`Queued` 记 uuid 进去重表;`Started` 不记;`Err` 落 `audit(action=error, reason=错误类别,如 queue_full)`(评审 P3-2)。F3 闸满由闸头排队语义兜底。
3. **落账**:`last_fired_at = due`(理论到期点,§3 定案)+ 展示用 `next_fire_at` 重算写回 + `record_audit_event(AuditKind::ScheduledTaskFired, payload={task_id, task_name, action[, reason]})`(挂目标 session,best-effort 沿 resend 审计惯例,不进事务)。
4. **已知丢失窗口(评审 P1,裁定接受)**:`last_fired_at` 前移后,若该轮最终没跑成——① 用户 Stop 清队(chat.rs:1124-1127);② daemon 重启内存队列蒸发(message_queue.rs:16-20);③ preflight 失败仍返回 `Ok(Started)`、错误只走 SSE(chat.rs:403-445)——该次 fire 不补偿(接受,prd Out-of-Scope 已列)。可观测兜底:Stop 清队路径对带 origin 条目补 `audit(action=lost)`(best-effort,小面改动);③ 接受不补偿。根治方案(落账后移到消费点)需队列回执机制,MVP 不做。
5. **lost 审计覆盖边界(二道评审 P3-1 定案)**:仅覆盖 Stop 语义的两处清队点——chat.rs:1126(驱动器 cancel break)与 commands/cancel.rs:69(Stop 命令);sessions.rs 三处破坏性命令(delete_session / clear_session_messages 等)属会话/消息本身销毁,任务已随 FK 级联或内容有意清空,不审计(delete 场景连挂靠 session 都不存在)。实现提示:`clear_session` 只返回 count 不返回条目,识别带 origin 条目需清队前 `list_session` 快照(两次 await 间驱动器可能并发 drain,best-effort 可接受)。

## 5. 调度循环与装配

```rust
// daemon/server.rs(新 wrapper,仿 spawn_backup_task 形)
pub fn spawn_task_scheduler(state: &Arc<AppState>) { /* tokio::spawn(run_loop) */ }
```

- tick 30s(`tokio::time::interval`,首 tick 立即 = 启动补偿评估);循环体每 tick 重复 §4 算法。
- **CancellationToken 挂载点(评审 P2 定案)**:`AppState` 加 `scheduler_cancel: CancellationToken` 字段——`CancellationToken::new()` 纯分配无 spawn,`load_inner` 直接构造(比 OnceLock 更简;二道评审核实 tunnel_manager 是 `Arc<TunnelManager>` 空壳先例,load 只建不 spawn 的约束不受影响);`spawn_task_scheduler` 从 state 读 token;`shutdown_signal` 在 tunnel stop(步骤 ①.5)之后插一步 `scheduler_cancel.cancel()`(步骤 ①.6),cancel 后循环当 tick 退出,正在 fire 的单个注入由既有 `cancel_and_drain_all_agent_loops` 兜底。
- kill switch:`scheduled_tasks_enabled`(app_config,fail-open,每 tick 读一次);`AppConfigPayload` 加字段供前端展示(additive)。
- 装配点:bin/everlasting-daemon.rs 的 spawn 插槽段(backup/sweeper 旁);GUI Full 模式零改动(AC7;HACKING/ROADMAP 注明)。

## 6. IPC / wire(双 transport Q0 单源)

commands 模块 `commands/scheduled_tasks.rs`:

| command | 入参(camelCase invoke) | 语义 |
|---|---|---|
| `list_scheduled_tasks` | `projectId?` | 全量/按 project;附 next_fire_at 展示值 |
| `create_scheduled_task` | `projectId, targetSessionId?, name, prompt, schedule, enabled?` | targetSessionId 缺省 = 新建专用 session(name 同任务名,cwd 取 project 根) |
| `update_scheduled_task` | `id, {name?, prompt?, schedule?, enabled?, targetSessionId?}` | enabled false→true 时 `last_fired_at = now`(存量不补跑,§3 定案);不维护 next_fire_at(每 tick 重算)。**WP2 实施加参 `targetSessionId`**(additive 超集,支撑编辑态换目标,校验同 create) |
| `delete_scheduled_task` | `id` | 硬删 |

- 校验(create/update):目标 session 必须存在且 `session_type='chat'`(群聊拒绝,AC7);project 归属一致;schedule 反序列化合法(internally tagged enum)。
- daemon routes:`POST /api/v1/scheduled_tasks/{command}` 四条,转发同一 `_inner`(Q0);请求体 snake_case(沿 ChatRequest 惯例)。
- wire DTO additive,无破坏性字段变更;`TaskOrigin` 不进 chat 事件主链,但随 `QueuedMessage` 序列化进 `list_queued_messages` 排队占位 IPC(§4.1 定案)。

## 7. 前端

- `stores/scheduledTasks.ts`:list/create/update/delete actions + reactive 列表缓存(沿 audit/memory store 模式);`transport/http.ts` 加 4 条命令→域映射。
- Settings 新增第 8 个 tab「定时任务」:任务卡片列表(名称/目标 session·project/schedule 人话/prompt 摘要/上次·下次触发/启停 Switch/删除;停用行灰显,展示值为 schedule 下一到期点)+ 新建表单(project 下拉 → session 下拉(按 project 过滤,仅 classic)或勾选「新建专用 session」→ 档位选择(daily 时间选择器 / interval 数字输入 / weekly 周几+时间)→ prompt textarea)。**软警示**:所选 session 已有 enabled 任务时表单内联提示「该 session 已有定时任务,同时触发将合并为一轮处理」(不硬拒;配合调度器同 tick 串行化,评审 P2-2)。面板顶部注明「调度仅在 daemon 进程运行时生效」(Full/tauri 逃生模式可建任务但不会触发)。移动端沿 S6b 适配约定。
- 消息流:`MessageItem` 对 `metadata.scheduled` 气泡加「定时」小标识(task_name 作 title)——**零 rehydrate 改动**(二道评审核实:`MessageRow.metadata` wire 整体透传 streamRehydrate.ts:301,前端 `ChatMessage.metadata?` 字段已存在 chat.types.ts:437,MessageItem 泛型读 metadata 有 edited_at/compaction_summary 先例);排队占位(输入区上方 F1 队列视图)经 `list_queued_messages` 的 origin 显示「定时」徽标(P2-1);session header 徽章:AppShell 启动拉一次 list,session 有 enabled 任务 → header 时钟小图标(title 显示任务名)。
- **live 可见性预期(评审 P2 注记,验收按此不算缺陷)**:跨端实时认领(`adoptForeignRequest`)只建 assistant 占位,带「定时」标识的 user 气泡在该轮 done 后权威重拉才出现;busy 红点/完成 toast 实时生效(F6 既有面)。

## 8. 测试策略

- **纯函数**(compute):daily/weekly 跨日/跨周边界、interval 锚点与逆推、`most_recent_due` 的 `not_before` 约束与三态(从未触发/窗口内错过/无错过)、catch-up 幂等(同窗口重复评估不双 fire)、`most_recent_due` 与 `next_fire_display` 一致性、**interval 无累积漂移不变量(1min 连续模拟 + tick 抖动,相邻 due 间隔恒等步长)**。
- **DB**:CRUD、FK 级联(删 session 删任务)、schedule JSON 非法拒绝、**update false→true 置 `last_fired_at=now`(重启用不补跑)**。
- **集成**(mock LLM,harness 先例 `MockProvider` llm/provider/mock.rs:238 + tests_common):spawn scheduler + 预置 last_fired_at 在过去的任务 → 单 tick 后 messages 落库且 `metadata.scheduled` 三键齐全(origin 全链断言)、audit 落 `ScheduledTaskFired`;去重(队列滞留时第二 tick `skipped_dedup`)、kill switch 空转、queue-disabled 跳过、**同 session 双任务同 tick → 第二个延到下 tick(deferral 断言)**、lost 审计(Stop 清队带 origin 条目);**多 drain 钉现状**(scheduled 与手动同轮 drain,断言当前落库结果——RULE-QUEUE-001 根治前防漂移)。
- **parity**:4 条 daemon route 与 Tauri command 同 _inner(Router oneshot 模板 routes/sessions.rs:411,spec §6 明文要求)。
- **前端**:store actions + Settings tab 组件测试(建表单档位切换/校验/同 session 软警示/列表启停)。
- 常规门:cargo test --lib 全量 + clippy -D warnings + fmt;pnpm test + vue-tsc + build。

## 9. 权衡与已接受损耗(评审定案)

- **调度器自研 vs cron crate**:D2 裁定 preset 档位,自研纯函数(~100 行)零依赖,匹配项目惯例(serde_yml 废弃、frontmatter 手写 parser)。
- **HTTP 自呼 vs 进程内调 chat_inner**:选进程内(省一次回环 + 无端口依赖);与路由共享 `_inner` 语义不漂移。
- **忙时注入 vs 忙时跳过**:选「照常入队 + 本任务滞留去重」——daily 任务恰逢对话中,报告仍会送达;interval 任务的堆积被去重闸住。去重只对本任务上次条目生效,不拦用户手动消息(用户排队语义不受影响)。
- **同 session 多任务(二道评审 P2-2)**:调度器 per-session 每 tick 至多一次 fire(§4 伪代码 `fired_sessions`),把「两个 daily@09:00 绑同一 session」这类确定性 RULE-QUEUE-001 触发消解为「第二个任务延 30s 独立一轮」(两条 prompt 各自落库);创建表单软警示兜底;与手动消息同轮的残余窗口仍属 RULE-QUEUE-001 既有缺陷面,钉行为测试跟踪。
- **落账时机 = 记理论到期点 due**(而非消费确认或触发时刻):消费回执机制改动大;记 due 消灭 interval 相位漂移(§3);丢失窗口三类(§4.4)接受,`lost`/`error` 审计兜底可观测。
- **多 drain 非尾条不落库(F1-A 既有缺陷 → DEBT §RULE-QUEUE-001)**:驱动器 drain_all 全队进 turn 输入(chat.rs:1069-1072)但 persist 只写尾条(init.rs:783-784)——scheduled 条目被顶掉尾位时,其 prompt 无 DB 行、「定时」标识随失(LLM 仍看到并执行了该轮;catch-up 已落账不会重复跑)。F2 侧缓解 = 去重 + 同 tick 串行化 + `lost` 语义 + 钉行为测试;根治(持久化全部非尾 drained 条目)归 DEBT,不阻塞 F2。

## 10. 运维与回滚

- 零 migration 风险(新表 IF NOT EXISTS + 列 helper);回滚 = revert 后表残留无副作用(无人读)。
- 新增依赖:无。
- 观测:审计五动作 + tracing(tick 有命中任务时一行);turn-smoke 不受影响(不建任务即零行为差异——空转 tick 仅 2–3 条轻查询:kill-switch + queue-enabled + 任务表)。
