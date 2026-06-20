# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

This project uses **SQLite via `sqlx`** (compile-time unchecked — see the
`SQLX_OFFLINE` note in `app/src-tauri/Cargo.toml` if you need to regenerate
the query cache). No ORM, no migration crate, no `sea-orm` / `diesel`. All
queries are hand-written `sqlx::query(...)` / `sqlx::query_as(...)` with
explicit `bind(...)` chains. Row types are plain `#[derive(Serialize)]`
structs in `app/src-tauri/src/db.rs`; the same struct doubles as the
in-memory Rust type and the IPC payload to the frontend.

The DB lives at the Tauri `app_data_dir` and is opened with
`init_pool(path)` (see `app/src-tauri/src/db.rs`); the path is plumbed
through `AppState.db: SqlitePool` and accessed by every IPC command via
`State<'_, Arc<AppState>>`.

---

## Migrations

All schema management is in one function: `run_migrations(pool)`. It is
called from `AppState::load` on every startup and is **idempotent** —
re-running it against an existing DB is a no-op.

### Pattern: `CREATE TABLE IF NOT EXISTS`

For tables added in a fresh-DB flow, use `CREATE TABLE IF NOT EXISTS
...` so a greenfield install and a DB-upgrade install see the same
result.

```rust
sqlx::query(
    r#"
    CREATE TABLE IF NOT EXISTS projects (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL,
        ...
    )
    "#,
)
.execute(pool)
.await?;
```

### Pattern: `add_*_column_if_missing` for `ALTER TABLE`

When adding a column to an existing table (the multi-step migration flow
this project uses — step 3b-1, step 4, step 4 follow-up, PR1 of
multi-model, …), use the `add_*_column_if_missing` helper:

```rust
add_session_column_if_missing(pool, "worktree_path", "TEXT").await?;
add_session_column_if_missing(
    pool,
    "worktree_state",
    "TEXT NOT NULL DEFAULT 'none'",
)
.await?;
```

The helper probes `PRAGMA table_info(<table>)` first because SQLite 3.35
does not have `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` reliably. Always
make the new column **nullable** OR provide a `DEFAULT` so pre-existing
rows survive the migration. **Never** add a `NOT NULL` column without
`DEFAULT` — it will fail on any non-empty table.

### Pattern: one-time data backfill after ALTER

Some ALTERs require post-fill. Use a single `UPDATE ... WHERE ...` after
the column add. Idempotent: re-running on a partially-filled DB is fine.

```sql
UPDATE sessions
   SET worktree_state = 'active'
 WHERE worktree_path IS NOT NULL
   AND (worktree_state IS NULL OR worktree_state = '');
```

### Pattern: idempotent seed on first run

For first-install defaults (PR1 of multi-model seeded 2 providers + 4
models + `default_model_id`), gate the seed on a count check. The
function becomes a no-op once the user has any data, so re-running is
safe.

```rust
pub async fn seed_default_providers_and_models(
    pool: &SqlitePool,
) -> Result<(), sqlx::Error> {
    let count: i64 = sqlx::query("SELECT COUNT(*) FROM providers")
        .fetch_one(pool).await?.try_get(0)?;
    if count > 0 { return Ok(()); }
    // ... insert defaults ...
    Ok(())
}
```

Call this at the **end** of `run_migrations` so all the new tables
exist before the seed tries to `INSERT` into them.

---

## Naming Conventions

| Element        | Convention                                      | Example                          |
|----------------|-------------------------------------------------|----------------------------------|
| Table names    | plural snake_case                               | `projects`, `providers`, `models`|
| Column names   | singular snake_case                             | `project_id`, `display_name`     |
| Primary key    | `id TEXT PRIMARY KEY` (UUID v4 string)          | `id = Uuid::new_v4().to_string()`|
| Foreign key    | `<singular_referenced_table>_id TEXT`           | `provider_id`, `project_id`      |
| Boolean column | `INTEGER NOT NULL DEFAULT 0` (not BOOLEAN)      | `supports_thinking`, `is_git_repo`|
| Enum column    | `TEXT` + Rust enum + `from_str_opt` lenient parse| `protocol`, `worktree_state`     |
| Timestamp      | `TEXT NOT NULL` (RFC 3339)                      | `created_at`, `updated_at`       |

