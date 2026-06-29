# Memory Contract — Static Instruction Loader (B5) + Autonomous Runtime Memory (V2 2 期)

> **基线**:
> - 2026-06-10 commit `b5-memory-user-project-2layer` (V2 1 期: 4 文件 static loader)
> - 2026-06-29 `06-29-am-p2-readwrite` (V2 2 期 P1 DB + P2 手工读写闭环)
> **来源**:
> - V2 1 期 B5 Memory 任务后端模块 — `## Scenario: Two-Layer Memory Injection`(本文 §Scenario 1)
> - V2 2 期 自主记忆 epic — `## Scenario: Autonomous Memories (V2 2 期)`(本文 §Scenario 2)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) — system prompt + synthetic user message 注入(两个 scenario 都引用其 §2 协议映射)
> - [tool-contract.md](./tool-contract.md) — ReadGuard 失败兜底模式 + `remember` 工具 silent-allow 权限模型
> - [error-handling.md](./error-handling.md) — tracing::warn! 模式
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider 抽象隔离
> - [agent-loop-architecture.md](./agent-loop-architecture.md) — turn 循环内 recall 注入点
>
> **何时读本文**:涉及 4 文件 memory 加载 / system prompt 注入 / 监听 inotify / `MemoryCache` / `read_memory_*` IPC / `open_memory_in_editor` IPC / `autonomous_memories` 表 / FTS5 召回 / `remember` tool / `memory_recall` 注入 / runtime memories UI 时。**Scenario 1 与 Scenario 2 是 sibling,不是替代**:同一个文件中两套独立子系统,边界不可混。

---

# Memory Contract

> Two-layer Markdown memory (User + Project) loaded into the LLM
> system prompt at the ⑤a context-construction stage.

---

## Overview

B5 Memory is V2 第一档 (first-tier) task landed 2026-06-10.
V2 1 期 ships **2 layers** (User + Project); Session and Runtime
layers are forward-compat enum variants that exist on the type
level but are never populated. The contract here describes V2
1 期; the Session / Runtime design is deferred to V2 2 期.

> **⚠️ Updated 2026-06-15 (RULE-C-001/C-002/C-004)**: the
> `notify`-based watcher was **removed**. Freshness is now a
> read-through **mtime fence** — every `load_for_session` stats
> each file's `mtime` and reloads the slot on change. The
> `invalidate_*` API, the debounce loop, and `MemoryWatcher` are
> all gone; C-002 (new-project watch) and C-004 (dropped-watcher
> hazard) are satisfied for free. The `notify watcher` /
> `Decision: ...watcher-driven invalidation` sections below
> describe the **old** design and are kept as historical
> reference — they no longer match the code. See
> `.trellis/tasks/06-15-p1-memory-watcher-appstate/`.

The memory system is a **read-through cache** whose freshness
is decided at **read time** by an mtime fence (no background
watcher). The agent core reads the 4 fixed memory files (2
layers × 2 filenames) on every chat turn, stats each file's
`mtime`, and reloads any slot whose `mtime` changed since the
last load — so the next turn always sees the latest content.

---

## Scenario: Two-Layer Memory Injection

### 1. Scope / Trigger

- Trigger: the agent loop needs to inject per-user / per-project
  Markdown memory (CLAUDE.md / AGENTS.md) at the ⑤a context-
  construction stage (per `docs/ARCHITECTURE.md` §2.2).
- Why code-spec depth: the system prompt is the LLM's only
  ground truth on the user's environment. A misformatted
  injection silently confuses the model; a missing memory
  layer silently downgrades the user experience. Both
  failure modes are hard to debug in production.
- V2 1 期: 2 layers only. Session / Runtime memory explicitly
  out of scope (V2 2 期).

### 2. Signatures

