<!-- Schema spec for subagent_runs table. Moved from database-guidelines.md 2026-06-21 (B6 PR2) -->

# subagent_runs Schema (B6 PR2, 2026-06-20)

> **Source**: extracted from `.trellis/spec/backend/database-guidelines.md` §"subagent_runs" (2026-06-21 doc-trim task).
>
> **Cross-references**:
> - Parent doc: [database-guidelines.md](./database-guidelines.md)

## subagent_runs (B6 PR2, 2026-06-20)

> **Source**: `.trellis/tasks/06-20-b6-pr2-subagent-persistence` PRD;
> the migration lives in `app/src-tauri/src/db/migrations.rs` (B6 PR2
> section, line ~470); the CRUD layer is `app/src-tauri/src/db/subagent_runs.rs`.

The B6 Subagent PR1 (2026-06-19) landed `dispatch_subagent` and
`SubagentBufferSink` — the worker subagent's chat-event / tool:call /
tool:result transcript stays in-memory for the lifetime of the worker
process. PR2 (2026-06-20) lifts that transcript into SQLite so (1) the
PR3 frontend `ToolCallCard` expand UI can render the worker's
intermediate state, (2) a session reload after app restart still shows
the worker's transcript, (3) per-run token usage is auditable.

### Full schema

```sql
CREATE TABLE IF NOT EXISTS subagent_runs (
    id TEXT PRIMARY KEY,                                            -- UUID v4 nanoid
    parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_request_id TEXT NOT NULL,                                -- worker rid (NOT a FK; cancellations in-memory)
    subagent_name TEXT NOT NULL,                                    -- 'researcher' | 'general-purpose'
    status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error','incomplete')),
    started_at TEXT NOT NULL,                                       -- RFC 3339, set on INSERT
    finished_at TEXT,                                               -- NULL = running, set on UPDATE
    token_usage_json TEXT,                                          -- JSON TokenUsage { input / output / cache_creation / cache_read }
    summary TEXT,                                                   -- final_text 纯文本 (NO status prefix; status 字段独立)
    transcript_json TEXT,                                           -- JSON Vec<TranscriptEntry> after 4 MiB cap
    transcript_truncated INTEGER NOT NULL DEFAULT 0,                -- 1 = over 4 MiB cap
    turn_count INTEGER,                                             -- 2026-06-22 (RULE-FrontSubagent-004): actual completed LLM turn iterations the worker executed before reaching terminal state (completed/cancelled/error/incomplete). NULL on pre-PR2 rows; drawer degrades to wall-clock "at X.Xs" suffix for legacy rows.
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
    ON subagent_runs(parent_session_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
    ON subagent_runs(parent_request_id);
```

### Why the schema follows `session_audit_events` precedent

The new table intentionally mirrors `session_audit_events` (introduced
in A2+B7 PR1, 2026-06-13) because both tables share the same conceptual
shape: **per-event child rows of a parent chat session**. Same
characteristics fall out naturally:

- **`id` is a UUID v4 nanoid** (TEXT PK), not auto-increment — matches
  `projects` / `sessions` / `session_tool_permissions` table family, and
  the id is referenced across the agent loop without a DB roundtrip.
- **`parent_session_id` is a hard FK to `sessions(id)` with `ON DELETE
  CASCADE`** — deleting a session cleans up all its worker
  `subagent_runs` in one shot. The `PRAGMA foreign_keys = ON` setting
  (per-connection; `init_pool` enables it on first use) is **required**
  for the cascade to fire. Cascade test in `db/subagent_runs_tests.rs`
  (拆分自 `db/tests.rs`,2026-06-23 按 SQL 域拆为 6 个 `*_tests.rs`):
  `subagent_runs_cascade_delete_with_parent_session`.
- **`parent_request_id` is a TEXT NOT NULL with NO FK** — it carries
  the worker rid (the `"{parent_rid}-sub-{seq}"` string the agent loop
  builds at `agent/subagent/dispatch.rs`,拆分自 `chat_loop.rs:1989` 当年的位置,
  2026-06-23 run_subagent 抽离 chat_loop 主循环至 `subagent/dispatch.rs`)。
  The rid is **not durable** (the
  `cancellations` map is in-memory), so a hard FK would be wrong. This
  is a deliberate **soft-FK-style** column (no constraint, but the
  value is meaningful for cross-referencing cancellation/audit rows
  when the request is in flight).
- **`status` is a CHECK-constrained TEXT column** with 5 wire values
  (`running` / `completed` / `cancelled` / `error` / `incomplete`). The Rust enum
  `db::subagent_runs::SubagentStatusDb` has matching `as_str()` and
  lenient `from_str_opt()`. The CHECK constraint catches typos at INSERT
  time; the Rust enum catches forward-compat drift (unknown strings fall
  back to `Running`).
