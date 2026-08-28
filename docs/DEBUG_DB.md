# DEBUG_DB — SQLite 直连调试指引

> 调试 / 调查 / 数据修复时**直连 SQLite** 的速查表。**默认路径走项目 IPC 命令**(Tauri command + 前端),这条指引只用于"需要看 raw state"或"批量修复"的场景。
>
> **维护承诺**:本文件给出路径 + schema 索引 + 5 个常用查询;具体表结构以 `app/src-tauri/src/db/migrations/schema.rs` 为权威来源(改 schema 时同步本文件 §2 索引)。

---

## 1. DB 文件路径

DB 文件位置由 Tauri `app_data_dir()` 解析,各平台:

| 平台 | 路径 |
|------|------|
| **WSL / Linux** | `~/.local/share/dev.everlasting.app/everlasting.db` |
| macOS | `~/Library/Application Support/dev.everlasting.app/everlasting.db` |
| Windows | `%APPDATA%\dev.everlasting.app\everlasting.db` |

> 路径常量定义在 [`app/src-tauri/src/state.rs:283`](../app/src-tauri/src/state.rs)(`db_path = app_data_dir.join("everlasting.db")`,在 `load_inner` 内)。WAL 模式下还有 `everlasting.db-wal` / `-shm` 两个伴生文件。

### 1.0 daemon 化后的三条解析路径(2026-07 同步)

`app_data_dir/` 这个子目录是 `dev.everlasting.app/`(= `tauri.conf.json` 的 `identifier`),三套进程各自按下面解析,**必须对齐到同一个文件**,否则会打开空 DB / 孤儿 DB:

| 进程 / 形态 | 怎么算出 data dir | 代码 |
|---|---|---|
| **GUI(Full 模式)** | Tauri `app.path().app_data_dir()` = `dirs::data_dir().join(identifier)` | `state.rs` `app_data_dir()` 读取 + `state.rs::db_path`(行号漂移风险,看 `state.rs` 实际位置)|
| **daemon 裸跑** | `resolve_data_dir()` = `dirs::data_dir().join(EVERLASTING_APP_IDENTIFIER)`,`EVERLASTING_APP_IDENTIFIER` 由 `build.rs` 从 `tauri.conf.json` 读出编译期注入 | `bin/everlasting-daemon.rs::resolve_data_dir`,`env!` 在 `bin/everlasting-daemon.rs`(行号漂移风险,看 daemon bin 实际位置)|
| **daemon sidecar(GUI spawn)** | GUI 把 Tauri-resolved `app_data_dir` 经 `--data-dir <PATH>` 显式传给 daemon,daemon 优先用 arg | `bin/everlasting-daemon.rs::parse_data_dir_from_args` |

> ⚠️ **行号漂移说明(2026-08-18 同步)**:08-14~18 多次改 schema / 加 turn_trace 三 token 列 / 加 messages_fts / 加 supports_images 字段,`state.rs` 与 daemon bin 行号已变。`app/src-tauri/src/state.rs` 与 `app/src-tauri/src/bin/everlasting-daemon.rs` **仍在 workspace 内的 `app/src-tauri/` member**(2026-08-11 workspace 翻后路径不变,根 `Cargo.toml` members = `app/src-tauri` + `crates/everlasting-remote` + `crates/everlasting-remote-protocol`)。

**孤儿 DB 坑(2026-07-23 commit `16548fd` 修复前)**:`resolve_data_dir()` 曾用 `dirs::data_dir().join("everlasting")`(无 `dev.` 前缀),与 Tauri 的 `app_data_dir()`(`.../dev.everlasting.app/`)对不上 → daemon 裸跑打开一个空 DB,看不到 GUI 写入的 151 条历史消息。修复方式:build.rs 注入 `EVERLASTING_APP_IDENTIFIER` 使两者走同一个 identifier。**如果升级/迁移后 GUI 看不到历史、daemon 看得到(或反之)**,先 `ls` 两个候选目录确认是不是又分叉了:
```bash
ls -la ~/.local/share/dev.everlasting.app/   # 应有 everlasting.db(GUI + 修后 daemon 都在这)
ls -la ~/.local/share/everlasting/           # 旧孤儿(修前 daemon 误建),有就是分叉,需手动 merge / 删
```

### 1.1 速查

```bash
# WSL / Linux
ls -la ~/.local/share/dev.everlasting.app/

# 或者用 sqlite3 直接打开
sqlite3 ~/.local/share/dev.everlasting.app/everlasting.db
```

