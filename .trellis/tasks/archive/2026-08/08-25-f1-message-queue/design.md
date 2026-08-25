# Design — F1 消息队列(用户连发档 MVP)

> 对应 prd.md R1-R7。核心思路:**所有发送一律入队,驱动器负责消费**——把"忙时排队"与"空闲即跑"统一为同一条入队路径,后端单点决策,天然满足 Tauri IPC 与 daemon REST 双入口一致(AC7)。

## 1. 架构与边界

```
send()(前端,解锁后的编辑器)
  └─ invoke("chat") / POST /api/v1/agent/chat
       └─ chat_inner 路由临界区(见 §2):
            ├─ 入队(所有发送一律入队,统一数据路径)
            ├─ 忙(session_active_request 命中)→ 返回 {queued:true, position}(本次 RPC 无流)
            └─ 闲 → 同临界区内注册 rid + spawn 驱动器,响应形状与现状一致(unit)
       └─ 驱动器任务(原 tokio::spawn 体改造):
            loop {
              run_chat_loop / run_group_chat_loop   // 群聊仍走原入口,不入队(R5)
              if cancelled        → 清空队列, break   // Stop/删除/替换,统一规则
              if had_error        → 队列保留, break   // 非 user 过错,不丢输入
              if 续轮数触顶(50)   → 队列保留, break   // 同错误终止语义
              drained = drain(queue)
              if drained empty    → break(emit Done)
              emit ChatEvent::TurnContinuation        // 前端续轮渲染边界,见 §3
              persist(drained) → 作为下一轮初始 user 输入再进 run_chat_loop
            }
            // 退出协议(反搁浅,收进路由锁):拿路由锁 → 队列空才注销
            // session_active_request 并退出;非空则继续循环。消灭
            // 「break 注销后、新入队无消费者」窗口。
```

- **队列本体**:`AppState.session_message_queues: Arc<Mutex<HashMap<session_id, VecDeque<QueuedMessage>>>>`。`QueuedMessage { id: uuid, text, attachments, enqueued_at, priority: u8 }`,`priority` 仅占位(R6,MVP 纯 FIFO,字段不参与调度——注释显式标注防误用),MVP 调度纯 FIFO。`id` 用 uuid(非位置)——位置随增删漂移,撤销/修改(R8)按 id 寻址。
- **上限**:`SESSION_QUEUE_MAX = 20`,超限返回错误(前端 toast),有界防打爆(AC6)。
- **生命周期**:入队仅内存;注入时才持久化(经 init 现有 persist + seq 游标)。崩溃/重启丢失 = 与 composer 未发送文本同等风险姿态(PRD 已决)。

## 2. 入队/驱动判定(竞态收口)

**统一数据路径**(PRD D7 的落地口径,§4 错误保留承诺依赖它):所有发送一律先入队;「查 `session_active_request` + 入队 + 注册/spawn」在**同一把路由锁临界区**内完成:

- 忙:仅入队,立即返回 `{queued:true, position}`(**本次 RPC 无流**,事件仍按 session 广播给所有端)。
- 闲:注册 rid(现状逻辑)+ spawn 驱动器;响应形状与现状一致(unit),流照常走事件通道。
- 滞留项顺序由统一路径天然保证:错误终止后保留的队列项排在前,下次发送的 新消息排在队尾,驱动器 drain 全队 FIFO 注入。
- defense-in-depth 的 `cancel_inflight_for_session`(chat.rs:348-377)保留:漏进来的第二请求仍取消替换,**并清空队列**(与 Stop 同语义)。

**锁纪律**:路由临界区内**不得 await**(`await_inflight_exit` 留在区外);驱动器退出的「查队列 + 注销」同样走这把锁(见 §1 退出协议)。

## 3. 续轮协议(wire + 前端渲染)

复用群聊"单个 rid 保活跨多个内层 turn"模式(`groupChat: true` + stop_reason 白名单先例):

