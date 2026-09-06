# 群聊 P1a checkpoint 落库与续跑

> 来源:GCE-ROADMAP §4/§6(P1a checkpoint 落库 / P1b 续跑 = M3 打断的信任底座余项 +
> M4 定时审议的容错前置)。2026-09-06 立项 brainstorm 收敛。

## Goal

中断的群聊讨论可拾起、可续跑:daemon 崩溃/被杀/熔断/硬停后,讨论进度以 checkpoint
形式留在 DB,对外可见为「中断于 X」,GUI 与 API 同权提供续跑入口。M3 打断的信任
底座一次闭环,并为 M4 定时审议容错铺路(F2 cron 死场可续)。

## Background(代码级事实,2026-09-06 调研)

- **转录即状态,全量持久**:每 speaker turn 经 `run_chat_loop` 落库;`reload_messages`
  (group_chat_loop.rs:187)从 DB 重建,speaker 归属往返。崩溃不丢内容。
- **roster 可从 DB 重建**:`build_group_chat_ctx`(group_chat.rs:128)解析
  `sessions.metadata.participants` + session 自身 model(moderator)+ `current_cwd`。
  续跑无需另存 roster(注意:续跑用的是**当前** metadata 的 roster,中途改配置生效)。
- **编排器真正的易失状态**(group_chat_loop.rs):`round` 计数
  (0..`MAX_ORCHESTRATION_ROUNDS`=30)、`consecutive_error_turns`(GC5,连错 3 熔断)。
  `pending_injects` 纯内存——F1 同风险姿态,已接受,不进本任务。
- **lifecycle 契约**(session_crud.rs:925/953):开跑 `clear_group_chat_lifecycle`
  (stop_reason/discussion_summary 置 NULL),退出 `finalize_group_chat_lifecycle`。
  **崩溃/被杀 = 无 finalize** → 重启后 busy=false(内存 map 派生)、stop_reason=NULL,
  轮询方看到「空闲、从未跑过」;且新 chat 会静默重启编排器并清 lifecycle(M3 P1-1
  副作用)——被中断的讨论无痕蒸发。这是本任务要修的核心缺口。
- **现有终态**:`group_chat_end`(正常,带 summary)/ `max_rounds` / `error`(GC5)/
  `cancelled`(硬 Stop,无 summary)/ `preempted`(M3 收束式,带 summary)。
- **入口与注册链**:`chat_inner`(chat.rs:702)对 group_chat session 不 busy 时走
  legacy 路径(3a 防御性取消 + 认领 + preflight + spawn),busy 时用户消息走注入
  (P0)。`ChatEntry`(chat.rs:180)是收敛二元签名,群聊/队列/经典三分支共享套件。
- **命令先例**:`preempt_group_chat_inner`(commands/cancel.rs:115)+ daemon 路由
  (routes/cancel.rs:39)+ Tauri 注册(commands/mod.rs:92)三件套。
- **schema 授权**:GCE-ROADMAP §7「v1 不新增 DB 表……M3 checkpoint 需要时随内部 P1a
  立项」——本任务是被授权加表的立项点。建表先例:schema.rs `CREATE TABLE IF NOT
  EXISTS`(如 scheduled_tasks,schema.rs:1323)。
- **wire 现状**:`SessionSummary.stop_reason`(types.rs:454)已在 list_sessions
  wire 上;`AppState::load_from_dir`(state.rs:326)是 daemon bin 与 Tauri Full 的
  共享初始化点。crash 场景本就无 SSE——新 stop_reason 值只经 DB/轮询面出现,
  live 事件零新增。
- **M4 消费方**:F2 scheduler(daemon/server.rs:199)经 `chat_inner` 派发。

## 已定决策(brainstorm 2026-09-06)

1. **Q1 范围 = 全量**:checkpoint 表 + boot 中断标记 + resume 命令(daemon 路由 +
   Tauri command)+ GUI 中断态通知与「续跑」按钮 + 脚本可见可验证。
