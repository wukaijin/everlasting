//! E2 (harness trace pipeline, 2026-07-14) — trace emit + persist helpers.
//!
//! Each `record_*` helper does a **double write**:
//! 1. Emits a `ChatEvent` via the `ChatEventSink` (live panel).
//! 2. Upserts the corresponding `turn_trace` column (回看).
//!
//! Both writes are **always-on** (the live panel may be closed, but
//! the emit is cheap and the DB write is ~ms). DB failures are
//! `warn!`-logged and swallowed — the trace pipeline must NEVER
//! break the agent loop (same best-effort contract as `record_*_audit`).
//!
//! Per-turn token usage does NOT go through a `record_*` helper here
//! — it reuses the existing `ChatEvent::Done { usage }` event (already
//! emitted by the agent loop) and only adds the DB upsert at the
//! `update_last_turn_usage` call site (inside the `!skip_persist` gate).

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::agent::context::CompactResult;
use crate::agent::helpers::emit_chat_event_via_sink;
use crate::llm::types::ChatEvent;
use crate::state::ChatEventSink;

// ---------------------------------------------------------------------------
// record_compaction — C3 context compaction
// ---------------------------------------------------------------------------

/// Record a C3 compaction trace signal. Called from `chat_loop.rs`
/// after `compact_messages` returns, on BOTH the normal compaction
/// branch (`dropped_count > 0`) and the `StillOver` error branch
/// (the turn is about to abort, but the trace row still captures
/// the degradation).
///
/// **Best-effort**: DB upsert failure is `warn!`-logged and
/// swallowed. The emit always fires (even if the DB write fails)
/// so the live panel still sees the signal.
pub async fn record_compaction(
    sink: &Arc<dyn ChatEventSink>,
    db: &SqlitePool,
    rid: &str,
    session_id: &str,
    seq: i64,
    result: &CompactResult,
) {
    let degradation = result.degradation.as_str().to_string();
    let event = ChatEvent::ContextCompacted {
        seq,
        tokens_before: result.tokens_before,
        tokens_after: result.tokens_after,
        dropped_count: result.dropped_count as u32,
        degradation: degradation.clone(),
    };
    emit_chat_event_via_sink(sink, rid, &event);

    let payload = serde_json::json!({
        "tokens_before": result.tokens_before,
        "tokens_after": result.tokens_after,
        "dropped_count": result.dropped_count,
        "degradation": degradation,
    });
    if let Err(e) =
        crate::db::trace::upsert_turn_trace_compaction(db, session_id, seq, &payload).await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            seq,
            "trace: upsert_turn_trace_compaction failed (non-fatal)"
        );
    }
}

// ---------------------------------------------------------------------------
// record_loop_hint — C2 soft hint (1-2 consecutive hits)
// ---------------------------------------------------------------------------

/// Record a C2 loop-detection soft hint. Called from `chat_loop.rs`
/// at the soft-hint write point (when `loop_hit_count` is 1 or 2 and
/// `verdict_kind` is `Some`). The ≥3 intervention path already writes
/// `loop_intervention` audit rows; this helper covers the
/// pre-intervention turns only.
///
/// **Best-effort**: same contract as `record_compaction`.
pub async fn record_loop_hint(
    sink: &Arc<dyn ChatEventSink>,
    db: &SqlitePool,
    rid: &str,
    session_id: &str,
    seq: i64,
    hit_count: u32,
    verdict_kind: &str,
) {
    let event = ChatEvent::LoopHint {
        seq,
        hit_count,
        verdict_kind: verdict_kind.to_string(),
    };
    emit_chat_event_via_sink(sink, rid, &event);

    let payload = serde_json::json!({
        "hit_count": hit_count,
        "verdict_kind": verdict_kind,
    });
    if let Err(e) =
        crate::db::trace::upsert_turn_trace_loop_hint(db, session_id, seq, &payload).await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            seq,
            "trace: upsert_turn_trace_loop_hint failed (non-fatal)"
        );
    }
}

// ---------------------------------------------------------------------------
// record_breadcrumb — workflow breadcrumb snapshot
// ---------------------------------------------------------------------------

/// Record a workflow breadcrumb trace signal. Called from
/// `chat_loop.rs` right after `append_workflow_breadcrumb` (the
/// trace call lives in chat_loop, not in inject.rs, so it has
/// access to `seq` + `db` + `sink`).
///
/// `task_slug` / `status` are `None` when there is no active
/// workflow task (the bootstrap breadcrumb branch).
///
/// **Best-effort**: same contract as `record_compaction`.
pub async fn record_breadcrumb(
    sink: &Arc<dyn ChatEventSink>,
    db: &SqlitePool,
    rid: &str,
    session_id: &str,
    seq: i64,
    task_slug: Option<&str>,
    status: Option<&str>,
    breadcrumb_text: &str,
) {
    let event = ChatEvent::WorkflowBreadcrumb {
        seq,
        task_slug: task_slug.map(|s| s.to_string()),
        status: status.map(|s| s.to_string()),
        breadcrumb_text: breadcrumb_text.to_string(),
    };
    emit_chat_event_via_sink(sink, rid, &event);

    let payload = serde_json::json!({
        "task_slug": task_slug,
        "status": status,
        "breadcrumb_text": breadcrumb_text,
    });
    if let Err(e) =
        crate::db::trace::upsert_turn_trace_breadcrumb(db, session_id, seq, &payload).await
    {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            seq,
            "trace: upsert_turn_trace_breadcrumb failed (non-fatal)"
        );
    }
}