Why `TEXT` for booleans / enums / timestamps: SQLite has no native
boolean / enum / datetime types. The conventional mapping is documented
in the table above and is **always** paired with the matching Rust type
in `db.rs`. Don't introduce `INTEGER` booleans or `INTEGER` Unix
timestamps — they conflict with the existing pattern.

---

## Enum pattern: lenient parse for forward-compat

When a column is constrained to a Rust enum (`WorktreeState`,
`ProviderProtocol`), the DB stores the string and the Rust enum is
reified on read. The enum implements both directions:

```rust
impl ProviderProtocol {
    pub fn as_str(&self) -> &'static str {
        match self { Self::Anthropic => "anthropic", Self::Openai => "openai" }
    }
    /// Unknown strings fall back to the default; new variants added
    /// in a future binary don't crash an older binary reading newer DBs.
    pub fn from_str_opt(s: &str) -> Self {
        match s {
            "openai" => Self::Openai,
            _ => Self::Anthropic,
        }
    }
}
```

Writes use the string form (`protocol: &str` in `create_provider`),
reads use the enum. Lenient parse is critical for forward-compat: a
release that adds a new protocol variant must not crash users on the
old release.

See `WorktreeState` (`db.rs:65`) for the original instance of this
pattern.

---

## Soft FK pattern (deliberate, not a bug)

Some FK columns are intentionally **not** declared with `REFERENCES ...`
constraints even though the schema looks like it should. The
`sessions.model_id` column (PR1 of multi-model) is one such example:

> `sessions.model_id` is a soft FK to `models.id` (no FK constraint)
> so a legacy dump with a dangling `model_id` doesn't break INSERTs and
> PR2 can wire a resolve-default fallback in the agent loop.

**When to use a soft FK**:
- The column was added in a later migration and you want to keep the
  `ALTER TABLE` backward-compatible with already-existing data.
- The `id` value may legitimately be missing (`NULL` is meaningful —
  e.g. "session not yet attached to a model").
- The consumer (read path) is responsible for the fallback / join;
  you don't want the DB to reject inserts on a missing reference.

**When to use a hard FK** (with `ON DELETE CASCADE` etc.):
- The referenced table is the sole owner of the row (e.g. `models`
  cascades to `providers` because a model without a provider is
  meaningless).

This split is documented in `db.rs:130-150` next to the
`models.provider_id` column.

---

## Pattern: `update_message_metadata` for post-persist metadata patches (B2 PR3, 2026-06-17)

The `messages.metadata TEXT` JSON column is the right place for
"computed-after-the-fact" message-level state — anything that the
agent loop only knows AFTER `persist_turn` has already happened.
The `persist_turn` signature accepts an `Option<serde_json::Value>`
parameter, but the value must be **available at persist time** to
use that parameter.

When the metadata is computed **after** the message is persisted
(B2 PR3: the `@`-file injection manifest is computed in
`inject_at_tokens`, which runs after `persist_turn` to keep the
DB row's `content` as the source of truth at the original
`@relpath` form), use a separate `update_message_metadata`
function instead:

```rust
// app/src-tauri/src/db/sessions.rs
pub async fn update_message_metadata(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    metadata: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE messages
           SET metadata = ?,
               updated_at = datetime('now')
         WHERE session_id = ?
           AND seq = ?
        "#,
    )
    .bind(&metadata)
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}
```

### When to use which path

| Path | When |
|---|---|
| `persist_turn(..., metadata: Some(...))` | Metadata is known at the moment of INSERT (e.g. worktree event metadata, latency breakdown, role-based flags). |
| `update_message_metadata(session_id, seq, json)` | Metadata is computed AFTER the message is persisted (e.g. injection manifest, future "post-render preview" data). |

### Rules

- `update_message_metadata` keys on `(session_id, seq)` (the
  stable handle the agent loop already has), not the auto-increment
  `id`. This keeps the call site close to the agent loop without
  forcing a `find_message_id_by_seq` round-trip.
- The `UPDATE` is single-row by composite key. If the row doesn't
  exist (race between cancel and persist), the update is a no-op
  and `Result::Ok(())` is returned — same defensive no-op
  pattern as `record_tool_duration` (F5).
- Bump `updated_at` on the row so observers can see "this row
  was patched post-insert".
