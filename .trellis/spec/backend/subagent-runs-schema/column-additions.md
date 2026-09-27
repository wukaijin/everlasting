<!-- Moved from subagent-runs-schema.md 2026-09-28 (doc-split): B6+ C + L3b PR1 + turn_trace 增量段 -->

# subagent_runs 列增量与扩展 — B6+ C(2026-07-03)/ L3b PR1(2026-06-27)/ turn_trace(2026-08-20)

> hub:[subagent-runs-schema.md](../subagent-runs-schema.md)(建表 schema / CRUD / IPC / cap / audit 契约)。

## B6+ C additions (2026-07-03)

### New column: `model_display TEXT NULL`

Carries the worker's **actual** model display name (the third element of `resolve_worker_provider`'s return tuple: `Some(display_name)` on catalog hit, `None` on parent inheritance / catalog miss). The frontend reads this for the dispatch card's model chip and the `<SubagentDrawerHeader>`'s model slot (AC14-15); both are `v-if`-guarded on non-null so legacy / parent-inheriting rows render without a chip.

**Why a dedicated column, not parse-from-tool_result**: the wire `[model: <name>]` line in the dispatch_subagent `tool_result` (added by task 07-03-subagent-frontmatter-model) is an implementation detail of the parent-facing LLM signal. Parsing it back on the frontend would re-implement the parser in 2 places (card + drawer) and is brittle to format changes (the format is `format_dispatch_result_with_model`'s private contract). Persisting the value structurally:
- Single source of truth (`run_subagent` is the only writer — same site that builds the `[model:]` line, so the two never disagree).
- Trivially testable (DB round-trip is the regression net).
- Frontend parsers stay shallow (a single `v-if="modelDisplay"`).

**Lifecycle**:
- `insert_run_with_id` carries a new trailing `model_display: Option<&str>` parameter (the value the dispatch path writes; `None` → `NULL`). Pre-C callers pass `None` (legacy compat).
- The value is set ONCE at INSERT time and never updated (the worker's model doesn't change mid-run).
- The cell's actual value comes from `resolve_worker_provider`'s `Some(name)` arm (catalog hit). Parent inheritance (`resolve_worker_provider` returns `None`) writes NULL — consistent with the `[model:]` line omission in `format_dispatch_result_with_model` for the same `None` case.

**NULL semantics** (all render the same way in the UI: chip hidden, no "inherit parent" placeholder):
- Pre-C rows (column didn't exist; migration is non-destructive).
- C-task builtin / user / project agents with no override and no frontmatter `model:` (parent inheritance).
- C-task DB override pointing at a model that's been deleted out from under it (catalog miss → parent fallback at dispatch; the override's `id` is in `subagent_runs.model_display` only when the catalog hit succeeded).

**Wire form**: `modelDisplay: string | null` (camelCase via the backend `#[serde(rename_all = "camelCase")]` on both `SubagentRunRow` and `SubagentRunSummary`).

### Migration: `add_subagent_runs_model_display_column`

Idempotent column-add via `add_subagent_runs_column_if_missing` (same pattern as `task` / `final_text` / `turn_count` / `worktree_path` — see "L3b PR1 additions" below for the helper's contract). NULL-able, no DEFAULT — pre-C rows keep NULL and the UI degrades to "no chip" cleanly.

## L3b PR1 additions (2026-06-27)

### New column: `worktree_path TEXT NULL`

Tracks the worker worktree path so a future PR3 `merge_worker` / `discard_worker` tool (and the future SubagentDrawer merge/discard UI) can locate the branch + worktree for a preserved-changes run. Lifecycle:

- **INSERT (`insert_run_with_id`)**: `worktree_path` is NOT set at INSERT time (it's NULL) — the worker worktree is created AFTER the row, when isolation is active.
- **UPDATE on worker exit (post-loop, in `run_subagent`)**: `worktree_path` is updated to `Some(worker_worktree_path)` if `probe_worker_changes` reports changes; NULL if destroyed (no changes / explicit destroy).

The column is **nullable** because not all subagent runs are isolated (`researcher` / `isolation=false` → no worker worktree).

### New function: `insert_run_with_id` (replaces `insert_run` for the isolated path)

```rust
pub async fn insert_run_with_id(
    pool: &SqlitePool,
    id: &str,                       // caller-supplied UUID (vs auto-generated)
    parent_session_id: &str,
    parent_request_id: &str,
    subagent_name: &str,
    task: Option<&str>,
    // B6+ C (2026-07-03): worker's *actual* model display; see
    // "B6+ C additions" above. Pre-C callers pass `None`.
    model_display: Option<&str>,
) -> Result<(), sqlx::Error>
```

`insert_run` (auto-generates UUID) is retained for the `db/subagent_runs_tests.rs` integration suite, but is `#[allow(dead_code)]` in the lib build (the db integration tests are not visible to `cargo check --lib` since they're a separate test target). The new `insert_run_with_id` lets `run_subagent` pre-generate the UUID (e.g. `Uuid::new_v4()`) and pass it explicitly, so the worker worktree path (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`) can be **derived from the id BEFORE the row is inserted** — the worktree is created first, then the DB row records its path.

### Migration: `add_subagent_runs_worktree_path_column`

Idempotent column-add migration. CHECK constraint unchanged (still `running | completed | cancelled | error | incomplete`). Index strategy unchanged (still indexed by `(parent_session_id, child_ts DESC)` per the table-level pattern). The column-add is non-breaking: existing rows get `NULL`, which is the correct "not isolated" state for pre-L3b runs.

## turn_trace 关联:per-run worker 轮度量(2026-08-20,08-20-worker-turn-trace-persist)

`subagent_runs.id` 现在同时是 `turn_trace.run_id` 的取值域(worker 行):
worker 的每个真实 LLM turn 落一行 `(parent_session_id, run_id, seq)`,
携带 usage_json + tools_token + system_token + context_window(memory/
images/@文件 列按 worker 契约 NULL)。**run_id 无 FK** —— `''` 哨兵
(主 loop 行)不是合法 run id;run 行无独立删除路径,生命周期由
turn_trace 自身的 `session_id` CASCADE 兜底(删 session 同时级联两者)。
读侧 `list_worker_turn_traces(run_id)`(SubagentDrawer「Token 明细」,
前端 `useSubagentRunsStore.runTracesByRunId` 粘性缓存);`token_usage_json`
(run 级累计)仍是 run 行自己的权威字段,per-turn 行是明细不是替代。
完整切片语义与唯一键重建迁移见
[token-usage-tracking/08(worker per-turn 行)](../token-usage-tracking/08-worker-per-turn-trace.md)
与 [database-guidelines §表约束加宽](../database-guidelines.md)。
