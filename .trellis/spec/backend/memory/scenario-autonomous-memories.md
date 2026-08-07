## Scenario: Autonomous Memories (DB-backed Runtime Memory, V2 2 期)

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

> **Layer 2** = 工具执行前召回(spike-007 §4)。**与 layer 1(P2 session-start FTS5)是两套独立检索**:
> - Layer 1:per-turn FTS5(query = most-recent user message text,模糊召回多种 memory kind)
> - Layer 2:per-tool `trigger_key` 精确匹配(只召回 `kind = 'pitfall'` + `status = 'active'`)
>
> Layer 2 **不**走 FTS5,**不**消费 layer 1 的 query 文本,**不**产出新 message 块 — 它产出的是一个 plain-text 注脚,prepend 到 `tool_result.content`。

**Signatures**(已在 `db/memories.rs:1046` 由 P1 产出):

```rust
// db::memories::find_pitfalls_by_trigger
pub async fn find_pitfalls_by_trigger(
    pool: &SqlitePool,
    tool_name: &str,
    command_pattern: Option<&str>,   // shell 命令字符串片段(精确匹配)
    path: Option<&str>,              // 文件路径(精确匹配)
) -> Result<Vec<AutonomousMemoryRow>>

// agent::permissions::recall_pitfall_footnote (P3 新增)
pub async fn recall_pitfall_footnote(
    pool: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,  // 完整 tool_input
) -> Result<Option<String>, sqlx::Error>  // 命中 → Some("⚠️ Memory: ...") / 不命中 → None
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 触发时机 | `chat_loop` 拿到 `Decision::Allow` 之后、`execute_tool` 之前 | 不在 `permissions::check()` 内部(见 permission-layer.md §4.2) |
| 召回对象 | `kind = 'pitfall'` AND `status IN (candidate, active, verified)` | P3 落地时 active-only;**P5 放宽**到三态分档(`recall_pitfall`):verified+`is_full_match`→SoftBlock,active/candidate→Footnote。见 P5 contract |
| 匹配方式 | `find_pitfalls_by_trigger` 的 `tool_name` + `command_pattern` / `path` **精确匹配** | 命中 `idx_am_pitfall` 索引(migration.rs:756);O(1) 不是 O(n) |
| 注脚格式 | `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n• [title] content\n...` | imperative 强提示;多命中时多行 bullets |
| 注入位置 | `tool_result.content` 前缀(plain text),**envelope wrap 之前** | `tool_use_id` 配对 / `is_error` 语义 / envelope `{result, cwd}` shape 全部不变 |
| `bump_hit_count` 时机 | 命中后 fire-and-forget(`tokio::spawn`) | 不阻塞 recall 步骤;P5 状态机读取 `hit_count` 决定晋升 |
| 召回失败 | `Err(sqlx::Error)` → `tracing::warn!` + 返回 `None` | 工具照常执行(降级放行);**永不阻断工具执行** |
| Decision 语义 | **不参与**决策链,`check()` 仍返回 `Decision::Allow` | 注脚是 hint,不是 gate |

**为什么 layer 2 是 `trigger_key` 精确匹配而非 FTS5**:
- 工具执行前的"我要不要做这个"是 yes/no 决定,精确率优先(漏一条能用 layer 1 补,注入一条错的污染工具输出)
- FTS5 bm25 在 trigger_key 字段上召回会引入与本工具无关的 pitfall(噪音)
- `command_pattern` + `path` 双键命中让"同类操作"语义无歧义

#### Event-driven bypass reflection contract (P4, write side of the loop) — 2026-06-29, 06-29-am-p4-event-reflect

> **P4 是 P3 的写入对偶**。spike-007 §3 路径2(spike-007 §6 接入点 C)定义的"连续 ≥2 次同名工具失败后成功 → 旁路 LLM reflection → 自动产出 pitfall(active)"。P3 是"读"(工具执行前召回已有 pitfall),P4 是"写"(事件驱动把新 pitfall 写库)。P3 + P4 闭合完整自动闭环:踩坑 → 记住 → 下次规避。
>
> P4 **不**改 `permissions::check()` 内部,**不**改 P3 的 pre-execute seam,**不**改 `ToolResultPayload` shape — 它在 chat_loop 的 **post-execute seam**(与 P3 的 pre-execute seam 互补)读 `ToolResultPayload.is_error` 信号。

**Signatures**(在 `app/src-tauri/src/agent/auto_reflect.rs`):

```rust
// agent::auto_reflect::FailureTracker — per-session 状态机
pub struct FailureTracker {
    // (tool_name) -> TrackerEntry { consecutive_failures, last_failure_input,
    //                                last_failure_content, last_failure_path }
    inner: Mutex<HashMap<String, TrackerEntry>>,
}

pub const REFLECTION_FAILURE_THRESHOLD: usize = 2;

impl FailureTracker {
    pub fn new() -> Self;
    pub fn try_record_outcome(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        content: &str,
        is_error: bool,
    ) -> Option<ReflectionTrigger>;
    // Some(_)= 触发 reflection(call site 调 tokio::spawn 跑 reflect_to_pitfall);
    // None  = 不触发(失败计数 0/1,或 success 但无前置 ≥2 失败)
}

