//! A2 + B7 (Permission system + per-session Mode) — DB CRUD.
//!
//! Three concerns live here:
//!
//! 1. **Per-session Mode persistence** ([`update_session_mode`]) —
//! the `sessions.mode` column that drives both the ⑨ 关 permission
//! policy and the ⑧a Mode check (tool list filtering + system
//! prompt prefix + runtime intercept).
//!
//! 2. **Per-session "always allow" set**
//! ([`grant_tool_permission`] / [`has_tool_permission`] /
//! [`revoke_tool_permission`]) — backs the ⑨ 关 Tier 4 short-circuit
//! ("this session has previously granted this tool, don't ask again").
//! All three `match_kind` values are now live:
//! - `tool` — whole-tool grant (e.g. web_fetch); `match_value IS NULL`.
//! - `path` — sqlite GLOB on a filesystem path (path tools); checked
//!   by `permissions::check_path_grant`.
//! - `prefix` — exact command-prefix match for `shell` (e.g. `cargo`);
//!   the write side (`match_value_for_allow_always`) has existed since
//!   the re-grill; the read side (`permissions::check_prefix_grant`)
//!   was wired in the 三档分类 refactor (2026-06-14), closing the old
//!   "stored but never queried" gap.
//!
//! 3. **Audit log persistence** ([`record_audit_event`] /
//! [`list_audit_events`]) — every ⑨ 关 decision path hits the
//! audit hook with a typed [`AuditKind`] and a JSON payload. The
//! UI query side (C4) is out of scope for A2; PR1 only writes
//! the rows.
//!
//! 4. **Audit-log keyset pagination** ([`list_audit_events_page`],
//! RULE-PERM-001 2026-08-30) — bounded-page read with server-side
//! kind/critical filters + exact counts, for the AuditLogModal's
//! paged UI. The full-pull [`list_audit_events`] is untouched
//! (`traceStore` still consumes it).
//!
//! All functions return `Result<T, sqlx::Error>` (no logging) so
//! the caller decides how to surface the error (the agent loop
//! wraps each call in `tracing::warn!` on failure).

use serde::Serialize;
use sqlx::{Row, SqlitePool};

use super::Mode;

// ---------------------------------------------------------------------------
// Per-session Mode persistence
// ---------------------------------------------------------------------------

/// Update the `mode` column on a session row. Called by the
/// `set_session_mode` Tauri command on every user toggle of the
/// `ModeSelect.vue` dropdown. The function is a single UPDATE
/// statement; the new value is `mode.as_str()` (lowercase string).
///
/// `updated_at` is bumped to the current time so the session
/// list re-sorts correctly (matches `update_session_model_id` /
/// `rename_session` / `set_session_color` semantics).
///
/// Returns `Ok(())` on success; `Err(sqlx::Error)` if the DB
/// write fails. The frontend surfaces the error as a toast.
pub async fn update_session_mode(
    pool: &SqlitePool,
    session_id: &str,
    mode: Mode,
) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
 UPDATE sessions
 SET mode = ?, updated_at = ?
 WHERE id = ?
 "#,
    )
    .bind(mode.as_str())
    .bind(&now)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-session "always allow" set
// ---------------------------------------------------------------------------