---

## 2. Schema 索引(14 张表 — 13 张业务表 + 1 张 FTS5 虚拟表)

权威定义在 [`app/src-tauri/src/db/migrations/schema.rs`](../app/src-tauri/src/db/migrations/schema.rs)(`migrations.rs` 是转发 hub,`init_pool` 在 `pool.rs`、schema 序列在 `schema.rs`、一次性数据迁移在 `schema_helpers.rs`);每张表的 CRUD 函数按表分文件组织在 `app/src-tauri/src/db/{table}.rs`。

| # | 表 | 文件 | 关键列 |
|---|----|------|--------|
| 1 | `projects` | `db/migrations/schema.rs` | `id` (TEXT PK) / `name` / `path` / `git_branch` / `hidden` / `created_at` / `updated_at` |
| 2 | `sessions` | `db/migrations/schema.rs` | `id` / `project_id` / `title` / `model_id` / `mode` (edit/plan/yolo) / `session_type` (chat/group_chat,2026-07-29 群聊) / `cwd` / `color` / token 累计 4 列 |
| 3 | `messages` | `db/migrations/schema.rs` | `id` (INTEGER PK AUTOINCREMENT) / `session_id` / `seq` / `role` (user/assistant) / `content` (JSON 序列化的 ContentBlock[]) / `text` (纯文本冗余列,messages_fts 的内容源) / `has_tool_calls` / `has_tool_results` / `metadata` (JSON,可空) / `ttfb_ms` / `gen_ms` / `total_ms` / `thinking_ms` / `speaker` (群聊参与者标识,2026-07-29) / **`status`**(08-24 崩溃恢复:NULL=终态 / `in_progress`=流式检查点行(daemon 流式中周期 upsert,重启残留即崩溃现场)/ `interrupted`=启动恢复 pass 标记,内容尾部带 `[异常中断已恢复]` marker 块,见 `db/sessions/messages.rs::recover_interrupted_messages`)。错误状态不在顶层列,在 content JSON 的 tool_result 块 `is_error` 字段(见 §3.4) |
| 4 | `providers` | `db/migrations/schema.rs` | `id` / `kind` (anthropic/openai) / `base_url` / `has_key` (BOOL,因 RULE-D-001 api_key 加密) |
| 5 | `models` | `db/migrations/schema.rs` | `id` / `provider_id` / `model_name` / `display_name` / `context_window` / **`thinking_effort`**(07-10 multi-model PR3,缺省 `high`,Anthropic → `thinking.adaptive.effort` / OpenAI → 顶层 `reasoning_effort`)/ **`supports_images`**(B1 08-16, INTEGER NOT NULL DEFAULT 0,见 `db/migrations/schema.rs:1012` `add_models_column_if_missing`) |
| 6 | `app_config` | `db/migrations/schema.rs` | 单行 kv 表(默认 model_id / 默认 cwd 等);remote tunnel 4 个 key:`remote_url` / `shared_secret` / `tunnel_node_id` / `tunnel_display_name`(存 daemon DB,清配置即停 tunnel,见 `daemon/tunnel/config.rs`) |
| 7 | `session_tool_permissions` | `db/migrations/schema.rs` | `session_id` / `match_kind` (tool/prefix/path) / `match_value` / `decision` (allow/deny) / `expires_at` |
| 8 | `session_audit_events` | `db/migrations/schema.rs` | `id` / `session_id` / `ts` / `kind` (AuditKind 字符串) / `payload_json` / **`turn_seq`**(E2 07-14,nullable,21 类调用点落表时携带对应 turn 序号,见 `db/migrations/schema.rs:936-941`) |
| 9 | `subagent_runs` | `db/migrations/schema_helpers.rs`(约 L153) | `id` / `parent_session_id` / `parent_request_id` / `subagent_name` / `status` (running/completed/cancelled/error/incomplete) / `started_at` / `finished_at` (NULL while running) / `task` / `final_text` / `summary` / `turn_count` / `token_usage_json` / `transcript_json` / `transcript_truncated` / `worktree_path` / `isolation` (L3b PR1+) |
| 10 | `autonomous_memories` | `db/migrations/schema.rs` | `id` / `memory_id` (TEXT UNIQUE) / `scope` / `project_id` / `kind` / `status` (candidate/active/verified) / `title` / `content` / `tags` (JSON) / `tool_name` / `command_pattern` / `path_globs` (JSON) / `source_session_id` / `source_ref` / `confidence` / `hit_count` / `last_used_at` / `demoted_reason`(V2 2 期,2026-06-29 落地,状态机候选/激活/已验证) |
| 11 | `subagent_model_overrides` | `db/migrations/schema.rs` | `agent_name` (TEXT PK) / `model_id` / `updated_at`(B6+ C,2026-07-03,builtin agent 无 frontmatter 文件可改 → 全局 DB override,优先级 `DB > frontmatter > parent`) |
| 12 | `turn_trace` | `db/migrations/schema.rs` | `id` (INTEGER PK) / `session_id` (FK CASCADE) / **`run_id`**(08-20 表重建并入唯一键;**`''` = 主 loop 行哨兵**,worker 行 = `subagent_runs.id`;不用 NULL 因 SQLite UNIQUE 视 NULL 互异)/ `seq` / `token_usage_json` / `compaction_json` / `loop_hint_json` / `breadcrumb_json` / `created_at`(E2,2026-07-14,turn-level harness trace,**UNIQUE(session_id, run_id, seq)**)/ **`tools_token INTEGER`**(C7 08-14,实测 session 起步 tools_token -38.5%)/ **`memory_token INTEGER`**(memory-gov 08-15,实测指令块 -79.5%)/ **`images_token INTEGER`**(B1 PR4 08-16,图片 attachment 计量)/ **`at_files_token INTEGER`**(unified-context-budget WP1 08-19,@文件注入体 cl100k 估算)/ **`system_token INTEGER`**(08-19,system prompt 体 + skill-listing 合成消息)/ **`context_window INTEGER`**(08-19,请求时模型 context_window 快照,前端预算行分母) |
| 13 | `scheduled_tasks` | `db/migrations/schema.rs:1279` | F2/F2b 定时任务(2026-08-28):`id` (TEXT PK) / `project_id` + `target_session_id` (双 FK CASCADE) / `name` / `prompt` / `schedule` (JSON:preset 类型 + 参数,6 档) / `enabled` (INTEGER, kill switch 之外的 per-task 开关) / `created_by` (user 默认) / `created_at` / `last_fired_at` / `next_fire_at` / **`run_count`** / **`max_runs`** / **`ends_at`**(F2b 08-28 结束条件两列 + 计数;`run_count` 只计真正送入 chat_inner 的 fire,dedup 跳过不计数)。CRUD 在 `db/scheduled_tasks.rs`;调度内核在 `scheduler/` 模块 |
| 14 | `messages_fts` | `db/migrations/schema.rs:1142` | FTS5 虚拟表(external-content + trigram tokenizer + `UPDATE OF text` 触发器防写放大 + `messages_fts_docsize` 影子表守卫回填,见 `db/migrations/schema.rs:1153-1174` INSERT/DELETE/UPDATE 触发器) — D2 跨 session 全文搜索的存储后端(2026-08-17);另有 `autonomous_memories_fts`(schema.rs:886,自主记忆自身搜索,未在此编号) |

