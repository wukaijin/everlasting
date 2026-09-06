# Review — GCE-M3 控制面暴露层(planning 门评审)

> 评审对象:`prd.md` + `design.md` + `implement.md`(2026-09-06,基于 main `92ddb29e`)。
> 任务状态:`planning`(`task.json.status`),尚未 start;implement/check.jsonl 均为 seed 占位。
> 证据核验:2026-09-06 对工作区实际代码逐一比对(Rust daemon、前端 store/组件、scripts 层)+ 实测 wire 预算。

## 结论摘要

PRD / design / implement 整体质量高:M3 本体 = 暴露层(JS MCP 工具 + GUI + 文档)的定位与
代码事实一致;三决策(裸 SSE 透传、零鉴权、预算 3200)均与 roadmap §4 / M0-M2 一脉相承;
P0 内核全链落地属实(preempt 命令 + controls 注入缓冲 + 双轨标记 + 轮头落库 + finalize 白名单)。
**核验发现 1 个 start 前需定案的实质缺口(P1:inject misfire 防护对「已收官群聊 session」有损)
与 4 个实现定义缺口/细节修订(P2×1 + P3×3)——均不推翻已定决策,修订量小。**

**建议状态:需修订后放行。**

---

## 1. 证据核验结果(PRD/design 引用逐条)

| 引用 | 实测 | 判定 |
|---|---|---|
| Fact 1:preempt 端点全链落地(`commands/cancel.rs:135` + `daemon/routes/cancel.rs:39`,body `{session_id}`→`{"preempted":bool}`)+ DAEMON-API §4 已文档化 | `commands/cancel.rs:96-133` `preempt_group_chat_inner`(controls 注册表查 session → 置 `preempt_requested=true`);Tauri command `:136-141`;`daemon/routes/cancel.rs:39-45` + route 注册 `:50`;`lib.rs:218` + `commands/mod.rs:92` 注册;DAEMON-API §4「进行中干预」段确已档;空闲报错「该会话当前没有进行中的群聊讨论」(`:131`) | ✅ 行号微偏(P0 后新增注释行,实 115-141;核心引用成立) |
| Fact 1:缺 JS 封装,`group-chat-run.mjs` 只有 rid 域 `cancelChat` | run.mjs 导出清单:`cancelChat(base, requestId)`(`:357`),无 preempt/session 域 helper | ✅ |
| Fact 2:`POST /api/v1/agent/chat` 恒返回 JSON `ChatAcceptance`(agent.rs:46) | `daemon/routes/agent.rs:44-52` handler 返 `Json<ChatAcceptance>`;struct 定义前接 ChatRequest | ✅ |
| Fact 2:三态 `Started | Queued{id,position} | Injected`(chat.rs:164-174) | `chat.rs:164` enum `ChatAcceptance`,Started 165 / Queued 166-171 / Injected 172-174 | ✅ |
| Fact 2:busy 群聊 → Injected(controls 缓冲) | `chat.rs:360-400`:group_chat_ctx + 尾条 user + busy → 推 `pending_injects` 返 `Injected`;纯图片注入报错 `:379-383` | ✅ |
| Fact 2:双轨标记由编排器轮头 `insert_user_inject` 落(`session_crud.rs:853`) | 实为 `db/sessions/session_crud.rs:853`;group_chat_loop.rs 轮头 `:356-383` 取 `pending_injects` → 落库 `[用户插入] ` 前缀 + `metadata.kind="user_inject"`(实现内 `:879-885` 区) | ✅ **路径缺 `db/sessions/` 前缀**(M2 review 踩过同款,见 P3-1) |
| Fact 3:Tauri transport 纯透传(`tauri.ts:19`) | `tauri.ts:19-20` invoke 直委 `@tauri-apps/api/core` | ✅ |
| Fact 3:http transport 顶层 key camelCase→snake_case(http.ts 头注)→ `invoke("preempt_group_chat", {sessionId})` 两模式通用 | `http.ts:13-20` 头注明确「顶层 key 扳正」;command 已注册(见 Fact 1) | ✅ |
| Fact 3:全前端现无调用方 | grep 全 `app/src` 无 `preempt_group_chat` 调用 | ✅ |
| Fact 4:单全局流 `GET /api/v1/stream`,payload 带 request_id+session_id(sse.rs:22-27) | `state.rs:734-742` ChatEventPayload `request_id`+`session_id`(后端回填,2026-08-27);sse.rs 头注同述 | ✅ |
| Fact 4:chat-event `Speaker`/`Delta`/`Done{stop_reason,usage}`(event.rs:64-141) | `llm/types/event.rs:75` Speaker / 87 Delta / 90 ThinkingDelta / 114 Done;范围 64-141 覆盖 | ✅ |
| Fact 4:Last-Event-ID 重放 + `stream-resync` sentinel + snapshot 补齐 | `daemon/sse.rs:61-88`(replay/淘汰/sentinel)+ `routes/stream.rs`(subscribe/replay/live)+ http.ts 头注 | ✅ |
| Fact 5:`TOOLS_BUDGET_CHARS=2300`(mcp.mjs:265 + smoke `:23`)已用 1945 | mcp.mjs:265 定义;smoke `:20` 常量;实测 smoke 输出 1945 chars ≈486 token | ✅ smoke 行号实为 `:20`(见 P3-2) |
| Fact 6:created_via 建群写入且 Rust 不读 | grep 全 `app/src-tauri/src` 无 `created_via` 读取;仅 `buildCreateSessionBody` 写 metadata(JS) | ✅ |
| Fact 7:inject 误发副作用可闭合(request_id 调用方生成,buildChatBody) | `run.mjs:166-171` buildChatBody 带 request_id;`cancelChat`(rid 域)可即时止损 | ⚠️ **成立但设计不完整——见 P1-1** |
| Fact 8:`sending=true` + 终态 `preempted` finalize 白名单 + notice(streamController.ts:541-545) | `groupChatNotice` `preempted` case 在 streamController.ts:543;但 **finalize 白名单在 streamEvents.ts:616-627**(isTerminal 含 preempted);「sending」实为 store `isCurrentSessionStreaming`(chat.ts:599,源自 controller.streamingSessionIds) | ✅/⚠️ 白名单跨文件(见 P3-3) |
| design §3:M1 共享层将新增 `preemptGroupChat` helper;`inject` 不新增 helper | run.mjs 现无 preempt helper;cancel 簇先例存在 | ✅ |
| design §4.1:ChatPanel.vue:211-218 群聊 indicator 区 | `:215-217` 是 `isGroupChat` computed(TS 区);群聊 chip **template 在 718-727** | ⚠️ 见 P3-4 |
| design §5:7 父 + 2 worker 事件通道;30s keepalive | routes/stream.rs 头注「parent 7 channel + worker subagent:event/finished」;`KEEPALIVE_INTERVAL_SECS=30` | ✅ |
| design §5:零订阅者 8s 快拒 | `daemon/sse.rs:360-364` has_live_observer(subscriber_count>0);server.rs:383 8s;DAEMON-API §5 已档 | ✅ |
| 预算 3200 两侧同步 + smoke 六工具 | 实测六工具 wire = **2881 chars** < 3200(余量 319 ≈ 10%);2881 > 2300 →「加两工具必超」成立 | ✅ 见 §3 D3 |