```rust
// app/src-tauri/src/memory/types.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    User,      // ~/.claude/CLAUDE.md (Claude Code interop) + ~/.config/everlasting/AGENTS.md (Everlasting-native)
    Project,   // <project.path>/{CLAUDE.md,AGENTS.md}
    #[allow(dead_code)] Session,  // V2 2 期
    #[allow(dead_code)] Runtime,  // V2 2 期
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Claude,    // CLAUDE.md
    Agents,    // AGENTS.md
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "reason")]
pub enum LayerStatus {
    Loaded,                            // file read successfully
    Missing,                           // file does not exist
    Error { reason: String },          // I/O / encoding / size failure
}

pub struct MemoryLayer {
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub path: PathBuf,
    pub content: String,
    pub tokens: u32,
    pub status: LayerStatus,
}

pub struct MemoryLayerInfo {
    // No `content` — fetched on demand via read_memory_content.
    pub kind: MemoryKind,
    pub source: MemorySource,
    pub path: PathBuf,
    pub tokens: u32,
    pub status: LayerStatus,
    pub char_count: usize,
}
```

```rust
// app/src-tauri/src/memory/loader.rs
pub struct MemoryCache {
    user: RwLock<[Option<MemoryLayer>; 2]>,
    project: RwLock<HashMap<String, [Option<MemoryLayer>; 2]>>,
}

impl MemoryCache {
    pub fn new() -> Self;
    pub fn arc() -> Arc<Self>;
    pub async fn invalidate_user(&self);
    pub async fn invalidate_user_slot(&self, source: MemorySource);
    pub async fn invalidate_project(&self, project_id: &str);
    pub async fn invalidate_project_slot(&self, project_id: &str, source: MemorySource);
    pub async fn peek_user(&self, source: MemorySource) -> Option<MemoryLayer>;
    pub async fn peek_project(&self, project_id: &str, source: MemorySource) -> Option<MemoryLayer>;
}

pub async fn load_for_session(
    cache: &MemoryCache,
    project_id: &str,
    project_path: &str,
) -> Vec<MemoryLayer>;

pub fn build_banner(layers: &[MemoryLayer]) -> String;
pub fn build_layers_block(layers: &[MemoryLayer]) -> String;
```

> **2026-06-11 B5 refactor**: `build_banner` / `build_layers_block`
> are still part of the public surface (the frontend `MemoryPreview`
> consumes them as `String` to render the in-app preview panel),
> but they are **not** what the agent loop uses to inject
> instructions into the LLM context. Use `build_instructions_blocks`
> for that — it returns a `Vec<ContentBlock>` shaped for the
> synthetic user message and carries `cache_control: Some(Ephemeral)`
> on the first block so Anthropic can cache the 4 instruction
> files across the 20-turn loop. See `docs/IMPLEMENTATION.md §4`
> 2026-06-11 entry for the full rationale.

```rust
// app/src-tauri/src/memory/loader.rs (added 2026-06-11)
pub fn build_instructions_blocks(layers: &[MemoryLayer]) -> Vec<ContentBlock>;
// Returns an empty Vec when no layer is Loaded (caller skips the
// synthetic message entirely on a fresh install).
// Non-empty Vec shape:
//   Block 0: banner text + cache_control: Some(CacheControl::Ephemeral)
//            — the cache breakpoint on subsequent turns.
//   Blocks 1..N: per loaded layer, in canonical order, with
//            AGENTS.md wrapped in <primary instructions>...</primary>
//            and CLAUDE.md in <reference>...</reference>.
//            No cache_control on body blocks (Anthropic's
//            "last cache_control block is the breakpoint" rule
//            means only Block 0 needs the marker).
```

```rust
// app/src-tauri/src/memory/tokens.rs
pub async fn count_tokens(text: &str) -> u32;  // cl100k_base
```

### 3. Contracts

#### 4 fixed file paths

| Layer | Source | Path |
|---|---|---|
| User | CLAUDE.md | `<home_dir>/.claude/CLAUDE.md` |
| User | AGENTS.md | `<config_dir>/everlasting/AGENTS.md` |
| Project | CLAUDE.md | `<project.path>/CLAUDE.md` |
| Project | AGENTS.md | `<project.path>/AGENTS.md` |