- **Timestamps are RFC 3339 TEXT** (`started_at` / `finished_at` /
  `created_at`), set via `Utc::now().to_rfc3339()`. Consistent with
  every other table in the project — no Unix-epoch integers, no SQL
  `DATETIME` type (SQLite has no native datetime).
- **`turn_count` (2026-06-22, RULE-FrontSubagent-004)**: nullable INTEGER
  carrying the **actual** completed LLM turn iterations the worker
  executed before reaching terminal state (`completed` / `cancelled` /
  `error` / `incomplete`). NOT the `SUBAGENT_MAX_TURNS=200` constant
  (that's the budget ceiling; `turn_count` is the real count at exit —
  may be < 200 on clean completion / cancel / error, or == 200 on the
  `incomplete` soft-cap exit). Sourced from
  `SubagentBufferSink::turns_completed()` (real per-turn `Done`
  increment; synthetic `cancelled` / `max_turns` terminal events do
  NOT increment — the counter is always the real turn count at exit).
  Pre-PR2 rows keep `NULL`; the drawer's `statusDisplay` checks
  `turnCount !== null && turnCount !== undefined` and falls back to
  the wall-clock `terminalDurMs` suffix for legacy rows (backward
  compat). Idempotent migration via the existing
  `add_subagent_runs_column_if_missing` helper (no DEFAULT, so legacy
  rows preserve NULL). Wire: `#[serde(rename_all = "camelCase")]`
  projects to `turnCount` on the JS side.

### Indexing

Two indexes beyond the PK:

- **`idx_subagent_runs_session_started`** on
  `(parent_session_id, started_at DESC)` — supports the PR3
  `list_runs_by_session` query (returns runs in newest-first order) and
  any per-session stats / counts. The DESC ordering matches the
  convention used by `idx_session_audit_events_session_ts`.
- **`idx_subagent_runs_request`** on `parent_request_id` — supports a
  future "look up the run for an in-flight worker rid" path (e.g. the
  user clicks "cancel" on a still-running worker; the run is found by
  rid without a session scan).

The two indexes are independent (no composite key overlap), so the
SQLite query planner can choose between them freely per query.

### `insert_run` / `update_run_finished` pattern: best-effort warn+continue

The CRUD layer's two writes (`insert_run` at worker start,
`update_run_finished` at worker end) follow the project's "audit /
metadata writes are best-effort" pattern (see `RULE-A-003` discussion in
`error-handling.md`): a `Result::Err(sqlx::Error)` from either helper
triggers `tracing::warn!` and **continues** — the dispatch_subagent
`tool_result` is the user-visible signal, and a failed persistence
shouldn't break the user's view of the worker's output.

This is different from the **normal-path** persist (initial user
message / assistant turn / tool_result) where `emit_persist_failure`
emits a typed `ChatEvent::Error{Server}` and aborts the loop. The
`subagent_runs` writes are **terminal** writes (the worker is already
done by the time `update_run_finished` is called; the user already saw
the worker streaming), so a `tracing::warn!` is the right level — the
alternative (`emit_persist_failure` mid-dispatch-result) would produce
a second terminal Error event on top of the dispatch tool_result,
violating the "exactly one terminal event per request" invariant (see
`error-handling.md §"Agent Loop Error Paths"`).

`insert_run` failure is even rarer (it runs at worker start, BEFORE
the user has waited for the worker): the practical case is the parent
session was deleted between dispatch and worker start. `tracing::warn!`
+ `format_dispatch_result(SubagentStatus::Error, "spawn failed")` gives
the user a visible "couldn't start worker" signal without crashing.

### IPC event contract: `subagent:event` + `subagent:finished`

The worker emits TWO Tauri IPC channels from `run_subagent`, both keyed by
the **DB row id** (`subagent_runs.id`, the UUID `insert_run` returns as
`worker_run_id`):

**`subagent:event`** — streamed live while the worker runs. One payload per
`SubagentBufferSink::record()` (chat_event / tool_call / tool_result /
permission_ask). Wire shape (built by `build_subagent_event_payload` in
`agent/subagent/transcript.rs`,原 `agent/subagent.rs` 整文件已拆为
`agent/subagent/{mod,sink,transcript,truncate_summary,dispatch}.rs` 5 文件,2026-06-23 完成):

```json
{ "runId": "<DB id>", "sessionId": "<parent session_id>", "kind": "<snake_case TranscriptKind>", "payload": <entry body, camelCase>, "timestamp": "<RFC 3339>" }
```

**`subagent:finished`** — one-shot terminal signal, emitted by `run_subagent`
AFTER `update_run_finished` commits (only on the `Ok(())` arm; a DB write
failure leaves the row `running` so emitting would cache a stale running row
as terminal). Wire shape (built by `build_subagent_finished_payload`):

