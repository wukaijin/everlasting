# F1 消息队列(输入排队/优先级/批量注入)— 用户连发档(MVP)

> 状态:规划完成,待批准。范围 A + Stop 清空队列 + 单条撤销/修改已拍板;两份评审(review-glm / review-d4f)已甄别采纳(glm 两项 P1 实锤并修正,d4f D-2 系误读),2026-08-25。
> ROADMAP 源条目:[docs/ROADMAP.md §2 第三档 F1](../../../docs/ROADMAP.md);技术设计见 [design.md](./design.md),执行计划见 [implement.md](./implement.md)。

## Goal

当前 turn 串行且流式期间前端编辑器被整体置只读——用户连打字都不行,更别说排队。本档补**输入侧**消息队列的最小闭环:turn 进行中可继续打字、发送、撤销、修改,消息进后端 per-session 队列,当前轮结束后批量注入为下一个 turn。为 F2 定时任务/F6 异步任务预留统一入口接口,但不实现生产者。

## Background

### 现状与 ROADMAP 原述的偏差(2026-08-25 实测)

1. **前端不止"丢弃",是锁死**:流式期间 CodeMirror 编辑器置 `EditorState.readOnly`(`app/src/utils/chatInputCodeMirror.ts:796,837`),发送键 disabled(`ChatInput.vue:426-431`)。
2. **后端从不阻塞也不排队,而是取消替换**:`chat_inner` 注册新请求前先 `cancel_inflight_for_session` + `await_inflight_exit` 杀掉同 session 旧轮(`app/src-tauri/src/agent/chat.rs:348-377`,defense-in-depth,正常路径靠前端 guard 挡住)。
3. **群聊已是抢占语义**:group_chat 流式中发新消息 → 先 `cancel()` 再发(`chatSendActions.ts:229-232`)。

### 约束

- DESIGN §3.2 硬约束:触发源必须本地;主动权在本地用户。
- cache 断点:注入必须 APPEND 到 messages 尾部,不得前插(`.trellis/spec/backend/agent-loop-architecture.md:9-37`)。
- 队列持久性遵循现状风险姿态:未发送的输入本就不落库(composer 文本崩溃即失),in-memory 队列等价,不新增持久化语义。
- `run_chat_loop` 新参数追加尾部(signature spec 惯例)。

## Key Decisions(2026-08-25 拍板)

| # | 决策 | 理由 |
|---|---|---|
| D1 | 范围 = A 用户连发档 | 最快解最高频痛点;后端 AppState 队列天然为 F2/F6 留接口 |
| D2 | 打断语义:Stop 为唯一打断路径;群聊保持 cancel+resend 抢占不变 | 排队是主路径,打断显式化 |
| D3 | Stop = 杀当前轮 + **清空队列**(toast 告知 N 条被丢弃) | Stop 的用户意图就是"停",自动续轮违背预期 |
| D4 | 单条**撤销**(删除)+ **修改**(退回输入框回填 composer)进 MVP | 内存态未落库所以便宜;无单条粒度则"Stop 全清"成唯一错误恢复手段 |
| D5 | 注入形态 = 每条独立 user message 逐条 APPEND | 与 drive.rs:791-853 后台通知循环同构,wire 层零改动 |
| D6 | 队列 in-memory,注入时才持久化 | 与 composer 未发送文本同等风险姿态 |
| D7 | 所有发送一律入队,驱动器单点消费(忙排队/闲即跑统一一条数据路径) | IPC/REST 双入口天然一致,路由竞态收进单一锁临界区;错误滞留项 + 新消息的 FIFO 顺序由统一路径保证 |
| D8 | @@ 强制派发前缀在经典 session 流式中直接拒绝(toast) | 派发语义与延迟注入组合不清晰;续轮永不携带 forcedDispatch |
| D9 | Esc=Stop 仅当编辑器为空;非空不触发(点 Stop 按钮) | 解锁后防"打好的输入连队被误清" |
| D10 | 新增 `ChatEvent::TurnContinuation` 作续轮渲染边界 | `start` 是 run 内 LLM 调用边界不能复用(泛化会拆散多工具轮);事件先于 delta 恢复 assistant 尾部不变量 |

## Requirements(R1-R8)

