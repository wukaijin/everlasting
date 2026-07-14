# E2 harness trace viewer — 技术设计

> 配套 `prd.md`。Scope = 完整版(后端 trace 管道 + 前端独立面板 live+回看 + always-on 落盘)。

## 1. 架构与边界

```
agent loop (chat_loop.rs / context.rs / loop_detection.rs / inject.rs)
  │
  │  每轮 4 类 trace 信号(emit always-on)
  ├─ ContextCompacted  (C3 压缩时)
  ├─ LoopHint          (C2 1-2 连击 soft hint)
  ├─ WorkflowBreadcrumb(每轮 breadcrumb 注入)
  └─ Done.usage        (per-turn token,已有 event,补落盘)
        │
        ▼
  trace_pipeline helper (新,agent/trace.rs)
   ├─ emit ChatEvent → chat-event 通道(live 面板)
   └─ upsert turn_trace 行(回看)
        │
        ▼
  SQLite: turn_trace 表(v7 新)+ session_audit_events.turn_seq 列(v7 加)
        │
        ▼
  IPC: list_turn_traces / list_session_audit_events(已有,带 turn_seq)
        │
        ▼
  前端 useTraceStore → <TracePanel>(AppShell drawer,live+回看同构)
```

**边界**:
- 后端 child-1 产出:3 新 ChatEvent 变体 + turn_trace 表 + 审计 turn_seq 列 + trace_pipeline helper + list_turn_traces IPC。不动 agent 决策逻辑(只在已有写点旁 emit + 落盘)。
- 前端 child-2 产出:`<TracePanel>` + `useTraceStore` + AppShell drawer 挂载 + 渲染(复用 parseAuditPayload/icon family)。依赖 child-1 的 event + IPC + 数据结构。
- 不改 agent 决策行为:C3 仍按 CompactResult 决定降级/终止;C2 仍按 loop_hit_count 决定干预;breadcrumb 仍注入 prompt。trace 只是旁路观测。

## 2. 数据模型(v7 migration)

### 2.1 新表 `turn_trace`

一个 turn 一行,各维度列分立(信号在不同写点到达,用 UPSERT 累积):

```sql
CREATE TABLE IF NOT EXISTS turn_trace (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id        TEXT NOT NULL,
    seq               INTEGER NOT NULL,        -- 对齐 messages.seq (per-session 计数器)
    token_usage_json  TEXT,                    -- Done.usage 5 字段,或 NULL(cancel/error 无 usage)
    compaction_json   TEXT,                    -- {tokens_before, tokens_after, dropped_count, degradation} 或 NULL(未压缩)
    loop_hint_json    TEXT,                    -- {hit_count, verdict_kind} 或 NULL(无 soft hint)
    breadcrumb_json   TEXT,                    -- {task_slug, status, breadcrumb_text} 或 NULL(无 workflow)
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    UNIQUE(session_id, seq)                    -- 每 turn 一行,UPSERT 锚点
);
CREATE INDEX IF NOT EXISTS idx_turn_trace_session_seq ON turn_trace(session_id, seq);
```

**为什么新表而非 messages 加列**:messages 是对话内容(content blocks),trace 是 harness 观测,语义不同;新表不污染 messages schema 稳定性;每 turn 一行 + UNIQUE 锚点天然支持多写点 UPSERT 累积。

### 2.2 `session_audit_events` 加 `turn_seq` 列

```sql
ALTER TABLE session_audit_events ADD COLUMN turn_seq INTEGER;  -- NULL = 历史行/无 turn 上下文
```

历史行回填 NULL(不反推,秒精度 ts 不可靠)。新行由 `record_audit_event` 传入当前 seq。

### 2.3 migration 编号

v7(紧跟 v6 Mode 3 档化 backfill,2026-06-13)。非破坏性(加表 + 加列),`run_migrations`(`migrations.rs:50`)末尾追加 v7 块。

## 3. 数据流

### 3.1 后端 emit + 落盘(trace_pipeline)

新模块 `agent/trace.rs`,提供 3 个 helper(各对应一维),每个 helper **双写**:emit ChatEvent(live)+ upsert turn_trace(回看)。

```rust
// 伪码
pub fn record_compaction(sink: &dyn ChatEventSink, db: &Db, session_id: &str, seq: i64, r: &CompactResult) {
    let payload = CompactionTrace { tokens_before, tokens_after, dropped_count, degradation: r.degradation.as_str() };
    sink.emit_chat_event(ChatEvent::ContextCompacted { seq, ..payload });     // live
    let _ = db::upsert_turn_trace_compaction(session_id, seq, &payload);      // 回看(best-effort,失败 warn!)
}
// record_loop_hint / record_breadcrumb 同构
```