- Never use `update_message_metadata` to **fix up** the `content`
  column — `content` is the source of truth, the `metadata` column
  is the parallel channel. The agent loop persists the original
  `@relpath` in `content`; the `metadata` carries the post-inject
  manifest. The two are read together at rehydrate time.

### Why a separate function (not just `persist_turn` with a 2nd call)

A second `persist_turn` call on the same `(session_id, seq)`
would either:
1. Conflict on the unique key (FK error), or
2. Require a separate "is this an insert or an update?" branch in
   the SQL.

The single-purpose `update_message_metadata` keeps the call site
explicit and the SQL trivial.

### Tests

| Test | Asserts |
|---|---|
| `update_message_metadata_writes_json_to_column` | A call with `serde_json::json!({"injections": [...]})` lands in the `metadata` column verbatim. |
| `update_message_metadata_bumps_updated_at` | The row's `updated_at` advances past `created_at`. |
| `update_message_metadata_on_unknown_seq_is_noop` | A `(session_id, seq)` pair with no matching row returns `Ok(())` with no error. |
| `rehydrate_reads_injections_from_metadata` | The rehydrate path (e.g. `load_session` + `streamController.ts` `metadata` parse) surfaces the injected field back to the UI. |

---

## Pattern: `edit_user_message` — in-place edit + cascade delete + audit (D3 PR1, 2026-06-17)

Editing a user message requires **three** changes in one logical
step: replace the row's `content`/`text`, wipe every strictly-later
message in the session (so the next resend starts from a clean
slate — assistant `tool_use` blocks on row N+1+ reference the old
prompt), and append an audit row. `db::sessions::edit_user_message`
wraps all three in a single `sqlx::Transaction` so a partial
failure cannot leave the DB in a split-brain state (e.g. content
updated but tail not deleted → assistant turn still references the
old prompt).

### When to use

A user-driven IPC that wants to mutate the content of an existing
user message AND clear everything the LLM generated downstream of
it. The function is intentionally **not** exposed for assistant
message edits — see "Don't edit assistant messages" below.

### Schema impact

**Zero**. The edit re-uses the existing `messages.content` /
`text` / `metadata` columns. `metadata` carries the per-edit
fields (`edited_at` + `original_content`) — the same JSON column
that `update_message_metadata` writes. No new migration needed.

### Persisted shape after one edit

```jsonc
// messages.metadata JSON, first edit:
{
  "edited_at": "2026-06-17T12:34:56+00:00",
  "original_content": "old prompt text"
}

// after a SECOND edit (original_content preserved; edited_at bumped):
{
  "edited_at": "2026-06-17T13:00:00+00:00",
  "original_content": "old prompt text"  // unchanged — points at pre-ANY-edit value
}
```

### Implementation (`db::sessions::edit_user_message`)