`<home_dir>` is `dirs::home_dir()` — on Linux (the dev
platform) this is `~/.claude/`. This path matches Claude
Code's own user-level CLAUDE.md so the two tools share the
same file (locked 2026-06-26 user-claude-md-home-dir).

`<config_dir>` is `dirs::config_dir()` — on Linux (the dev
platform) this is `~/.config/`. The trailing `/everlasting/`
subdirectory is hard-coded (so the loader never collides with
other tools' `~/.config/` files). AGENTS.md is
Everlasting-native and stays in the original location; only
CLAUDE.md moved.

`<project.path>` is the raw `projects.path` column from SQLite;
the chat command has already validated it through
`projects::boundary::assert_within_root`.

#### Canonical order

`load_for_session` returns 4 layers in this order:
1. User CLAUDE.md
2. User AGENTS.md
3. Project CLAUDE.md
4. Project AGENTS.md

The chat command and the Tauri command `read_memory_layers`
both rely on this order. The 0-based index is used in the
LLM banner position; the LLM is told the labels inline so
the index is internal-only.

#### System prompt injection

The chat command's `build_context` step calls
`load_for_session` and prepends the memory to the system
prompt:

```text
<system>已加载 N 个 memory: [User CLAUDE.md] (X tokens) / [Project AGENTS.md] (Y tokens)</system>

[User CLAUDE.md]
<user claude body>

[Project AGENTS.md]
<project agents body>

<base system prompt from build_system_prompt>
```

The banner uses `<system>...</system>` (Anthropic XML tag) so
the model treats it as a server-injected reminder, not as
user content. The base system prompt (worktree state, project
info, etc.) follows the memory block per the existing
`build_system_prompt` order — the order is **Memory →
Role → Skill → history** (the ⑤a sub-step of
`docs/ARCHITECTURE.md` §2.2).

#### Failure tolerance (HARD RULE)

Every file is loaded in isolation. A single failure does not
abort the chat. The 6 known failure modes and their effects:

| Condition | Layer `status` | Other layers | `tracing` |
|---|---|---|---|
| File does not exist | `Missing` | unaffected | (none) |
| File empty (0 bytes) | `Loaded` (tokens=0) | unaffected | (none) |
| File > 100 KiB | `Error { reason: "..." }` | unaffected | `warn!` |
| File not UTF-8 | `Error { reason: "not UTF-8" }` | unaffected | `warn!` |
| Permission denied | `Error { reason: "read failed: ..." }` | unaffected | `warn!` |
| I/O error / symlink loop | `Error { reason: "..." }` | unaffected | `warn!` |

A fresh install with 0 memory files produces an empty banner
and an empty layers block — the chat request proceeds with
the base system prompt alone, no banner noise.

#### Size cap (HARD RULE)

`MAX_FILE_SIZE = 100 * 1024` (100 KiB). Above this, the file
is rejected with `LayerStatus::Error`. Rationale: 4 files *
100 KiB ≈ 100K tokens (the entire context window of a 200K
model). A single memory file > 100 KiB is almost certainly
a content-store accidentally placed at a memory path, not
a real CLAUDE.md. Frontend can offer the user a "preview
the over-cap file" path (out of scope for PR1).

#### Token estimation

V2 1 期 uses `tiktoken-rs` cl100k_base. The encoder is held
in a process-wide `OnceLock<Mutex<CoreBPE>>`; the lock is
held only for the duration of the encode call (microseconds).
The estimation has ~1-2% drift from Anthropic's tokenizer,
which is invisible at the "preview chip" granularity the
UI displays.

The encoder does not raise on pathological inputs. The
100 KiB cap (above) is the only size guard.

#### notify watcher

A single `notify::RecommendedWatcher` watches the user dir
+ every project's dir (registered at app startup, non-
recursive). Events flow through a 1-second debounce so a
single editor save (which fires several inotify events:
Modify → CloseWrite → Attrib) yields one cache
invalidation per file. The watcher holds a `Weak<MemoryCache>`
so it does NOT keep `AppState` alive past Drop.

