# PRD — 实时 per-turn token 事件(TurnUsage)+ 5h 滚动窗口配额视图

> 2026-08-20 brainstorm 起草。用户套餐计费(5 小时滚动窗口刷新额度),**不做 $ 换算**;关注"窗口期烧了多少 / session 消耗多少 token"与"per-turn token 明细即时可见"。

## Goal

1. **WP1(实时 TurnUsage 事件)**:主 loop 每轮 LLM 返回后,per-turn token 数据(usage 五字段 + 6 切片)随 SSE 事件即时推到前端,TracePanel TurnCard 的 token cells 不再等"下一条用户消息触发 `loadHistory`"才出现。
2. **WP2(5h 滚动窗口配额视图)**:按 provider 聚合滚动 5h 窗口 token 消耗(主 loop + worker 全算),额度可配置(缺省只显示消耗不设上限),顺带补 per-session 累计消耗的展示位。

## Background / 已确认事实(代码取证)

### WP1 现状(为什么慢一拍)

- `ChatEvent::Done { usage }`(`app/src-tauri/src/llm/types/event.rs:104`)已带 usage,前端 `streamEvents.ts` done 分支实时刷新 ChatInput hint(`setLastTurnUsage`,覆盖式快照)——**输入框上方 hint 已实时,本任务不改它**。
- TracePanel TurnCard 的 token 切片(tools/memory/at_files/images/system/context_input/context_window)只能靠 `list_turn_traces` IPC 回看 DB;`traceStore.loadHistory` 仅在 `startRequest → resetForNewSession`(下一条用户消息)或切 session 时触发(`app/src/stores/streamController.ts:1008`),`reloadAfterFinalize` 不刷新 traceStore → **刚跑完的轮次要等下一次发送才可见**。
- traceStore live 路径只监听 3 种事件(context_compacted / loop_hint / workflow_breadcrumb,`app/src/stores/traceStore.ts` applyEvent),均不带 token。
- 后端写点单点齐备:`app/src-tauri/src/agent/chat_loop/drive.rs:1322` Done 臂 `upsert_turn_trace_token(&db, &session_id, run_key, seq, t, Some(tools_token), memory_tok, images_tok, at_files_tok, Some(system_token), Some(context_window))` —— emit 事件所需的全部值(含 seq / run_key / 各切片)在此都在手上。worker 路径(run_key 非空、skip_persist=true)同点写 run 维度行。
- 双写模板:`agent/trace.rs` 的 `record_*` helper(emit + upsert,best-effort,DB 失败 warn 吞掉)。
- 前端 `TurnTrace` 类型(`app/src/types/turnTrace.ts:187`)已含 tokenUsage/toolsToken/memoryToken/imagesToken/atFilesToken/systemToken/contextWindow 全部字段 —— live upsert 后 TurnCard 零渲染改动。

### WP2 现状(数据面)

- `turn_trace` 表:per-turn `token_usage_json` + 6 切片列 + `created_at` + `run_id`(08-20 起并入 run 维度,`UNIQUE(session_id, run_id, seq)`)。
- **turn_trace 无 provider/model 归因列**;provider 只能经 `session_id → sessions.model_id → models.provider_id` join(session 中途切模型则归因失真;worker 换 provider 时 worker 行归因到父 session 的 provider,失真)。
- `sessions` 表有 `input_tokens_total` / `output_tokens_total` 累计列(A4),但**无写点也无前端展示位**(2026-06-26 snapshot fix 后 `update_last_turn_usage` 只写 last_* 快照列,累计列成孤儿列;`tokenUsageBySession` 存 last-turn 快照)。session 累计的可信口径 = turn_trace 全量聚合(见 AC6)。
- worker 消耗在 `subagent_runs.token_usage_json` + turn_trace run 行;烧的是同一个套餐额度,必须计入窗口。
- 配置模式:DB `config` 表 `get_config_value(&db, "<key>")`(先例 `context_budget_enabled`,`app/src-tauri/src/agent/chat_loop.rs:629`)。

## Requirements

### WP1 — ChatEvent::TurnUsage(实时 per-turn token)

1. 新增 `ChatEvent::TurnUsage` 变体(serde tag `kind = "turn_usage"`):`{ request_id, seq, run_id, usage(TokenUsage 五字段), tools_token, memory_token, images_token, at_files_token, system_token, context_window }`(request_id = activeRequests gate 路由键;切片 Option,worker 行按 drive.rs:1315-1321 既有 NULL 契约传 None)。read-only / 非持久化,同 `Recall` / `BudgetTrim` 模式。
2. emit 点:drive.rs Done 臂 upsert 旁(值同源,emit 与 DB 写一致性天然成立);follow `record_*` best-effort 契约。
3. 前端:`streamEvents.ts` 加 `turn_usage` case → `traceStore.applyEvent` upsert 对应 `TurnTrace`(主 loop 行;事件带 session 归属经 activeRequests gate)。TurnCard 零改动(cells 已从 TurnTrace 渲染)。
4. worker 维度 live 不做(维持 08-20 落的 `list_worker_turn_traces` DB 点查 + drawer 粘性缓存);worker 的 TurnUsage 事件落 SubagentBufferSink transcript,前端不消费。

### WP2 — 5h 滚动窗口配额视图

