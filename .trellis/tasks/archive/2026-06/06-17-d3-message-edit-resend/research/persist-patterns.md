# Research: Chat message edit 持久化模式对比

- **Query**: D3 (session 内消息编辑/重发) 的持久化模式选择 — 评估 5 种常见模式 (in-place / append-only+lineage / soft column / branch / versioned snapshot) 的存储开销、查询简单度、undo / audit 可行性、迁移成本与业界参照,推荐 1-2 种适合本项目 MVP 的方案
- **Scope**: 内部 schema (`app/src-tauri/src/db/`) + 行业惯例 (Claude Code / Cursor / Aider / Cline / ChatGPT / Slack / Notion / GitHub) + SQLite 特定 gotcha
- **Date**: 2026-06-17
- **关联 task**: `.trellis/tasks/06-17-d3-message-edit-resend/`
- **关联 spec**: `.trellis/spec/backend/database-guidelines.md`, `docs/IMPLEMENTATION.md §4 2026-06-17 (D2 降档决策)`
- **D3 MVP 范围假设** (来自 prd.md A1-A4): edit 仅 user message; edit = 改 + 级联删后续 + resend; 无 version history; 单用户桌面应用

---

## TL;DR — 推荐

**MVP 强烈推荐: 模式 1 (in-place update) + 模式 3 light (单列 `edited_at` 无 original)**

具体落地:

```sql
-- 单条 ALTER,nullable,无 DEFAULT(对齐 F5 latency 列的迁移模式)
ALTER TABLE messages ADD COLUMN edited_at TEXT;
```

```rust
// db/sessions.rs 新增
pub async fn edit_user_message(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let content_json = serde_json::to_string(new_content)?;
    let text = new_content.to_text();
    sqlx::query(
        r#"
        UPDATE messages
        SET content = ?,
            text    = ?,
            edited_at = ?
        WHERE session_id = ?
          AND seq = ?
          AND role = 'user'
        "#,
    )
    .bind(&content_json)
    .bind(&text)
    .bind(&now)
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_messages_after_seq(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM messages WHERE session_id = ? AND seq > ?",
    )
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await
    .map(|_| ())
}
```

**推荐理由**:
1. **匹配 A1-A4 假设**(MVP 范围不需 version history)
2. **迁移成本 = 1 条 ALTER**(对齐项目既有的 nullable INTEGER/TEXT 模式,见 `migrations.rs:351-353` F5 latency 迁移)
3. **UI 简单** — `edited_at` 触发"已编辑"小标签(类 Slack/Discord),`content` 始终是当前真值,rehydrate 路径无 split-brain
4. **D2 FTS5 同步简单** — 未来加 FTS5 虚拟表时,edit 走 `INSERT OR REPLACE INTO messages_fts` 即可(如果建的是 contentless FTS5,需要显式 `delete` + `insert`;trigger 自动同步在 deferred-D2 实施时一并加)
5. **undo 不可行** — 但 D3 prd.md 显式标 A4 "out of scope: message version history / undo",可接受

**不推荐**:
- 模式 2 (append-only + lineage) — 复杂度远超 MVP,query "load latest" 需 traverse lineage 链,rehydrate 路径要 +30-50 行
- 模式 4 (branch/fork) — 严重过度设计,跟"单 session 单时间线"心智模型冲突
- 模式 5 (versioned snapshot) — 写放大太重,跟 A4 假设矛盾

---

## 一、5 模式对比表

