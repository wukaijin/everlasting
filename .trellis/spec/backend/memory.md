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


---

## Part Index (08-07-large-file-splitting)

- [scenario-two-layer-memory-injection](./memory/scenario-two-layer-memory-injection.md)
- [scenario-autonomous-memories](./memory/scenario-autonomous-memories.md)
- [scenario-observability-management](./memory/scenario-observability-management.md)
- [decisions](./memory/decisions.md)