```rust
pub async fn edit_user_message(
    pool: &SqlitePool,
    session_id: &str,
    message_seq: i64,
    new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
    // 1. Resolve (session_id, seq) → id via find_message_id_by_seq.
    //    Unknown pair → silent Ok(()) (defensive no-op, matches F5
    //    latency IPC contract).
    let message_id = match find_message_id_by_seq(pool, session_id, message_seq).await? {
        Some(id) => id,
        None => return Ok(()),
    };

    let mut tx = pool.begin().await?;

    // 2. Read current content (for the no-op check + backup).
    let current_content_str: Option<String> = sqlx::query_scalar(
        "SELECT content FROM messages WHERE id = ? AND session_id = ?",
    )
    .bind(message_id)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current_content_str = match current_content_str {
        Some(s) => s,
        None => {
            tx.rollback().await?;
            return Ok(());  // row vanished mid-transaction
        }
    };

    // 3. No-op fast path: same JSON → return without writing.
    let new_content_json = serde_json::to_string(new_content)?;
    if new_content_json == current_content_str {
        tx.rollback().await?;
        return Ok(());
    }

    // 4. Compute the metadata patch (edited_at always; original_content
    //    only on the first edit). Uses SQLite's `json_patch` (RFC 7396)
    //    so a `COALESCE(metadata, '{}')` falls back to a fresh object.
    let now = Utc::now().to_rfc3339();
    let existing_edited_at: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(metadata, '$.edited_at') FROM messages WHERE id = ?",
    )
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await?;
    let metadata_patch = if existing_edited_at.is_some() {
        serde_json::json!({ "edited_at": &now }).to_string()
    } else {
        let original_content_value = serde_json::from_str(&current_content_str)
            .unwrap_or_else(|_| serde_json::Value::String(current_content_str.clone()));
        serde_json::json!({
            "edited_at": &now,
            "original_content": original_content_value,
        })
        .to_string()
    };
    let new_metadata_json: String = sqlx::query_scalar(
        "SELECT json_patch(COALESCE(metadata, '{}'), ?) FROM messages WHERE id = ?",
    )
    .bind(&metadata_patch)
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await?;

    // 5. UPDATE the row: content + text + metadata.
    sqlx::query(
        "UPDATE messages SET content = ?, text = ?, metadata = ? \
         WHERE id = ? AND session_id = ?",
    )
    .bind(&new_content_json)
    .bind(new_content.to_text())
    .bind(&new_metadata_json)
    .bind(message_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    // 6. Cascade-delete every strictly-later message in the session.
    //    `messages` has no outgoing FKs to other tables (only an index
    //    on `(session_id, seq)`), so a single DELETE is enough. Audit
    //    events (`session_audit_events`) are session-scoped and kept —
    //    they record what the agent DID, not the live message buffer.
    sqlx::query("DELETE FROM messages WHERE session_id = ? AND seq > ?")
        .bind(session_id)
        .bind(message_seq)
        .execute(&mut *tx)
        .await?;

    // 7. Audit row: kind='edit_message' (string literal — the cross-
    //    module call graph stays tight; matches `set_session_mode`'s
    //    `mode_changed` audit pattern).
    let audit_payload = serde_json::json!({
        "message_seq": message_seq,
        "new_text_preview": new_content.to_text().chars().take(80).collect::<String>(),
        "edited_at": &now,
    })
    .to_string();
    sqlx::query(
        "INSERT INTO session_audit_events (session_id, ts, kind, payload_json) \
         VALUES (?, datetime('now'), 'edit_message', ?)",
    )
    .bind(session_id)
    .bind(&audit_payload)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}
```

### Tauri command wiring

The IPC layer is `commands::sessions::edit_user_message`. It:

1. Cancels any in-flight chat on the session via
   `cancel_inflight_for_session` + `await_inflight_exit` (the
   same pattern `delete_session` / `clear_session_messages` use).
   Without this gate, an in-flight `persist_turn` could race the
   cascade DELETE.
2. Confirms the session exists + the (session_id, seq) pair
   resolves to a user-role row (explicit error to the frontend,
   even though the DB-layer helper is silent).
3. Delegates to `db::sessions::edit_user_message`. Errors wrap
   as `String` for the IPC rejection.

### Don't edit assistant messages

Editing assistant messages is intentionally NOT supported in D3
PR1. Reasons:

- Assistant turns can carry `tool_use` blocks with stable
  `tool_use_id`s that downstream `tool_result` turns reference.
  Mutating the assistant content without rewriting every
  following `tool_result` would produce an orphan-request /
  orphan-result pair (Anthropic returns `400` on the next turn).
- The industry consensus is "user-only" (Cursor / Cline / Cody /
  OpenHands / OpenCode / OpenCode / ChatGPT — see
  `.trellis/tasks/06-17-d3-message-edit-resend/research/industry-edit-resend.md`).
- A future PR can wire assistant edits as a higher-level
  "rewrite turn" operation that cascades the rewrite into the
  dependent `tool_result` rows. Out of scope for PR1.

### Permission / audit

Edit is a **user-initiated direct IPC** — not an LLM tool
invocation. The ⑨ 关 permission layer (Tier 2 deny list, Tier 3
"始终允许" check, Tier 4 Mode interception) does **not** apply.
The audit log captures every edit (`session_audit_events` row
with `kind='edit_message'`) so the user can review changes later.

### Tests (D3 PR1, `db/tests.rs`)

