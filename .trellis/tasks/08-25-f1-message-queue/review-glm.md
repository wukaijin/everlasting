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