- **R1 编辑器解锁**:经典聊天 session 流式期间解除输入框只读态,允许继续输入与发送;群聊 session 保持现锁定行为。
- **R2 后端 per-session 队列**:`chat` 入口所有发送一律入队(AppState 内存 VecDeque,FIFO,uuid 寻址);忙时返回 `{queued:true, position}`(闲时响应形状不变),上限 20,超限拒绝并提示;@@ 前缀流式中直接拒绝(D8)。
- **R3 turn 边界批量注入续轮**:正常结束且队列非空 → 同一驱动器内自动开下一轮,drain 全部按序批量注入;只在真结束 emit 一次 Done(复用群聊"单 rid 保活跨内层 turn"模式)。取消→清空队列;错误终止→队列保留不续轮。
- **R4 队列可见性**:排队中消息前端有明确视觉态(queued 徽标/位置),视图经 `list_queued_messages` 从后端 SoT 水合(切 session/刷新/PWA 第二端一致);续轮开始时经 `ChatEvent::TurnContinuation` 物化占位并推新 assistant 气泡,流式全程可见(D10)。
- **R5 打断语义不变**:Stop 仍为唯一打断路径(D3);群聊零改动;handoff 忙时拒绝保持;defense-in-depth 二请求替换语义保持(替换时清队列)。
- **R6 优先级字段预留**:队列项含 `priority` 字段,MVP 行为纯 FIFO,不做分档调度。
- **R7 清空队列均有反馈**:Stop / defense-in-depth 替换 / edit·resend·retry(先 cancel 链路)触发的队列清空,一律 toast「已丢弃 N 条」,N = `cancel_chat` 返回的 `cleared_queued_count`(后端 SoT);错误终止与续轮上限触顶则保留队列并 toast 保留计数。
- **R8 单条撤销/修改**:撤销 = `remove_queued_message`(id 寻址)移除单条;修改 = `recall_queued_message` 移除并返回原文回填 composer(复用现有输入框);drain 窗口 not-found → 错误提示「已开始处理」。不做拖动调序(FIFO 契约)。

## Acceptance Criteria

> 回填说明(2026-08-25 归档时):AC1-AC9 全部验证通过 —— 单测/集成/vitest(vitest 1195 含 6 新增,cargo 1971)+ live 真机实测(长 turn 连发含撤销/Stop + turn-smoke 双轮 cache 对照 + curl REST 排队分支)。覆盖细节见 implement.md #9 覆盖实况与 review-glm 核查表。

- [x] AC1 流式期间可打字并成功发送 ≥2 条消息,均显示排队态;当前轮结束后自动续轮按序注入,聊天历史逐条呈现、内容与输入一致。
- [x] AC2 注入后的请求中排队消息位于 messages 尾部(APPEND),双轮 cache 率不劣化(turn-smoke 对照)。
- [x] AC3 空闲且队列为空的 session 发送行为与现状一致(回归锁)。
- [x] AC4 群聊 session 发送仍是抢占语义(回归锁);handoff 忙时拒绝文案不变。
- [x] AC5 流式中 Stop → 当前轮取消 + 队列清空 + toast 数量正确(N = cancel 返回值)+ 占位回收,后续无自动续轮;edit/resend/retry 触发的清空同样有 toast。
- [x] AC6 队列上限有界(20),第 21 条被拒并有明确反馈。
- [x] AC7 daemon REST 路径(httpTransport/PWA)同样获得排队能力(Tauri IPC 与 HTTP 双入口一致性)。
- [x] AC8 排队中单条撤销后,该消息不再进入注入;其余消息顺序不受影响。
- [x] AC9 排队中单条修改(退回输入框)后,原队列项消失、composer 载入原文;not-found 场景有「已开始处理」提示。

## Out of Scope(follow-up 档)

- 优先级分档调度与高优抢占(user > system 两档)→ B 档/F2 前置档。
- daemon 层统一入口服务化(chat 路由改 enqueue + worker 消费)、F2 定时任务生产者、F6 异步任务接入 → C 档。
- 群聊编排收编进统一队列(群聊保持 cancel+resend 抢占现状)。
- 排队消息持久化(崩溃/重启丢失,D6 已接受)与拖动调序。

## Technical Notes(可复用基建)

| 块 | 位置 | 用途 |
|---|---|---|
| `AppState` 三 map(cancellations/session_active_request/inflight_exits) | `state.rs:105-128` | 忙判定(handoff 先例 `commands/sessions.rs:1370`) |
| per-turn 注入 seam(request clone 上 APPEND) | `chat_loop/drive.rs:788-850` | 注入点;cache 断点契约先例 tool-contract/06-background-shell.md:144-161 |
| 多条连续 user 消息先例 | `drive.rs:791-853` | 注入形态直接沿用,wire 层(to_wire.rs)直通零改动 |
| `drain_notifications` 破坏性出队模板 | `background_shell/mod.rs:292`, `in_memory.rs:357` | 队列 API 模板 |
| 群聊编排器外循环 + 非 terminal `Done.stop_reason` 协议(`groupChat: true` 保活先例) | `group_chat_loop.rs:210-636`, `streamController.ts:994` | 续轮驱动器骨架 |
| seq 占位乐观 UI / D-D 入口去重 guard | `chatSendActions.ts:296-413`, `chat_loop/init.rs:727-770` | 排队占位渲染 / persist 时序防重参考 |

全部外部入口清单(普通 send / REST+PWA / @@派发 / D3 编辑重发 / 重试 / /command / @文件 / 群聊 / handoff / 后台 shell 通知)及各自行为矩阵见 [design.md §5](./design.md)。
