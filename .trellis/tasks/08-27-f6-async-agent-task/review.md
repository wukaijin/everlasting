# F6 异步 agent 任务 — 评审记录

> 评审对象:task `08-27-f6-async-agent-task`(planning 阶段)的 `prd.md` / `design.md` / `implement.md`。
> 评审方式:design/implement 引用的取证锚点逐一与真实代码(2026-08-27 main `7a30c18`)核对。
> 结论:**方案总体成立,核心洞察(D2「执行已存在,缺的是编排面」)与代码事实完全吻合,可进入实施;1 处必须修的执行序缺陷(P1,PR3 并发闸)+ 1 处需补实现方案的边角(P2,关闭确认的 Tauri 窗口句柄)+ 3 处文档/方案细节同步(P3)**。

## 结论概要

三件套质量高,取证锚点绝大多数精确:busy 权威表无出口、SSE 零订阅者静默丢弃、客户端断开非 cancel 源、recover_interrupted_messages 跨重启终态、`chat_inner` 统一入口、buildPendingNotification 模式、streamEvents 跨客户端认领——全部核实成立。planning 状态合理(task.json 仍 `planning`,check/implement.jsonl 未填)。核心风险集中在 PR3 的并发闸,**在 spawn 闭包开头 acquire 会与信号量队列语义发生一次顺序反转,需把许可获取前移到路由临界区内**(P1),这是当前三件套里唯一影响 AC3 成败的设计缺口。其余为细节。

---

## P1(必须修)PR3 并发闸的许可获取点:闭包头 acquire 会把「排队等待许可」变成「并行排队 + 依次插队」,且 Start 迟到语义反向

design §R3 的伪代码在 spawn 闭包**第一行** `acquire_owned`。真实代码中 spawn 闭包开头(chat.rs:549 之后)先做 ChatLoopDeps 解构、再三分支分发,每个分支的首个动作是**注册**到 `session_active_request` / `cancellations` / `inflight_exits`(drive 路径在路由临界区 chat.rs:350-353 已注册;legacy 路径在 chat.rs:468 起注册)。`select!` 只对 acquire 与 token.cancelled 做竞态,不阻止注册发生。后果:

1. **busy 表抢先于许可**:N+1 个 send 全部在闭包内排到 acquire,但 `session_active_request` 已含全部 N+1 个 session → `SessionSummary.busy` 即刻全亮、`streamingSessionIds` 即刻全红,与「第 3 个 Start 延迟」的 AC3 观察(只红前 2 个)矛盾;更糟的是忙表占位与「全局闸」想表达的「同时只有 4 个在跑」语义错位,关闭确认 R4 的 busy 计数也会把排队中的会话误判为在跑(用户看到「关闭将终止 N 个在跑会话」,实际是排队)。
2. **排队期间的取消事件流错位**:select! 早退臂「补发 cancelled Done + 清 slot 注册」在闭包内执行,但此时会话已注册 → 需要清理的注册清单与 acquire 成功路径重叠;实现稍漏即 rid 泄漏(design 已自列该边角,但把 root cause 放进了闭包内,清不干净)。
3. **F1-A 队列驱动器语义被破坏**:design 要求「队列驱动器全程持 1 个许可」。驱动器是在路由临界区**内**被 claim 并随请求 spawn 的——若驱动器自身也在闭包内 acquire,队列里第 2+ 条消息会各自触发新驱动器 spawn,每个驱动器各自 acquire,**排队语义从「session 粒度」退化成「消息粒度」**,与 R3「第 N+1 个不同 session 的 send 排队」的设定不符,还可能让同一 session 的驱动器并发(驱动器 A 持有许可期间 B 又 acquire 到新许可)。

**修正(关键一行)**:许可获取点**上移进路由临界区**(chat.rs:334 的 `'routing:` 块内),在 `session_active_request.insert` 之前 acquire——即「claim 即持闸」,与 F1-A「claim 即注册」同一位置。具体:

