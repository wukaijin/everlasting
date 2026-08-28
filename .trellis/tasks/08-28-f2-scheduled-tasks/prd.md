# F2 定时任务(本地 cron 式)

## Goal

本地定时唤醒 agent 跑任务(定时拉取 / 定时汇报):daemon 常驻进程按计划向目标 session 注入一轮 agent 运行,结果落在 session 消息流里,复用 F6 编排面(busy 红点 / 完成 toast)跨端可见。用户经 Settings「定时任务」面板管理任务;PWA/移动端同样可管理。

## 背景与定位

- ROADMAP 第四档唯一待做的调度类功能(docs/ROADMAP.md:177);F1-C(daemon 统一注入入口)已裁定归 F2,原定两个消费者:cron + LLM detached dispatch —— 本任务交付前者,后者拆 follow-up(D5)。
- DESIGN §3.2 硬约束(docs/DESIGN.md:115,117):触发源必须本地(系统时间 / fs 事件 / 本地 webhook),不接云端触发器;不做云端自动推送任务回写本机。
- 代码勘察 + 外部评审(2026-08-28,两个 Explore 代理取证 + 独立评审代理实施前审查):注入链路与周期任务装配模式均已就位,本任务是**近似纯编排层**——核心链路零改动,但来源标记需要一个小面 agent-loop 增量(P0,见 R3/勘误)。

## 已确认事实(代码勘察)

### 注入链路(基本免费)

- 统一入口即 `chat_inner(state: &Arc<AppState>, entry: ChatEntry)`(app/src-tauri/src/agent/chat.rs:191-194),fire-and-forget spawn(chat.rs:552,755);「闲也入队」路由已统一:每条发送都先入 per-session 队列,闲时入队+认领+spawn 驱动器,忙时仅入队返 `Queued{position}`(chat.rs:306-383)。
- **F6/F3/F1 对定时注入轮自动生效**:busy enrich(commands/sessions.rs:24-46)、`loop_permits` 信号量——闸满排队不拒绝(chat.rs:578,985-995)、完成 toast(前端 streamController.ts:500-513)都走同一 chat_inner 路径。
- 程序化建 session 无障碍:`create_session_inner(state, project_id, initial_cwd, ...)`(commands/sessions.rs:56-146),必填仅 project_id + initial_cwd。

### 周期任务装配模式(现成先例)

- daemon bin 已有 3 个 detached 周期任务,全部只在 `bin/everlasting-daemon.rs` 装配:**GUI 零 timer 是硬约束**(Full/tauri 模式无调度,与 tunnel 空壳同构):backup 24h(server.rs:72-117)、shell sweeper 5min(server.rs:139-153)、tunnel 心跳 10s(tunnel/client.rs:220-240,带 CancellationToken 优雅退出,最完整样板)。
- spec 约束:.trellis/spec/backend/daemon-server.md:255-275「wrapper + detached + `AppState::load`/`default_registry()` 绝不 spawn」。
- 装配插槽:bin/everlasting-daemon.rs:158-176(backup 与 sweeper 附近);启动恢复(recover_interrupted_messages)先于 bin 的 spawn 点。

### 持久化与配置

- 无 schema version 机制,全序列幂等重放;新表直接在 schema.rs 追加 `CREATE TABLE IF NOT EXISTS`(app_config 先例 schema.rs:242-249);幂等列 helper 在 migrations/columns.rs。
- 配置走 KV 表 `app_config` + fail-open 语义(仅字面 "false" 才关);新开关在 `AppConfigPayload` 加字段(commands/config.rs:489-510)+ daemon route 自动跟随(_inner 单源);加 key 不需 migration。

### 缺口(本任务新建)

- 全库零调度代码(`tokio::time::interval` 仅 3 处运维任务);`scheduled_tasks` 持久化不存在。
- **无「消息来源」概念**:messages 表无 source 列;wire 层 `ChatMessage` 只有 speaker/attachments,**没有 metadata 字段**——`messages.metadata` 由 agent loop init 阶段写,且门控为「有 injections 或 attachments 才写」(init.rs:972-1003,信封固定 `{injections, attachments}`)。定时注入要带来源标记,**必须小面扩展**:`ChatEntry` 加 origin → 路由入队时拷进 `QueuedMessage.origin` → 驱动器取尾条 origin 经 `ChatLoopRequest` 传入 init persist 点,门控放宽 + 信封加 `scheduled` 键(additive)。评审确认 `ChatEntry` 单独加字段不够:忙时条目由另一个请求的驱动器消费,round>0 不携带请求级上下文(chat.rs:1076-1080 同款丢弃机制),载体必须在 `QueuedMessage` 上。
- **F1-A 既有缺陷(评审发现,登记 DEBT §RULE-QUEUE-001)**:驱动器把全部 drained 条目塞进 turn 输入(chat.rs:1069-1072),但 persist 点只写尾条(init.rs:783-784)——多条同轮 drain 时非尾条 user message 不落库,reload 后从时间线消失。F2 定时注入经常在队列里等待,放大触发概率。F2 侧缓解见 R4;根治归 DEBT。
- 群聊 session 不走消息队列路由(chat.rs:338 legacy 路径)→ 定时注入目标限定 classic session;`message_queue_enabled=false` 时 legacy 分支对忙 session 是「取消在跑轮 + 顶替」语义(chat.rs:470-500),fire 必须避开。

