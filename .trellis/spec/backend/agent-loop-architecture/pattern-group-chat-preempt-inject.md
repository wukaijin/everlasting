# Pattern: 群聊注入 / 体面打断(controls 注册表 + 轮边界语义)

> 来源:任务 `.trellis/tasks/09-06-gc-p0-preempt-min-semantics/`(GCE-M3 硬前置,
> GROUP-CHAT-API-ROADMAP §4/§6)。消费方:群聊编排器、`chat_inner` 路由层、
> daemon API 调用方(DAEMON-API §4「进行中干预」)。

## 问题与语义(为什么存在)

群聊(busy)期间的用户消息,旧路径走 legacy 3a 防御性取消——**cancel 整场讨论再
重开**(GUI 曾有意包装成 D9-Q4「打字即打断」,但对外部 API 调用方是毁场事故);
唯一停讨论手段是 CancellationToken 硬停(在途发言被斩、无总结)。

P0 定案(2026-09-06 brainstorm,Q1/Q2 用户决策):

1. **打字/发消息 = 注入(inject,非破坏)**:进 controls 缓冲 → 编排器轮头落库
   (带标记)→ 下一 moderator 轮 reload 可见(`role_history` 对 user 行原样透传,
   投递零额外机制)。
2. **preempt = 收束式打断(session 域)**:`preempt_group_chat(session_id)` 置位 →
   轮边界检测 → **等在途 speaker 跑完**(不立杀流;立杀是 M3 的 per-speaker
   child token 增强)→ moderator 收束一轮(复用 end_discussion 落 summary)→
   `stop_reason="preempted"`;收束失败重试 1 次后兜底立断(summary 如实缺)。
3. `cancel_chat`(Stop)= rid 域硬停,语义保留(止损最后手段)。

## 载体结构(load-bearing)

```rust
// state.rs — 锁纪律:group_chat_controls 永远最后获取
AppState.group_chat_controls: Arc<Mutex<HashMap<session_id, GroupChatControl>>>
GroupChatControl = Arc<Mutex<GroupChatControlInner { pending_injects, preempt_requested }>>
```

- **注册**:编排器入口 `run_group_chat_loop` 开头 insert(同 GC1 的 cancellations
  先例:一次注册覆盖整场);**清理**:编排器尾部单点 remove(全退出路径汇于函数尾,
  与 cancellations/session_active_request 同点)。insert 覆写语义自愈 panic 残留。
- **写入方**:`chat_inner` 路由临界区(busy 分支 push 注入,返回
  `ChatAcceptance::Injected`,无流无 rid)/ `preempt_group_chat_inner`(置位)。
- **消费方**:编排器轮头 drain + preempt 检查;退出尾部 flush 残余注入。

## seq 纪律(违反即打断在途发言轮)

**注入绝不从命令层直插 DB。** 活跃 inner loop 持内存 seq 游标(compaction_summary
先例,`session_crud.rs` insert_compaction_summary 文档),独立 `MAX(seq)+1` 直插轻则
自己撞 `(session_id, seq)` 主键(RULE-PERSIST-001 裸 INSERT 是刻意的 bug 信号),
重则占走在途 speaker 下一次 persist 要用的 seq、**反过来打断发言轮**——participant
burst(max_turns=20)窗口分钟级,撞上是常态。

正确落点:编排器**轮头**(上一 inner loop 已退出、下一未进入,无活跃游标)与
**退出尾部 flush**——仅这两处可安全调 `db::insert_user_inject`(内部走 MAX+1)。
缓冲纯内存,daemon 崩溃丢未消费注入(F1 队列同风险姿态,接受)。

## schema 决议(R3,双轨标记)

注入行 = user 行(`speaker=NULL`,role 归属不变——注入者是人):

- `metadata.kind = "user_inject"`(API/检索侧机器可辨,仿 worktree_event 先例);
- text 列带 `[用户插入] ` 前缀(LLM 侧确定性可辨——`ChatMessage` 不携带 metadata,
  文本标记是唯一能到达 moderator 上下文的通道;前缀落库是有意分歧:注入行只经
  DB reload 进入视野、永不进 chat 载荷,不会污染 rehydrate/edit 路径);
- moderator prompt 内置 `[用户插入]` 语义指引(用户新指令,优先吸收;不因此
  扩大 end_discussion 权限)。纯图片注入 P0 不支持(路由层报错,不静默丢)。

M3 `inject_message` 的参数形状继承本决议(topic 文本;已交付 2026-09-06,task
`09-06-gce-m3-control-plane`)。**M3 落地补记(评审 P1-1)**:空闲/已收官群聊 session 一旦
收到 chat 消息会重启编排器并无条件清 lifecycle(上一场 stop_reason/summary 不可逆丢失),
故 MCP `inject_message` 在客户端层前置 busy guard(非 busy 根本不发起),fireChat 后
acceptance 非 `injected` 用自有 rid 即时 cancel 仅作竞态兜底——**该副作用是 daemon 语义,
不可在包装层修复;换任何暴露层(GUI 之外的新入口)都必须复制此前置 guard**。