// agent::auto_reflect::try_record_outcome (public entry,chat_loop 调用)
pub fn try_record_outcome(
    tracker: &Arc<Mutex<FailureTracker>>,
    request_id: &str,
    session_id: &str,
    project_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    content: &str,
    is_error: bool,
);

// agent::auto_reflect::reflect_to_pitfall (private,fire-and-forget 内核)
async fn reflect_to_pitfall(
    request_id: &str,
    session_id: &str,
    project_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    failure_content: &str,
    success_content: &str,
    provider: Arc<dyn Provider>,
    pool: SqlitePool,
);
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 触发时机 | `chat_loop` 的 `execute_tool()` 返回之后、audit 写之前 | `!token.is_cancelled()` 守卫(与 RULE-A-004 audit-skip 对齐) |
| 触发信号 | 同一 `tool_name` 连续 `REFLECTION_FAILURE_THRESHOLD = 2` 次 `is_error=true` **之后**的 `is_error=false` | 单次失败不触发(PRD AC #3);计数器在成功或触发后重置 |
| 状态机存储 | `Arc<Mutex<HashMap<tool_name, TrackerEntry>>>` 内嵌于 `run_chat_loop` 局部,per-session 内存 | **不**跨 session 持久化(v1 接受 session 边界重置,v2 扩展位 spike-007 §10) |
| 调用点 | `chat_loop.rs` parallel-batch L2 path + serial path,两处(seam 与 P3 镜像) | 共享同一 `failure_tracker` 句柄 |
| Reflection LLM 调 | 走主 provider 同一实例(不另起);独立 `REFLECT_SYSTEM_PROMPT` + `REFLECT_USER_TEMPLATE`;**不**消费主 system prompt / 不消费消息历史 | 1 个 user message 含"失败+成功 transcript 片段";空 `tools` 数组;`max_tokens=512` |
| Reflection 期望产出 | JSON `{title, content, trigger_key: {tool, command_pattern, path_globs}}` | markdown 代码围栏剥离;JSON parse 失败 → `warn!` + 丢弃 |
| 写库参数 | `kind=Pitfall, status=Active, scope=Project, source_session_id, source_ref=<request_id>:<tool_name>` | 走 P1 `insert_memory` 复用安全网(敏感过滤 / 长度 / 敏感路径 / frequency cap 50/session)|
| 触发阈值 | `consecutive_failures >= 2`(常量 `REFLECTION_FAILURE_THRESHOLD`) | PRD AC #3:单次失败不触发 |
| Fire-and-forget | `tokio::spawn` 整段 reflection | **不** await 主 loop;失败一律 `tracing::warn!` + 静默吞;**不** panic / `unwrap()` / `expect()` |
| Decision 语义 | **不参与** `permissions::check()` 决策链 | P4 不在 P3 的 pre-execute seam,也不在 5-tier 内部 |
| ToolResultPayload 污染 | **无** — P4 是 read-only consumer,只读 `is_error` / `content` / `tool_input` | 协议 `tool_use_id` 配对 / `is_error` 语义 / envelope `{result, cwd}` 全部不变 |

**为什么 P4 写在 post-execute 而非 pre-execute(P3 seam)**:
- P3 是"工具执行前查已知 pitfall" — 写发生在 pre-execute 之前;**读**则在 P3 的 pre-execute seam
- P4 是"工具执行完记录新经验" — 需要看到 `is_error` 真实结果(成功/失败)才能决策,**写**发生在 post-execute 之后
- 两个 seam 是 sibling,不互相依赖(顺序独立:同一个 tool_use_id P3 在前 P4 在后,中间夹 `execute_tool` + audit)

**为什么 P4 走 P1 `insert_memory` 而非自写 INSERT**:
- 写入安全网(sensitive regex / 长度 cap / 敏感路径 deny-list / frequency cap 50/session)单源;旁路绕过 P1 安全网会引入敏感泄漏 / 库膨胀 / 路径泄漏
- 状态机字段(`hit_count` / `last_used_at` / `demoted_reason`)由 P1 维护,P5 消费;旁路 INSERT 会破坏 P5 状态机读取
- 复用 `MemoryKind::Pitfall` 枚举 + `MemoryStatus::Active` 强类型,P4 写时直接用 `MemoryInput { kind, status, ... }`

**P3 ↔ P4 闭环**(P4 单元测试 `reflected_pitfall_is_recallable_by_p3_helper` 锁定):
1. session A:同 `tool_name='shell'` 连续 2 次 `cargo test --no-default-features` 失败,后 1 次 `cargo test` 成功
2. P4 状态机触发 → `tokio::spawn` reflection → 调 LLM 提炼 `{title: "WSL cargo test 需显式 features", content: "...", trigger_key: {tool: "shell", command_pattern: "cargo test", path_globs: null}}` → 写 `insert_memory(kind=pitfall, status=active)`
3. session B:agent 跑 `cargo test` → P3 pre-execute seam 调 `find_pitfalls_by_trigger('shell', Some("cargo test"), None)` → 命中 session A 写的 pitfall → 注脚 prepend 到 tool_result → agent 看到 "⚠️ Memory: ..." 提示 → 第一次执行就规避

**Reflection prompt 模板**(独立常量,`auto_reflect.rs` 内部):

```text
// REFLECT_SYSTEM_PROMPT
"你是一个经验提炼助手。给定一个工具调用连续失败的 transcript + 后续成功的 transcript,
提炼成一句 200 字内的'可复用经验'。输出严格 JSON,字段:
  title: 短标题(≤30 字符)
  content: 一句可复用的踩坑经验(≤200 字符,imperative 语气)
  trigger_key: 结构化触发键,字段:
    tool: 工具名(shell/edit_file/grep/read_file/...)
    command_pattern: 触发命令模式(可空)
    path_globs: 触发路径 glob 列表,可空(null = 不限路径)
**只输出 JSON**,不要 markdown 包装,不要解释。"

// REFLECT_USER_TEMPLATE
"<failure>
  tool: {tool_name}
  input: {tool_input_json}
  error: {failure_content_truncated_2kib}
</failure>
<success>
  tool: {tool_name}
  input: {tool_input_json}
  output: {success_content_truncated_2kib}
</success>
请提炼上述失败→成功经验。"
```

#### P5 quality-layer contract (verified soft-intercept + state-machine promotion + hygiene job) — 2026-06-29, 06-29-am-p5-quality

> **P5 是 P3 的质量收口层**(spike-007 §9 步 6 + §3 状态机 + §4 软拦截分档)。三块:(1) verified pitfall 软拦截重判(动 loop,兑现"第一时间规避");(2) 状态机自动晋升 candidate→active→verified(靠 hit_count + 存续时长);(3) 异步卫生 job(dedup/降权)。
>
> **P5 不改** P3/P4 的 seam 位置,**不改** `permissions::check()` 内部,**不改** `ToolResultPayload` shape —— 它把 P3 的 `recall_pitfall_footnote`(返 `Option<String>`)升级为分档 `recall_pitfall`(返 `PitfallRecall` enum),并在 chat_loop 两 path 的 pre-execute seam 加 SoftBlock 短路。

**Signatures**(`agent/permissions/check.rs` + `db/memories.rs` + `agent/memory_hygiene.rs`):

```rust
// agent::permissions::check — P5 分档
pub enum PitfallRecall {
    None,
    Footnote(String),                              // active / candidate / 二次命中(同 P3 注脚)
    SoftBlock { hint: String, memory_id: String }, // verified + is_full_match + 本 session 未拦过
}
pub const PITFALL_SOFT_BLOCK_ENABLED: bool = true;  // feature flag;false → 退回 P3 纯注脚
pub async fn recall_pitfall(
    db: &SqlitePool, tool_name: &str, tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,             // session 级 D1 防循环记账
) -> PitfallRecall;  // DB err → warn! + None(不阻断)

// db::memories — P5 状态机(嵌 bump_hit_count 同连接,避免 bump↔promote 竞态)
pub async fn promote_if_eligible(pool: &SqlitePool, memory_id: &str) -> Result<(), sqlx::Error>;
pub async fn update_status(pool, memory_id, new_status: MemoryStatus, demoted_reason: Option<&str>)
    -> Result<(), StatusTransitionError>;          // 事务 + 转换矩阵校验
pub async fn find_pitfalls_by_trigger_all_status(...) -> ...;  // P5 放宽版(含 candidate/active/verified)
pub async fn count_memories_by_scope_kind(pool, scope, kind) -> i64;  // 卫生 job 触发计数

// agent::memory_hygiene — P5 卫生 job
pub fn char_trigrams(s: &str) -> HashSet<String>;  // Unicode char(中文友好,非 byte)
pub fn jaccard(a: &str, b: &str) -> f32;            // char-trigram 集合 Jaccard 0.0..=1.0
pub async fn run_hygiene_pass(pool: SqlitePool);    // dedup_pass + age_out_pass,fire-and-forget
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 软拦截触发 | `status='verified'` AND `is_full_match` AND `memory_id ∉ already_blocked` AND `PITFALL_SOFT_BLOCK_ENABLED` | 首条命中 row 胜出 SoftBlock;其余进 Footnote |
| SoftBlock 回合改法 | **不调** `execute_tool`、**不写** `tool_executed` audit、构造 `ToolResult{content:hint, is_error:false}`、`emit_tool_result`、记 `memory_id` 入 set、`bump_hit_count` | 复用 `Decision::Deny` 的"不执行+回填"模式;下一轮 send LLM 重判 |
| `is_error=false` | 提示非错误 | 避免 LLM 误判"工具坏了"换工具;语义是经验提示,不是错误 |
| D1 死循环防护 | 每条 pitfall 每 session 软拦截 **1 次**;同坑二次(`already_blocked` 含)→ 降级 Footnote + 正常 execute | 保证不卡到 `MAX_TURNS`(50);`already_blocked` = session 级 `Arc<Mutex<HashSet<String>>>`,loop 顶部建(同 `FailureTracker` 生命周期) |
| `is_full_match` 语义 | 行上每个 `Some(_)` 字段(command_pattern 子串 + path_globs glob)都匹配,且至少一个 `Some` | ⚠️ 偏离 design 字面"三者皆中"——内置工具探针对称(Shell 无 path 探针、Path 工具无 command_pattern),字面不可行;宽泛 pitfall(皆 None)→ 永不 SoftBlock,降级 Footnote(比字面更保守) |
| 晋升 candidate→active | `hit_count ≥ 2`(被召回命中过 2 次) | `promote_if_eligible` 嵌 `bump_hit_count` 同连接读回 hit_count + `update_status` |
| 晋升 active→verified | `hit_count ≥ 5` AND `created_at` 距今 ≥ 3 天 | "存续时长"代理"未翻车"(v1 无跨 session 翻车信号,P4 `FailureTracker` 是 session 内) |
| 非法转换 | `update_status` 矩阵拒绝 → `StatusTransitionError::Illegal` | 合法集:candidate→{active,verified,demoted}, active→{verified,demoted}, verified→demoted, demoted→active |
| recall filter 方向 | session-start FTS **保持** `IncludeCandidate`;pre-tool **放宽** candidate+active+verified 分档 | ⚠️ P5 推翻 P2 注释"收紧到 ActiveVerifiedOnly"——收紧会掐断 candidate 晋升(candidate 靠被召回命中晋升,排除则永不命中) |
| 卫生 dedup | 同 `(scope,kind)`:pitfall 按 trigger_key 全等;其余按 char-trigram Jaccard >0.7 | 合并保留高 confidence/高 hit_count,`delete_memory` 删冗余 |
| 卫生 age-out | `status IN (candidate,active)` AND age(`last_used_at`‖`created_at`)>30 天 AND `hit_count<2` → `Demoted("aged_out")` | verified 豁免(已证明价值) |
| 卫生 job 触发 | `insert_memory` 后 `(scope,kind)` 计数 `%10==0` → spawn;app 启动(`lib.rs` setup)spawn 一次 | `if !cfg!(test)` 守卫防测试 flaky;fire-and-forget,失败 warn! 吞 |
| Decision 语义 | **不参与** `check()` 决策链 | SoftBlock 在 Allow 之后、execute 之前短路,非 Deny |

**P3 ↔ P4 ↔ P5 三层闭环**(集成测试 `agent_loop_p5_soft_block_short_circuits_execute` + `agent_loop_p5_soft_block_second_hit_degrades_to_execute` 锁定):
1. session A:P4 旁路 reflection 写 `active` pitfall(带 trigger_key)
2. session B+:P3/P5 pre-tool recall 命中 → bump hit_count → `promote_if_eligible` 晋升 active→verified(多次命中 + 存续 3 天)
3. session N:verified + `is_full_match` → P5 SoftBlock 短路 execute → LLM 重判(第一时间规避);同坑二次命中 → Footnote + 正常执行

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `remember` with sensitive content (API key regex hit) | `insert_memory` returns `Err`; tool surfaces "rejected: sensitive content detected" |
| `remember` with content > 500 chars | `insert_memory` returns `Err`; tool surfaces "rejected: content exceeds 500 char cap" |
| `remember` with `scope=Project` but no `project_id` in context | `Err("project_id required for project scope")` |
| `remember` with `count_memories_for_session >= 50` | Tool returns "rejected: per-session cap of 50 memories reached" |
| FTS5 query empty | `build_recall_text` returns `None`; no block added; no error |
| FTS5 query non-empty, 0 matches | `build_recall_text` returns `None`; no block added; no error |
| FTS5 query N matches, sum > 500 tokens | Truncate at line boundary; newer first until budget exhausted |
| FTS5 query N matches, first entry alone > 500 tokens | Surface first entry anyway (defensive; should not happen with P2 content cap) |
| `bump_hit_count` fails (DB transient error) | `warn!`; recall text already in prompt; non-blocking |
| `delete_memory` with id not found | 0 rows affected; idempotent success |
| `delete_memory` for `status=Active`/`Verified` row (P5+) | P2 allows deletion of any status; P5+ may restrict to Candidate/Archived |
| Frontend `fetchMemories` IPC failure | `runtimeMemoriesError` set; UI shows error state |
| Frontend `deleteMemory` IPC failure | Toast / inline error; optimistic remove rolled back |
| **P3** `recall_pitfall_footnote` with `tool_name` no match in DB | Returns `Ok(None)`; no footnote; tool executes normally |
| **P3** `recall_pitfall_footnote` with active pitfall, `command_pattern` matches | Returns `Ok(Some("⚠️ Memory: ..."))`; prepended to `tool_result.content`; `bump_hit_count` fired |
| **P3** `recall_pitfall_footnote` with verified-status pitfall | Returns `Ok(None)` — `verified` is **P5 scope** (soft-intercept), P3 active-only filter strictly excludes |
| **P3** `recall_pitfall_footnote` with candidate-status pitfall | Returns `Ok(None)` — `candidate` is **P2 scope**; not yet promoted to recallable |
| **P3** `recall_pitfall_footnote` SQL `Err(sqlx::Error)` | `tracing::warn!` + `Ok(None)`; tool executes normally; **never blocks** (PRD hard rule) |
| **P3** `bump_hit_count` for pre-tool hit fails (fire-and-forget) | `warn!`; recall footnote already in tool_result; non-blocking; P5 state machine may read stale `hit_count` (acceptable) |
| **P4** `try_record_outcome` with single `is_error=true` (no prior failure for this `tool_name`) | Tracker increments to 1, no reflection triggered; subsequent success resets to 0 (PRD AC #3) |
| **P4** `try_record_outcome` with 2 consecutive `is_error=true` followed by `is_error=false` for same `tool_name` | Reflection triggered; `tokio::spawn` runs `reflect_to_pitfall`; tracker resets to 0; main loop continues immediately (PRD AC #1, #2) |
| **P4** `try_record_outcome` with success as the first event for a `tool_name` (no prior failures) | Tracker stays at 0; no reflection triggered (success is a no-op for trigger detection) |
| **P4** `try_record_outcome` with 1 failure followed by 1 success for same `tool_name` | Tracker increments to 1 on failure, resets to 0 on success (no trigger — below threshold) (PRD AC #3) |
| **P4** `try_record_outcome` per-tool isolation | `shell` failures do NOT increment `edit_file` counter; each `tool_name` has its own `TrackerEntry` |
| **P4** reflection LLM call returns non-JSON text (markdown wrapper, prose) | `strip_code_fence` + JSON parse; parse failure → `tracing::warn!` + silent drop; main loop unaffected (no panic, no `unwrap`) |
| **P4** `reflect_to_pitfall` calls `insert_memory` with sensitive content (LLM hallucinates an API key) | `insert_memory` safety net rejects (returns `Err`); `tracing::warn!` + silent drop; no row written |
| **P4** `reflect_to_pitfall` calls `insert_memory` with content > 500 chars (LLM verbose) | `insert_memory` safety net rejects; `tracing::warn!` + silent drop; no row written |
| **P4** `reflect_to_pitfall` calls `insert_memory` with `count_memories_for_session >= 50` | `insert_memory` frequency cap rejects; `tracing::warn!` + silent drop; no row written |
| **P4** reflection LLM call transient network/timeout error | `tracing::warn!` + silent drop; no retry; main loop unaffected (fire-and-forget hard rule) |
| **P4** `insert_memory` returns `Err(sqlx::Error)` (DB transient) | `tracing::warn!` + silent drop; no retry; main loop unaffected |
| **P4** P3 ↔ P4 close-the-loop | P4 writes pitfall via `insert_memory` → immediately recallable by P3's `find_pitfalls_by_trigger` (no extra index/migration; `idx_am_pitfall` already covers P4 writes) |
| **P5** verified pitfall + `is_full_match` + `memory_id ∉ already_blocked` | `recall_pitfall` 返 `SoftBlock`;chat_loop 短路 `execute_tool`、回灌 `is_error=false` 提示、记 `memory_id` 入 session set、`bump_hit_count` |
| **P5** 同坑二次命中(`already_blocked` 已含) | 降级 `Footnote` + 正常 `execute_tool`(D1 防循环,不卡 `MAX_TURNS`) |
| **P5** candidate `hit_count` 跨 2 | `promote_if_eligible` 升 active;active `hit_count` 跨 5 且 age≥3 天升 verified(嵌 `bump_hit_count` 同连接) |
| **P5** 宽泛 pitfall(command_pattern/path_globs 皆 None) | `is_full_match=false` → 永不 SoftBlock,降级 Footnote(比 design 字面更保守) |
| **P5** `PITFALL_SOFT_BLOCK_ENABLED=false` | `recall_pitfall` 永不返 SoftBlock,退回 P3 纯注脚(feature flag 回滚) |
| **P5** 卫生 job:同 `(scope,kind)` Jaccard>0.7 / trigger_key 全等 | 合并保留高 confidence/hit_count,`delete_memory` 删冗余;age>30天 且 hit<2 → `Demoted("aged_out")` |

### 5. Good / Base / Bad Cases

#### Good: full loop

1. Session 1 — user: "I prefer tabs over spaces". LLM calls `remember(title="pref-tabs", kind=Preference)`. Row inserted with `status=Candidate`, `source_session_id="s1"`.
2. Session 2 — user: "format this code". `build_recall_text("format this code", ...)` FTS5-hits the tabs preference. `build_recall_block` wraps in `<autonomous-memories>...</autonomous-memories>`, appended to instructions in the same `messages[0]` synthetic user message. LLM sees the preference, recommends tabs.

#### Base: fresh install

No memories. `build_recall_text` returns `None` for any query. No recall block. Prompt = base system prompt + instruction blocks only.

#### Bad: separate user message for recall

```rust
// BAD — recall as messages[1]
if let Some(text) = build_recall_text(...).await? {
    messages.push(synthetic_user_message(text));  // new message
}
provider.send(messages).await;
```

New user message shifts the Anthropic cache breakpoint → 5-10× cost on every turn. The instructions (4 files) are no longer the cache anchor. The recall block must **append** to `messages[0]`, not insert at index 1.

#### Bad: `cache_control` on recall block

```rust
// BAD — adding cache_control to the recall block
ContentBlock::Text { text: recall_text, cache_control: Some(Ephemeral) }
```

Anthropic's rule: "the last cache_control block is the breakpoint". Adding a second `cache_control: Ephemeral` shifts the breakpoint to the recall block, demoting the instruction files from cache anchor to plain text. The instruction block already carries the cache_control marker; recall blocks do not.

#### Bad: Tier 4 ask on `remember`

```rust
// BAD — treat remember like any other write tool
PermissionContext::new(...).with_ask(Tier4Ask::Write).check(REMEMBER_TOOL_NAME)?;
```

LLM either silently abstains (most common — predictive abstention) or interrupts the user 50 times per session. The whole point of autonomous memory is that the LLM writes it; the safety net is the actual guard.

#### Bad: 收紧 recall filter 到 ActiveVerifiedOnly(P5 推翻的预测)

1. P5 设计时曾预期"状态机落地后,session-start recall 收紧到 `RecallStatusFilter::ActiveVerifiedOnly`"(P2 注释 + 本节早期版本)。
2. 实现时发现**收紧会掐断 candidate 晋升路径**:candidate→active 的唯一 v1 触发是"被召回命中"(recall 命中 → `bump_hit_count` → 达 D2 阈值晋升);把 candidate 排除出 recall 则它永不命中、永不晋升(preference/fact 类无 `trigger_key`,只靠 FTS 召回,断路尤甚)。
3. **P5 实际决策**:session-start recall **保持 `IncludeCandidate`**;噪音靠"低阈值快速晋升"(candidate 命中 ≥2 即升 active,流出 candidate 池)+ 卫生 job age-out 控制,**不靠 filter 收紧**。

`RecallStatusFilter` 枚举仍是 load-bearing contract;P5 的教训是 —— **filter 收紧与"靠召回命中晋升"的状态机互斥**:设计晋升路径时,必须确认 recall filter 覆盖所有待晋升状态,否则状态机死锁。见 P5 contract "recall filter 方向"行。

#### Bad: pre-tool recall inside `permissions::check()` (P3 anti-pattern)

1. P3 lands with a Tier 1 hook that calls `recall_pitfall_footnote` from inside `check()`.
2. `check()` becomes a function that both *decides* (5-tier) and *recalls* (DB read) — mixed responsibilities.
3. Tooling that mocks `check()` (e.g. `permissions::tests_check.rs`) now has to mock the pool too, blowing up the test surface.
4. If recall fails, it now pollutes the `Decision` return — was previously a clean `Decision::Allow`, now it's `Result<Decision, ...>`.

The recall is **information injection**, not a *decision*. It lives at the chat_loop seam (check → execute), not inside `check()`. See [permission-layer.md §4.2](./permission-layer.md#42-tier-1-hooks-实际实现路径--p3-工具执行前召回2026-06-29-06-29-am-p3-tool-recall).

#### Bad: implementing verified soft-intercept in P3

1. P3 ships with verified-status pitfall hard blocking the tool (returning `Decision::Deny`).
2. P5 lands later wanting a "soft" intercept (return `Decision::Allow` + structured hint to LLM).
3. The P3 hard-block path is now dead code; the seam is in the wrong place.

P3 is **active-only footnote**, period. Verified soft-intercept is **P5 scope** (spike-007 §4, 命中分档表). The function `recall_pitfall_footnote` returns `Result<Option<String>, sqlx::Error>` because the recall result is a **hint, not a decision**. **P5 落地(2026-06-29)**:verified soft-intercept 用 `PitfallRecall` enum(`None` / `Footnote` / `SoftBlock`)+ `recall_pitfall` 分档函数 —— **不返 `Decision`**(SoftBlock 在 chat_loop seam 短路 `execute_tool`,不是权限 Deny);`recall_pitfall_footnote` 保留为 Footnote 档基础。早期"P5 加 sibling `verified_pitfall_decision` 返 `Decision`"的预测未采纳(enum 比 sibling 更统一)。见 P5 contract。

#### Bad: P4 reflection awaits the main loop (P4 anti-pattern)

1. P4 lands with `try_record_outcome` returning a `Future` that the main loop `await`s.
2. Main loop blocks while the LLM reflection runs (typically 1-5s).
3. The "fire-and-forget" guarantee is lost — the user's tool result is delayed by reflection latency.
4. If the LLM call hangs (network issue), the main loop hangs.

P4's reflection is **fire-and-forget**: `tokio::spawn` wraps the entire `reflect_to_pitfall` call, the spawned `JoinHandle` is dropped (not `.await`ed), and any failure is absorbed at `tracing::warn!`. The main loop sees the original `is_error` / `content` signal and continues immediately. See [agent-loop-architecture.md front-matter "Per-tool auto-reflect seam (P4)"](./agent-loop-architecture.md#).

#### Bad: P4 bypasses P1's `insert_memory` and writes a raw `INSERT` (P4 anti-pattern)

1. P4 lands with a direct `sqlx::query("INSERT INTO autonomous_memories ...")` to avoid P1's `MemoryInput` struct.
2. P1's safety net (sensitive regex / length cap / 敏感路径 deny-list) is bypassed.
3. A hallucinated API key or local `/home/user/.ssh/...` path leaks into the autonomous memory table.
4. The 50/session frequency cap is bypassed — the agent self-poisons its own memory library.

P4's reflection **must** route through P1's `insert_memory` (`MemoryInput { kind: Pitfall, status: Active, scope: Project, ... }`) so the safety net, the state-machine fields (`hit_count` / `last_used_at` / `demoted_reason`), and the type enums (`MemoryKind` / `MemoryStatus` / `MemoryScope`) all flow through the single source of truth. There is no second write path.

#### Bad: P4 fires on every tool failure (P4 anti-pattern)

1. P4 ships with `REFLECTION_FAILURE_THRESHOLD = 1`.
2. Every single `is_error=true` triggers an LLM reflection.
3. The agent over-writes its memory library with shallow "this command failed" entries.
4. Cost + latency explodes; the precision-first P3 recall filter drowns in noise.

P4's threshold is **2 consecutive failures followed by a success** (per spike-007 §3 路径2 contract). Single failures are absorbed. The success-after-threshold pattern is the actual signal of "the agent tried X, failed twice, then found a working approach Y" — which is the only signal worth remembering. See [P4 contracts table — 触发阈值].

### 6. Tests Required

| Test | Asserts |
|---|---|
| `insert_memory_roundtrip` | Insert + read returns same fields; `status=Candidate`, `hit_count=0` |
| `insert_memory_rejects_sensitive_content` | API-key-shaped content → `Err`; no row inserted |
| `insert_memory_rejects_oversize_content` | >500 chars → `Err`; no row inserted |
| `search_memories_fts_finds_title` | Insert "tabs over spaces" + search "prefer tabs" → hit |
| `search_memories_fts_finds_content` | Same with content-only keyword |
| `search_memories_fts_trigram_supports_substring` | Insert "everlasting" + search "lastin" → hit (trigram, not just prefix) |
| `search_memories_fts_filters_by_status` | Insert Candidate + Active, filter Candidate only → 1 row |
| `list_memories_orders_by_created_at_desc` | Insert 3 rows with distinct timestamps → first is newest |
| `delete_memory_removes_row` | Insert + delete + list → 0 rows |
| `count_memories_for_session_returns_count` | Insert 2 in same session → 2 |
| `bump_hit_count_increments` | Insert + bump + read → `hit_count=1` |
| `build_recall_text_returns_none_for_empty_query` | `""` → `None` |
| `build_recall_text_returns_none_when_no_matches` | Non-matching query → `None` |
| `build_recall_text_surfaces_candidate_match` | 1 Candidate + search → text contains title+content |
| `build_recall_text_truncates_at_token_budget` | N rows summing >500 tokens → truncated; newer first |
| `build_recall_block_has_no_cache_control` | Returned `ContentBlock::Text.cache_control == None` |
| `inject_recall_appends_to_instruction_message_blocks` | Existing instruction message + recall block → `blocks.len()` grows by 1; cache_control on block 0 unchanged |
| `tools_remember_execute_writes_candidate_roundtrip` | Tool call → row with `status=Candidate`, `source_session_id=ctx.session_id` |
| `tools_remember_execute_rejects_sensitive_content` | Tool call with API-key content → `Err` |
| `tools_remember_execute_rejects_when_session_cap_reached` | Pre-seed 50 rows for session → 51st call → `Err` |
| `tools_remember_execute_no_turn_cap_p2` | 4 `remember` calls in same turn (P2) → all succeed (deferred to P5) |
| `commands_list_autonomous_memories_returns_runtime_list` | Insert 2 + invoke Tauri command → 2 rows in response |
| `commands_delete_autonomous_memory_removes_row` | Insert + invoke Tauri command → row gone |
| `commands_delete_autonomous_memory_project_isolation` | Insert in A, delete from B → row not deleted (404 / no-op) |
| `store_fetch_memories_happy_path` | Mock IPC → `runtimeMemories` populated |
| `store_fetch_memories_error_path` | Mock IPC rejects → `runtimeMemoriesError` set |
| `store_delete_memory_happy_path` | Mock IPC + 2 rows → 1 row left after delete |
| `store_delete_memory_error_path` | Mock IPC rejects → `runtimeMemoriesError` set; row not optimistically removed |
| `MemoryPreview_renders_runtime_memories_list` | 2 rows in store → component renders 2 list items |
| `MemoryPreview_delete_button_opens_confirm` | Click delete → `ConfirmDialog` opens with title |
| `MemoryPreview_confirm_delete_calls_store` | Confirm click → `store.deleteMemory(id)` invoked |
| `MemoryPreview_cancel_delete_keeps_row` | Cancel click → row remains; IPC not invoked |
| `recall_pitfall_footnote_active_hit_returns_text` (P3) | Insert active pitfall with `tool_name='shell'` + `command_pattern='cargo test'`; recall with matching `tool_name` + `command` → `Some("⚠️ Memory: ...")` |
| `recall_pitfall_footnote_unrelated_tool_returns_none` (P3) | Insert active pitfall for `shell`; recall with `tool_name='read_file'` → `None` |
| `recall_pitfall_footnote_verified_hit_returns_none_for_p3` (P3) | Insert verified pitfall (promote via direct DB write); recall → `None` (verified is P5 scope, P3 strictly excludes) |
| `recall_pitfall_footnote_candidate_hit_returns_none` (P3) | Insert candidate pitfall; recall → `None` (candidate is P2 scope, not yet promoted) |
| `recall_pitfall_footnote_command_pattern_mismatch_returns_none` (P3) | Insert pitfall with `command_pattern='cargo test'`; recall with `command='npm test'` → `None` |
| `recall_pitfall_footnote_empty_db_returns_none` (P3) | Empty DB; recall with any `tool_name` + `tool_input` → `None` (no panic, no error) |
| `single_failure_does_not_trigger` (P4) | `FailureTracker` with one `is_error=true` for `shell` → `try_record_outcome` returns `None`; no reflection spawned (PRD AC #3) |
| `two_failures_then_success_triggers` (P4) | `shell` × 2 `is_error=true` then `is_error=false` → `try_record_outcome` returns `Some(_)`; tracker resets to 0 (PRD AC #1) |
| `first_call_success_does_not_trigger` (P4) | `is_error=false` as first event for any `tool_name` → `try_record_outcome` returns `None` |
| `one_failure_then_success_does_not_trigger` (P4) | `shell` × 1 failure then 1 success → no trigger (counter resets on success; below threshold) (PRD AC #3) |
| `counter_resets_after_trigger` (P4) | Trigger fires, then 2 more failures → counter goes 0→1→2, no second trigger until a new success-then-failure cycle |
| `tools_have_independent_counters` (P4) | `shell` failure × 2 does NOT affect `edit_file` counter; each `tool_name` has its own `TrackerEntry` |
| `try_record_outcome_writes_active_pitfall_end_to_end` (P4) | Real DB + MockProvider: 2 failures + 1 success → `reflect_to_pitfall` runs → `insert_memory` writes a row with `kind=Pitfall`, `status=Active`, `scope=Project`, `source_ref=<request_id>:<tool_name>`, populated `trigger_key` (PRD AC #1) |
| `invalid_json_from_llm_does_not_panic_or_write` (P4) | MockProvider returns prose-without-JSON → `strip_code_fence` + `serde_json::from_str` fail → `tracing::warn!` + silent drop; no row in DB; no panic |
| `try_record_outcome_does_not_block_caller` (P4) | MockProvider with `tokio::time::sleep(10s)`; `try_record_outcome` returns in < 100ms (fire-and-forget hard rule) (PRD AC #2) |
| `strip_code_fence_handles_common_cases` (P4) | Input `"```json\n{...}\n```"` → `{...}`; input ` ```\n{...}\n``` ` → `{...}`; input `{...}` (no fence) → `{...}` |
| `truncate_for_reflect_under_cap_passes_through` (P4) | Input 1 KiB → output identical (under 2 KiB cap) |
| `truncate_for_reflect_over_cap_appends_marker` (P4) | Input 4 KiB → output truncated to 2 KiB with `…(truncated)` marker |
| `reflected_pitfall_is_recallable_by_p3_helper` (P4) | End-to-end: P4 reflection writes a pitfall with `tool_name='shell'`, `command_pattern='cargo test'`; subsequent `permissions::recall_pitfall_footnote(pool, 'shell', tool_input_with_cargo_test)` returns `Some("⚠️ Memory: ...")` (PRD AC #4) |
| `agent_loop_p5_soft_block_short_circuits_execute` (P5) | 端到端:verified pitfall + `is_full_match` → 首次 tool_use 触发 SoftBlock(`execute_tool` 未调、`is_error=false` 提示、`memory_id` 入 session set) |
| `agent_loop_p5_soft_block_second_hit_degrades_to_execute` (P5) | 同坑二次命中 → `Footnote` + 正常 `execute_tool`(D1 防循环) |
| `p5_recall_verified_full_match_returns_soft_block` (P5) | `recall_pitfall`:verified + 完全匹配 → `SoftBlock`;path/command-agnostic(皆 None)→ `Footnote` |
| `promote_if_eligible_*` (P5) | hit_count 跨 2 升 active;跨 5 + age≥3 天升 verified;demoted 不被 bump 晋升(矩阵拒绝) |
| `jaccard_*` / `char_trigrams_*` (P5) | 中文短句重叠 Jaccard>0.7;identical=1.0;disjoint=0.0;Unicode char 非 byte |
| `pick_keeper_*` / `trigger_key_equal_*` (P5) | 高 confidence/hit_count 胜出;trigger_key 三字段(tool+command_pattern+path_globs)全等 |

30+ tests across DB / agent / tool / IPC / store / component.

### 7. Wrong vs Correct

#### Wrong: per-tool Tier 4 ask on `remember` → Correct: silent-allow + safety net

```rust
// BAD
PermissionContext::new(...).with_ask(Tier4Ask::Write).check(REMEMBER_TOOL_NAME)?;

// GOOD — remember is a knowledge-write, not a file mutation.
// No Tier 4 ask. Safety net lives in insert_memory:
// - sensitive content regex
// - 500-char content cap
// - per-session count cap (50)
insert_memory(pool, InsertMemoryInput {
    scope, kind, title, content, tags, source_session_id: ctx.session_id, ...
}).await?;
```

The tool returns the new id. The user sees the new memory in `MemoryPreview` (runtime memories section) and can delete it. The write is "autonomous" — visible and revocable, not pre-approved.

#### Wrong: separate user message for recall → Correct: append to instruction message

```rust
// BAD — recall as messages[1]
if let Some(text) = build_recall_text(...).await? {
    messages.push(synthetic_user_message(text));
}
provider.send(messages).await;

// GOOD — recall is a new block in messages[0]
if let Some(text) = build_recall_text(...).await? {
    let block = memory_recall::build_recall_block(&text);
    messages[0].content.push(block);  // append, not insert
}
provider.send(messages).await;
```

Recall is ephemeral (not persisted to message history); it lives in the same `messages[0]` synthetic user message as `build_instructions_blocks`. The `cache_control: Ephemeral` breakpoint on the first instruction block stays put.

#### Wrong: `cache_control` on recall block → Correct: no `cache_control`

```rust
// BAD
ContentBlock::Text { text: recall_text, cache_control: Some(Ephemeral) }

// GOOD — no cache_control on recall blocks
ContentBlock::Text { text: recall_text, cache_control: None }
```

Anthropic's "last cache_control block is the breakpoint" rule: recall blocks must NOT carry a `cache_control` marker. The instruction block already carries the marker; adding another shifts the breakpoint to the recall block and demotes the instructions from cache anchor.

---
