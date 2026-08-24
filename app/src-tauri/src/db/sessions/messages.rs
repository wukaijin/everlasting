//! Message persistence for sessions: persist_turn, latency, metadata, edit.
//!
//! Split out of `db/sessions.rs` (2026-08-08 batch3).

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::llm::types::{ContentBlock, MessageContent, Role};

// ---------------------------------------------------------------------------
// Message persistence
// ---------------------------------------------------------------------------

/// Derive the four content-derived columns (`content` JSON / denormalized
/// `text` / `has_tool_calls` / `has_tool_results`) from a
/// [`MessageContent`]. Shared by every messages INSERT/UPSERT site
/// (`persist_turn` and the RULE-PERSIST-001 checkpoint family below) so
/// the denormalization can never drift between the normal-path persist
/// and the checkpoint overwrites that land on the same row.
fn content_columns(content: &MessageContent) -> Result<(String, String, bool, bool), sqlx::Error> {
    let content_json = serde_json::to_string(content)
        .map_err(|e| sqlx::Error::Encode(format!("serialize content: {}", e).into()))?;
    let text = content.to_text();
    let has_tool_calls = matches!(content, MessageContent::Blocks(b)
     if b.iter().any(|x| matches!(x, ContentBlock::ToolUse { .. })));
    let has_tool_results = matches!(content, MessageContent::Blocks(b)
     if b.iter().any(|x| matches!(x, ContentBlock::ToolResult { .. })));
    Ok((content_json, text, has_tool_calls, has_tool_results))
}

