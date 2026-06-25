# DEBUG_DB — SQLite 直连调试指引

> 调试 / 调查 / 数据修复时**直连 SQLite** 的速查表。**默认路径走项目 IPC 命令**(Tauri command + 前端),这条指引只用于"需要看 raw state"或"批量修复"的场景。
>
> **维护承诺**:本文件给出路径 + schema 索引 + 5 个常用查询;具体表结构以 `app/src-tauri/src/db/migrations.rs` 为权威来源(改 schema 时同步本文件 §2 索引)。

---

## 1. DB 文件路径

DB 文件位置由 Tauri `app_data_dir()` 解析,各平台:

| 平台 | 路径 |
|------|------|
| **WSL / Linux** | `~/.local/share/dev.everlasting.app/everlasting.db` |
| macOS | `~/Library/Application Support/dev.everlasting.app/everlasting.db` |
| Windows | `%APPDATA%\dev.everlasting.app\everlasting.db` |

> 路径常量定义在 [`app/src-tauri/src/state.rs:212-214`](../../app/src-tauri/src/state.rs):`app_data_dir().join("everlasting.db")`。WAL 模式下还有 `everlasting.db-wal` / `-shm` 两个伴生文件。

### 1.1 速查

```bash
# WSL / Linux
ls -la ~/.local/share/dev.everlasting.app/

# 或者用 sqlite3 直接打开
sqlite3 ~/.local/share/dev.everlasting.app/everlasting.db
```

---

## 2. Schema 索引(9 张表)

权威定义在 [`app/src-tauri/src/db/migrations.rs`](../../app/src-tauri/src/db/migrations.rs);每张表的 CRUD 函数按表分文件组织在 `app/src-tauri/src/db/{table}.rs`。

| # | 表 | 文件 | 关键列 |
|---|----|------|--------|
| 1 | `projects` | `migrations.rs:59` | `id` (TEXT PK) / `name` / `path` / `git_branch` / `hidden` / `created_at` / `updated_at` |
| 2 | `sessions` | `migrations.rs:103` | `id` / `project_id` / `title` / `model_id` / `mode` (edit/plan/yolo) / `cwd` / `color` / token 累计 4 列 |
| 3 | `messages` | `migrations.rs:192` | `id` / `session_id` / `seq` / `role` (user/assistant) / `content` (JSON 序列化的 ContentBlock[]) / `is_error` / `parent_tool_use_id` |
| 4 | `providers` | `migrations.rs:235` | `id` / `kind` (anthropic/openai) / `base_url` / `has_key` (BOOL,因 RULE-D-001 api_key 加密) |
| 5 | `models` | `migrations.rs:250` | `id` / `provider_id` / `model_name` / `display_name` / `context_window` |
| 6 | `app_config` | `migrations.rs:276` | 单行 kv 表(默认 model_id / 默认 cwd 等) |
| 7 | `session_tool_permissions` | `migrations.rs:404` | `session_id` / `match_kind` (tool/prefix/path) / `match_value` / `decision` (allow/deny) / `expires_at` |
| 8 | `session_audit_events` | `migrations.rs:428` | `id` / `session_id` / `ts` / `kind` (AuditKind 字符串) / `payload_json` |
| 9 | `subagent_runs` | `migrations.rs` (06-23 添) | `id` / `session_id` / `parent_request_id` / `status` / `task` / `final_text` / `started_at` / `completed_at` |

**索引**:`idx_sessions_updated_at` / `idx_sessions_project_id` / `idx_messages_session_seq` / `idx_session_audit_events_session_ts` / `idx_subagent_runs_request` 等(`migrations.rs` 顶部)。

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
SELECT seq, role,
       substr(content, 1, 120) AS content_preview,  -- content 是 JSON,预览前 120 字符
       is_error
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
SELECT id, session_id, status, task, started_at
FROM subagent_runs
WHERE status NOT IN ('completed', 'failed', 'cancelled')
ORDER BY started_at DESC;
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
- **RULE-D-001(api_key 加密)**:不要 SELECT `providers` 表查 api_key — 已经不存明文(列从 `api_key` 改为 `api_key_enc` + `key_migrated_at` 哨兵,详见 [IMPLEMENTATION §4 2026-06-24](../IMPLEMENTATION.md#4-决策日志))
- **DB 文件泄露威胁模型**:见 `app/src-tauri/src/crypto.rs:5` 注释,无 machine-id 解不开 `api_key_enc`;但 session 标题 / message 历史仍是明文,**DB 文件跟 OS 账号权限走**
- **调试时停 app**:Tauri 进程持有 WAL writer,直连查询安全(`-readonly` 模式无写竞争),但**不要**在 app 运行时用写模式(`-cmd "UPDATE..."`)连接,会撞 `SQLITE_BUSY`

---

## 5. 故障排查入口

| 现象 | 查表 / 查列 | 备注 |
|------|------------|------|
| Session 列表不显示某项目 | `projects.hidden` | 1 = 隐藏 |
| Session 标题乱码 | `sessions.title` | 应是 UTF-8;若 ? 替换查前端 encoding |
| Token 计数对不上 | `sessions.{input,output,cache_creation,cache_read}_total` | 单条 LLM 响应的 token 在 `chat-event` 实时更新,DB 累计是 turn 边界 commit 的 |
| 权限决策错了 | `session_audit_events` 同 session_id + kind = 'tool_denied' / 'tool_allowed' | payload_json 里有 reason / critical / mode |
| Subagent 卡死 | `subagent_runs.status` NOT IN 终态 | 配合 `started_at` 算 wall-clock |
| FTS5 搜索不返回 | `messages_fts`(如已建) | FTS5 虚拟表是单独表,messages 主表 INSERT 时需同步;查 [IMPLEMENTATION §4 2026-06-17 "D2 降档"](../IMPLEMENTATION.md#4-决策日志) 状态 |

---

## 6. 相关文档

- [docs/ARCHITECTURE.md §1.2 数据流](../ARCHITECTURE.md) — session 切换 / message 持久化的架构意图
- [docs/IMPLEMENTATION.md §4 2026-06-17 D3 决策日志](../IMPLEMENTATION.md#4-决策日志) — session 内消息编辑/重发的 partial persist 逻辑
- [docs/HACKING-llm.md](../HACKING-llm.md) — token 计数的 LLM provider 差异(Anthropic SSE vs OpenAI Stream)
- `app/src-tauri/src/db/` — schema + CRUD 函数(权威)
- `.trellis/spec/backend/llm-contract.md` — DB column → wire shape 对应