## 2. 主要发现

### P1-1 inject misfire 防护对「已收官群聊 session」有损——需加前置 busy guard

PRD Fact 7 / design §2.2 的止损语义 = fireChat → acceptance 非 `Injected` → 自有 rid 即时
`cancelChat` 止损。此设计对**忙碌中**群聊天然成立(那本来就是 Injected 分支),真正会 misfire
的是**非 busy 的群聊 session**——即调用方拿一个**已收官的 session_id**(典型:刚读完
`discussion_result` 顺手 inject)误调。此时:

1. `fireChat` 走 F1 统一路径:busy=false → claim slot → **重启编排器**(group_chat 分支)。
2. `run_group_chat_loop` 开头**无条件 `clear_group_chat_lifecycle`**(`db/sessions/session_crud.rs:925-944`,
   置 `stop_reason=NULL, discussion_summary=NULL`)——**上一场已收官的 summary 被抹掉**。
3. 此时 misfire 防护才 cancel(rid):编排器在首个 `token.is_cancelled()` 处退出,
   finalize 落 `stop_reason=cancelled`(group_chat_loop.rs:850-851),原 `group_chat_end`/`preempted`
   终态与 summary **不可恢复**(COALESCE 只会保留本场写入的新 summary,而本场被 cancel 无 summary)。

