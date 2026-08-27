# F6 异步 agent 任务(detach 后台跑)

## Goal

用户派一个大任务给 agent 在后台跑,期间继续用其他 session 或离开;任务完成时收到通知,回来能看到完整结果;关窗口/重启不静默丢任务。补齐异步执行家族的编排面(F6),含 F3 最小档(跨 session 并发上限)。

**核心洞察(勘察结论)**:detach 的运行时语义今天已免费成立——`chat_inner` 的 agent loop 是丢弃 JoinHandle 的 fire-and-forget `tokio::spawn`(`agent/chat.rs:549`),事件对零 SSE 订阅者静默丢弃(`daemon/sse.rs:189`),客户端断开不在 cancel 触发源内(仅 Stop / 破坏性命令 / daemon 停机,`agent/helpers.rs:188,320`)。F6 缺的不是执行面,是**编排面**:跨端忙可见性、完成通知、并发闸、关闭边界。

**给实施者的一句话**:F6 前后执行模型零变化——一条 loop 的生灭路径完全不变,发消息在今天就已经是「daemon 里后台跑一个 loop」(SSE 是观察通道,不是执行通道)。交付物是可观测性/通知/边界三件套,不要带着「要造后台执行」的误解进场。

## Background(已确认事实)

### 运行时与队列

- 发送统一入口 `chat_inner`(`agent/chat.rs:190`):Tauri IPC 与 daemon REST `POST /api/v1/agent/chat`(`daemon/routes/agent.rs:44`)共用;handler 立即返回 `ChatAcceptance`(started/queued)。
- F1-A per-session 串行已保证:路由临界区(`chat.rs:334-380`,锁序 message_queues → session_active_request)忙则入队(上限 20),闲则 spawn 队列驱动器;驱动器随请求起灭、退出注销 slot(`chat.rs:1079-1089`)。
- **跨 session 并发无任何上限**(F3 空白);worker 并发已有 `DELEGATION_MAX_CONCURRENT_CHILDREN=3` 自限;群聊是单 loop。

### 忙可见性空白

- 进程内权威忙表 `AppState.session_active_request: HashMap<session_id, rid>`(`daemon/state.rs:112`)**无任何 REST/IPC 出口**;`list_sessions` 返回的 `db::SessionSummary`(`db/types.rs:376-421`)无 busy 字段(daemon route 直返 DB 类型,`daemon/routes/sessions.rs:39`)。
- 侧栏红点 `streamingSessionIds` 是纯前端内存推断(`streamController.ts:597-616`):后端 detach 跑的 loop 对第二个客户端(冷启动的 PWA/浏览器)完全不可见。

### 跨重启终态已有机器(不需要新表)

- messages.status 行内状态(`NULL/'in_progress'/'interrupted'` + partial index,schema.rs:387)+ 启动恢复 `recover_interrupted_messages`(`state.rs:348` 挂载,`db/sessions/messages.rs:357`:空占位删除/有内容加 INTERRUPTED_MARKER 块)。daemon 崩溃/停机后重启,在跑轮次自动获得 interrupted 终态。
- 「任务」= 在跑轮次(session 即载体),其持久化已覆盖;**不建 background_tasks 表**;subagent_runs 模板(status CHECK + 收割器 + finished 事件)仅作实现参照。

### 通知通道现状

- 系统级通知不存在(无 tauri-plugin-notification / Web Notification / 声音)——out of scope。
- 应用内:`projectsStore.showToast` 单例 + `buildPendingNotification` 纯函数模式(当前 session 抑制 + sessionId 附着 + 点击跳转,`streamController.ts:472-487`);pending-indicator 三档提醒为纯内存态,后台任务命中 permission ask / question 时既有三档提醒天然唤回用户,无需新策略。
- 跨客户端事件:昨日 session_id 认领已闭环(`streamEvents.ts:81-138` adoptForeignRequest;终结判定 `:141-153` foreign Done/Error 也 finalize)——完成通知的事件源已就绪。
- Tauri 窗口关闭事件:全仓无 close-requested 处理(白纸);Thin 模式 GUI 退出回收 sidecar daemon(RunEvent::Exit 钩子)。