The watcher fires `Modify | Create | Remove` events but
filters at the path level: only events whose path is one
of the 4 fixed memory files (per `all_paths`) cause a
cache invalidation. Everything else is silently ignored.

**Limitation**: only files that exist at startup are
watched. The PRD's "新建 memory 文件需重启 session 生效"
rule extends to "新建 project 也需要重启 watcher"; new
projects created at runtime are not auto-watched. Same
goes for newly-created memory files at the existing 4 paths.

#### delete_session / delete_project cache invalidation

`commands::sessions::delete_session` calls
`MemoryCache::invalidate_project(project_id)` after a
successful delete so a future session in the same project
re-reads the files. (delete_project does not exist in the
codebase today; the cache invalidation is wired for when
it is added.)

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `ANTHROPIC_API_KEY` missing | Memory loads as normal; only LLM call fails. |
| All 4 memory files missing | Empty banner + empty layers block; chat proceeds with base prompt alone. |
| User CLAUDE.md > 100 KiB | User CLAUDE.md → `Error`; rest of system unaffected. The banner lists only loaded layers. |
| Project path is a symlink to outside the project | The chat command's existing `assert_within_root` rejects it before `load_for_session` is called. |
| notify watcher fails to start (inotify limit, etc.) | `tracing::warn!`; cache works as a pure read-through (no hot-reload). Subsequent `invalidate_*` calls from `delete_session` still work. |
| notify fires an event for a non-memory file in the user dir | Filtered out by `lookup_key`; no cache mutation. |
| Editor saves a memory file while a chat is streaming | The cache invalidation fires; the in-flight turn does NOT pick up the new content (system prompt is built once per turn). The next turn sees the new content. This is intentional — the ⑤a stage is per-turn. |
| Two chat requests in parallel on the same project | Both get the same `MemoryCache::peek_project` hit; only the first writer (after a miss) does the disk I/O. |
| tiktoken fails to initialise | Process panics at first `count_tokens` call. Acceptable: this is a 1-time setup cost (~200ms) and the failure mode is "LLM is broken anyway". |
| User dir is unwritable | `read_to_string` fails → `Error { reason: "read failed: ..." }` + `tracing::warn!`. Other layers unaffected. |

### 5. Good / Base / Bad Cases

#### Good: typical happy path

1. User has `~/.claude/CLAUDE.md` (1 KB, 250
   tokens) and `<project>/AGENTS.md` (4 KB, 1000 tokens).
2. App starts → watcher registers the user dir and the
   project dir. Both directories are non-recursive.
3. User opens a session, sends a question.
4. Chat command → `load_for_session` → cache miss on first
   call → reads both files → returns 2 loaded + 2 missing.
5. `build_banner` returns
   `"<system>已加载 2 个 memory: [User CLAUDE.md] (250 tokens) / [Project AGENTS.md] (1000 tokens)</system>"`.
6. `build_layers_block` returns the 2 section bodies.
7. System prompt = banner + layers block + base prompt.
8. LLM sees the memory at the top of its context.
9. User edits `~/.claude/CLAUDE.md` in
   `$EDITOR`. Editor save fires 3 inotify events.
10. Watcher debounces → 1 second after the last event, the
    user CLAUDE.md slot is invalidated.
11. User sends another question. Next `load_for_session`
    sees the cache miss, re-reads, and the new content is
    in the prompt.

#### Base: fresh install

1. User installs the app, no memory files exist anywhere.
2. `load_for_session` returns 4 `Missing` layers.
3. `build_banner` returns `""` (no loaded layers).
4. `build_layers_block` returns `""`.
5. Chat command: `system_prompt = base_prompt` (no memory
   section). The chat works exactly as before B5 landed.