| 维度 | 1. In-place update | 2. Append-only + lineage | 3. Soft column (edited_at + original_content) | 4. Branch / fork | 5. Versioned snapshot |
|---|---|---|---|---|---|
| **存储开销** | 1 row / message(改不动 row count) | N rows / N edits(lineage 链) | 1 row / message + JSON blob history | N rows / branch,N branches / session | 1 row / snapshot,K rows / K snapshots |
| **改 row count** | 永远 0 | +1 per edit | 0 | +N per fork | +K per snapshot |
| **查询"load latest"** | `SELECT * FROM messages WHERE session_id = ? ORDER BY seq` — 直接 | `SELECT * FROM messages WHERE session_id = ? AND is_head = 1 ORDER BY seq` 或 traverse lineage 找 head | 同模式 1(`is_edited` 仅 UI 标志) | `SELECT * FROM messages WHERE branch_id = ? ORDER BY seq` | `SELECT * FROM snapshots WHERE session_id = ? ORDER BY ts DESC LIMIT 1` |
| **Undo edit 可行性** | 不可行(原值已丢) | 可行(指向旧 row) | 简单(`UPDATE SET content = original_content`) | 切 branch 即可 | 简单(`UPDATE messages` from snapshot) |
| **Audit / replay 可行性** | 弱(只能 audit"何时改了",不知改了什么) | 强(lineage 链 = 完整历史) | 强(`edit_history` JSON 含每版) | 强(每 branch 是独立完整历史) | 强(snapshot 序列) |
| **回放"edit 后所有 assistant turn"** | 不可行 | 可行(lineage 找所有 is_head=false 的兄弟) | 不可行 | 可行(切 branch 即可) | 可行 |
| **SQL 复杂度** | 低(单条 UPDATE) | 中(需 `parent_message_id` 列 + 标记 head 的列/子查询) | 低-中(单条 UPDATE + history 字段 append) | 高(branch 表 + JOIN) | 中(snapshots 表 + 双读路径) |
| **代码复杂度** | 极低(20-30 行) | 中(60-100 行,lineage 遍历 helper) | 低(20-40 行) | 高(branch 切换 UI + DB) | 中(snapshot 触发器 + 比对) |
| **rehydrate 路径改动** | 0(已 work) | +30-50 行(`MessageRow` 加 `parent_id` / `is_head` filter) | 0(只多读一列) | 重(全 UI 要选 branch) | 中(要先 read snapshot 再读增量) |
| **FTS5 sync 复杂度** | 低(改 `content` 列 → `UPDATE messages_fts SET ... WHERE rowid = ?`) | 高(新增 row → INSERT FTS5 + 旧 row → DELETE FTS5,头标记变化) | 低(同模式 1) | 高(每 branch 独立 FTS5 row) | 中(snapshot 时全量 rebuild) |
| **迁移成本 (从 no-version schema)** | 0(无需 schema 变更) | 高(+1 FK 列 + backfill 旧 row `parent_id = NULL` `is_head = 1`) | 极低(单条 nullable TEXT) | 极高(全新表) | 高(全新表) |
| **行业例子** | Slack message edit (核心) / Discord / iMessage / GitHub comment edit / Linear issue body | GitHub PR review comments(edit 后 history 显示) / Notion page history(完整 lineage) / Reddit edit 显示 "(edited by)" 但**不存历史**(混合 1+3) | Notion page version history / Confluence page history | ChatGPT "branch from this message" (2024 落地) / Claude.ai "fork" (新 feature,2025) | Google Docs version history / Wiki version history / Git commits |
| **单用户桌面 app 适用度** | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐(推荐) | ⭐ | ⭐⭐ |
| **MVP 范围适用度** | ⭐⭐⭐⭐⭐ | ⭐ | ⭐⭐⭐⭐(over-engineering if unused) | ⭐ | ⭐ |

---

## 二、5 模式逐项细节

### 模式 1: In-place update

```
[messages]
id | session_id | role | content | seq | ...
1  | sess-1     | user | "hi"    | 0   | ...
2  | sess-1     | asst | "..."   | 1   | ...
3  | sess-1     | user | "fix it"| 2   | ...    <-- edit: UPDATE content = "fix it now" (id 不变)
```

**代码**(已与项目 `update_message_metadata` 模式一致 — `app/src-tauri/src/db/sessions.rs:742-763`):

```sql
UPDATE messages
   SET content = ?, text = ?
 WHERE session_id = ? AND seq = ? AND role = 'user'
```

**优点**:
- 零 schema 变更(用现有 `content` / `text` 列)
- 跟 `record_tool_duration` (`sessions.rs:788-854`) 模式一致 — 都是 in-place patch on existing row
- `load_session` SQL 零修改(`sessions.rs:190-201`)

**缺点**:
- 不可 undo(原 content 永久丢失)
- audit trail 弱 — 不知道"曾几何时是 X,现在是 Y"

**适用条件**: 跟项目 A1-A4 假设完全匹配。

### 模式 2: Append-only + lineage

```
[messages]
id | session_id | role | content | seq | parent_message_id | is_head
1  | sess-1     | user | "hi"    | 0   | NULL              | 0   <-- 旧版
2  | sess-1     | asst | "..."   | 1   | NULL              | 1
3  | sess-1     | user | "fix it"| 2   | NULL              | 0   <-- 旧版
4  | sess-1     | user | "fix now"| 2  | 3                 | 1   <-- 新 head,parent = 旧 row
```

