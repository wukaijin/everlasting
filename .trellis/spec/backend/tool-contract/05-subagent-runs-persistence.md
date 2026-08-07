## Scenario: subagent_runs persistence (B6 PR2, 2026-06-20)

> **Source of truth**: the migration lives in
> `app/src-tauri/src/db/migrations.rs` (PR2 schema, 2026-06-20);
> the CRUD layer is `app/src-tauri/src/db/subagent_runs.rs`; the
> 4 MiB cap helper is `app/src-tauri/src/agent/subagent.rs::truncate_transcript_for_persistence`;
> the worker→persist bridge is `run_subagent` in
> `app/src-tauri/src/agent/chat_loop.rs` (~:1802-2200).
>
> **何时读本文**:涉及 `subagent_runs` 表 / 增删改查 helper / 4 MiB transcript cap / PR3 前端 ToolCallCard 展开 / C4 audit 查 worker 决策 / worker token usage 折算进父 session 等。
>
> 上一段 Scenario(PR1 的 `dispatch_subagent tool`)锁定 worker 注册 / 拦截路径 / SubagentBufferSink 行为 / `format_dispatch_result` 输出格式。本段锁定 worker 跑完后 transcript + token usage + summary 如何落 DB,以及**审计不污染父 session** 的分工。

### 1. Scope / Trigger

- Trigger: PR1 落地的 `SubagentBufferSink` transcript(worker chat-event / tool:call / tool:result)是**进程内 in-memory** —— 关掉 app 后 reload 主对话,worker 中间过程全丢。PR3 前端 `ToolCallCard` 展开 UI 需要持久化 transcript 才能渲染;reload-after-restart 必须仍能查;每条 worker 决策(Tier 2/3/4 collapse)需要可审计但不污染父 `session_audit_events`(审计完整性 RULE-A-016 closed B6 PR3a 2026-06-20)。
- Why code-spec depth: mandatory —— 涉及新表 migration / 5 个 CRUD helper / 4 MiB 截断 cap / streaming token_usage 累加(PR2a 顺手解 RULE-A-015 over-broad gate)/ parent-session CASCADE / audit 不污染父(transcript `PermissionAsk` vs `session_audit_events`)分工,每项都是可执行合约。
- ROADMAP §1.2 B6 Subagent PR2 计划项 + DEBT.md §"子 task 编排建议" PR1+2 拆分。

### 2. Signatures

#### `subagent_runs` schema(跟随 `session_audit_events` 模式,`db/migrations.rs` 2026-06-20 新增)

```sql
CREATE TABLE IF NOT EXISTS subagent_runs (
    id TEXT PRIMARY KEY,                                           -- UUID v4 nanoid
    parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_request_id TEXT NOT NULL,                               -- worker rid "{parent}-sub-{seq}"(非 FK,cancellations in-memory)
    subagent_name TEXT NOT NULL,                                   -- 'researcher' | 'general-purpose'
    status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error')),
    started_at TEXT NOT NULL,                                      -- ISO 8601 RFC 3339
    finished_at TEXT,                                              -- NULL = running
    token_usage_json TEXT,                                         -- JSON TokenUsage { input / output / cache_creation / cache_read }
    summary TEXT,                                                  -- final_text 纯文本(无 status 前缀;PR2 PRD 决策 #3)
    transcript_json TEXT,                                          -- JSON Vec<TranscriptEntry>(4 MiB cap 后)
    transcript_truncated INTEGER NOT NULL DEFAULT 0,               -- 1 = 超过 4 MiB cap
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
    ON subagent_runs(parent_session_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
    ON subagent_runs(parent_request_id);
```

CASCADE 要求 `PRAGMA foreign_keys = ON`(`init_pool` 在每个 connection 首次启用),与 `session_audit_events` 一致。

#### `db::subagent_runs` module API(`app/src-tauri/src/db/subagent_runs.rs`)