per-turn token:在 `Done{usage}` 已有写点(`chat_loop.rs:1800` `update_last_turn_usage` 旁)加 `upsert_turn_trace_token(session_id, seq, usage)`。注意 worker gate:worker turn 复用 parent session_id,`update_last_turn_usage` 已 `!skip_persist` gate(`RULE-A-015`),turn_trace token 写入复用同一 gate,避免 worker 冲掉 parent。

### 3.2 写点(精确,来自调研)

| 维度 | ChatEvent 变体 | emit + 落盘写点 | 现有 tracing 旁 |
|---|---|---|---|
| C3 压缩 | `ContextCompacted{seq, tokens_before, tokens_after, dropped_count, degradation}` | `chat_loop.rs:1261`(`compact_messages` 返回后) | 现有 `tracing::info!` |
| C2 soft hint | `LoopHint{seq, hit_count, verdict_kind}` | `chat_loop.rs:2181` 附近(soft hint 注入 tool_result 前) | 现无 tracing |
| workflow breadcrumb | `WorkflowBreadcrumb{seq, task_slug, status, breadcrumb_text}` | `inject.rs:343` `append_workflow_breadcrumb` 内 | 现无 tracing |
| per-turn token | 复用 `Done{usage}`(已有) | `chat_loop.rs:1800` `update_last_turn_usage` 旁 | — |

`seq` 来源:agent loop 的 per-session `next_seq` 计数器(同 `TurnComplete.seq`)。breadcrumb 写点在 `messages[0]` 注入时,seq 取当前轮序号(inject 在 turn 循环内,seq 已知)。

### 3.3 前端 live + 回看(同构)

```
useTraceStore
  ├─ currentSessionTraces: Map<seq, TurnTrace>   // live,监听 3 新 event + TurnComplete + tool:call/result 增量
  ├─ loadHistory(sessionId)                       // 回看,invoke list_turn_traces + list_session_audit_events
  └─ TurnTrace 前端统一类型(两源都映射到它)
```

- live:`streamController` `handleChatEvent` 加 3 个 case(`context_compacted` / `loop_hint` / `workflow_breadcrumb`),upsert `currentSessionTraces`。session 切换 / `startRequest` 清空(同 `recallHitsBySession` 模式)。
- 回看:`list_turn_traces(sessionId)` 拉 `turn_trace` 行;`list_session_audit_events`(已有)拉审计,按 `turn_seq` 归组到对应 `TurnTrace.auditEvents`。
- worker turn:不冒泡到主 chat(`SubagentBufferSink` 隔离,同 `Recall`/`Retrying`),主 chat trace 不受 worker 影响。worker 自己的 trace 落 `turn_trace`(复用 parent session_id + seq)但 live 不上行主面板 — MVP 不特殊隔离,如需可加 `is_worker` 列(Phase 2)。

## 4. 契约

### 4.1 新 ChatEvent 变体(`llm/types.rs:341`,加在 `Recall` 后)

```rust
/// E2 trace: C3 context 压缩观测。emit always-on(live 面板)+ 落 turn_trace(回看)。
ContextCompacted { seq: i64, tokens_before: u32, tokens_after: u32, dropped_count: u32, degradation: String },
/// E2 trace: C2 循环 soft hint(1-2 连击,未到主动干预阈值)。
LoopHint { seq: i64, hit_count: u32, verdict_kind: String },
/// E2 trace: 每轮注入的 workflow breadcrumb 快照。
WorkflowBreadcrumb { seq: i64, task_slug: Option<String>, status: Option<String>, breadcrumb_text: String },
```

wire(`#[serde(tag="kind", rename_all="snake_case")]`):`context_compacted` / `loop_hint` / `workflow_breadcrumb`。

### 4.2 新 IPC

```rust
#[tauri::command]
pub async fn list_turn_traces(state: State<'_, Arc<AppState>>, session_id: String)
    -> Result<Vec<TurnTraceRow>, AppCommandError>
// TurnTraceRow { id, sessionId, seq, tokenUsageJson, compactionJson, loopHintJson, breadcrumbJson, createdAt }
// ORDER BY seq ASC
```

注册 `lib.rs` + `commands/mod.rs` 白名单(同 `list_session_audit_events` 模式)。

### 4.3 清理 IPC

```rust
#[tauri::command]
pub async fn clear_session_trace(state, session_id: String) -> Result<(), AppCommandError>
// DELETE FROM turn_trace WHERE session_id=? (+ 可选级联审计,turn_seq 标记的行)
```

### 4.4 record_audit_event 签名扩展