**回放"edit 后所有 assistant turn"** 关键 query:

```sql
-- 找 seq=2 的当前 head
SELECT id FROM messages WHERE session_id = ? AND seq = 2 AND is_head = 1;
-- 拿到 id=4,找它的 lineage
SELECT * FROM messages
 WHERE session_id = ? AND id IN (
   WITH RECURSIVE lineage(id) AS (
     SELECT 4 UNION ALL
     SELECT parent_message_id FROM messages m JOIN lineage l ON m.id = l.id
     WHERE m.parent_message_id IS NOT NULL
   )
   SELECT id FROM lineage
 ) ORDER BY seq;
```

**优点**:
- 完整历史(每改一次 = 一行,可做完整 audit)
- "branch out" 跟模式 4 自然融合(同套 lineage 机制)

**缺点**:
- **rehydrate 路径全改** — `load_session` 要遍历 `(session_id, seq)` 找 head,从前 `ORDER BY seq` 直接是按 seq 全选 — 现在要先 `GROUP BY seq HAVING is_head = 1` 或类似
- **seq 不再 unique** — 项目当前 schema `UNIQUE(session_id, seq)` (`migrations.rs:203`) 跟 lineage 模型根本冲突(同 seq 多个版本 = unique 违反)
- **seq 索引降级** — `idx_messages_session_seq` (`migrations.rs:216`) 从 covering index 变成"含 dead row",rehydrate 全靠内存 filter
- **跟现有 D1 (color_tag) / F5 (latency) / B2 PR3 (metadata) 模式不符** — 这些都是 in-place 改,引入 lineage 会让两条范式冲突

**迁移成本**: 需要
1. 加 `parent_message_id INTEGER` nullable + `is_head INTEGER NOT NULL DEFAULT 1`
2. backfill 旧 row:`UPDATE messages SET is_head = 1 WHERE parent_message_id IS NULL`(idempotent)
3. 改 `persist_turn` (`sessions.rs:566-638`) 接受"可选 lineage 父 id"
4. 改 `load_session` 加 `AND is_head = 1` filter

**评估**: MVP 强烈不推荐 — 复杂度远超需求。

### 模式 3: Soft column (edited_at + original_content)

```
[messages]
id | session_id | role | content      | original_content | edited_at | seq
1  | sess-1     | user | "fix now"    | "fix it"         | 2026-...  | 2   <-- edit 1 次
2  | sess-1     | user | "fix now v3" | "fix now"        | 2026-...  | 2   <-- edit 2 次
```

或 `edit_history` JSON 数组:

```json
{"edits": [
  {"at": "2026-...", "from": "fix it", "to": "fix now"},
  {"at": "2026-...", "from": "fix now", "to": "fix now v3"}
]}
```

**优点**:
- 简单(单条 ALTER + 现有 UPDATE 模式)
- undo 可行(`SET content = original_content`)
- UI 可显示"已编辑"标签(类 Slack/Discord/Reddit 那种小灰字)
- rehydrate 路径零修改

**缺点**:
- `original_content` 单列只能存"上一版"(多版历史需 JSON 数组 = 改 SQL 范式)
- `edit_history` JSON 数组 — 越长越重(无界增长),需要 cap(如保留最近 5 版)
- audit 不完整(JSON 数组能查"曾几何时",但不能查"哪个 assistant 响应了哪一版" — 这要 lineage 模式才能)

**评估**: 推荐作为 MVP 方案(在模式 1 基础上加 `edited_at` 单列) — 平衡"显示已编辑"和"复杂度"。

### 模式 4: Branch / fork

```
[sessions]
id       | ...
sess-1   | ...
sess-1/b | ... (branch "b" from sess-1 at message seq=2)

[messages]
session_id | seq | role | content
sess-1     | 0   | user | "hi"
sess-1     | 1   | asst | "..."
sess-1     | 2   | user | "fix it"      <-- 用户 fork 在此
sess-1/b   | 0   | user | "fix it"      <-- 复制起点
sess-1/b   | 1   | asst | "...new..."   <-- 新分支的新 response
```

**优点**:
- 完整保留原 session(无破坏)
- 多探索路径并行
- ChatGPT / Claude.ai 在 2024-2025 落地了类似 feature

