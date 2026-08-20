# Review — 实时 per-turn token 事件(TurnUsage)+ 5h 滚动窗口配额视图

> 评审对象:PRD / design / implement(任务处于 **planning** 阶段,仓库中**尚无任何本任务代码**,`git status` 仅有该任务目录;`TurnUsage` / `usage_window` / `QuotaChip` / `quota_window_hours` 全库无实现痕迹)。
> 评审方式:逐条核对 PRD/design 引用的代码事实(行号/结构/先例),验证设计取舍在现有代码上的可落地性。
> 结论:**文档取证质量高,主体设计成立,可直接进入实现**。发现 4 个需要实现期留意的点(P1×1、P2×3),集中在 WP2 归因列的数据面与验收方式,均不阻塞开工。

---

## 1. 代码取证核实结果(抽样,与文档一致性)

| 文档主张 | 实际代码 | 判定 |
|---|---|---|
| `ChatEvent::Done { usage }` 已带 usage(`event.rs:104`) | `event.rs:104` `Done { stop_reason, usage }`;serde tag `kind` + snake_case(`event.rs:63`) | ✅ 一致 |
| `record_*` best-effort 模板 = emit + upsert,DB 失败 warn 吞掉(`trace.rs`) | `record_compaction`(`trace.rs:48`):emit 在 upsert 前,upsert 失败 `tracing::warn!`;`record_loop_hint` / `record_breadcrumb` 同构 | ✅ 一致 |
| Done 臂 upsert 单点(`drive.rs:1322`),值含 seq/run_key/全切片 | `drive.rs:1316-1323`:`if !skip_persist \|\| !run_key.is_empty()` 门内 `upsert_turn_trace_token(&db, &session_id, run_key, seq, t, Some(tools_token), memory_tok, images_tok, at_files_tok, Some(system_token), Some(context_window))`;run_key = `worker_run_id.unwrap_or("")`(`drive.rs:181`)。**注意:该点包在 `if let Some(t) = usage` 与 `if !skip_persist \|\| !run_key.is_empty()` 双层门内,不是所有 Done 都经过** | ✅ 一致(含文档未明说的一处细节,见 §3-P2) |
| `LoopInit` 无 provider/context_window 字段,需新增 | `init.rs:31` LoopInit 现无 provider/context_window 字段;`run_chat_loop` 持有 `provider: Arc<dyn Provider>`(`chat_loop.rs:321`)+ `context_window: u32`(`chat_loop.rs:322`),两者都是 `drive_turn` 的既有参数(在 `chat_loop.rs:1029/1032` 传入)——**provider_id 本可沿 drive_turn 参数直达 Done 臂,不必走 LoopInit** | ⚠️ 见 §3-P1 |
| `ModelRow.provider_id`(`db/types.rs:100`) | `types.rs:100` `ModelRow { id, provider_id, model_name, ... }`;`get_model`(`db/models.rs:115`)可按 mid 取行 | ✅ 一致 |
| worker `resolve_worker_provider` 得实际 provider | `resolve.rs:151` 返回 `(Arc<dyn Provider>, u32, Option<String>)`,只有 display_name,**无 provider_id**;`WorkerModel`(`dispatch/model.rs:45`)同样只带 `worker_provider: Arc<dyn Provider>` / `worker_ctx` / `worker_display: Option<String>` | ⚠️ 见 §3-P1 |
| 前端 `traceStore.applyEvent` 只收 3+1 种事件、不带 token | `traceStore.ts:220` applyEvent 联合类型 = ContextCompacted/LoopHint/WorkflowBreadcrumb/BudgetTrim,分支各 set 单维(`applyContextCompacted` 等),merge 语义(`...existing`) | ✅ 一致 |
| `loadHistory` 仅在 startRequest/resetForNewSession 触发 | `streamController.ts:1009` `resetForNewSession`;`reloadAfterFinalize`(`streamEvents.ts:1119`)只重查 load_session/audit/latency,**不刷 traceStore** | ✅ 一致 |
| `TurnTrace` 字段已齐(`turnTrace.ts:187-227`) | `turnTrace.ts:187+` tokenUsage/toolsToken/memoryToken/imagesToken/atFilesToken/systemToken/contextWindow 全在,`parseTurnTraceRow`(`turnTrace.ts:304`)做 DB 行→TurnTrace 映射(含 `!= null` 判空) | ✅ 一致 |
| `streamEvents` budget_trim 防御模板(`:705-713`) | `streamEvents.ts:700+` `typeof event.freed_tokens !== "number" ... break` 防御 + applyEvent | ✅ 一致 |
| `CMD_TO_DOMAIN` 需手工同步(`http.ts:51`) | `http.ts:51` 表注释明说 Rust routes 注册 + 此映射两处需手工对齐;`list_worker_turn_traces: "permissions"`(`http.ts:97`)、`handoff_session: "sessions"`(`http.ts:138`)为 POST 先例;`http.test.ts:136` 有 handoff_session 回归锚 | ✅ 一致 |
| 路由注册链 6 处(daemon route + invoke + commands/mod.rs 白名单) | `lib.rs:337` invoke 注册、`commands/mod.rs:144` 白名单、`daemon/routes/permissions.rs:130/191` 路由+`post()`、`http.ts:51` CMD_TO_DOMAIN —— `list_worker_turn_traces` 五处链齐全(design 写"6 处"略多,实际同构命令是 5 处) | ✅ 一致 |
| turn_trace 重建迁移模板(`schema_helpers.rs:290`)+ 幂等加列 helper | `schema_helpers.rs:208+` `rebuild_turn_trace_with_run_id`(probe→rename→copy 显式列清单→重建 UNIQUE(session_id,run_id,seq));`columns.rs:190` `add_turn_trace_column_if_missing`;绿地 CREATE(`schema.rs:944`)+ 测试内 `db/trace.rs:851` 另一份 CREATE(需同步改) | ✅ 一致 |
| `sessions.input_tokens_total` 等累计列 | `types.rs:308-309` + cache_creation/cache_read_total 全在 | ✅ 一致 |
| worker 消耗在 `subagent_runs.token_usage_json` | `subagent_runs.rs:178` 字段在;写点在 `dispatch.rs` worker 退出 `collect_outcome` 后 | ✅ 一致 |
| `get_config_value` 先例(`chat_loop.rs:629`) | `chat_loop.rs:622/629` `tools_stub_enabled` / `context_budget_enabled`;`db/config.rs:46` set_config_value 同文件 | ✅ 一致 |
| S6a <430px 窄屏先例 | `Sidebar.vue:234` 有 320px 上限注释(移动端导航);reka-ui Popover 已在 `ModeSelect.vue`/`TriggerMenu.vue`/`WorktreeChip.vue` 使用(`package.json:32` reka-ui ^2.9.9) | ✅ 一致 |
| `openSessionInProject` 跳转 D2 已有 | `stores/chatSessionActions.ts` / `stores/chat.ts` 存在 | ✅ 一致 |
| TurnCard 零改动(trace 字段已渲染) | TracePanel 经 `TurnTimeline` 渲染 TurnCard,`toolsToken` 等字段已被消费(同文档) | ✅ 一致 |