2. **Q2 可续跑终态集**:可续跑 = 中断(crash 残留)+ `cancelled` + `error`;
   不可 = `group_chat_end` / `preempted`(有收束 summary 的完整终局,续跑会推翻
   自己刚写的总结)/ `max_rounds`(预算烧完)。**checkpoint 行的存留编码可续跑性**:
   可续跑退出保留行、终局退出删行;行在 + !busy = 可拾起。
3. **Q3 存储 + 可见性 = 新表 + boot sweep**:新表 `group_chat_checkpoints`
   (runtime 状态与 GUI 可编辑的 sessions.metadata 分离,避开读-改-写 clobber);
   boot 一条 SQL 把「有 checkpoint 行且 stop_reason=NULL」的 session 写
   `stop_reason='interrupted'`——既有「!busy + stop_reason → 终态」派生零 API
   变更即可见,M1 脚本 / MCP `discussion_status` 无需改。
4. **轮预算继承(技术推荐采纳)**:续跑从 checkpoint.round 进入,总预算仍封顶
   `MAX_ORCHESTRATION_ROUNDS`(crash→resume 循环不能无限烧轮)。
5. **续跑入口形状(技术推荐采纳)**:独立命令 `resume_group_chat(session_id)`
   (preempt 三件套先例),内部经 `ChatEntry` 复用 chat_inner 全套认领/preflight/
   许可机制——不复用 chat 加旗标直达(无尾条消息,语义惊讶)。
6. **moderator 续跑指令(技术推荐采纳)**:恢复后首个 moderator 轮的 system prompt
   追加「从中断处继续主持」指令(`moderator_wrapup_instruction` 先例)。
7. **GC5 计数重置(技术推荐采纳)**:resume 进入时 streak 归零(人已介入,旧熔断
   计数不作数);crash 残留的 streak 同样不继承。

## Requirements

- **R1 checkpoint 落库**:新表 `group_chat_checkpoints`;编排器每轮头 upsert
  (round + error_streak),speaker 轮后 streak 变化时同步;开新场(resume.is_none)
  先删旧行(与 `clear_group_chat_lifecycle` 对称,防复用 session 残留旧 started_at)。
- **R2 中断可见**:boot sweep 在共享初始化点(`AppState::load_from_dir` 的
  `load_inner`,紧邻 `reap_orphaned_runs` / `recover_interrupted_messages` 两个
  崩溃恢复先例):stop_reason=NULL 且有 checkpoint 行 → 写
  `stop_reason='interrupted'`(**不写 `sessions.updated_at`**——侧栏按它排序
  + 展示时间,写 boot 时刻会造成排序突变与错误的中断时刻;中断时刻 ≈ 最后
  消息落库时刻,误差一轮内);同时清理孤儿行(行在 + stop_reason 为终局三值
  → 删行,自愈「finalize 成功但删行失败」的残留)。幂等,绝不覆盖已有终态值;
  计数留日志。
- **R3 续跑命令**:`resume_group_chat(session_id)`(Tauri command + daemon 路由):
  校验(session 为 group_chat、!busy、checkpoint 行在、round < MAX、
  **stop_reason 不为终局三值** `group_chat_end`/`preempted`/`max_rounds`——行
  存留是 best-effort 编码,删行失败残留时该兜底防止续跑已收官场、推翻 summary)
  → 经 `ChatEntry{resume}` 走 chat_inner → 编排器从 start_round 进入、round 0 视为
  reload(不吃空尾条)、首 moderator 轮带恢复指令、streak 归零、lifecycle 照常
  clear→finalize。
- **R4 GUI 同权**:群聊 session 处于可续跑态(!busy + stop_reason ∈
  {interrupted, cancelled, error})时,ChatPanel 呈现中断通知 + 「续跑」按钮;
  点击走 resume → `Started` 受理 → SSE 照常跟随。正常发送仍走新开一场(不劫持),
  通知文案明示「发送新消息将开始新讨论,不再续跑」。**数据通路**:前端
  `SessionSummary` TS 类型现无 `stop_reason` 字段(需补,wire 已有值);终态后
  内存 summary 不刷新(`reloadAfterFinalize` 只重拉消息缓冲)——需在 finalize
  路径把 load_session 返回的 session 字段合并回 `sessions[]`,否则按钮不出现。
  按钮提交后防抖(受理/busy 翻转前 disable,防双击双 resume)。
