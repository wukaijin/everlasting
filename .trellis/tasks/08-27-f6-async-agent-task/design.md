# F6 异步 agent 任务 — 技术设计

对应 prd.md D1-D5 / R1-R5。总体判断:**零 schema migration、零新表、零新事件类型**——三块编排面全部搭在既有机制上。

## 架构总览

```
┌─ 既有(不动)──────────────────────────────────────────────┐
│ send → chat_inner → [F1-A 路由临界区] → tokio::spawn ──┼── fire-and-forget
│   (chat.rs:334-380)                        (chat.rs:549) │   loop 跑完落 DB
│ SSE 广播(零订阅者静默丢弃,sse.rs:189)                     │
│ messages.status + recover_interrupted_messages 跨重启终态 │
└──────────────────────────────────────────────────────────┘
新增三块:
① busy 字段(R1):session_active_request → SessionSummary.busy(daemon 层 enrich)
② 完成通知(R2):streamEvents 终结事件 → 非当前 session → toast(纯前端)
③ 并发闸(R3):AppState 全局 Semaphore,spawn 闭包头 acquire
④ 关闭确认(R4):Tauri close-requested → busy 时 ConfirmDialog(纯前端)
```

## R1 跨端忙可见性

**后端**:
- `db::SessionSummary`(`db/types.rs:376`)加 `#[serde(default)] pub busy: bool`——serde default 保证 additive wire(AC6);DB 层构造恒 `false`(busy 是运行时态,不属于 DB 层语义)。
- daemon `list_sessions`(`daemon/routes/sessions.rs:39`):db 调用后锁 `state.session_active_request`,对命中的 session 置 `busy=true`。锁是 `std::sync::Mutex` 还是 tokio?——`state.rs:112` 为 `Arc<Mutex<HashMap>>`(chat.rs:350-353 `lock().await` → tokio Mutex),enrich 用同款 `.lock().await`,临界区只做 key 匹配,微秒级。
- Tauri command `list_sessions`(Full 模式路径)同款 enrich(抽一个 `enrich_busy(sessions, &state)` helper 两处共用,防漂移)。

**前端**(`streamController.ts` / sessions store):
- 红点条件:`streamingSessionIds.has(id) || sessionsStore.serverBusy.has(id)`(`SessionList.vue` 两处 v-for 槽位,`:484` 与 `:577`)。
- **stale-busy 消解**(关键边角):冷启动时 list 带回 busy=true,但该 loop 的事件流早于连接开始,SSE 无回放 → 本端 activeRequests 不认识它;loop 结束的 Done 事件仍会全局广播到已连接客户端 → 终结路径最终走到 finalize。**`serverBusy.delete(sessionId)` 挂在 finalize 的公共出口**(评审 P3-2 处置:不依赖 adoptForeignRequest 的认领分支,一处覆盖本地/认领/群聊全部终结路径,防御性简化——注:认领本身在终结事件上也会发生,resolveRequest 对未知 rid 恒走 adoptForeignRequest(streamEvents.ts:137-138),仅 completedRequests 命中才 drop,评审「冷启动不认领」的前提不成立,但挂公共出口仍是更稳的落点)。兜底:session 列表任何 reload 自然刷新。
- `SessionList` 两个重复模板分支(flat/grouped)都要改——顺手评估抽子组件(与 DEBT 无关,纯防漏改,若 diff 大则放弃抽象只改两处)。

## R2 完成通知(完成 toast)

**纯前端,后端只补一个配置读出口**:
- 挂点:`streamEvents.ts` 终结判定处(`:141-153` isTerminal)——`done` 且 `!req.groupChat`(或 groupChat 终结 stop_reason)或 `error`,且 `sessionId !== currentSessionId` → 通知。
- 通知构造:新增纯函数 `buildTurnFinishedNotification(sessionId, kind)`(镜像 `buildPendingNotification`,`streamController.ts:472-487` 的当前-session 抑制 + sessionId 附着 + 点击跳转模式);`kind: "done" | "error"` 区分文案(「会话 X 已完成」/「会话 X 出错停止」)。session 标题查 sessions store,查不到降级用 sessionId 前缀。
- 触发:`projectsStore.showToast(message, "info", 6000, {sessionId})` 既有管道(`stores/projects.ts:101-118`)。
- 开关:`app_config` 键 `turn_complete_notify_enabled`(fail-open 缺省开,`chat_loop.rs:378` 读法先例);**读出口定案(评审 P3-3)**:`routes/config.rs` 新增通用 `get_app_config`(或最小 `get_flag_config`)POST 路由——现状核实该文件全是具体命令路由(get_llm_config / get_remote_config / get_web_search_config),无通用读法——前端 configStore 启动拉取一次缓存,照 config router 全 POST 先例。开关关闭 → hook 直接 return。