/// Grant a "always allow" permission for `(session_id, tool_name)`
/// with the given `match_kind` and `match_value`. UPSERT semantics:
/// re-granting the same `(session_id, tool_name, match_kind,
/// match_value)` row updates `granted_at` to the current time
/// instead of inserting a duplicate.
///
/// MVP scope: `match_kind = 'tool'`, `match_value = NULL`. The
/// 3-kind schema is reserved for a future PR (`prefix` for shell
/// command prefixes, `path` for file path globs).
///
/// Returns `Ok(())` on success. The frontend re-loads
/// `session_tool_permissions` lazily (no separate IPC — the agent
/// loop's next turn reads from DB on every tool_use decision).
pub async fn grant_tool_permission(
    pool: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    match_kind: &str,
    match_value: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 INSERT INTO session_tool_permissions
 (session_id, tool_name, match_kind, match_value, granted_at)
 VALUES (?, ?, ?, ?, datetime('now'))
 ON CONFLICT(session_id, tool_name, match_kind, match_value)
 DO UPDATE SET granted_at = datetime('now')
 "#,
    )
    .bind(session_id)
    .bind(tool_name)
    .bind(match_kind)
    .bind(match_value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns `true` if the session has an "always allow" row for
/// `tool_name`. Used by ⑨ 关 Tier 3 to short-circuit the
/// permission modal: the user previously clicked "始终允许" on
/// this tool, so subsequent calls of the same tool on this
/// session go straight to Tier 6 (audit) and execute.
///
/// MVP matches `match_kind = 'tool'` + `match_value IS NULL`
/// only — the future `prefix` / `path` kinds are not yet
/// consulted.
pub async fn has_tool_permission(
    pool: &SqlitePool,
    session_id: &str,
    tool_name: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
 SELECT 1 FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = 'tool' AND match_value IS NULL
 LIMIT 1
 "#,
    )
    .bind(session_id)
    .bind(tool_name)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Remove ONE "always allow" row identified by its full PK
/// `(session_id, tool_name, match_kind, match_value)`. Wired to
/// the permission-grant management UI's per-row "撤销" button
/// (task 07-01-permission-grant-list-ui).
///
/// **NULL match_value (design D2)**: `match_kind = 'tool'` rows
/// store `match_value IS NULL`. SQLite evaluates
/// `match_value = NULL` as always-false, so a naive
/// `WHERE match_value = ?` bound to NULL would silently delete 0
/// rows (revoke looks successful but the grant survives). We
/// branch: `None` → `match_value IS NULL`, `Some(v)` →
/// `match_value = ?`. Covered by the
/// `revoke_tool_permission_null_value_tool_kind` test.
///
/// Only the exact PK row is deleted; sibling grants for the same
/// `tool_name` under a different `match_kind`/`match_value` (e.g.
/// other path globs on `read_file`) are preserved.
pub async fn revoke_tool_permission(
    pool: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    match_kind: &str,
    match_value: Option<&str>,
) -> Result<(), sqlx::Error> {
    match match_value {
        None => {
            sqlx::query(
                r#"
 DELETE FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = ? AND match_value IS NULL
 "#,
            )
            .bind(session_id)
            .bind(tool_name)
            .bind(match_kind)
            .execute(pool)
            .await?;
        }
        Some(v) => {
            sqlx::query(
                r#"
 DELETE FROM session_tool_permissions
 WHERE session_id = ? AND tool_name = ?
 AND match_kind = ? AND match_value = ?
 "#,
            )
            .bind(session_id)
            .bind(tool_name)
            .bind(match_kind)
            .bind(v)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Read every "always allow" row for `session_id`, newest first.
/// Wired to the permission-grant management UI's "load on open"
/// call (task 07-01-permission-grant-list-ui). The row set is the
/// raw `session_tool_permissions` rows; the frontend renders each
/// row's `match_kind` + `match_value` (path glob / prefix token)
/// so the user can distinguish multiple grants on the same tool.
///
/// Empty / missing session returns an empty `Vec` (NOT an error)
/// — the modal renders its empty-state placeholder. The
/// `ORDER BY granted_at DESC, rowid DESC` is a stable sort:
/// `granted_at` is `datetime('now')` (1-second resolution), so
/// same-second grants tie on `granted_at` and break on `rowid`
/// (SQLite's implicit monotonic insertion id).
pub async fn list_tool_permissions(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<PermissionGrantRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
 SELECT session_id, tool_name, match_kind, match_value, granted_at
 FROM session_tool_permissions
 WHERE session_id = ?
 ORDER BY granted_at DESC, rowid DESC
 "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(PermissionGrantRow {
                session_id: r.try_get("session_id")?,
                tool_name: r.try_get("tool_name")?,
                match_kind: r.try_get("match_kind")?,
                match_value: r.try_get("match_value")?,
                granted_at: r.try_get("granted_at")?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Audit log persistence
// ---------------------------------------------------------------------------

/// Append one row to `session_audit_events`. Called from the
/// agent loop's `permission::check` after each ⑨ 关 decision path
/// (Allow / Deny / Ask / Timeout). `kind` is a stringified
/// [`crate::agent::permissions::AuditKind`] (e.g.
/// `"tool_allowed"`, `"tool_denied"`, `"tool_permission_ask"`,
/// `"permission_granted"`, `"mode_changed"`,
/// `"yolo_entered"`, `"yolo_exited"`, `"tool_denied_yolo"`,
/// `"permission_timeout"`, `"request_cancelled"`). `payload_json`
/// is a free-form JSON object the caller builds — typically
/// `{ "tool_name": "...", "tool_input": {...}, "reason": "..." }`.
///
/// `turn_seq` (E2, 2026-07-14): the per-session turn counter value
/// at the time of the audit event, used by the trace viewer to
/// group audit rows by turn. `None` for db helpers writing audit
/// rows outside the turn loop (e.g. `db::sessions::edit_user_message`)
/// and for IPC-handler audit writes (e.g. `commands/question.rs`
/// resolve_* handlers), and for historical rows (pre-v7 migration
/// backfills NULL).
///
/// MVP scope: write-only. Read-side UI (C4 audit log panel)
/// is out of scope for A2; [`list_audit_events`] is here as a
/// future hook.
pub async fn record_audit_event(
    pool: &SqlitePool,
    session_id: &str,
    kind: &str,
    payload_json: Option<&str>,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 INSERT INTO session_audit_events
 (session_id, ts, kind, payload_json, turn_seq)
 VALUES (?, datetime('now'), ?, ?, ?)
 "#,
    )
    .bind(session_id)
    .bind(kind)
    .bind(payload_json)
    .bind(turn_seq)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read all audit events for `session_id`, newest first. Wired to
/// the C4 audit-log UI's `list_session_audit_events` Tauri command
/// (2026-06-14). Sorted by `ts DESC` (the schema's index supports
/// this — `idx_session_audit_events_session_ts`).
pub async fn list_audit_events(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<AuditEventRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, session_id, ts, kind, payload_json, turn_seq
        FROM session_audit_events
        WHERE session_id = ?
        ORDER BY ts DESC
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(AuditEventRow {
                id: r.try_get("id")?,
                session_id: r.try_get("session_id")?,
                ts: r.try_get("ts")?,
                kind: r.try_get("kind")?,
                payload_json: r.try_get("payload_json")?,
                turn_seq: r.try_get("turn_seq")?,
            })
        })
        .collect()
}

/// Row shape for [`list_audit_events`] and the
/// `list_session_audit_events` Tauri command. The `payload_json`
/// column stays a `String` (raw JSON text) so callers can re-parse
/// per row rather than committing to a typed shape upfront —
/// different `kind` values carry different payload schemas today,
/// and a locked struct would force premature commitments.
///
/// C4 PR1 follow-up (check phase, 2026-06-14): the struct uses
/// `#[serde(rename_all = "camelCase")]` — NOT plain snake_case —
/// matching every other `db::*Row` type that crosses the IPC
/// boundary (`SessionRow`, `SessionSummary`, `ProviderRow`,
/// `ModelRow`). The Rust fields stay snake_case (per Rust style)
/// but the wire shape is camelCase: the C4 audit-log UI's TS
/// interface reads `sessionId` / `payloadJson`, not `session_id`
/// / `payload_json`. This is mandated by
/// `.trellis/spec/backend/database-guidelines.md` ("All Serialize
/// structs that cross the IPC boundary have
/// #[serde(rename_all = \"camelCase\")]") and confirmed by every
/// existing Row struct in `db/types.rs`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventRow {
    pub id: i64,
    pub session_id: String,
    pub ts: String,
    pub kind: String,
    pub payload_json: Option<String>,
    /// E2 (2026-07-14): the per-session turn counter value at the
    /// time of the audit event. `None` for historical rows (pre-v7)
    /// and for IPC-handler audit writes that have no turn-loop
    /// context. The trace viewer uses this to group audit rows by
    /// turn.
    pub turn_seq: Option<i64>,
}

// ---------------------------------------------------------------------------
// Audit-log keyset pagination (RULE-PERM-001, 2026-08-30)
// ---------------------------------------------------------------------------

/// Default page size for [`list_audit_events_page`] when the caller
/// omits `limit` (design C3: 页大小固定 100).
pub const AUDIT_PAGE_DEFAULT_LIMIT: i64 = 100;

/// Hard cap for [`list_audit_events_page`]'s `limit` (design C3:
/// 后端接受 limit 但 cap 500,防误用). Also the lower clamp bound:
/// SQLite reads a negative `LIMIT` as *unlimited*, so clamping is a
/// correctness guard, not just cosmetics — a naive `LIMIT ?` bound
/// to `-3` would bypass the cap entirely and full-pull the table.
pub const AUDIT_PAGE_MAX_LIMIT: i64 = 500;

/// Query knobs for [`list_audit_events_page`] — the keyset-paginated,
/// filter-pushdown sibling of [`list_audit_events`]. The old
/// full-pull function is intentionally unchanged (design D3 / R6:
/// `traceStore` still needs every row); new consumers (AuditLogModal)
/// go through this.
///
/// Field notes:
/// - `limit` — page size; `None` → [`AUDIT_PAGE_DEFAULT_LIMIT`],
///   values > [`AUDIT_PAGE_MAX_LIMIT`] (or < 1) are clamped.
/// - `before_ts` / `before_id` — the two halves of the keyset cursor
///   `(ts, id)`: the `(ts, id)` of the **last row of the previously
///   fetched page**. `ts` is second-resolution (`datetime('now')`),
///   so same-second rows are the norm (multiple tool calls per turn)
///   and the `id` tie-break segment is what makes the cursor exact.
///   Both must be `Some` together — a partial cursor is rejected with
///   `sqlx::Error::InvalidArgument` (an `id`-less ts cursor would
///   silently skip the rest of the cursor's own second).
/// - `kind` — optional equality filter, pushed to SQL (design D2).
/// - `critical_only` — when `true`, only rows whose
///   `payload_json` parses as JSON **and** carries `$.critical = 1`
///   are returned. The `json_valid` guard is mandatory (R7): NULL or
///   malformed payloads evaluate to non-critical instead of erroring,
///   matching the frontend's legacy `isCritical` tolerance.
#[derive(Debug, Clone, Default)]
pub struct AuditEventPageQuery {
    pub limit: Option<i64>,
    pub before_ts: Option<String>,
    pub before_id: Option<i64>,
    pub kind: Option<String>,
    pub critical_only: bool,
}

/// Page result for [`list_audit_events_page`]. One call answers the
/// list AND the counter chips (design D2/R3): `events` is the ordered
/// page, `matched` is the total hit count under the *active*
/// kind/critical filters, `total_all` / `total_critical` are the
/// unfiltered totals (critical count deliberately ignores `kind`, so
/// the chips stay consistent no matter which filter combination is
/// active).
///
/// Wire shape is camelCase (`#[serde(rename_all = "camelCase")]`,
/// house rule for every db Row crossing IPC — see the
/// [`AuditEventRow`] doc for the full rationale).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventPageRow {
    /// The page: `ORDER BY ts DESC, id DESC` (explicit id tie-break
    /// in SQL — the schema index only covers the `ts` segment; the
    /// frontend must not re-sort, R4).
    pub events: Vec<AuditEventRow>,
    /// Rows matching the active kind/critical filters (cursor NOT
    /// applied — it bounds pages, not counts). Drives the "加载更多"
    /// visibility (`events.len() < matched`).
    pub matched: i64,
    /// Unfiltered row count for the session (== what the old
    /// full-pull command's `.len()` reported).
    pub total_all: i64,
    /// Critical row count for the session, unaffected by `kind`
    /// (== the modal's critical chip).
    pub total_critical: i64,
}

/// Keyset-paginated audit read for `session_id` (RULE-PERM-001,
/// 2026-08-30). Companion to [`list_audit_events`] — same rows, same
/// `ts DESC, id DESC` order, but bounded pages, server-side filters
/// and exact counts so the audit-log UI can page instead of
/// full-pulling (PRD: 千级行数的 session 会把 IPC 载荷 / 首屏渲染 /
/// 内存都拖成线性增长).
///
/// **Why keyset, not OFFSET (design D1 / R5)**: the table is
/// append-only and the modal may stay open while the agent writes new
/// rows. An OFFSET page 2 shifts by one on every newer insert
/// (duplicate + skipped row); the `(ts, id)` cursor anchors on the
/// last-seen row, so earlier pages stay stable regardless of
/// appends. The behavior test
/// `audit_page_keyset_stable_when_new_row_appended_mid_pagination`
/// pins this.
///
/// **SQL shape** (all fragments are static strings — only the
/// composition is dynamic; every value is bound, never formatted in):
/// `kind = ?` for the kind filter,
/// `(json_valid(payload_json) AND json_extract(payload_json,
/// '$.critical') = 1)` for critical (the `json_valid` guard keeps
/// NULL/malformed payloads non-erroring per R7), and
/// `(ts < ? OR (ts = ? AND id < ?))` for the cursor.
///
/// Counts come back in the same call: one query for
/// `total_all` + `total_critical` (`COUNT(*) FILTER (WHERE ...)`,
/// SQLite ≥ 3.30) and one `COUNT(*)` with the active filter
/// predicates for `matched`.
///
/// Empty / missing session returns a zeroed page (empty `events`,
/// three zeros) — NOT an error, mirroring [`list_audit_events`].
pub async fn list_audit_events_page(
    pool: &SqlitePool,
    session_id: &str,
    query: AuditEventPageQuery,
) -> Result<AuditEventPageRow, sqlx::Error> {
    // The cursor is a two-part key: accepting a ts-only cursor would
    // silently drop the remainder of the cursor's own second.
    if query.before_ts.is_some() && query.before_id.is_none() {
        return Err(sqlx::Error::InvalidArgument(
            "list_audit_events_page: before_ts requires before_id (the keyset cursor is (ts, id))"
                .to_string(),
        ));
    }
    // Default 100 / cap 500 / clamp off the LIMIT -1 = unlimited footgun.
    let limit = query
        .limit
        .unwrap_or(AUDIT_PAGE_DEFAULT_LIMIT)
        .clamp(1, AUDIT_PAGE_MAX_LIMIT);

    // Shared filter fragment: composed into both the page query and
    // the `matched` count so the two can never drift.
    let mut filter_sql = String::from("session_id = ?");
    if query.kind.is_some() {
        filter_sql.push_str(" AND kind = ?");
    }
    if query.critical_only {
        // R7: `json_valid` runs first so NULL / malformed payload rows
        // fall out as non-critical instead of raising.
        filter_sql.push_str(
            " AND (json_valid(payload_json) AND json_extract(payload_json, '$.critical') = 1)",
        );
    }

    let mut page_sql = format!(
        "SELECT id, session_id, ts, kind, payload_json, turn_seq \
         FROM session_audit_events WHERE {filter_sql}"
    );
    if query.before_ts.is_some() {
        // Keyset cursor: strictly-before the anchor row in
        // (ts DESC, id DESC) order.
        page_sql.push_str(" AND (ts < ? OR (ts = ? AND id < ?))");
    }
    page_sql.push_str(" ORDER BY ts DESC, id DESC LIMIT ?");
    let matched_sql =
        format!("SELECT COUNT(*) AS matched FROM session_audit_events WHERE {filter_sql}");

    // Page query — bind order follows the composed fragment order.
    let mut page_q = sqlx::query(&page_sql).bind(session_id);
    if let Some(kind) = &query.kind {
        page_q = page_q.bind(kind);
    }
    if let Some(ts) = &query.before_ts {
        page_q = page_q.bind(ts).bind(ts).bind(query.before_id);
    }
    page_q = page_q.bind(limit);
    let rows = page_q.fetch_all(pool).await?;
    let events: Vec<AuditEventRow> = rows
        .into_iter()
        .map(|r| {
            Ok(AuditEventRow {
                id: r.try_get("id")?,
                session_id: r.try_get("session_id")?,
                ts: r.try_get("ts")?,
                kind: r.try_get("kind")?,
                payload_json: r.try_get("payload_json")?,
                turn_seq: r.try_get("turn_seq")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;

    // matched: same predicates as the page, cursor NOT applied.
    let mut matched_q = sqlx::query(&matched_sql).bind(session_id);
    if let Some(kind) = &query.kind {
        matched_q = matched_q.bind(kind);
    }
    let matched: i64 = matched_q.fetch_one(pool).await?.try_get("matched")?;

    // totals: one query, no kind/critical filters (critical counted
    // via FILTER so the chip is kind-independent per R3).
    let totals = sqlx::query(
        r#"
        SELECT COUNT(*) AS total_all,
               COUNT(*) FILTER (WHERE json_valid(payload_json)
                                AND json_extract(payload_json, '$.critical') = 1)
                   AS total_critical
        FROM session_audit_events
        WHERE session_id = ?
        "#,
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    Ok(AuditEventPageRow {
        events,
        matched,
        total_all: totals.try_get("total_all")?,
        total_critical: totals.try_get("total_critical")?,
    })
}

/// Row shape for [`list_tool_permissions`] and the
/// `list_session_tool_permissions` Tauri command (task
/// 07-01-permission-grant-list-ui). `match_value` is `None` for
/// `match_kind = 'tool'` (whole-tool grants); `Some(glob)` for
/// `path`; `Some(prefix_token)` for `prefix`. Mirrors
/// [`AuditEventRow`]'s wire convention —
/// `#[serde(rename_all = "camelCase")]` per
/// `.trellis/spec/backend/database-guidelines.md` (the frontend TS
/// reads `matchKind` / `matchValue`, not snake_case).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrantRow {
    pub session_id: String,
    pub tool_name: String,
    pub match_kind: String,
    pub match_value: Option<String>,
    pub granted_at: String,
}