**缺点**:
- **严重过度设计 for D3 MVP** — A1-A4 假设"edit = 改 + 级联删后续"
- 跟项目"单 session 单时间线"心智模型冲突
- 业务价值低 — coding agent session 多为"一次任务一次对话",branch 是 ChatGPT 那种开放聊天场景的需求
- 迁移成本极高(新表 + 全 UI 改 branch picker)

**评估**: D3 MVP 强烈不推荐,可能适合"未来 3.0 探索"但**不进 D3 范围**。

### 模式 5: Versioned snapshot

```
[sessions]
id       | current_snapshot_id
sess-1   | snap-3

[session_snapshots]
id      | session_id | ts        | messages_json
snap-1  | sess-1     | 2026-...  | '[{"seq":0,"role":"user",...},...]'
snap-2  | sess-1     | 2026-...  | '[{"seq":0,"role":"user",...},{...edited...}]'
snap-3  | sess-1     | 2026-...  | '[...]'

[messages]
... live working state ...
```

**优点**:
- 全 session 原子版本
- Google Docs / Wiki 标准模式
- 可做"回到任意历史点"

**缺点**:
- 写放大太重(每改一次 = 序列化全 session)
- 当前项目已有"事实表 + metadata 模式"(B2 PR3 走 `update_message_metadata` 增量 patch),snapshot 模式跟这个范式冲突
- A4 假设"无 version history",直接违反

**评估**: 强烈不推荐 — 跟项目模式 + MVP 假设双重冲突。

---

## 三、SQLite 特定 gotcha

### 3.1 `UNIQUE(session_id, seq)` 跟 lineage 模式冲突

**当前 schema** (`migrations.rs:201-204`):
```sql
CREATE TABLE messages (
    ...
    seq INTEGER NOT NULL,
    UNIQUE(session_id, seq)
)
```

模式 2 (append-only + lineage) 要求**同 seq 多版本**,这把 unique 约束直接打破。要采用模式 2 必须:
1. 删 unique constraint(用 `CREATE UNIQUE INDEX ...` 替代,加 `is_head` 进去:`UNIQUE(session_id, seq, is_head) WHERE is_head = 1`)
2. 或删 unique 改用应用层 enforce

**MVP 模式下不踩这个坑**(模式 1 / 3 都保持 unique 不变)。

### 3.2 索引影响

`idx_messages_session_seq` (`migrations.rs:216-219`) 是 covering index,`load_session` 走 `WHERE session_id = ? ORDER BY seq ASC`。

- 模式 1 / 3:in-place UPDATE 不影响索引(`seq` 没变,只是 row 内容变),zero impact
- 模式 2:新 row 用同 `seq` + `is_head=0`,索引 leaf page 多了 N 个 entry,rehydrate `AND is_head = 1` filter 在 index seek 后还要多扫,效率降
- 模式 4:每个 branch 独立 `session_id`,索引分裂,影响中等
- 模式 5:snapshots 表加独立索引,跟 messages 索引无关

**MVP 选模式 1 / 3 索引零影响**。

### 3.3 WAL 模式

项目当前用 `init_pool` (`migrations.rs:24-50`) — 未显式设 `PRAGMA journal_mode = WAL`。WAL 对 D3 的影响:
- 模式 1 / 3:in-place UPDATE = 单行写,WAL 几乎免费
- 模式 2:多行 INSERT(同 turn N 个 edit),WAL 帧更多
- 模式 5:全 session JSON 序列化写,WAL 帧大

**推荐**: D3 落地时同步把 `PRAGMA journal_mode = WAL` + `PRAGMA synchronous = NORMAL` 加进 `init_pool` — 这是 Tauri/SQLite 桌面 app 标配,rehydrate 读不被写阻塞(agent loop 写 messages 不会卡 UI load)。但**不阻塞 D3** — 这是独立优化。

### 3.4 FTS5 sync(为 D2 未来实施铺路)

D2 降档决策(`docs/IMPLEMENTATION.md §4 2026-06-17`)写明:未来 D2 FTS5 形态 = `messages_fts` 虚拟表(FTS5,unicode61)+ `search_messages` Tauri command。

**当前 D2 没实施,但 spec 已预想**(`memory/types.rs:19` 提到 memories FTS5 已实施,messages FTS5 未实施)。

**D3 + D2 未来 FTS5 sync 模式**:

