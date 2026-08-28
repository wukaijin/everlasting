# F2 定时任务 — 评审记录

> 评审对象:task `08-28-f2-scheduled-tasks`(planning 阶段)的 `prd.md` / `design.md` / `implement.md`。
> 评审方式:三件套引用的取证锚点逐一与真实代码(main `0eea026`,2026-08-28)核对 + 调度/去重/落账语义推演。本轮为 brainstorm 外部评审(已闭环 P0/P1 项)之后的第二道 artifact review 门(workflow §1.4 前)。
> 结论:**方案成立,「近似纯编排层」的定位与代码事实吻合,可进入实施;1 处实施前必须定案的调度语义缺口(P1,interval 网格锚点 → 相位漂移)+ 3 处需补定案/点名的设计边角(P2)+ 5 处细节(P3)**。P1 不需要推翻任何结构,是 compute.rs 动工前的一行定案。

## 结论概要

三件套质量高于平均水平:上一轮评审修订的四个要点(origin 载体必须在 QueuedMessage、单一扫描算法、落账丢失窗口、RULE-QUEUE-001 登记)全部落进文档且与代码事实一致。本轮新增核对的链路全部吻合:前端可见性链(R6)比 design 写的还要顺——`MessageRow.metadata` 经 rehydrate **整体透传**(streamRehydrate.ts:301 `msg.metadata = meta`),`ChatMessage.metadata?: Record<string, unknown>`(chat.types.ts:437)已存在,MessageItem 已有泛型读 metadata 的先例(edited_at / compaction_summary),「定时」标识零 rehydrate 改动即可渲染。mock LLM 集成测试先例成立(`MockProvider` llm/provider/mock.rs:238 + agent tests_common harness);daemon route oneshot 测试模板存在(routes/sessions.rs:411 起,spec §6 明文要求)。ChatEntry 全库仅 2 处构造点(routes/agent.rs:61、chat.rs:107),「补 None」工作量可忽略。

核心风险集中在 **interval 档位的调度网格锚点**:design 的 `most_recent_due(schedule, now, not_before)` 签名里没有锚点参数,D2 的 schedule JSON 也没有锚点字段——按现有签名实现,interval 网格只能锚定 `not_before`,而 §4 伪代码两处 `last_fired_at = now` 落账会让相位每周期漂移并单调累积(详见 P1)。

---

## P1(实施前必须定案)interval 调度网格锚点缺失;`last_fired_at = now` 落账会产生单调累积的相位漂移

design §3 写「interval 按 k 逆推」但未定义网格锚点。daily/weekly 的网格是自然时间(每天/每周 X 的 HH:MM),无锚点问题;**interval 网格必须有一个锚点**(`anchor + k·N` 的 k=0 时刻)。现状签名 `most_recent_due(schedule, now, not_before)` 里 schedule JSON 只有 `every_min`,锚点只能自然落在 `not_before` 上,而:

- §4 伪代码 fire 分支落账 `last_fired_at = now`(实际触发时刻,非理论到期点 due);
- tick 30s 粒度下 `now - due ∈ [0, 30s)`,若网格锚定 `not_before`(= last_fired_at),每个周期相位后移 `now - due`,**单调累积,永不回正**——1min interval 任务跑数小时后实际间隔趋近 90s 且持续恶化;
- dedup 分支同样 `last_fired_at = now`,把「跳过」记在任意时刻(必然非网格点),同样推漂网格;
- 「去重前移防每 tick 重判」的设计意图(§4.2)不受影响——前移到 due 一样能防重判。

**修正(推荐,改动最小)**:两处落账改为 **`last_fired_at = due`**(理论到期点),语义升格为「最近一次已消费的到期点」。这样 `not_before` 恒在网格上,锚定 not_before 也稳定,`most_recent_due` 签名不变;catch-up 判定语义反而更精确(「到期点是否已消费」而非「何时消费的」)。替代方案:签名加 `anchor`(= created_at)参数并把网格锚死在 created_at——也可行但动签名。**无论选哪个,§8 纯函数单测必须加一条不变量:1min interval 连续模拟 fire,断言相邻 fire 的 due 间隔恒等于网格步长(无累积漂移)**——这是把本条锁死的回归测试。

## P2-1(需定案)TaskOrigin 的 serde 与「不上 wire」自相矛盾;`QueuedMessage: Serialize` 会把它带进 `list_queued_messages` IPC

`QueuedMessage` derive 了 `Serialize`(message_queue.rs:55,服务 R8 排队占位 hydration 的 `list_queued_messages` IPC)。给它加 `origin` 字段,`TaskOrigin` 就**必须**满足 `Serialize`(字段类型约束),于是 origin 必然出现在该 IPC 的返回体里。design §4.1 给 TaskOrigin 标了 `#[serde(tag = "kind")]`、§6 又写「TaskOrigin 不上 wire(进程内类型)」——两处不能同时成立。定案二选一:

