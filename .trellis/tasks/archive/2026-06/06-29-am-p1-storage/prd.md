# P1: 存储底座 — autonomous_memories 表 + FTS5 + CRUD

> Trellis child `06-29-am-p1-storage` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §5](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> **无前置依赖**,P2-P5 的地基。纯数据层,无 LLM / UI / agent loop 改动。
> **版本**:v3(2026-06-29)。v2 吸收 spike-review 7 条;v3 吸收 deepseek-review 10 条(见文末 §review-log)。

## Goal

落地自主记忆系统的数据层:SQLite `autonomous_memories` 表 + FTS5 全文索引 + 触发器 + CRUD + Rust 结构体 + 写入安全网 + 单元测试。这是整个系统的地基,P2(读写闭环)/P3(工具前召回)/P4(事件写入)/P5(质量层)都建在它之上。

## What I already know(代码现状)

- DB 模块 `app/src-tauri/src/db/`:`migrations.rs:55 run_migrations` 幂等建表(CREATE TABLE IF NOT EXISTS);CRUD 子模块范式见 `subagent_runs.rs` / `sessions.rs`
- 建表模板:`migrations.rs:56-97` projects 表;加列用 `add_*_column_if_missing` 幂等模式
- test 模式:`db/tests_*.rs` 6 域文件,各复制 `test_pool`(无 common),PKG_CONFIG_PATH 环境下跑 `cargo test --lib`
- `memory/loader.rs` 是 mtime fence 读时检查(**非** notify watcher)→ 自主记忆走 DB 不走文件,省 watcher 且能关联 `source_session_id` / `source_ref`
- sqlx + SQLite;**FTS5 是否启用 + tokenizer 中文支持 待验证**(Open Q #1,本 task 第一个动作)
- FK 惯例不统一:`messages.session_id` 有 FK+CASCADE,`subagent_runs` 无 FK → 本表立场见 §1 注释(H1)

## Requirements

### 1. 表结构(完整 DDL,加到 `migrations.rs run_migrations`)

> **schema 演变**(M1):本 schema 在 spike-007 §5 基础上,按 external review 将 `trigger_key`(JSON TEXT)拆为 `tool_name`/`command_pattern`/`path_globs` 三个类型列——可索引、规避 `json_extract` 的 SQLite ≥3.38 依赖与 LIKE 顺序敏感。
> **FK 决策**(H1):`project_id` **不设 REFERENCES + CASCADE**——记忆是持久经验,project 删除后 memories 保留(项目可能恢复),孤儿数据由 P5 卫生 job / 独立 sweep 清理。

```sql
CREATE TABLE IF NOT EXISTS autonomous_memories (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,  -- 内部自增,FTS5 content_rowid 需要
    memory_id           TEXT    NOT NULL UNIQUE,            -- UUID v7(时间有序,B-tree 友好),对外引用
    scope               TEXT    NOT NULL,                   -- user | project (global 留 v2)
    project_id          TEXT,                               -- scope=project 必填(无 FK CASCADE,见上);scope=user 时 NULL
    kind                TEXT    NOT NULL,                   -- pitfall | preference | fact | decision
    status              TEXT    NOT NULL,                   -- candidate | active | verified | demoted
    title               TEXT    NOT NULL,                   -- 写入强制,≤200 字符
    content             TEXT    NOT NULL,                   -- 正文(经验式措辞,非规则),≤500 字符
    tags                TEXT    NOT NULL DEFAULT '[]',      -- JSON array
    -- pitfall 触发键拆 3 列(非 JSON):工具执行前高频召回走索引精确匹配
    tool_name           TEXT,                               -- pitfall:触发工具(shell/edit_file/grep...),精确匹配走 idx_am_pitfall
    command_pattern     TEXT,                               -- pitfall:命令模式(子串/glob,P3 定匹配规则)
    path_globs          TEXT,                               -- pitfall:路径 glob JSON array;NULL=不限路径(任何路径触发),非 NULL=限定(M2)
    source_session_id   TEXT,                               -- 产生此记忆的 session(P2/P4 写入提供;P4 频率控制用)(B3)
    source_ref          TEXT,                               -- 溯源:turn_id/tool_call_id(精确执行点,与 source_session_id 互补)
    -- 以下为 forward-compat 字段,P1 只存/提供接口,不主动消费(P5 状态机/卫生 job 用)(H5)
    confidence          REAL    NOT NULL DEFAULT 0.5,       -- P5 状态机晋升消费
    hit_count           INTEGER NOT NULL DEFAULT 0,         -- P5 消费(P1 只提供 bump 接口)
    last_used_at        TEXT,                               -- P5 衰减用
    created_at          TEXT    NOT NULL,
    updated_at          TEXT    NOT NULL,
    demoted_reason      TEXT,                               -- P5 demoted 用
    -- 枚举 + 长度合法性:DB CHECK + Rust enum 双保险(SQLite 不支持后加 CHECK,必须建表定档)(B1/2.2)
    CHECK(scope  IN ('user','project')),
    CHECK(kind   IN ('pitfall','preference','fact','decision')),
    CHECK(status IN ('candidate','active','verified','demoted')),
    CHECK(length(title)   <= 200),
    CHECK(length(content) <= 500)
);
-- session 开始召回(FTS5)+ scope/project 过滤
CREATE INDEX IF NOT EXISTS idx_am_recall  ON autonomous_memories(scope, project_id, status, kind);
-- pitfall 工具执行前召回:tool_name 精确匹配 + status 过滤
CREATE INDEX IF NOT EXISTS idx_am_pitfall ON autonomous_memories(tool_name, status) WHERE tool_name IS NOT NULL;
-- 注:source_session_id 索引按 P2/P4 频率控制需要再加(低频写路径,P1 暂不加)
```

### 2. FTS5 虚拟表 + 同步触发器(sketch,B2)

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS autonomous_memories_fts USING fts5(
    title, content, tags,
    content='autonomous_memories', content_rowid='id'
    -- tokenizer 待 Open Q #1 验证:默认 unicode61 对中文不友好,可能需 trigram/simple
);

-- external content table 标准同步模式(DELETE/UPDATE 用 FTS5 约定的 'delete' 特殊行)
CREATE TRIGGER IF NOT EXISTS am_fts_insert AFTER INSERT ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(rowid, title, content, tags)
    VALUES (new.id, new.title, new.content, new.tags);
END;
CREATE TRIGGER IF NOT EXISTS am_fts_delete AFTER DELETE ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(autonomous_memories_fts, rowid, title, content, tags)
    VALUES ('delete', old.id, old.title, old.content, old.tags);
END;
CREATE TRIGGER IF NOT EXISTS am_fts_update AFTER UPDATE ON autonomous_memories BEGIN
    INSERT INTO autonomous_memories_fts(autonomous_memories_fts, rowid, title, content, tags)
    VALUES ('delete', old.id, old.title, old.content, old.tags);
    INSERT INTO autonomous_memories_fts(rowid, title, content, tags)
    VALUES (new.id, new.title, new.content, new.tags);
END;
```

> preference/fact 的 session 开始召回走 FTS5;pitfall 工具执行前召回走 `tool_name` 精确匹配(不走 FTS5)。两套索引各管一类。

### 3. CRUD 接口(新建 `db/memories.rs` 子模块,与 `subagent_runs.rs` 同级)

- `insert_memory(pool, input) -> Result<MemoryRow>` — 含写入安全网(§4)+ FTS 自动同步(触发器);**memory_id UNIQUE 冲突 → Err**(UUIDv7 碰撞概率极低,不 upsert)
- `search_memories_fts(pool, project_id, scope: Option<Scope>, query, limit) -> Vec<MemoryRow>` — FTS5 MATCH + bm25,query 先过 `escape_fts5`;**scope/project_id 交互语义**(H2):
  - `scope=Some(User)` → `WHERE scope='user'`(忽略 project_id)
  - `scope=Some(Project)` + `project_id=NULL` → **Err**(project 查询必须有 id)
  - `scope=None`(session 开始召回搜两层) → `WHERE scope='user' OR (scope='project' AND project_id=?)`
- `find_pitfalls_by_trigger(pool, tool_name, command_pattern?, path?) -> Vec<MemoryRow>` — `tool_name` 精确匹配(走 `idx_am_pitfall`)+ command_pattern 二级匹配(P3 定规则);`path_globs` 为 NULL 的 pitfall 视为不限路径(M2)
- `bump_hit_count(pool, memory_id)` — 召回命中时调用,P5 消费
- `update_status(pool, memory_id, new_status, reason?) -> Result<()>` — **事务包裹**(读旧 status → 校验迁移合法性 → 写新 status + demoted_reason),P5 消费
- `list_memories(pool, scope?, project_id?) -> Vec<MemoryRow>` — UI 列表用(P2)
- `delete_memory(pool, memory_id)` — UI 删除用(P2);FTS 触发器自动清

### 4. 写入安全网(在 `insert_memory` 入库前,吸收 spike-005 §4 + reviews)

- **敏感信息过滤**:匹配 `(?i)(api[_-]?key|secret|password|token=|bearer)` → **拒绝** + warn
- **敏感路径 deny-list**(2.3):命中 `.ssh`/`.aws`/`.gnupg`/`credentials`/`id_rsa` → **拒绝**;`/tmp`/`/var/log` 临时路径 → 拒绝;其余 `/home/<user>/...` → 泛化 `~`
- **长度上限**(B1):title > 200 / content > 500 → **拒绝**(DB CHECK 兜底 + 安全网前置)
- **空值**(2.2):empty title / empty content → 拒绝 Err
- **频率控制**:同 session remember 计数(按 `source_session_id`,B3 提供);本 task 先留接口位 + 注释,P2 接 remember tool 时补
- **FTS5 query 转义**(1.2 + H3 tradeoff):

```rust
// 防 FTS5 运算符注入(" / NEAR / AND / OR / NOT / * / ^)。
// tradeoff(H3):整个 query 包双引号 → FTS5 phrase match,要求 title/content/tags 出现
// 连续子串,词序不对搜不到(如 "WSL cargo" 搜不到 "cargo 在 WSL")。MVP 接受(精度优先);
// V2 可改为 tokenize 后各 token 单独转义 + OR 连接(召回优先,需配合中文 tokenizer)。
fn escape_fts5(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}
```

- `source_session_id` / `source_ref` 由调用方提供(P2/P4 传入)

### 5. Rust 结构体(`db/types.rs` 或 `memories.rs` 内)

- `MemoryKind` / `MemoryScope` / `MemoryStatus` enum(serde + sqlx,与 DB CHECK 双保险)
- `MemoryRow` struct(对应表列,sqlx::FromRow;pitfall 三字段 + path_globs 都是 Option)
- `MemoryInput`(insert 入参:title/content/tags/kind/scope/project_id/tool_name/command_pattern/path_globs/source_session_id/source_ref)
- tags / path_globs:`TEXT` + serde_json;Rust 侧 `Option<Vec<String>>`,**None = 不限路径/无标签约束**(M2)

## Acceptance Criteria

- [ ] FTS5 feature + tokenizer 中文支持验证通过(Open Q #1);不可用按退路落 LIKE + 记录决策
- [ ] migration 幂等(重复 run_migrations 不报错、不丢数据)
- [ ] `insert_memory` CRUD roundtrip 单测通过
- [ ] FTS5 搜索:插入 N 条 → `search_memories_fts` 中文/英文关键词 bm25 排序正确
- [ ] **FTS5 query 转义**(1.2):含 `"WSL cargo" test`/`NEAR`/`*` 等特殊字符的 query 经 `escape_fts5` 不报错、不误解析(单测)
- [ ] **scope/project_id 交互**(H2):User 忽略 project_id / Project+NULL 报 Err / None 搜两层 三种语义单测覆盖
- [ ] **project 隔离**:`scope=project` 的记忆不被别的 project_id 搜到(单测)
- [ ] **索引验证**(1.3):`EXPLAIN QUERY PLAN` 确认 `scope='user' AND project_id IS NULL` 与 project 级查询都走 `idx_am_recall`(记录)
- [ ] `find_pitfalls_by_trigger`:`tool_name` 精确命中、不误命中;`path_globs=NULL` 视为不限路径(单测)
- [ ] **边界行为**(2.2/B1):empty title/content → 拒绝;title>200/content>500 → DB CHECK reject;memory_id UNIQUE 冲突 → Err;kind/status 非法值 → CHECK reject(单测)
- [ ] 写入安全网:敏感内容/敏感路径(.ssh/.aws)/超长 全被拒;路径泛化生效(单测)
- [ ] `bump_hit_count` / `update_status`(事务)状态机字段更新正确(单测)
- [ ] **FTS 触发器**(B2):INSERT/UPDATE/DELETE 后 FTS 同步(单测:删主表行后搜不到;UPDATE title 后能搜新词)
- [ ] `cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib` 0 失败 0 warning;`cargo check` 0 warning

## Implementation Plan (small PRs,H4 拆分)

- **PR1a** — migration 基础:`autonomous_memories` 表(含 CHECK length/enum)+ 2 索引;`run_migrations` 幂等;**SQLite PRAGMA 现状检查**(Open Q #4)。零风险(纯表结构,无 FTS5 依赖)
- **PR1b** — FTS5:虚拟表 + 3 触发器(sketch 见 §2)+ **FTS5 feature/tokenizer 验证**(Open Q #1)。退路(LIKE)只影响本 PR
- **PR2** — `db/memories.rs`:enum + `MemoryRow`/`MemoryInput` 结构体 + `insert_memory`(含安全网:敏感过滤/路径 deny-list/长度/空值)+ `list_memories` + `delete_memory` + 单测(含边界)
- **PR3** — `search_memories_fts`(bm25 + `escape_fts5` + scope 语义)+ `find_pitfalls_by_trigger`(tool_name 精确匹配)+ `bump_hit_count` + `update_status`(事务)+ 单测(project 隔离 / trigger 精确匹配 / FTS 转义 / scope 交互 / FTS 同步 / EXPLAIN)
- **PR4** — `db/mod.rs` 导出 + 收尾(`cargo test --lib` + `cargo check` 全绿)

## Open Questions

1. **【blocking,第一个动作】FTS5 是否启用 + tokenizer 中文支持?** ✅ **CLOSED(2026-06-29)**
   - 验证:写最小 `CREATE VIRTUAL TABLE ... USING fts5` migration 试跑;查 sqlx feature flags;**插入中文内容测搜索效果**(默认 unicode61 对中文按字符边界,可能需 `tokenize='trigram'` 或 `simple`)
   - 启用 + 中文 OK → 按上方方案;启用但中文差 → 换 tokenizer(trigram 对中文/子串友好);未启用退路 A:sqlx 开 fts5 feature;退路 B(都不行):降级 `content LIKE '%kw%'`(tool_name 精确匹配不受影响)。决策记录进 prd
   - **验证结论**:
     - **FTS5 启用**:系统 SQLite 3.53.0 编译时启用 FTS5(sqlx 非 bundled,链系统 libsqlite3;实测 `CREATE VIRTUAL TABLE ... USING fts5` 在 `cargo test --lib memories::fts5_trigram_tokenizer_is_available_for_cjk` 通过)。
     - **默认 `unicode61` 对 CJK 失效**:实测插入中文行后,`MATCH 'cargo'`(CJK 文本里夹的 ASCII 词)**和** `MATCH '权限'`(纯中文)**都搜不到**——unicode61 在 CJK 字符间找不到词边界,把整段当一 token。
     - **`trigram` tokenizer 是正确选择**:`tokenize='trigram'` 后 ASCII 子串(`cargo`/`WSL`)和 ≥3 字符中文(`权限管理`/`注意权限`)都 MATCH。tradeoff:**trigram 要求 query ≥3 字符**(2 字符中文如 `权限` 不 MATCH);v1 接受(精度优先,title 字段仍提供主要搜索信号)。
     - **最终路径**:**FTS5 + `tokenize='trigram'`**(不走 LIKE 退路)。已落地到 `migrations.rs` 的 `autonomous_memories_fts` 虚拟表 + 3 个同步触发器(insert/delete/update)。
2. **JSON 字段映射**:tags / path_globs 用 `TEXT` + serde_json(trigger_key 已拆列)。实施时确认现有项目 JSON 列惯例。✅ **CLOSED**:tags / path_globs 用 `TEXT` + serde_json;Rust 侧 CRUD 函数对 `path_globs` 做 `serde_json::from_str<Vec<String>>`(find_pitfalls_by_trigger 的 glob 匹配),tags 透传(由 P2 前端 parse)。与 `subagent_runs.transcript_json` / `session_audit_events.payload_json` 同一 JSON-in-TEXT 惯例。
3. **UUID crate**:倾向 `uuid` crate **v7**(时间有序,B-tree 友好,RFC 9562);项目若已有 uuid 依赖则复用。✅ **CLOSED**:`Cargo.toml` 加 `"v7"` feature(保留 `"v4"` 兼容旧代码);`insert_memory` 用 `Uuid::now_v7()`。
4. **SQLite 并发模型**(2.1):PR1a 查 `PRAGMA journal_mode`/`busy_timeout` 现状——**已开 WAL 则不动**(项目已有 DB 层在用,可能已开);未开则记为独立改进(影响全 DB 层,非本 task 范围)。`update_status` 事务包裹(已在 §3 落地)。✅ **CLOSED**:
   - `init_pool` **未设** `journal_mode` 也未设 `busy_timeout`——全 DB 层走默认(in-memory 测试 `journal_mode='memory'`,文件库默认 `'delete'` 回滚日志;`busy_timeout` 默认 5000ms)。
   - **WAL 改造记为独立改进**(影响全 DB 层,非本 task 范围);现状对单写者访问模式足够,`update_status` 事务包裹防止 status 读与 `bump_hit_count` 竞争。
   - 测试 `am_pragma_status_recorded_for_open_q4` 锁定现状(任何一项漂移会失败)。

## Out of Scope(本 task 明确不做)

- remember tool / 任何 LLM 调用(→ P2);session 开始召回注入(→ P2);工具执行前召回 hook(→ P3);旁路事件 reflection(→ P4)
- 状态机自动晋升逻辑 + 卫生 job(→ P5;本 task 只提供 `update_status`/`bump_hit_count` 接口)
- 前端 UI(→ P2);频率控制 session 计数落地(留接口位,P2 补)
- **全 DB 层 WAL 改造**(若现状未开,记独立改进)
- source_session_id 索引(P2/P4 频率控制需要时再加)

## Definition of Done

- migration 幂等 + CRUD + FTS5(或退路)+ 安全网(escape/deny-list/length)+ 状态机字段 全部单测覆盖
- `cargo test --lib` + `cargo check` 0 warning(PKG_CONFIG_PATH 环境)
- FTS5 + tokenizer 决策、SQLite PRAGMA 现状记录在 prd Open Q
- P2-P5 可直接消费本 task 的 CRUD 接口开工

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md)
- 完整设计 + schema 来源:[spike-007 §5](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
- 写入安全网来源:spike-005 §4;FTS5 来源:spike-006 §4.1

## §review-log

**spike-review(2026-06-29,v2 吸收)**

| 条目 | 处理 |
|---|---|
| 1.1 trigger_key JSON 查询语义 | ✅ 拆 3 列 + idx_am_pitfall |
| 1.2 FTS5 运算符注入 | ✅ escape_fts5 + AC |
| 1.3 NULL 复合索引行为 | ✅ EXPLAIN 验证 |
| 2.1 WAL/busy_timeout | ✅ PR1a 查现状;update_status 事务化 |
| 2.2 边界 + 枚举非法值 | ✅ CHECK + 边界单测 |
| 2.3 敏感路径 deny-list | ✅ .ssh/.aws 直接拒 |
| 2.4 UUID v7 | ✅ Open Q #3 定 v7 |

**deepseek-review(2026-06-29,v3 吸收)**

| 条目 | 处理 |
|---|---|
| B1 title 建议/拒绝矛盾 | ✅ CHECK(length(title)<=200)+content<=500,统一拒绝 |
| B2 FTS5 触发器缺失 | ✅ §2 补 3 触发器 sketch(含 'delete' 特殊行) |
| B3 缺 source_session_id | ✅ DDL 加列(修复 §4 频率控制内部不一致) |
| H1 FK 决策未记录 | ✅ 不设 CASCADE,保留 + P5/sweep 清,§1 注释定档 |
| H2 scope/project_id 交互 | ✅ §3 定 3 种语义 + AC |
| H3 escape phrase tradeoff | ✅ §4 注释记 tradeoff,V2 tokenize OR |
| H4 PR1 偏大 | ✅ 拆 PR1a/PR1b |
| H5 confidence 无消费方 | ✅ 字段注释标 forward-compat/P5 |
| M1 schema 演变未记 | ✅ §1 上方加 trigger_key 拆列说明 |
| M2 path_globs NULL 语义 | ✅ 注释 NULL=不限路径 + Rust Option<Vec> |

**implementation-log(2026-06-29,P1 落地决策)**

| 决策 | 处理 |
|---|---|
| FTS5 + tokenizer 选型 | ✅ FTS5 启用 + `tokenize='trigram'`(默认 `unicode61` 对 CJK 失效,trigram 对 ASCII 子串 + ≥3 字符中文友好;tradeoff:trigram 要求 query ≥3 字符,2 字符中文 query 不命中)。详见 Open Q #1 闭合段 |
| glob 方言 | ✅ `session_tool_permissions` 风格 glob(`*` 不跨 `/`,非原生 SQLite GLOB——check 期实测 SQLite 3.53 `'a/b' GLOB 'a*'` 返回 1,原生 GLOB 的 `*` **会**跨 `/`;此处用的是 re-grill 定档的自定义变体);`app/src-tauri/*` 只匹配单层(`app/src-tauri/Cargo.toml`),不匹配深层(`app/src-tauri/src/lib.rs`)。spike-007 re-grill 已明示不接受 `**` 递归。`find_pitfalls_by_trigger` 的 `glob_matches_path` 是手写字节级 matcher(避免 globset crate 依赖);**char-level caveat**:`?` 按 byte 匹配,非 SQLite GLOB 的 `sqlite3Utf8Read` char-level——CJK glob 带 `?`(如 `中?` 匹配 `中文`)在此实现不命中(P1 接受,CJK path glob 罕见);test `find_pitfalls_path_globs_semantics` 锁定 ASCII 行为 |
| dead_code 策略 | ✅ 模块级 `#![allow(dead_code)]` + 文档注释解释 P1 是存储底座无 production 消费方(P2 remember tool 是首个 caller)。沿用 `subagent_runs.rs` PR2 的先例;P2 落地后移除 |
| 长度检查时机 | ✅ Rust 安全网前置拒绝(DB CHECK 兜底);错误信息可操作("title length 201 exceeds 200" vs DB 的 "CHECK constraint failed") |
| `escape_fts5` phrase match | ✅ H3 tradeoff:v1 整 query 包双引号(FTS5 phrase match,要求词序连续);`"WSL cargo"` 搜不到 `cargo ... WSL`。v1 接受(精度优先);v2 改 tokenize 后各 token OR-join |
| 测试范式 | ✅ 新建 `db/memories_tests.rs`,复制 `test_pool`(无 common helper,遵循 db 6 域文件惯例);26 测覆盖全部 AC |
