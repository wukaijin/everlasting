# Review — 08-25-f1-message-queue 规划评审

> 评审人:GLM(2026-08-25)。任务状态 `planning`,代码未动 —— 本评审针对 prd.md / design.md / implement.md 三件套。
> 方法:三件套中的代码/规范引用逐条对照仓库现状实证(§1),再评估设计完备性与 AC 可验证性(§2-§6)。

## TL;DR

规划质量高于平均水平:现状偏差描述("锁死而非丢弃""取消替换而非阻塞")全部实证成立,16 处代码引用无一虚构,决策矩阵(§4 取消清理、错误保留队列、D-D guard 核对、attachments 落盘时序)显示出对真实代码路径的深入掌握,PR 分层与回滚单元清晰。

但发现 **2 个 P1 问题必须在批准实现前解决**:

1. **路由口径自相矛盾**:PRD D7/§9 说"所有发送一律入队(统一路径)",design §1/§2 与 implement #3 却写"闲 → 注册原路径"。两个口径下错误保留队列的兑现方式完全不同,而 §4 明确承诺"下次发送统一入队自然一起注入"——只有统一入队口径能兑现。
2. **前端续轮渲染缺口**:design §3 断言"经典聊天前端 finalize 只认 Done → 中间态零改动",但 `streamEvents.ts:66-67` 会在尾部消息非 assistant 时**静默丢弃全部流事件**,`:93` 的"新 turn 推新 assistant 占位"又被 `req.groupChat` 门控。经典续轮的 Start/Delta 要么被丢、要么串进第一轮气泡。implement.md 没有任何任务项覆盖这件事,AC1 会抓到,但按"零改动"的理解去实现必然返工。

另有 5 个 P2 决策空缺与若干 P3 边缘,见 §3/§4。**结论:有条件通过 —— 解决 P1 两项、拍板 P2 后即可开工,不需要重写规划。**

---

## 1. 事实核查(引用实证)

PRD/implement 引用的代码事实逐条核对,全部成立(部分路径不精确,不影响判断):

| 引用 | 结果 | 实证位置 |
|---|---|---|
| `chatInputCodeMirror.ts:796,837` readOnly compartment 随 `sending` 置只读 | ✅ | `app/src/utils/chatInputCodeMirror.ts:796`(初始)+ `:837`(watcher reconfigure) |
| `ChatInput.vue:426-431` 发送键 disabled + Esc=Stop | ✅ | 实际路径 `components/chat/ChatInput.vue:426-434`(`sendDisabled` 含 `props.sending`;`onEscKeydown` sending 即 onStop) |
| `chat.rs:348-377` defense-in-depth 取消替换 | ✅ | `agent/chat.rs:348-377`(注释明确"normal paths 是 no-op,load-bearing defense in depth") |
| `chatSendActions.ts:229-232` 群聊 cancel+resend 抢占 | ✅ | 实际路径 `stores/chatSendActions.ts:229-232`(经典 session early-return,群聊 cancel 后续走) |
| `chatSendActions.ts:296-413` seq 乐观占位 | ✅ | 同文件 `:314-413`(nextSeq 推算 + 占位推送) |
| `state.rs:105-128` 三 map | ✅ | 实际路径 `src/state.rs`(非 `agent/`):cancellations `:105` / session_active_request `:112` / inflight_exits `:128` |
| `drive.rs:791-853` 通知逐条 APPEND 注入先例 | ✅ | `chat_loop/drive.rs:802-853`(每条通知一个 user message,req clone 尾部 push) |
| `group_chat_loop.rs:210-636` 编排外循环;MAX_ORCHESTRATION_ROUNDS=30 | ✅ | `:88` 常量、`:210` 函数头 |
| `background_shell/mod.rs:292` / `in_memory.rs:357` drain 模板 | ✅ | 实际路径 `src/background_shell/`(非 `src/agent/background_shell/`) |
| `init.rs:727-770` D-D guard | ✅ | 且注释明确 guard 要求 `group_chat_state.is_some()`,"ordinary chat never enters" —— **implement #7"排队项不应触发 guard"的判断正确** |
| `commands/sessions.rs:1370` handoff 忙拒 | ✅ | `:1362-1374`(`session_active_request.contains_key` → "当前有轮次进行中") |
| spec cache 断点(APPEND 约束) | ✅ | `.trellis/spec/backend/agent-loop-architecture.md:9-37` |
| `streamController.ts:994` groupChat 保活先例 | ✅ | `:994`(`groupChat: args.groupChat ?? false`)+ `streamEvents.ts` 群聊逐轮占位机制 |
| `memory_digest_enabled` fail-open 开关先例 | ✅ | `chat_loop/init.rs:449`(`get_config_value("memory_digest_enabled")`) |
| turn_trace per-turn 写入(AC2 验证手段) | ✅ | loop 内 per-turn 落行,续轮各得一行,`turn-smoke.sh --turns 2` 对照成立 |
| `chat` 双入口共用 `chat_inner` | ✅ | `agent/chat.rs:104` + `daemon/routes/agent.rs:59` |