- **(推荐)顺势上 wire**:排队中的定时消息前端占位可显示「定时」徽标,补齐 R6 的中间态可见性(排队中 → 注入 → done 后权威行,三个状态都有标识);成本为零。
- `#[serde(skip)]` 保「不上 wire」:多写一个属性,前端排队占位无定时标识。

改 design §6 一句话即可。

## P2-2(需点名)同一 session 多任务同 tick 齐到期 = RULE-QUEUE-001 的**确定性**触发器

R4 去重是任务粒度(peek 只查「本任务上次条目」),跨任务不去重。两个任务绑同一 target session 且同 tick 到期(最典型:两个 daily 都在 09:00),同轮 drain 两条,**非尾条的 prompt 必然不落库**、其「定时」标识随失——不是概率放大,是每天必现。design §9 对 RULE-QUEUE-001 的表述是「去重降低滞留概率」,未覆盖这个零概率成本场景。建议(按成本递增,任选其一并写进 §9):

1. **零成本**:接受 + 把 implement.md AC8 的「多 drain 钉现状」测试场景从「scheduled 与手动」扩成**也含「scheduled + scheduled」**;
2. 低成本:create/update 加软校验「同 target_session 已有 enabled 任务时 UI 警示」(不硬拒);
3. 硬约束:同 session enabled 任务上限 1(过严,不推荐——用户可能就要两个不同周期的汇报任务)。

倾向 1(+2 可选):DEBT 根治前这是与手动连发同级的既有缺陷面,钉住行为即可。

## P2-3(需定案)disable→enable 的存量补跑语义未定义

`not_before = max(created_at, last_fired_at)` 不含 enabled 状态。任务 disable 一周后 enable,首 tick `most_recent_due` 会命中 disable 期间错过的最近到期点 → **立即 catch-up fire 一次**。用户对「重新启用」的预期更可能是「从下一个到期点开始」。两个自洽的定案:

- (a) 接受「enable 即补跑最近一次」——与 D4 停机补跑语义同构(把 disable 视为用户主动的停机),UI 文案说明;
- (b) `update_scheduled_task` 在 enabled 从 false→true 时把 `last_fired_at` 前移到 now(= 跳过存量,从下个到期点开始)。

倾向 (a)(零特例代码,语义正交:调度器永远只看「有没有未消费的到期点」);但必须写进 design §3/§6,否则 AC4 的测试编写者无从选择。

## P3-1 lost 审计的清队点覆盖面要写边界

`clear_session` 调用点全库 5 处:chat.rs:1126(驱动器 cancel break)、commands/cancel.rs:69(Stop 命令)、commands/sessions.rs:313 / 472 / 940(delete_session / clear_session_messages 等破坏性命令)。design §4.4 只点名 Stop。建议明确:lost 审计只覆盖前两处(Stop 语义);sessions.rs 三处属于会话/消息本身在销毁,任务同步级联或内容清空是有意行为,不审计(其中 delete 场景 audit 连挂靠 session 都不存在)。实现提示:`clear_session` 只返回 count 不返回条目,识别「带 origin 条目」需清队前 `list_session` 快照(两次 await 之间驱动器可能 drain,best-effort 可接受)。

## P3-2 QueueError::Full 落 `error` 审计语义糙

fire 遇队列满(20)→ chat_inner 返回 Err → audit `action=error`。去重保证单任务自队最多 1 条,满队需其他来源 19 条滞留,概率极低;但真发生时 `error` 会误导排障。可选:payload 加 `reason: "queue_full"` 区分,或复用 `skipped_dedup`;接受现状也可,写一句即可。

## P3-3 前端 audit kind 人话映射未列入 WP2 清单

AuditLogItem 走 `labelForKind(props.row.kind)`(AuditLogItem.vue:186 → utils/audit.ts)。新增 `scheduled_task_fired` 不加映射,AC5 的「审计查询 UI 可见」会显示原始蛇形串。把「utils/audit.ts labelForKind + icon 加一档」补进 implement.md WP2。

## P3-4 两个未定常数

- catch-up 判定的「宽限」(§4:`due 显著早于 now - 宽限`):建议 2×tick = 60s,写进 design;
- `record_audit_event` 带 `turn_seq` 参数,scheduler 非轮内上下文:传值(0 或 None,按签名)在 implement 时定,勿留 TODO。

## P3-5 next_fire_at NOT NULL × enabled=false 的展示值

DDL `next_fire_at INTEGER NOT NULL`:停用任务也必须有值(存「按 schedule 的下一个到期点」即可,反正不参与判定);UI 停用行灰显。提示性,无行为风险。