#### Bad: front-end sends arbitrary path to read_memory_content

1. Frontend sends `path = "/etc/passwd"`.
2. `commands::memory::read_memory_content` matches against
   `all_paths(project_path)`.
3. `/etc/passwd` matches no known path → returns
   `"read_memory_content: path '/etc/passwd' is not a known memory file"`.
4. Frontend surfaces the error to the user.

The path-allowlist is the security boundary. The IPC MUST
NOT leak arbitrary file content to the frontend.

#### Bad: 100 KiB file in a memory path

1. User has a 200 KiB `CLAUDE.md` (mistakenly placed a
   content dump there).
2. `load_file_inner` checks `meta.len() > MAX_FILE_SIZE`
   → returns `Error { reason: "file is 204800 bytes, exceeds 102400 byte cap" }`.
3. The chat proceeds with the other 3 layers; the banner
   does not list the over-cap file.
4. `tracing::warn!` records the rejection.
5. Frontend preview shows the `Error` status with the
   reason; user can move the file out of the memory path.

#### Bad: editor save during an in-flight chat

1. Chat is mid-stream, system prompt already built.
2. User saves `CLAUDE.md`.
3. Watcher invalidates the cache slot.
4. The in-flight turn completes with the OLD system prompt
   (intentional — system prompt is per-turn, not per-token).
5. The next turn (started by the user clicking Send) sees
   the new content.

### 6. Tests Required

| Test | Asserts |
|---|---|
| `tokens_count_empty_string_is_zero` | `count_tokens("") == 0`. |
| `tokens_count_ascii_short` | `count_tokens("hello")` is 1-2 tokens. |
| `tokens_count_cjk_grows` | 10 CJK chars > 5× a single CJK char. |
| `tokens_count_mixed` | ASCII + CJK mix returns > 0 and < 100 tokens. |
| `file_load_missing_returns_missing_status` | Non-existent file → `(empty, 0, Missing)`. |
| `file_load_loaded_returns_body_and_tokens` | Written file → `(body, tokens>0, Loaded)`. |
| `file_load_empty_file_is_loaded_with_zero_tokens` | Empty file → `(empty, 0, Loaded)`. |
| `file_load_oversize_returns_error` | 100 KiB + 1 → `Error` + `(empty, 0)`. |
| `file_load_non_utf8_returns_error` | 0xFF 0xFE → `Error` + `(empty, 0)`. |
| `loader_load_for_session_with_all_files_present` | 4 files → 4 `Loaded` in canonical order. |
| `loader_load_for_session_with_all_files_missing` | 0 files → 4 `Missing`. |
| `loader_load_for_session_partial_files` | 2 files → 2 `Loaded` + 2 `Missing`. |
| `loader_invalidate_user_slot_re_reads` | Cache hit before invalidation, miss after → file content seen. |
| `loader_invalidate_project_does_not_touch_user` | Project invalidation leaves user slot cached. |
| `loader_different_projects_have_independent_caches` | Project A and B have separate slots. |
| `banner_with_no_loaded_layers_is_empty` | All missing → banner is `""`. |
| `banner_with_some_loaded_layers_lists_them` | 1 loaded + 3 missing → banner has only the loaded one. |
| `layers_block_renders_only_loaded_layers` | 2 loaded → block has only 2 sections. |
| `all_paths_yields_four_entries_in_canonical_order` | `User / Claude`, `User / Agents`, `Project / Claude`, `Project / Agents` order. |
| `memory_cache_arc_smoke` | `Arc<MemoryCache>` exposes the public API without panic. |

20 tests as of PR1. Future PRs (PR2 frontend) will add
`commands::memory` IPC tests on top.

### 7. Wrong vs Correct

#### Wrong: panic on missing file

```rust
// BAD — fail loudly, treat memory as a hard requirement
pub async fn load_layer(...) -> Result<MemoryLayer, String> {
 let body = std::fs::read_to_string(&path)
 .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
 ...
}
```