| Test | Asserts |
|---|---|
| `edit_user_message_cascade_deletes_subsequent_messages` | 3-turn session → after edit, only the edited row survives; cascade wipes assistant + tool_result |
| `edit_user_message_writes_edited_at_metadata` | First edit stamps a non-empty `edited_at` (RFC3339, contains "T") |
| `edit_user_message_preserves_original_content_on_first_edit` | `metadata.original_content` equals the pre-edit value verbatim |
| `edit_user_message_preserves_original_across_subsequent_edits` | Re-edit does NOT overwrite `original_content`; it stays at the pre-ANY-edit value |
| `edit_user_message_records_audit_event` | `session_audit_events` has 1 row with `kind='edit_message'` + JSON payload carrying `message_seq` / `new_text_preview` / `edited_at` |
| `edit_user_message_is_noop_when_content_unchanged` | Save-without-change: 3 turns intact, `metadata` null, no audit row |
| `edit_user_message_on_unknown_seq_is_silent_noop` | Unknown `(session_id, seq)` → Ok(()) with no error; original history intact |
| `edit_user_message_atomic_rollback_on_db_error` | Synthetic `RAISE(FAIL)` trigger on audit INSERT → entire transaction rolls back; 3 turns intact, no audit row committed |

---

## Pattern: `record_message_resend_audit` — user-initiated resend audit (D3 PR3, 2026-06-17)

The Resend feature (D3 PR3) re-fires an existing user prompt — no
content mutation, no cascade delete, just a fresh LLM stream over
the same prompt. The audit row exists so the user can review
"which prompts did I re-run at which timestamp" in the C4
`<AuditLogModal>` alongside the EditMessage rows (D3 PR1).

Unlike `edit_user_message` (which is destructive and lives inside a
transaction), `record_message_resend_audit` is **best-effort, post-
persist**. The agent loop's user-message persist site is the
natural place to fire it (the user message is in the DB; we know
its seq), but the audit failure must NOT abort the chat — the user
has already seen the visual confirmation (the new assistant turn
is about to stream). The helper returns `Result<(), sqlx::Error>`
and the caller (`chat_loop.rs` user persist site) wraps it in
`tracing::warn!` + swallow.

### When to use

A user-driven IPC that wants to record a "resend" event on an
existing user message without modifying the message content or
deleting downstream rows. The function is intentionally NOT used
for assistant messages — there's no defined "resend" semantics
on an assistant response (the user re-runs the user prompt that
spawned it).

### Schema impact

**Zero**. The audit row re-uses the existing
`session_audit_events` table. The `kind` column is plain TEXT; a
new wire string (`"resend_message"`) requires no migration.

### Persisted shape

```jsonc
// session_audit_events.payload_json, one row per resend click:
{
  "message_seq": 3,                                    // seq of the ORIGINAL user message being re-run
  "content_text_preview": "re-run: explain the cancellation token pattern"  // first 80 chars of the user prompt
}
```

### Implementation (`agent::permissions::record_message_resend_audit`)

```rust
// app/src-tauri/src/agent/permissions/mod.rs
pub async fn record_message_resend_audit(
    db: &SqlitePool,
    session_id: &str,
    message_seq: i64,
    content_text_preview: &str,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "message_seq": message_seq,
        "content_text_preview": content_text_preview.chars().take(80).collect::<String>(),
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ResendMessage.as_str(),  // "resend_message"
        Some(&payload_str),
    )
    .await
}
```

### Tauri command wiring

`record_message_resend_audit` is NOT wired as a Tauri command —
it's called directly from `chat_loop.rs`'s user-message persist
site (the `chat` IPC accepts an optional `resendSeq: Option<i64>`
parameter; when `Some(seq)`, the persist site fires this helper
after `persist_turn` succeeds). The IPC signature is
`#[tauri::command] pub async fn chat(..., resendSeq: Option<i64>)`
in `app/src-tauri/src/agent/chat.rs` — see the "Resend 走 chat
IPC metadata flag, 不引入新 IPC" decision in `docs/IMPLEMENTATION.md §4`
"D3 完成 ADR" (2026-06-17).

### Permission / audit

Resend is a **user-initiated direct IPC** — not an LLM tool
invocation. The ⑨ 关 permission layer (Tier 2 deny list, Tier 3
"始终允许" check, Tier 4 Mode interception) does **not** apply.
The audit log captures every resend (`session_audit_events` row
with `kind='resend_message'`) so the user can review re-run
history later.

### Diff vs `edit_user_message`