- 驱动器**只在真正结束时 emit 一次 `Done`**;内层 turn 之间只发 `TurnComplete(seq)` 等常规事件。经典聊天前端 finalize 只认 Done → 请求生命周期管理零改动。
- **前端续轮渲染需要显式适配(评审 P1-2 修正,原"中间态零改动"断言作废)**:`streamEvents.ts:66-67` 在尾部消息非 assistant 时静默丢弃全部事件——排队 user 占位在尾部时,续轮 Start/Delta 会全部被丢;`:93` 的"新 start 推新 assistant 占位"被 `req.groupChat` 门控。且**不能**直接泛化该门控:`start` 是 run 内每次 LLM 调用的边界(tool_use 后的下一轮也发),经典多工具轮会因此被错误拆泡。
- 正确边界 = 新增 **`ChatEvent::TurnContinuation`**:驱动器在每次续轮内层 run 开始前 emit(与群聊 `Speaker` 同位置同角色)。前端 handler 收到后:① 把尾部排队 user 占位物化为普通气泡(去 queued 徽标);② push 新 assistant 占位;③ seal 上一 turn thinking(复用 start 分支的 `sealActiveThinking`/`flushPendingTimelineText` 模式)。该事件先于新 run 的任何 delta 到达(sink 保序),`streamEvents.ts:66-67` 的 assistant 尾部不变量自动恢复,**无需触碰该守卫**。
- 注入请求内位置:messages 尾部 APPEND(每条独立 `role:user`,与 drive.rs:791-853 通知循环同构),cache 断点不变量自动满足(AC2)。
- 入队确认走 RPC 返回值 `{queued:true, position}`;排队视图经 `list_queued_messages` 水合(design §6/§7)。

## 4. 取消与清理矩阵

| 触发 | 当前轮 | 队列 | 用户反馈 |
|---|---|---|---|
| Stop 按钮(cancel_chat) | 取消 | **清空**(PRD 已决) | toast「已丢弃 N 条」,N = `cancel_chat` 返回的清空数(P2-2) |
| defense-in-depth 二请求替换 | 取消 | 清空 | 同上 |
| provider 错误终止 | 结束 | **保留**,不续轮;下次发送统一入队自然一起注入(顺序由统一数据路径保证) | error Done 后前端按本地队列计数 toast「N 条排队消息保留,下次发送时注入」 |
| 续轮上限触顶(50) | 结束(Done) | **保留**(同错误终止语义) | 同上 |
| edit / resend / retry(先 cancel 再发) | 取消 | 清空(cancel 链路顺带)| **复用「N 条被丢弃」toast**(P2-3:原设计静默清队,用户无感知,不可接受) |
| delete_session / clear_messages | 取消(现有) | 随 session 删除 | 无需 |
| daemon graceful shutdown | drain(现有) | 内存队列随进程消失(已接受) | 无需 |

## 5. 各入口行为

| 入口 | 行为 |
|---|---|
| 经典聊天 send(IPC + REST/PWA 同路径) | 解锁编辑器;忙→入队,闲→直跑 |
| group_chat | **零改动**:编辑器保持现锁定行为,cancel+resend 抢占保持(AC4 回归锁) |
| D3 编辑/重发、重试 | 保持"先 cancel 再发"(cancel 已清队列),随后走正常 send |
| handoff | 忙时拒绝文案不变 |
| @@强制派发 | **经典 session 流式中输入 @@ 前缀 → 前端直接拒绝并 toast**(P2-4:派发语义与延迟注入组合不清晰,MVP 最简;续轮注入因此永不携带 forcedDispatch,实现期验证项消解) |
| B3 /command、B2 @文件 | 展开后即普通 send,自动获得排队能力 |

## 6. 前端设计

