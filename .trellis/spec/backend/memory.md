# Memory Contract — User / Project Two-Layer Loader (B5)

> **基线**:2026-06-10 commit `b5-memory-user-project-2layer` (PR1: backend loader + injection)
> **来源**:V2 1 期 B5 Memory 任务后端模块
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) — system prompt 注入(本文件引用其 §2 协议映射)
> - [tool-contract.md](./tool-contract.md) — ReadGuard 失败兜底模式
> - [error-handling.md](./error-handling.md) — tracing::warn! 模式
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider 抽象隔离
>
> **何时读本文**:涉及 4 文件 memory 加载 / system prompt 注入 / 监听 inotify / `MemoryCache` / `read_memory_*` IPC / `open_memory_in_editor` IPC 时。

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

The memory system is a **read-through cache** with **notify**-
based hot-reload. The agent core reads the 4 fixed memory files
(2 layers × 2 filenames) on every chat turn, formats them into
the system prompt, and watches the files for editor saves so
the next turn sees the latest content.

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
    User,      // ~/.config/everlasting/{CLAUDE.md,AGENTS.md}
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

```rust
// app/src-tauri/src/memory/tokens.rs
pub async fn count_tokens(text: &str) -> u32;  // cl100k_base
```

### 3. Contracts

#### 4 fixed file paths

| Layer | Source | Path |
|---|---|---|
| User | CLAUDE.md | `<config_dir>/everlasting/CLAUDE.md` |
| User | AGENTS.md | `<config_dir>/everlasting/AGENTS.md` |
| Project | CLAUDE.md | `<project.path>/CLAUDE.md` |
| Project | AGENTS.md | `<project.path>/AGENTS.md` |

`<config_dir>` is `dirs::config_dir()` — on Linux (the dev
platform) this is `~/.config/`. The trailing `/everlasting/`
subdirectory is hard-coded (so the loader never collides with
other tools' `~/.config/` files).

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

1. User has `~/.config/everlasting/CLAUDE.md` (1 KB, 250
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
9. User edits `~/.config/everlasting/CLAUDE.md` in
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