| 维度 | `edit_user_message` (D3 PR1) | `record_message_resend_audit` (D3 PR3) |
|---|---|---|
| **Destructive?** | Yes (in-place content update + cascade delete + 3-step transaction) | No (read-only; just writes an audit row) |
| **Transaction?** | Yes (single `sqlx::Transaction` wrapping UPDATE + DELETE + INSERT audit) | No (single audit INSERT; best-effort, non-fatal) |
| **Caller failure mode** | On failure → rollback + `emit_persist_failure` | On failure → `tracing::warn!` + swallow |
| **Wire string** | `"edit_message"` | `"resend_message"` |
| **Helper location** | `db::sessions::edit_user_message` (DB layer) | `agent::permissions::record_message_resend_audit` (permissions layer, like `record_tool_executed_audit`) |
| **Tauri command?** | Yes (`edit_user_message`) | No (called inline from `chat_loop.rs`) |
| **Audit trigger site** | Inside the edit transaction's commit | After `persist_turn` succeeds (best-effort) |

### Tests (D3 PR3, `db/tests.rs`)

| Test | Asserts |
|---|---|
| `resend_message_audit_round_trips_via_list_audit_events` | (1) short content → `kind='resend_message'`, payload carries `message_seq` + verbatim preview; (2) 200-char content → preview truncated to 80 chars |
| `resend_message_audit_on_deleted_session_returns_error` | Helper returns `Err(sqlx::Error::RowNotFound)` on FK violation (defensive; agent loop log-and-swallow path) |

---

## Pattern: denormalized list endpoints

When the UI renders a list with parent-table fields (e.g. a model picker
that groups models under their provider's `display_name`), do **not**
require the frontend to do a second IPC roundtrip. Add a `With*` wrapper
struct that flattens the parent fields onto the row:

```rust
pub struct ModelWithProvider {
    #[serde(flatten)]
    pub model: ModelRow,
    pub provider_display_name: String,
    pub provider_protocol: String,
}
```

The `#[serde(flatten)]` keeps the row's own fields at the top level in
the JSON payload (so the frontend reads `mwp.model.id` OR
`mwp.id` — both work). The parent fields are appended at the same
level. The `list_*` query does a `JOIN` to populate them.

---

## Error handling

- Internal DB functions return `Result<T, sqlx::Error>`.
- Tauri IPC commands (in `lib.rs`) wrap these and return
  `Result<T, String>` via `.map_err(|e| format!("<command> failed: {}", e))`.
- Tests use `.unwrap()` freely; production code must not `.unwrap()` on
  DB results.

---

## Tests

Tests live in `#[cfg(test)] mod tests` at the bottom of `db.rs`. Use
`test_pool()` (in-memory SQLite) and call `run_migrations(&pool)` to
bootstrap:

```rust
async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON").execute(&pool).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}
```

`PRAGMA foreign_keys = ON` is required for `ON DELETE CASCADE` to take
effect (SQLite defaults to OFF per-connection). **Don't forget it** —
without it the cascade tests pass incorrectly (the rows don't get
deleted but the FK violation is also off, so the test silently
asserts nothing).

Each CRUD function should have a happy-path test + at least one
edge-case test (missing id, dangling FK, etc.). The `seed_*` function
should have an idempotency test that runs it twice and asserts the
catalog count is unchanged.

---

## Common Mistakes

### Don't: drop the `signature` (or any opaque blob) to "save space"

The `signature` on a `ContentBlock::Thinking` is a cryptographic
anchor for Anthropic. Drop it and the next turn 400s. The DB stores
it in full; the rehydrate path emits it in full. The same pattern
applies to any opaque blob the upstream API hands back — store
verbatim, emit verbatim. See
`backend/llm-contract.md` for the Anthropic-specific list.

### Don't: `NOT NULL` column without `DEFAULT` on existing tables

`ALTER TABLE ... ADD COLUMN foo TEXT NOT NULL` will fail on any
non-empty table. Either:
1. Make the column nullable and treat `NULL` as a meaningful value
   ("not yet attached"), OR
2. Add a `DEFAULT` so existing rows get a sensible value.

PR1's `sessions.model_id` is nullable (the seed backfills it on first
run after the migration). `worktree_state` has a `DEFAULT 'none'`.

### Don't: add a `FOREIGN KEY` constraint to a soft-FK column