- **解锁**:`readOnly` compartment 与 sendDisabled 的判定从 `sending` 改为 `sending && isGroupChat`(群聊保持锁;AC1 要求流式中可成功发送,故发送键必须一并解锁)。
- **Esc 规则(P2-5 拍板)**:经典 session 流式中,编辑器**为空**时 Esc 仍 = Stop(清队列);**非空**时 Esc 不触发 Stop(防误触把打好的输入连队清掉),要停就点 Stop 按钮。群聊保持现行为。
- **排队视图**:chat store 新增 per-session `queuedMessages` Map(key = 后端 uuid,乐观占位 + queued 徽标 + position);RPC 返回 `{queued:true}` 时标记对应占位;消息真正持久化(TurnComplete 后 reload/对齐)时转普通态。
- **单条撤销/修改(R8)**:排队气泡 hover 出两个操作——撤销(×)= `remove_queued_message` + 移除占位;修改 = `recall_queued_message`(后端移除并返回原文)+ 回填 composer(复用现有输入框,零新建 inline 编辑 UI)。
- **视图水合**:新增只读 IPC `list_queued_messages(session_id)`;切 session / 页面刷新后从后端 SoT 重建排队视图(消除"后端持有但看不见"缺口)。附带收益:PWA 第二端水合同一列表,跨设备可见性几乎免费获得。- **竞态窗口**:drain 发生到 persist 完成之间,revoke/recall 按 id 找不到条目 → 返回 not-found 错误,前端 toast「已开始处理,无法撤销」。
- **toast**:Stop 后按被清数量提示。

## 7. IPC 面(R8 新增)

| 命令(Tauri + daemon 路由 1:1) | 语义 |
|---|---|
| `list_queued_messages(session_id)` | 只读,返回 `[{id, text, position, enqueued_at}]` |
| `remove_queued_message(session_id, id)` | 删除单条;not-found → Err |
| `recall_queued_message(session_id, id)` | 删除单条并返回原文(供回填 composer);not-found → Err |
| `cancel_chat`(现有命令返回值扩展) | unit → `{cleared_queued_count}`(Stop toast 的 N 来源,P2-2);edit/resend/retry 走同一 cancel 链路同享该值 |

权限:均为用户侧会话内操作,不涉 agent 工具面,不走 ⑨ 决策层(同 list_sessions 类)。

## 8. 兼容与回滚

- 空闲路径回归锁(AC3):闲且队列为空时,可观测行为与现状一致(既有 harness 快照不改)。**机制差异(评审 Round 2 修正,原"逐字节对齐"说法作废)**:统一路径下闲时发送同样入队并由驱动器消费,LLM 请求历史从客户端 `messages` 改为 DB reload(群聊 D-B 同构,DB 是唯一事实源)——语义等价,非逐字节;若未来出现"仅存于客户端 history、未落库"的请求内容会被 reload 丢弃(当前无此形态,记录在案)。
- 开关:`message_queue_enabled`(缺省 on,fail-open 同 memory_digest 先例);off 时退化为现状(忙时前端恢复锁定?否——开关只关后端入队,前端解锁跟随开关下发,实现期定最简方案)。
- **返回形状扩展(向后兼容字段叠加,P2-1;Round 2 补 `id`)**:`chat` 两入口现状返回 unit(Tauri `Result<(),_>` / REST `Json(())`);忙时改返 `{queued:true, id, position}`——`id` 是队列项 uuid(R8 撤销/退回的稳定寻址键:位置随增删漂移,前端占位必须按 id 寻址),闲时保持 unit 不变——调用方按返回值分支。`httpTransport.invoke("chat")` 需把返回值透传给调用方(transport 层一行检查)。新增 `ChatEvent::TurnContinuation` 为 additive 变体,无 rename。
- 回滚单元 = 整个 PR:队列结构独立成模块(`agent/message_queue.rs`),chat_inner 改动集中在一处分流点,revert 单 commit 即可。
- 无 DB migration、无 breaking wire rename。

## 9. 关键 trade-offs 记录

- **统一入队 vs 忙时特判**:选统一入队(一条路径,REST 天然一致),代价是空闲路径也过一次锁+队列入出(纳秒级,可忽略)。
- **错误保留队列 vs 一律清空**:选保留(非 user 过错不丢输入),代价是"错误后队列滞留";以 error Done 后的保留计数 toast + 下次发送自然消化兜底(§4)。
- **跨设备可见性靠水合而非事件广播**:入队/撤销不广播 ChatEvent,他端经 `list_queued_messages` 水合看到最终态;省一条 wire 变体,代价是他端视图非实时(下次切 session 才刷新),MVP 接受。