`record_audit_event` 加 `turn_seq: Option<i64>` 参数。所有调用点(21 类落表,grep `record_audit_event\|record_tool_executed_audit\|record_*_audit` 找全)传当前 seq;无 turn 上下文的调用点(如 `commands/question.rs` 的 IPC 处理器,不在 turn 循环内)传 `None`。

> 改动面:record_audit_event 签名 + 所有 record_* helper + 调用点。机械但扩散。替代方案(不改签名,用 thread-local turn 上下文)不符合 Rust 习惯,不取。

## 5. 前端面板

### 5.1 挂载:AppShell drawer(推荐)

AppShell body 从 `Sidebar + main` 扩为 `Sidebar + main + <TracePanel>(drawer)`。TracePanel 从右侧滑入,可折叠(默认收起)。理由:live 跟踪要求能同时看 chat + trace;切视图会挡 chat,违背 live 定位。

入口:AppHeader 加 trace toggle 按钮(或复用现有审计入口旁)。状态落 `useTraceStore.panelOpen`。

### 5.2 组件结构

```
<TracePanel>              // drawer shell(reka-ui 或原生 transition)
  ├─ header: session 选择(当前/历史)+ 模式(live/回看)+ 清理按钮
  ├─ <TurnTimeline>       // 主轴 seq,每 turn 一卡片
  │   └─ <TurnCard>       // seq + latency + token 分布 + compaction + loop + breadcrumb + tool calls(点开)
  │       └─ <AuditEventItem>(复用 AuditLogItem 渲染逻辑)
  └─ 空态/loading/error
```

### 5.3 渲染复用

- 事件分桶:复用 `parseAuditPayload`(`utils/audit.ts:191`)+ 13 类 icon family。
- 失败高亮:`tool_executed.exit_code != 0` / `compaction.degradation == "still_over"` → 红边(复用 `audit-item--critical` class)。
- token 分布:per-turn `token_usage_json` 5 字段(input/output/cache_creation/cache_read/context),迷你条形图(纯 CSS,复用 tokenUsage 色阶)。

## 6. 兼容性 / migration

- v7 非破坏性(加表 + 加列)。老 DB 升级自动跑 v7,历史 turn 无 trace 数据(回看显示"无 trace 记录")。
- 新 ChatEvent 变体:老前端不处理则忽略(serde tag 未匹配 → 无 case,不影响)。但本任务前后端同 PR/同 child 交付,不存在版本错配。
- `record_audit_event` 签名变:内部调用,无 wire 兼容问题。
- always-on emit + 落盘:每轮多 1 upsert + 0-3 emit。SQLite local write ~ms 级,可忽略;emit 无 listener 也 cheap。性能验收见 AC8 零回归。

## 7. Tradeoff

| 决策 | 取 | 舍 | 理由 |
|---|---|---|---|
| 落盘形态 | 新表 turn_trace | messages 加列 | 语义隔离 + UNIQUE 锚点支持多写点 UPSERT |
| turn_seq 对齐 | record_audit_event 加参数 | thread-local 上下文 | Rust 习惯 + 精确对齐是 viewer 核心价值 |
| live 数据结构 | 与回看同构 TurnTrace | 独立 live buffer | 两源统一,面板代码单路径 |
| 面板形态 | drawer(不切视图) | 独立路由视图 | live 要求同时看 chat + trace |
| worker trace | 复用 parent session_id+seq,不隔离 | 加 is_worker 列 | MVP 简单,worker live 不上行主面板已够 |

## 8. 回滚

- v7 migration:SQLite 3.35+ `DROP TABLE turn_trace` + `ALTER TABLE session_audit_events DROP COLUMN turn_seq`。
- 新 ChatEvent 变体:enum 删变体(前端无依赖即安全)。
- 新 IPC:删命令 + 注册。
- 前端 TracePanel:删组件 + drawer 挂载,AppShell 复原 `Sidebar + main`。
- trace_pipeline helper:删模块,写点复原(现有 tracing 不动)。

## 9. 风险点

- **record_audit_event 签名扩散**:21 类调用点,漏传 seq 会导致审计行 turn_seq NULL(回看时该事件不归组)。mitigate:grep 全调用点 + 编译器强制(参数非 Option 默认值需显式)。
- **breadcrumb seq 取值**:inject 在 turn 循环内,但 breadcrumb 写点 `append_workflow_breadcrumb` 是否能拿到当前 seq 需确认(可能要传 seq 参数进 inject)。implement 时验证。
- **worker turn_trace 混入 parent**:worker 复用 parent session_id,turn_trace 行会与 parent turn 混。MVP 接受(回看时按 seq 排序,worker turn seq 连续可见);如混乱再加 is_worker。
- **always-on 落盘 DB 增长**:长 session(200 turn)多 ~200 行 turn_trace + 审计 turn_seq。清理入口(AC7)兜底。
