<!-- Moved from subagent-runs-schema.md 2026-09-28 (doc-split): "When to apply" + "When NOT to apply" -->

# subagent_runs 模式泛化 — 何时套用 / 何时别套

> hub:[subagent-runs-schema.md](../subagent-runs-schema.md)(schema / CRUD / IPC / cap / audit 契约)。

### When to apply this pattern

A future "B6-style" subagent / child-context feature should follow
this table pattern verbatim:

- [x] **One row per child invocation** (mirrors the parent's
      one-row-per-request model). The row's lifecycle is: INSERT at
      start with `status='running'` + UPDATE at end with terminal
      status. This matches the `started_at` / `finished_at` /
      `created_at` 3-timestamp shape and gives a future "5 workers
      active" UI badge a natural query target.
- [x] **Hard FK to parent with `ON DELETE CASCADE`** + a soft-FK
      parent_request_id TEXT (for in-flight cross-references that
      don't survive app restart).
- [x] **CHECK-constrained status enum** with a paired Rust enum
      (`SubagentStatusDb` / `WorkerKind` / etc.) and `as_str` +
      `from_str_opt` lockstep. The lenient `from_str_opt` makes
      forward-compat safe (a future binary adding a new status
      variant doesn't crash an older binary reading a newer DB).
- [x] **JSON-typed payload columns** (e.g. `token_usage_json` /
      `transcript_json`) rather than 4 separate columns. The
      payload is serialized via `serde_json::to_string`; reads
      decode with `serde_json::from_str` + a typed wrapper struct.
- [x] **Best-effort `tracing::warn!` + continue** on the
      `update_*_finished` failure path (terminal writes, not
      normal-path persists — see hub "Pattern: best-effort
      warn+continue").
- [x] **A separate streaming variant** for any per-turn data
      accumulation (token usage, etc.) so the per-turn path can
      bypass the `skip_persist` gate (see RULE-A-015 lesson —
      gate by *contention invariant*, not by *call site shape*).
- [x] **Indexed by `(parent_session_id, child_ts DESC)`** for the
      list-by-parent query (mirrors `idx_session_audit_events_session_ts`).
- [x] **At least one happy-path + one CASCADE + one cap / payload
      test per CRUD function** (7 tests in `db/subagent_runs_tests.rs` for
      PR2a, 拆分自 `db/tests.rs`,2026-06-23 按 SQL 域拆为 6 个 `*_tests.rs`;
      same density recommended for future child-context
      features).

### When NOT to apply this pattern

- The child is a **prompt-level** entity (e.g. a single
  `tool_result` for a one-shot tool call) — too small to warrant
  its own table; the existing `messages` table + `metadata`
  JSON column carries it.
- The child's lifecycle is **fully in-memory and never needs
  to survive reload** (e.g. a transient LLM scratchpad). Don't add
  a table — use the `SubagentBufferSink`-style in-memory accumulator
  and don't persist.
- The child is **part of an existing table's row** (e.g. an
  additional role on the `messages` table) — extend the parent
  table rather than add a new one.