- **R5 退出路径的行存留**:`cancelled` / `error` 退出保留 checkpoint 行(round 记
  到终局轮);`group_chat_end` / `preempted` / `max_rounds` 退出删行。
- **R6 文档与脚本接线**:DAEMON-API.md(新 stop_reason 值 `interrupted` + resume
  端点 + lifecycle 三态机补中断态)、GCE-ROADMAP §4/§6 落账、ROADMAP §1.2 行;
  M1 脚本 `group-chat-run.mjs` 的 `EXIT_BY_STOP_REASON` 补 `interrupted` 档
  (专属 exit code,不落入「脚本自身错误」的 exit 1)+ 转录 note 提示可经
  resume 续跑(判定逻辑零改动)。

## Acceptance Criteria

- [x] **AC1 落库与生命周期**:讨论进行中每轮头 checkpoint 行更新(round 单调);
  正常收官/preempted/max_rounds 后行删除;cancelled/error 后行保留且 round 正确。
- [x] **AC2 crash 中断可见**:模拟崩溃(SIGKILL;daemon.sh stop 是 SIGTERM 走
  graceful → finalize `cancelled`,测不出 interrupted)后重启,boot sweep 将该
  session 标 `stop_reason='interrupted'` 且**不改 `updated_at`**;已有终态值的
  session 不被改写;孤儿行(终局 stop_reason 残留)被清理;MCP
  `discussion_status`(`isTerminal` open-ended)与 M1 `run` 轮询不改判定逻辑
  即可报出中断态。
- [x] **AC3 续跑语义**:resume 后 moderator 首轮看到完整 reload 转录(含中断前
  内容)+ 恢复指令;轮预算继承(start_round 起算,总帽不重置);续跑场再正常收官
  → summary 完整、stop_reason=group_chat_end、行删除。live 时观察重跑轮的转录
  重复度(轮头粒度固有:崩溃在上一轮完成后 → resume 重跑完整上一轮,已落库
  发言在转录中近似重复;可接受,必要时后续加 speaker 级断点)。
- [x] **AC4 命令校验**:非 group_chat session / busy / 无 checkpoint 行 /
  round≥MAX / stop_reason 为终局三值(模拟删行失败残留)五类拒绝路径各有明确
  错误。
- [x] **AC5 GUI 同权**:可续跑态可见通知 + 续跑按钮(TS `SessionSummary` 补
  `stop_reason` 字段;finalize 后 session 字段合并回 `sessions[]` 使按钮即时
  出现;提交后防抖);点击后讨论续跑且事件流正常(占位/流式/收官);正常发送
  不受影响(新开一场)。前端单测锁按钮门与受理。
- [x] **AC6 回归**:经典聊/队列/群聊既有行为零变化(全量 `cargo test -p
  everlasting --lib` + 前端 vitest + vue-tsc + clippy/fmt 绿)。
- [x] **AC7 live 验证**:一场真 LLM 讨论 → 中途 SIGKILL(pidfile 直杀,非
  daemon.sh stop)→ 重启见 interrupted → resume 续跑 → 正常收官出 summary
  (脚本驱动,转录留档 out/)。

## Out of Scope

- `pending_injects` 崩溃保全(F1 同款风险姿态,已接受)
- MCP `resume_discussion` 工具面(M4 cron 落地时随需;`discussion_status` 报中断态
  靠 stop_reason 零改动达成;`isTerminal` 的 JSDoc「四值枚举」注记顺手更新)
- F2 scheduler 自动 resume(消费方另立)
- preempted / max_rounds 的续跑(已决策不可续跑;max_rounds 续跑 = 预算扩展,
  属「延长」语义另议)
- per-speaker 轮级细粒度断点(轮头粒度足够:至多重跑中断所在轮——含该轮已落库
  发言在转录中近似重复的固有代价,AC3 live 观察后如痛点再议)