Soft FKs are soft for a reason — see "Soft FK pattern" above. If you
need a hard FK, declare it explicitly with `REFERENCES ... ON DELETE
CASCADE` and run `PRAGMA foreign_keys = ON` on every connection
(connection-scoped pragma, easy to forget).

### Don't: re-implement the `make_pool` alias

A new test section that needs an in-memory pool should call
`test_pool()` (or the existing `make_pool` alias) — `test_pool` already
calls `run_migrations` and enables `PRAGMA foreign_keys = ON`. Defining
your own helper risks missing one of those two critical steps.

### Don't: emit `signature_delta` per SSE event (LLM streaming)

`signature_delta` is buffered in the `BlockState` state machine and
emitted as a single `ChatEvent::SignatureDelta` on `content_block_stop`.
Per-event emit was the step 6 v1 implementation; the check phase caught
it because the upstream might split the signature across N events and
a per-event emit would scatter chunks across N thinking blocks.

---

## When you add a new user-managed catalog (checklist)

The `providers` / `models` pair is a template for future user-managed
catalogs (e.g. `roles` / `agents` per BACKLOG §4). When you add a new
one, walk this checklist:

- [ ] Two tables: parent (e.g. `providers`) + child (e.g. `models`)
      with `child.parent_id REFERENCES parent(id) ON DELETE CASCADE`
- [ ] Both `parent_id` and the parent's `display_name` are surfaced in
      the child's denormalized list view (`ModelWithProvider`) so the
      frontend doesn't need a second IPC roundtrip
- [ ] Optional fields use `Option<T>` and `NULL` (e.g.
      `max_tokens INTEGER`, `thinking_effort TEXT`) — never `NOT NULL`
      with sentinel empty string
- [ ] Booleans are `INTEGER NOT NULL DEFAULT 0`, not `BOOLEAN`
- [ ] Enums are `TEXT` + Rust enum + `from_str_opt` lenient parse
- [ ] `created_at` + `updated_at` are `TEXT NOT NULL` (RFC 3339)
- [ ] `id` is `TEXT PRIMARY KEY` (`Uuid::new_v4().to_string()`)
- [ ] All `Serialize` structs that cross the IPC boundary have
      `#[serde(rename_all = "camelCase")]` (Tauri 2 default is
      snake_case, JS expects camelCase)
- [ ] All IPC args use Rust snake_case (Tauri 2 auto-converts from JS
      camelCase — verified in HACKING-wsl FU-4)
- [ ] At least one happy-path test + one error-path test per CRUD
      function; cascade test for `ON DELETE CASCADE` parent
- [ ] `PRAGMA foreign_keys = ON` in `test_pool` so cascade tests are
      real

---

## Pattern: `match_kind` discriminator for permission tables (added 2026-06-13)

> **Source**: A2+B7 task `06-12-a2-b7-permission-and-mode` + re-grill
> `06-13-a2-b7-regrill-path-based` (path-based model).

The `session_tool_permissions` table uses a `match_kind` TEXT
column with a CHECK constraint to discriminate between the
three kinds of "always allow" rows the user can grant:

```sql
CREATE TABLE session_tool_permissions (
    session_id TEXT NOT NULL,
    tool_name  TEXT NOT NULL,
    match_kind TEXT NOT NULL CHECK (match_kind IN ('tool', 'prefix', 'path')),
    match_value TEXT,           -- NULL for 'tool', command-prefix or glob otherwise
    granted_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (session_id, tool_name, match_kind, match_value),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
```

### Why `match_kind` and not a separate table

Three reasons:

1. **Same query path.** The Tier 4 lookup is
   `SELECT match_value FROM session_tool_permissions WHERE
    session_id = ? AND tool_name = ? AND match_kind = ?`.
   A separate table would either force the same column
   triplet on every tool-type table (3 × duplicate) or
   a UNION across tables (3 × the indexes).
2. **Uniqueness by composite key.** `(session_id,
   tool_name, match_kind, match_value)` is the natural
   composite key — a user shouldn't be able to grant
   "the same path-glob twice" any more than "the same
   prefix twice". Splitting across tables would lose
   this natural UNIQUE invariant.
3. **Single `ON DELETE CASCADE` foreign key.** All three
   kinds are session-scoped, so one `sessions(id)` FK
   covers everything. Splitting would force 3 FKs to
   maintain.