**未核实到/与文档有出入的引用**(实现期注意):

- PRD 引 `event.rs:104` 的 Done 位置正确,但 design 未注明该变体是 provider stream 事件、**emit 在 drive 层已有统一出口**——emit TurnUsage 时应参照 `record_*` 在 Done 臂内直发,不要试图复用 provider 的 Done 事件本身。
- `turn-smoke.sh` 当前**只轮询 DB、不订阅 SSE**(`curl POST /agent/chat >/dev/null`,无 `-N` / event-stream 消费)。PRD AC8「TurnUsage 事件随 SSE 推出」在现有脚本形态下**无法直接断言**,需在脚本里加 SSE 订阅(或用 `--keep` 手工侧信道),见 §3-P4。

---

## 2. 设计评估

### WP1 — 事件变体 + 同点 emit(整体:成熟,风险低)

- 新变体 `TurnUsage` + 前端 merge upsert,是对 `record_*` / `applyBudgetTrim` 既有模式的直接复用;TurnCard 零改动的论断被 `parseTurnTraceRow` 的字段映射证实。
- 事件缺失→退化现状(等 loadHistory)的语义成立:前端 case 不存在时 event 被忽略,`loadHistory` 路径不受影响。
- worker 事件进 `SubagentBufferSink` transcript 不冒泡:核实 sink 侧**没有按 event kind 的过滤**,任何 ChatEvent 都会被记录进 transcript(若有),但前端 `SubagentEventSink` 走的是 `subagent:event` 通道、与主 SSE 分离——「主 chat 不出现 worker 行」这一层靠 transport 隔离而非事件过滤,design 的表述("emit 侧不做 worker 特判")与此吻合,可行。**但注意**:TurnUsage 若是 ChatEvent 变体,会被 sink 的 `record()` 包进 transcript,`transcript` 已由 PR2 落库(`subagent_runs.transcript`);worker transcript 的序列化/体积因此会带上每轮 usage 字段——对配额视图无碍,但对 drawer 的历史 transcript 有数据面影响,建议在实现期确认 transcript 序列化是否容忍新字段(只增不改,风险低)。