- 临界区头部:先 `let permit = acquire_owned().await`(信号量满时此处 await,天然阻塞后续注册),拿到许可后再走既有的 busy 检查 / 入队 / claim。排队中的 session **不注册** busy,行为回到「第 N+1 个 started 但未红」,Start 迟到语义正确。
- 信号量满 + 队列尾部校验:**不要**在校验 user 尾条前 acquire(会阻塞不该入队的请求),顺序应为:先做现有 F1-A 判断(是否需要走 queue/能否直接 spawn),**决定要 spawn 时**再 acquire。
- **必须处理「acquire 时 token 已被 cancel」**:select! 改为在临界区 acquire 点做(acquire vs `token.cancelled()`),cancel 时**跳过注册直接返回**,补发 cancelled 终态事件由既有 handle_cancel 路径承担——但注意:临界区内已 `return Err` / 已 insert 的分支要逐一核对,acquire 放最前则不会 insert。legacy 3a 路径(队列关闭/群聊/无尾条)同样在注册(chat.rs:468)前 acquire。
- **驱动器持闸**:驱动器自身不再各自 acquire;闸的持有者是「在跑的 loop 实例」,驱动器是同一实例。群聊/经典 loop 同理。
- 不变量:任何 `session_active_request.insert` 发生时,本请求必已持有许可;任何 drop 许可时,对应注册必已注销。实现时按这个不变量写注释并让集成测试断言「排队中会话不在 activeRequests 且 busy=false」。

implement.md PR3 的集成测试(permits=2 三连发)要加断言:第 3 个 send **未**出现在 activeRequests / busy 表(而不仅是 Start 事件延迟)。

## P2(需补实现方案)Tauri close-requested 的挂载点:design 用 `isTauriTransport()` 判断,但默认 transport 恒为 httpTransport,该判断拿不到「是否 Tauri 窗口」

设计 R4 伪代码用 `isTauriTransport()`(或 `isTauriWebview()`)作守卫。核实:**全仓默认 transport 恒为 httpTransport**(transport/index.ts resolveTransport),Tauri 壳内也是 sidecar + HTTP/SSE;`isTauriWebview`(transport/env.ts:31)判的是 `window.__TAURI_INTERNALS__`,Tauri 壳内恒 true、浏览器恒 false——**用它判断恰好成立**(Web/PWA 不注入该全局)。但两点需明确:

1. `close-requested` 只能由 Tauri 壳挂,浏览器无此事件——守卫建议直接改判 `isTauriWebview()`,语义是「运行在 Tauri webview 内」而非「transport 种类」;design 的 `isTauriTransport()` 会误导(它不存在,且默认 transport 判断不出壳形态)。
2. **挂载点的触发面**:design 写「AppShell 或 main.ts」。核实 AppShell.vue 是 Vue 组件 setup,`onCloseRequested` 的 handler 不受 Vue 生命周期约束,但获取 `getCurrentWindow()` 需 `@tauri-apps/api/window`;建议挂 main.ts 的 Tauri-init 分支(与 transport 选择同层),避免 AppShell 被复用/热重载时重复注册。补充实现细节:`e.preventDefault()` 后需持有 `Window` 实例,确认回调里 `destroy()`——设计已写,此处补一句「handler 内捕获 window 句柄,确认时复用,不要重新 getCurrentWindow」。

## P3-1(文档同步)PRD D5 与 design R4 的守卫判据措辞

PRD D5「确认弹窗只挂 Tauri 窗口关闭事件」、design R4「isTauriTransport()」、implement PR4「仅 Tauri transport」——三处措辞不一致,建议统一为「仅 Tauri webview(Thin/Full GUI 壳)」,并把「判断 = isTauriWebview(而非 transport 种类)」写进 design,避免实施时在 transport 层找守卫。

## P3-2(方案细节)stale-busy 兜底路径要写清「终结事件到达时可能没有 activeRequests 条目」