**索引**:`idx_sessions_updated_at` / `idx_sessions_project_id` / `idx_messages_session_seq` / `idx_session_audit_events_session_ts` / `idx_subagent_runs_request` / `idx_am_pitfall`(autonomous_memories 的 `tool_name` 等 trigger 命中)/ `idx_autonomous_memories_status` / **`idx_session_audit_events_turn_seq`**(E2 07-14)/ **`idx_turn_trace_run`**(08-20,partial index `ON turn_trace(run_id) WHERE run_id != ''`,专服 `list_worker_turn_traces` 按 run_id 点查)/ **`idx_messages_status`**(08-24,partial index `ON messages(status) WHERE status IS NOT NULL`,启动恢复扫描专用)/ **`idx_scheduled_tasks_due`**(08-28,`ON scheduled_tasks(enabled, next_fire_at)`,调度器每 tick 扫描未到期任务)/ **FTS5 影子表 `messages_fts_docsize`** 索引(`db/migrations/schema.rs` 顶部)。

> **崩溃恢复排查**(08-24):`sqlite3 -readonly` 查 `SELECT session_id, seq, status, substr(text,1,80) FROM messages WHERE status IS NOT NULL;` —— 非空结果只应在 daemon 流式期间看到 `in_progress`;**daemon 已重启后仍有 `in_progress`** = 恢复 pass 没跑或失败(看 daemon 日志 `startup: recovered interrupted messages` 行)。

