//! `ChatEventSink` trait 实现(拆分自 sink.rs,08-08-a-class-sink-split)。
//!
//! 5 个 emit 方法 + `record_worker_messages`:方法整体平移自原 impl 块,
//! 锁序与 emit 顺序零变化(所有 mutex 均为方法内单锁或顺序锁,无嵌套持有)。

#![allow(unused_imports)]
use std::collections::HashMap;
use std::time::Instant;

use crate::agent::permissions::PermissionAskPayload;
use crate::llm::types::ChatEvent;
use crate::state::{ChatEventPayload, ToolCallPayload, ToolResultPayload};

use super::super::transcript::TranscriptKind;
use super::SubagentBufferSink;

impl crate::state::ChatEventSink for SubagentBufferSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        // Track terminal signals + accumulate text deltas for the
        // final summary.
        match &payload.event {
            ChatEvent::Delta { text } => {
                self.text_parts
                    .lock()
                    .expect("SubagentBufferSink text_parts mutex poisoned")
                    .push(text.clone());
            }
            ChatEvent::Error { .. } => {
                self.had_error
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ChatEvent::Done { stop_reason, usage } => {
                // Capture per-turn token usage for the worker run's
                // `subagent_runs.token_usage_json` (written at worker
                // exit by `dispatch.rs::run_subagent`).
                //
                // 2026-06-26 (RULE-A-015 reversal): the parent's
                // `sessions.last_*` snapshot is NO LONGER updated
                // by the worker. `update_last_turn_usage` is back
                // inside the `!skip_persist` gate at `chat_loop.rs`,
                // so worker turns (which run with `skip_persist=true`)
                // don't touch the parent's snapshot. Worker token
                // usage stays isolated in `per_turn_usage` here +
                // the eventual `token_usage_json` write.
                //
                // 2026-06-21 (R3): synthetic terminals
                // (`max_turns` / `cancelled`) are emitted with
                // `usage = last_usage` for `max_turns` (see
                // `chat_loop.rs:1797-1804`). The prior per-turn
                // Done for the final turn ALREADY pushed its
                // `usage: Some(t)` into the Vec; pushing again
                // here would double-count the last turn. The
                // stop_reason guard skips the push for synthetic
                // terminals so the Vec holds exactly one entry
                // per real per-turn Done, no more.
                if let Some(u) = usage {
                    if stop_reason.as_deref() != Some("cancelled")
                        && stop_reason.as_deref() != Some("max_turns")
                    {
                        self.per_turn_usage
                            .lock()
                            .expect("SubagentBufferSink per_turn_usage mutex poisoned")
                            .push(*u);
                        // 2026-06-22 (RULE-FrontSubagent-004):
                        // increment the turn counter on the SAME
                        // discriminator as the `per_turn_usage`
                        // push so the two stay 1:1
                        // (turns_completed() == per_turn_usage.len()
                        // at worker exit). Synthetic terminals
                        // (cancelled / max_turns) do NOT increment
                        // because they reuse the prior turn's
                        // usage (would double-count). The counter
                        // thus always reflects the actual count of
                        // real per-turn Dones — even when the
                        // worker exited via the soft-cap or cancel.
                        self.turns_completed
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                if stop_reason.as_deref() == Some("cancelled")
                    || stop_reason.as_deref() == Some("max_turns")
                {
                    // Treat max_turns as a soft "ran out of budget"
                    // — the worker did useful work but didn't
                    // cleanly finish. The summary still carries
                    // whatever it produced. Status prefix =
                    // "incomplete" with a note appended (R2
                    // 2026-06-21); for cancelled (user Stop
                    // propagated to worker) we use status=cancelled.
                    if stop_reason.as_deref() == Some("cancelled") {
                        self.was_cancelled
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    } else if stop_reason.as_deref() == Some("max_turns") {
                        // 2026-06-21 (R2): distinct from
                        // `was_cancelled` so `run_subagent`'s
                        // status picker can distinguish the
                        // budget-exhaustion path from the
                        // clean-failure path. Mutually exclusive
                        // with `was_cancelled` in practice.
                        self.was_incomplete
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                } else if stop_reason.as_deref() == Some("loop_terminated") {
                    // C2+ (2026-07-05, task `07-05-c2-loop-active-
                    // intervention` PR3): the worker took the
                    // C2+ direct-break short-circuit (consecutive
                    // loop-detection hits ≥ 3). Mutually exclusive
                    // with `was_cancelled` / `was_incomplete` /
                    // `had_error` in practice — the worker's C2+
                    // branch emits exactly one terminal `Done`.
                    // `run_subagent` reads this to (1) pick the
                    // `status: incomplete` prefix (the worker did
                    // not cleanly finish — it was force-stopped by
                    // the harness mid-loop) and (2) append the
                    // `[loop terminated: ...]` line to the
                    // dispatch_result content so the parent LLM
                    // sees the loop-termination signal.
                    self.was_loop_terminated
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            _ => {}
        }
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::ChatEvent, payload_json);
    }

    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        // B6 PR3 redesign (2026-06-21): record the `Instant` of this
        // tool_call so the paired `emit_tool_result` can compute the
        // wall-clock `duration_ms`. The frontend drawer pairs the
        // two transcript entries by `tool_use_id` and renders the
        // duration in the merged card header (see
        // `.trellis/tasks/06-21-redesign-subagent-drawer-entry-as-toolcard-style/prd.md`
        // §"Technical Approach"). The Instant is the wall-clock now
        // (`Instant::now()`), not the message's emit timestamp —
        // matches the main panel's `ToolCallCard` duration contract
        // (F5), which is "from tool_call to tool_result wall-clock".
        let mut map = self
            .tool_call_received_at
            .lock()
            .expect("SubagentBufferSink tool_call_received_at mutex poisoned");
        map.insert(payload.id.clone(), Instant::now());
        // Defensive cap: if a worker ever produces a runaway number
        // of distinct tool_use_ids without results landing (e.g. an
        // error-loop worker spamming tool_use), bound the map. The
        // 1024 cap is generous for the 20-turn worker's realistic
        // case (a busy tool-heavy turn produces ~5-10 distinct
        // tool_use_ids). The eviction policy is "drop oldest entry"
        // to keep the most recent measurements intact.
        if map.len() > 1024 {
            if let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, v)| v.elapsed())
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest_key);
            }
        }
        drop(map);
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        // Inject the `tool_use_id` field at the top level of
        // payload_json so the frontend can pair tool_call with the
        // matching tool_result. The original `ToolCallPayload` does
        // not serialize `id` separately (it has `request_id` and
        // `id`, but the frontend `TranscriptEntry` projection
        // historically only exposed `payload_json.{name,input}` for
        // tool_call — see `subagentRuns.ts:TranscriptEntry`). Adding
        // the field at serialization time keeps the Rust struct
        // stable for cross-process Tauri commands (no DB migration
        // needed — see PRD §"Cross-layer Decision Points").
        let mut payload_obj = match payload_json {
            serde_json::Value::Object(m) => m,
            other => {
                tracing::warn!(
                    tool_use_id = %payload.id,
                    "tool_call payload_json not an object; wrapping as-is"
                );
                let mut m = serde_json::Map::new();
                m.insert("raw".into(), other);
                m
            }
        };
        payload_obj.insert(
            "tool_use_id".into(),
            serde_json::Value::String(payload.id.clone()),
        );
        let enriched = serde_json::Value::Object(payload_obj);
        self.record(TranscriptKind::ToolCall, enriched);
    }

    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        // B6 PR3 redesign (2026-06-21): look up the matching
        // `tool_call` Instant, compute the wall-clock gap, and embed
        // it (plus `tool_use_id`) into payload_json so the frontend
        // drawer can render the per-tool duration on the merged
        // card header. Orphan tool_result (no matching tool_call —
        // possible if the IPC `subagent:event` was lost or the
        // transcript was truncated at the 4 MiB cap) falls back to
        // `duration_ms = 0` with a `tracing::warn!`; the entry
        // still lands in the transcript so the user sees the result,
        // the drawer's pairing layer treats it as a standalone
        // "orphan result" card.
        let mut map = self
            .tool_call_received_at
            .lock()
            .expect("SubagentBufferSink tool_call_received_at mutex poisoned");
        let duration_ms: u64 = if let Some(start) = map.remove(&payload.tool_use_id) {
            let ms = start.elapsed().as_millis();
            // Saturating cast — a `u128` ms value cannot realistically
            // exceed `u64::MAX`, but the saturating cast keeps the
            // conversion safe under any pathological clock behavior.
            u64::try_from(ms).unwrap_or(u64::MAX)
        } else {
            tracing::warn!(
                tool_use_id = %payload.tool_use_id,
                "tool_result arrived without matching tool_call; duration_ms=0"
            );
            0
        };
        drop(map);
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        // Enrich payload_json with `tool_use_id` (top-level) +
        // `duration_ms` so the frontend pairing layer can locate the
        // matching call and render the duration. The Rust struct
        // `ToolResultPayload` does not derive `tool_use_id` at the
        // top level (it has `request_id` + `tool_use_id` as separate
        // fields, but the original `TranscriptEntry` projection in
        // `subagentRuns.ts` only exposed
        // `payload_json.{content,is_error}`). Adding the field at
        // serialization time keeps the Rust struct stable.
        let mut payload_obj = match payload_json {
            serde_json::Value::Object(m) => m,
            other => {
                tracing::warn!(
                    tool_use_id = %payload.tool_use_id,
                    "tool_result payload_json not an object; wrapping as-is"
                );
                let mut m = serde_json::Map::new();
                m.insert("raw".into(), other);
                m
            }
        };
        payload_obj.insert(
            "tool_use_id".into(),
            serde_json::Value::String(payload.tool_use_id.clone()),
        );
        payload_obj.insert(
            "duration_ms".into(),
            serde_json::Value::Number(duration_ms.into()),
        );
        let enriched = serde_json::Value::Object(payload_obj);
        self.record(TranscriptKind::ToolResult, enriched);
    }

    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        // 2026-06-22 (RULE-FrontSubagent-003 fix): worker asks now
        // go through the full interactive round-trip
        // (`register_ask + tokio::select!{cancel, timeout, oneshot}`)
        // instead of auto-denying at the Tier 4 is_worker collapse.
        //
        // The ask is delivered to the frontend over TWO channels:
        //   1. `permission:ask` (emitted below when AppHandle is
        //      present) → consumed by `usePermissionsStore` →
        //      live pending entry (`pendingWorkerByRunId`) →
        //      interactive Allow/Deny card in `<SubagentDrawer>`
        //      + `<WorkerAskBanner>` counter.
        //   2. `subagent:event` (via self.record below) →
        //      transcript, consumed by `useSubagentRunsStore` →
        //      historical render in the drawer (also captures
        //      the ask when no AppHandle is wired, e.g. in unit
        //      tests that use the test collector).
        //
        // Both channels carry the same rid; the drawer dedups by
        // rid (interactive while the permissions store has it
        // pending, historical once resolved). The dual emit is
        // the correct separation: worker chat events stay on
        // `subagent:event` (don't pollute the main chat), while
        // `permission:ask` is the shared approval channel both
        // main-chat and worker asks use.
        //
        // The resolve side (user Allow / Deny / timeout / cancel)
        // does NOT write a follow-up audit row to the parent's
        // `session_audit_events` per RULE-A-016 — the transcript
        // is the worker's audit-like record.
        //
        // PR1.5 (2026-06-22): emit BEFORE `record()` so the
        // frontend permissions store is armed before/alongside
        // the transcript entry (avoids a render race where the
        // transcript card appears historical before the live
        // entry lands). Both are synchronous emits so ordering
        // is minor, but emit-first is the safer choice.
        //
        // transport-abstraction 2026-07-20 (P1.3): route through
        // the `SubagentEventSink` trait instead of branching on
        // `Option<AppHandle>`. The trait impl handles
        // `app.emit` (production) or no-op (test) uniformly.
        self.event_sink.emit_permission_ask(&payload);
        // Test-only: when no app_handle is wired, the payload is
        // still captured via the transcript record below (test
        // collectors inspect transcript entries). The IPC emit
        // path is exercised in integration, not unit, tests.
        let payload_json = serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null);
        self.record(TranscriptKind::PermissionAsk, payload_json);
    }

    /// 2026-06-22 (RULE-WorkerAsk-001): trait override of
    /// `ChatEventSink::emit_permission_ask_resolved`. Records the
    /// worker's `PermissionAsk` resolve outcome as a
    /// `PermissionAskResolved` transcript entry. Called by
    /// `ask_path`'s worker branch AFTER its `tokio::select!` arm
    /// returns its outcome.
    ///
    /// **Transcript-only** (no dual IPC emit). The live
    /// interaction card's disappearance is driven by the
    /// permissions store removing the pending entry on resolve
    /// (Session 62 `89e5ba1`). This transcript entry is the
    /// **historical-replay record** — when the user reopens the
    /// drawer after the worker exits, the frontend pairs this
    /// entry to the matching ask by `rid` and surfaces the
    /// outcome as a badge on the card.
    ///
    /// **No audit** (RULE-A-016): worker resolve events stay in
    /// the transcript, NOT in `session_audit_events`.
    ///
    /// `outcome` is one of `"allow"` / `"deny"` / `"timeout"` /
    /// `"cancel"` (DEBT-locked four-state wire). The caller
    /// (`ask_path` worker branch) maps its `tokio::select!` arm
    /// to the appropriate outcome string before calling this.
    fn emit_permission_ask_resolved(&self, rid: &str, outcome: &str) {
        self.record_permission_ask_resolved(rid, outcome);
    }

    /// C1 (07-26-subagent-resume): stash the worker's final messages
    /// snapshot. Called ONCE by `run_chat_loop` on its normal
    /// completion path (gated on `is_worker == Some(true)`).
    /// Overwrites any prior snapshot — the snapshot is the complete
    /// history at loop exit, not an append log. `run_subagent` reads
    /// it via `worker_messages()` after the loop returns.
    fn record_worker_messages(&self, messages: &[crate::llm::types::ChatMessage]) {
        *self
            .worker_messages
            .lock()
            .expect("SubagentBufferSink worker_messages mutex poisoned") = messages.to_vec();
    }
}