### Storage conventions per kind

| `match_kind` | `match_value` | Example | Used for |
|---|---|---|---|
| `tool` | `NULL` | `NULL` | `web_fetch`, future tool-level grants |
| `prefix` | first whitespace token of the shell command | `'cargo'` | Shell command prefixes (whitelist/asklist entries) |
| `path` | sqlite GLOB pattern (parent + `/*`) | `'/Users/me/Documents/*'` | Path tools (read/write/edit/list_dir/grep/glob) |

The GLOB in `path` uses sqlite GLOB semantics (`*` does
NOT cross `/`, `?` matches one char). The re-grill PRD
explicitly accepts that `**` recursion is **not** supported
(`out of scope`).

### Indexing

The PK `(session_id, tool_name, match_kind, match_value)`
covers the Tier 4 query as a covering index. No extra
index is needed for the MVP.

### PR1 vs re-grill diff

PR1 of A2+B7 wrote only `match_kind='tool'`, `match_value=NULL`.
The re-grill task wires the 3-variant schema. The CHECK
constraint was already in place (it was reserved in the
original schema), so no migration is needed.

---

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
    status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error')),
    started_at TEXT NOT NULL,                                       -- RFC 3339, set on INSERT
    finished_at TEXT,                                               -- NULL = running, set on UPDATE
    token_usage_json TEXT,                                          -- JSON TokenUsage { input / output / cache_creation / cache_read }
    summary TEXT,                                                   -- final_text 纯文本 (NO status prefix; status 字段独立)
    transcript_json TEXT,                                           -- JSON Vec<TranscriptEntry> after 4 MiB cap
    transcript_truncated INTEGER NOT NULL DEFAULT 0,                -- 1 = over 4 MiB cap
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
  for the cascade to fire. Cascade test in `db/tests.rs`:
  `subagent_runs_cascade_delete_with_parent_session`.
- **`parent_request_id` is a TEXT NOT NULL with NO FK** — it carries
  the worker rid (the `"{parent_rid}-sub-{seq}"` string the agent loop
  builds at `chat_loop.rs:1989`). The rid is **not durable** (the
  `cancellations` map is in-memory), so a hard FK would be wrong. This
  is a deliberate **soft-FK-style** column (no constraint, but the
  value is meaningful for cross-referencing cancellation/audit rows
  when the request is in flight).
- **`status` is a CHECK-constrained TEXT column** with 4 wire values
  (`running` / `completed` / `cancelled` / `error`). The Rust enum
  `db::subagent_runs::SubagentStatusDb` has matching `as_str()` and
  lenient `from_str_opt()`. The CHECK constraint catches typos at INSERT
  time; the Rust enum catches forward-compat drift (unknown strings fall
  back to `Running`).
- **Timestamps are RFC 3339 TEXT** (`started_at` / `finished_at` /
  `created_at`), set via `Utc::now().to_rfc3339()`. Consistent with
  every other table in the project — no Unix-epoch integers, no SQL
  `DATETIME` type (SQLite has no native datetime).

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
`agent/subagent.rs::truncate_transcript_for_persistence`) is applied
**at the write boundary** in `run_subagent`:

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

**Known follow-up** (RULE-A-016, open as of 2026-06-20): the Tier 4
`ask_path` / `ask_shell` collapse path (`if ctx.is_worker {
Decision::Deny }`) still calls `record_audit_event(ToolDenied)` BEFORE
the worker check filters it out. With PR2b's `is_worker` threading
in place, the collapse fires correctly — but the `record_audit_event`
call lands in the parent's `session_audit_events` because it's
unconditional inside the collapse branch. The fix is ~5 lines in
`permissions/mod.rs::ask_path` (gate the audit on `!ctx.is_worker`),
and PR3 needs to extend the
`audit_not_polluted_by_worker` test to cover the
`general-purpose + Edit + Tier 4` scenario. PR2 leaves this for a
follow-up; the persistence layer itself is correct, and the
pollution is a UX / audit-integrity issue, not a correctness issue.

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
      test per CRUD function** (7 tests in `db/tests.rs` for
      PR2a; same density recommended for future child-context
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

**Last updated**: 2026-06-20 (B6 PR2 subagent_runs schema + audit invariant).
