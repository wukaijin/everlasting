# 规划评审 — F1 消息队列(用户连发档 MVP)

> 评审日期:2026-08-25(规划阶段,未实施)。对象:prd.md / design.md / implement.md 三件套。
> 方法:对照真实代码逐条核验文档引用的文件与行号;检查 PRD R1-R8 / AC1-AC9 ↔ design §1-§9 ↔ implement PR1-PR4 的对应关系与缺口。

## 总体评价:✅ 规划扎实,可批准进入实现

**文档引用的代码事实全部核验通过**(后端 12 处 + 前端 8 处 + spec 3 处,无一失效)。范围决策(D1-D7)、竞态收口(design §2 单锁临界区)、续轮协议(design §3 单 rid 保活)、取消矩阵(design §4)均自洽。前端解锁方案经实测确认**只改 CodeMirror `readOnly` 判定即可达成 R1**(见 🟡 D-2,有隐含但可接受的后果)。

评审未发现 🔴 级阻塞问题。3 项 🟡(一个设计缺口、一个口径缺口、一个实现期未知)与 2 项 🟢 建议,均不阻塞启动,建议在实现期按下方闭环处理。

---

## ✅ 代码事实核验(全部通过)

| 文档引用 | 实测 | 结果 |
|---|---|---|
| `chatInputCodeMirror.ts:796,837` readOnly | 796 行 `readOnly.of(opts.sending...)`、837 行 reconfigure,均存在 | ✅ |
| `ChatInput.vue:426-431` sendDisabled/Esc | 426-427 `sendDisabled`、430-431 `onEscKeydown`,均存在 | ✅ |
| `chat.rs:348-377` cancel_inflight | 343-368 为 3a 防御性取消注释 + 调用,行号精确 | ✅ |
| `state.rs:105-128` 三 map | cancellations/session_active_request/inflight_exits 注释块在 105-128 | ✅ |
| `drive.rs:788-850` 注入 seam | 785-855 含通知循环、checklist、缓存注释,吻合 | ✅ |
| `drive.rs:791-853` 连续 user 消息先例 | 逐条 `role:user` APPEND 形态实存 | ✅ |
| `background_shell/mod.rs:292`、`in_memory.rs:357` drain 模板 | `drain_notifications` 声明/实现行号精确 | ✅ |
| `group_chat_loop.rs:210-636` 编排器 | 文件 667 行,外循环 + 单 rid 保活 + 非 terminal Done 实存 | ✅ |
| `group_chat_loop.rs` MAX_ORCHESTRATION_ROUNDS=30 | `:88` `const MAX_ORCHESTRATION_ROUNDS: usize = 30;` | ✅ |
| `streamController.ts:994` 保活先例 | 994 `groupChat: args.groupChat ?? false` | ✅ |
| `chatSendActions.ts:229-232` 群聊抢占 | 229-231 `isCurrentSessionStreaming` + `cancel()` | ✅ |
| `chatSendActions.ts:296-413` 占位 UI | 294-300 seq 计算、405-415 占位 push,吻合 | ✅ |
| `init.rs:727-770` D-D guard | 727-770 注释块 + guard 逻辑实存 | ✅ |
| `commands/sessions.rs:1370` 忙判定 | 1362/1370 均引用 `session_active_request` | ✅ |
| `agent-loop-architecture.md:9-37` 缓存断点 | 9-37 含 APPEND 不变量 + cache_control 契约 | ✅ |
| `tool-contract/06-background-shell.md:144-161` | 144-168 通知逐条 user 消息 + APPEND 契约 | ✅ |
| `DESIGN.md:117` §3.2 硬约束 | "主动权必须在本地用户",实存 | ✅ |
| daemon 路由与 chat_inner 共用 | `daemon/routes/agent.rs:22,59` 直接调用 `chat_inner`(REST 与 IPC 双入口确为单份逻辑) | ✅ |

---

## ✅ 结构一致性核验

- **R1-R8 ↔ AC1-AC9 ↔ design §1-§9 ↔ implement PR1-PR4**:全部一一对应,R3 的 AC2(APPEND 缓存不劣化)有明确验证命令(turn-smoke 对照)。✅
- **design §5 入口矩阵 ↔ PRD Background 现状**:7 类入口行为逐一落地,群聊/handoff/编辑重发/@@派发均明确归类。✅
- **D2/D5/D7 决策直接物化为架构**:统一入队(§1)、单 rid 续轮(§3)、defense-in-depth 替换清队(§4)——决策与设计无漂移。✅
- **上限常量取值自洽**:`SESSION_QUEUE_MAX=20`(AC6)与续轮上限 50(参照群聊 30)合理。✅
- **无 DB migration、无 breaking wire rename、单模块独立 + 开关 fail-open + 单 commit 回滚**:回滚单元清晰。✅

---