A user who hasn't set up `CLAUDE.md` is the COMMON case (a
fresh install). Making memory load a hard requirement
silently bricks every new user. The whole point of B5 is
that memory is **opportunistic**, not mandatory.

#### Correct: missing file is a valid state, not an error

```rust
// GOOD — absorb the failure into LayerStatus, log a warn,
 // never block the chat
async fn load_file_inner(path: &Path) -> (String, u32, LayerStatus) {
 if !path.exists() {
 return (String::new(), 0, LayerStatus::Missing);
 }
 ...
}
```

A `Missing` layer is a first-class state. The banner
formatter skips it. The layers block formatter skips it.
The chat proceeds with whatever subset loaded.

#### Wrong: cache forever

```rust
// BAD — load once, never re-read
pub async fn load_for_session(...) -> Vec<MemoryLayer> {
 let layers = load_all_files_from_disk().await;
 LAYERS_CACHE.set(layers.clone()).await; // one-shot
 layers
}
```

The user edits `CLAUDE.md` and wonders why their changes
aren't picked up. The PRD explicitly demands hot-reload
(`notify` listener).

#### Correct: read-through + watcher-driven invalidation

```rust
// GOOD — cache is invalidated by the watcher; the
// read-through path re-reads on the next miss
async fn read_or_load_user(cache: &MemoryCache, source: MemorySource) -> MemoryLayer {
 if let Some(cached) = cache.peek_user(source).await {
 return cached;
 }
 let layer = load_layer(MemoryKind::User, source, None).await;
 let mut guard = cache.user.write().await;
 guard[slot_index(source)] = Some(layer.clone());
 layer
}
```

The watcher fires the invalidation; the read-through
absorbs the miss. The pattern is identical to the
`ReadGuard` design in `tools/edit_file`.

#### Wrong: write through the watcher

```rust
// BAD — watcher auto-reloads the cache from disk
fn on_event(event: Event) {
 for path in &event.paths {
 let layer = read_file_from_disk(path); // sync I/O in event loop
 cache.put(path, layer); // race with concurrent reader
 }
}
```

Sync I/O on the notify event loop blocks the watcher.
Concurrent reads race with the writer — a reader can
see half-written state.

#### Correct: invalidate, do not reload

```rust
// GOOD — watcher just clears the slot; the next reader
// does the I/O on its own task
fn on_event(event: Event) {
 for path in &event.paths {
 if let Some(key) = lookup_key(&path_table, path) {
 debounce_table.entry(key).or_insert(Instant::now());
 }
 }
}
```

The watcher is a pure state-mutation callback. It
debounces, then sets the cache slot to `None` (via
`apply_invalidation`). The next `load_for_session` call
takes the read-lock and does the disk I/O. No race, no
sync I/O on the watcher's task.

---

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
  - P3 `06-29-am-p3-tool-recall` (planning): tool-execution-time recall (before each `tool_use`)
  - P4 `06-29-am-p4-event-reflect` (planning): event-driven auto-write hooks
  - P5 `06-29-am-p5-quality` (planning): state machine auto-promotion + hygiene job

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
| 召回对象 | `kind = 'pitfall'` AND `status = 'active'` | `verified` 是 P5 软拦截范围,严格排除;`candidate` 是 P2 范围,严格排除 |
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

#### Bad: unfiltered candidate recall in P5+

1. P5 lands with state machine: Candidate → Active → Verified promotion.
2. Code updates `build_recall_text` to `RecallStatusFilter::P5Auto`.
3. P2 `search_memories_fts_recall` with `P2Manual` filter is still reachable from a stray call site (e.g., legacy `update_checklist`).
4. Candidate memories pollute the recall → LLM sees unverified noise.

The `RecallStatusFilter` enum is a load-bearing contract; updating it requires a code-wide audit of call sites.

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