## 前端契约

- 群聊 busy 打字 = 注入:`chatSendActions` 的先-cancel 分支已退役(D9-Q4 毁场式
  抢占);`@@` 前缀流式中拒绝(经典排队/群聊注入同规)。
- `{"status":"injected"}` 受理:assistant 占位回收、activeRequests 出表、user
  消息保留**无**队列徽标(不进经典队列视图,无撤销/退回)。
- `stop_reason="preempted"`:finalize 白名单 + notice(终态异常类,同 error 位)。

## 回归锚点(tests_group_chat.rs)

| 用例 | 锁定 |
|---|---|
| `group_chat_inject_lands_next_moderator_round_and_survives_discussion` | 注入落库双轨标记 + 下一 moderator 轮可见 + 讨论不死 + seq 在发言后 + 注册表清理 |
| `group_chat_preempt_wrapup_produces_summary_and_distinguishable_stop_reason` | 收束轮 WRAP-UP prompt + summary 落库 + preempted 可区分 + 边界 drain 与退出 flush 两路注入都落库 |
| `group_chat_preempt_wrapup_fallback_halts_without_summary` | 收束两次失败兜底立断,stop_reason 仍 preempted,summary 如实缺 |

改动本模式任何一处(注册表生命周期 / 落库时机 / prompt 标记)必跑
`cargo test --lib "tests_group_chat"`;prompt 结构变更另跑 `scripts/turn-smoke.sh`
(行为层)。

## P1a 扩展:checkpoint 落库与续跑(09-06-gc-p1a-checkpoint-resume,2026-09-06)

打断的信任底座 = 中断可拾起。三层结构(全 best-effort warn+swallow 契约,
与 clear/finalize 同族):

1. **`group_chat_checkpoints` 表**(session_id PK,CASCADE):编排器**轮头**
   upsert(`round` + GC5 streak;preempt 检测后、moderator 轮前——被 preempt 的
   轮不算「到过」),streak 变更点同步;`ON CONFLICT` 永不触 `started_at`(行
   生命周期 = 一场讨论;**新场开跑先删旧行**,与 clear_group_chat_lifecycle
   对称,否则复用 session 携带上一场的 started_at)。
2. **行存留编码可续跑性**:`cancelled` / `error` 退出**留行**,终局
   (`group_chat_end` / `preempted` / `max_rounds`)退出**删行**;「行在 +
   !busy」= 可拾起。删行是 best-effort——**命令侧第五校验(终局 stop_reason
   拒绝)是 belt,boot sweep 清孤儿行是 braces**,缺一会让直调 API 续跑已
   收官场、推翻 summary。
3. **boot sweep**(`recover_group_chat_checkpoints`,挂 `load_inner` 崩溃恢复块,
   reap_orphaned_runs / recover_interrupted_messages 之后——RULE-PERSIST-001
   的 session 级兄弟):行在 + stop_reason=NULL(= 无 finalize,只有进程死亡会
   这样)→ 标 `interrupted`。**不写 `sessions.updated_at`**(侧栏按它排序 +
   展示,写 boot 时刻 = 排序突变 + 假时刻;中断时刻 ≈ 最后消息时刻)。

**续跑语义**(`resume_group_chat` 命令,daemon 路由 + Tauri 同权):五类校验
(非群聊 / busy / 无行 / round≥MAX / 终局 stop_reason)→ `ChatEntry{
resume_group_chat: Some(round)}` 走 chat_inner(复用全套认领/preflight/许可,
messages 空)。编排器侧:`for round in start_round..MAX`(**预算继承**——
crash-resume 循环不能无限烧轮)、`round == 0 && resume.is_none()` 才吃传入
 messages(空尾条绝不进 D-D 持久化)、首 moderator 轮 prompt 追加
`moderator_resume_instruction()`(wrap-up 同款追加式)、streak 归零。**前端零
流接线**:新 rid 的 SSE 事件经 adoptForeignRequest 自动认领(scheduled-task
fire 同款);续跑按钮门 = `stop_reason ∈ {interrupted, cancelled, error}` +
!busy(TS `SessionSummary` 已补字段;`reloadAfterFinalize` 把 load_session 的
session 行终局字段合并回 `sessions[]`,否则按钮要等下次 list_sessions)。

**回归锚点(追加)**:`group_chat_resume_enters_at_checkpoint_round_with_reload_and_instruction`
/ `group_chat_error_exit_keeps_checkpoint_and_fresh_run_resets_it` /
`group_chat_cancelled_exit_keeps_checkpoint_row`
(tests_group_chat.rs)+ `resume_group_chat_route_validates_five_gates`
(routes/agent.rs)+ DB 三例(sessions_tests)。编排器/lifecycle 改动仍必跑
`cargo test --lib "tests_group_chat"`;live 三链验证范式:SIGKILL(非
daemon.sh stop——那是 SIGTERM graceful → cancelled)→ 重启见 interrupted →
resume → group_chat_end。