| D3 模式 | FTS5 sync |
|---|---|
| 模式 1 in-place update | **trigger 自动同步**:`CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN INSERT INTO messages_fts(rowid, content) VALUES (new.id, new.content); END; CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN DELETE FROM messages_fts WHERE rowid = old.id; END; CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN UPDATE messages_fts SET content = new.content WHERE rowid = old.id; END;` — FTS5 的 contentless table 不能用 trigger 自动 sync,需要 `external content` 配置 |
| 模式 2 append-only | 复杂:edits 不只改一 row,而是 INSERT 新 row + DELETE 旧 row(配合 head 标记切换),trigger 要处理 head 切换语义 |
| 模式 3 soft column | 同模式 1(trigger 自动 sync) |
| 模式 4 branch | 极复杂:每 branch 独立 FTS5 row,FK / CASCADE 都要重做 |
| 模式 5 snapshot | 极简:snapshot 时全量 `INSERT INTO messages_fts SELECT ... FROM messages WHERE session_id = ?`,但写入路径要小心 race |

**结论**: 模式 1 / 3 跟未来 D2 FTS5 sync 路径最自然。

**Deferred D2 同步 spec 写法**(`database-guidelines.md` 待 D2 实施时补):
```sql
-- FTS5 virtual table,external content 模式(数据存 messages.content,不在 FTS5 内)
CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    content='messages',
    content_rowid='id',
    tokenize='unicode61'
);
-- trigger 自动 sync 写入/删除
-- edit(D3 模式 1)走 messages_au,自动同步
```

### 3.5 事务一致性

D3 edit 流程的 race:
1. 用户触发 edit_user_message(seq=2)
2. **同时** agent loop 仍在 stream 写 messages(seq=3, 4, 5)
3. 顺序错乱 → "edit 完发现后续 message 是旧版" / "新 assistant 响应挂在旧 user message 上"

**强制顺序**: D3 edit 必须跟 agent loop 串行 — 用户 edit 时如果有 active stream,**先 cancel 后 edit**。`streamController.activeRequests` 已有 busy lock 框架(`streamController.ts`),可复用。

**SQLite 事务内 edit + cascade delete**:

```rust
pub async fn edit_user_message_and_cascade(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    // 1. UPDATE edited message
    sqlx::query(
        "UPDATE messages SET content = ?, text = ?, edited_at = ? WHERE session_id = ? AND seq = ? AND role = 'user'",
    )
    .bind(&serde_json::to_string(new_content)?)
    .bind(&new_content.to_text())
    .bind(&Utc::now().to_rfc3339())
    .bind(session_id)
    .bind(seq)
    .execute(&mut *tx)
    .await?;
    // 2. DELETE 后续 message
    sqlx::query("DELETE FROM messages WHERE session_id = ? AND seq > ?")
        .bind(session_id)
        .bind(seq)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
```

**必须用事务** — 否则 step 1 成功 step 2 失败 → "user message 是新版,但 assistant response 还在" = 数据分裂。**`emit_persist_failure` 路径需要走这个事务失败**(`chat_loop.rs:957` 已有 helper)。

### 3.6 `seq` 复用 vs 续号

Edit + cascade delete 后,新 resend 写的 assistant 消息 `seq`:
- 选项 A: 续号(seq=3 已被删,新 assistant 用 seq=3)— 简单,但 seq 跟物理存储位置不一致
- 选项 B: 不续号(seq 跳号)— 一致性好,但 `MAX(seq)+1` 计算无影响(`insert_system_event` 现有 `migrations.rs:512-516` 模式)

**推荐 A**(续号) — 跟项目现有"MAX(seq)+1"模式一致,且 rehydrate 路径不用感知"seq 跳号"。

---

## 四、行业惯例对照(2025-2026 状态)

> 注: mcp__exa__web_search_exa 未在本环境实际暴露,本节为 knowledge cutoff (2026-01) 综合,无 web 搜索直接证据。下表为通用知识,具体引用前需在实施 PR 中用 `context7` 或 web 二次确认。

### Claude Code (Anthropic CLI,2025-2026)

- **edit 行为**: 通过 `/edit` slash command + 选消息 + 改文本 + 重新提交。`/rewind` 支持回到任意 turn(checkpoint 机制)
- **持久化**: JSONL 文件,append-only(类似模式 2 但不叫 lineage — 每条消息是独立 entry,含 turn_id + parent_turn_id)
- **参考**: 项目 README 引用过 Aider `history.py` 模式(可能就是 append-only JSONL)
- **对本项目启示**: Claude Code 的 `/rewind` 是 checkpoint 机制,**不是** message edit 模式 — D3 范围(cascade delete)更简单