## 🟡 需注意(不阻塞启动,实现期闭环)

### D-1(设计缺口):错误终止后"队列保留但不续轮",队列无过期机制,可能滞留至下次发送才消化

design §4:provider 错误终止 → 队列保留、不续轮,靠"下次发送统一入队自然一起注入"消化。但若错误后用户**不再发送**(改为 Stop 或直接切走),队列中的消息会无限滞留——R4 视图水合会让它们在 UI 上永久显示"排队中"。这虽是设计有意接受(§9 trade-off:"错误后队列滞留需靠下次发送或 Stop 消化,无专门 UI"),但 R4 的排队视图 + Stop 清空会产生一个**语义小裂缝**:UI 显示"排队中"但实际永不续轮,且用户无感知。

建议:实现期在排队视图上为滞留队列加一行状态提示(如"等待下次发送时注入")或在下一次任何发送/Stop 时自然消化;不新增专门 UI。若实现期发现 UI 误导,升为 P2。

### D-2(口径缺口):R1"解锁"只改 CodeMirror readOnly 判定,发送按钮与 Esc 行为不变(已实测)

按 design §6 改 `readOnly` 判定为 `sending && isGroupChat` 后:
- CodeMirror 可输入(✅ R1 达成);
- 但 `ChatInput.vue` 的 `sendDisabled()`(`:426` `props.sending || ...`)与 `onEscKeydown()`(`:430` 流式中 Esc → Stop)均只认 `props.sending`,不受影响——即**发送按钮流式中仍显示 Stop、回车无法发送**。

这意味着 R1 的实际 UX 是"流式中可打字、但需等 Stop/轮结束才能发送"——与 AC1"流式期间可成功发送 ≥2 条"**不冲突**(AC1 未要求流式期间发送,只要求"流式期间可打字"+"当前轮结束后自动续轮注入"),但比"流式中即可发送"弱。**若产品意图是流式中就能点发送**,则 sendDisabled 判定也需改为 `sending && isGroupChat`(群聊保持锁),这属于 design §6 未覆盖的小改动,建议实现期与 R1 一并确认。当前文档自洽,不阻塞。

### D-3(实现期未知):@@强制派发在续轮注入时的展开语义

design §5 注明"forcedDispatch 语义在续轮注入时的展开留实现期验证"。续轮注入绕过 send() 前端路径,@@ 的"强制派发"如何作用于注入后的 turn 需要实现期定。design 已显式标注为待验证,合理,但 implement PR1/PR2 未列出该项的验证步骤——建议在 PR2 集成测试中补一条(或明确记录为接受现状)。

---

## 🟢 建议(可选)

- **G-1**:implement PR1 步骤 2 提到"数据面组内,catalog-after-db 不变式不触碰"——`state.rs` 的 AppState 字段是否需同步更新 schema/catalog 类文档,建议实现期 grep 确认(design §8 已声明无 DB migration,应无影响)。
- **G-2**:R6 `priority: u8` 字段仅为占位,建议在 `QueuedMessage` 注释中显式标注"MVP 纯 FIFO,优先级字段不参与调度"以防未来误用(PRD 已写清,属锦上添花)。
- **G-3**:AC7 的 REST 验证在 implement 中未单列命令,建议 PR4 live 冒烟时用 curl 打 `POST /api/v1/agent/chat` 验证入队分支(design §1 已保证逻辑共用,属验证覆盖建议)。

---

## ✅ 做得好的地方

1. **代码引用精确到行号且全部实测通过**——`chat.rs:348-377`、`drive.rs:791-853`、`background_shell/in_memory.rs:357` 等引用经核实一字不差,极大降低实现期踩空风险。
2. **竞态收口设计正确**:把"查忙 + 入队/注册"收进单一锁临界区(design §2),直接消灭了"两个并发 chat 都读到闲"的既有竞态,且 group_chat 与 defense-in-depth 语义清晰分离。
3. **续轮协议复用成熟先例**:单 rid 保活跨内层 turn(group_chat 已实现)+ 逐条 APPEND 注入(drive.rs 通知循环已实现),wire 层零改动,风险面小。
4. **回滚设计务实**:队列独立模块 + 单点分流 + fail-open 开关 + 单 commit 可 revert,符合本项目"可回滚单元"惯例。
5. **风险文件表**:chat.rs / init.rs / chatSendActions.ts / chatInputCodeMirror.ts 的风险点与回滚方式逐条对应,与实测代码位置一致。

---

## 结论

**批准进入实现**。按 implement.md 的 PR1→PR4 顺序执行;实现期闭环 D-1(错误滞留的 UI 提示)、D-2(确认解锁口径是"仅可打字"还是"可打字+可发送")、D-3(@@ 展开验证)。建议实现完成后以本文件为准做实施核验(届时对照 AC1-AC9 逐条实测)。