**群聊**:复用既有 isTerminal 判定(`group_chat_end`/`cancelled`/`max_rounds` 才终结),中间轮 done 不触发(AC2)。

**worker**:worker 事件走 `SubagentBufferSink` 不进主 chat-event 通道,天然不误触。

## R3 F3 并发闸(全局信号量)

- `AppState` 加 `loop_permits: Arc<tokio::sync::Semaphore>`;构造时读 `max_concurrent_loops`(缺省 4,parse 失败回退 4;**改配置需重启 daemon**,PR 说明里写明)。
- 获取点:`chat_inner` spawn 闭包**第一行**(chat.rs:549 之后、三分支分发之前):
  ```rust
  let permit = tokio::select! {
      p = state.loop_permits.clone().acquire_owned() => p,
      _ = token.cancelled() => { /* 早退:回滚注册,见下 */ }
  };
  ```
  许可随闭包 drop 自动释放,无手动归还面。队列驱动器(F1-A 路径)全程持 1 个许可(跨内层多轮)——「持闸者 = loop 实例」,经典/群聊 loop 同理各持 1。
- **取消早退臂的回滚模板**:复用 pre-flight 失败回滚先例(chat.rs:429-432)——`cancellations.remove(&rid)` + `session_active_request.remove(&session_id)`(rid 守卫,防误删顶替者)+ `inflight_exits.remove(&rid)`,再补发 cancelled 终态事件。注册发生在 claim(路由临界区),等闸取消必须完整回滚,否则 session 被假在途请求卡死。
- **busy 语义定案(评审 P1 处置)**:claim 即注册(chat.rs:366-378 在 spawn 之前),等闸的 session **busy 即亮**——这是有意语义:「busy = 已接受在途(含排队等闸)」,而非「正在执行」。理由:(a) 注册即 claim 是 F1-A per-session 互斥的机制本身,不能推迟到拿到许可后;(b) R4 关闭确认把等闸轮次计入「将被终止」是**正确的**——等闸的 spawn 闭包同样是 daemon 内 tokio task,GUI 关闭杀 daemon 时一并死;(c) 默认 4 的闸日常几乎不触发,亮灯场景罕见。AC3 测试断言以此为准。
- **不选**「handler/路由临界区内 acquire」(评审 P1 建议的修正,**驳回**):路由临界区持有**全局** `message_queues` 锁(chat.rs:349,跨所有 session 共享),在此 await 许可会 (a) 队头阻塞所有 session 的发送,破坏 ChatAcceptance 立返;(b) **死锁**:驱动器 turn 边界 `drain_all`(chat.rs:967)要拿队列锁 ↔ 等待者持队列锁等许可、许可要等驱动器退出才释放——环成立。acquire 放临界区之前(锁外)也不行:第 N+1 个 send 的 HTTP handler 会挂到有许可为止(可能数分钟),与 F1-A「忙则立返 Queued」的既有契约相反。
- 与 F1-A 的正交性(纠正评审 P1 第 3 点):busy 时消息**只入队不 spawn**新驱动器(chat.rs:366 `if !busy` 守卫 + :381-389 立返 Queued),per-session 互斥由 claim 注册保证,与全局信号量无关;「消息粒度排队」「同 session 驱动器并发」场景不存在。
- Legacy 3a 顶替路径(取消旧 loop + await 退出再 spawn):spawn 发生在旧 loop 退出后,无许可死锁;信号量排队不改变该路径语义。
- 闸外:worker(`DELEGATION_MAX_CONCURRENT_CHILDREN=3` 自限)、daemon 内非 loop 后台任务(背景 shell 有自己的 max_runtime)。

## R4 关闭确认(Thin GUI)

- **守卫判据(评审 P2 采纳)**:`isTauriWebview()`(`transport/env.ts:31`,判 `window.__TAURI_INTERNALS__`)——语义是「运行在 Tauri webview 内(Thin/Full GUI 壳)」。**不能用 transport 种类判断**:daemon 化后 Tauri 壳内默认也是 httpTransport(sidecar),transport 分辨不出壳形态;浏览器/PWA 不注入该全局,天然排除。
- **挂载点**:main.ts 的 Tauri 初始化分支(与 transport 选择同层),不挂 AppShell——避免组件复用/热重载导致 handler 重复注册(评审 P2)。
- 实现:
  ```ts
  if (isTauriWebview()) {
    const win = getCurrentWindow();          // handler 外捕获句柄
    win.onCloseRequested(async (e) => {
      const busyCount = controller.streamingSessionIds.size + serverBusyCount();
      if (busyCount === 0) return;           // 直接关
      e.preventDefault();
      openConfirmDialog(busyCount);           // 复用 ConfirmDialog
      // 确认 → win.destroy()(复用捕获句柄,不重新 getCurrentWindow);取消 → 关弹窗
    });
  }
  ```