### WP2 — 归因列 + 聚合 IPC(主体成立,数据面有 3 个待决点)

- 零新表 + turn_trace 聚合是合理的最小面;`json_extract` 对 NULL `token_usage_json` 返回 NULL、SUM 忽略的论断正确。
- `provider_id` 落列 vs join sessions 的取舍正确:worker 换 provider 时 join 必失真(worker 行归到父 session 的 provider),落列是唯一一次写准的方案。
- 配置经 `config` 表 + `set_config_value` 有直接先例;`quota_limit_tokens` NULL=只显示消耗的语义自洽。
- **移动端降级 / QuotaChip 布局**属于 PR3 前端工程,先例齐备,无设计风险。
- **聚合刷新时机 = finalize 后重查**的取舍合理,5h 尺度下不轮询成立。

---

## 3. 实现期必须处理的问题

### P1 — worker 的 provider_id 目前拿不到(设计第 58 行高估了现成度)

design 写「worker 经 `run_subagent` 的 `resolve_worker_provider` 结果同样从 prepare_loop_state 穿入」,但核实:

- `resolve_worker_provider`(`resolve.rs:151`)返回值**只有 provider Arc + context_window + display_name,没有 provider_id**;`WorkerModel`(`dispatch/model.rs:45`)同样。
- `run_chat_loop` 持有的 `provider: Arc<dyn Provider>`(`chat_loop.rs:321`)是个 trait object,`Provider` trait(`provider/mod.rs:66`)只有 `send` / `capabilities` / `protocol`,**没有 id/name 成员**;`build_provider`(`provider/mod.rs:141`)构造出的 Anthropic/OpenAIProvider 不携带 provider_row.id。
- catalog 的 key 是 **model_id** 而非 provider_id(`state.rs:465` `build_provider_catalog` 以 model_row 为键插入)。

**结论:worker 的 provider_id 不能从现有 provider Arc 上取,必须显式解析**。最小改动路径:

- `resolve_worker_provider` 内部已经在 `catalog.get(mid)` 命中时 fetch 过 `get_model(db, mid)`(`resolve.rs:168`),该 ModelRow 的 `provider_id` 顺手可得——把返回值扩成 4 元组(或 `WorkerModel` 加字段)即可,改动点集中在 `resolve.rs` + `dispatch/model.rs` + `dispatch.rs` 穿参,不动 provider trait。
- 主路径同理:`lookup_provider_for_session`(`chat.rs:650`)已经拿到 `mwp.model.provider_id` 并构造了 `ResolvedChatProviderWrapper`(`chat.rs:770`),把 `provider_id` 加进 wrapper、经 `chat_inner`→`run_chat_loop`→`drive_turn` 穿下去,比走 LoopInit 更近(drive_turn 已收 provider 参数)。design 的「LoopInit 新字段」方案也能成,但要多动 init.rs 的构造+解构+每处 `Ok(LoopInit{...})`,而 provider/context_window 本来就在 drive_turn 签名上——**推荐走 drive_turn 参数而非 LoopInit**,省一层。
- 注意主 loop 的模型可能是 `sessions.model_id` 覆盖(`resolve_model_id_for_session` `chat.rs:732`),provider_id 取自该 model 行,与"哪个 provider 实际发了请求"一致;worker 换 provider(dispatch_model)时按上述解析,归因即准确。

### P2 — 无 usage 的 Done 事件不落 TurnUsage(AC1 断言边界要写清楚)

Done 臂的 upsert + (未来的)TurnUsage emit 都在 `if let Some(t) = usage` 与 `if !skip_persist || !run_key.is_empty()` 双层门内(`drive.rs:1240` + `:1316`):