/// Persist one message (assistant turn or tool_result turn). The `seq` is
/// caller-managed and must be strictly increasing within a session.
///
/// If the message is a user message and the session title is still the
/// default "新对话", auto-generate a title from the message text.
///
/// F5 (LLM Latency Tracking): the optional `latency` carries the
/// three millisecond values (ttfb / gen / total) measured by the
/// frontend's `Date.now()` deltas around the `start` / first
/// `delta` / `done` events. The values are NULL when the caller
/// has not measured them (e.g. `tool_result` rows; the tool
/// result is emitted as a user-role row by the agent loop and
/// the latency is per assistant turn, not per tool). Pre-F5
/// callers can pass `None` and the columns stay NULL.
///
/// RULE-PERSIST-001 (08-24-p1-turn-crash-recovery): this stays a
/// **bare INSERT** (no ON CONFLICT). The UNIQUE(session_id, seq)
/// violation is a deliberate bug signal for every caller whose
/// seq should never collide (user rows, tool_result rows,
/// synthetic repair rows) — silently upserting here would mask
/// seq-drift bugs (RULE-A-003 family). Only the assistant
/// turn-finalize site in `drive.rs` knows a checkpoint row may
/// already occupy its seq; that site alone uses
/// [`finalize_turn_persist`].
pub async fn persist_turn(
    pool: &SqlitePool,
    session_id: &str,
    role: Role,
    content: &MessageContent,
    seq: i64,
    latency: Option<&MessageLatency>,
    speaker: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let role_str = match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let (content_json, text, has_tool_calls, has_tool_results) = content_columns(content)?;

    sqlx::query(
        r#"
 INSERT INTO messages
 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq, ttfb_ms, gen_ms, total_ms, thinking_ms, speaker)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
 "#,
    )
    .bind(session_id)
    .bind(role_str)
    .bind(&content_json)
    .bind(&text)
    .bind(has_tool_calls as i64)
    .bind(has_tool_results as i64)
    .bind(&now)
    .bind(seq)
    .bind(latency.and_then(|l| l.ttfb_ms))
    .bind(latency.and_then(|l| l.gen_ms))
    .bind(latency.and_then(|l| l.total_ms))
    // F5 follow-up: thinking-phase duration. Persisted
    // alongside the three latency columns in the same
    // INSERT — both go in at the moment the agent loop
    // calls `persist_turn` for the assistant row, which
    // is also the row the frontend will fire
    // `update_message_latency` / `update_message_thinking`
    // against (those IPCs are the patch-after-the-fact
    // path for rows persisted BEFORE the per-message
    // telemetry was wired through the agent loop).
    .bind(latency.and_then(|l| l.thinking_ms))
    // Group chat (07-29-group-chat): which participant
    // authored this turn. NULL for classic-chat rows and
    // for user messages. The group-chat orchestration sets
    // this on each participant's assistant turn.
    .bind(speaker)
    .execute(pool)
    .await?;

    // Auto-title from first user message.
    if matches!(role, Role::User) {
        sqlx::query(
            r#"
 UPDATE sessions
 SET title = CASE
 WHEN title = '新对话' AND ? != '' THEN substr(?,1,50)
 ELSE title
 END
 WHERE id = ?
 "#,
        )
        .bind(&text)
        .bind(&text)
        .bind(session_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// RULE-PERSIST-001 (08-24-p1-turn-crash-recovery): turn 流式检查点 + 崩溃恢复
//
// 状态机(daemon kill -9 / OOM / 断电时 drain 覆盖不到的窗口):
//   (stream 就绪) ──占位──▶ in_progress ──周期 upsert(同 seq)──▶ in_progress
//       │ 正常/cancel/error 收尾:finalize_turn_persist 覆盖 + status=NULL
//       │ 空 turn 收尾:delete_in_progress_turn 删占位
//       │ daemon 崩溃(行停留 in_progress)
//       └▶ 启动恢复 recover_interrupted_messages:空删 / 有内容加
//          INTERRUPTED_MARKER 块 + status='interrupted';另扫每个
//          session 尾部的孤儿 tool_use(W2 窗口)合成 is_error
//          tool_result,保证下次请求不 400(pair atomicity)。
// ---------------------------------------------------------------------------

/// Upsert the `status='in_progress'` checkpoint row for the assistant
/// turn currently streaming at `seq`. Called once as a placeholder when
/// the LLM stream becomes ready, then periodically (time-gated by the
/// caller in `drive.rs`) with the accumulated snapshot blocks.
///
/// Why upsert here while [`persist_turn`] stays a bare INSERT: the
/// checkpoint row and the turn's final row share `(session_id, seq)` by
/// design — the finalize site overwrites this row. Every other INSERT
/// site (user rows at a different seq, tool_result rows at seq+1) can
/// never collide, and for those the UNIQUE violation remains a
/// load-bearing seq-drift bug signal (RULE-A-003 family) that an upsert
/// would silently swallow.
///
/// Latency columns are not written: checkpoints have no latency to
/// report, and the finalize overwrite fills them. `created_at` is set
/// on the initial INSERT only (the DO UPDATE leaves it — the row was
/// born when the placeholder landed).
pub async fn upsert_in_progress_turn(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    blocks: &[ContentBlock],
    speaker: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let (content_json, text, has_tool_calls, has_tool_results) =
        content_columns(&MessageContent::Blocks(blocks.to_vec()))?;
    sqlx::query(
        r#"
 INSERT INTO messages
 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq, speaker, status)
 VALUES (?, 'assistant', ?, ?, ?, ?, ?, ?, ?, 'in_progress')
 ON CONFLICT(session_id, seq) DO UPDATE SET
   role = 'assistant',
   content = excluded.content,
   text = excluded.text,
   has_tool_calls = excluded.has_tool_calls,
   has_tool_results = excluded.has_tool_results,
   speaker = excluded.speaker,
   status = 'in_progress'
 "#,
    )
    .bind(session_id)
    .bind(&content_json)
    .bind(&text)
    .bind(has_tool_calls as i64)
    .bind(has_tool_results as i64)
    .bind(&now)
    .bind(seq)
    .bind(speaker)
    .execute(pool)
    .await?;
    Ok(())
}

/// Finalize the assistant turn row: same signature/semantics as
/// [`persist_turn`] but as an INSERT ... ON CONFLICT DO UPDATE that
/// overwrites the `in_progress` checkpoint row (if any) at the same
/// `(session_id, seq)` and clears `status` back to NULL (终态).
///
/// Only the assistant turn-finalize site in `drive.rs` may call this —
/// it is the one site that knows a checkpoint row may already occupy
/// its seq. All other persist sites keep the bare-INSERT bug-signal
/// semantics (see [`persist_turn`]). The auto-title branch is kept for
/// signature parity with `persist_turn`; user rows never route here so
/// it cannot fire in practice.
pub async fn finalize_turn_persist(
    pool: &SqlitePool,
    session_id: &str,
    role: Role,
    content: &MessageContent,
    seq: i64,
    latency: Option<&MessageLatency>,
    speaker: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let role_str = match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let (content_json, text, has_tool_calls, has_tool_results) = content_columns(content)?;

    sqlx::query(
        r#"
 INSERT INTO messages
 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq, ttfb_ms, gen_ms, total_ms, thinking_ms, speaker, status)
 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
 ON CONFLICT(session_id, seq) DO UPDATE SET
   role = excluded.role,
   content = excluded.content,
   text = excluded.text,
   has_tool_calls = excluded.has_tool_calls,
   has_tool_results = excluded.has_tool_results,
   ttfb_ms = excluded.ttfb_ms,
   gen_ms = excluded.gen_ms,
   total_ms = excluded.total_ms,
   thinking_ms = excluded.thinking_ms,
   speaker = excluded.speaker,
   status = NULL
 "#,
    )
    .bind(session_id)
    .bind(role_str)
    .bind(&content_json)
    .bind(&text)
    .bind(has_tool_calls as i64)
    .bind(has_tool_results as i64)
    .bind(&now)
    .bind(seq)
    .bind(latency.and_then(|l| l.ttfb_ms))
    .bind(latency.and_then(|l| l.gen_ms))
    .bind(latency.and_then(|l| l.total_ms))
    .bind(latency.and_then(|l| l.thinking_ms))
    .bind(speaker)
    .execute(pool)
    .await?;

    // Auto-title from first user message (parity with persist_turn;
    // unreachable in practice — only assistant rows route here).
    if matches!(role, Role::User) {
        sqlx::query(
            r#"
 UPDATE sessions
 SET title = CASE
 WHEN title = '新对话' AND ? != '' THEN substr(?,1,50)
 ELSE title
 END
 WHERE id = ?
 "#,
        )
        .bind(&text)
        .bind(&text)
        .bind(session_id)
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Delete the `in_progress` placeholder row at `(session_id, seq)`.
/// Called by the empty-turn finalize path (a turn that produced zero
/// content must not leave an empty row behind). The `status='in_progress'`
/// guard is deliberate: this function may only ever eat its own
/// placeholder — a misuse against a terminal row (status NULL) is a
/// silent no-op instead of data loss. Returns the number of rows
/// deleted (0 when nothing matched).
pub async fn delete_in_progress_turn(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM messages WHERE session_id = ? AND seq = ? AND status = 'in_progress'",
    )
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Per-count summary of [`recover_interrupted_messages`] for the
/// startup log (mirrors the `reaped = n` shape of
/// `reap_orphaned_runs`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Step A: in_progress rows that had content → marked
    /// `interrupted` (INTERRUPTED_MARKER block appended).
    pub interrupted: usize,
    /// Step A: empty in_progress placeholders → deleted.
    pub deleted: usize,
    /// Step B: per-session orphan assistant(tool_use) tail rows →
    /// synthetic is_error tool_result row inserted at seq+1.
    pub orphan_repaired: usize,
}

impl RecoveryReport {
    /// Total rows touched across both steps — drives the "nothing to
    /// do" fast-path in the startup log.
    pub fn total(&self) -> usize {
        self.interrupted + self.deleted + self.orphan_repaired
    }
}

/// Startup recovery pass for crash-orphaned `messages` rows
/// (RULE-PERSIST-001). Two steps, run in order:
///
/// **Step A — `status='in_progress'` residue (W1 crash window)**: the
/// daemon died mid-stream, leaving the checkpoint row. Empty content
/// (no blocks, or only empty text) → the placeholder is deleted (a
/// crash before any content must not leave an empty row); otherwise a
/// `Text` block carrying [`crate::agent::helpers::INTERRUPTED_MARKER`]
/// (independent block + `\n\n` prefix, mirroring the cancel/error
/// marker convention in `drive.rs`) is appended and the row is marked
/// `status='interrupted'` — the user sees the recovered partial
/// content on reload.
///
/// **Step B — per-session orphan tool_use tail (W2 crash window)**:
/// the daemon died during tool execution, leaving an
/// assistant(tool_use) row with no following tool_result row → every
/// later request 400s (pair atomicity, llm-contract.md §Pair Atomicity).
/// For each such tail we insert a synthetic user-role
/// `is_error` tool_result row at seq+1 (bare INSERT — that seq cannot
/// exist, otherwise the row wouldn't be the tail), mirroring
/// `build_synthetic_tool_result_message` semantics with content noting
/// the daemon interruption. Runs AFTER Step A so an interrupted row
/// that itself carries tool_use is repaired too.
///
/// Affected sessions get `touch_session` (UI list ordering reflects
/// the interruption). Best-effort like `reap_orphaned_runs`: each row
/// is written independently (no wrapping transaction — SQLite single
/// writer; a mid-pass crash just re-runs the idempotent pass next
/// boot), and a caller-level failure logs rather than blocks startup.
pub async fn recover_interrupted_messages(
    pool: &SqlitePool,
) -> Result<RecoveryReport, sqlx::Error> {
    let mut report = RecoveryReport::default();
    let mut touched_sessions: Vec<String> = Vec::new();

    // ---- Step A: in_progress residue → delete-or-mark ----
    let rows = sqlx::query(
        "SELECT id, session_id, seq, content FROM messages WHERE status = 'in_progress'",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let session_id: String = row.try_get("session_id")?;
        let content_str: String = row.try_get("content")?;
        // Corrupt content JSON is unrecoverable-by-construction — but
        // the row must not stay in_progress forever. Treat it as
        // "empty" (delete): a row whose blocks we can't parse never
        // carries user-visible content worth keeping.
        let blocks: Vec<ContentBlock> = serde_json::from_str(&content_str).unwrap_or_default();
        let has_content = blocks.iter().any(|b| match b {
            ContentBlock::Text { text, .. } => !text.is_empty(),
            _ => true,
        });
        if !has_content {
            sqlx::query("DELETE FROM messages WHERE id = ? AND status = 'in_progress'")
                .bind(id)
                .execute(pool)
                .await?;
            report.deleted += 1;
        } else {
            // Marker convention mirrors drive.rs cancel/error: `\n\n`
            // prefix only when there is preceding visible text (the
            // `text` column joins Text blocks, so to_text() == the
            // pre-marker text).
            let mut new_blocks = blocks;
            let had_text = new_blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text, .. } if !text.is_empty()));
            let marker = if had_text {
                format!("\n\n{}", crate::agent::helpers::INTERRUPTED_MARKER)
            } else {
                crate::agent::helpers::INTERRUPTED_MARKER.to_string()
            };
            new_blocks.push(ContentBlock::Text {
                text: marker,
                cache_control: None,
            });
            let content = MessageContent::Blocks(new_blocks);
            let (content_json, text, has_tool_calls, has_tool_results) = content_columns(&content)?;
            sqlx::query(
                r#"
 UPDATE messages
 SET content = ?, text = ?, has_tool_calls = ?, has_tool_results = ?, status = 'interrupted'
 WHERE id = ?
 "#,
            )
            .bind(&content_json)
            .bind(&text)
            .bind(has_tool_calls as i64)
            .bind(has_tool_results as i64)
            .bind(id)
            .execute(pool)
            .await?;
            report.interrupted += 1;
        }
        if !touched_sessions.contains(&session_id) {
            touched_sessions.push(session_id);
        }
    }

    // ---- Step B: per-session orphan tool_use tail (W2) ----
    // The tail-of-session check IS the "no following tool_result"
    // check: a healthy assistant(tool_use) row is always followed by
    // its tool_result row at seq+1, which would then be the tail
    // instead. MAX(seq) per session walks idx_messages_session_seq.
    let tails = sqlx::query(
        r#"
 SELECT m.id, m.session_id, m.seq, m.content
 FROM messages m
 JOIN (SELECT session_id, MAX(seq) AS max_seq FROM messages GROUP BY session_id) t
   ON m.session_id = t.session_id AND m.seq = t.max_seq
 WHERE m.role = 'assistant' AND m.has_tool_calls = 1
 "#,
    )
    .fetch_all(pool)
    .await?;
    for row in tails {
        let session_id: String = row.try_get("session_id")?;
        let seq: i64 = row.try_get("seq")?;
        let content_str: String = row.try_get("content")?;
        let blocks: Vec<ContentBlock> = match serde_json::from_str(&content_str) {
            Ok(b) => b,
            // Unparseable tail: cannot know which tool_uses to answer.
            // Leave it — a follow-up manual repair is better than a
            // wrong synthetic row set.
            Err(_) => continue,
        };
        let tool_uses: Vec<(String, String)> = blocks
            .into_iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, .. } => Some((id, name)),
                _ => None,
            })
            .collect();
        if tool_uses.is_empty() {
            continue;
        }
        let result_blocks: Vec<ContentBlock> = tool_uses
            .iter()
            .map(|(id, name)| {
                let content = format!(
                    "Tool execution was interrupted: the app daemon exited \
unexpectedly (crash or restart) before this tool's result was persisted. \
The tool {} did not run; its result was lost.",
                    name
                );
                ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content,
                    is_error: true,
                    images: None,
                    resolved: None,
                }
            })
            .collect();
        // Bare INSERT (no ON CONFLICT): seq+1 cannot exist — the row
        // we're answering IS the session tail. A UNIQUE violation here
        // would itself be a bug signal (rule family of persist_turn).
        let now = Utc::now().to_rfc3339();
        let content = MessageContent::Blocks(result_blocks);
        let (content_json, text, has_tool_calls, has_tool_results) = content_columns(&content)?;
        sqlx::query(
            r#"
 INSERT INTO messages
 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq, speaker)
 VALUES (?, 'user', ?, ?, ?, ?, ?, ?, NULL)
 "#,
        )
        .bind(&session_id)
        .bind(&content_json)
        .bind(&text)
        .bind(has_tool_calls as i64)
        .bind(has_tool_results as i64)
        .bind(&now)
        .bind(seq + 1)
        .execute(pool)
        .await?;
        report.orphan_repaired += 1;
        if !touched_sessions.contains(&session_id) {
            touched_sessions.push(session_id);
        }
    }

    // R2.4: bump updated_at so the sidebar ordering surfaces the
    // interrupted sessions. Best-effort per session (log-free — the
    // caller already treats the whole pass as best-effort).
    for sid in &touched_sessions {
        let _ = super::session_crud::touch_session(pool, sid).await;
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// F5: per-message latency + per-tool-result duration persistence
// ---------------------------------------------------------------------------

/// Three-field latency breakdown measured by the frontend around
/// the SSE event boundaries of one chat invocation. All three
/// fields are optional because the cancel / error paths may
/// only know the total (no `delta` was ever received → no
/// `ttfb_ms`).
///
/// Field semantics (mirrored in `.trellis/spec/backend/llm-contract.md`
/// "Scenario: Latency Tracking" §2):
/// - `ttfb_ms`: send → first `delta` event (time-to-first-byte)
/// - `gen_ms`:  first `delta` → `done` (active generation)
/// - `total_ms`: send → `done` (end-to-end; always set when
///   `total_ms.is_some()`)
/// - `thinking_ms`: F5 follow-up — first `thinking_delta` →
///   first non-thinking boundary (text `delta`, `tool:call`,
///   `done`, or `error`). `None` when the message never
///   entered the thinking phase. Drives the
///   "Thought for X.Xs" header in ThinkingBlock.vue.
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageLatency {
    pub ttfb_ms: Option<i64>,
    pub gen_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub thinking_ms: Option<i64>,
}

/// Update the latency + thinking-time columns on an
/// already-persisted message row. Called from the frontend's
/// `streamController.handleChatEvent("done")` after the four
/// `Date.now()` deltas resolve (TTFB / gen / total +
/// thinking). Updates the assistant row's four columns in
/// one SQL statement; a no-op if the message id is unknown
/// (defensive — the controller could in principle race the
/// agent loop's `persist_turn` if the user cancels mid-stream
/// and the cancel cleanup path persists the partial turn at
/// a later time).
///
/// The `id` is the SQLite `messages.id` (auto-incrementing). The
/// controller tracks this via the `seq` on the assistant message;
/// the IPC layer looks up the id by `(session_id, seq)` and passes
/// it here. See `find_message_id_by_seq` for the helper.
pub async fn update_message_latency(
    pool: &SqlitePool,
    message_id: i64,
    latency: &MessageLatency,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
 UPDATE messages
 SET ttfb_ms = ?, gen_ms = ?, total_ms = ?, thinking_ms = ?
 WHERE id = ?
 "#,
    )
    .bind(latency.ttfb_ms)
    .bind(latency.gen_ms)
    .bind(latency.total_ms)
    .bind(latency.thinking_ms)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Find a session message's auto-incrementing row id by its
/// caller-managed `seq`. Used by the F5 `update_message_latency`
/// IPC: the frontend tracks the seq of the assistant placeholder
/// (it appears in `messages.content` as a JSON-serialized
/// `Vec<ContentBlock>`), but doesn't know the SQLite id at
/// stream end (the row was inserted by the agent loop's
/// `persist_turn`, not by the frontend). This helper bridges
/// the two.
pub async fn find_message_id_by_seq(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM messages WHERE session_id = ? AND seq = ?")
            .bind(session_id)
            .bind(seq)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id,)| id))
}