### 2.1 remote 侧 DB(云端 everlasting-remote,2026-08 remote epic)

云端 `everlasting-remote` 服务端有**完全独立**的 SQLite,与 daemon DB 无任何共享:

| 项 | 值 |
|---|---|
| 默认路径 | `$HOME/.local/share/dev.everlasting.remote/remote.db`(`crates/everlasting-remote/src/config.rs` 的 `DEFAULT_DB_SUBDIR`;固定 `dev.everlasting.remote` 子目录,与 daemon identifier 解耦) |
| 覆盖方式 | CLI `--db-path` > env `EVERLASTING_REMOTE_DB_PATH` > 默认(`resolve_db_path`,`config.rs`) |
| 表 | `nodes` / `devices` / `pairing_codes` + 索引 `idx_devices_node`(`db/schema.rs`) |
| 内容边界 | 只存节点 / 设备 / 配对码,**不持文件、不存 agent 数据**(`db/mod.rs` 不变量注释) |

- 配对码 60s 一次性;生成撞码时 retry ≤3 次(`db/crud.rs`);删除 device 后需重新配对
- daemon 侧 app_config 的 tunnel key(`remote_url` 等)与此 DB 无关,见 §2 表第 6 行

---

## 3. sqlite3 速查(只读)

### 3.1 推荐连接(只读模式防误改)

```bash
sqlite3 -readonly -header -column ~/.local/share/dev.everlasting.app/everlasting.db
```

或者用 URI 模式开"备份 + 只读":
```bash
sqlite3 "file:~/.local/share/dev.everlasting.app/everlasting.db?mode=ro" <<< ".tables"
```

### 3.2 输出格式优化(交互式)

```sql
.mode box          -- 表格框
.headers on        -- 显示列名
.timer on          -- 显示查询耗时
.nullvalue NULL    -- NULL 显示成 NULL(默认空字符串)
```

### 3.3 5 个常用查询