### 开关先例

- `app_config` KV 表 + fail-open 缺省开读法(`tools_stub_enabled` 等,`chat_loop.rs:378` 模式)。

### 触发面现状(执行已存在,触发已被 D2 消解)

- 用户侧:发消息即触发,无需也无一等显式触发器——loop 活在 daemon 进程,网页关闭/换设备/锁屏均不影响(standalone daemon 形态)。
- LLM 侧:**无「派独立后台任务」能力**。两个近似物都不等价:`run_background_shell`(L1,仅 shell 命令,完成通知回到同一 loop 下一轮)、`dispatch_subagent`(B6,在当前 turn 内跑、父轮被占用;但用户切走后该 session 的 loop 含 worker 照跑,F6 busy 可见性对它同样生效)。
- 真正缺口 =「LLM 派任务到新独立 session 后台跑」:需要 LLM tool → 创建 session → 无 GUI 调 `chat_inner` 注入,最后一环即 F1-C。**F1-C 因此有两个消费者:F2 cron + LLM detached dispatch**(见 Out of Scope)。

## Decisions(已定案)

- **D1 任务载体 = Session 即载体**:后台任务 = 普通会话里在跑的一轮的产品化;不建独立任务面板/一等任务实体;结果即完整 chat 历史,复用既有 UX。
- **D2 隐式普遍化,无「后台发」概念**:所有 session 的在跑轮次自动获得跨端忙可见性 + 完成通知(当前 session 抑制 + 点击跳转);零新入口;app_config 开关 `turn_complete_notify_enabled` 默认开可关;不加时长阈值(单人使用,切走后完成即想要的信号)。
- **D3 F3 最小档留在本任务**:`chat_inner` spawn 点加全局信号量,超出排队不拒绝,`ChatAcceptance` 语义不变、`Start` 延迟到拿到许可;app_config `max_concurrent_loops` 默认 4;worker/群聊不在本闸管辖(前者自限、后者单 loop)。**busy 语义(评审 P1 处置后定案)**:busy = 已接受在途(含排队等闸)——claim 即注册,等闸 session 红点即亮,是有意语义;R4 关闭确认计入等闸轮次(它们随 daemon 一并死,计数正确)。
- **D4 F1-C 移出本任务,归 F2**:chat_inner 已是 transport-agnostic 统一入口,F6 MVP 无额外消费者;F2 cron 落地时按调度需求再包装。
- **D5 GUI 关闭策略 = 接受边界 + 关闭确认弹窗**:detach 边界 = daemon 进程(文档化);耐久后台走 standalone daemon/browser 模式(已存在);Thin GUI 收到关闭请求时有 session 在跑 → 确认弹窗「关闭将终止 N 个在跑会话」,不静默杀。完整分离 daemon 模式留 F2(需常驻进程,届时与 cron 一起设计)。**两种关闭语义必须区分**(用户已踩过的困惑点):确认弹窗**仅挂 Tauri webview(Thin/Full GUI 壳)**的窗口关闭事件(X 按钮 / Alt+F4 / 任务栏关闭),判据 `isTauriWebview()`(`transport/env.ts:31`)而非 transport 种类——daemon 化后 Tauri 壳内默认也是 httpTransport;Web/PWA 关闭标签(Ctrl+W)/杀 App **不影响任务**——客户端断开非 cancel 源,standalone daemon 独立存活,无需任何拦截逻辑。

## Requirements