```json
{ "runId": "<DB id>", "sessionId": "<parent session_id>", "status": "completed|cancelled|error|incomplete", "finishedAt": "<RFC 3339>" }
```

**`runId` contract (RULE — B6 PR3b hotfix, 2026-06-21)**: BOTH channels'
`runId` MUST be the `subagent_runs.id` DB row id, NOT the human-readable
`worker_rid` (`"{parent_rid}-sub-{tool_use_id}"`). The frontend
`subagentRuns` store keys `liveTranscript` / `getRunCache` by `event.runId`,
while `ToolCallCard` opens the drawer with `summary.id` (the same DB id). If
the two diverge, the drawer's `transcript` / `status` computeds look up the
wrong key and render blank + stuck-on-running — the exact bug this contract
exists to prevent. `run_subagent` threads `worker_run_id_opt` into the
`SubagentBufferSink` (fallback `worker_rid` only when `insert_run` failed —
no DB row exists, drawer can't open, so the runId value is moot).

The frontend `subagent:finished` listener flushes the transcript debounce
buffer for the run + refetches `get_subagent_run` (drawer: terminal status +
finishedAt + full transcript) and `list_subagent_runs_by_session` (card:
status). This flips the drawer / card from `running` to the terminal state
without polling. Distinct from `subagent:event` (a transcript-entry stream),
`subagent:finished` carries no `kind` / `payload` / `timestamp` and is NOT a
transcript entry — folding it into `TranscriptKind` would pollute the drawer's
transcript rendering (the `transcript` computed would list it as an entry).

### Streaming token usage vs one-shot accumulation

The `db::subagent_runs::add_token_usage_streaming` helper accumulates
per-turn `TokenUsage` into the **parent's** `sessions` table
(`input_tokens_total` / `output_tokens_total` /
`cache_creation_input_tokens` / `cache_read_input_tokens`). The choice
between streaming (per turn) and one-shot (on worker exit) is a UX
trade-off:

- **Streaming** lets the parent's UI show the token counter ticking up
  in real-time as the worker burns tokens. This is the chosen design
  (per B6 PR2 PRD §"token_usage 汇总进父 session 时机"). The parent's
  per-session total cost is accurate at any moment, not just at worker
  exit.
- **One-shot** would defer the counter update to `update_run_finished`,
  making the parent UI show a frozen counter for 30+ seconds during a
  busy worker — a confusing UX. The implementation has the streaming
  data path available (the worker emits `ChatEvent::Done { usage }` at
  each turn end), so streaming is essentially "free".

> **⚠ 2026-06-26 STALE（task 06-26-fix-token-usage-snapshot）**：本节
> 「token_usage streaming 汇总进父 session」的设计已被 **reversal**。
> worker token 现在隔离到 `subagent_runs.token_usage_json`，**不**进父
> session（实测 fold-into-parent 导致父「上下文占用 %」1.7M/100% 爆表）。
> `add_token_usage_streaming` 函数已删除（无 production callsite）。
> 父 session 的 token 改为快照语义（`update_last_turn_usage`）。下面
> streaming vs one-shot 的论证仅作历史记录，当前实现是「都不进父」。

The streaming helper is a separate function (not a flag on the
existing `add_token_usage`) because of the PR2a RULE-A-015 fix:
`add_token_usage` was incorrectly inside the `if !skip_persist { ... }`
gate, and the streaming path must not be gated. The separate function
makes the contract obvious at the call site (`run_subagent` calls
`add_token_usage_streaming` directly, no gate check).

### 4 MiB transcript cap: defense at the write boundary

`transcript_json` is a `TEXT` column with SQLite's default 1 GiB cap
— high enough that a runaway worker could store 100s of MB of
transcript per row, multiplying with concurrent workers to threaten
DB size and slow reloads. The 4 MiB cap (`TRANSCRIPT_MAX_BYTES` in
`agent/subagent/truncate_summary.rs::truncate_transcript_for_persistence`,
拆分自原 `agent/subagent.rs`,2026-06-23 整文件拆为 5 文件时分配到 truncate_summary.rs)
is applied **at the write boundary** in `run_subagent`:

1. Worker exits → `worker_sink.transcript_snapshot()` returns
   `Vec<TranscriptEntry>`.
2. `truncate_transcript_for_persistence(transcript, TRANSCRIPT_MAX_BYTES)`
   returns `(capped, truncated: bool)`. The function is pure
   (no I/O), lives next to the sink so the cap semantics are
   co-located with the type it bounds.
3. `capped` is serialized to JSON and passed to `update_run_finished`.
4. `truncated` becomes the DB column `transcript_truncated`
   (`bool` → `i64` 0/1).

The cap is **defense, not a typical concern** — a 20-turn worker's
busy transcript is ~100 KiB, three orders of magnitude below the cap.
The cap's role is to prevent a single bad worker from blowing up
the DB while still landing a degraded-but-non-empty transcript. The
`transcript_truncated=1` flag gives PR3's expand UI a signal to
display "transcript truncated, show full via..." (UX detail left to
PR3). See `tool-contract.md §"subagent_runs persistence" §8 Design
Decisions` for the full size-selection rationale.

### Audit invariant: subagent_runs writes do NOT contaminate session_audit_events

`db::subagent_runs` is **read/write isolated from the audit layer**.
The helper never calls `record_audit_event`; worker ⑨ decisions
(Tier 2 / Tier 3 / Tier 4 collapse) are captured in the transcript's
`TranscriptKind::PermissionAsk` events, which land in
`subagent_runs.transcript_json` after PR2 persistence.

The reasoning (per B6 PR1 decision 5 + PR2 §R6):

- The parent's `session_audit_events` is the audit log the **user**
  reads via the C4 `<AuditLogModal>` to understand "what ⑨ decisions
  did the **parent** agent make?". Worker ⑨ decisions are a different
  responsibility scope (the worker's autonomous choices, not the
  parent's).
- Worker reuses `parent_session_id` (so `record_audit_event(&db,
  &ctx.session_id, ...)` would write to the **parent's** audit
  table). Without the transcript-based split, the parent's C4 audit
  log would be polluted with worker decisions.
- The transcript already carries all worker emit events; replay
  reconstructs the worker's ⑨ decision history. PR3's expand UI can
  render the transcript with the PermissionAsk events highlighted
  (UX detail left to PR3).

**Closed** (RULE-A-016, closed B6 PR3a 2026-06-20): the Tier 4
`ask_path` / `ask_shell` collapse path (`if ctx.is_worker {
Decision::Deny }`) previously called `record_audit_event(ToolDenied)`
BEFORE the worker check filtered it out, landing in the parent's
`session_audit_events`. The fix (PR3a) is in
`agent/permissions/ask.rs::ask_path` (拆分自原 `permissions/mod.rs`,2026-06-23
mod.rs 拆为 8 模块,ask_path + build_ask_reason 落 ask.rs)
worker branch: the branch no longer calls `record_audit_event`; instead
it emits a `PermissionAskPayload` via the sink →
`SubagentBufferSink::emit_permission_ask` records a
`TranscriptKind::PermissionAsk` entry in the worker's transcript
(PR3 drawer can render the deny as part of the transcript). The
`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`
test was updated to assert `tool_denied count == 0` in parent audit +
`permission_ask count == 1` in worker transcript + audit delta ≤ 2;
`audit_not_polluted_by_worker` test assertion `delta == 2` is unchanged
(researcher silent allow never wrote audit anyway).

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
      normal-path persists — see "Pattern: best-effort
      warn+continue" above).
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
- The child's lifecycle is **fully in-memory and never needs to
  survive reload** (e.g. a transient LLM scratchpad). Don't add a
  table — use the `SubagentBufferSink`-style in-memory accumulator
  and don't persist.
- The child is **part of an existing table's row** (e.g. an
  additional role on the `messages` table) — extend the parent
  table rather than add a new one.

---

**Last updated**: 2026-06-27 (L3b PR1 — `worktree_path` column + `insert_run_with_id` for caller-supplied id; see "L3b PR1 additions" below). Previously 2026-06-21 (IPC event contract — `runId` must be DB id + `subagent:finished` terminal signal; B6 PR3b hotfix). Previously 2026-06-20 (B6 PR2 subagent_runs schema + audit invariant; RULE-A-016 closed B6 PR3a).

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
) -> Result<(), sqlx::Error>
```

`insert_run` (auto-generates UUID) is retained for the `db/subagent_runs_tests.rs` integration suite, but is `#[allow(dead_code)]` in the lib build (the db integration tests are not visible to `cargo check --lib` since they're a separate test target). The new `insert_run_with_id` lets `run_subagent` pre-generate the UUID (e.g. `Uuid::new_v4()`) and pass it explicitly, so the worker worktree path (`<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`) can be **derived from the id BEFORE the row is inserted** — the worktree is created first, then the DB row records its path.

### Migration: `add_subagent_runs_worktree_path_column`

Idempotent column-add migration. CHECK constraint unchanged (still `running | completed | cancelled | error | incomplete`). Index strategy unchanged (still indexed by `(parent_session_id, child_ts DESC)` per the table-level pattern). The column-add is non-breaking: existing rows get `NULL`, which is the correct "not isolated" state for pre-L3b runs.