```sql
-- 1. 看最近 20 个 session
SELECT id, project_id, title, mode, updated_at
FROM sessions
ORDER BY updated_at DESC
LIMIT 20;

-- 2. 看某个 session 的全部消息(按时间序)
--    注意:messages 顶层没有 is_error 列;错误状态在 content JSON 的
--    tool_result 块里(§3.4 有提取示例)。这里用 text 纯文本列预览。
SELECT seq, role,
       substr(text, 1, 120) AS text_preview,  -- text 是纯文本冗余列
       has_tool_calls, has_tool_results, status
FROM messages
WHERE session_id = 'YOUR_SESSION_ID'
ORDER BY seq ASC;

-- 3. 看某个 session 的权限决策
SELECT ts, kind, payload_json
FROM session_audit_events
WHERE session_id = 'YOUR_SESSION_ID'
ORDER BY ts DESC
LIMIT 50;

-- 4. 看 token 用量 top 10 session
SELECT id, title,
       input_tokens_total, output_tokens_total,
       cache_creation_total, cache_read_total
FROM sessions
ORDER BY (input_tokens_total + output_tokens_total) DESC
LIMIT 10;

-- 5. 看活跃的 subagent run(未完成)
--    注意:关联列是 parent_session_id(不是 session_id),
--    终态是 completed/cancelled/error/incomplete(没有 'failed')。
SELECT id, parent_session_id, subagent_name, status, task, started_at
FROM subagent_runs
WHERE status NOT IN ('completed', 'error', 'cancelled', 'incomplete')
ORDER BY started_at DESC;

-- 6. 看 candidate / active 的 autonomous memory(已验证不进工具前召回池)
--    工具前召回走 idx_am_pitfall(tool_name equality)+ command_pattern + path_globs
SELECT memory_id, kind, status, title, tool_name, command_pattern,
       hit_count, confidence
FROM autonomous_memories
WHERE status IN ('candidate', 'active')
ORDER BY hit_count DESC, last_used_at DESC;

-- 7. D2 (2026-08-17) — 跨 session 全文搜索(走 messages_fts FTS5)
--    FTS5 命中走 rowid 回查 messages 主表拿完整 ContentBlock;
--    0 命中走 LIKE 兜底(见 db/search.rs 双路分派)
SELECT m.session_id, m.seq, m.role,
       substr(m.content, 1, 200) AS preview
FROM messages_fts mf
JOIN messages m ON m.rowid = mf.rowid
WHERE messages_fts MATCH ?1                    -- ?1 = 用户输入的 FTS5 query
  AND m.session_id IN (SELECT id FROM sessions WHERE project_id = ?2)
ORDER BY rank                                   -- FTS5 BM25 rank
LIMIT 50;

-- 8. C7+memory-gov+B1+budget (2026-08-14~20) — 看某 session 的 turn-level token 切片
--    tools_token(08-14 C7)+ memory_token(08-15 memory-gov)+ images_token(08-16 B1)
--    + at_files_token/system_token(08-19 unified-context-budget)+ context_window
--    run_id = '' 只出主 loop 行;worker 行按 subagent_runs.id 单独点查(见下)
SELECT seq, run_id,
       json_extract(token_usage_json, '$.input_tokens') AS input_tokens,
       tools_token, memory_token, images_token, at_files_token, system_token, context_window
FROM turn_trace
WHERE session_id = 'YOUR_SESSION_ID'
  AND run_id = ''
ORDER BY seq DESC
LIMIT 20;

-- 8b. worker per-turn 度量 (2026-08-20 worker-turn-trace-persist) —
--     某 run 的逐轮 trace 行(list_worker_turn_traces IPC 的 SQL 等价)
SELECT seq, tools_token, memory_token, at_files_token, system_token
FROM turn_trace
WHERE run_id = 'YOUR_SUBAGENT_RUN_ID'   -- subagent_runs.id
ORDER BY seq;

-- 9. F2/F2b (2026-08-28) — 定时任务状态(调度器每 tick 扫 enabled+next_fire_at)
--    run_count 只计真正送入 chat_inner 的 fire;dedup/queue-disabled 跳过不计数。
--    max_runs / ends_at 命中后任务自动停用(enabled=0)并落 completed 审计。
SELECT id, name, enabled, next_fire_at, last_fired_at,
       run_count, max_runs, ends_at
FROM scheduled_tasks
ORDER BY enabled DESC, next_fire_at ASC;

-- 9b. 定时任务触发审计(六动作: fired / catchup / skipped_dedup /
--      skipped_queue_disabled / lost / error)
SELECT ts, kind, payload_json
FROM session_audit_events
WHERE kind = 'ScheduledTaskFired'
ORDER BY ts DESC
LIMIT 20;
```

### 3.4 消息内容(content 是 JSON)解析

`messages.content` 列存的是 `Vec<ContentBlock>` 的 JSON 序列化。常用解析:

```sql
-- 提取 user/assistant 的 text 内容
SELECT seq, role,
       json_extract(content, '$[0].text') AS text
FROM messages
WHERE session_id = 'YOUR_SESSION_ID'
  AND role = 'assistant'
  AND json_extract(content, '$[0].type') = 'text';

-- 看 tool_use 块
SELECT seq, json_extract(content, '$[0].name') AS tool_name,
       json_extract(content, '$[0].input') AS tool_input
FROM messages
WHERE session_id = 'YOUR_SESSION_ID'
  AND json_extract(content, '$[0].type') = 'tool_use';

-- 看 tool_result 块(is_error + content 前 200 字符)
SELECT seq, json_extract(content, '$[0].is_error') AS is_error,
       substr(json_extract(content, '$[0].content'), 1, 200) AS preview
FROM messages
WHERE session_id = 'YOUR_SESSION_ID'
  AND json_extract(content, '$[0].type') = 'tool_result';
```

---

## 4. 安全提醒

