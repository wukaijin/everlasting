//! One-shot data migrations + path helpers used by the schema bootstrap.
//!
//! Split out of `db/migrations.rs` (2026-08-08 batch3).

use chrono::Utc;
use sqlx::{Row, SqlitePool};

/// RULE-D-001 (2026-06-24): one-shot idempotent migration of provider
/// api_keys from plaintext (`providers.api_key`) to encrypted
/// (`providers.api_key_enc`).
///
/// For every row with `api_key <> '' AND key_migrated_at IS NULL`:
/// read the plaintext, `crypto::encrypt(.., aad = provider id)` it,
/// write the ciphertext to `api_key_enc`, blank the old `api_key`
/// column, and stamp `key_migrated_at`. The `WHERE` clause makes it
/// idempotent — re-running on a fully-migrated DB is a no-op, and a
/// mid-migration crash leaves partially-migrated rows that resume on
/// the next startup (each row's UPDATE is its own transaction).
///
/// If `derive_master_key` fails (e.g. `/etc/machine-id` unreadable)
/// we log + return early WITHOUT blanking any plaintext — secrets
/// stay recoverable and the migration retries next boot. Per-row
/// encrypt failures likewise skip the row (kept plaintext) and retry.
pub(crate) async fn migrate_provider_api_keys_to_encrypted(
    pool: &SqlitePool,
) -> Result<(), sqlx::Error> {
    let master_key = match crate::crypto::derive_master_key() {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!(
            error = %e,
            "api_key migration: derive master key failed; keeping plaintext, will retry next startup"
            );
            return Ok(());
        }
    };
    let rows = sqlx::query(
        r#"
 SELECT id, api_key FROM providers
 WHERE api_key <> '' AND key_migrated_at IS NULL
 "#,
    )
    .fetch_all(pool)
    .await?;
    let now = Utc::now().to_rfc3339();
    let mut migrated = 0usize;
    for row in rows {
        let id: String = row.try_get("id")?;
        let plain: String = row.try_get("api_key")?;
        match crate::crypto::encrypt(&master_key, &plain, &id) {
            Ok(enc) => {
                sqlx::query(
                    r#"
 UPDATE providers
 SET api_key_enc = ?, api_key = '', key_migrated_at = ?
 WHERE id = ?
 "#,
                )
                .bind(&enc)
                .bind(&now)
                .bind(&id)
                .execute(pool)
                .await?;
                migrated += 1;
            }
            Err(e) => {
                tracing::warn!(
                provider_id = %id,
                error = %e,
                "api_key migration: encrypt failed; skipping row (will retry next startup)"
                );
            }
        }
    }
    if migrated > 0 {
        tracing::info!(
            migrated,
            "provider api_keys encrypted (RULE-D-001 migration)"
        );
    }
    Ok(())
}

