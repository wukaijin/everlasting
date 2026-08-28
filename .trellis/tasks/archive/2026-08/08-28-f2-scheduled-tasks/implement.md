# F2 定时任务 — 实施计划

> 前置:prd.md(D1–D6 + R1–R6)、design.md(2026-08-28 评审修订版)。按 WP 顺序实施,每个 WP 独立可验证、可提交。
> 外部评审(2026-08-28)结论「有条件是」已闭环:P0-1 origin 载体、P1-1 单一算法、P1-2 落账语义、P1-3 DEBT 登记、P2×4 全部落进下面对应条目。二道评审(review.md,2026-08-28)已甄别闭环:P1 interval 锚点/落账记 due、P2-1 origin 上 wire、P2-2 同 tick 串行化、P2-3 重启用不补跑(裁定取 (b))、P3×5,见 review.md 末尾甄别记录。

## WP1 后端调度内核(origin 链 + 调度循环,无 wire/前端变更)

- [x] `db/scheduled_tasks.rs`:CRUD;schema.rs 追加 `scheduled_tasks` 表 + 索引(§2 DDL);schedule JSON internally-tagged enum 反序列化 + 校验。
- [x] `scheduler/compute.rs`:`most_recent_due` + `next_fire_display` 两个纯函数 + 单测(§8 清单;catch-up 与常规触发同一算法,无独立 pass;**interval 无累积漂移不变量**)。
- [x] **origin 载体链(评审 P0,四点一线)**:
  - [x] `TaskOrigin` enum(scheduler/mod.rs 或 llm/types,internally tagged,随 QueuedMessage 序列化进排队占位 IPC,不进 chat 事件主链);
  - [x] `ChatEntry.origin: Option<TaskOrigin>`(chat.rs,全库 2 处构造点补 None:routes/agent.rs:61、chat.rs:107);
  - [x] `QueuedMessage.origin: Option<TaskOrigin>`(message_queue.rs additive)+ `chat_inner` 路由临界区 push 时拷入;
  - [x] 驱动器取 `drained.last()` origin 经 `ChatLoopRequest` 新字段传入 → init.rs persist 门控 `… || origin.is_some()` + metadata 信封加 `scheduled` 键。
- [x] `AuditKind::ScheduledTaskFired` + as_str;payload 动作枚举 `fired/catchup/skipped_dedup/skipped_queue_disabled/lost/error`(error 附 reason);`turn_seq` 按签名传零值,不留 TODO。
- [x] `daemon/server.rs::spawn_task_scheduler` wrapper:30s tick + 单一扫描算法(§4 伪代码:**落账记 due;fired_sessions 同 session 每 tick 至多一次**);去重(队列滞留 peek,只记 Queued uuid);fire 构造(prompt + 注脚 + origin)→ chat_inner;queue-disabled gate。
- [x] `AppState.scheduler_cancel: CancellationToken` 字段(load_inner 直接构造,纯分配无 spawn)+ bin 装配 + shutdown_signal 步骤 ①.6 cancel。
- [x] Stop 语义两处清队点(驱动器 cancel break chat.rs:1126 + Stop 命令 cancel.rs:69)对带 origin 条目补 `lost` 审计——清队前 `list_session` 快照识别(best-effort;sessions.rs 破坏性清理三处不审计)。
- [x] `update_scheduled_task` false→true 分支:`last_fired_at = now`(重启用不补跑存量)。
- [x] 集成测试:预置 last_fired_at 在过去的任务实 fire(mock LLM,`MockProvider` + tests_common)断言 `metadata.scheduled` 三键、audit 动作;去重 / kill switch / queue-disabled / catch-up 幂等 / **同 session 双任务 deferral** / **lost(Stop 清队)** / FK 级联 / 群聊目标拒绝;**多 drain 场景钉现状**(AC8,RULE-QUEUE-001 根治前防漂移)。
- 验证:`cargo test -p everlasting --lib` 全绿 + clippy -D warnings + fmt;live:建 1min interval 临时任务,turn-smoke 式观察 fire 一轮后删除。

## WP2 wire + 前端管理面

- [x] `commands/scheduled_tasks.rs` 四命令 `_inner` + Tauri 注册(lib.rs)+ daemon routes 4 条(Q0 单源)+ parity 测试。
- [x] `stores/scheduledTasks.ts` + `transport/http.ts` 映射;Settings 第 8 个 tab「定时任务」(列表 + 新建表单 + 编辑/启停/删除;停用行灰显;同 session 已有 enabled 任务软警示;顶部「调度仅在 daemon 进程运行时生效」提示;移动端 S6b 约定)。
- [x] `AppConfigPayload` 加 `scheduledTasksEnabled`(additive)。
- [x] `MessageItem` 定时来源标识(metadata.scheduled,零 rehydrate 改动——`MessageRow.metadata` 已整体透传);F1 排队占位「定时」徽标(list_queued_messages origin);session header 活跃任务徽章(AppShell 启动拉一次);`utils/audit.ts` labelForKind + icon 加 `scheduled_task_fired` 档。
- [x] 前端测试:store actions、表单档位切换/校验/软警示、列表启停交互。
- 验证:pnpm test + vue-tsc + pnpm build;浏览器模式(daemon serve dist)手测 CRUD 全链;PWA 320–430px 零溢出过一遍。

## WP3 live 验收 + 文档收口

- [ ] live 验收(留用户,环境需重建 daemon + 真 LLM):AC1–AC8 逐条过;AC4 停 daemon 跨 fire 点实测补跑一次;AC2 验证 user 气泡「定时」标识于 done 后出现(R6 预期)
- [x] 文档:ROADMAP §1.2 加行 + §2 第四档 F2 划掉(F1-C cron 消费者交付注记);ARCHITECTURE F1 段「生产者未实现」更新;HACKING-wsl 新增「调度边界」节;design §6 补 WP2 targetSessionId 加参注记。spec 沉淀(3.3)见下方进行中。

## 验证命令速查

```bash
cargo test -p everlasting --lib          # 全量(需 PKG_CONFIG_PATH,见 HACKING-wsl 坑 1)
cargo clippy -p everlasting --lib -- -D warnings
cd app && pnpm test && pnpm build        # vitest + vue-tsc
scripts/turn-smoke.sh                    # 改 fire 链路后的 live 冒烟
```

## 风险点与回滚

- **风险 1(origin 链穿三个结构体)**:ChatEntry/QueuedMessage/ChatLoopRequest 三处 additive 字段 + 驱动器尾条取值——所有既有调用点补 None,`drained.last()` 在 drained 非空防御分支保证;多 drain 时 origin 取尾条与「persist 只写尾条」对齐。
- **风险 2(路由临界区)**:push 拷 origin 在既有临界区内完成(纯赋值,无 await);去重 peek 遵守「先 queues 后 active_request、临界区内无 await」锁纪律,不新增临界区。
- **风险 3(catch-up 与任务编辑竞态)**:update 仅写库、判定每 tick 重算 → `not_before=max(created_at, last_fired_at)` 不含 updated_at,编辑 prompt/schedule 后下一 tick 即按新 schedule 判定(可接受;若要编辑后强制等一周期,再评估)。
- **风险 4(丢失窗口)**:last_fired_at 前移后 Stop/重启/preflight 三类丢失接受不补偿(§4.4);`lost`/`error` 审计兜底;若 dogfooding 期实测丢损不可接受,再立「消费回执」follow-up。
- 回滚单元:每 WP 一组提交;调度 wrapper 独立装配,可单点摘除(bin 一行);origin 链字段全部 `Option` + 既有路径 None,可独立 revert。