即「cancel 止损」闭合了「新讨论继续跑」的副作用,却**没闭合「旧场终态被抹」**——对一个刚
收官的讨论,误发 inject 的代价是永久丢失其 summary。这不是防御分支:正是 MCP 宿主最可能犯的
用法错误(拿 result 里返回的 session_id 顺手注入)。

**建议(修订量小,design §2.2 增前置 guard)**:`coreInject` 在 fireChat **前**先走
`findSession`/load_session 判定目标状态——busy=false 或 stop_reason≠null(即非进行中)→
**不 fireChat**,直接报语义错误「目标不是进行中的群聊」(零副作用,连 Started/Queued 都不产生);
busy=true 才 fireChat 并断言 `Injected`。MCP server 已有 `findSession`(coreStatus/coreResult
共用,能拿 busy/stop_reason/session_type),前置 guard 零新机制。misfire 的 cancel 兜底降级为
竞态防御(编排器已启动、guard 判定与落库间的窗口),不再是主止损路径。同步修 AC2 表述:
「不留下新起的讨论」由「cancel 止损」改为「前置 guard 根本不发起」+ implement.md Phase A/B
用例补「已收官群聊 session 注入 → 报错且 stop_reason/summary 原样保留」一档。

### P2-1 AC1「轮询到终态后 stop_reason=preempted」在自然收官竞态下可能不成立

preempt 是轮边界信号:编排器在**轮头**检测 `preempt_requested`(group_chat_loop.rs:384-389)。
若打断请求恰好落在 moderator 已在跑收尾轮(该轮内 `end_discussion`)的时刻,本轮结束 break 为
`HaltReason::DiscussionEnded`(535)→ 终态 `stop_reason=group_chat_end` 而非 `preempted`。
窗口极窄(打断与自然收官同刻),端点返回的 `preempted:true`(信号已置)不受影响,但 AC1
「轮询到终态后 stop_reason=preempted 且 summary 非空」的表述过强。建议 implement.md 验收时
把该断言写为「`preempted`(或同刻自然收官的 `group_chat_end`)且 summary 非空」,live 验收
按此判定——否则实现者会为了修一个假失败而误改轮边界语义。

### P3-1 引用路径缺 `db/sessions/` 前缀(事实正确)

`insert_user_inject` 在 `db/sessions/session_crud.rs:853`,非根 `db/session_crud.rs`(根下是
`sessions.rs` 33 行 hub 纯 re-export——M2 review P3-1 同款)。建议 PRD Fact 2 改引用为
`db/sessions/session_crud.rs:853`。

### P3-2 smoke 预算常量行号

`group-chat-mcp-smoke.mjs` 的 `BUDGET` 常量在 `:20`(PRD 写 `:23`)。预算变更需改**两处**
硬编码:定义 `mcp.mjs:265` + 常量 `smoke:20`(design 说「两侧」成立,但 PRD 行号微偏)。
`group-chat-mcp.test.mjs` 从模块 import `TOOLS_BUDGET_CHARS`(`:12`),自动跟随,无需改——
design 的「两侧同步」表述可注明测试侧自动同步,防实现者去 test 里找第三处。

### P3-3 finalize 白名单与 notice 分居两文件

Fact 8 引 `streamController.ts:541-545` 覆盖了 `groupChatNotice` 的 `preempted` 文案
(`:543`),但**终态 finalize 白名单在 `streamEvents.ts:616-627`**(isTerminal 含 preempted)。
两处均已随 P0 交付,事实成立;仅引用位置跨文件。GUI 打断入口的「终态呈现零新工作」断言
成立(见 §3 D3 佐证)。

### P3-4 ChatPanel 群聊 indicator 引用位置失准 + store 层 transport 归属

design §4.1 引 `ChatPanel.vue:211-218` 为「群聊 indicator 区」——实测 `:211-218` 是
`isGroupChat` computed(TS 注释区),群聊 chip template 在 `:718-727`。建议实现期锚点改为
template 区;按钮可见性的「sending && isGroupChatSession」命名在 ChatPanel 语境下应为
`isGroupChat`(该名只在 ChatInput.vue:215 存在)。另 design §4.2 说「store 层 action →
`transport.invoke(...)`」——实测 **transport 只由 streamController 持有**(chat store 经
`controller.cancel(rid)`/`controller.start` 访问,chat.ts:893 / streamController.ts:1220),
store 层无 transport 引用。GUI preempt 的正确形态 = chat store action 调 controller 新增的
`preempt(sessionId)` → `transport.invoke("preempt_group_chat", { sessionId })`(两 transport
自动扳正 key)。建议 design §4.2 补一句「经 controller 访问 transport,与 cancel 同构」,
避免实现者往 store 里硬塞 transport。

