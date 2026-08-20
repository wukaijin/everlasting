# Design — worker turn_trace 度量盲区闭合

> 前置:prd.md R1-R5/D6 + research/worker-path-analysis.md(代码锚点都在那,
> 本文只写要改什么、契约长什么样)。

## 1. 存储层

### 1.1 新表形(greenfield,migrations/schema.rs CREATE 同步改)

```sql
CREATE TABLE IF NOT EXISTS turn_trace (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id        TEXT NOT NULL,
    -- 本任务:run 维度。'' = 主 loop 行(哨兵,非 NULL —— SQLite
    -- UNIQUE 视 NULL 互异,NULL 主行的 (sid,seq) 冲突将不再触发
    -- upsert 而是插入第二行,既有语义破坏)。worker 行 = subagent_runs.id。
    -- 不加 FK:'' 不是合法 subagent_runs.id;subagent_runs 行无独立删除
    -- 路径(discard_worker 只清 worktree_path),生命周期由 session_id
    -- CASCADE 兜底(删 session 同时级联两者)。
    run_id            TEXT NOT NULL DEFAULT '',
    seq               INTEGER NOT NULL,
    token_usage_json  TEXT,
    compaction_json   TEXT,
    loop_hint_json    TEXT,
    breadcrumb_json   TEXT,
    tools_token       INTEGER,
    memory_token      INTEGER,
    images_token      INTEGER,
    at_files_token    INTEGER,
    system_token      INTEGER,
    context_window    INTEGER,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    UNIQUE(session_id, run_id, seq)
)
```

索引:旧 `idx_turn_trace_session_seq (session_id, seq)` 不再重建(新 UNIQUE
索引前缀 (session_id, run_id) 已覆盖按 session 查主行的路径);新增
`idx_turn_trace_run (run_id) WHERE run_id != ''`(partial index,专服
list_worker_turn_traces;写放大可忽略 —— worker 行每 run ≤20 轮)。

### 1.2 重建迁移(migrations/schema_helpers.rs 新 helper,老库路径)

`rebuild_turn_trace_with_run_id(pool)`,五步舞照 widen_subagent_runs 先例
(schema_helpers.rs:123),差异点:

1. 探针:`pragma_table_info('turn_trace')` 无 `run_id` 列才重建(比先例的
   `sqlite_master.sql contains` 探针更直接);表不存在直接 return。
2. 拷贝必须**显式列清单** `INSERT INTO turn_trace (id, session_id, run_id, seq,
   ...) SELECT id, session_id, '', seq, ... FROM turn_trace_old`(新列插中间,
   `SELECT *` 列序不齐会错位/失败 —— 先例靠列序相同才敢 SELECT *,这里不行)。
3. FK 论证平移:turn_trace 无外部表引用它;自身 FK 只指 sessions(重建期间
   sessions 不动),不 toggle `PRAGMA foreign_keys`(先例的测试池多连接论证
   原样适用)。
4. 事务包裹(rename→create→copy→drop→index 五步一个 `BEGIN IMMEDIATE`),
   崩溃不留半重建态(留下 turn_trace_old 的话下次探针看 turn_trace 缺列会
   重走,CREATE IF NOT EXISTS + 显式列拷贝对残留 old 表 DROP IF EXISTS 兜底)。

幂等:探针短路;greenfield 库 CREATE 已含 run_id,helper no-op。

### 1.3 db/trace.rs API(4 个 upsert 统一 + 读侧)

- `upsert_turn_trace_token / _compaction / _loop_hint / _breadcrumb` 签名各加
  `run_id: &str`(主路传 `""`);SQL 的 INSERT 列与 `ON CONFLICT(session_id,
  run_id, seq)` 同步扩。
- `TurnTraceRow` 加 `run_id: String`(wire `runId`,camelCase 随 struct rename;
  主行恒 `''`)。
- `list_turn_traces`:`WHERE session_id = ? AND run_id = '' ORDER BY seq ASC`
  —— 既有消费面(前端 Map<seq> 契约、list_speaker_cache_usage 语义)零变化。
- 新 `list_worker_turn_traces(pool, run_id) -> Vec<TurnTraceRow>`:
  `WHERE run_id = ? ORDER BY seq ASC`(run_id 全局唯一,不限定 session)。

## 2. 写点(agent 层)

### 2.1 穿参