## 已定决策(brainstorm 2026-08-28,用户逐项裁定)

- **D1 载体模型 = 绑单一 target session**:建任务时选定目标 session(既有 session,或表单里一键新建专用 session);每次 fire 往该 session 注入一条带来源标记的 user message,agent 在该 session 上下文续跑。每次 fire 新建 session 的模式不做(报告类长期追加进专用 session,C3+ 压缩兜底变长)。
- **D2 调度语法 = preset 档位**:结构化枚举存 DB——`daily HH:MM` / `interval 每 N 分钟` / `weekly 周X HH:MM`,后续档位 additive 扩展;不引 cron 表达式/解析依赖(符合项目零依赖手写惯例)。UI 用下拉 + 时间选择器。
- **D3 MVP 触发源 = 只做系统时间**:fs 事件 / 本地 webhook 是另一类触发源(不同基建),不进本任务。
- **D4 错过调度 = 补跑一次(catch-up once)**:daemon 启动时检查,停机期间有到期点未触发则补跑一次(不追多次);`last_fired_at` 为补偿判定基准。
- **D5 LLM detached dispatch = 拆 follow-up**:MVP 只做用户创建的任务;调度循环落地后 `schedule_task` tool 是同一张表的薄封装(created_by=agent 区分),独立小任务补齐(权限面/审计/C7D stub 评估/防递归排程另议)。
- **D6 管理 UI = Settings tab 单面**:Settings 新增 tab「定时任务」——任务列表(目标 session / schedule / prompt 摘要 / 上次与下次触发 / 启停开关)+ 新建表单 + 编辑 + 删除;PWA 同样可用。session 侧只加小徽章(有活跃任务)。session 级创建入口不做。

## Requirements

- **R1 任务模型与持久化**:`scheduled_tasks` 表——id / project_id / target_session_id / name / prompt(注入的 user message 正文)/ schedule(preset JSON)/ enabled / created_by('user')/ created_at / last_fired_at / next_fire_at(纯展示,权威触发判定在调度器内重算)。CRUD 经 Tauri command + daemon route 双注册(Q0 单源 `_inner`)。
- **R2 调度循环(单一扫描算法)**:daemon bin 装配 detached wrapper(沿 daemon-server.md Pattern;CancellationToken 挂 AppState 字段,shutdown 链 cancel)。每 tick:载入 enabled 任务 → Rust 侧重算「自 `max(created_at, last_fired_at)` 以来最近到期点,且 ≤ now」→ 命中即 fire 一次并落账;**落账记理论到期点 `last_fired_at = due`**(保证 interval 实际间隔恒等于步长,tick 量化误差不累积;catch-up 判定语义 =「到期点是否已消费」)。catch-up(D4)与常规触发是**同一算法**,不做独立 pass(防同 tick 双 fire)。**同一 target_session 每 tick 至多 fire 一个任务**,余者顺延下 tick(消掉多任务同 tick 对 RULE-QUEUE-001 的确定性触发)。存库 `next_fire_at` 仅 UI 展示(停用任务存 schedule 下一到期点,灰显)。
- **R3 fire = 注入一轮 + 来源标记载体**:经与用户发送同源的内部路径进 chat_inner(「闲也入队」免费获得忙时排队);来源标记链 = `ChatEntry.origin`(仅 scheduler 传)→ 路由入队拷入 `QueuedMessage.origin` → 驱动器取 drained 尾条 origin → persist 门控放宽(init.rs `… || origin.is_some()`)→ metadata 信封新增 `scheduled` 键 `{task_id, task_name, fired_at}`(不动既有 injections/attachments)。origin 随 `QueuedMessage` 序列化进 `list_queued_messages`,前端排队占位同样可显「定时」徽标。注入正文追加一行触发时间注脚(模型带日期上下文)。每轮 fire 落新 `AuditKind::ScheduledTaskFired`,payload 动作枚举 `fired | catchup | skipped_dedup | skipped_queue_disabled | lost | error`(error 附 reason,如 queue_full)。
- **R4 防堆积与落账语义**:仅 classic session 可作目标(创建/更新校验,群聊拒绝;同 session 已有 enabled 任务时表单软警示);`message_queue_enabled=false` 时 fire 跳过并审计(legacy 分支会砍在跑轮,不可用);同一任务上一次注入条目仍在队列未消费 → 本次跳过去重(**uuid 仅 `Queued` 返回路径可得,`Started` 路径不记**);**disable→enable 不补跑存量**(update false→true 时 `last_fired_at = now`,从下一个到期点开始——显式禁用是用户主动行为,与 daemon 非自愿停机的 D4 补跑语义分界)。fire 后「账已落、轮没跑」的丢失窗口(Stop 清队 / daemon 重启丢内存队列 / preflight 失败仍返回 `Ok(Started)` 且错误只走 SSE):接受为已知损耗,`Err` 必记 `error` 审计、Stop 语义两处清队点(驱动器 cancel break + Stop 命令)对带 origin 条目补 `lost` 审计(best-effort;delete_session 等破坏性清理不审计);preflight 假开跑窗口接受不补偿(design §9 记录)。
- **R5 管理界面**:Settings「定时任务」tab(D6);新建表单含 project → session 选择(含「新建专用 session」)、preset 档位、prompt 编辑;列表含上次/下次触发时间与启停开关;全局 kill switch `scheduled_tasks_enabled`(fail-open);面板注明「调度仅在 daemon 进程运行时生效」(GUI Full/tauri 逃生模式可建任务但不会触发)。
- **R6 前端可见性**:消息气泡对 `metadata.scheduled` 显示「定时」标识;session header 小徽章(有活跃任务);完成通知/busy 红点由 F6 既有面自动覆盖。注:跨端实时认领只建 assistant 占位(streamEvents `adoptForeignRequest`),带标记的 user 气泡在该轮 done 后权威重拉才出现——验收按此预期,不算缺陷。

