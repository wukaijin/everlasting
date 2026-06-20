# B6 PR2: subagent_runs 持久化

## Goal

把 PR1 落地的 `SubagentBufferSink` transcript(worker 中间过程)从**进程内 in-memory** 升级到**SQLite 持久化**到 `subagent_runs` 表,供 PR3 前端 `ToolCallCard` 展开 + 后端 audit/token 归属 + reload 不丢。承接 PR1 决策 5(audit/token 不污染父 session)+ spec §Scenario: dispatch_subagent tool §3 已定的 schema。

## What I already know(已读源码 + spec)

### 现有 migration 风格(`db/migrations.rs`)
- `CREATE TABLE IF NOT EXISTS` 幂等 + `ALTER TABLE ... ADD COLUMN` probe 补列
- `id TEXT PRIMARY KEY`(nanoid,跟 projects/sessions 一致)或 `INTEGER PRIMARY KEY AUTOINCREMENT`(session_audit_events)
- 时间戳 `TEXT`(ISO 8601 / RFC 3339,`datetime('now')` 默认)
- FK 用 `REFERENCES sessions(id) ON DELETE CASCADE`(audit 删 session 一起删)
- 索引 `idx_xxx_session_ts` 模式: `(session_id, ts DESC)`

### TokenUsage 字段(`llm/types.rs:316`)
```rust
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}
```

### session_audit_events schema(`migrations.rs:428`)
- `id INTEGER PK AUTOINCREMENT, session_id TEXT FK CASCADE, ts TEXT, kind TEXT, payload_json TEXT`
- `add_token_usage` 在 sessions 表累积(测试 `add_token_usage_accumulates_across_turns` 锁定)

### PR1 spec §Scenario: dispatch_subagent tool §3 已锁定的 schema
- `subagent_runs(id, parent_session_id, parent_request_id, subagent_name, status, started_at, finished_at, token_usage_json, summary, transcript_json)`
- INSERT 时机:worker 启动 INSERT(running)+ worker 完成 UPDATE(finished_at + status + summary + token_usage)
- audit 不污染父:worker ⑨ 决策存于 transcript 的 `TranscriptKind::PermissionAsk` 事件(不写 session_audit_events)
- token_usage 汇总进父 session 累积(用户见总消耗)

### PR1 暴露的 API
- `SubagentBufferSink::transcript_snapshot() -> Vec<TranscriptEntry>`(`subagent.rs:455`)
- `SubagentBufferSink::final_text() -> String` / `had_error()` / `was_cancelled()`
- `format_dispatch_result(SubagentStatus, &str) -> (String, bool)`

### 待 PR1 caller 接入位置
- `chat_loop.rs:1802` `run_subagent` 函数,worker 嵌套调 `run_chat_loop` 完成后,在 `worker_text = worker_sink.final_text()` 那段(约 `:1967`)后,需要调用新的 PR2 persist helper。

## Open Questions(待用户拍板)

1. **[scope ✅ resolved] RULE-A-014 修复归属** → **PR2 顺手修,关闭 DEBT**:`thread is_worker: Option<bool>` 到 `run_chat_loop` 第 21 参(None = 走 session row 默认 false);嵌套 worker 路径传 `Some(true)`;+ 端到端测试(general-purpose + Edit mode + 写工具应 deny 不挂起)+ DEBT RULE-A-014 Closed At 填 commit hash。
2. **[scope ✅ resolved] transcript 大小 cap** → **4MB**:transcript 落 DB 前截断到 4MB(SQLite TEXT 默认 1GB 上限下的安全阈值,远超 20 turn worker 实际需要);超过标记 truncated=true + 保留首尾片段。
3. **[scope ✅ resolved] summary 字段语义** → **`final_text` 纯文本,status 字段独立**:subagent_runs.summary 存 `SubagentBufferSink::final_text()` 纯文本;status 字段存 `SubagentStatus::{Completed,Cancelled,Error}` enum。前端重组展示时根据 status 加前缀。语义清晰。
4. **[impl]** token_usage 汇总进父 session 时机 — worker 跑完时一次性累加,还是 worker 跑期间 streaming 累积?

## Requirements (evolving)