- **默认走项目 IPC,不要直连修改**:CRUD 逻辑在 `app/src-tauri/src/db/{table}.rs`,经过 type-safe 包装 + business rules;直连 UPDATE 可能绕过"tool_use/tool_result 配对保护"等不变量,导致 agent loop 状态错乱
- **直连只读时也别用生产 DB**:复制到 `/tmp/everlasting-debug.db` 再操作(`sqlite3 ~/.local/.../everlasting.db ".backup /tmp/everlasting-debug.db"`)
- **RULE-D-001(api_key 加密)**:不要 SELECT `providers` 表查 api_key — 已经不存明文(列从 `api_key` 改为 `api_key_enc` + `key_migrated_at` 哨兵,详见 [IMPLEMENTATION §4 2026-06-24](./IMPLEMENTATION/decisions-2026-06.md))
- **DB 文件泄露威胁模型**:见 `app/src-tauri/src/crypto.rs:5` 注释,无 machine-id 解不开 `api_key_enc`;但 session 标题 / message 历史仍是明文,**DB 文件跟 OS 账号权限走**
- **调试时停 daemon(daemon 化后 2026-07 同步)**:**持有 WAL writer 的是 daemon 进程**,不是 GUI。Thin 模式(默认)下 GUI 根本不开 `SqlitePool`(`sidecar.rs` 注释:Thin 模式 GUI does NOT load `AppState` / does NOT open a `SqlitePool`)。直连查询安全(`-readonly` 无写竞争),但**不要**在 daemon 运行时用写模式(`-cmd "UPDATE..."`)连接,会撞 `SQLITE_BUSY`。要安全地直连写:先 `./scripts/daemon.sh stop` 停 daemon(Thin 模式下 GUI 还开着也不影响,GUI 没开 pool)。Full 模式(`?transport=tauri`)例外 —— 那是 GUI 进程持有 writer,要停 GUI

---

## 5. 故障排查入口

| 现象 | 查表 / 查列 | 备注 |
|------|------------|------|
| Session 列表不显示某项目 | `projects.hidden` | 1 = 隐藏 |
| Session 标题乱码 | `sessions.title` | 应是 UTF-8;若 ? 替换查前端 encoding |
| Token 计数对不上 | `sessions.{input,output,cache_creation,cache_read}_total` | 单条 LLM 响应的 token 在 `chat-event` 实时更新,DB 累计是 turn 边界 commit 的 |
| 权限决策错了 | `session_audit_events` 同 session_id + kind = 'tool_denied' / 'tool_allowed' | payload_json 里有 reason / critical / mode |
| Subagent 卡死 | `subagent_runs.status` NOT IN 终态 | 配合 `started_at` 算 wall-clock。**daemon 启动时** `reap_orphaned_runs`(daemon 化后 2026-07:reap 发生在 daemon 的 `load_inner` 即 `state.rs:297`,Thin 模式 GUI 不调 `load_inner`,所以是 daemon 进程在 reap)会把残留 `running`(上一进程崩溃 / 被杀留下的孤儿)标记为 `error`,所以重启后看到的假 running 已被清理 |
| FTS5 搜索不返回 | `messages_fts`(`messages_fts_docsize` 影子表守卫) | D2 主搜索走 `messages_fts` virtual table(2026-08-17,external-content + trigram),`autonomous_memories_fts` 是另一张 FTS 表(自主记忆自身搜索);`messages_fts` INSERT/UPDATE/DELETE 触发器见 `db/migrations/schema.rs:1062-1085`,`UPDATE OF text` 防写放大 |
| Memory 召回不命中 | `autonomous_memories.status NOT IN ('verified', 'active')` | status='candidate' 不进 recall;查 `tool_name` / `command_pattern` 是否精确匹配,`hit_count` 是否 < 阈值(quality 层 P5 软拦截) |
| 定时任务没触发 | `scheduled_tasks.enabled` + `next_fire_at` + kill switch | F2/F2b(08-28):全局开关 `scheduled_tasks_enabled`(app_config,fail-open);per-task `enabled`=0 是停用;`next_fire_at` 是理论到期点,过了才 fire;同 session 每 tick 至多一 fire(重看 `idx_scheduled_tasks_due`)。触发动作落 `ScheduledTaskFired` 审计(见 §3.3 查询 9b) |

---

## 6. 相关文档

- [docs/ARCHITECTURE.md §1.2 数据流](./ARCHITECTURE.md) — session 切换 / message 持久化的架构意图
- [docs/REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) — remote 云端部署手册(systemd + nginx + 配对);§2.1 remote 侧 DB 的部署上下文
- [docs/REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md) — remote E2E 验收手册(配对 / PWA 手机访问)
- [docs/IMPLEMENTATION/decisions-2026-06.md 2026-06-17 D3 决策日志](./IMPLEMENTATION/decisions-2026-06.md) — session 内消息编辑/重发的 partial persist 逻辑
- [docs/HACKING-llm.md](./HACKING-llm.md) — token 计数的 LLM provider 差异(Anthropic SSE vs OpenAI Stream)
- `app/src-tauri/src/db/` — schema + CRUD 函数(权威)
- `.trellis/spec/backend/llm-contract.md` — DB column → wire shape 对应