## Acceptance Criteria

- [ ] **AC1** Settings「定时任务」tab 可建/编辑/启停/删;任务落 DB,daemon 重启后仍在且触发判定正确重算(双 transport 下行为一致)。
- [ ] **AC2** 到点自动 fire(live 验证:建 1min interval 临时任务实跑):目标 session 收到带来源标记的 user message 并完成一轮 agent 回复;TurnCard/trace 正常落值;user 气泡「定时」标识在轮次 done 后可见(R6 预期)。
- [ ] **AC3** 目标 session 忙时 fire 不堆积(R4 去重语义生效);F3 闸满时排队不拒绝;busy 红点与完成 toast 对定时轮生效。
- [ ] **AC4** 补偿:daemon 停机跨过 fire 点,重启后补跑一次;同一次停机不重复补跑;无到期点错过则不补;**disable→enable 不补跑存量**(从下一个到期点开始)。
- [ ] **AC5** 每轮 fire 落 `ScheduledTaskFired` 审计,动作枚举覆盖 `fired/catchup/skipped_dedup/skipped_queue_disabled/lost/error`(error 附 reason);审计查询 UI 可见(人话 label + icon 映射)。
- [ ] **AC6** 删除目标 session → 关联任务级联删除;`scheduled_tasks_enabled=false` 时调度循环空转(不 fire);`message_queue_enabled=false` 时 fire 跳过并审计。
- [ ] **AC7** 群聊 session 在创建/更新时被拒;GUI Full(tauri transport)模式不跑调度(GUI 零 timer 约束保持),Thin/裸跑 daemon 模式正常。
- [ ] **AC8** 测试:触发判定纯函数单测(daily/interval/weekly/边界/catch-up 与常规触发同一算法/**interval 无累积漂移不变量**);fire 注入集成测试(mock LLM,含 busy 入队 + 驱动器消费后 metadata.scheduled 落库断言);去重/级联/校验/queue-disabled 分支单测;**同 session 双任务同 tick 的 deferral 断言**;多 drain 场景钉住现状行为(scheduled 与手动消息同轮 drain 的落库结果被显式断言,RULE-QUEUE-001 根治前防漂移);CRUD 双 transport parity。全量 cargo test --lib + pnpm test + clippy/fmt 绿。

## Out of Scope(明确不做)

- fs 事件 / 本地 webhook 触发源(D3;留 follow-up)
- LLM `schedule_task` tool / agent 自主排程(D5;独立 follow-up)
- 每次 fire 新建 session 的投递模式(D1)
- cron 表达式语法(D2;档位后续 additive)
- 系统级 OS 通知 / unread 持久化 / 等待态心跳(F6 遗留增强,另立)
- 跨设备任务配置同步(remote epic 边界外)
- 多任务编排(依赖链 / DAG)/ 任务执行超时强杀(轮次生命周期归 agent loop 既有机制)
- F1-A 多 drain 非尾条不落库的根治(→ DEBT §RULE-QUEUE-001;F2 只做 R4 缓解 + AC8 钉行为测试)
- fire 后「队列内存丢失/Stop 清队」场景的补偿重跑(R4 接受为已知损耗,只审计不补偿)

## Technical Notes(给 design.md 的输入)

- 调度计算(最近到期点 / catch-up)做成纯函数单测;时区用本地时间(chrono Local)。
- fire 走进程内函数调用(与 HTTP handler 同一 `_inner` 源),非 HTTP 自呼。
- 注入正文注脚给模型带触发日期;metadata.scheduled 信封键为前端渲染依据(链路见 R3)。
- GUI Full 模式无调度是既有硬约束的自然结果;Settings 面板提示 + HACKING/ROADMAP 注明。
- 去重 uuid 仅 `Queued` 返回路径可得(design §4);`QueuedMessage.origin` 为 additive 新字段,不动 push/drain 既有语义。