补充核查(三件套未提但对评审关键):

- **`chat` 当前返回 `Result<(), _>`(Tauri)/ `Json(())`(REST)** —— `{queued:true, position}` 是真实的返回形状变更,见 P2-1。
- **REST handler 立即返回空 body,事件全走 SSE 广播**(daemon/routes/agent.rs:13-15)—— 与 design §2"本次 RPC 无流,事件仍按 session 广播"的描述吻合,排队路径在该 transport 上可落地。

## 2. P1 — 批准实现前必须解决

### P1-1 路由口径自相矛盾(统一入队 vs 忙时特判)

三份文档存在两种互斥口径:

- **统一入队**:PRD D7"所有发送一律入队,驱动器单点消费(忙排队/闲即跑统一一条路径)";design §9"选统一入队……代价是空闲路径也过一次锁+队列入出"。
- **分流特判**:design §1 伪代码"闲 → 原路径";§2"闲:注册 rid(现状逻辑),spawn 驱动器";implement #3"「查忙 + 入队 **or 注册**」收进同一锁临界区"。

矛盾点在于**错误保留队列的兑现**。design §4 承诺"provider 错误终止 → 队列保留……**下次发送统一入队自然一起注入**"。这只在统一入队口径下成立:新消息进队尾,驱动器 drain 整个队列,FIFO 一起注入。若走"闲 → 注册原路径",新消息直接作为初始输入跑、保留项要等本轮结束后才 drain——注入顺序变成"新消息在前、滞留项在后",既不是 FIFO 也谈不上"一起注入",§4 的承诺落空。

另有一个依赖同一口径的竞态:驱动器"drain 判空 → break → CancellationGuard 清理 map"与"chat_inner 查忙入队"之间存在窗口——此窗口内入队的消息会得到 `{queued:true}` 但没有消费者,只能等下一次发送冲刷。统一入队口径下这次发送会 drain 到它;分流口径下它继续排在新消息之后。

**建议**:拍板统一入队。§1 伪代码与 §2、implement #3 相应改为"闲 → 也入队 + 同临界区内 spawn 驱动器消费"。AC3 的措辞("空闲**且队列为空**")已经兼容,无需改。锁实现上注意:若复用 `session_active_request` 的 Mutex 做路由临界区,临界区内不得 await(`await_inflight_exit` 留在区外),design §2 应补这句约束。

### P1-2 前端续轮渲染缺口(经典聊天第二个 assistant 气泡)

design §3:"经典聊天前端 finalize 只认 Done → **中间态零改动**"。实证不成立,两处反例:

1. `streamEvents.ts:66-67`:`handleChatEvent` 开头 `let last = msgs[msgs.length - 1]; if (!last || last.role !== "assistant") return;` —— 排队消息的前端占位是 **user** 气泡(design §6),续轮 Start/Delta 到达时缓冲区尾部是排队 user 占位,**所有事件被静默丢弃**,续轮流式完全不可见。
2. `streamEvents.ts:93`:"第二次及以后的 `start` 推新 assistant 占位"逻辑被 `if (req.groupChat && req.groupChatStarted)` 门控——这是群聊专属机制。经典续轮即使过了第 1 关,Delta 也会**追加进第一轮的 assistant 气泡**(两轮回复拼接、无轮次分隔、注入的 user 消息不出现其间)。

