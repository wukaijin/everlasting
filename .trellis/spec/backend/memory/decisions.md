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

## Decision: CLAUDE.md 层 digest 化(分级注入 + load_memory_sections,2026-08-15)

**Context**: 4 层全量注入实测 memory 块 ~10k tok,占首轮 context 72%
(C7D 治完 tools 后反超为最大单项)。memory 是行为指导不能机械压缩,
但 CLAUDE.md(`<reference>` 语义,Claude-Code interop 文件)与 AGENTS.md
(`<primary>`)本就是两个等级。任务 `08-15-memory-block-governance`。

**Decision**(`memory/digest.rs`):
- **Tier 规则**:AGENTS.md 永不 digest(primary,always-on 主指令);
  CLAUDE.md 且 tokens > 600(`DIGEST_THRESHOLD_TOKENS`)才 digest;
  ≤600 小层全量豁免(user CLAUDE.md 36B 自然落入)。
- **digest 形态**:fence-aware 切节(``` 状态机,code block 内 `# 注释`
  不切节)+ 目录(节标题 + ≤120 chars 首句;纯 fence 节回退取节内首个
  非空行)。纯机械生成,同输入同输出 — **禁止 LLM 生成摘要**(非确定性
  会打爆 session 内前缀稳定)。
- **寻址**:banner label 命名空间(`Project CLAUDE.md#节标题`),匹配
  = 精确 → 唯一前缀 → 唯一子串(标题是自然语言,需容错);错误消息附
  可用清单自愈。
- **粘性**:进程级 `OnceLock` 单例 `MemoryDigestRegistry`(对标
  `memory/tokens.rs` ENCODER 先例,**不走 AppState/run_chat_loop 穿参**
  — 那条路要动 72 个调用点);已加载节全文**追加在目录之后**(保住目录
  段前缀缓存);`delete_session_inner` 清理(Tauri + daemon 共用路径)。
- **`load_memory_sections` 元工具**:drive.rs 侧挂 append(gate =
  `memory_digest_enabled`(缺省 on,fail-open)&& !worker && !群聊,与
  注入同源);执行在 `chat_loop/tools.rs` serial 顶部按名拦截,**独立于
  stub gate**(两开关正交 — stub off 时不能变成未知工具)。read-only
  自有数据,不走权限链。
- **不变量**:banner 块仍是唯一 cache 断点;`load_for_session` 恒返 4
  元素;worker 注入路径 `subagent/prompt.rs` 一行不动(继续调 legacy
  `build_instructions_blocks`);digest_off 与 legacy 逐字节一致(单测锁)。

**Consequences**:
- ✅ live 实测(2026-08-15):memory 10124→2080(-79.5%),首轮 context
  -47%;双轮 cache 率 99.8%(不劣化于 off 的 99.7%);定向探针确认模型
  会按目录主动拉节并遵循。
- ⚠️ 模型不拉节时只看目录(标题 + 首句)—— 目录质量决定可发现性;
  Phase 2 候选:节级 summary frontmatter 约定。
- ⚠️ 拉取节当轮有一次 prefix miss(内容变长),粘性后重新稳定 — 设计
  接受,cache 率度量裁决。

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