- `usage=None`(取消/错误终止)的 Done **不会**产生 TurnUsage → 前端该轮 token cells 保持 "—",与「事件缺失退化为现状」语义自洽,但 AC1「轮次完成后立即出现」应注明只覆盖正常完成轮(usage=Some 且 upsert 实际执行)。
- 同理,worker 的 `insert_run` 失败降级(run_key='')时 `!skip_persist` 为 false,worker TurnUsage 也不发——与"worker 不进主 chat"语义一致,但实现期测试别按"每个 worker 轮必有事件"写断言。

### P3 — 加列时机与回填的先后顺序(表约束加宽守则)

- 新列走 `add_turn_trace_column_if_missing` 幂等 helper(`columns.rs:190`)追加在 `schema.rs:1028` 既有链尾,再叠加 migration 回填 UPDATE;必须在 `rebuild_turn_trace_with_run_id` 之后或之前排对顺序——重建迁移的拷贝列清单(`schema_helpers.rs:322`)是显式的,**若 provider_id 加在重建之前,重建会丢列;加在重建之后则新库/老库都要过**。design 第 57 行已提「两处同步」但没列第三处:`db/trace.rs:851` 测试内的 CREATE 也要同步(否则该测试重建会缺列)。建议实现时以 `list_worker_turn_traces` 那次的顺序为模板(先加列后重建,重建前拷列清单里补新列)。
- 回填 UPDATE 依赖 `sessions.model_id` 非空、models 行存在;`resolve_model_id_for_session` 有"session model_id 指向已删模型 → 回退 default"的既有逻辑,历史行近似回填时会出现 join 不到 providers 的行,聚合端要有 `NULL/unknown` 兜底桶(design 已提,确认实现)。

### P4 — AC8 的"SSE 事件断言"需要扩 turn-smoke.sh

现有 `scripts/turn-smoke.sh` 只轮询 DB、把 `/agent/chat` 的响应丢弃(`>/dev/null`)。要满足 PRD AC8「TurnUsage 事件随 SSE 推出、字段齐全」,必须在脚本里加 SSE 订阅(参考 `scripts/remote-e2e-smoke.mjs` 或 daemon 的 HTTP SSE 客户端),否则只能退化为「DB 轮询 + 报告列」的间接验证。建议给脚本加 `--assert-turn-usage` 开关:SSE 流里捕获 `kind=turn_usage` 且断言字段非空,再与 DB 落值比对同值(正好把 AC1 的"事件与 upsert 同值"也验了)。

---

## 4. 计划完备性

- implement.md 三段 commit 划分(PR1 事件 / PR2 归因+IPC / PR3 前端)合理,每段独立可回滚,与仓库既有 commit 粒度(每任务 1-5 commit)一致。
- 测试矩阵覆盖了 AC1-AC6 的 Rust/前端两层;AC3(DB 失败事件照发)实现期可能难注入,文档已预留降级为代码审查点,可接受。
- 漏列的一个实现步骤:**`turn-smoke.sh` 的列存在性检查 + 报告列需为 provider_id 加一行**(脚本 `:286-305` 已对每列做存在性检查,是既有模式,加列应同步该清单);以及 `http.test.ts` 的 CMD_TO_DOMAIN 回归锚应加 `usage_window`(handoff_session 先例)。
- 风险文件列表准确(drive.rs Done 臂、upsert 扩参、schema_helpers),`db/trace.rs` 测试内 CREATE 与 `commands/mod.rs` 白名单应补进风险清单。

---

## 5. 结论

PRD/design/implement 三份文档的证据链与仓库现状高度吻合,取证扎实(行号级引用几乎全数核实通过);设计取舍(新事件变体 vs 扩 Done、provider 落列 vs join、finalize 后刷新 vs 轮询)在当前代码结构上都是正确选择。**建议按 implement.md 进入实现**,实现期按 §3 处理 4 个点:worker provider_id 显式解析(推荐走 drive_turn 参数而非 LoopInit)、AC1 断言边界限定正常完成轮、加列顺序按既有重建迁移模板并补 `db/trace.rs` 测试 CREATE、turn-smoke.sh 扩 SSE 订阅满足 AC8。P1 需要在 implement 阶段先行确认(影响 PR2 的签名设计),其余可在各 PR 内顺带消化。