---

## 取证核对(确认无误的关键点)

| 锚点 | 核对结果 |
|---|---|
| `chat_inner` 统一入口(chat.rs:191-194) | ✅ `pub(crate) async fn chat_inner(state, entry) -> Result<ChatAcceptance, _>`;解构在 :195 |
| 「闲也入队」路由临界区(chat.rs:306-383) | ✅ 查忙+入队+认领同临界区;忙时仅入队返 `Queued{position}`;锁纪律「先 queues 后 active_request、区内无 await」与 module doc 逐字一致 |
| `ChatAcceptance::Started` 不带 uuid、`Queued{id, position}` 带 | ✅ chat.rs:163-166;design「uuid 仅 Queued 路径可得」成立 |
| 驱动器 drain 全队进 turn 输入(chat.rs:1069-1072) | ✅ `turn_messages.extend(drained.iter().map(\|qm\| qm.message.clone()))`;RULE-QUEUE-001 的「LLM 单次看到」半边属实 |
| persist 只写尾条 user(init.rs:783-784) | ✅ `messages.iter().rev().find(\|m\| m.role == Role::User)`;RULE-QUEUE-001 全量属实 |
| round>0 丢弃请求级上下文(chat.rs:1076-1080) | ✅ `(round_resend, round_forced)` round>0 恒 (None, None)——「origin 载体必须在 QueuedMessage」的推理前提成立 |
| Stop 清队(chat.rs:1124-1127) | ✅ `clear_session` 在 `token.is_cancelled()` break 分支 |
| QueuedMessage 现 4 字段(id/message/enqueued_at/priority) | ✅ message_queue.rs:55-70;`origin` additive 可行(但见 P2-1 serde 交互) |
| 队列上限 20(SESSION_QUEUE_MAX) | ✅ message_queue.rs:47;push 超限返回 `QueueError::Full` |
| init metadata 门控 `(!injections \|\| has_attachments) && snapshot.is_some()`(init.rs:972-1003) | ✅ 信封条件构造 `{injections[, attachments]}`;`update_message_metadata` 单写路径、整体覆盖 |
| origin 门控不会重复写 metadata | ✅ 推演:首轮 persist 后次轮 reload,`already_in_db` 命中 → snapshot None → 写入跳过;后续轮携带 origin 无副作用 |
| **R6 前端可见性链零 rehydrate 改动** | ✅(比 design 表述更顺)`MessageRow.metadata` wire 透传(db/types.rs:447);前端 rehydrate 整体挂载(streamRehydrate.ts:301 `msg.metadata = meta`);`ChatMessage.metadata?: Record<string, unknown>`(chat.types.ts:437);MessageItem 泛型读 metadata 有 edited_at / compaction_summary 先例(MessageItem.vue:375-413) |
| daemon wrapper 先例(server.rs:72 backup / :139 sweeper)+ bin 装配插槽(158/165) | ✅ 形态与 spec Pattern 描述一致 |
| shutdown 步骤①(SSE)/①.5(tunnel stop)/②(drain agent loops) | ✅ server.rs:397-437;插「①.6 scheduler cancel」结构吻合 |
| spec「AppState::load / default_registry 绝不 spawn」(daemon-server.md:255-275) | ✅ RULE-DAEMON-001 段落明文 |
| AppState 空壳先例 | ✅ `tunnel_manager`(state.rs:255,构造 :521 `TunnelManager::new()` 只建不 spawn);注:它是 `Arc<TunnelManager>` 非 OnceLock,`OnceLock<CancellationToken>` 无直接先例但可行 |
| ChatEntry 构造点仅 2 处 | ✅ routes/agent.rs:61(daemon)+ chat.rs:107(Tauri);「全部调用点补 None」= 2 处,可忽略 |
| HttpSseSink 构造(routes/agent.rs:47) | ✅ `Arc::new(HttpSseSink { registry: state.sse.clone() })` |
| AppConfigPayload + fail-open 读法(commands/config.rs:491-510) | ✅ `turn_complete_notify_enabled` 先例,加字段 additive |
| session_type 列(DEFAULT 'chat') | ✅ schema.rs:453;「仅 classic session」校验有数据基础 |
| mock LLM 集成测试先例 | ✅ `MockProvider`(llm/provider/mock.rs:238,与 OpenAI/Anthropic 同 trait 并列)+ agent tests_common.rs 完整 harness(MockEmitter / test_pool / ChatLoopRequest 构造) |
| daemon route oneshot 测试模板 | ✅ routes/sessions.rs:411 `mod tests`:`router(state).oneshot(POST …)`;spec §6「new IPC commands get a Router oneshot test」明文 |
| Settings 现 7 tab,design「第 8 个」正确 | ✅ SettingsModal.vue:40-59(providers/models/default/memory/subagents/remote/search);文件头注释「6 tabs」已过时,与 F2 无关 |
| 完成通知对定时轮自动生效 | ✅ `buildTurnFinishedNotification`(streamController.ts:502)挂在终结事件路径,与消息来源无关 |
| audit API 可扩 | ✅ `AuditKind` 枚举(permissions/audit.rs:34)+ `crate::db::record_audit_event(db, session_id, kind.as_str(), payload, turn_seq)`;独立 record_*_audit 函数先例充分(resend/loop_intervention/softcap 等) |
| DEBT §RULE-QUEUE-001 已登记 | ✅ .trellis/reviews/DEBT.md P2 区,锚点/缓解/根治路径与本任务 design §9 一致 |