```rust
// DB-side status 枚举,字符串与 migration CHECK 约束一一对应
#[serde(rename_all = "lowercase")]
pub enum SubagentStatusDb { Running, Completed, Cancelled, Error }
impl SubagentStatusDb {
    pub fn as_str(&self) -> &'static str;  // wire form, lockstep with CHECK
    pub fn from_str_opt(s: &str) -> Self;  // lenient parse:unknown → Running
}

// Row 形状(IPC 边界 camelCase)
#[derive(FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunRow {
    pub id: String, pub parent_session_id: String, pub parent_request_id: String,
    pub subagent_name: String, pub status: String, pub started_at: String,
    pub finished_at: Option<String>, pub token_usage_json: Option<String>,
    pub summary: Option<String>, pub transcript_json: Option<String>,
    pub transcript_truncated: i64, pub created_at: String,
}

// 5 个公开 helper
pub async fn insert_run(pool, parent_session_id, parent_request_id, subagent_name)
    -> Result<String, sqlx::Error>;                            // → id; status='running' + 空 TokenUsage + '[]' transcript

pub async fn update_run_finished(
    pool, id, status: SubagentStatusDb, finished_at, summary,
    token_usage: &TokenUsage, transcript_json: &str, truncated: bool,
) -> Result<(), sqlx::Error>;

pub async fn get_run(pool, id) -> Result<Option<SubagentRunRow>, sqlx::Error>;
pub async fn list_runs_by_session(pool, parent_session_id) -> Result<Vec<SubagentRunRow>, sqlx::Error>;

pub async fn add_token_usage_streaming(
    pool, parent_session_id: &str, usage: &TokenUsage,
) -> Result<(), sqlx::Error>;                                // 复用 db::add_token_usage,但语义独立(worker streaming)
```

#### 4 MiB transcript cap(`agent/subagent.rs::truncate_transcript_for_persistence`)

```rust
pub const TRANSCRIPT_MAX_BYTES: usize = 4 * 1024 * 1024;  // 4 MiB

/// 纯函数。serialize `Vec<TranscriptEntry>` → JSON 字节数:
/// - ≤ cap → 原样返回 (transcript, truncated=false)
/// - > cap → 保留 head + tail 各 half 字节,重新 parse,标记 truncated=true
/// - 极端 case(head/tail 落在单元素 JSON 边界)→ 退化为保留 head 字节,
///   transcript_truncated=1 信号让 PR3 UI 显示 "transcript truncated" badge
pub fn truncate_transcript_for_persistence(
    transcript: Vec<TranscriptEntry>, max_bytes: usize,
) -> (Vec<TranscriptEntry>, bool);
```

### 3. Contracts

#### `insert_run` 行为(`run_subagent` 启动时立即调)

- 生成 UUID v4 nanoid 作为 `id`,`started_at = Utc::now().to_rfc3339()`。
- `status='running'`,`finished_at=NULL`,
  `token_usage_json = serde_json::to_string(&TokenUsage::default())`(全 0 占位,后续 UPDATE 覆盖),
  `summary=NULL`,`transcript_json='[]'`,`transcript_truncated=0`。
- 返回 `id` 给 `run_subagent` 作为 `worker_run_id`,PR3 展开 UI 用此 id 调 `get_run`。
- 失败:`Result::Err(sqlx::Error)`(FK 违反说明 parent_session_id 不存在,罕见;`run_subagent` 走 warn+continue 不 block dispatch)。

#### `update_run_finished` 行为(`run_subagent` 跑完时一次性调)

