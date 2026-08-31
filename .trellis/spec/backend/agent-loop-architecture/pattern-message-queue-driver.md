# Pattern: 消息队列驱动器(输入侧排队 + 续轮批量注入,F1-A 2026-08-25)

## Problem

经典 session turn 串行,流式期间前端编辑器整体只读——用户连打字都不行,更别说连发。忙时第二请求只能拒绝或丢弃。

## Solution: 所有发送一律入队,驱动器消费

单点决策在 `chat_inner` 路由临界区(`agent/chat.rs`),消费在驱动器(spawn 体)。忙/闲统一为同一条入队路径,IPC 与 REST 双入口天然一致。

```
chat_inner 路由临界区(单一 Mutex,锁序 queues → active):
  入队(一律)→ 查 session_active_request
    忙 → 返回 {queued:true, id, position}(本次 RPC 无流,事件仍按 session 广播)
    闲 → 注册 rid + spawn 驱动器,响应保持 unit 形状

驱动器 loop(agent/message_queue.rs + chat.rs spawn 体):
  run_chat_loop
  → cancelled      → 清空队列, break
  → had_error      → 队列保留, break   // 非 user 过错不丢输入,下次发送 FIFO 一起注入
  → 续轮上限(50)   → 队列保留, break
  → drain 非空     → emit TurnContinuation → persist(drained) → 作为下一轮初始 user 输入再进 run
                    // persist 分工(RULE-QUEUE-001 根治,08-29):非尾条由
                    // init.rs persist 循环补写(带 origin/附件的行随行落
                    // metadata 信封),尾条由既有尾条 persist 点写;seq 同
                    // 一 next_seq 起算连续自增。多 drain 全部落库,reload 完整。
  → drain 空       → break(emit Done)
  退出协议:拿路由锁 → 队列空才注销 session_active_request;非空继续循环(反搁浅)
```

## 硬约束(违反即回归)

1. **路由临界区内不得 await**(锁序 queues → active 全仓文档化;`await_inflight_exit` 留在区外)。驱动器退出的「查队列 + 注销」走同一把锁——消灭"break 注销后、新入队无消费者"窗口。
2. **DriverSink 只在真结束 emit 一次 Done**:单 rid 跨内层轮保活(群聊 stop_reason 白名单的经典聊天泛化),内层 turn 只发常规事件。**`emit_chat_event` 必须按 `forward` 返回值转发;Error 分支不得自转发**(P0 实证:双发导致前端二次 finalize)。**非 chat 通道全数透传**:`tool_question` / `mode_change_request` / `task_state_transition` 在 trait 上是默认静默 no-op,装饰器漏写不报错、只吞事件——2026-08-31 实证三个方法缺失致阻塞交互卡片实时不渲染(刷新靠 `get_pending_interaction` 兜底才恢复)。新增 `ChatEventSink` 通道时,AppHandleSink / HttpSseSink / DriverSink / MockEmitter / RecordingSink 五个实现点必须同步,并在 `driver_sink_forwards_blocking_interaction_channels` 补断言。
3. **`TurnContinuation` 是唯一的续轮渲染边界,不得泛化 `start` 的 `groupChat` 门控**:`start` 是 run 内每次 LLM 调用的边界(tool_use 后下一轮也发),泛化会把经典多工具轮错误拆泡。handler 三步:物化尾部排队占位 → push 新 assistant 占位 → seal 上一 turn thinking。
4. **队列项按 uuid 寻址,永不按 position**:位置随增删漂移(撤销后右移全部错位——曾致撤错条/静默失效);`ChatAcceptance::Queued` 必须带 `id`,前端占位按 id 直达后端 IPC。
5. **注入形态:每条独立 `role:user` APPEND 到 messages 尾部**(与 drive.rs 通知循环同构),cache 断点不变量自动满足;排队项未落过库,不触发 D-D guard;attachments 落盘时序从 turn 启动提前到入队时。
6. **break 矩阵不对称**:user 主动终止(Stop/edit/resend/retry/defense-in-depth 替换)→ 清空 + toast 计数(cancel 返回 `clearedQueued`);provider 错误/续轮触顶 → 保留,下次发送自然消化。
7. **guard 双抑制**:驱动器 spawn 前置 `skip_cancellations`(slot/token 生命周期自持,不被外层 guard 提前清理)。
8. **跨端可见性走水合不走广播**:入队/撤销不广播 ChatEvent,他端经 `list_queued_messages` 重建(省 wire 变体,代价非实时,MVP 接受)。

## Wrong vs Correct

```ts
// Wrong — 泛化 start 门控当续轮边界:经典多工具轮(tool_use 后的下一轮也发 start)被错误拆泡
if (payload.type === 'start') pushAssistantPlaceholder()

// Correct — 独立续轮事件,先于新 run 任何 delta 到达(sink 保序),
// 尾部 assistant 不变量自动恢复,streamEvents 的 :66-67 守卫零触碰
case 'turn_continuation':
  materializeQueuedPlaceholders()  // 排队占位 → 普通气泡
  pushAssistantPlaceholder()
  sealActiveThinking(); flushPendingTimelineText()
```

## 闲路径口径(评审 Round 2 修正)

统一路径下闲时发送同样入队由驱动器消费,LLM 请求历史从客户端 `messages` 改为 **DB reload**(群聊 D-B 同构,DB 是唯一事实源)——语义等价,**非**逐字节。若未来出现"仅存于客户端 history、未落库"的请求内容会被 reload 丢弃(当前无此形态,记录在案)。

## Tests

- `agent/message_queue.rs` 单测:FIFO 序 / 上限 20 拒 / remove(id) / recall(id) / not-found
- driver 集成测试:忙时入队 → 续轮按序注入逐条落库 / `TurnContinuation` 事件序 + 内层 Delta 到达 sink(P0 回归锁)/ Stop 清队返回计数 / 错误终止队列保留 / **多 drain 全落库**(RULE-QUEUE-001:`multi_drain_persists_all_drained_user_rows_rule_queue_001` + 全 manual 对照 `multi_drain_all_manual_persists_every_row_without_metadata`)
- vitest `messageQueueStream.test.ts`:续轮物化 + 新 assistant 占位 + delta 不被尾部守卫丢弃;`dropQueuedPlaceholder` 按 id 删占位重排位次;水合物化去重
- live 冒烟(真机):长 turn 连发 3 条 + 单条撤销 + Stop toast + curl REST 排队分支
