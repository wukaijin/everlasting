# Research — worker turn_trace 路径分析(2026-08-20,主会话代码探索)

> 锚点相对 `app/src-tauri/src/` 与 `app/src/`。任务动机:unified-context-budget PRD
> Out-of-Scope 留的「worker turn_trace 度量盲区(`skip_persist` 不写行)——独立小额 follow-up」。

## 1. 现状:worker per-turn 度量完全缺失

- worker(subagent)由 `run_subagent`(`agent/subagent/dispatch.rs:105`)驱动,阶段 F
  `dispatch/drive.rs:111` 调嵌套 `run_chat_loop`:**复用父 `parent_session_id`**、
  `skip_persist=true`、`is_worker=Some(true)`、`worker_run_id_opt`(subagent_runs 行
  UUID,insert 失败时 None)。
- run 级用量已有:`subagent_runs.token_usage_json`(worker 退出时写累计值,
  `db/subagent_runs.rs` 模块头注释,2026-06-26 snapshot 语义:worker 不写父
  `sessions.last_*`)。**per-turn 明细(usage/tools_token/system_token/context_window)零记录**。
- Done 臂写点 `agent/chat_loop/drive.rs:1254-1279`:`update_last_turn_usage` +
  `upsert_turn_trace_token` **整体在 `if !skip_persist` 门内** —— 后者即盲区本体。
- 切片数据在 worker 路径**本来就有算**:`drive_turn` 顶部无条件序列化 tools_json 并
  `count_tokens` 得 `tools_token`(drive.rs:322);`system_token` / `context_window`
  经 LoopInit 穿入;`memory_token` worker 为 None(prompt 走 `subagent/prompt.rs`,
  init 不算,trace.rs:53-56 注释即此语义);images/at_files worker 恒 None
  (无附件、无 @注入)。→ 补写只需开闸 + 穿 `worker_run_id`,零新计算。

## 2. 根因:为什么不能直接开闸写 turn_trace

### 2.1 seq 空间冲突(核心)

worker loop 的 seq 起点与父后续轮次**共享同一区间**:`chat_loop/init.rs:163-170`
`next_seq = max(messages.seq)+1`(查 DB messages;worker 消息不落库)。worker 从
父当前 max+1 起递增;worker 结束后父下一 turn **重新从 DB messages max+1 起** ——
父 seq 会重用 worker 用过的值。而 `turn_trace` 表约束
`UNIQUE(session_id, seq)`(migrations/schema.rs:1000)是 UPSERT 锚点:

- 直接开闸 → 父后续 `upsert_turn_trace_token` 撞 (sid, seq) 冲突子句,**覆写
  worker 行**(usage/切片被父值顶掉),反之 worker 行污染父行。
- 并发 fan-out(L3b)时多个 worker 从同一起点各自递增,同 seq 不同 run,现约束
  下互相覆写。

→ 必须把 run 维度并入唯一键:`UNIQUE(session_id, run_id, seq)`。

### 2.2 SQLite 约束不可 ALTER —— 表重建

表约束无法 drop,需重建表(rename → create → copy → drop → 重建索引)。先例:
`db/migrations/schema_helpers.rs:123` `widen_subagent_runs_status_check_for_incomplete`
(同款 5 步 + 探针短路幂等 + 不动 `PRAGMA foreign_keys` 的论证,turn_trace 同样
无外部表引用它、FK 只指向 sessions 不变,论证可平移)。注意:该先例
`INSERT ... SELECT *` 依赖列序不变;本任务新表**加了 run_id 列,必须显式列清单
拷贝**。

### 2.3 NULL run_id 陷阱

`UNIQUE(session_id, run_id, seq)` 若 run_id 用 NULL 表示主行:SQLite UNIQUE 视
NULL 互异 → 主行 (sid, NULL, seq) 冲突**不再触发 upsert**,同 (sid,seq) 插入第二
行成功 → 行爆炸、既有语义破坏。→ 主行必须用 **`run_id TEXT NOT NULL DEFAULT ''`**
空串哨兵。'' 不可能是合法 subagent_runs UUID,故 run_id **不加 FK**(否则 '' 违反
FK);生命周期由 session_id FK CASCADE 兜底(subagent_runs 行无独立删除路径:
discard_worker 只清 worktree_path 不删行,仅 session 删除级联,而那同时级联
turn_trace)。注释写明。

## 3. 顺带发现:两处既有旁路写点已在污染父 trace(本任务必须一并归位)