/// B2 PR3: write the per-user-turn `@relpath` injection
/// manifest to `messages.metadata`. Called from the agent loop
/// after `inject_at_tokens` returns the manifest — the
/// `persist_turn` call earlier in the same turn already wrote
/// the row with `metadata: None`, so this is a patch on top of
/// the just-inserted row.
///
/// The function is a single `UPDATE` keyed by
/// `(session_id, seq)`. The frontend rehydrate path reads
/// `metadata` back via `MessageRow.metadata` (see
/// `db::types.rs::MessageRow`) and parses it into the
/// `ChatMessage.injections` array. Bumps no `updated_at` —
/// the message is immutable from the moment it's inserted.
pub async fn update_message_metadata(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    metadata: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let meta_str = serde_json::to_string(metadata)
        .map_err(|e| sqlx::Error::Encode(format!("serialize metadata: {}", e).into()))?;
    sqlx::query(
        r#"
 UPDATE messages
 SET metadata = ?
 WHERE session_id = ? AND seq = ?
 "#,
    )
    .bind(&meta_str)
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await?;
    Ok(())
}

/// Patch the `duration_ms` field onto a `tool_result` content
/// block embedded in `messages.content` JSON, keyed by
/// `tool_use_id`. Per PRD ADR-lite decision 1, the per-tool
/// duration is embedded in the `tool_result` block rather
/// than a column — zero schema change for the tool side.
///
/// The function reads the matching message row, walks the
/// `content` JSON array, finds the `tool_result` block with
/// the matching `tool_use_id`, and writes
/// `{"duration_ms": <n>}` into the block. Other blocks and
/// the rest of the message row are untouched. A missing
/// `tool_use_id` is a no-op (the controller could in principle
/// fire `tool:result` for a tool_use that hasn't been persisted
/// yet, e.g. if the agent loop bails out before `persist_turn`
/// runs — we don't want to surface that as an error).
///
/// Both user-role rows that carry `tool_result` blocks
/// (the post-tool-execution turn the agent loop persists)
/// AND assistant-role rows that were repaired by the
/// 2013-orphan fix are supported: the search walks every
/// `tool_result` block in the row's content array, so a
/// durationMs patch lands on whichever row holds the
/// matching block.
pub async fn record_tool_duration(
    pool: &SqlitePool,
    session_id: &str,
    tool_use_id: &str,
    duration_ms: i64,
) -> Result<bool, sqlx::Error> {
    // Load every message row in the session that has tool_results,
    // patch the matching block in memory, and UPDATE the row if
    // the patch landed. SQLite's `json_patch` is also an option
    // (no Rust-side parsing), but loading + writing in Rust keeps
    // the patch logic readable and gives a free `did we actually
    // find a block` boolean for the IPC return value.
    let rows = sqlx::query(
        r#"
 SELECT id, content FROM messages
 WHERE session_id = ? AND has_tool_results =1
 ORDER BY seq ASC
 "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: i64 = row.try_get("id")?;
        let content_str: String = row.try_get("content")?;
        let mut value: serde_json::Value = match serde_json::from_str(&content_str) {
            Ok(v) => v,
            Err(_) => continue, // corrupt content — skip silently
        };
        let Some(blocks) = value.as_array_mut() else {
            continue;
        };
        let mut patched = false;
        for block in blocks.iter_mut() {
            let Some(obj) = block.as_object_mut() else {
                continue;
            };
            let is_tool_result = obj.get("type").and_then(|v| v.as_str()) == Some("tool_result");
            if !is_tool_result {
                continue;
            }
            let matches = obj.get("tool_use_id").and_then(|v| v.as_str()) == Some(tool_use_id);
            if !matches {
                continue;
            }
            obj.insert(
                "duration_ms".to_string(),
                serde_json::Value::Number(duration_ms.into()),
            );
            patched = true;
        }
        if !patched {
            continue;
        }
        let new_content = serde_json::to_string(&value)
            .map_err(|e| sqlx::Error::Encode(format!("re-serialize content: {}", e).into()))?;
        sqlx::query("UPDATE messages SET content = ? WHERE id = ?")
            .bind(&new_content)
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// D3 (session 内消息编辑/重发, PR1 2026-06-17):
// edit_user_message — in-place content patch + cascade-delete tail + audit
// ---------------------------------------------------------------------------

/// Edit a single user message in place: replace its `content` / `text`
/// with the new value, stamp `messages.metadata` with `edited_at` and
/// `original_content`, cascade-delete every strictly later message
/// (so the next resend starts from a clean slate — the assistant
/// tool_use chain on row N+1+ no longer references the old prompt),
/// and append an `edit_message` audit row.
///
/// All three operations (UPDATE message + DELETE tail + INSERT audit)
/// run inside a single `sqlx::Transaction` so a partial failure cannot
/// leave the DB in a split-brain state (e.g. content updated but tail
/// not deleted → assistant turn still references the old prompt).
/// Matches the `emit_persist_failure` single-rollback invariant the
/// agent loop uses for its own persist sites (RULE-A-003, 2026-06-15).
///
/// ### No-op fast path
///
/// If the new `content` serializes to the same JSON as the current
/// row's `content`, the function is a no-op: it returns `Ok(())`
/// without writing any state. The caller (the `edit_user_message`
/// Tauri command) sees success and the audit log gets no row —
/// this avoids spurious audit entries on save-without-change clicks.
///
/// ### `original_content` semantics
///
/// `original_content` is the JSON-serialized value of the row BEFORE
/// this edit. The first edit on a row writes `original_content` from
/// the previously-stored `content`; subsequent edits (re-edit of an
/// already-edited row) do NOT overwrite `original_content` — it
/// always points at the original (pre-any-edit) value. This gives a
/// future "undo edit" affordance a stable restore target.
///
/// ### `edited_at` semantics
///
/// `edited_at` is the RFC3339 timestamp of the latest edit on this
/// row. It is overwritten on every edit (so the UI can show "last
/// edited at X"). NULL for never-edited rows.
///
/// ### Cascade delete scope
///
/// `DELETE FROM messages WHERE session_id = ? AND seq > ?` removes
/// every strictly-later message in the session — assistant turns,
/// tool_result turns, the synthetic tool_result orphan-repair rows,
/// etc. The `messages` table has no FKs to other tables (just an
/// index on `(session_id, seq)`), so a single DELETE is enough — no
/// other table holds a reference to a `messages.id`. Audit events
/// (`session_audit_events`) are NOT touched: they record what the
/// agent DID, not the live message buffer, so they survive the
/// cascade delete (mirrors `delete_messages_by_session` semantics in
/// `B3 /clear`, `sessions.rs:265-274`).
///
/// ### Atomicity
///
/// A single `sqlx::Transaction` wraps the entire flow. If any of
/// the three SQL calls fails, the transaction is dropped (sqlx
/// auto-rollback on Drop) and the function returns the underlying
/// `sqlx::Error`. The caller wraps the error in
/// `emit_persist_failure`-style error handling.
///
/// ### Permission
///
/// This function does NOT consult the ⑨ 关 permission layer. Edit is
/// a user-initiated direct IPC call, not an LLM tool invocation; the
/// industry consensus (Cursor / Cline / Cody / OpenHands / OpenCode;
/// see `.trellis/tasks/06-17-d3-message-edit-resend/research/industry-edit-resend.md`)
/// is to bypass the modal entirely. The audit log captures every
/// edit so the user can review changes later.
///
/// ### Args
///
/// - `session_id` — the session containing the row to edit. The
///   cascade delete is scoped to this session.
/// - `message_seq` — the caller-managed `seq` of the user message to
///   edit. Resolved to the auto-incrementing `id` via
///   `find_message_id_by_seq`. Returns `Ok(())` silently if the
///   pair is unknown (defensive — the frontend's view can race the
///   agent loop's persist on a mid-stream edit/cancel).
/// - `new_content` — the new `MessageContent` to write. The
///   `text` column is denormalized from `MessageContent::to_text()`
///   (excludes thinking text per the project invariant).
pub async fn edit_user_message(
    pool: &SqlitePool,
    session_id: &str,
    message_seq: i64,
    new_content: &MessageContent,
) -> Result<(), sqlx::Error> {
    // 1. Resolve (session_id, seq) → message_id. The same helper
    // F5 uses for the latency IPC; the seq is unique per session
    // by the UNIQUE(session_id, seq) constraint. Returns Ok(())
    // silently on unknown pair to mirror `update_message_latency`'s
    // defensive no-op contract.
    let message_id = match find_message_id_by_seq(pool, session_id, message_seq).await? {
        Some(id) => id,
        None => return Ok(()),
    };

    // 2. Read the current `content` for the no-op check + for
    // the `original_content` backup. We need it inside the
    // transaction (concurrent edit/cancel races) so a later
    // writer doesn't sneak in between the read and the UPDATE.
    let mut tx = pool.begin().await?;

    let current_content_str: Option<String> =
        sqlx::query_scalar("SELECT content FROM messages WHERE id = ? AND session_id = ?")
            .bind(message_id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    let current_content_str = match current_content_str {
        Some(s) => s,
        // The row vanished between the find_message_id_by_seq and
        // the SELECT (e.g. concurrent cascade delete). No-op.
        None => {
            tx.rollback().await?;
            return Ok(());
        }
    };

    // 3. No-op fast path: if the new content serializes to the
    // same JSON as the current row, return without writing. This
    // keeps the audit log clean on save-without-change clicks and
    // avoids spurious `edited_at` bumps.
    let new_content_json = serde_json::to_string(new_content)
        .map_err(|e| sqlx::Error::Encode(format!("serialize content: {}", e).into()))?;
    if new_content_json == current_content_str {
        tx.rollback().await?;
        return Ok(());
    }

    // 4. Read the current metadata (if any) to decide whether to
    // seed `original_content` from the pre-edit value (first edit
    // only — subsequent edits preserve the original). We use
    // SQLite's `json_extract` so the parse stays on the SQL side.
    let existing_edited_at: Option<String> = sqlx::query_scalar(
        r#"
 SELECT json_extract(metadata, '$.edited_at')
 FROM messages WHERE id = ?
 "#,
    )
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await?;
    let already_edited = existing_edited_at.is_some();

    // 5. Build the new metadata JSON. `edited_at` is always
    // overwritten (latest edit timestamp); `original_content` is
    // seeded on the FIRST edit only — subsequent edits preserve
    // the original so a future "undo edit" affordance can restore
    // the pre-any-edit text.
    //
    // SQLite's `json_patch` (RFC 7396) merges the patch into the
    // existing metadata object. When the existing metadata is
    // `NULL` (no prior metadata), `json_patch` returns the patch
    // object directly — no extra branch needed.
    let now = Utc::now().to_rfc3339();
    let metadata_patch = if already_edited {
        serde_json::json!({ "edited_at": &now }).to_string()
    } else {
        // First edit: parse the current content as JSON (it's the
        // serialized `MessageContent`). If parsing fails, fall back
        // to the string form so the backup is never lossy.
        let original_content_value = serde_json::from_str(&current_content_str)
            .unwrap_or_else(|_| serde_json::Value::String(current_content_str.clone()));
        serde_json::json!({
        "edited_at": &now,
        "original_content": original_content_value,
        })
        .to_string()
    };
    let new_metadata_json: String = sqlx::query_scalar(
        r#"
 SELECT json_patch(COALESCE(metadata, '{}'), ?)
 FROM messages WHERE id = ?
 "#,
    )
    .bind(&metadata_patch)
    .bind(message_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_text = new_content.to_text();
    sqlx::query(
        r#"
 UPDATE messages
 SET content = ?, text = ?, metadata = ?
 WHERE id = ? AND session_id = ?
 "#,
    )
    .bind(&new_content_json)
    .bind(&new_text)
    .bind(&new_metadata_json)
    .bind(message_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;

    // 6. Cascade-delete every strictly-later message in this
    // session. This wipes the (now-stale) assistant turn, the
    // tool_result turns, the orphan-repair rows — everything that
    // chained off the old user prompt. The next resend starts
    // from a clean slate.
    //
    // Single-table FK story: `messages` has no outgoing FKs to
    // other tables (only an index on `(session_id, seq)`), so the
    // DELETE doesn't cascade anywhere. Audit events
    // (`session_audit_events`) are session-scoped and intentionally
    // kept — they record what the agent DID, not the live
    // message buffer.
    sqlx::query("DELETE FROM messages WHERE session_id = ? AND seq > ?")
        .bind(session_id)
        .bind(message_seq)
        .execute(&mut *tx)
        .await?;

    // 7. Audit row. Single INSERT into `session_audit_events`
    // with kind `edit_message` (mirrors the
    // `AuditKind::EditMessage` enum string in
    // `agent::permissions::AuditKind::as_str`). The chat command
    // path uses the string literal directly so the cross-module
    // call graph stays tight (same pattern as
    // `set_session_mode`'s `mode_changed` audit).
    let audit_payload = serde_json::json!({
    "message_seq": message_seq,
    "new_text_preview": new_text.chars().take(80).collect::<String>(),
    "edited_at": &now,
    })
    .to_string();
    sqlx::query(
        r#"
 INSERT INTO session_audit_events
 (session_id, ts, kind, payload_json, turn_seq)
 VALUES (?, datetime('now'), 'edit_message', ?, NULL)
 "#,
    )
    .bind(session_id)
    .bind(&audit_payload)
    .execute(&mut *tx)
    .await?;

    // 8. Commit. Any error in steps 2-7 leaves the transaction
    // uncommitted and sqlx drops it (auto-rollback on Drop),
    // giving the caller a clean `sqlx::Error` to wrap in
    // `emit_persist_failure`.
    tx.commit().await?;
    Ok(())
}