design R1 前端 stale-busy 消解写「终结事件 → adoptForeignRequest → finalize → 清 serverBusy」。核实 streamEvents.ts:157-159:若 `messagesBySession` 无该 session(冷启动未加载),终结事件仍会 `finalizeRequest`——但 **adoptForeignRequest 只在 resolveRequest 命中时发生**,冷启动端收到终结事件时该 request 从未在 activeRequests,`adoptForeignRequest` 不会创建条目(finalize 走的是已认领分支,streamEvents.ts:159 的 guard 分支)。所以「清 serverBusy」的挂点应在 **finalize 的公共出口**(isTerminal 三处:streamEvents.ts:159 / 202 / 606 / 702),而不是依赖 adoptForeignRequest 的认领。design 的「终结事件上也能认领」表述对 **done 携带 session_id 且消息已加载**的场景成立,对纯冷启动场景不成立——补一句「清理挂 finalize 公共出口,不依赖 adoptForeignRequest」。

## P3-3(文档)config 读出口写「按 config.rs 现状取最小改法」,但现状可核实

design R2 说「routes/config.rs 既有路由扩一键,或前端 configStore 启动拉取——按现状取最小改法」。核实现状:`daemon/routes/config.rs` 无通用的 get_config 路由(全是具体命令 get_llm_config / get_remote_config / get_web_search_config);`app_config` 表读法先例在 agent 侧(`chat_loop.rs:378`),无前端出口。**建议定案**:扩一个 `config/get_app_config` 路由(或复用 `get_llm_config` 的壳模式),前端 configStore 启动拉取一次缓存——implement.md PR2 已写「前端启动拉取」,与 design 的「或」留有歧义,建议 implement 为准(启动拉取),design 同步。

---

## 取证核对(确认无误的关键点)