5. 归因:`turn_trace` 加 `provider_id TEXT NULL` 列(与 WP1 同一写点落值,主 loop = session model 的 provider,worker = `resolve_worker_provider` 实际 provider);历史行 NULL 走 session join 近似回填(migration 一次性)。
6. 聚合 IPC:`usage_window(provider_id 可选过滤)` → per-provider 窗口内 input/output/cache_read/cache_creation 累计 + 主 loop/worker 拆分 + 按小时分布 + top N session(含 session 累计 input/output)。SQL 聚合 turn_trace(`created_at > now - N h`,N 经 config `quota_window_hours` 读、缺省 5,非 IPC 参数),零新表。
7. 配置:`config` 表 `quota_window_hours`(缺省 5)+ `quota_limit_tokens`(NULL = 只显示消耗,不画上限/撞线预估;设了才显示"已用/上限 + 按当前速率撞线预估")。
8. 前端视图(**2026-08-20 用户拍板:AppHeader chip + 弹层**):顶栏常驻 `<QuotaChip>`(窗口总量缩写,设了 limit 加占比条),点开 `<QuotaPopover>` 看 per-provider 拆分 + 主/worker 拆分 + 小时分布 + top sessions(可跳转 session)+ 配额设置入口;移动端 <430px 降级(S6a 档先例)。
9. per-session 累计展示:session 维度数据走聚合 IPC 的 top N(或全量列表),不在 ChatInput hint 加行(hint 语义 = 上轮上下文快照,不混累计口径)。

## Acceptance Criteria

- **AC1(WP1)**:**正常完成轮**(usage=Some 且 upsert 门通过;usage=None 的取消/错误轮不发事件、cells 保持 "—" 属预期退化)——live 轮次完成后、用户不发下一条消息,TracePanel 中该轮 TurnCard 的 token cells(context_input / tools / mem / img / @ / sys / window)立即出现且与 DB 落值一致(事件与 upsert 同点同值)。
- **AC2(WP1)**:worker run 的主 chat TracePanel 不出现 run 维度行(worker TurnUsage 不冒泡;worker `insert_run` 失败降级 run_key='' 时无事件,亦属预期);SubagentDrawer「Token 明细」行为不变(DB 点查)。
- **AC3(WP1)**:trace DB 写失败时事件照发(emit 不被 DB 失败拖挂),事件缺失时行为退化为现状(等 loadHistory)。
- **AC4(WP2)**:构造跨 5h 边界的 turn_trace 数据(含 worker run 行 + 跨 provider),聚合 IPC 返回的 per-provider 累计 / 主-worker 拆分 / 小时分布 / top session 与手工 SQL 一致;窗口滚动(旧数据滑出)正确。
- **AC5(WP2)**:`quota_limit_tokens` 未设时视图只显示消耗;设置后显示已用/上限 + 撞线预估;`quota_window_hours` 可调。
- **AC6(WP2)**:session 累计消耗可见(top N 列表),数值 = **turn_trace 全量聚合**(实现期发现 `sessions.input_tokens_total` 自 2026-06-26 snapshot 重构后已无写点、是孤儿列,`update_last_turn_usage` 只写 last_* 快照——原"与 sessions 列同源"前提不成立,改从 turn_trace 聚合;pre-E2 session 无 trace 行累计为 0,可接受)。
- **AC7**:后端 `cargo test -p everlasting --lib` 全绿 + 前端 `pnpm test` 全绿 + vue-tsc 0 err;新 IPC 在 httpTransport CMD_TO_DOMAIN 有映射(POST,D2 P1 先例)。
- **AC8**:live 冒烟(`scripts/turn-smoke.sh` **扩 SSE 订阅断言**,`--assert-turn-usage` 开关:捕获 `kind=turn_usage` + 字段非空 + 与 DB 落值同值——现状脚本丢弃 chat 响应只轮询 DB,断言不了事件,需先扩脚本);窗口聚合在真库上返回非零。

## Out of Scope

- $ 换算 / 价格表(用户套餐计费,明确不做)。
- worker per-turn 的 live 推送(维持 DB 点查)。
- 模型加权(套餐按权重折算额度如 Opus 5x——不做,原始 token 计)。
- 跨设备/remote 端额外视图(PWA 复用同一前端,自然继承,不单独验收)。
- 配额撞线的拦截/干预行为(只观测不拦截)。

## Key Decisions(已拍板)

1. **WP2 视图落点 = AppHeader chip + 弹层**(2026-08-20 用户选定;备选 Settings tab / TracePanel 区块已否)。技术细节(事件变体设计、provider_id 归因列、聚合刷新时机、limit 可选语义)见 `design.md` 取舍表。
2. **外部模型评审(`review.md`)4 点全部核实采纳**(2026-08-20,行级甄别通过):P1 provider_id 穿参改走 `drive_turn` 参数链(wrapper / resolve_worker_provider 扩返回值,不走 LoopInit);P2 AC1/AC2 断言边界限定正常完成轮;P3 加列三处 CREATE 同步 + 重建迁移顺序守则;P4 turn-smoke.sh 扩 SSE 订阅满足 AC8。评审 §2 的 transcript 容忍疑点已代查实锤:worker sink 无 kind 过滤会记录 TurnUsage 进 transcript,但 `runAccumulator.buildSectionsFromRaw` 语句级 switch 静默忽略未知 kind,drawer 无害。