- busy 计数 = 本端在跑 ∪ 服务端 busy(含**其他端**发起的 loop 与**等闸排队**的轮次——Thin 模式关 GUI 杀 daemon,全部陪葬,所以必须数服务端口径且含等闸;见 R3 busy 语义定案)。
- Web/PWA:无 close-requested 概念,不挂——关闭标签(Ctrl+W)/杀 App/锁屏仅断开 SSE 订阅,loop 照跑(standalone daemon 独立存活),无需也无法拦截。与 Tauri 窗口关闭是**两种不同语义**,R5 文档必须分开写清(用户已踩过的困惑点)。
- 确认后退出:daemon 走既有优雅停机(cancel + drain)→ 下次启动 `recover_interrupted_messages` 标 interrupted——**该链路已存在,零新代码**。

## R5 文档

- ROADMAP §1.2 加 F6 行(做了什么 + 时间);§2/§4 更新 F3/F6 状态与 F1-C 归 F2 的裁定。
- docs/REMOTE-DEPLOY.md(或 HACKING-wsl)加「detach 边界」小节:后台任务存活 = daemon 进程;耐久路径 = standalone daemon / browser 模式;Thin GUI 关闭有确认弹窗。

## 兼容与回滚

- wire:`busy` additive + serde default,旧前端忽略(AC6);`ChatAcceptance`/事件类型零改动。
- 配置开关:`turn_complete_notify_enabled=false` 关通知;`max_concurrent_loops` 调大等效关闸——两特性均可运行时(前者)/重启(后者)降级,回滚面小。
- 各 PR 独立可回滚:PR1(busy 后端)/PR2(前端可见性+通知)/PR3(信号量)/PR4(关闭确认+文档)无相互依赖,仅 PR2 依赖 PR1 的字段。

## 已识别边角(实现时验证)

1. stale-busy(见 R1):靠终结事件顺带清 + reload 兜底;若 live 验证发现长静默 loop(长工具执行期)红点悬挂,再评估 list 轮询/焦点刷新(不进 MVP)。
2. 信号量排队期间取消:select! 早退臂必须完整清 slot/token/queue 注册并补 cancelled 终态事件,否则 rid 泄漏(镜像 F1-A DriverSink 的终态补发教训)。
3. `SessionSummary` 构造点全部要补 `busy` 字段(Rust 无 default 构造)——grep 所有字面量构造,测试构造器优先走 fixture/`..Default::default()`。
4. 双 transport(Tauri IPC / daemon REST)enrich helper 单点共用,防只改一边(F1-A「路由口径统一」评审教训)。

## 评审处置(2026-08-27,`review.md` 逐点甄别后的裁定)

| 评审点 | 裁定 | 理由(均经代码复核) |
|--------|------|---------------------|
| P1 并发闸:acquire 应上移进路由临界区 | **驳回修正方案,采纳其暴露的 busy 语义问题** | 修正方案有死锁:临界区持全局 `message_queues` 锁(chat.rs:349),await 许可 → 队头阻塞所有发送 + 与驱动器 turn 边界 `drain_all`(chat.rs:967)的队列锁构成环(持许可者等锁 ↔ 持锁者等许可);锁外 acquire 则 handler 挂起,违反 ChatAcceptance 立返。但其观察「等闸 session busy 即亮」成立 → 定案为有意语义(R3 busy 语义段)。第 3 点(per-message 驱动器 spawn)与代码不符:busy 只入队不 spawn(chat.rs:366/:381-389)。 |
| P2 守卫应判 isTauriWebview 而非 transport | **采纳** | `transport/env.ts:31` 存在;daemon 化后 Tauri 壳内默认 httpTransport,transport 分辨不出壳形态。挂载 main.ts、句柄复用细节一并采纳。 |
| P3-1 三处守卫措辞统一 | **采纳** | 统一为「仅 Tauri webview(Thin/Full GUI 壳)」。 |
| P3-2 stale-busy 清理挂 finalize 公共出口 | **采纳落点,纠正前提** | 评审称冷启动端「adoptForeignRequest 不会创建条目」与代码不符:resolveRequest 对未知 rid 恒走认领(streamEvents.ts:137-138),仅 completedRequests 命中才 drop——认领在终结事件上成立。挂公共出口仍采纳(一处覆盖全部终结路径,防御性简化)。 |
| P3-3 config 读出口消除「或」歧义 | **采纳** | 核实 routes/config.rs 无通用读法(全是具体命令);定案新增 `get_app_config` POST 路由 + configStore 启动拉取。 |

评审「取证核对」表的锚点与我的勘察一致,无冲突。