| 锚点 | 核对结果 |
|---|---|
| chat_inner 统一入口(chat.rs:190),daemon REST 复用 | ✅ `pub(crate) async fn chat_inner` chat.rs:190;`daemon/routes/agent.rs:22` 直引;`POST /api/v1/agent/chat` 注释写明「和 Tauri chat 命令共用 chat_inner」 |
| spawn 是 fire-and-forget `tokio::spawn`(chat.rs:549) | ✅ 唯一 spawn 点 chat.rs:549,`ChatLoopDeps::from_app_state` + 解构后分发,JoinHandle 未持有 |
| SSE 零订阅者静默丢弃(sse.rs:189) | ✅ `broadcast`(sse.rs:165)fan-out 只对 `senders` 迭代,零订阅者自然无人收到;`subscriber_count()` sse.rs:219 存在 |
| 客户端断开不在 cancel 触发源内(helpers.rs:188,320) | ✅ `cancel_inflight_for_session`(helpers.rs:188 起)是命令/Stop 触发;`cancel_and_drain_all_agent_loops`(helpers.rs:320 起)是 daemon 停机触发;均非连接断开 |
| 忙表 `session_active_request: HashMap<session_id, rid>`(state.rs:112)无 REST/IPC 出口 | ✅ state.rs:112 定义;daemon `list_sessions`(routes/sessions.rs:39-43)直返 `list_sessions_inner` 的 DB 类型,无 busy 字段 |
| `SessionSummary`(db/types.rs:376)无 busy 字段 | ✅ types.rs:376 起逐字段核对,无 busy;daemon route 直返该类型 |
| 侧栏红点是纯前端 `streamingSessionIds`(streamController.ts:597-616) | ✅ streamController.ts:597 computed,`activeRequests` 推导,无服务端来源 |
| messages.status 行内状态 + partial index(schema.rs:387) | ✅ `db/migrations/schema.rs` `add_messages_column_if_missing(status)` + `CREATE INDEX ... WHERE status IS NOT NULL` 属实;注释写明 NULL/in_progress/interrupted 三态 |
| recover_interrupted_messages(DB 挂载 state.rs:348;messages.rs:357 空占位删除/有内容加 INTERRUPTED_MARKER) | ✅ state.rs:348 挂载;`db/sessions/messages.rs` recover 函数 Step A 查 `status='in_progress'` 逐行删或打标记,Step B 插 synthetic is_error 行,注释写明幂等 |
| 不建 background_tasks 表,subagent_runs 仅参照 | ✅ schema.rs 无 background_tasks;subagent_runs 在 schema.rs:523 起,status CHECK + 收割器模式真实存在 |
| app_config KV 表 + fail-open 读法先例(tools_stub_enabled,chat_loop.rs:378) | ✅ schema.rs:242 `CREATE TABLE IF NOT EXISTS app_config`;`chat_loop.rs:378` `get_config_value(&db, "tools_stub_enabled")` fail-open |
| `run_background_shell` / `dispatch_subagent` 不等价 | ✅ `run_background_shell` 是 shell 工具(L1),完成通知回同一 loop;`dispatch_subagent` 当前 turn 内跑(B6)——与 PRD 描述一致 |
| Tauri 窗口关闭无 close-requested 处理(全仓白纸) | ✅ 前端 `grep close-requested/onCloseRequested` 零命中;sidecar.rs RunEvent::Exit 钩子(kill_managed)是 daemon 侧退出,非窗口关闭拦截 |
| Thin 模式 GUI 退出回收 sidecar(RunEvent::Exit) | ✅ sidecar.rs:19 注释「RunEvent::Exit hook kills the sidecar」;kill_managed 注册成立 |
| adoptForeignRequest(streamEvents.ts:81-138)+ 终结判定(:141-153) | ✅ adoptForeignRequest streamEvents.ts:81;isTerminal 判定 streamEvents.ts:150-153(done 且 !groupChat / group_chat_end / cancelled / max_rounds;error 恒终结) |
| buildPendingNotification 模式(streamController.ts:472-487) | ✅ streamController.ts:472 导出纯函数,当前 session 抑制 + sessionId 附着 + 标题回退「另一项目的会话」;toast 点击跳转消费方 AppShell.vue:85-92(同 project 才 switchSession,Q4) |
| projectsStore.showToast(stores/projects.ts:101-118) | ✅ projects.ts:101 showToast(message, kind, durationMs, opts.sessionId),durationMs 默认 3500(design 写 6000 属可调参数,非契约) |
| 群聊终结判定复用 | ✅ streamEvents.ts:592-603 群聊 done 分支 isTerminal 逻辑真实存在(group_chat_end/cancelled/max_rounds),中间轮 done 不终结 |
| worker 事件走 SubagentBufferSink 不进主 chat-event 通道 | ✅ 通道分离成立(worker 产物落 subagent_runs + 独立 sink) |
| ChatAcceptance(started/queued)handler 立返 | ✅ chat.rs:334-380 路由临界区返回 `ChatAcceptance::Queued` / started;spawn 在 handler 内,返回不阻塞 |
| F1-A 队列驱动器随请求起灭(chat.rs:1079-1089) | ✅ chat.rs:1070-1095 驱动器尾段注销 cancellations + session_active_request(rid 守卫) |
| legacy 3a 顶替路径(spawn 在旧 loop 退出后) | ✅ chat.rs:468 起 3a 防御性取消 + 注册;`await exit` 后才继续 → spawn 前旧 loop 已退,信号量不会死锁 |
| SessionSummary 构造点多(Rust 无 default 构造)→ fixture 化 | ✅ types.rs:376 全字段字面量构造,无 Default 派生;implement PR1 的 fixture 化方向正确 |
| 双 transport enrich 单 helper 共用 | ✅ Tauri command list_sessions(commands/sessions.rs:27,37)与 daemon route 各自独立,helper 抽取合理 |
| R4 确认后退出走既有优雅停机 | ✅ cancel_and_drain_all_agent_loops(helpers.rs:320)+ recover_interrupted_messages 链路已存在,零新代码成立 |

---

## 建议动作

1. design §R3 伪代码改为「路由临界区内、注册之前 acquire」(P1),implement PR3 集成测试补「排队中 session 不在 activeRequests/busy」断言;
2. design §R4 守卫改判 `isTauriWebview()` 并补挂载层(main.ts Tauri-init 分支)与 window 句柄持有细节(P2);
3. design R1 stale-busy 消解补「清理挂 finalize 公共出口,不依赖 adoptForeignRequest」(P3-2);
4. design R2 config 读出口定案为「routes/config.rs 扩 get_app_config 路由 + configStore 启动拉取」,消除「或」歧义(P3-3);
5. PRD D5 与 implement PR4 的守卫措辞统一为「仅 Tauri webview」(P3-1);
6. 其余按现有三件套执行,无需其他改动。
