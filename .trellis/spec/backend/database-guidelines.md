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