- **R1 跨端忙可见性**:`SessionSummary` additive 加 `busy` 字段(daemon/Tauri 层从 `session_active_request` enrich,DB 层恒 false);前端侧栏红点条件扩展为「本端推断 OR 服务端 busy」,冷启动客户端即可见其他端发起的在跑轮次。
- **R2 完成通知**:frontend 在 foreign/非当前 session 的终结事件(Done 成功 / Error)处挂 toast,复用 buildPendingNotification 模式(当前 session 抑制 + sessionId 附着 + 点击跳转);Error 文案区分;群聊按既有终结判定(group_chat_end/cancelled/max_rounds 才算完);`turn_complete_notify_enabled` 开关。
- **R3 F3 并发闸**:全局 `tokio::sync::Semaphore`,`max_concurrent_loops`(默认 4)启动时读入;许可在 spawn 闭包开头获取(不阻塞 handler,自动随 drop 释放);F1-A 队列驱动器全程持一个许可。
- **R4 关闭确认**:仅 Tauri webview(Thin/Full GUI 壳)`close-requested` → 有在跑 session(本端 streaming ∪ 服务端 busy,含等闸轮次)时弹 ConfirmDialog,确认后才真正退出;无在跑直接退;Web/PWA 无此逻辑(客户端关闭不杀 standalone daemon)。
- **R5 文档**:ROADMAP §1.2 增 F6 行;detach 边界(daemon 进程)与耐久路径(standalone daemon)写入 REMOTE-DEPLOY 或 HACKING。

## 已知边界(MVP 接受,显式非目标)

- 可观测性粒度 = session 级三元状态(**忙 / 终态(完成·出错) / 中断**),无执行进度条:长静默期(长工具执行/LLM TTFB)内,「在跑第 N 个工具」与「挂死」不可区分——这是 SSE 无等待态语义的既有缺口(brainstorm 前置讨论已确认),心跳/等待态是独立后续方向。
- 冷启动中途加入的端,需等该 loop 的下一个事件(delta/tool_call)才能经 session_id 认领看到实时内容;纯静默期只有红点(busy)。
- 细粒度历史回看(token/每轮延迟/压缩)走既有 E2 TracePanel,非本任务范围。

## Acceptance Criteria

- [ ] **AC1 跨端红点**:PC 端 session A 发起长任务,新开浏览器/PWA 冷启动 → 侧栏 A 显示红点;发起端窗口关闭后另一端红点仍在;任务结束后红点消失。
- [ ] **AC2 完成通知**:用户正在 session B 时 A 完成 → toast「A 已完成」点击跳转 A;A 是当前 session 时无 toast;A 以 Error 终态结束时 toast 文案为失败语义;开关置 false 后无 toast;群聊多轮中间 Done 不触发 toast。
- [ ] **AC3 并发闸**:`max_concurrent_loops=2` 下第 3 个 send 仍**立返** started(queued 语义不变),其 busy 即亮(已接受在途语义)、Start 事件延迟至第 1 个许可释放且按 FIFO 启动;等闸中取消 → session 注册完整回滚、可直接再发;默认 4 时常规使用零感知。
- [ ] **AC4 关闭确认**:Thin GUI 有在跑会话时点窗口关闭 → 弹确认;取消则不退;确认则退出(下次启动 interrupted 恢复既有行为不变);无在跑时直接退。
- [ ] **AC5 全量回归**:cargo 全量 + vitest 全量 + vue-tsc/clippy/fmt 绿;turn-smoke.sh live 一轮通过;F1-A 队列、群聊、worker 既有行为不回归。
- [ ] **AC6 wire 兼容**:`SessionSummary.busy` additive(旧客户端忽略不炸);daemon/Tauri 两 transport 一致。

## Out of Scope

- F1-C 无 GUI 生产者注入入口(归 F2;**两个消费者:F2 cron + LLM detached dispatch**,届时一起设计注入 payload 形态)
- LLM detached dispatch(LLM 派任务到新独立 session 后台跑、当前会话继续聊)
- F2 定时任务 cron(异步家族下一站)
- 系统级通知(tauri-plugin-notification / PWA push / 声音)
- daemon 分离模式(GUI 退出保活 sidecar,归 F2)
- SSE 等待态/心跳语义(长静默期区分等待与挂死,见「已知边界」)
- 离线错过通知的持久化(unread 标记;回来靠 session 预览感知)
- 独立任务面板/任务模板/重跑
- 跨机器任务同步(remote 只中继,现状即满足)
