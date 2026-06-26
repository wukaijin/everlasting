//! `ask_path` — the Tier 4 interactive ask round-trip (IPC emit
//! + oneshot / timeout / cancel). Worker + parent paths. Plus
//! `build_ask_reason` + the `WorkerAskTerminal` / `ASK_TIMEOUT`
//! constants. Split out of `mod.rs` on 2026-06-23.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::state::ChatEventSink;

use super::audit::{record_audit, AuditKind};
use super::check::match_value_for_allow_always;
use super::payload::PermissionAskPayload;
use super::store::{register_ask, resolve_ask, PermissionStore};
use super::types::{risk_for_tool, Decision, PermissionContext, PermissionResponse, Risk};

/// Default timeout for Tier 3 user response. Matches PRD
/// `### IPC 异常路径` "用户从不响应" → 120s auto-deny.
pub const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Internal error type for the worker's `tokio::select!` arm
/// (2026-06-22, RULE-FrontSubagent-003 fix). The 3-arm select
/// in the worker branch of `ask_path` produces a
/// `Result<PermissionResponse, WorkerAskTerminal>` where:
/// - `Ok(resp)` is either a user response or a synthetic Deny
///   injected by the cancel / timeout arm (the match block
///   later distinguishes cancel / timeout / user-deny by
///   inspecting the synthetic reason string).
/// - `Err(WorkerAskTerminal::OneshotDropped)` is a
///   `oneshot::RecvError` — the sender was dropped before
///   delivering (e.g. `cancel_session_asks` ran on the worker's
///   permission session id, or `resolve_ask` removed the
///   pending entry by a different path). Treated as Deny by
///   the match block (consistent with the parent path's
///   `Err(_) → Deny` arm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerAskTerminal {
    /// The oneshot sender was dropped before delivering.
    /// Equivalent to the parent path's `oneshot::RecvError`
    /// case.
    OneshotDropped,
}

