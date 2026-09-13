## Scenario: Autonomous Memories (DB-backed Runtime Memory, V2 2 期)

> **分篇**(2026-09-13):本文保留核心契约(两系统边界 / DB schema / recall 注入 / `remember` tool / 权限模型)+ 两起跨项目泄漏 Addendum;P3 / P4 / P5 契约与 §4-§7 验证体系已拆至同目录 `scenario-am-{p3-tool-recall,p4-event-reflection,p5-quality-layer,validation}.md`(原锚点以 stub 保留在本文)。
> **基线**:2026-06-29 `06-29-am-p2-readwrite` (P1 落库 + P2 手工读写闭环)
> **epic**:V2 2 期 自主记忆(5-PR rollout,P1 archived,P2 见本文,P3-P5 planning)
> **何时读本文**:`autonomous_memories` 表 / FTS5 召回 / `remember` tool / `memory_recall` 注入 / `stores/memory.ts` runtime memories 状态 / `MemoryPreview` runtime section 任一相关时。

### 1. Scope / Trigger

- **Trigger**: the agent needs long-term **runtime** memory — facts / preferences / decisions that survive across sessions. Distinct from the B5 static instruction files in the scenario above (4 fixed Markdown files, no LLM write surface).
- **Why code-spec depth**: the recall injection is **inside the Anthropic cache breakpoint** (same synthetic user message as `build_instructions_blocks`). A wrong shape (separate message, wrong `cache_control`) silently invalidates the prompt cache and adds 5-10× cost on every turn. The `remember` tool is the LLM's only write surface for this memory — wrong permission semantics (Tier 4 ask) silently degrades the LLM to never remembering anything.
- **V2 2 期 epic rollout**:
  - P1 `06-29-am-p1-storage` (archived): `autonomous_memories` + FTS5(trigram) + `insert_memory` + `search_memories_fts` + safety net
  - **P2 `06-29-am-p2-readwrite` (this scenario)**: `remember` tool + `memory_recall` per-turn injection + `MemoryPreview` runtime section
  - P3 `06-29-am-p3-tool-recall` (archived): tool-execution-time recall (before each `tool_use`)
  - P4 `06-29-am-p4-event-reflect` (archived): event-driven auto-write hooks
  - P5 `06-29-am-p5-quality` (archived): verified soft-intercept + state-machine auto-promotion + hygiene job (see P5 contract below)

### 2. Signatures

```rust
// app/src-tauri/src/db/memories.rs
pub enum MemoryScope { User, Project }                                    // snake_case in DB
pub enum MemoryKind { Preference, Fact, Decision, Pitfall, Skill, Other }
pub enum MemoryStatus { Candidate, Active, Verified, Archived, Deleted }

pub struct Memory {
    pub id: i64,
    pub project_id: Option<String>,                                       // None → user scope
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub title: String,                                                    // ≤200 chars
    pub content: String,                                                  // ≤500 chars (P2 safety net)
    pub tags: Option<String>,                                            // JSON array as TEXT
    pub source_session_id: Option<String>,
    pub trigger_key: Option<String>,                                      // for P3 pitfall recall
    pub status: MemoryStatus,                                             // insert defaults to Candidate
    pub hit_count: i64,
    pub created_at: i64,                                                  // unix epoch ms
    pub updated_at: i64,
    pub last_hit_at: Option<i64>,
}

pub struct InsertMemoryInput<'a> { /* project_id, scope, kind, title, content,
                                       tags, source_session_id, trigger_key */ }

pub async fn insert_memory(pool: &SqlitePool, input: InsertMemoryInput<'_>)
    -> Result<i64, sqlx::Error>;                                          // returns new id; status fixed to Candidate
pub async fn search_memories_fts(pool, query, project_id, statuses, limit) -> Result<Vec<Memory>>;
pub async fn list_memories(pool, project_id, statuses, limit) -> Result<Vec<Memory>>;
pub async fn delete_memory(pool, id: i64) -> Result<(), sqlx::Error>;
pub async fn count_memories_for_session(pool, session_id: &str) -> Result<i64, sqlx::Error>;
pub async fn bump_hit_count(pool, id: i64) -> Result<(), sqlx::Error>;    // fire-and-forget on recall hit

// Recall-specific
pub enum RecallStatusFilter {
    P2Manual,                                                             // [Candidate, Active, Verified]
    P5Auto,                                                               // [Active, Verified] — P5+ only
}
pub async fn search_memories_fts_recall(pool, query, project_id, filter) -> Result<Vec<Memory>>;
```