### P3-5 planning 完成度

`task.json.status=planning`;implement/check.jsonl 仍为 seed 占位(workflow 要求 complex 任务
start 前各含真实条目)。放行条件见 §5。

## 3. 设计决策核对(D1–D3)

三条决策与代码事实一致,无回退:

1. **D1 follow 裸 SSE 透传、daemon 零改动、只补文档** —— daemon 已在全局流发
   Speaker/Delta/Done + `session_id`(state.rs:734-742),Last-Event-ID/replay/sentinel/snapshot
   /30s keepalive 全在(routes/stream.rs);轮询已够用、缺的是外部消费专章的判断成立。R3 文档
   专章的六项内容清单逐项有机制可档,零代码达成 AC5 可行。
2. **D2 v1 零鉴权、全域可打断/注入** —— 与 M2「零鉴权(本机)」同构;真暴露面仍在 daemon
   0.0.0.0(§8 安全边界已档);粒度推迟 M4 随认证一体议与 roadmap 一致。
3. **D3 预算 2300→3200** —— 实测支撑:四工具 1945,按 design 描述密度 + 简单 schema 注入两
   新工具后六工具 wire **2881 chars**,3200 余量 319(≈10%)。2881 > 2300 证实「加两工具必超」。
   ⚠️ 余量不宽:interrupt_discussion 的响应指引文案若写进 description、或 inject 的描述再扩
   会吃掉余量。建议 implement 后以 smoke/单测实测为准(两侧锁自动断言),若超 3200 需回评审
   (锁数字本身是有意变更);描述仍按 design 纪律不刻意压缩、但不过度铺陈。

另确认 design §1「Rust 侧预期零 diff」基线成立(P0 已交付内核,本任务面纯 JS/前端/文档);
§3 共享层纪律(纯逻辑区零 SDK import / node --test)与 M2 一致;§7 兼容性声明(增量暴露、
唯一破坏性变更 = 预算数字)成立。

## 4. 对 Acceptance Criteria 的检查

| AC | 判定 | 备注 |
|---|---|---|
| AC1 interrupt 打断 + 空闲报错 | ⚠️ | 端点就绪、工具为薄包装;「轮询到 preempted」断言需按 P2-1 放宽(自然收官竞态) |
| AC2 inject busy 注入 + 非 busy 报错且不留新讨论 | ⚠️ | 注入落库/双轨由 P0 保证;「不留新讨论」的止损路径对已收官群聊有损(抹旧 summary)——需按 P1-1 加前置 busy guard |
| AC3 GUI 打断与 API 同权同语义 | ✅ | 端点两 transport 通用;finalize 白名单 + notice 已随 P0 交付(streamEvents.ts:616-627);新增按钮不碰 Stop/Esc 链路 |
| AC4 预算 3200 + smoke 六工具 + 两套 node --test | ✅ | 实测 2881 < 3200;smoke want 数组扩两项 + 常量改 3200;test 自动跟随 |
| AC5 SSE follow 文档照做可消费 | ✅ | 机制齐全,专章零代码可达;live follow 按文档实跑可验 |

## 5. 放行条件(修订项汇总)

1. **design §2.2 + implement.md**:`coreInject` 加**前置 busy guard**(findSession 判定,
   非 busy/已收官 → 不 fireChat 直接语义报错);misfire cancel 降级为竞态兜底;AC2 表述改
   「前置 guard 根本不发起」;Phase A/B 用例补「已收官群聊注入 → 报错且 stop_reason/summary
   原样保留」(P1-1)。
2. **AC1 / implement.md**:打断断言放宽为「`preempted`(或同刻自然收官 `group_chat_end`)
   且 summary 非空」(P2-1)。
3. **P3 细节**:PRD Fact 2 引用改 `db/sessions/session_crud.rs:853`;smoke 预算行号改 `:20`
   并注明 test 自动跟随;Fact 8 补 finalize 白名单在 streamEvents.ts;design §4 锚点改
   ChatPanel template `:718-727` + 按钮可见性命名 `isGroupChat` + store action 经 controller
   访问 transport。

完成上述(修订量小,均不推翻已定决策)后,填充 implement/check.jsonl 真实条目,可执行
`task.py start` 进入实现。