/// Widen the `subagent_runs.status` CHECK constraint to include
/// `'incomplete'` (the 2026-06-21 max_turns soft-terminal variant).
/// SQLite has no `ALTER TABLE ... DROP CONSTRAINT` /
/// `ALTER TABLE ... ADD CONSTRAINT` for CHECK expressions, so the
/// only reliable way to widen the constraint is the 12-step
/// table-rebuild pattern:
///
/// 1. Rename `subagent_runs` → `subagent_runs_old`.
/// 2. `CREATE TABLE subagent_runs (...)` with the wider CHECK.
/// 3. `INSERT INTO subagent_runs SELECT * FROM subagent_runs_old`.
/// 4. `DROP TABLE subagent_runs_old`.
/// 5. Re-create the two indexes (`idx_subagent_runs_session_started` +
///    `idx_subagent_runs_request`) — they were not transferred by
///    the rebuild because they were attached to `_old`.
///
/// **Idempotency**: the function probes `sqlite_master.sql` for
/// the literal `'incomplete'` in the `subagent_runs` CREATE
/// statement. If it's already there, the function returns
/// `Ok(())` without rebuilding. A re-run on a dev DB that
/// already has the widened constraint is therefore a no-op.
///
/// **FK safety**: this function does NOT toggle
/// `PRAGMA foreign_keys`. The standard 12-step pattern requires
/// `PRAGMA foreign_keys=OFF` because the rebuild temporarily
/// creates a window where FK references could fire (e.g. if
/// some other table referenced `subagent_runs`). The
/// `subagent_runs` table has NO outgoing FK references (the
/// only FK is `parent_session_id REFERENCES sessions(id)` on
/// the column itself, which keeps pointing at the same
/// `sessions` rows throughout the rebuild) and NO incoming FK
/// references (no other table references `subagent_runs`).
/// Toggling `PRAGMA foreign_keys=OFF` is therefore unnecessary,
/// and skipping it avoids polluting the per-connection pragma
/// state of the test pool (which uses multiple connections —
/// setting the pragma on one connection doesn't propagate to
/// the others, and the test pool's `PRAGMA foreign_keys=ON` is
/// per-connection, so a toggle on one connection can leave
/// other connections in an inconsistent state across
/// concurrently-running tests).
pub(crate) async fn widen_subagent_runs_status_check_for_incomplete(
    pool: &SqlitePool,
) -> Result<(), sqlx::Error> {
    // Probe the live CREATE statement for the `incomplete` literal.
    // A re-run on a dev DB that already has the widened CHECK sees
    // the literal and short-circuits.
    let sql_row: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'subagent_runs'",
    )
    .fetch_optional(pool)
    .await?;
    let already_widened = sql_row
        .as_deref()
        .map(|s| s.contains("'incomplete'"))
        .unwrap_or(false);
    if already_widened {
        return Ok(());
    }

    // Probe failed (table missing entirely or constraint narrow).
    // Rebuild via the table-rebuild dance. Both are no-ops when the
    // condition doesn't apply.
    // Step 1: rename existing.
    sqlx::query("ALTER TABLE subagent_runs RENAME TO subagent_runs_old")
        .execute(pool)
        .await
        .ok(); // benign if the table doesn't exist
               // Step 2: create the widened table.
    sqlx::query(
        r#"
 CREATE TABLE subagent_runs (
 id TEXT PRIMARY KEY,
 parent_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
 parent_request_id TEXT NOT NULL,
 subagent_name TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('running','completed','cancelled','error','incomplete')),
 started_at TEXT NOT NULL,
 finished_at TEXT,
 token_usage_json TEXT,
 summary TEXT,
 transcript_json TEXT,
 transcript_truncated INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL DEFAULT (datetime('now'))
 )
 "#,
    )
    .execute(pool)
    .await?;
    // Step 3: copy rows from the old table (if it exists). The
    // SELECT * works because the new table has the same column
    // set, only the CHECK constraint differs.
    sqlx::query(
        r#"
 INSERT INTO subagent_runs
 SELECT * FROM subagent_runs_old
 "#,
    )
    .execute(pool)
    .await
    .ok(); // benign if the old table didn't exist
           // Step 4: drop the old table.
    sqlx::query("DROP TABLE subagent_runs_old")
        .execute(pool)
        .await
        .ok();
    // Step 5: re-create the two indexes.
    sqlx::query(
        r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_session_started
 ON subagent_runs(parent_session_id, started_at DESC)
 "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
 CREATE INDEX IF NOT EXISTS idx_subagent_runs_request
 ON subagent_runs(parent_request_id)
 "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Rebuild `turn_trace` with the `run_id` column + the widened
/// UNIQUE constraint (08-20-worker-turn-trace-persist).
///
/// SQLite cannot `ALTER` a table constraint, so widening
/// `UNIQUE(session_id, seq)` → `UNIQUE(session_id, run_id, seq)`
/// (the upsert anchor) requires the table-rebuild dance — same
/// 5 steps as [`widen_subagent_runs_status_check_for_incomplete`]
/// above, with three deliberate differences:
///
/// 1. **Probe**: `pragma_table_info('turn_trace')` for a `run_id`
///    column (more direct than the precedent's
///    `sqlite_master.sql contains` probe — the column is the
///    actual migration target). Table missing entirely → return
///    Ok (the greenfield CREATE in `run_migrations` owns that
///    case; calling this helper standalone on a pre-schema pool
///    is a no-op).
/// 2. **Explicit column list in the copy** (NOT `SELECT *`): the
///    new `run_id` column sits in the MIDDLE of the column order
///    (after `session_id`), so a positional `SELECT *` would
///    mis-align every column after it. The precedent could use
///    `SELECT *` only because its rebuild kept the identical
///    column set. The list below must cover every legacy column —
///    which is why `run_migrations` invokes this AFTER the six
///    `add_turn_trace_column_if_missing` backfills (an old-shape
///    table missing e.g. `context_window` would fail the named
///    SELECT).
/// 3. **Transaction-wrapped**: rename → create → copy → drop →
///    index in one `BEGIN IMMEDIATE` (via `pool.begin`), so a
///    crash cannot leave a half-rebuilt state. Residue safety: if
///    a pre-tx-era crash somehow left a `turn_trace_old` behind,
///    the `ALTER TABLE ... RENAME TO turn_trace_old` below fails
///    loudly on the duplicate name — the `DROP TABLE IF EXISTS`
///    preamble clears it instead (its rows are already in the
///    current table or lost with that crash; the probe then
///    re-runs the rebuild).
///
/// **FK safety**: same argument as the `widen_subagent_runs`
/// precedent — no other table references `turn_trace`, and its
/// only FK (`session_id → sessions`) keeps pointing at the same
/// `sessions` rows throughout, so `PRAGMA foreign_keys` is NOT
/// toggled (avoids polluting the multi-connection test pool's
/// per-connection pragma state).
pub(crate) async fn rebuild_turn_trace_with_run_id(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // Probe 1: table must exist (greenfield CREATE not yet run /
    // helper invoked standalone) → nothing to rebuild.
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'turn_trace'",
    )
    .fetch_one(pool)
    .await?;
    if table_exists == 0 {
        return Ok(());
    }
    // Probe 2: `run_id` column already present (greenfield CREATE
    // or a previous rebuild) → short-circuit, re-runs are no-ops.
    let has_run_id: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('turn_trace') WHERE name = 'run_id'",
    )
    .fetch_one(pool)
    .await?;
    if has_run_id > 0 {
        return Ok(());
    }

    // Residue guard (see doc comment point 3): a leftover
    // `turn_trace_old` from an aborted pre-transaction rebuild
    // would collide with the RENAME below.
    sqlx::query("DROP TABLE IF EXISTS turn_trace_old")
        .execute(pool)
        .await?;

    // Five-step dance inside one transaction (BEGIN IMMEDIATE via
    // pool.begin — bound to a single pooled connection).
    let mut tx = pool.begin().await?;
    // Step 1: rename existing.
    sqlx::query("ALTER TABLE turn_trace RENAME TO turn_trace_old")
        .execute(&mut *tx)
        .await?;
    // Step 2: create the new-shape table (mirrors the greenfield
    // CREATE in schema.rs — keep the two in sync).
    sqlx::query(
        r#"
        CREATE TABLE turn_trace (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id        TEXT NOT NULL,
            run_id            TEXT NOT NULL DEFAULT '',
            seq               INTEGER NOT NULL,
            token_usage_json  TEXT,
            compaction_json   TEXT,
            loop_hint_json    TEXT,
            breadcrumb_json   TEXT,
            tools_token       INTEGER,
            memory_token      INTEGER,
            images_token      INTEGER,
            at_files_token    INTEGER,
            system_token      INTEGER,
            context_window    INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            UNIQUE(session_id, run_id, seq)
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;
    // Step 3: copy rows with an EXPLICIT column list — old rows
    // are all main-loop rows, so run_id seeds '' (the sentinel).
    // NOT SELECT *: run_id is new and sits mid-order (see doc
    // comment point 2).
    sqlx::query(
        r#"
        INSERT INTO turn_trace (id, session_id, run_id, seq,
                                token_usage_json, compaction_json,
                                loop_hint_json, breadcrumb_json,
                                tools_token, memory_token, images_token,
                                at_files_token, system_token,
                                context_window, created_at)
        SELECT id, session_id, '', seq,
               token_usage_json, compaction_json,
               loop_hint_json, breadcrumb_json,
               tools_token, memory_token, images_token,
               at_files_token, system_token,
               context_window, created_at
        FROM turn_trace_old
        "#,
    )
    .execute(&mut *tx)
    .await?;
    // Step 4: drop the old table (its indexes — incl. the legacy
    // idx_turn_trace_session_seq — go with it; the new UNIQUE
    // covers that lookup path with its (session_id, run_id)
    // prefix).
    sqlx::query("DROP TABLE turn_trace_old")
        .execute(&mut *tx)
        .await?;
    // Step 5: re-create the worker-rows partial index (the
    // run_migrations-level CREATE INDEX after this helper covers
    // the greenfield path; doing it here too keeps a standalone
    // rebuild self-contained. IF NOT EXISTS makes the double
    // create a no-op.)
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_turn_trace_run
        ON turn_trace(run_id) WHERE run_id != ''
        "#,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// `std::env::home_dir` was removed; this is the cross-platform
/// fallback. If the env vars are unset we fall back to "." so the
/// legacy row has *some* path (it'll be wrong, but the row will
/// exist; the user is expected to reassign or hide it).
pub(crate) fn home_dir_or_dot() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string())
}