P3 is **active-only footnote**, period. Verified soft-intercept is **P5 scope** (spike-007 §4, 命中分档表). The function `recall_pitfall_footnote` returns `Result<Option<String>, sqlx::Error>` specifically because P5 will add a sibling `verified_pitfall_decision` returning a structured `Decision` and the two can coexist at the chat_loop seam without touching each other.

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

## Decision: Read-through cache + watcher-driven invalidation

**Context**: PRD D3 (2026-06-10 grill decision) locked "启动
一次 + notify 监听" as the loading strategy. The implementation
question is: where does the I/O live — in the watcher, in the
chat command, or in a shared cache?

**Decision**: Read-through cache in `MemoryCache`. Watcher
calls `invalidate_*` only. The next `load_for_session` does
the I/O.

**Consequences**:
- ✅ Watcher is a pure state-mutation callback; no sync I/O
  on the notify event loop.
- ✅ Concurrent readers can hit the cache without racing
  the writer.
- ✅ I/O happens on the chat command's async task, which is
  exactly where we want it (we're already going to do I/O
  to send the request anyway).
- ⚠️ The first chat after a watcher invalidation pays the
  disk I/O cost. The user-perceived latency impact is
  negligible (one `read_to_string` of a ≤100 KiB file is
  sub-millisecond on SSD).

## Decision: 2 layers (V2 1 期), 4 layers (V2 2 期) with the same interface

**Context**: PRD D1 (2026-06-10 grill decision) locked the 2-
layer scope. Session / Runtime memory are V2 2 期. The
`MemoryKind` enum and the `MemoryCache` data structure need
to be forward-compat.

**Decision**: `MemoryKind` has 4 variants from day 1. Session
and Runtime are `#[allow(dead_code)]` placeholder variants
that return `None` from `resolve_path` and are silently
filtered by the loader. The cache type is generic over
`(ProjectId, MemoryKind, MemorySource)`.

**Consequences**:
- ✅ V2 2 期 adds new layers without changing the
  `load_for_session` signature.
- ✅ The enum is exhaustively matched in the loader, so a
  future "Session" variant must be explicitly handled (no
  accidental catch-all).
- ⚠️ Two `#[allow(dead_code)]` attributes look like
  dead code to a casual reader. The doc comments
  (above) explain the forward-compat purpose.

## Decision: `tiktoken-rs` cl100k_base for token estimation

**Context**: PRD D7 locked "不限制 token". The display layer
(the frontend preview chip) needs a token count, but the
display granularity is "X tokens" — we don't need
per-model precision.

**Decision**: cl100k_base. `tiktoken-rs` 0.6 is the closest
stable release; the encoder is held in a process-wide
`OnceLock<Mutex<CoreBPE>>` (the underlying BPE state is
`!Send`).

**Consequences**:
- ✅ 1-2% drift from Anthropic's tokenizer — invisible at
  the "X tokens" display granularity.
- ✅ Single BPE table, no per-model complexity.
- ✅ No SDK / API key required (unlike Anthropic's
  tokenizer, which would require an LLM round-trip).
- ⚠️ The cl100k_base table is ~2 MB. Cold-start cost is
  ~200ms one-time; subsequent calls amortise to <1µs/token.

## Decision: Hard size cap (100 KiB) at the loader level

**Context**: PRD D7 says "不限制 token" but also implicitly
trusts the user not to put a 50 MB file in `CLAUDE.md`. A
missing cap means a single bad file can blow the entire
context window.

**Decision**: `MAX_FILE_SIZE = 100 * 1024`. Above this, the
file is rejected with `LayerStatus::Error` + a `tracing::warn!`.

**Consequences**:
- ✅ Worst case: 4 files * 100 KiB ≈ 100K tokens (within
  the 200K context window).
- ✅ A bad file is surfaced as a per-layer `Error`, not a
  global failure.
- ⚠️ A user with a 101 KiB CLAUDE.md has to trim it before
  it shows up in the preview UI. The 100 KiB cap is
  deliberately conservative; we can lift it later if real
  workloads hit the limit.