/// Emit `permission:ask` + await the user's response (or
/// timeout). Centralizes the Tier 4 ask path so the three
/// branches (path / shell / web_fetch) share the same IPC
/// flow.
///
/// **Wire path-field semantics (re-grill §1, 2.4 check)**:
///
/// - `path_or_cmd` is the full argument string the user needs
///   to see in the modal (path for path tools, command for
///   shell, URL for web_fetch). It is ALWAYS used for
///   `build_ask_reason` and `match_value_for_allow_always` —
///   both need the "what did the LLM try to do" text, not
///   a path-only scope.
/// - `path_for_modal` is the **optional** string to surface in
///   the `PermissionAskPayload.path` field — the field the
///   frontend's `<PermissionModal>` reads to render the
///   "path scope" row (in-repo / out-of-repo badge). The
///   frontend's `v-if="hasPath"` hides the row entirely when
///   the field is absent (the struct has
///   `#[serde(skip_serializing_if = "Option::is_none")])`.
///   Per the re-grill spec, **only path tools** populate this
///   field (read_file / write_file / edit_file / list_dir /
///   grep / glob). Shell and web_fetch pass `None` because
///   the modal renders the command / URL inline via
///   `toolInput` (no separate "path scope" row) — surfacing
///   a misleading "仓库外" badge for a shell command or URL
///   is a UX bug.
pub(super) async fn ask_path(
    sink: &Arc<dyn ChatEventSink>,
    db: &SqlitePool,
    store: &PermissionStore,
    ctx: &PermissionContext,
    tool_name: &str,
    tool_input: &serde_json::Value,
    path_or_cmd: &str,
    path_for_modal: Option<&str>,
    tool_use_id: &str,
    token: &tokio_util::sync::CancellationToken,
) -> Decision {
    // 2026-06-22 (RULE-FrontSubagent-003 fix): worker subagents
    // now go through the full interactive ask round-trip (not the
    // pre-fix collapse-to-Deny). The pre-fix behavior silently
    // denied all worker asks — UX-correctness wise it was
    // over-conservative (Claude Code / Cline / opencode all let
    // workers surface ask modals). Three things had to be true
    // before re-enabling this path:
    //
    // 1. **Worker has its own permission session id** — the worker's
    //    oneshot lives under `format!("worker:{}", worker_run_id)`,
    //    so worker asks do NOT pollute the parent's
    //    `permission_asks` map. The parent's IPC handler (`resolve_ask`)
    //    keys by `rid` alone; we ensure worker's `rid` is unique by
    //    prefixing with the worker permission session id internally
    //    (the wire `rid` is still a UUID the frontend echoes back).
    // 2. **Cancellation token is parent-derived child** — `run_subagent`
    //    creates `parent_token.child_token()` (chat_loop.rs:2166) and
    //    passes it as the worker's `token`. So user Stop on parent
    //    propagates to the worker's `tokio::select!` cancel arm.
    //    Switching sessions (no user action) does NOT cancel — the
    //    parent token is only cancelled by explicit user Stop
    //    (the C1 cancel path). Switching sessions keeps the ask alive
    //    so the user can switch back and respond.
    // 3. **Audit not polluting parent's C4 audit log** — worker
    //    decisions go to the worker's `subagent_runs.transcript_json`
    //    via the sink's `emit_permission_ask` impl (RULE-A-016
    //    2026-06-20 already established this). The 4 new AuditKind
    //    variants (`WorkerAskAllowed` / `WorkerAskDenied` /
    //    `WorkerAskTimedOut` / `WorkerAskCancelled`) record the
    //    resolve side on the parent's `session_audit_events` table
    //    (the parent is the "owning" audit log even though the
    //    decision was for a worker — the user reviews the parent
    //    session's audit log and sees "worker X was allowed / denied
    //    tool Y" as part of the parent session timeline).
    //
    // Yolo bypass: still applies (Yolo never reaches Tier 4
    // thanks to the Tier 4 bypass at line ~550 in `check`). A
    // worker under Yolo mode never enters `ask_path` — workers
    // inherit parent's mode via `run_subagent`'s `permission_ctx`
    // build (chat_loop.rs:386).
    if ctx.is_worker {
        let worker_run_id = ctx.worker_run_id.clone().unwrap_or_else(|| {
            // Defensive: is_worker=true MUST carry worker_run_id.
            // `run_chat_loop` is only invoked as a worker through
            // `run_subagent` (subagent/dispatch.rs) which threads
            // worker_run_id. If this panic fires, a new caller has
            // been added without threading the field — fix the
            // caller, not this default.
            tracing::error!(
                session_id = %ctx.session_id,
                tool = %tool_name,
                "permissions::ask_path: is_worker=true but worker_run_id is None; \
                 check that run_chat_loop is called with worker_run_id Some(...)"
            );
            String::from("UNKNOWN_WORKER")
        });
        // Internal permission_session_id — scopes the oneshot map
        // entry (via register_ask below) to the worker so it cannot
        // collide with the parent's pending asks. This composite is
        // INTERNAL to the register_ask/resolve_ask store keying only.
        //
        // The IPC payload's `session_id` field MUST be the PARENT
        // session id (ctx.session_id), NOT this composite — the
        // frontend `WorkerAskBanner` filters worker asks by
        // `ask.sessionId === parentSessionId`
        // (`permissions.ts::pendingWorkerCountForSession`); a
        // composite value there breaks the filter and the banner
        // never renders (PR1.5 cross-layer fix, 2026-06-22).
        let permission_session_id = format!("worker:{}", worker_run_id);

        let rid = uuid::Uuid::new_v4().to_string();
        let risk = risk_for_tool(tool_name);
        let reason = build_ask_reason(tool_name, path_or_cmd, risk);
        let payload = PermissionAskPayload {
            rid: rid.clone(),
            // Parent session id — the banner groups worker asks by
            // parent session. The composite `permission_session_id`
            // ("worker:<id>") is INTERNAL to register_ask/resolve_ask
            // store keying only (line ~1151); it must NOT leak onto
            // the wire or the frontend's per-session banner filter
            // mismatches.
            session_id: ctx.session_id.clone(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            tool_input: tool_input.clone(),
            risk,
            reason: Some(reason.clone()),
            path: path_for_modal.map(|p| p.to_string()),
            worker_run_id: Some(worker_run_id.clone()),
        };
        // Emit the IPC. On the worker path the sink is the
        // SubagentBufferSink (which appends to the worker's
        // transcript + also forwards to the `subagent:event`
        // IPC channel when AppHandle is present — so the frontend
        // SubagentDrawer sees the ask live). The AppHandleSink's
        // emit_permission_ask impl (production non-worker path)
        // is irrelevant here because the worker branch always uses
        // the buffer sink.
        let _ = sink.emit_permission_ask(payload);

        // Register the ask against the worker-owned session_id so the
        // store's pending map separates worker asks from parent asks.
        // (Parent production path uses `ctx.session_id`; worker path
        // uses the prefixed `permission_session_id` so the worker
        // cannot collide with — or be cancelled by — the parent's
        // pending asks.)
        let rx = register_ask(store, &permission_session_id, rid.clone()).await;

        // Three-arm select: parent-derived cancel token / 120s timeout
        // / oneshot response. `biased` ensures the cancel arm is
        // checked first (in case the user hits Stop right at the 120s
        // boundary — without bias, the timeout arm might fire even
        // though cancel was ready).
        //
        // Each arm also encodes the RULE-WorkerAsk-001 outcome wire
        // string (`"allow"` / `"deny"` / `"timeout"` / `"cancel"`) so
        // the post-`match` resolve-emit can route correctly. The
        // outcome strings are DEBT-locked four-state.
        let resp: Result<PermissionResponse, WorkerAskTerminal> = tokio::select! {
            biased;
            _ = token.cancelled() => {
                // Parent cancel (user Stop) → Deny.
                // Drop the pending oneshot to free the map entry.
                //
                // **No audit** (RULE-A-016 lineage): the worker's
                // resolve events stay in the worker's transcript
                // (`SubagentBufferSink`'s `TranscriptKind::PermissionAsk`
                // entry captured via `sink.emit_permission_ask` above),
                // NOT in the parent's `session_audit_events`. Writing
                // a row here would re-introduce the pre-PR3a audit
                // pollution that polluted C4 audit reads with worker
                // ⑨ decisions.
                let mut map = store.lock().await;
                map.remove(&rid);
                drop(map);
                tracing::info!(
                    session_id = %ctx.session_id,
                    worker_run_id = %worker_run_id,
                    tool = %tool_name,
                    "permission::check: worker ask cancelled by parent"
                );
                // RULE-WorkerAsk-001: record the cancel outcome on the
                // worker's transcript so the drawer can surface
                // 「⊘ 已取消」 on the historical card after the worker
                // exits. Transcript-only (no IPC dual emit).
                sink.emit_permission_ask_resolved(&rid, "cancel");
                Ok(PermissionResponse::Deny {
                    reason: "cancelled by parent session stop".to_string(),
                })
            }
            _ = tokio::time::sleep(ASK_TIMEOUT) => {
                // 120s without user response → Deny.
                // Drop the pending oneshot to free the map entry.
                // No audit (RULE-A-016 lineage — see cancel arm).
                let mut map = store.lock().await;
                map.remove(&rid);
                drop(map);
                tracing::warn!(
                    session_id = %ctx.session_id,
                    worker_run_id = %worker_run_id,
                    tool = %tool_name,
                    "permission::check: worker ask timed out after 120s"
                );
                // RULE-WorkerAsk-001: record the timeout outcome on
                // the worker's transcript so the drawer can surface
                // 「⏱ 已超时」 on the historical card.
                sink.emit_permission_ask_resolved(&rid, "timeout");
                // Encode the timeout in a synthetic Deny response so
                // the downstream `match resp` arms know to write the
                // right `Decision::Deny { reason: "permission timed
                // out..." }` (matching the parent-path convention —
                // a user-visible reason string the LLM can read in
                // its tool_result(is_error) content). The downstream
                // match inspects the `reason` to discriminate.
                Ok(PermissionResponse::Deny {
                    reason: "permission timed out after 120s, treat as denied".to_string(),
                })
            }
            resp = rx => resp.map_err(|_| WorkerAskTerminal::OneshotDropped),
        };

        // Match the oneshot response. Same shape as the parent path's
        // match block below (AllowOnce / AllowAlways / Deny / Err) but
        // with worker-specific audit kinds + no "始终允许" persistence
        // (workers do NOT write to session_tool_permissions — a worker's
        // "始终允许" must NOT become a parent-session grant, that would
        // cross privilege boundaries. The Deny message is also surfaced
        // to the worker LLM as tool_result(is_error) per the standard
        // path — the `Decision::Deny { reason }` return value carries
        // the reason for the agent loop's tool_result construction).
        //
        // RULE-WorkerAsk-001: the oneshot arm (the path NOT already
        // handled by the cancel / timeout select arms above) records
        // its outcome here — allow / deny / cancel (OneshotDropped).
        // The cancel / timeout arms already recorded their outcomes
        // inside the select body (they had the outcome context handy
        // at that point); we skip re-recording here for those paths.
        match resp {
            Ok(PermissionResponse::AllowOnce) => {
                // RULE-WorkerAsk-001: user-approved (AllowOnce).
                sink.emit_permission_ask_resolved(&rid, "allow");
                Decision::Allow
            }
            Ok(PermissionResponse::AllowAlways) => {
                // 2026-06-26 (task 06-26-subagent-per-run-grant): the
                // worker's "always allow" is now persisted to the
                // per-run in-memory grant cache (NOT to
                // `session_tool_permissions` — that would leak the
                // grant across the privilege boundary into the
                // parent session's grant table, which would silently
                // authorize the parent and all future workers). The
                // cache lives for the duration of this worker's
                // `run_chat_loop` call and dies with it.
                //
                // Same `(kind, value)` rule as the parent path
                // (`match_value_for_allow_always`) — tool / prefix /
                // path variants mirror the DB table, so the read
                // path in `check.rs` matches identically.
                //
                // Still no audit (RULE-A-016 lineage — worker grant
                // writes stay in the worker's transcript, not the
                // parent's session_audit_events).
                //
                // RULE-WorkerAsk-001: worker AllowAlways outcome wire
                // string is `"allow"` (Session 62 invariant; the
                // cache write is invisible to the wire).
                if let Some(cache) = &ctx.run_grants {
                    cache.grant_for_run(tool_name, tool_input, path_or_cmd);
                }
                sink.emit_permission_ask_resolved(&rid, "allow");
                tracing::info!(
                    session_id = %ctx.session_id,
                    worker_run_id = %worker_run_id,
                    tool = %tool_name,
                    "permission::check: worker ask AllowAlways (cached per-run — \
                     worker grants do not extend parent session permissions)"
                );
                Decision::Allow
            }
            Ok(PermissionResponse::Deny { reason }) => {
                // Distinguish three Deny sources via the synthetic
                // reason string the select! arms wrote. No audit
                // (RULE-A-016 lineage — worker's resolve events stay
                // in the transcript, not in parent's session_audit_events).
                //
                // RULE-WorkerAsk-001: the cancel / timeout select arms
                // already recorded their outcome strings ("cancel" /
                // "timeout") inside the select body — we skip re-
                // recording here. Only the user-initiated Deny path
                // records its outcome ("deny") at this layer.
                if reason == "cancelled by parent session stop"
                    || reason == "permission timed out after 120s, treat as denied"
                {
                    // Outcome already recorded in the select arm.
                    Decision::Deny {
                        reason,
                        critical: false,
                    }
                } else {
                    // User-initiated deny — record the "deny" outcome
                    // for the historical card 「✗ 已拒绝」 badge.
                    sink.emit_permission_ask_resolved(&rid, "deny");
                    let deny_reason = if reason.trim().is_empty() {
                        "user denied".to_string()
                    } else {
                        reason
                    };
                    Decision::Deny {
                        reason: deny_reason,
                        critical: false,
                    }
                }
            }
            Err(WorkerAskTerminal::OneshotDropped) => {
                // oneshot sender dropped (e.g. cancel_session_asks
                // ran on the worker permission session id, or the
                // resolve_ask path dropped the sender before
                // delivering). Same as parent path: treat as Deny.
                // No audit (RULE-A-016 lineage).
                //
                // RULE-WorkerAsk-001: sender-dropped is effectively a
                // cancellation (the receiver will never get a real
                // response). Record the "cancel" outcome wire string
                // — same visual treatment as parent-stop on the
                // historical card.
                sink.emit_permission_ask_resolved(&rid, "cancel");
                Decision::Deny {
                    reason: "permission ask cancelled before response".to_string(),
                    critical: false,
                }
            }
        }
    } else {
        let rid = uuid::Uuid::new_v4().to_string();
        let risk = risk_for_tool(tool_name);
        let reason = build_ask_reason(tool_name, path_or_cmd, risk);
        let payload = PermissionAskPayload {
            rid: rid.clone(),
            session_id: ctx.session_id.clone(),
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            tool_input: tool_input.clone(),
            risk,
            reason: Some(reason.clone()),
            // The `path` field is populated ONLY for path tools
            // (the spec's Q10 "path 范围行" UX). For shell / web_fetch
            // the field is `None` and serde's `skip_serializing_if`
            // keeps it OFF the wire — so the frontend's `v-if="hasPath"`
            // does not render a misleading scope row for non-path
            // asks.
            path: path_for_modal.map(|p| p.to_string()),
            // Parent path: no worker_run_id. Frontend reads the absence
            // (= `workerRunId` is `undefined` on the wire) and routes
            // the ask to the parent session's `<PermissionModal>`
            // (the pre-PR1 behavior).
            worker_run_id: None,
        };
        let _ = sink.emit_permission_ask(payload);
        let _ = record_audit(
            db,
            ctx,
            AuditKind::ToolPermissionAsk,
            tool_name,
            tool_input,
            Some(&reason),
        )
        .await;
        let rx = register_ask(store, &ctx.session_id, rid.clone()).await;
        let resp = tokio::select! {
            biased;
            _ = token.cancelled() => {
                let _ = resolve_ask(
                    store,
                    &rid,
                    PermissionResponse::Deny {
                        reason: "request cancelled by user".to_string(),
                    },
                )
                .await;
                let _ = record_audit( db, ctx, AuditKind::RequestCancelled, tool_name, tool_input, None).await;
                return Decision::Deny {
                    reason: "request cancelled by user".to_string(),
                    critical: false,
                };
            }
            _ = tokio::time::sleep(ASK_TIMEOUT) => {
                let mut map = store.lock().await;
                map.remove(&rid);
                drop(map);
                tracing::warn!(
                    session_id = %ctx.session_id,
                    tool = %tool_name,
                    "permission::check: Tier 4 timed out after 120s"
                );
                let _ = record_audit( db, ctx, AuditKind::PermissionTimeout, tool_name, tool_input, None).await;
                return Decision::Deny {
                    reason: "permission timed out after 120s, treat as denied".to_string(),
                    critical: false,
                };
            }
            resp = rx => resp,
        };
        match resp {
            Ok(PermissionResponse::AllowOnce) => {
                let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                Decision::Allow
            }
            Ok(PermissionResponse::AllowAlways) => {
                // Persist the "always allow" row with the
                // tool-specific match_kind. The match_value is
                // computed by `match_value_for_allow_always`
                // (path → parent/* glob; shell → first token;
                // web_fetch → tool/NULL).
                let (kind, value) = match_value_for_allow_always(tool_name, tool_input, path_or_cmd);
                if let Err(e) = crate::db::grant_tool_permission(
                    db,
                    &ctx.session_id,
                    tool_name,
                    kind,
                    value.as_deref(),
                )
                .await
                {
                    tracing::warn!(
                        error = %e,
                        "permission::check: grant_tool_permission failed (non-fatal)"
                    );
                }
                let _ = record_audit( db, ctx, AuditKind::PermissionGranted, tool_name, tool_input, None).await;
                Decision::Allow
            }
            Ok(PermissionResponse::Deny { reason }) => {
                // Surface the user's optional "拒绝并说明" feedback as the
                // tool_result(is_error) content so the LLM learns *why* it
                // was denied. Empty feedback falls back to "user denied".
                let deny_reason = if reason.trim().is_empty() {
                    "user denied".to_string()
                } else {
                    reason
                };
                let _ = record_audit(
                    db,
                    ctx,
                    AuditKind::ToolDenied,
                    tool_name,
                    tool_input,
                    Some(&deny_reason),
                )
                .await;
                Decision::Deny {
                    reason: deny_reason,
                    critical: false,
                }
            }
            Err(_) => {
                let _ = record_audit( db, ctx, AuditKind::ToolDenied, tool_name, tool_input, None).await;
                Decision::Deny {
                    reason: "permission ask cancelled before response".to_string(),
                    critical: false,
                }
            }
        }
    }
}

/// Build the human-readable reason string shown in the
/// PermissionModal header. Re-grill Q1 path-based: the
/// reason explicitly mentions the path / command / URL so
/// the user can decide without inspecting `toolInput` JSON.
pub(super) fn build_ask_reason(tool_name: &str, path_or_cmd: &str, risk: Risk) -> String {
    if tool_name == "shell" {
        format!(
            "The tool {} requires your confirmation (risk: {}, command: {}).",
            tool_name,
            risk.label_cn(),
            path_or_cmd
        )
    } else if tool_name == "web_fetch" {
        format!(
            "The tool {} requires your confirmation (risk: {}, URL: {}).",
            tool_name,
            risk.label_cn(),
            path_or_cmd
        )
    } else {
        format!(
            "The tool {} requires your confirmation (risk: {}, path: {}).",
            tool_name,
            risk.label_cn(),
            path_or_cmd
        )
    }
}