- `status` 由 caller 决定:`Completed` / `Cancelled` / `Error`(`Cancelled` 由 `was_cancelled()` 触发;`Error` 由 `had_error()` 触发;两者皆 false 走 `Completed`)。
- `finished_at = Utc::now().to_rfc3339()`(terminal 必填;DB 没 NOT NULL 但契约层强制)。
- `summary = worker_sink.final_text()`(PR2 PRD #3 决策:**纯文本,无 `[status: ...]` 前缀**;`status` 字段独立,前端重组展示时按 status 加前缀)。
- `token_usage_json = serde_json::to_string(&token_usage_sum)`,`token_usage_sum` 是 worker 全 turn 累加(`sink.cumulative_usage()` 返回的 4 字段 TokenUsage)。
- `transcript_json = serde_json::to_string(&truncate_transcript_for_persistence(worker_sink.transcript_snapshot(), TRANSCRIPT_MAX_BYTES).0)`,`transcript_truncated` 是 cap 函数的第二个返回(`bool` → i64)。
- 失败:best-effort —— `tracing::warn!` + 继续。dispatch_subagent 的 `tool_result` 才是用户可见信号;DB 持久化失败不应阻止 user 收到 worker 产出(RULE-A-003 风格)。

#### `add_token_usage_streaming` 行为(worker 每 turn 调,与 `update_run_finished` 互补)

- 走 `db::add_token_usage` 同源(对 `sessions` 表的 `input_tokens_total` / `output_tokens_total` 等列累加),语义独立为"worker streaming"版本,避免被 `skip_persist` gate 误拦(PR2a RULE-A-015 修复)。
- 在 `run_chat_loop` 的 turn 边界(已有 `add_token_usage(db, &session_id, &turn_usage)` 调用)拆出来 —— worker 路径用 `parent_session_id`(`&parent_session_id` 而非 `&session_id`)。父 UI 可在 worker 跑期间看到 token counter 实时上涨(不是 worker 跑完才一次性更新)。
- 失败:best-effort warn+continue(token_usage 是元数据,不污染 messages 表也不参与 messages.seq,失败不应中断 agent loop)。
- 测试 `subagent_runs_token_usage_streaming_missing_session_is_noop` 锁定:parent_session_id 不存在时返回 Ok(())(实际场景不会发生,但防御性一致 —— 与 `update_message_metadata` 的"未知 seq → noop"对称)。

#### `get_run` / `list_runs_by_session` 行为(PR3 API 表面)

- `get_run(id)` 按主键查,None 表示 id 不存在,Some 返回 `SubagentRunRow`。
- `list_runs_by_session(parent_session_id)` 按 `(parent_session_id, started_at DESC)` 索引扫描,返回该 session 所有 worker run(包含 `running` 中,前端可显示 "5 workers active" 状态徽章)。
- IPC surface:`#[serde(rename_all = "camelCase")]` 跨 Tauri 2 边界(与所有 `db::*Row` 一致),前端 TS 直接读 `row.id` / `row.parentSessionId` / `row.transcriptJson` 等。

#### `SubagentStatusDb::as_str` ↔ migration CHECK 约束的 lockstep

- `as_str()` 返回的字符串必须与 migration 的 `CHECK(status IN ('running','completed','cancelled','error'))` 精确一致;未来增加新状态(如 `'timed_out'`)必须**同时改** migration CHECK + 枚举 + 测试 `audit_kind_round_trip` 类。`from_str_opt` 对 unknown 字符串 lenient 解析为 `Running`(forward-compat 兜底,见 `database-guidelines.md` "Enum pattern: lenient parse for forward-compat")。

#### `transcript_json` cap 4 MiB 的取舍(PR2 PRD #2 决策)

- 4 MiB = `4 * 1024 * 1024 = 4_194_304 bytes`。SQLite TEXT 默认上限 1 GiB,4 MiB 远在安全阈值内;实际 20 turn worker transcript 在 busy tool-use 场景约 100 KiB(3 个数量级 below cap),4 MiB 是"防 worst case 单 worker 撑爆 DB"的兜底,不是"贴近真实使用"的量级。
- 超过 cap → `truncate_transcript_for_persistence` 头+尾各半 + `transcript_truncated=1` 标记。PR3 展开 UI 看到 `truncated=true` 时显示 "transcript truncated, show full via..." 提示(具体 UX 留 PR3 设计)。

#### token_usage 折算父 session 的 streaming 设计

- worker `run_chat_loop` 内部 turn 边界已有 `add_token_usage(db, &session_id, &turn_usage)` 调用。worker 复用 `parent_session_id`,且 PR2a 拆出 `add_token_usage_streaming` 版本(独立于 `skip_persist` gate,见 RULE-A-015)。Streaming 路径:worker 每 turn 调 `add_token_usage_streaming(pool, &parent_session_id, &turn_usage)` → 直接累加进父 `sessions.input_tokens_total` 等;**不**经过 `update_run_finished`(那里只算 worker's own `token_usage_json` 字段,作为 run-level snapshot,不是累加源)。
- 测试 `agent_loop_dispatch_subagent_token_usage_folds_into_parent` 锁定:worker 跑 2 turn,父 session `input_tokens_total` 等于 2 turn 累加值;`subagent_runs.token_usage_json` 解码后是同一 sum(两个来源一致)。

#### audit 不污染父的分工(RULE-A-016 closed B6 PR3a 2026-06-20)

- **当前行为(post-PR3a)**:worker 路径**不**调 `record_audit_event`(由 `run_chat_loop` 函数体内的 16 个 `if !skip_persist` gate 自动实现 —— `record_*_audit` 调用在 gate 内);worker 的 ⑨ 决策存于 transcript 的 `TranscriptKind::PermissionAsk` 事件。
- **Tier 4 `ask_path` worker 分支(post-PR3a)**:`ask_path` 顶部 `if ctx.is_worker { ... }` collapse 路径**不**调 `record_audit_event`;改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 记录 `TranscriptKind::PermissionAsk` entry 到 worker transcript(PR3 drawer 可见)。父 `session_audit_events` 不被 worker 决策污染。
- **Worker ask resolve outcome (2026-06-22, RULE-WorkerAsk-001)**:post-FRONT-001 fix,worker `ask_path` 的 `tokio::select!{cancel, timeout, oneshot}` 返回后,`SubagentBufferSink::emit_permission_ask_resolved(&rid, outcome)` 写 `TranscriptKind::PermissionAskResolved` transcript entry(payload `{rid, outcome}`,outcome ∈ `{"allow","deny","timeout","cancel"}`;worker `AllowAlways` 当 `AllowOnce` 统一记 `"allow"`;`OneshotDropped` 记 `"cancel"`)。trait `ChatEventSink::emit_permission_ask_resolved` **默认 no-op**,仅 `SubagentBufferSink` override(避免 `Arc<dyn>` downcast,`AppHandleSink` / 测试 sink 零改动继承默认)。**transcript-only** — 不双发 `permission:ask` IPC;live 期间交互卡从 interactive 翻 historical 由 permissions store rid removal 驱动(Session 62 `89e5ba1` 行为不变)。前端 `pairSections` 按 `rid` 预扫 `PermissionAskResolvedSection` → 给 `PermissionAskSection.outcome` 透传 → `<PermissionAskBody>` historical 分支显 ✓已允许 / ✗已拒绝 / ⏱已超时 / ⊘已取消 badge(色 token 复用 `--color-tool-write` / `--color-tool-error` / `--color-text-muted`,无 one-off 新 token)。pre-fix 老 transcript(无 `PermissionAskResolved` entry)降级显中性 "worker asked for X",不 crash(`pairSections` 配对找不到 resolved → `outcome` undefined → `<PermissionAskBody>` 走 neutral 分支)。
- **测试覆盖**:`audit_not_polluted_by_worker`(researcher silent allow / Tier 5)断言父 audit `delta == 2`;`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(general-purpose + Edit + write_file 触发 Tier 4 ask_path collapse)断言父 `tool_denied count == 0` + worker transcript `PermissionAsk count == 1` + audit delta ≤ 2。两个场景都覆盖。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `insert_run` parent_session_id 不存在 | FK 违反 → `Err(sqlx::Error::Database)`;`run_subagent` warn+continue 走 `format_dispatch_result(SubagentStatus::Error, "spawn failed")` |
| `update_run_finished` 重复调(同 id 调 2 次) | 第 2 次覆盖(running → terminal 是单向,terminal → terminal 是覆盖);不影响 DB 完整性 |
| `update_run_finished` 时 worker 0 turn(`transcript` 空) | `transcript_json = '[]'`,`transcript_truncated = 0`,`summary` 走 `final_text` 空文本退化(`(worker produced no final text)`) |
| `transcript` serialize 后 > 4 MiB | `truncate_transcript_for_persistence` 头+尾各半;`transcript_truncated=1`;DB 写 OK |
| `transcript` 极端 case:head 字节落在单元素 JSON 边界 | `truncate_transcript_for_persistence` 退化为保留 head 字节(single-element vector);`truncated=true`;JSON 可能不严格 valid 但 `transcript_truncated=1` 标记已警示 |
| `add_token_usage_streaming` parent_session_id 不存在 | `Ok(())` 静默 noop(与 `update_message_metadata_on_unknown_seq_is_noop` 防御模式一致) |
| `get_run(id)` 未知 id | `Ok(None)`(前端 PR3 展示 "run not found",不是 Err) |
| `list_runs_by_session` 空 session | `Ok(vec![])` |
| 删父 session | `ON DELETE CASCADE` 同步删所有 `subagent_runs` 行;测试 `subagent_runs_cascade_delete_with_parent_session` 锁定 |
| `status='running'` 行无 `finished_at` | 运行时(running)合法;terminal update 后 `finished_at` NOT NULL |
| RULE-A-016 已修复(post-PR3a) | Tier 4 collapse worker 分支 emit `PermissionAskPayload` via sink → worker transcript;**不**写父 `session_audit_events`(PR3a 修复,见 §3) |

### 5. Good / Base / Bad Cases

**Good**(happy path):parent turn 1 派 `general-purpose`("分析 /tmp/下的 .log 文件")→ worker 跑 3 turn(读 + grep + 总结)→ final_text "找到 3 个 .log,最大的 2GB..." → `run_subagent`:
1. INSERT subagent_runs row: status='running', summary=NULL, transcript_json='[]'
2. 3 turn 各自 streaming 累加进父 sessions.input_tokens_total
3. UPDATE subagent_runs row: status='completed', finished_at=NOW, summary="找到 3 个 .log...", token_usage_json=sum, transcript_json=[3 turn events] (JSON ~5 KiB), transcript_truncated=0
→ parent 构造 ContentBlock::ToolResult `[status: completed]\n找到 3 个 .log...`,tool_use/tool_result 配对,parent turn 2 继续。用户 reload 主对话,ToolCallCard 展开时调 `get_run(worker_run_id)` 取 transcript 渲染。

**Base**(cancel 中途):parent_token 在 worker turn 2 cancel → worker_token child 触发 → SubagentBufferSink.was_cancelled=true → status=Cancelled → `update_run_finished`: status='cancelled', finished_at=NOW, summary="partial result\n\n[CANCELLED_MARKER]", transcript_json 包含 cancel 之前 2 turn 的事件 + `Done{cancelled}` terminal 事件(PR2a 修复 RULE-A-015 后 terminal Done emit 必到 sink), transcript_truncated=0。父 UI reload 后 ToolCallCard 展开看到 "cancelled at turn 2" + 中间 2 turn 的 transcript(可读性 + 可恢复性的中间态)。

**Good**(RULE-A-016 fixed B6 PR3a):parent turn 1 派 `general-purpose` + main=Edit + `write_file` 触 Tier 4 ask → worker Tier 4 ask_path 立即 `Decision::Deny`(`is_worker=true` collapse),`format_dispatch_result(SubagentStatus::Error, "denied: ...")` 回 parent,tool_result `is_error=true`;PR3a 修复后,worker 分支**不**写 `record_audit_event(ToolDenied)`,改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 记录 `TranscriptKind::PermissionAsk` entry 到 worker transcript → 父 `session_audit_events` 不被污染,C4 audit log UI 只看父自己的 ⑨ 决策;PR3 drawer 可见 worker 的 deny 决策(transcript 中)。

**Bad**(RULE-A-015 已修但 spec 留档):如果 PR2a 没拆 `add_token_usage` 和 terminal `Done` emit 2 处 `skip_persist` gate,worker 跑完时 `subagent_runs.status` 永远='completed'(cancelled 路径也走 Completed 因为 `was_cancelled=false`,sink 没收到 terminal Done),`sessions.input_tokens_total` 永远是 parent dispatch 前的值(worker 期间的 token 没 streaming 进来)。回归测试 `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled` 锁定 fix。

### 6. Tests Required

**Unit**(`db/tests.rs::subagent_runs_*`,PR2a 7 个):

| Test | 断言 |
|---|---|
| `subagent_runs_insert_creates_running_row` | INSERT 后 row 存在 + status='running' + finished_at=NULL + transcript_json='[]' + transcript_truncated=0 + token_usage_json 解码为全 0 TokenUsage |
| `subagent_runs_update_finished_sets_status_and_fields` | UPDATE 后 status 切到 terminal + finished_at NOT NULL + summary = 输入 final_text + token_usage_json 解码 = 输入 TokenUsage + transcript_json 包含 emit 的事件 |
| `subagent_runs_update_finished_records_truncated_flag` | UPDATE 时 truncated=true → transcript_truncated=1;transcript_json 包含 head+tail 头尾(中间段已截断) |
| `subagent_runs_cascade_delete_with_parent_session` | 2 个 subagent_runs row 挂在 s1 → 删 s1 → 2 row 同步消失(RULE-CASCADE FK 生效) |
| `subagent_runs_list_by_session_orders_by_started_desc` | 2 row 不同 started_at → list 返回按 started_at DESC 排序 |
| `subagent_runs_token_usage_streaming_accumulates_in_parent` | 2 turn usage 调 streaming 累加 → parent session input_tokens_total = 2 turn sum(解 add_token_usage 路径) |
| `subagent_runs_token_usage_streaming_missing_session_is_noop` | parent_session_id 不存在 → Ok(())(防御性,无 panic / Err) |

**Unit**(`subagent.rs::tests`,PR2a truncate 函数 ~5 个):
- `truncate_transcript_under_cap_passes_through` (transcript 字节 < cap → 原样返 + truncated=false)
- `truncate_transcript_over_cap_keeps_head_and_tail` (transcript 字节 > cap → 头尾各半 + truncated=true)
- `truncate_transcript_empty_input` (空 transcript → 返回 ([], false))
- `truncate_transcript_respects_max_bytes_constant` (TRANSCRIPT_MAX_BYTES == 4*1024*1024 常量锁)
- `truncate_transcript_handles_extreme_head_fallback` (极端:head parse fail → 退化为 head 字节 single-element)

**Integration**(`agent/tests.rs::agent_loop_dispatch_subagent_*`,PR2a 4 + PR2b 1 = 5 个):

| Test | 断言 |
|---|---|
| `agent_loop_dispatch_subagent_persists_subagent_run` | 跑完 worker → subagent_runs row 存在 + status='completed' + summary=worker_text + transcript_json 非空 + transcript_truncated=0 + transcript_snapshot() 包含 emit 的 chat-event / tool:call / tool:result |
| `agent_loop_dispatch_subagent_cancelled_persists_status_cancelled` | parent_token cancel 中途 → status='cancelled' + finished_at NOT NULL(RULE-A-015 回归:terminal Done emit 不在 skip_persist gate) |
| `agent_loop_dispatch_subagent_audit_not_polluted_by_worker` | researcher worker(silent allow)→ 父 session_audit_events 行数 == parent 自己的 ⑨ 决策行数(worker Tier 5 不写 audit) |
| `agent_loop_dispatch_subagent_token_usage_folds_into_parent` | worker 跑 2 turn → 父 sessions.input_tokens_total = 2 turn 累加 + subagent_runs.token_usage_json = 同一 sum |
| `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` | B6 PR2b + RULE-A-014 + RULE-A-016:parent Edit mode + general-purpose + write_file → `tokio::time::timeout(15s)` 包裹;worker Tier 4 ask_path 立即 Decision::Deny(无 oneshot 等待,无挂起)→ tool_result is_error=true + deny 原因;**post-PR3a** worker deny 不写父 audit,改走 sink → transcript PermissionAsk(1 entry);父 audit `tool_denied count == 0` + audit delta ≤ 2|

### 7. Wrong vs Correct

#### Wrong: `SubagentBufferSink` 直接调 `update_run_finished`(无 4 MiB cap)

```rust
// BAD — transcript 直接序列化,worker busy 时可能撑到数十 MB;
//      单 worker 撑爆 DB,慢 reload,UI 渲染卡顿
let transcript_json = serde_json::to_string(&worker_sink.transcript_snapshot()).unwrap();
db::subagent_runs::update_run_finished(
    &pool, &worker_run_id, status, finished_at, &summary, &token_usage,
    &transcript_json, false,  // ← truncated 永远 false,真实体积不可见
).await?;
```

**Why it's wrong**:无 cap → DB 体积不可控 → 慢 reload → 共享 DB 资源争抢。4 MiB 是 PR2 PRD #2 决策的硬上限,必须经过 `truncate_transcript_for_persistence` 后再 serialize。

#### Correct: `run_subagent` 调 `truncate_transcript_for_persistence` → `update_run_finished`

```rust
// GOOD — cap 在 persist 前必经
let (transcript_capped, truncated) =
    truncate_transcript_for_persistence(worker_sink.transcript_snapshot(), TRANSCRIPT_MAX_BYTES);
let transcript_json = serde_json::to_string(&transcript_capped).unwrap_or_else(|_| "[]".into());
db::subagent_runs::update_run_finished(
    &pool, &worker_run_id, status, finished_at, &summary, &token_usage_sum,
    &transcript_json, truncated,
).await?;
```

`truncated=true` 透传到 DB 的 `transcript_truncated=1`,PR3 展开 UI 检测到时显示"transcript truncated"提示(UX 留 PR3)。

#### Wrong: worker 决策也写 `session_audit_events`(污染父)

```rust
// BAD — 复用现有 record_audit_event 路径,不分 worker/parent
if let Decision::Deny { reason, critical: _ } = decision {
    record_audit_event(&db, &ctx.session_id, AuditKind::ToolDenied.as_str(), Some(...))
        .await;  // ← ctx.session_id 是 parent_session_id(worker 复用),污染父 audit
    return ToolResult { is_error: true, content: reason };
}
```

**Why it's wrong**:worker 复用 `parent_session_id`,`record_audit_event` 不区分 worker → worker 的 Tier 4 collapse 决策写进父的 `session_audit_events` → C4 audit log 看到 worker 决策,混淆责任归属(RULE-A-016)。

#### Correct: worker 决策走 transcript PermissionAsk,父 audit 仅 parent 自己决策

```rust
// GOOD — 入口先看 ctx.is_worker,worker 改写 transcript
if let Decision::Deny { reason, critical: _ } = decision {
    if ctx.is_worker {
        // worker 路径:append PermissionAsk 事件到 SubagentBufferSink transcript,
        //              transcript 已在 PR2 落 subagent_runs.transcript_json
        //              (PR3 展开 UI 看到 "permission: ask, denied: ..." 事件流)
    } else {
        record_audit_event(&db, &ctx.session_id, AuditKind::ToolDenied.as_str(), Some(...))
            .await;
    }
    return ToolResult { is_error: true, content: reason };
}
```

PR3a implementation matches this Correct pattern exactly (see `permissions/mod.rs::ask_path` worker branch post-PR3a): the `if ctx.is_worker { ... }` block emits a `PermissionAskPayload` via `sink.emit_permission_ask(...)` (which the `SubagentBufferSink` impl records as a `TranscriptKind::PermissionAsk` transcript entry) instead of calling `record_audit_event`. The `audit_not_polluted_by_worker` test (`delta == 2`) + the `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` test (`tool_denied count == 0` in parent audit + `permission_ask count == 1` in worker transcript + audit delta ≤ 2) lock the post-fix invariant.

### 8. Design Decisions

#### Decision: streaming 累加 token_usage(每 turn 调,父 UI 实时可见)

- **Context**:worker 复用 `parent_session_id`,且 `run_chat_loop` turn 边界已有 `add_token_usage(db, &session_id, &turn_usage)` 调用。被 16 处 `if !skip_persist` gate 守护(PR1 spec 写 18,PR2a RULE-A-015 拆出 2 处后),但 `add_token_usage` 本身不在 messages 表的 `(session_id, seq)` UNIQUE 范围内,理应不被 gate。
- **Decision**:worker 路径每 turn 调 `db::subagent_runs::add_token_usage_streaming(pool, &parent_session_id, &turn_usage)`,语义独立(不依赖 `skip_persist` gate 的当前值),直接累加进父 `sessions.input_tokens_total` / `output_tokens_total` / `cache_creation_input_tokens` / `cache_read_input_tokens`。父 UI 在 worker 跑期间看到 token counter 实时上涨,不是 worker 跑完才一次性跳。
- **Why 不一次性累加**(worker 跑完 `update_run_finished` 时 add 整个 sum):streaming 让用户感知 worker 进度(类比 Claude Code 跑任务时 token counter 滚动);一次性累加则 worker 跑 30 秒内父 UI 看到 counter 冻住,UX 差。无功能差异,纯 UX 决策。
- **Why 拆出独立 `add_token_usage_streaming` 函数**(而非改 `add_token_usage` + 检查 `is_worker`):语义清晰、worker 路径显式、PR2 单元测试可独立测(无 worker / run_chat_loop 上下文)。

#### Decision: audit 不污染父 — worker 决策走 transcript `PermissionAsk` 而非 `session_audit_events`

- **Context**:worker 复用 `parent_session_id` 是必须的(PR1 决策 5:`UNIQUE (session_id, seq)` 约束 + DB linkage 简化),但 `record_audit_event` 用 `ctx.session_id` 写入,**不分 worker / parent** → worker 的 ⑨ 决策会被审计行误写到父 `session_audit_events` 表。
- **Decision**:worker 路径的 ⑨ 决策(Deny / ToolExecuted / PermissionAsk)**不写**父 `session_audit_events`;由 `SubagentBufferSink` 的 `transcript` 累积 `TranscriptKind::PermissionAsk` 事件,PR2 落 `subagent_runs.transcript_json`。前端 C4 audit log 查父 session 时不显示 worker 决策行(`audit_not_polluted_by_worker` 测试锁定 —— researcher silent allow 场景);RULE-A-016(closed B6 PR3a 2026-06-20)已修 Tier 4 collapse 路径 —— `ask_path` worker 分支不调 `record_audit_event(ToolDenied)`,改为 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 写 transcript `PermissionAsk` entry。`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` 测试断言反转:`tool_denied count == 0` in parent audit + transcript `PermissionAsk count == 1` + audit delta ≤ 2。
- **Post-FRONT-001 (2026-06-22, RULE-WorkerAsk-001) extension**:worker ask resolve 后,`SubagentBufferSink::emit_permission_ask_resolved` 追加一个 `TranscriptKind::PermissionAskResolved` entry 携带 `{rid, outcome}`(outcome ∈ `{"allow","deny","timeout","cancel"}`)。这条 entry **不进** `permission:ask` IPC(避免与 live interaction 路径双发混淆)、**不进**父 `session_audit_events`(同 RULE-A-016 隔离原则)。**transcript-only** 是 outcome 的唯一归属;前端 `pairSections` 按 `rid` 配对 → `<PermissionAskBody>` historical badge。这是 transcript-as-audit 模式的自然延伸 —— worker ask 现在在 transcript 里有完整 `PermissionAsk` + `PermissionAskResolved` 两段,historical 回放可看到决策 + 结果。
- **Why 不直接给 `record_audit_event` 加 `is_worker` 参**:保持 record_audit_event 接口稳定(已 11+ 调用点),在 caller 层(if-else 写 audit / 写 transcript)显式区分;新增 `is_worker` 参会让所有 caller 多 1 个无关参数,影响面大。`if !ctx.is_worker` 入口守门是更小的 change。

#### Decision: 4 MiB transcript cap(防单 worker 撑爆 DB)

- **Context**:transcript 是 `Vec<TranscriptEntry>` 序列化,1 turn busy tool-use ≈ 2-5 KiB,20 turn worker 实际约 100 KiB;但恶意 LLM 或未来结构变化(thinking + extended thinking + parallel batch)可能把单 worker 推到数 MB 甚至 GB。SQLite TEXT 默认上限 1 GiB,worker 单条超 100 MB 已属异常。
- **Decision**:`TRANSCRIPT_MAX_BYTES = 4 * 1024 * 1024 = 4 MiB`,由 `truncate_transcript_for_persistence` 强制;超 cap 时头+尾各半(每半 2 MiB)保留 + `transcript_truncated=1` 标记 + DB 写仍 OK(降级,从不拒写)。
- **Why 4 MiB(不是 1 MiB / 16 MiB / 64 MiB)**:**远超真实使用**(20 turn worker ≈ 100 KiB,3 个数量级 below);**远在 SQLite TEXT 安全阈值内**(1 GiB,5 个数量级 below);**降级可读性**(head+tail 各 2 MiB 仍可读,不是粗暴截断末位)。比 1 MiB 宽(给 4-5 turn parallel batch + extended thinking 留足);比 16 MiB 严(防单 worker 占太大,多 worker 并发时不相互饿死)。
- **Why 降级不拒写**:worker 已经跑完 token 烧了,用户已经付出等待成本,即使 transcript 超 cap 也应落 DB(降级可读);拒写会让 worker 跑完但 user 看不到中间过程,UX 更差。`transcript_truncated=1` 给前端足够信号展示"transcript 截断"提示,用户知情。

---
