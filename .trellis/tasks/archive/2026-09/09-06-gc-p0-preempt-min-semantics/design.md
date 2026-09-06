# design.md — 群聊 P0 打断最小语义

## 1. 架构总览

三个机制,一个注册表:

```text
AppState
└─ group_chat_controls: Arc<Mutex<HashMap<session_id, GroupChatControl>>>   [新增]
   GroupChatControl = Arc<Mutex<GroupChatControlInner>>
   GroupChatControlInner {
     pending_injects: Vec<ChatMessage>,   // 注入缓冲(编排器独占消费)
     preempt_requested: bool,             // preempt 信号(Q1:轮边界语义)
   }
```

- **注册**:chat_inner 群聊分支 spawn 前注册(同 `cancellations` 注册时序,GC1:
  一次注册覆盖整场编排);**清理**:编排器全退出路径经 Drop guard(同
  CancellationGuard 先例)。注册表条目存在 ⇔ 场次在跑(busy 判定仍以
  `session_active_request` 为准,注册表只作控制通道)。
- **锁纪律**:既有全仓固定序 `message_queues → session_active_request` 之后,
  `group_chat_controls` 永远最后获取(文档写进 state.rs 字段注释)。

## 2. R1 注入通道(busy 期间用户消息非破坏)

### 2.1 路由(chat.rs 'routing 临界区改造)

群聊分支从「直接 break 'routing(legacy)」改为:

```text
group_chat_ctx.is_some():
  busy = session_active_request.contains(session_id)
  if busy && group_chat_controls 有该 session 条目:
      push 消息进 control.pending_injects
      return ChatAcceptance::Injected            [新枚举变体,无流无 rid]
  else: fall through 既有 legacy 路径(不 busy = 正常开新场,逐字节不变;
        busy 但注册表缺条目 = 防御分支,warn + fall through legacy,现状行为)
```

- 不 busy 时不注册 controls、不入缓冲——正常发起新场,零行为变化(AC4)。
- `ChatAcceptance` 增 `Injected`(wire:daemon routes/agent.rs 序列化与
  `Queued` 同形处理;Tauri IPC 同步跟进)。

### 2.2 seq 纪律 = 编排器独占落库(本设计的关键取舍)

**否决「命令层直插 DB」**:compaction_summary 先例(`session_crud.rs:785+`)实证
活跃 loop 持内存 seq 游标,独立 `MAX(seq)+1` 直插轻则自己撞 `(session_id, seq)`
主键(RULE-PERSIST-001 裸 INSERT),重则占走在途 speaker 下一次 persist 要用的
seq、**反过来打断在途发言轮**。participant burst(max_turns=20)窗口分钟级,
撞上是常态不是边角。

**采纳「缓冲 + 编排器边界落库」**:控制通道只进内存缓冲;编排器在**轮头**
(上一 inner loop 已退出、下一未进入,无活跃游标)drain 缓冲逐条落库,
`MAX(seq)+1` 此时安全。属性与 F1 队列一致:daemon 崩溃丢未消费缓冲
(可接受,F1 先例);讨论终态前退出路径 flush 残余(best-effort)。

### 2.3 schema 区分(R3,决议)

持久化形状(user 行,`speaker=NULL`——注入者是人,role 归属不变):

- **metadata.kind = "user_inject"**:API/检索侧机器可辨(仿 `worktree_event` /
  `compaction_summary` 先例,新 db helper `insert_user_inject`);
- **text 列落 `[用户插入] ` 前缀**:LLM 侧确定性可辨(role_history 对 user 行
  原样透传 `group_chat_prompts.rs:142`,metadata 不进 ChatMessage,文本标记是
  唯一能到达 moderator 上下文的通道);注入行只经 DB reload 进入视野、永不进
  chat 载荷,前缀不会污染 rehydrate/edit 路径;
- moderator prompt 增一段:`[用户插入]` = 用户新指令,优先吸收、可调整议程,
  但 end_discussion 权限不因此扩大(prompt 结构变更 → 触发 turn-smoke,R5)。

「文本标记 + metadata 双轨」即 P0 schema 决议,落 `.trellis/spec`(M3
`inject_message` 参数形状继承:topic 文本 + 可选 provenance)。

## 3. R2 preempt 信号 + 收束轮

### 3.1 信号与命令

- `GroupChatControlInner.preempt_requested`(轮边界语义,Q1 决议:等在途
  speaker 跑完,不立杀流——立杀是 M3 的 per-speaker child token 增强)。
- 新命令 `preempt_group_chat(session_id)`(commands + daemon route 五处接线):
  注册表查不到 → 明确报错「无进行中的讨论」;查到 → 置位返回 Ok。
  与 M3 `interrupt_discussion` 1:1。
- Stop 按钮 / cancel_chat 语义**不动**(硬取消止损保留,stop_reason=cancelled)。

### 3.2 编排器轮头(收束状态机)

```text
round head:
  drain pending_injects → 落库(此时无活跃 seq 游标,安全)
  if preempt_requested:
      收束轮(moderator turn,prompt 追加「用户已打断,立即 end_discussion
      总结当前进展,勿 nominate」):
        end_discussion 命中 → stop_reason = "preempted" + discussion_summary ✅
        未命中/报错 → 重试 1 次 → 仍失败 → 强制立断兜底:
            stop_reason = "preempted"(仍可区分),summary 缺,转录如实呈现
  else: 正常轮次(moderator reload 天然看到已落库的注入行)
```

- 新常量 `STOP_REASON_PREEMPTED = "preempted"`;落库走 GC2 既有通道;
  前端 finalize 白名单 + notice 加一档(终态异常结束类,同 error 位);DAEMON-API
  §4 文档补值(M1/M2 驱动对任意终态值天然兼容,零改动)。
- GC5 熔断与收束轮交互:收束轮自身 ERROR 不进熔断计数(它不是内容轮),
  直接走兜底立断。
- preempt 与 cancel 竞态:cancel 先到 → 整场死(cancelled 既有语义);
  preempt 先到 + 收束中 cancel → 收束轮被杀,stop_reason=cancelled(止损优先,
  符合「Stop 是最后手段」语义)。

## 4. 兼容性 / 迁移

- 经典聊(非群聊)路径零改动:路由分支仅 `group_chat_ctx.is_some()` 进入,
  3a 防御性取消对经典聊逐字节保留(AC4)。
- 群聊不 busy:行为逐字节不变(controls 注册发生在 spawn 侧,不碰发起点)。
- 无 DB schema 变更(messages.metadata 已有;stop_reason 复用 GC2 列)。
- 回滚:单 commit 族 revert 即可,无迁移残留。

## 5. 权衡记录

| 决策 | 取 | 舍 | 理由 |
|---|---|---|---|
| 注入走缓冲而非直插 | seq 纪律安全,不打断在途轮 | daemon 崩溃丢未消费注入 | F1 队列同属性;直插会主动毁场(§2.2) |
| 文本前缀 + metadata 双轨 | LLM 确定性可见 + API 机器可辨 | text 列带标记(检索原始文本含前缀) | metadata 到不了 LLM(ChatMessage 无此字段);前缀被 DB-reload-only 路径隔离 |
| preempt 轮边界检测 | 零 per-speaker cancel 改造 | 打断延迟 = 在途 burst 剩余时长 | Q1 决议;立杀留 M3 |
| 收束轮复用 end_discussion | summary 不丢零新机制 | 收束多一次 LLM 调用 + 秒级延迟 | M3 AC① 直接满足;兜底立断保底 |