```rust
// app/src-tauri/src/agent/memory_recall.rs
pub const RECALL_TOKEN_BUDGET: u32 = 500;

pub async fn build_recall_text(
    pool: &SqlitePool,
    query: &str,
    project_id: Option<&str>,
    filter: RecallStatusFilter,
) -> Result<Option<String>, sqlx::Error>;
// Returns None when query empty or no matches; otherwise
// newline-separated "<title>: <content>" entries, truncated
// at RECALL_TOKEN_BUDGET (count_tokens) — newer entries first
// (created_at DESC). Stable ordering = no cache thrash.

pub fn build_recall_block(recall_text: &str) -> ContentBlock;
// Wraps recall_text in <autonomous-memories>...</autonomous-memories>
// with NO cache_control — the breakpoint is on the first instruction
// block (build_instructions_blocks). Adding another cache_control
// here would shift the breakpoint and invalidate the Anthropic cache.
```

```rust
// app/src-tauri/src/tools/remember.rs
pub const REMEMBER_TOOL_NAME: &str = "remember";

// Permission: silent-allow (NO Tier 4 ask). Safety net lives in
// `insert_memory` (sensitive content regex + 500-char cap) +
// `count_memories_for_session` rate cap (default 50 per session).
// Per-turn cap (≤3) OUT OF SCOPE for P2; deferred to P5
// (requires ToolContext turn counter).
```

```typescript
// app/src/stores/memory.ts
interface AutonomousMemory {
  id: number;
  projectId: string | null;
  scope: 'user' | 'project';
  kind: 'preference' | 'fact' | 'decision' | 'pitfall' | 'skill' | 'other';
  title: string;
  content: string;
  tags: string[] | null;
  sourceSessionId: string | null;
  triggerKey: string | null;
  status: 'candidate' | 'active' | 'verified' | 'archived' | 'deleted';
  hitCount: number;
  createdAt: number;
  updatedAt: number;
  lastHitAt: number | null;
}

const runtimeMemories = ref<AutonomousMemory[]>([]);
const runtimeMemoriesLoading = ref(false);
const runtimeMemoriesError = ref<string | null>(null);

async function fetchMemories(): Promise<void>;
async function deleteMemory(id: number): Promise<void>;
```

```typescript
// Tauri commands (app/src-tauri/src/commands/memory.rs)
invoke<AutonomousMemory[]>('list_autonomous_memories', { projectId, statuses, limit })
invoke<void>('delete_autonomous_memory', { id })
```

### 3. Contracts

#### Two memory systems — DO NOT CONFUSE

| Property | B5 Static (V2 1 期, §Scenario 1 above) | Autonomous (V2 2 期, this section) |
|---|---|---|
| Storage | 4 fixed Markdown files (disk) | SQLite `autonomous_memories` table |
| Source | User / Project disk files | `remember` tool (LLM write) or `MemoryPreview` UI (user write) |
| Lifecycle | Read on session start, hot-reload via mtime | Per-turn FTS5 recall + LLM-initiated write |
| Injection | `build_instructions_blocks` → `messages[0]` synthetic | `memory_recall::build_recall_block` → appended to same `messages[0]` (P2) / before `tool_use` (P3) |
| Cache | `cache_control: Ephemeral` on first instruction block | **No** `cache_control` on recall block (preserves the instruction breakpoint) |
| LLM write surface | None (file-based, no LLM "write memory") | `remember` tool (silent-allow) |
| Promotion | N/A | P5 state machine: Candidate → Active → Verified |

#### DB schema (`autonomous_memories`)

| Column | Type | Notes |
|---|---|---|
| `id` | INTEGER PK AUTOINCREMENT | |
| `project_id` | TEXT NULL | NULL = user scope; FK `projects(id)` ON DELETE CASCADE for project-scope rows |
| `scope` | TEXT NOT NULL | `user` / `project` (denormalized for index efficiency) |
| `kind` | TEXT NOT NULL | one of `MemoryKind` |
| `title` | TEXT NOT NULL | ≤200 chars |
| `content` | TEXT NOT NULL | ≤500 chars (P2 safety net) |
| `tags` | TEXT NULL | JSON array of strings |
| `source_session_id` | TEXT NULL | session that wrote it (for audit + rate cap) |
| `trigger_key` | TEXT NULL | for P3 pitfall recall; P2 schema only, not consumed |
| `status` | TEXT NOT NULL DEFAULT 'candidate' | one of `MemoryStatus` |
| `hit_count` | INTEGER NOT NULL DEFAULT 0 | bumped on recall hit |
| `created_at` | INTEGER NOT NULL | unix epoch ms |
| `updated_at` | INTEGER NOT NULL | unix epoch ms |
| `last_hit_at` | INTEGER NULL | unix epoch ms |

Indexes: `(project_id, status)`, `(status, kind)`, FTS5 virtual table on `(content, title, tags)` with **trigram tokenizer** (per P1, supports substring + CJK). `trigger_key` is a UNIQUE NULL-distinct partial index `(project_id, trigger_key) WHERE trigger_key IS NOT NULL` (P3).