`run_chat_loop` 已有 `worker_run_id: Option<String>`;`drive_turn` 签名加
`worker_run_id: Option<&str>`(chat_loop.rs 调用处透传,~45 → 46 参,已知债务
接受)。函数顶:`let run_key: &str = worker_run_id.unwrap_or("");`

### 2.2 Done 臂(drive.rs:1254-1279)

```
if !skip_persist { update_last_turn_usage(...) }        // 门不动
if !skip_persist || !run_key.is_empty() {               // 新:worker 也写 trace
    upsert_turn_trace_token(&db, &session_id, run_key, seq, ...)
}
```

worker 调用点传参:memory_token worker 本就是 None(LoopInit 语义);
images_tok / at_files_tok 维持现有 None 语义(空串门判 `skip_persist ||` 分支
可删 —— 死条件,顺手清理);system_token / context_window / tools_token 原值。

### 2.3 旁路写点归位

- `agent::trace::record_compaction / record_loop_hint` 签名各加 `run_id: &str`,
  透传 db upsert;drive.rs 两处调用传 `run_key`。emit 侧(ChatEvent)不动
  (worker 事件进 SubagentBufferSink transcript,现状维持)。
- `record_breadcrumb` 签名不动(worker 恒不触发);其 db upsert 调用传 `""`。

## 3. IPC + 读链(镜像 list_turn_traces 现有链)

- Tauri command `list_worker_turn_traces(run_id)`(commands 侧与
  `list_turn_traces` 同模块)+ lib.rs invoke_handler。
- daemon axum route(与 list_turn_traces 同款 GET/命名惯例,遵循「命令名即
  路径段」教训 —— B1 hotfix 2)。
- `app/src/transport/`(tauri + http,CMD_TO_DOMAIN 若硬编码 POST 则照
  list_turn_traces 现归类)。实施时以 `grep -rn "list_turn_traces"` 逐一镜像,
  不凭记忆写注册点。

## 4. 前端

- `types/turnTrace.ts`:`TurnTraceRow` 加 `runId: string`(旧 payload 无此字段
  → 容错 `row.runId ?? ''`,与 `method?` 可选字段同款处理);`parseTurnTraceRow`
  原样可用(TurnTrace 形状不变,runId 只留在 row 层)。
- `useSubagentsStore`:`loadRunTurnTraces(runId)` action(`runTracesByRunId:
  reactive Map<string, TurnTrace[]>`)+ per-run 加载态;drawer 关闭不清(粘性,
  与 getRunCache 同生命周期)。
- `SubagentDrawer` 新子组件 `WorkerTurnTraceList.vue`(drawer 目录内):折叠区
  「Token 明细」,默认收起、展开时按需拉取一次;每行:`#seq · in/out · cache-read
  (率) · tools tok · ctx/window %`;复用 formatTokens 类 utils(dns 若无现成
  formatter 则组件内轻量格式化)。空态(无行 = 旧 run / binary 旧行)显示
  「无 per-turn 记录」。

## 5. 测试设计

- db 单测(trace.rs tests 扩):
  - 重建迁移:老形状表(手建旧 schema + 数行)→ 跑迁移 → run_id='' 全量、
    二次跑 no-op、主行冲突 upsert 语义保持、主行+worker 行同 seq 共存、
    list_worker_turn_traces 按 run 过滤。
  - 4 个 upsert 的 run_id 透传(至少 token + compaction 两族)。
- 集成测试(subagent 测试簇,MockProvider):researcher dispatch 多轮 →
  AC2/AC3 断言(run 行落值/主行无污染/worker 不写 sessions.last_*)。
- 前端:store action 测试(mock IPC)+ WorkerTurnTraceList 渲染测试(空态/
  行渲染);traceStore 既有测试**零改动**通过(AC5 回归锚)。

## 6. 兼容性 / 回滚

- 旧前端 + 新后端:list_turn_traces 过滤主行,行为不变;TurnTraceRow 多
  runId 字段,TS 侧可选容错。
- 新前端 + 旧后端:runId undefined → `?? ''` 容错;loadRunTurnTraces 报错走
  drawer 空态(不崩)。
- 回滚单元 = 三个 PR 逐个 revert;迁移本身向后不兼容(新约束下旧行 run_id=''
  合法,旧 binary 读新表无碍 —— 旧 upsert 不写 run_id 列,DEFAULT '' 兜底,
  唯一键语义在旧 binary 下主行照常冲突 upsert,worker 行旧 binary 不写。
  即:迁移先行安全)。