- [R1] subagent_runs migration(跟随现有 v6/v7 migration 模式:CREATE TABLE IF NOT EXISTS + CASCADE + indexed ts + ALTER probe if needed)
- [R2] subagent_runs schema:`id TEXT PK / parent_session_id TEXT FK CASCADE / parent_request_id TEXT NOT NULL / subagent_name TEXT NOT NULL / status TEXT NOT NULL CHECK(...) / started_at TEXT NOT NULL / finished_at TEXT / token_usage_json TEXT / summary TEXT / transcript_json TEXT / created_at TEXT NOT NULL DEFAULT (datetime('now'))`
- [R3] worker 启动 INSERT running row + 跑完 UPDATE status/finished_at/summary/token_usage/transcript(对标 PR1 spec §3)
- [R4] **PR2 顺手修 RULE-A-014**:`run_chat_loop` 加第 21 参 `is_worker: Option<bool>`(None = 走 session row 默认 false);嵌套 worker 路径传 `Some(true)`;run_chat_loop 内部 PermissionContext 构造读这字段;production + 25 测试 + 新 worker 测试调用点更新
- [R5] token_usage 汇总进父 session 累积(复用 `add_token_usage`)
- [R6] audit 不污染父(worker ⑨ 决策存于 transcript PermissionAsk,`record_audit_event` worker 路径不调)
- [R7] DEBT.md `RULE-A-014` Closed At 填 PR2 commit hash;DEBT §优先级分布 表 P2 22→21(RULE-A-014 closed)

## Acceptance Criteria (evolving)

- [AC1] PR1 已有的 706 tests 全 pass(0 新 warning)
- [AC2] 新增 PR2 测试:subagent_runs INSERT / UPDATE / CASCADE delete / transcript 截断 cap / token_usage 汇总进父 session
- [AC3] **RULE-A-014 端到端测试**:`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` —— worker general-purpose + Edit mode + write_file 触发 Tier 4 ask → 不挂起,正常 deny 返回 tool_result is_error=true
- [AC4] C4 audit log 查父 session 不显示 worker ⑨ 决策行(session_audit_events 无 worker 行)
- [AC5] reload 主对话后,subagent_runs.transcript_json 仍可查(持久化生效)
- [AC6] `cargo test --lib` 全 pass,0 warning

## Definition of Done

- subagent_runs migration(跟随现有 v6/v7 migration 模式)
- worker 启动 INSERT running row + 跑完 UPDATE status/finished_at/summary/token_usage/transcript
- transcript 截断 cap 生效(防止单 worker 撑爆 DB)
- token_usage 汇总进父 session 累积(`add_token_usage` 已存在,复用)
- audit 不污染父(session_audit_events 不增 worker 行)
- Rust 集成测试覆盖 INSERT / UPDATE / token 累积 / CASCADE delete
- `cargo test --lib` 全 pass,0 warning
- spec 沉淀到 `tool-contract.md`(Scenario: subagent_runs persistence)+ `database-guidelines.md`(新表模式)

## Out of Scope (explicit)

- PR3 前端 ToolCallCard 展开 UI(留 PR3)
- worker 嵌套(worker 派 worker)—— MVP 禁嵌套不变
- 异步 fan-out `dispatch_subagents` plural
- Markdown frontmatter subagent 定义加载
- worker 独立 model
- subagent transcript 实时流可见(进行中)

## Technical Approach(摘要)

### subagent_runs 表
```sql
CREATE TABLE IF NOT EXISTS subagent_runs (
  id TEXT PRIMARY KEY,                           -- nanoid
  parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  parent_request_id TEXT NOT NULL,              -- worker rid (关联父 cancel/audit)
  subagent_name TEXT NOT NULL,                  -- researcher / general-purpose
  status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error')),
  started_at TEXT NOT NULL,                     -- ISO 8601
  finished_at TEXT,                             -- NULL = running
  token_usage_json TEXT,                        -- TokenUsage JSON { input/output/cache_creation/cache_read }
  summary TEXT,                                 -- final_text 纯文本(无 status 前缀)
  transcript_json TEXT,                         -- Vec<TranscriptEntry> JSON
  transcript_truncated INTEGER NOT NULL DEFAULT 0,  -- 1 = 超过 4MB cap
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
  ON subagent_runs(parent_session_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
  ON subagent_runs(parent_request_id);
```

### db::subagent_runs module
- `pub async fn insert_run(db, params) -> Result<String>` —— worker 启动,status='running',finished_at=NULL,token_usage_json='{"input":0,"output":0,"cache_creation":0,"cache_read":0}',transcript_json='[]',transcript_truncated=0,返回 id
- `pub async fn update_run_finished(db, id, status, finished_at, summary, token_usage, transcript, truncated)` —— worker 跑完一次性 UPDATE
- `pub async fn get_run(db, id) -> Option<SubagentRunRow>` —— PR3 前端展开 + audit 查
- `pub async fn list_runs_by_session(db, parent_session_id) -> Vec<SubagentRunRow>` —— PR3 列出 session 的所有 worker
- `pub async fn add_token_usage_streaming(db, parent_session_id, usage)` —— worker 每 turn 调用,累加进父 session