#### Recall injection contract (CRITICAL — cache-preserving)

`memory_recall::build_recall_block` is called **per turn** from `chat_loop.rs` after `build_instructions_blocks` and **before** `provider.send`. The block is **appended** to the same `messages[0]` synthetic user message — **NOT** a new message.

- **Query source**: most-recent user message text (`messages.iter().rev().find(User).to_text()`). Empty query → return `None` → no block added.
- **Filter**: P2 `RecallStatusFilter::P2Manual` (Candidate, Active, Verified). P5 narrows to `P5Auto`.
- **Order**: `created_at DESC` (newer first). P2 memories are all Candidate with `hit_count=0`, so `created_at` is the only meaningful sort. **Stable order is load-bearing** — reordering would re-tokenize the recall block and bust the cache.
- **Token cap**: `count_tokens` summed, truncate at `RECALL_TOKEN_BUDGET = 500`. Newer-first until budget exhausted.
- **First-line overflow**: when the first entry alone exceeds 500 tokens (defensive — P2 safety net caps content at 500 chars ≈ 200 tokens), surface it anyway.
- **`bump_hit_count`**: fire-and-forget on each recalled row. Failure is non-blocking (recall text already in prompt; stale hit_count is OK).
- **Empty / all-missing**: `None` → no block → no prompt noise.

#### `remember` tool contract (silent-allow + safety net)

`tools/remember::execute` does:
1. Parse input (`title`, `content`, `kind`, `scope`, `tags`, optional `trigger_key`).
2. **Safety net** (in `db::memories::insert_memory`; runs for both tool and UI paths):
   - Reject when `content` matches sensitive regex (API key / password / token patterns — see P1 spike-005 §4).
   - Reject when `content` > 500 chars.
3. **Rate cap** (in tool layer, per-call):
   - `count_memories_for_session(source_session_id) >= 50` → reject.
   - Per-turn cap (≤3) **OUT OF SCOPE for P2**; deferred to P5.
4. Insert with `status=Candidate`, `source_session_id=ctx.session_id`, `hit_count=0`, `created_at=now_ms()`, `updated_at=now_ms()`.
5. Return success + new id.

`scope=Project` requires a `project_id`; if missing → error. `scope=User` requires no `project_id`; if set → silently drop (user memory is global, project_id is not relevant).

#### Permission model (silent-allow, NOT Tier 4 ask)

`remember` is **silent-allow** — does NOT route through Tier 4 `permission_ask`. The LLM can write autonomous memory without user confirmation. Rationale (per spike-007 §5 + `06-29-autonomous-memory` ADR):

- The safety net (sensitive content regex + length cap) is the actual guard rail.
- Tier 4 `ask` would make the LLM silently never remember anything (the LLM would have to predict which writes the user will approve, defeating the purpose).
- "全自主写" is the epic-level decision; `remember` is its flagship tool.
- Other autonomous-write tools (future `auto_reflect`, P4 event-driven writes) follow the same silent-allow pattern.

For comparison, `write_file` / `edit_file` / `shell` (filesystem writes) **DO** route through Tier 4 `ask` — they are user-visible file mutations, not autonomous knowledge. The two permission classes are intentionally distinct.

#### Pre-tool pitfall recall contract (P3, layer 2 of 2) — 2026-06-29, 06-29-am-p3-tool-recall

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-p3-tool-recall.md`](./scenario-am-p3-tool-recall.md)。

#### Event-driven bypass reflection contract (P4, write side of the loop) — 2026-06-29, 06-29-am-p4-event-reflect

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-p4-event-reflection.md`](./scenario-am-p4-event-reflection.md)。

#### P5 quality-layer contract (verified soft-intercept + state-machine promotion + hygiene job) — 2026-06-29, 06-29-am-p5-quality

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-p5-quality-layer.md`](./scenario-am-p5-quality-layer.md)。

### 4. Validation & Error Matrix

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-validation.md`](./scenario-am-validation.md)。

### 5. Good / Base / Bad Cases

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-validation.md`](./scenario-am-validation.md)。

### 6. Tests Required

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-validation.md`](./scenario-am-validation.md)。

### 7. Wrong vs Correct

> **已拆出**(2026-09-13 doc-split):完整内容见 [`scenario-am-validation.md`](./scenario-am-validation.md)。

## Addendum: 2026-09-02 跨项目泄漏修复 + Settings 项目过滤 (settings-memory-project-filter)

> **Trigger**: 用户实测——在 jjh-mono 的记忆面板里看到 everlasting 的自主记忆。经 daemon HTTP 复现:`POST /api/v1/memory/list_autonomous_memories`(jjh-mono 的 project_id)返回 14 条,其中 13 条 project 行属于其它项目。

### 根因与契约修正(必须读)