### Cursor Agent Mode

- **edit 行为**: 聊天区"重新生成"(regenerate)按钮是主流;直接 edit user message 的入口隐藏(更倾向"再发"+"接受 / 拒绝")
- **持久化**: 闭源,推测 in-memory + 服务端持久化
- **对本项目启示**: "regenerate assistant response"是常见交互,但 D3 prd.md 写明 A1 "edit 仅 user message",所以 Cursor 行为对 D3 影响有限

### Aider

- **edit 行为**: `/undo` 命令是主流(回到上一 turn),不是 edit user message
- **持久化**: append-only JSONL(`aider/history.py`)
- **对本项目启示**: 跟 Claude Code 类似 — `/undo` 是"删最后 turn"语义,跟 D3 "edit + cascade delete"同构但更简单

### Cline (VS Code Extension)

- **edit 行为**: 没有"edit user message"功能,主流是"删到此处"+"重新发"
- **持久化**: VS Code state storage + extension local storage
- **对本项目启示**: Cline 验证了"无 edit message"是合理 MVP — 但 D3 prd.md 已决定要 edit,所以走"改 + cascade" 模式

### ChatGPT (2024-2025)

- **edit 行为**: 鼠标悬停 user message → 铅笔 icon → 改文本 → "Submit" 触发 resend,后续 assistant response 全部替换
- **持久化**: 服务端(闭源,推测 append-only + version)
- **branch/fork**: 2024 落地"Branch from this message"功能
- **对本项目启示**: 跟 D3 设计同构 — 改 user message + cascade delete + resend = ChatGPT 主流交互

### Claude.ai (网页,2024-2025)

- **edit 行为**: 跟 ChatGPT 类似
- **branch/fork**: 2025 落地"Edit + branch"模式(改 + 保留原 branch)
- **对本项目启示**: branch 是"未来 3.0"的方向,**不进 D3 MVP**

### Slack / Discord / iMessage

- **edit 行为**: 改消息原文 + 显示"(edited)"小标签(模式 3)
- **持久化**: 服务端 in-place update(模式 1)+ `edited_at` 时间戳
- **对本项目启示**: "edited_at 单列"是行业最简单可行范式,D3 MVP 跟它完全同构

### Notion / Confluence

- **edit 行为**: 完整 page version history(模式 5 snapshot)
- **持久化**: 每次 save = snapshot,UI 显示时间线
- **对本项目启示**: Notion 模式适合"文档编辑",不适合"聊天消息"(chat 场景不需要"回到任意点")

### GitHub PR 评论

- **edit 行为**: 改 comment + "(edited)"标签 + 有 edit 历史可查
- **持久化**: 模式 1 + 3 hybrid(`edited_at` + 内部历史表)
- **对本项目启示**: GitHub 模式值得参考 — `(edited)` 标签 + 历史可查是好的 UX,D3 MVP 至少做 `(edited)` 标签

### Linear

- **edit 行为**: 改 issue body = in-place,无 cascade(因为 issue 是单条目不是对话流)
- **持久化**: 模式 1
- **对本项目启示**: Linear 的简洁模式跟 D3 MVP 对齐

---

## 五、对本项目的具体推荐(展开 TL;DR)

### 推荐 1: In-place update + `edited_at` 单列(MVP 首选)

**Schema 改动**(1 条):
```sql
-- m6d3 ALTER,加在 migrations.rs 末尾(F5 latency 列之后)
add_messages_column_if_missing(pool, "edited_at", "TEXT").await?;
```

**代码改动**(`db/sessions.rs`):
```rust
/// D3: edit user message in place + bump edited_at.
/// Bumps the column on the existing row (id stays the same);
/// the rehydrate path picks up the new content via the same
/// `load_session` SELECT — no schema-level cascade needed
/// because `content` and `text` are the source of truth.
///
/// Idempotent on missing row: returns Ok(()) without error
/// (the agent loop may race — see chat_loop cancel/cleanup).
pub async fn edit_user_message(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let content_json = serde_json::to_string(new_content)
        .map_err(|e| sqlx::Error::Encode(format!("serialize content: {}", e).into()))?;
    let text = new_content.to_text();
    sqlx::query(
        r#"
        UPDATE messages
        SET content = ?,
            text = ?,
            edited_at = ?
        WHERE session_id = ?
          AND seq = ?
          AND role = 'user'
        "#,
    )
    .bind(&content_json)
    .bind(&text)
    .bind(&now)
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}

/// D3: cascade-delete all messages strictly after `seq` in
/// a session. Used after `edit_user_message` to wipe the
/// (stale) assistant + tool_result chain so the new resend
/// starts from a clean slate. Mirrors `delete_messages_by_session`
/// (B3 `/clear`, `sessions.rs:265-274`) but scoped to a seq
/// boundary.
///
/// Foreign-key CASCADE is not relied on (we DELETE messages
/// explicitly so behavior is correct even when PRAGMA
/// foreign_keys = ON wasn't set on the original row —
/// see `delete_session` rationale, `sessions.rs:243-257`).
pub async fn delete_messages_after(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM messages WHERE session_id = ? AND seq > ?",
    )
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}
```

