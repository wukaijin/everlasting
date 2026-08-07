## Scenario: V2-2+ Observability & Management (2026-07-06)

> **基线**:`07-06-am-observability-panel`(Phase A 后端 + Phase B 前端,2 commits)
> **epic**:V2 2 期自主记忆的**可观测性 + 管理层**。P1-P5 全是 agent 自主闭环(写 / 召回 / 反思 / 提升 / 卫生);本场景补**人**的入口 — 看召回命中、手动转状态、人工编辑、删除。
> **何时读本文**:`update_memory` / `update_status` IPC / `ChatEvent::Recall` / `validate_memory_text` helper / `MemoryRow.edited_by_user` 列 / `RuntimeMemoryModal.vue` / `recallHitsBySession` store state / ChatPanel recall chip 任一相关时。

### 1. Scope / Trigger

- **Trigger**: 用户想(1)看 agent 这次/历史召回命中了哪些记忆,(2)手动干预状态机(candidate→active 或 active→demoted),(3)修正一条写错的记忆,(4)删一条过时记忆。
- **Why code-spec depth**:
  - `ChatEvent::Recall` 是**只读 / 非持久化**事件(跟 `Retrying` 同类),必须不污染 messages DB shape;且 worker sink **不可**转发到主 chat channel(AC7,否则 worker 子记忆命中会冒泡到用户聊天)。
  - `update_memory` 复用 `insert_memory` 的安全网(空 / 超长 / sensitive regex / sensitive path / temp path + home 泛化),提取为 `validate_memory_text` 共享 helper — 否则用户手改能绕过 agent 写入的护栏。
  - `edited_by_user` 列区分 provenance(人 vs agent),让 UI 能标「人工编辑」徽标;默认 0(agent 写),仅 `update_memory` 置 1。
  - 状态机矩阵(P5)**前端只读副本**(`LEGAL_STATUS_TRANSITIONS`)+ **后端硬墙**(`update_status` transactional 二次校验):前端 dropdown 只 OFFER 合法目标,但 backend 永远 re-validate,race / stale dropdown 也兜得住。

### 2. Signatures

```rust
// app/src-tauri/src/db/memories.rs — A2/A3/A4
pub fn validate_memory_text(title: &str, content: &str) -> Result<(), MemoryWriteError>;
//   单源安全网,insert_memory + update_memory 共用。empty / oversize /
//   sensitive-regex / sensitive-path / temp-path + home 泛化。

pub async fn update_memory(
    pool: &SqlitePool,
    memory_id: &str,
    title: &str,
    content: &str,
) -> Result<MemoryRow, MemoryUpdateError>;
//   validate_memory_text → UPDATE title/content, edited_by_user=1,
//   updated_at=now() → 回读返 MemoryRow(authoritative timestamp)。
//   MemoryUpdateError = { SafetyNet(MemoryWriteError), NotFound, Db(sqlx::Error) }

// MemoryRow 新增字段(A4):
pub struct MemoryRow {
    // ... 既有字段 ...
    pub edited_by_user: bool,   // #[serde(default)] 向后兼容;默认 0
}
```

```rust
// app/src-tauri/src/llm/types.rs — A7
pub enum ChatEvent {
    // ... 既有变体 ...
    Recall { hits: Vec<RecallHit> },   // 只读 / 非持久化,同 Retrying
}

pub struct RecallHit {                 // 无 rename_all — snake_case 字段
    pub memory_id: i64,                // 对齐 MemoryRow.id(SQLite auto-id)
    pub title: String,
    pub kind: String,
    pub source: String,                // "fts" | "pitfall"
}
//   对齐既有 ChatEvent nested-payload 约定(Retrying/FileInjections 均 snake)。
//   与 MemoryRow(独立 #[serde(rename_all="camelCase")])是两个类型,勿混。
```