`db/memories/crud.rs` 的 `list_memories` 旧 scope=None 分支**没有任何 WHERE**(裸 `SELECT ... ORDER BY created_at DESC`),而 `commands/memory.rs` 的调用方注释错误地声称"DB 层会按 project_id 过滤 project 行"——两层对 H2 语义的认知漂移,泄漏即漂移的产物。FTS 侧的 `search_memories_fts` 一直是对的,所以 **recall 路径从未泄漏,只有面板列表路径泄漏**。

修正后 `list_memories` 的 H2 语义与 `search_memories_fts` 完全对齐:

| 调用形状 | 语义 |
|---|---|
| `(Some(User), _)` | 仅 user 行(project_id 忽略) |
| `(Some(Project), None)` | **Err**(`ProjectScopeMissingId`) |
| `(Some(Project), Some(id))` | 仅该项目的行 |
| `(None, Some(id))` | user 行 + 该项目行(**面板视图,project-isolated**) |
| `(None, None)` | **全表——admin "全部项目" 视图,故意不过滤**;仅允许 `list_autonomous_memories(project_id=None)` 这一条命令路径调用,per-project 代码路径禁止 |

`list_autonomous_memories` 的 `project_id` 因此从 `String` 变为 `Option<String>`(Tauri command + daemon route 同步):`Some` 走存在性校验 + 隔离查询;`None` 是 Settings 项目过滤的显式 admin 视图。

### 前端过滤器(store: `runtimeProjectFilter`)

三态:`"current"`(默认,跟随 active project——MemoryModal / ProjectTabs 入口零行为变化)/ `"all"`(`{ projectId: null }`)/ 具体 project id(Settings 下拉 pin 定,免 `loadForProject` 直查)。`setRuntimeProjectFilter` 同值 set 是 no-op。UI 仅 Settings MemoryTab 传 `project-filterable` 挂 reka-ui Select(遵循 2026-08-28 "settings 下拉走 reka"决策);全部项目视图给 project 行加属主项目名徽章;scope=project 徽章补 accent 配色(此前 user/project 徽章无任何样式差异,只剩文字可辨)。

### Gotcha(防止复发)

- **改 list/FTS 共享语义时,两个函数必须一起改**——它们各自手写 SQL,没有共享的 WHERE builder。新增 scope 形状时先 grep 两处。
- **command 层注释不是契约**——本次泄漏的调用方注释言之凿凿"这正是 project-isolation contract",但 DB 层不兑现。隔离语义的权威在 `crud.rs`/`search.rs` 的 SQL + `memories_tests/` 的隔离测试,不在注释。
- 隔离回归锚点:`list_memories_project_isolation`(对照 FTS 侧的 `search_memories_fts_project_isolation`)。

## Addendum 2: 2026-09-02 pitfall 触发召回跨项目修复(同日第二起泄漏)

> **Trigger**: 第一起草单修完后复查另一条召回路径发现——P3/P5 pre-tool pitfall recall(`find_pitfalls_by_trigger` / `find_pitfalls_by_trigger_all_status`)的 SQL **只有 `tool_name` + `kind='pitfall'` + `status` 过滤,完全没有 scope/project_id 条件**,函数签名里也没有 project_id 参数。project-scope pitfall(如 P4 反思产物)在任何项目的同名工具调用时都会命中。

**更正 Addendum 1 的一个错误结论**:"recall 路径从未泄漏"只对 **FTS 召回**成立;pitfall 触发召回一直跨项目泄漏。自愈教训:recall 有两份手写 SQL,当时只核对了 FTS 那份。

### 修正后的召回隔离契约(全路径)

| 召回路径 | 过滤 | 参数来源 |
|---|---|---|
| P2 会话启动 FTS(`search_memories_fts`, scope=None) | `scope='user' OR (scope='project' AND project_id=?)`(H2 (c)) | session 的 project_id |
| P3/P5 pre-tool pitfall(`find_pitfalls_by_trigger(_all_status)`) | **同上(2026-09-02 新增)**;`project_id: &str` 必填,无 admin 形状 | chat_loop `current_ctx.project_id`(并行/串行两个调用点) |

语义:project 级 pitfall 只在本项目命中;user 级全局命中(与 H2 写入语义对齐)。跨项目命中还会 bump hit_count 污染 P5 晋升输入,修复后一并消除。

### Gotcha

- **签名即契约**:`project_id` 设为必填 `&str`(不是 Option)——pitfall 召回天然 per-session,不存在"查全部"的合法场景;要加 admin 形状必须显式另开函数,不能顺手加 None 分支。
- 隔离回归锚点(DB 层)`find_pitfalls_project_isolation` + (seam 层)`recall_pitfall_project_isolation`,与 `list_memories_project_isolation` / `search_memories_fts_project_isolation` 构成四点矩阵。