## Decision: Watcher uses 1-second debounce, not 0

**Context**: Editor saves fire multiple inotify events
(Modify → CloseWrite → Attrib) in rapid succession. A 0-
debounce watcher would invalidate the cache N times per
save, causing N reads on the next chat.

**Decision**: 1-second debounce. The `pending` map keys by
`(kind, source, project_id)`; the debounce loop drains
buckets whose `Instant` is older than `WATCHER_DEBOUNCE_MS`.

**Consequences**:
- ✅ 1 save = 1 invalidation = 1 re-read.
- ✅ The user sees their edit "within 1 second" of saving.
- ⚠️ A user editing two different memory files in rapid
  succession gets both invalidations after a 1s pause.
  This is the desired behavior (each file is independent).

## Decision: Watcher does NOT auto-register new projects

**Context**: PRD D3 says "新建 memory 文件需重启 session".
The natural extension is "新建 project 也需要重启 watcher" —
the watcher's initial watch list is the project list at
startup. A new project created at runtime (e.g. the user
clicks "Add Project" in the UI) does not get its directory
watched until the app restarts.

**Decision**: Same as the PRD. New projects added at
runtime are not auto-watched; the project-layer memory
files for the new project are still readable on the next
chat (the cache miss path re-reads from disk) — they
just don't get hot-reload.

**Consequences**:
- ✅ Predictable behavior: the watch list is fixed at
  startup.
- ⚠️ A user who creates a new project, edits
  `<project.path>/CLAUDE.md`, and sends a chat in the
  same session gets the new content (cache miss path).
  But the watcher's hot-reload doesn't apply to that
  file until restart. For most users, the project path
  doesn't change frequently and a one-time restart is
  acceptable.

---

## Common Mistakes

### Mistake: Treating `MemoryKind::Session` / `Runtime` as live

These variants are forward-compat placeholders. They
return `None` from `resolve_path` and are silently
filtered. Calling `load_layer(Session, ...)` returns an
`Error` layer; the chat proceeds with the user / project
layers only. Do not add new code paths that branch on
"if Session" — that's V2 2 期 territory.

### Mistake: Putting `content` in `MemoryLayerInfo`

`MemoryLayerInfo` is the wire DTO. It must NOT carry
`content` — files can be up to 100 KiB, and putting 4 ×
100 KiB on the IPC for every preview-panel mount is
wasteful. The preview UI calls `read_memory_content(path)`
on demand.

### Mistake: Replacing the base system prompt with memory

The base system prompt (worktree state, project info, etc.)
must follow the memory block, not be replaced by it. The
order is **Memory → Role → Skill → history** per
`docs/ARCHITECTURE.md` §2.2 step ⑤a. Replacing the base
prompt with the memory would silently drop the worktree
state hint the LLM needs to ground its tool calls.

---

## Anti-Patterns

- **Don't** panic on a missing memory file. `Missing` is
  a first-class state.
- **Don't** lossy-convert non-UTF-8 file bodies. The
  corruption is invisible until the LLM misbehaves.
- **Don't** try to "fix" the watcher's hot-reload by
  spawning a background reloader. The watcher's job is
  invalidation; the read-through path handles reload.
- **Don't** put `notify::Event` types on the IPC. The
  frontend's preview panel calls
  `read_memory_layers` on its own cadence (and on
  `memory:reloaded` events from the backend, when the
  frontend is wired up in PR2).
- **Don't** add a per-file or per-layer "last modified"
  timestamp to the wire DTO. The user can read it from
  the OS (right-click → Properties in their file
  manager). The cache eviction is the only place that
  needs the timestamp.
- **Don't** add a `use_memory` tool. The PRD's "Out of
  Scope" section explicitly defers it to V2 2 期
  (Runtime memory). V2 1 期 memory is "preloaded" into
  the prompt; the LLM does not need to actively fetch
  it.