```rust
// app/src-tauri/src/agent/memory_recall.rs — A8
pub async fn build_recall_text_with_rows(
    pool: &SqlitePool,
    project_id: &str,
    query: &str,
) -> Option<(String, Vec<MemoryRow>)>;
//   sibling。原 build_recall_text 退化为 thin wrapper(.map(|(t,_)| t)),
//   保留为 4 个 P2 测试零回归;production 走 with_rows 拿伴随 rows emit。
```

```rust
// app/src-tauri/src/agent/permissions/check.rs — A9
pub async fn recall_pitfall_with_hits(/* same args as recall_pitfall */)
    -> (PitfallRecall, Vec<MemoryRow>);
//   sibling。共享 recall_pitfall_inner_with_rows(单源分类逻辑);
//   recall_pitfall 退化为 wrapper(.0),保留 P3 测试零回归。
//   PitfallRecall enum 字节不变(Footnote/SoftBlock/None)— P3/P4/P5 闭环零回归。
```

```rust
// app/src-tauri/src/commands/memory.rs — A5/A6 IPC
#[tauri::command]
pub async fn update_autonomous_memory_status(
    memory_id: String, new_status: String, demoted_reason: Option<String>,
) -> Result<(), AppCommandError>;
//   包 db::update_status。lenient from_str_opt + 矩阵硬墙。

#[tauri::command]
pub async fn update_autonomous_memory(
    memory_id: String, title: String, content: String,
) -> Result<MemoryRow, AppCommandError>;
//   包 db::update_memory。返 authoritative row。
```

### 3. Contracts

- **Recall event 生命周期**:emit 在 3 处 — FTS 召回(turn start,`chat_loop.rs:1391`)+ pitfall 召回(tool dispatch,`:2496` / `:3267`)。`emit_recall_event` helper 集中 emit 逻辑。LLM stream 路径的 defensive match arm(`:1710`)丢弃任何漏过来的 Recall(只读事件不该来自 LLM)。**新 user message(`startRequest`)清空该 session 的累积 recall hits**(per-turn,非跨对话累积 — design D7)。
- **worker 隔离 (AC7)**:`SubagentBufferSink.emit_chat_event` 只调 `self.record()`(→ `subagent:event` channel),**无 `chat-event` emit 路径**。结构性锁定:测试 `worker_sink_does_not_forward_recall_to_main_chat`。
- **状态机矩阵**:前端 `LEGAL_STATUS_TRANSITIONS`(memory.ts 导出)= backend `update_status` 矩阵的只读副本。candidate→{active,verified,demoted} / active→{verified,demoted} / verified→{demoted} / demoted→{candidate,active,verified}(自转换排除)。**backend 永远 re-validate**;前端 dropdown 只是 UX,不是安全边界。
- **`edited_by_user` provenance**:agent 写(`remember` / P4 auto-reflect)→ 0;人写(`update_memory`)→ 1。UI 渲染「人工编辑」徽标。migration 默认 0(旧行回填)。
- **乐观 + 回滚**:前端 `updateMemoryStatus` / `updateMemory` 均 optimistic patch + IPC + 失败回滚 + `runtimeMemoriesError` banner。`updateMemory` 成功后用 IPC 返回的 authoritative row 覆盖本地(避免 client/server 时钟漂移 on `updatedAt`)。

### 4. 常见错误

- **把 `RecallHit.memory_id` 当 UUID**:`memory_id` 是 SQLite auto-id(对齐 `MemoryRow.id`),不是 `memory_id` UUID 字段。RuntimeMemoryModal 用它打开匹配 row;删除仍走 `delete_autonomous_memory` 的 UUID。
- **前端复制矩阵合法性检查**:`LEGAL_STATUS_TRANSITIONS` 只是 dropdown 的 OFFER 列表,**不是**安全边界。永远让 backend `update_status` 矩阵做权威校验;前端做「合法检查」会跟 backend 漂移。
- **recall hits 累积进 messages buffer**:`ChatEvent::Recall` 是 transient(同 `Retrying`),handler 路由进 `memoryStore.recallHitsBySession`,**绝不**写进 `messages` 数组(会污染持久化 DB shape)。

---