### run_subagent 接入点(chat_loop.rs:1802)
1. Worker 启动:`insert_run(db, parent_session_id, worker_rid, subagent_name)` → 拿 `worker_run_id`
2. Worker run_chat_loop 每 turn:累加 `add_token_usage_streaming(parent_session_id, &turn_usage)`(streaming 累积,父 UI 实时看到)
3. Worker run_chat_loop 返回后:`worker_sink.transcript_snapshot()` → 4MB cap → `update_run_finished(worker_run_id, status, finished_at, summary, token_usage_sum, transcript, truncated)`
4. token_usage 汇总:worker final turn 的 usage 已经在 streaming 累积时累加进父,这里不再重复 add
5. audit 不污染父:worker 路径**不**调 `record_audit_event`,worker 的 ⑨ 决策靠 transcript PermissionAsk 回放

### 顺手修 RULE-A-014
- `run_chat_loop` 加第 21 参 `is_worker: Option<bool>`,`None` = 走 session row 默认 false
- 嵌套 worker 路径(`chat_loop.rs:1940`)传 `Some(true)`
- run_chat_loop 内部构造 PermissionContext 时读:`PermissionContext { ..., is_worker: is_worker.unwrap_or(false) }`(默认 false,worker 路径 true)
- 现有 26 调用点(production + 25 测试)加 `false`(对应 None 默认 → false,与之前 hardcode false 行为一致)
- 端到端测试:general-purpose + Edit mode + write_file → 不挂起,正常 deny

### streaming 累积的 token_usage 来源
- `run_chat_loop` turn 边界已有 `add_token_usage(db, &session_id, &turn_usage)` 调用(被 18 处 skip_persist gate 守护)
- worker 路径:把那个调用的 `&session_id` 改为 `&parent_session_id`(因为 worker 复用父 session_id,且 skip_persist 不触发 add_token_usage —— 但我们要 streaming 累加!)
- 解决方案:把 `add_token_usage` 调用**也**从 skip_persist gate 中拆出来(或者 worker 路径单独调一次 streaming 版)
- 简化:worker 路径里,sink 累积每 turn 收到的 `ChatEvent::Done { usage: Some(...) }` 的 usage,跑完时一次性 add_token_usage 进父 —— 但失去 streaming 效果
- 真正 streaming:需要 hook 进 worker 的 provider.send 返回的 usage 流(turn 边界 emit 时捕获),单独调 `add_token_usage_streaming(parent_session_id, &turn_usage)`
- 决定:worker 在 `run_chat_loop` 的 turn 边界(已有 skip_persist gate 外)调 `add_token_usage_streaming(parent_session_id, &turn_usage)` —— 需要修改 skip_persist 的边界(只 gate DB 写,不 gate token_usage 累加,因为后者是元数据,不污染 messages 表)

### transcript 4MB cap 实现
```rust
fn truncate_transcript_for_persistence(
    transcript: Vec<TranscriptEntry>,
    max_bytes: usize,  // 4MB
) -> (Vec<TranscriptEntry>, bool /* truncated */) {
    let json = serde_json::to_string(&transcript).unwrap_or_default();
    if json.len() <= max_bytes { return (transcript, false); }
    // 保留首 + 尾各半
    let half = max_bytes / 2;
    let (head, tail) = json.split_at(half.min(json.len()));
    let tail_start = json.len().saturating_sub(half);
    let tail_part = &json[tail_start..];
    // 重新 parse 两段拼起来,标记 truncated
    ...
}
```

## Implementation Plan (small chunks)

### PR2a — 后端持久化核心
- 新增 `db/migrations.rs` subagent_runs 表 CREATE + index(跟随 v6/v7 模式)
- 新增 `db/subagent_runs.rs` 模块:insert_run / update_run_finished / get_run / list_runs_by_session / add_token_usage_streaming
- 修改 `agent/chat_loop.rs:1802 run_subagent` 接入持久化(启动 INSERT running + 跑完 UPDATE + transcript 4MB cap + streaming 累加)
- `db/tests.rs` 加 5-6 测试:insert_run / update_run / CASCADE delete(删父 session 同步删 subagent_runs)/ transcript cap / token_usage streaming / list_runs_by_session

### PR2b — RULE-A-014 顺手修
- `agent/chat_loop.rs` `run_chat_loop` 加第 21 参 `is_worker: Option<bool>`
- `run_chat_loop` 函数体内构造 PermissionContext 时读:`is_worker: is_worker.unwrap_or(false)`
- 26+ 调用点更新:`chat.rs` 末尾加 `None`(对应默认 false)+ `agent/tests.rs` 25 测试调用点 + worker 嵌套调用传 `Some(true)` + `agent/permissions/mod.rs` 删除 `is_worker` 的 `#[allow(dead_code)]` 或确认无 warning
- 新增端到端测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
- `DEBT.md` `RULE-A-014` Status: open → **closed** + Closed At 填 PR2b commit hash
- `DEBT.md` §优先级分布 P2 22 → **21**(RULE-A-014 closed)