需要的适配(群聊先例已给出全部素材,成本不高但必须显式规划):

- 续轮 `start` 到达且尾部为排队占位时:把排队 user 占位**物化为普通气泡**(去 queued 徽标、按持久化 seq 对齐)+ 推新 assistant 占位(泛化 `:93` 的群聊门控为"同 rid 的第二个内层 turn"语义);
- design §6 "消息真正持久化(TurnComplete 后 reload/对齐)时转普通态"的触发者要写清楚——现状经典聊天只在最终 Done 后 `reloadAfterFinalize`,若依赖它,排队徽标会挂到整个驱动器结束;以内层 `start` 为物化边界更自然;
- implement.md PR3 需新增对应任务项与 vitest 用例(续轮 Start 物化占位 / 新 assistant 占位 / 事件不再被 `:67` 丢弃)。

## 3. P2 — 实现前应拍板(均不阻塞架构)

1. **`chat` 返回形状变更未如实入账**:两入口均从 unit 变 struct。design §8"无 breaking wire rename"只对了 rename 一半——REST 响应体从 `()` 变 `{"queued":true,...}`,PWA `httpTransport.invoke("chat")` 需把返回值透传给调用方(implement #11 只提了 chatSendActions 分支,没提 transport 层)。建议 §8 改口"返回形状扩展(向后兼容字段叠加),前端同 PR 消化"。
2. **Stop toast 的 N 无来源**:R7/AC5 要求"toast 数量正确",但 `cancel_chat` 现返回 unit,design §7 IPC 三件表也没有它。要么 `cancel_chat` 返回清空条数,要么 Done(stop_reason=cancelled)携带 queue_cleared 字段——需要选定并补进 §7。
3. **edit/resend/retry 静默清队列**:design §5 该行确认这些路径"先 cancel 再发"→ cancel 已清队列,但没有任何用户反馈。用户编辑一条历史消息,排队中的 3 条输入无声消失。至少应复用 Stop 的"N 条被丢弃"toast。
4. **@@forcedDispatch 忙时语义未定**:QueuedMessage 结构只有 text/attachments/enqueued_at/priority,装不下 request 级 forcedDispatch;design §5"留实现期验证"应现在拍板。建议 MVP 最简:**经典 session 流式期间输入 @@ 前缀 → 前端直接拒绝并 toast**(派发语义与延迟注入组合本来就不清晰),并在 R2 补一句。
5. **Esc=Stop 解锁后的误触**:解锁后用户正在打字,Esc(关面板/习惯键)即 Stop+清队列。design §6/implement #10 只写"复核"。建议拍板默认:经典 session 流式期间编辑器**有焦点或非空**时 Esc 不触发 Stop(或干脆仅群聊保留 Esc=Stop)。

## 4. P3 — 边缘与建议(记录即可)

- **搁浅竞态兜底**(依赖 P1-1 口径):驱动器 break 后、guard 清理 map 前入队的消息要等下次发送冲刷。可在驱动器退出前做一次"清理 map → 再查队列非空 → 重 spawn"的收尾,或接受窗口(毫秒级)并记录。
- **续轮上限 50 触顶后剩余队列处置未定义**(implement #4):建议语义 = break + Done + 队列保留(同错误终止),并在 §4 矩阵加一行。
- **错误终止后排队滞留无提示**(§9 已接受 trade-off):建议 error Done 后 toast"N 条排队消息保留,下次发送时注入",一行前端代码消除"排队徽标挂着但无事发生"的困惑。
- **跨端非实时**(入队/清队不广播,他端靠 `list_queued_messages` 水合):MVP 已接受;注意 Stop 在 A 端清队后 B 端仍显示排队项,直到下次水合——若 PWA 双端是真实使用场景,这条的刺比 §9 预估的更尖,留 follow-up 档评估入队/清队事件广播。

## 5. 做得好的(值得保持)

- **现状偏差实证**:"前端不止丢弃是锁死""后端是取消替换而非阻塞"两条对 ROADMAP 原述的修正都查了真实代码,defense-in-depth 的定位(chat.rs:348 注释"normal path no-op")理解准确。
- **决策都有理由且可追溯**:D1-D7 表、§4 取消矩阵、§9 trade-offs 三件套互相咬合(除 P1-1 那一处)。
- **D-D guard 核对**(implement #7)是容易被漏的深坑——注入的排队项是未落库的新消息,guard 需 `group_chat_state` 才触发,普通路径确实不会误伤,预判正确。
- **注入形态选"每条独立 user message APPEND"**:与 drive.rs 通知循环同构,wire 层零改动、cache 断点不变量自动满足,是最低风险路径。续轮若按"extend messages Vec 后原签名重入 run_chat_loop"实现,persist/attachments/memory_recall 查询(取最近 user 消息)全部免费正确。
- **风险文件表与回滚点**、验证命令与 AGENTS.md 的 WSL 说明一致,PR4 含 ROADMAP/ARCHITECTURE/spec 归档,符合仓库惯例。

## 6. AC 可验证性

| AC | 评估 |
|---|---|
| AC1 | 会直接暴露 P1-2——这正是它的价值;建议 vitest 补"续轮 Start 后事件不再被 `:67` 丢弃"的定点用例 |
| AC2 | turn_trace per-turn 落行已实证,turn-smoke 对照手段成立;注意对照基线要在同一 session 形态(有无排队)下取 |
| AC3 | 措辞("队列为空")已兼容统一入队口径,无需改 |
| AC4/AC5/AC6/AC8/AC9 | 后端可测性良好(harness + fake provider 先例俱在),AC5 依赖 P2-2 的 N 来源 |
| AC7 | 依赖 P2-1 的 transport 返回值透出;建议集成测试覆盖 REST 排队分支(daemon routes 层) |

## 7. 结论

**有条件通过。** 规划的事实基础、架构选择(统一入队、单 rid 保活、in-memory 风险姿态)都是对的,PR 切分合理。批准前完成:

1. 解决 P1-1:三处文档统一为"闲也入队",§4 承诺随之自洽;
2. 解决 P1-2:design §3 撤回"中间态零改动"论断,§6/implement PR3 补续轮物化与新 assistant 占位的显式任务;
3. 拍板 P2 五项(chat 返回形状入账 / Stop 的 N 来源 / edit-resend 清队 toast / @@ 忙时拒绝 / Esc 默认行为),写入 design 对应小节。

完成后可按 implement.md 顺序开工,PR1 后端队列模块本身无争议。

---

# Round 2 — 开发完成评审(2026-08-25,commit `92a480b`)

> 对象:F1-A 实现 commit `92a480b`(67 文件,+2616/−112)+ 修订后三件套。方法:通读全部核心 diff(后端 chat.rs/message_queue.rs/驱动器,前端 streamEvents/messageQueueStore/ChatInput/MessageItem),关键断言以临时测试实证(已还原),全套测试本地复跑。
> 结论先行:**不通过,需修复后重审**。发现 1 个 P0(生产环境经典聊天流式输出完全不可见,已实证)+ R8/R4 的一组前端占位生命周期缺陷。P0 修复量约几行;根因是 PR4 收尾验证(live 冒烟)未执行就提交。

## TL;DR

规划评审的两处 P1 修正都被高质量采纳——P1-2 的落地设计(新增 `TurnContinuation` 事件而非泛化 `start` 门控)甚至优于评审建议,实现者抓住了规划评审漏掉的细节(`start` 是 run 内每次 LLM 调用边界,泛化会拆散多工具轮)。后端路由临界区、驱动器循环、取消矩阵接线实现质量高。

但 **`DriverSink` 丢事件(P0)** 让整条链路在生产上不可见:除 `Error` 外的所有事件(Start/Delta/TurnComplete/FileInjections/Recall…)都不会到达前端——`queue` 默认开启,所有经典聊天从此没有流式输出,直到 Done 触发 reload 才一次性显出全文。该 bug 已用临时单测实证(断言 Delta 穿透包装器,收到 0 条,失败)。4 个新集成测试恰好都断言绕过或巧合穿透包装器的通道(Done/TurnContinuation 由驱动器直发 inner sink;Error 是包装器唯一转发的分支),所以测试全绿也没抓住。

## 验证结果(本地复跑)

| 项 | 结果 |
|---|---|
| vitest | ✅ 1189/1189 |
| `cargo test -p everlasting --lib` | 1969 过 / 1 失败(`plan_mode` 满载 flaky;单独重跑通过;commit 声称干净 HEAD 基线亦有,与前一 commit 15555c0 修的同族慢 CI 竞态,接受) |
| DriverSink Delta 穿透(临时单测) | ❌ **失败:0 条到达 inner sink**(测试已还原,工作区干净) |
| PR4 #16-18(live 冒烟/文档归档) | ⬜ 未执行(implement.md 未勾,ROADMAP 自述"留 follow-up") |

## P0(阻塞)— DriverSink 丢弃全部常规事件

`app/src-tauri/src/agent/message_queue.rs:318-339`:

```rust
fn forward(&self, payload: &ChatEventPayload) -> bool {
    match &payload.event {
        ChatEvent::Done { .. } => { /* 记录 */ false }        // 吞,正确
        ChatEvent::Error { .. } => { self.inner.emit_chat_event(payload); /* 置位 */ true }
        _ => true,                                             // ← 只返回 bool,没转发!
    }
}
impl ChatEventSink for DriverSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        self.forward(payload);   // ← 返回值被丢弃
    }
}
```

`_ => true` 分支不调用 `self.inner.emit_chat_event`,而 `emit_chat_event` 又忽略返回值——除 Error 外全部事件被吞。修复(注意 Error 分支要同时改为不自转发,否则按返回值转发会双发):

```rust
ChatEvent::Error { .. } => { self.status.lock()...errored = true; true }   // 去掉自转发
...
fn emit_chat_event(&self, payload: &ChatEventPayload) {
    if self.forward(payload) { self.inner.emit_chat_event(payload); }
}
```

并补一条单测锁契约:`DriverSink` 收 `Delta` → inner sink 收到 1 条(我的临时测试可直接复用)。**修复后必须跑一次 PR4 最小 live 冒烟**(重编 daemon + `turn-smoke.sh --turns 2` + 手工连发)——这个 P0 在任何一次真机流式里第一秒就会被发现,逃逸的唯一原因是收尾验证没做。

## P1(功能不可用)— R8 前端占位生命周期未闭环

三个关联缺陷,同根(占位气泡与 queue store 双份状态无同步):

1. **撤销/退回不移除占位气泡**:`messageQueueStore.revoke/recallToComposer` 只更新 `queuedBySession`,不碰 `messagesBySession` 里的占位 → 操作后"排队中"气泡残留到下次 reload。
2. **position 寻址漂移 → 撤错条/静默失效**:占位的 `queued.position` 在发送时定死;store 端撤销后位次重排,占位不更新。`MessageItem.vue:88/98` 的 `entriesFor(sid)[position-1]` 随即错位——轻则落进 `if (!entry) hydrate; return` 静默 no-op(撤销任意一条后,其右所有气泡的撤销/退回全部失灵),重则位次碰撞时**操作到错误条目**。AC8/AC9 的实操场景必现。
3. **recall 草稿不回填当前 session**:`recallToComposer` 写 `recallDraft`,但唯一消费点是 `ChatInput.vue` 对 `currentSessionId` 的 watch——当前 session 内点"退回输入框"composer 不回填(AC9 不达);草稿滞留,用户之后切走再切回时幽灵回填。

建议修法(一并解决三缺陷):后端 `ChatAcceptance::Queued` 增加 uuid 字段(wire additive;`push` 已生成 id 只是没返回),占位改存 id,撤销/退回按 id 直达后端(彻底去掉 position 寻址);成功后同步移除对应占位气泡;回填改为 watch `recallDraft` 本身或经事件回调直达 composer。

## P2 — R4 水合后无渲染(刷新/第二端排队项不可见)

`hydrate()` 只填 `queuedBySession`,而 `entriesFor` 的消费方只有 toast 计数与 MessageItem 寻址——**没有任何组件把水合结果渲染成占位气泡**;`rehydrateMessages` 不含 queued(占位纯内存),刷新后 messagesBySession 从 DB 重建自然没有它们。后果:页面刷新 / LRU 驱逐 / PWA 第二端——排队项后端在队、前端不可见,直到注入落库才随 reload 出现。日常单端切走再切回(LRU 内)时内存占位还在,感知不到;踩中的恰是 R4 声称覆盖的核心场景。修法:hydrate 成功后把 entries 物化为占位气泡 append 到该 session 消息尾部(与 turn_continuation 物化同构)。

## P3 — 记录在案(不阻塞)

- **implement #9 勾与实况不符**:三个子项(错误保留 + 新发送 FIFO 一起注入的顺序回归、反搁浅断言、AC4 chat_inner 层回归)无对应测试——4 个新测试均未覆盖;路由临界区本身零测试(测试头注已承认押给 live 冒烟,而 PR4 未执行)。至少应改勾并注明覆盖方式。
- **turn_continuation 分支缺 design §3 ③ 的 `sealActiveThinking`/`flushPendingTimelineText` 兜底**:主路径由 `turn_complete` 的 seal 兜住,但防御性应补(与自身设计文档对齐)。
- **design §8"闲且队列为空逐字节对齐现状"已不成立**:闲路径机制变为 入队→驱动器→DB reload(客户端 history 弃用)。语义等价(DB SoT,群聊 D-B 同构),但应改口径;附带记录:若未来出现"仅存于客户端 history、未落库"的请求内容会被 reload 丢弃(当前无此形态)。
- **驱动器退出 `session_active_request.remove(&sid)` 无 rid 守卫**:近不可能场景(queue 运行中途被关闭/同 session 混入 legacy 3a 替换)下会误删新注册;`if map.get(&sid) == Some(&rid)` 一行更稳。
- **跨 session edit/resend/retry 无清队 toast**(走 `controller.cancel` 而非 `chatStore.cancel`;当前 session 路径已覆盖,P2-3 常见面达标)。

## 规划评审落实核查(Round 1 → 实现)

| 项 | 落实情况 |
|---|---|
| P1-1 统一入队 | ✅ 临界区实现 + 锁序文档化(queues→active,区内无 await)+ preflight 失败全回滚且按 uuid 只弹自己那条;顺序回归测试缺(P3) |
| P1-2 续轮渲染 | ✅ `TurnContinuation` 方案优于评审建议(实现者指出 `start` 是 run 内 LLM 调用边界——规划评审时我漏掉的);handler 挂尾部守卫之前、不泛化群聊门控;vitest 定点回归质量高。**但被 P0 连累,整链当前不可见** |
| P2-1 返回形状 | ✅ `ChatAcceptance` additive(`{status:"started"|"queued",position}`)+ http transport 透传 + daemon `Json(acceptance)` |
| P2-2 Stop toast N | ✅ `CancelOutcome.clearedQueued`,Tauri + REST 双入口 |
| P2-3 edit/resend/retry toast | ✅ 当前 session 路径(`chatStore.cancel` 统一);跨 session 缺(P3) |
| P2-4 @@ 忙时拒绝 | ✅ D8 拍板 + 前端拒绝 toast + 后端纵深防御(驱动器 round>0 强制 `forced_dispatch=None`) |
| P2-5 Esc 规则 | ✅ 经典流式中编辑器非空不触发 Stop;群聊不变 |
| P3 反搁浅 | ✅ 退出协议 + 注释说明滞留项随新驱动器首轮 drain 一起注入 |
| d4f D-2(sendDisabled) | 实现顺带修正——d4f 称"AC1 未要求流式中可发送"系误读(AC1 明写"流式期间可成功发送 ≥2 条"),实现者驳回正确 |

## 做得好的

- `chat_inner` 路由临界区是整个 PR 质量最高的部分:锁序全仓文档化、区内零 await、忙/闲/legacy(群聊/关开关/无 user 尾条)三分干净,legacy 与 F1 前逐字节一致(AC4/AC3 的 legacy 面)。
- 驱动器以 DB reload 为 SoT(群聊 D-B 同构)让 D-D guard 天然不触发、attachments 经 metadata 重建——implement #7/#8 的两个"核实"项被架构选择直接消解。
- `run_chat_loop` 31 参 + 70 调用点机械补 `false` 的改动纪律好,群聊两处调用点注释明确(guard 仍是唯一清理者)。
- 集成测试手法干净(`HangingThenCancel` 挂起破环、late-enqueue watcher 模拟忙时连发);vitest `messageQueueStream.test.ts` 把 P1-2 契约锁得很准。
- 取消矩阵接线完整:cancel(rid→session 反查清队)/delete_session/clear_session_messages/edit_user_message(返回 `EditMessageOutcome`)全齐。
- commit message 里显式记录评审甄别(采纳/驳回)——这是好的工程文化。

## 重审门槛

1. 修 P0(DriverSink 转发 + Error 分支去自转发)+ 补 Delta 穿透单测;
2. 修 R8 三连(建议按 id 寻址方案)与 R4 水合渲染;
3. 跑 PR4 最小 live 冒烟(重编 daemon → `turn-smoke.sh --turns 2` → 手工长 turn 连发 3 条含撤销/Stop)——P0 类 bug 只有真机能兜底;
4. P3 清单酌情(至少 implement #9 的勾改实况)。

---

# Round 3 — 修复落地记录(2026-08-25 同日,评审人执行)

Round 2 的门槛 1/2/4 已修复并验证(工作区未提交,14 文件);门槛 3(live 冒烟)仍开放。

| 项 | 修复 | 验证 |
|---|---|---|
| P0 DriverSink | `emit_chat_event` 按 `forward` 返回值转发;Error 分支去自转发(防双发) | 新单测 2 例(常规事件穿透序 / Error 恰一次)+ driver 集成测试 2 处补 Delta 到达断言,全过 |
| P1 寻址漂移 | `ChatAcceptance::Queued` 加 `id`(wire additive);占位 `queued:{id,position}`;`revoke`/`recallToComposer` 按 id 直达;新增 `dropQueuedPlaceholder` 删占位并重排位次(拒绝删非排队行) | vitest 新增 3 例(删行重排 / 拒删持久化行 / 按 id 调 IPC) |
| P1 回填 | ChatInput 改 watch `recallDraft` 本身——当前 session 内 recall 立即回填 | vitest(recall 草稿取后端原文 + 一次性消费) |
| P2 水合可见性 | `hydrate` 返回 entries → ChatPanel 接 `materializeQueuedPlaceholders`(按 queued.id 去重物化) | vitest(刷新形态物化 + 去重 + 位次重排) |
| P3 三项 | turn_continuation 补 seal+flush(design §3 ③);驱动器 slot 注销加 rid 守卫(`retain`);design §8 AC3 口径修正 + implement #9 覆盖如实化 | 既有 8 例 vitest 回归过 |

全套验证:vitest 1195/1195(含 6 新增)、`vue-tsc --noEmit` 零错、`cargo test --lib` 1971 过/1 失败(`plan_mode` 满载 flaky,单独重跑通过,修复前同现象,预存)、clippy 仅 2 条预存警告(经干净 HEAD 对照确认非本次引入)、`cargo fmt --check` 干净。

遗留:PR4 #16-18(live 冒烟 + curl REST 排队分支 + 文档归档)待执行——重审门槛 3 仍然有效。

---

# Round 4 — 收尾确认(2026-08-25 归档日,补记)

重审门槛 3 关闭:live 冒烟 + curl REST 排队分支由用户(carlos)归档当日真机实测通过(P0 DriverSink 修复获得真机兜底);任务文档归档至 `archive/2026-08/08-25-f1-message-queue/`。F1-A 评审全链闭环。后续仅剩 ARCHITECTURE/spec 深度沉淀(锁序文档化等)留 follow-up。