**Tauri command 改动**(`commands/sessions.rs`):
```rust
/// D3 IPC: edit a user message + cascade-delete the tail.
/// Returns the new seq-anchored message id (for streamController
/// to wire up the resend). Caller must cancel any active
/// stream on the session first (busy lock in
/// `streamController.activeRequests`).
#[tauri::command]
pub async fn edit_message(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    seq: i64,
    new_content: MessageContent,
) -> Result<i64, String> {
    let mut tx = state.db.begin().await
        .map_err(|e| format!("begin tx failed: {}", e))?;
    db::sessions::edit_user_message_tx(
        &mut tx, &session_id, seq, &new_content,
    ).await.map_err(|e| format!("edit_user_message failed: {}", e))?;
    db::sessions::delete_messages_after_tx(
        &mut tx, &session_id, seq,
    ).await.map_err(|e| format!("cascade delete failed: {}", e))?;
    tx.commit().await.map_err(|e| format!("commit failed: {}", e))?;
    Ok(seq)
}
```

**前端改动**(`stores/chat.ts` + `MessageList.vue` + 新 `<MessageActionsMenu>`):
- reka-ui `DropdownMenu` (跟 B2 PR3 `<TriggerMenu>` 模式一致 — 都用 reka-ui)
- 鼠标悬停 message 出现 ⋯ 菜单(Edit / Resend / Copy 三项)
- Edit 点击 → 原地变 `<textarea>` (或 reka-ui `Popover` 隔离) → Save 触发 IPC
- `streamController.editMessage(sessionId, seq, newContent)` — 内部 `await cancel()` 现有 active stream → `invoke('edit_message')` → `await send(resendPayload)`

### 推荐 2(备选 / 未来): 模式 3 加 `original_content` 单列(若用户强烈要求"撤销编辑")

**Schema 改动**(+1 条):
```sql
add_messages_column_if_missing(pool, "original_content", "TEXT").await?;
```

**用途**:
- 第一次 edit 时备份 `content → original_content`
- 第二次 edit 覆盖(只保留"最近一次的原版")
- "撤销" 按钮 = `SET content = original_content, original_content = NULL, edited_at = NULL`

**评估**: 跟 A4 假设("无 version history")轻度冲突,但如果用户后续要求"撤销上一步 edit"(Slack 那种),这是最小成本扩展 — 改 1 列 + 改 1 个 UPDATE 路径。

**MVP 不实施,留作 v2 升级路径**。

### 不推荐(本项目)

- **模式 2 (append-only + lineage)**: 跟 `UNIQUE(session_id, seq)` 冲突 + rehydrate 路径全改,成本 / 收益完全不成正比
- **模式 4 (branch)**: 跟"单 session 单时间线"心智模型冲突,D3 MVP 范围外
- **模式 5 (snapshot)**: 写放大 + 跟项目 metadata 模式冲突,A4 假设反对

---

## 六、未来 D2 FTS5 + D3 协同(为后续实施铺路)

D2 降档到第三档(2026-06-17)但 spec 已预想 `messages_fts` 形态。D3 实施时**不写 FTS5 sync 代码**(D2 还没上),但 schema 决策要兼容未来 FTS5:

| D3 模式 | 未来 FTS5 sync 路径 | 改动 |
|---|---|---|
| 模式 1 / 3 (in-place) | `messages_fts` 配 `external content`,trigger 自动 sync — edit 走 `messages_au` 自动 | 0(D3 时无影响) |
| 模式 2 (lineage) | trigger 要处理"head 切换"语义,要写自定义 sync 函数 | 复杂 |
| 模式 4 (branch) | 每 branch 独立 FTS5 row | 极复杂 |
| 模式 5 (snapshot) | snapshot 时全量 rebuild,rehydrate 路径简单 | 中等 |