---

## 建议动作

1. **design §3/§4**:定案 interval 网格锚点——推荐「两处落账改 `last_fired_at = due`」(P1);§8 单测清单加「interval 无累积漂移」不变量;
2. **design §6**:TaskOrigin 上 wire 与否二选一(推荐顺势上 wire,排队占位可见「定时」),消除与 §4.1 serde derive 的矛盾(P2-1);
3. **design §9 + implement.md AC8**:点名「同 session 多任务同 tick」场景——钉行为测试扩成含 scheduled+scheduled 同轮 drain,可选加创建时软警示(P2-2);
4. **design §3/§6**:定案 disable→enable 存量补跑语义(推荐接受「补跑最近一次」并写明)(P2-3);
5. **implement.md WP1**:lost 审计边界写清「只覆盖 Stop 两处清队点,清队前快照 list_session」(P3-1);WP2 清单补 utils/audit.ts labelForKind 映射(P3-3);
6. 其余按现有三件套执行;P3-2/P3-4/P3-5 实施时随手处理。

---

## 甄别记录(主会话,2026-08-28)

> 结论:P1 / P2-1 / P2-2 / P3×5 全部采纳;P2-3 采纳但**裁定取 (b)**(与评审倾向相反,理由见下);P1 的缺陷定性采纳、机理表述修正一处。无整体驳回项。

- **P1(采纳,机理表述修正)**:锚点未定案 + 落账记 `now` 的缺陷属实,采纳推荐修法(落账恒记 `due`)。修正一处定性:网格随 `last_fired_at` 重锚时 period = N + 新鲜 tick 量化误差(有界,均值 N + tick/2),**并非单调累积**——每个周期的误差不进入下一周期的网格(网格每轮重锚),「趋近 90s 且持续恶化」不成立;但修法不变且更优(记 due 后间隔恒等于 N,严格无漂移)。漂移不变量测试照采纳。
- **P2-1(采纳推荐项)**:origin 顺势上 wire(随 `list_queued_messages` 序列化),排队占位显「定时」徽标;§6 矛盾已消除,改述为「不进 chat 事件主链」。
- **P2-2(采纳,取更强修法)**:除钉行为测试 + 软警示外,调度器加 **per-session 每 tick 至多一次 fire**(`fired_sessions` 集合,余者顺延下 tick、不前移 last_fired_at)——把确定性触发直接消解掉,而非仅接受。测试相应改为断言 deferral 行为。
- **P2-3(采纳,裁定取 (b) 而非评审倾向的 (a))**:update false→true 时 `last_fired_at = now`,重启用不补跑存量。理由:disable 是用户**主动**行为(预期「从下次开始」),daemon 停机才是**非自愿**(D4 补跑)——(b) 让两类停机的语义分界落在用户意图上,一行 update 分支的成本换掉「重新打开立刻跑一份过期报告」的意外。已写入 design §3/§6 与 prd R4/AC4。
- **P3-1(采纳)**:lost 审计边界写进 design §4.5——仅 Stop 语义两处清队点,`list_session` 快照识别;破坏性清理三处不审计。
- **P3-2(采纳)**:`error` 动作 payload 附 `reason`(如 queue_full),不加新动作变体。
- **P3-3(采纳)**:`utils/audit.ts` labelForKind + icon 映射补进 implement.md WP2。
- **P3-4(采纳)**:catch-up/fired 宽限定值 60s(2×tick)写入伪代码;`turn_seq` 按签名传零值、不留 TODO。
- **P3-5(采纳)**:停用任务的 `next_fire_at` 存 schedule 下一到期点,UI 灰显。
- **顺带采纳取证表两处增益**:前端 metadata rehydrate 整体透传已存在(零 rehydrate 改动)、`scheduler_cancel` 简化为普通 `CancellationToken` 字段(OnceLock 无先例且多余)。