`record_compaction`(drive.rs:568,机械压缩路径,WP1 起**无 worker 门** ——「群聊/
worker 同受益」指压缩本身)与 `record_loop_hint`(drive.rs:1771,C2 软提示,
同样无门)在 worker 路径触发时,会**今天就以父 session_id + worker seq 写
turn_trace 主行**:

- 场景 A(worker 撞机械压缩线):compaction 行落在 (sid, worker_seq);父后续 turn
  同 seq 的 Done upsert 合并进同一行 → 父 TurnCard 显示一次**从未发生的压缩**,
  worker 的压缩被记到父头上(跨归因错误)。
- 场景 B(worker 连续 1-2 轮 loop 命中):loop_hint 同理。
- worker loop 检测可达(C2+ worker ≥3 直接 break,1-2 次命中走 record_loop_hint),
  压缩线在长任务 worker 亦可撞 —— 非纯理论。

修法与盲区同一把钥匙:两处写点路由到 run 行(db 层 upsert 加 run_id 参数,
worker 时传 run UUID,主路传 '')。emit 侧(ChatEvent 经 SubagentBufferSink 进
transcript)维持现状。`record_breadcrumb` worker 恒不触发(workflow_ctx=None),
不必改签名;db 层 4 个 upsert 为 API 统一全部加 run_id。

## 4. 读路径与前端契约

- 前端 `traceStore.currentSessionTraces: Map<seq, TurnTrace>`(types/turnTrace.ts
  注释)按 seq 键控 —— worker 行若混入 `list_turn_traces` 返回,同 seq 互相顶掉。
  → `list_turn_traces` SQL 侧加 `AND run_id = ''` 过滤,**既有消费面零改动**。
- 新读:`list_worker_turn_traces(run_id)` 按 seq ASC 返回该 run 的行(复用
  `TurnTraceRow` struct + 前端 `parseTurnTraceRow` 解析,类型层零新增)。
- 展示面:SubagentDrawer per-run「Token 明细」折叠区(自然落点 —— TracePanel 是
  session 维度,worker 轮属 run)。数据经 useSubagentsStore 新 action 拉取。
- IPC 注册链参照 D2 先例(`search_messages` 6 处:Tauri command / lib.rs
  invoke_handler / daemon axum route / http.ts CMD_TO_DOMAIN / transport 方法 /
  store 调用),实施时按 `list_turn_traces` 现有链逐一镜像。

## 5. 写点改造面(drive_turn 穿参)

- `drive_turn`(drive.rs:82 起,~45 参)**未收 `worker_run_id`**,需穿参
  (`run_chat_loop` 已有该参,chat_loop.rs 调 drive_turn 处透传);
  `let run_key = worker_run_id.as_deref().unwrap_or("")` 后:
  - `update_last_turn_usage` 保持 `!skip_persist` 门不动(snapshot 语义,RULE-A-015
    reversal 不碰);
  - `upsert_turn_trace_token` 门改 `!skip_persist || !run_key.is_empty()`;
  - `record_compaction` / `record_loop_hint` 加 run_id 参数并传 run_key。
- `worker_run_id_opt=None`(insert_run 失败)时 run_key='' → worker 写点自然
  降级为不写(与 permission key 降级先例一致)。

## 6. worker 行的列语义(写什么)

| 列 | worker 值 | 说明 |
|---|---|---|
| token_usage_json | 每轮实际 usage | Done 臂现成 |
| tools_token | 当轮 tools_json 估算 | drive.rs:322 现成,worker 工具集已过滤 |
| system_token | LoopInit 穿入值 | worker system_prompt_override 语义 |
| context_window | worker_ctx 快照 | drive_worker 的 `worker_ctx` 参 |
| memory_token | NULL | worker 注入走 prompt.rs,§3.5a 语义不变 |
| images_token / at_files_token | NULL | worker 无附件 / 无 @注入 |
| compaction_json / loop_hint_json | 撞线时落 | §3 归位后的旁路写点 |

## 7. 验证手段

- db 层单测:重建迁移幂等 / 旧行 run_id='' / 新唯一语义(主行+worker 行同 seq
  共存、同 (sid,run,seq) 二写 upsert)。
- 集成测试:既有 dispatch 基建(MockProvider + subagent 测试簇,researcher 单
  dispatch)断言:run 行落值 + 主行无 worker 污染(无 run_id='' 的 worker seq 行)。
- live:turn-smoke 单轮不 dispatch worker,不适配;可选手动多轮 or 集成覆盖
  (AC 里以集成测试为准)。