**结论**: D3 MVP 选模式 1 / 3,未来 D2 FTS5 同步 trigger 是 trivial 改动(`CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN UPDATE messages_fts SET content = new.content WHERE rowid = old.id; END;`)。

---

## Caveats / Not Found

1. **行业惯例无 web 实查**: 本环境 mcp__exa__* 工具未实际暴露,行业对照基于 knowledge cutoff (2026-01) 综合。Claude Code / Cursor / ChatGPT / Claude.ai 的具体落地形式需在 D3 实施 PR 中用 context7 或 web 二次确认(可用 mcp__context7 查 `prisma` / `drizzle-orm` 等 ORM 的常见 chat pattern 文档)。
2. **Aider history.py 细节未读**: docs/README.md 引用过但未直接读源码。append-only JSONL 是高置信度假设(基于 Aider 设计哲学),具体 schema 需实施时核 `aider/history.py`。
3. **`PRAGMA journal_mode = WAL` 建议是独立优化**: D3 落地时建议加(几乎所有 Tauri 桌面 app 配 SQLite 都开),但**不阻塞 D3**。如果 D3 PR 不带,WAL 可以下个独立 PR 做。
4. **mode 1 不可 undo 的补救路径**: 如果用户后续强烈要求"撤销上一步 edit",MVP 不应该硬加 `original_content` 列(过早设计),而是用**临时方案**:edit 前把旧 content 推入前端 Pinia store 的 `undoStack`,前端层面 undo(MVP 可接受范围)。这跟 A4 假设("out of scope: version history")不冲突 — 前端 undo ≠ DB version。
5. **edit race 必走 cancel first**: 用户 edit 时如果有 active stream,**必须先 cancel**(streamController busy lock 已有),否则 SQL 事务顺序错乱(见 §3.5)。这是 D3 实施必加的不变量。
6. **RULE-A-007 联动**: prd.md 提到 D3 是修 A-007(error 路径 partial text 丢失)的天然窗口。error arm 的 `persist_turn` 路径跟 edit 的 cascade delete 是不同的 patch — A-007 修"已渲染 text 要落 DB",D3 修"edit 后 cascade 删 tail" — 可同 PR 做但**不同代码路径**。
7. **D2 + D3 不应同 PR 做**: D2 在第三档(已降档),D3 在第二档(未动)。D2 触发条件未到(session 积累浅),D3 跟 A-007/A-010 修复窗口紧迫 — D3 不等 D2。

---

## 附录: 决策矩阵(给 main agent 总结用)

| 维度 | 模式 1+3 light(推荐) | 模式 2 | 模式 4 | 模式 5 |
|---|---|---|---|---|
| 跟 A1-A4 假设匹配度 | ✅ 完全匹配 | ⚠️ 冲突 A4 | ❌ 严重过度 | ❌ 冲突 A4 |
| 迁移成本 | 1 ALTER | 多 ALTER + rehydrate 重写 | 全新表 | 全新表 |
| 跟 F5 / B2 PR3 / D1 模式一致性 | ✅ 一致(in-place patch) | ❌ 冲突 | ❌ 冲突 | ⚠️ 中立 |
| D2 FTS5 协同 | ✅ trigger 自动 sync | ⚠️ 自定义 sync | ❌ 极复杂 | ⚠️ rebuild |
| race 安全 | ✅ 单行 UPDATE | ⚠️ lineage 切换 race | ⚠️ branch 切换 race | ⚠️ snapshot race |
| MVP 范围(1 PR) | ✅ 可行 | ❌ 多 PR | ❌ 多 PR | ❌ 多 PR |
| 未来扩展到 undo | 备选 2(加 original_content) | ✅ 自带 | ✅ 自带(切 branch) | ✅ 自带(切 snapshot) |
| 未来扩展到 audit | 弱(只 edited_at) | ✅ 强 | ✅ 强 | ✅ 强 |

**最终结论**: D3 MVP 用模式 1 + 3 light(`edited_at` 单列,in-place update),cascade delete 走 `DELETE FROM messages WHERE session_id = ? AND seq > ?` 单 SQL,事务包裹保证一致性。**未来若用户要求 undo,加 `original_content` 列(模式 3 full)是单列扩展,不破坏现有模式**。
