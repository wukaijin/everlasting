//! P3d — background-shell escalation at the notification drain site
//! (design §2; task `09-01-a2-p3d-background-escalation`).
//!
//! A sandboxed background shell that fails on an out-of-face denial
//! queues its completion notification with an `EscalationOffer`
//! (baked in by the registry's wait task, which owns the full
//! stderr). When the NEXT turn of the owning session
//! drains the queue — i.e. the user just sent a message and is
//! definitionally present — this module resolves each offer BEFORE
//! any turn text is assembled, reusing the exact foreground §5.2
//! machinery (prefix-grant → Ask card → one-shot unsandboxed rerun):
//!
//! 1. **Plan gate**: a Plan turn never escalates (D3 — the
//!    deterministic read-only face must not be pierceable through a
//!    queued notification); the text degrades to failure + Plan
//!    guidance.
//! 2. **Prefix-grant hit** → rerun with NO card (same compound-command
//!    gate as Tier 4 / foreground escalation; grants are shared across
//!    the shell family — read side `IN ('shell','run_background_shell')`).
//! 3. **Ask card** → attached back to the ORIGINAL
//!    `run_background_shell` call via the offer's `tool_use_id` (the
//!    frontend matches `ask.toolUseId === call.id`), 120s timeout,
//!    cancel-safe via the turn token.
//! 4. Each offer produces EXACTLY one terminal text line describing
//!    the resolved state — the LLM never sees a half-open escalation
//!    and never authors the rerun itself (approval binds the exact
//!    command text, D4).

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::agent::permissions::escalation::{
    prefix_grant_hit, EscalationHandle, EscalationOutcome,
};
use crate::agent::permissions::{PermissionContext, PermissionStore};
use crate::background_shell::BackgroundShellNotification;
use crate::background_shell::{BackgroundShellRegistry, DefaultRegistry, EscalationBlock};
use crate::db::Mode;
use crate::state::ChatEventSink;

/// Everything `resolve_all` needs from the turn scope. All fields are
/// already in `drive_turn`'s bindings — this struct just bundles them
/// so the drain site stays one call.
pub(crate) struct EscalationEnv<'a> {
    pub registry: &'a DefaultRegistry,
    pub sink: Arc<dyn ChatEventSink>,
    /// Permission ask store (the QuestionStore-style oneshot registry
    /// `ask_path` resolves against).
    pub store: PermissionStore,
    pub perm_ctx: PermissionContext,
    pub db: SqlitePool,
    pub token: CancellationToken,
    pub session_id: &'a str,
}

/// The LLM-visible injection text for the plain path. MUST stay
/// byte-identical to the pre-P3d format (AC5 regression anchor —
/// `tests_background_escalation::plain_text_matches_legacy_format`).
pub(crate) fn plain_text(note: &BackgroundShellNotification) -> String {
    format!(
        "[system] 后台 shell {} 已完成,exit code {}。调 shell_status(session_id=\"{}\") 看输出。",
        note.shell_session_id,
        exit_code_label(note),
        note.shell_session_id,
    )
}

/// Resolve every drained notification into its terminal injection
/// text, in order. Offers are processed sequentially (each may run a
/// 120s ask round-trip); notifications without an offer pass through
/// untouched.
pub(crate) async fn resolve_all(
    notifications: &[BackgroundShellNotification],
    env: &EscalationEnv<'_>,
) -> Vec<String> {
    let mut texts = Vec::with_capacity(notifications.len());
    for note in notifications {
        texts.push(resolve_one(note, env).await);
    }
    texts
}

async fn resolve_one(note: &BackgroundShellNotification, env: &EscalationEnv<'_>) -> String {
    let Some(offer) = &note.escalation else {
        return plain_text(note);
    };

    // Gate a (design §4): Plan never escalates. The queued offer may
    // outlive the mode it was born under (shell started in Edit,
    // user switched to Plan mid-run) — the gate is evaluated at ask
    // time, matching the foreground `mode != Plan` gate exactly.
    // perm_ctx.mode is THE turn mode (same init-time source the
    // foreground escalation gate reads via ToolContext.mode).
    if env.perm_ctx.mode == Mode::Plan {
        return plain_text(note) + &guidance_suffix(&offer.stderr_evidence, env.perm_ctx.mode);
    }

    // Gate b: the entry was swept between drain and lookup (1h
    // retention vs. an immediate drain makes this near-impossible,
    // but the degradation must be silent and plain).
    let Some(source) = env
        .registry
        .escalation_source(env.session_id, &note.shell_session_id)
        .await
    else {
        return plain_text(note);
    };

    // The card's tool_input snapshot: the command is the only input
    // the ask-side audit rows and the ShellCard ask body need.
    let tool_input = serde_json::json!({ "command": source.command });
    let handle = EscalationHandle::new(
        env.sink.clone(),
        env.store.clone(),
        env.perm_ctx.clone(),
        env.db.clone(),
        env.token.clone(),
        offer.tool_use_id.clone(),
    );

    // (a) prefix-grant hit → rerun directly, no card (foreground §5.2
    //     top branch; the grant namespace is shared across the shell
    //     family by design — a "cargo" AllowAlways granted during a
    //     foreground escalation covers a background `cargo build`).
    let grant_hit = prefix_grant_hit(&env.db, env.session_id, &source.command).await;
    let approved = if grant_hit {
        tracing::info!(
            session_id = %env.session_id,
            shell_id = %note.shell_session_id,
            "background escalation: rerun via prefix-grant hit"
        );
        if let Err(e) = handle
            .audit_grant_rerun("run_background_shell", &tool_input)
            .await
        {
            tracing::warn!(error = %e, "background escalation: grant-rerun audit failed");
        }
        true
    } else {
        // (b) Ask card on the ORIGINAL call's card. Deny / 120s
        // timeout / turn cancel all collapse to Denied.
        matches!(
            handle
                .ask(
                    "run_background_shell",
                    &tool_input,
                    &source.command,
                    offer.block.to_sandbox(),
                    &offer.stderr_evidence,
                )
                .await,
            EscalationOutcome::Approved
        )
    };

    if !approved {
        let mut text = plain_text(note);
        text.push_str("\n[escalation] 已向用户请求不沙盒重跑,未获批准(拒绝/超时)。");
        text.push_str(&guidance_suffix(&offer.stderr_evidence, env.perm_ctx.mode));
        return text;
    }

    // Approved → one-shot unsandboxed rerun: byte-identical command /
    // cwd / max runtime, `sandbox=None`, `origin=None` (the rerun's
    // own completion can never escalate — structurally one-shot).
    match env
        .registry
        .start(
            env.session_id,
            source.command,
            source.cwd,
            Some(source.max_runtime_ms),
            None,
            None,
        )
        .await
    {
        Ok(new_shell_id) => {
            format!(
            "[system] 后台 shell {} 因沙盒拦截({})失败,exit code {}。{}同一命令不沙盒重跑 → {}。\
             完成后将另行注入通知;可 shell_status(session_id=\"{}\") 查询。",
            note.shell_session_id,
            block_label(offer.block),
            exit_code_label(note),
            if grant_hit { "依既有「总是允许」授权," } else { "已经用户批准," },
            new_shell_id,
            new_shell_id,
        )
        }
        // The rerun could not even spawn (cwd vanished, fork fail):
        // degrade to the denial text and say so — never lie by
        // omission about an approved rerun that didn't happen.
        Err(e) => {
            let mut text = plain_text(note);
            text.push_str(&format!(
                "\n[escalation] 已批准不沙盒重跑,但重跑启动失败: {e}。"
            ));
            text
        }
    }
}

/// Mode-aware sandbox guidance appended after a `\n` (same
/// append-only discipline as the foreground tool output).
fn guidance_suffix(stderr_evidence: &str, mode: Mode) -> String {
    match crate::sandbox::failure_guidance(stderr_evidence, mode) {
        Some(g) => format!("\n{g}"),
        None => String::new(),
    }
}

fn block_label(block: EscalationBlock) -> &'static str {
    match block {
        EscalationBlock::Write => "写面外",
        EscalationBlock::Network => "断网",
    }
}

fn exit_code_label(note: &BackgroundShellNotification) -> String {
    note.exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "N/A".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! The offer is plain data here — no kernel dependency (the
    //! kernel-gated offer *generation* lives in
    //! `background_shell::in_memory::tests`). Each test wires a real
    //! registry + permission store + ask-capturing sink, mirroring
    //! `tools/tests_escalation.rs` (share the sink Arc — two `default()`
    //! instances = 0 cards + 120s timeouts).

    use std::sync::Arc;

    use sqlx::SqlitePool;
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::agent::permissions::{
        new_permission_store, resolve_ask, PermissionAskPayload, PermissionContext,
        PermissionResponse,
    };
    use crate::background_shell::{BackgroundShellOutcome, BackgroundShellRegistry};
    use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};

    #[derive(Default)]
    struct EscAskSink {
        asks: std::sync::Mutex<Vec<PermissionAskPayload>>,
    }

    impl ChatEventSink for EscAskSink {
        fn emit_chat_event(&self, _p: &ChatEventPayload) {}
        fn emit_tool_call(&self, _p: &ToolCallPayload) {}
        fn emit_tool_result(&self, _p: &ToolResultPayload) {}
        fn emit_permission_ask(&self, p: PermissionAskPayload) {
            self.asks.lock().unwrap().push(p);
        }
    }

    /// Mirror of `tools/tests_escalation.rs::escalation_pool`: migrated
    /// pool + project + session row (ask_path's audit writes need the
    /// session FK; prefix_grant_hit queries session_tool_permissions).
    async fn esc_pool(session_id: &str) -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO projects (id, name, path, created_at, updated_at) \
             VALUES ('p3d-proj', 'p', '/tmp/p3d-proj', datetime('now'), datetime('now'))",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, title, created_at, updated_at, model, project_id) \
             VALUES (?, 't', datetime('now'), datetime('now'), '', 'p3d-proj')",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// Seed a plain `echo` shell in the registry (unsandboxed — the
    /// entry exists only so `escalation_source` can find the rerun
    /// inputs; the offer itself is constructed by hand).
    async fn seed_shell(
        reg: &DefaultRegistry,
        session: &str,
        command: &str,
        origin: Option<&str>,
    ) -> String {
        reg.start(
            session,
            command.to_string(),
            std::env::temp_dir(),
            None,
            None,
            origin.map(str::to_string),
        )
        .await
        .unwrap()
    }

    fn offer(
        tool_use_id: &str,
        block: EscalationBlock,
    ) -> crate::background_shell::EscalationOffer {
        crate::background_shell::EscalationOffer {
            tool_use_id: tool_use_id.to_string(),
            block,
            stderr_evidence: "sh: cannot create /root/x.txt: Permission denied".to_string(),
        }
    }

    fn note(
        shell_id: &str,
        exit: i32,
        escalation: Option<crate::background_shell::EscalationOffer>,
    ) -> BackgroundShellNotification {
        BackgroundShellNotification {
            shell_session_id: shell_id.to_string(),
            session_id: "p3d-sess".to_string(),
            outcome: if exit == 0 {
                BackgroundShellOutcome::Completed
            } else {
                BackgroundShellOutcome::Failed
            },
            exit_code: Some(exit),
            started_at: 0,
            completed_at: 1,
            escalation,
        }
    }

    struct Harness {
        capture: Arc<EscAskSink>,
        store: crate::agent::permissions::PermissionStore,
        registry: DefaultRegistry,
        pool: SqlitePool,
    }

    async fn harness(session: &str) -> Harness {
        let pool = esc_pool(session).await;
        let capture = Arc::new(EscAskSink::default());
        Harness {
            capture,
            store: new_permission_store(),
            registry: Arc::new(
                crate::background_shell::in_memory::InMemoryBackgroundShellRegistry::new(),
            ),
            pool,
        }
    }

    fn env<'a>(h: &'a Harness, session: &'a str, mode: Mode) -> EscalationEnv<'a> {
        let root = std::env::temp_dir();
        EscalationEnv {
            registry: &h.registry,
            sink: h.capture.clone(),
            store: h.store.clone(),
            perm_ctx: PermissionContext {
                session_id: session.to_string(),
                mode,
                cwd: root.clone(),
                is_worker: false,
                worker_run_id: None,
                run_grants: None,
                worktree_path: root.clone(),
                project_main_path: root,
                turn_seq: None,
            },
            db: h.pool.clone(),
            token: CancellationToken::new(),
            session_id: session,
        }
    }

    /// Resolve the first ask after a short poll (mirrors the frontend
    /// `permission_response` IPC handler). `resolve_ask` returns false
    /// while the ask payload has been emitted but `register_ask` has
    /// not yet inserted the oneshot (ask.rs emits first, registers
    /// second) — retry until the resolve lands.
    fn spawn_resolver(h: &Harness, decision: PermissionResponse) -> tokio::task::JoinHandle<()> {
        let capture = h.capture.clone();
        let store = h.store.clone();
        tokio::spawn(async move {
            for _ in 0..1200 {
                let rid = {
                    let asks = capture.asks.lock().unwrap();
                    asks.first().map(|a| a.rid.clone())
                };
                if let Some(rid) = rid {
                    if resolve_ask(&store, &rid, decision.clone()).await {
                        return;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
    }

    // -- AC5: the plain path is byte-identical to the pre-P3d format.

    #[test]
    fn plain_text_matches_legacy_format() {
        let n = note("bsh_abc", 2, None);
        assert_eq!(
            plain_text(&n),
            "[system] 后台 shell bsh_abc 已完成,exit code 2。调 shell_status(session_id=\"bsh_abc\") 看输出。"
        );
        let n_none = BackgroundShellNotification {
            exit_code: None,
            ..note("bsh_abc", 0, None)
        };
        assert!(
            plain_text(&n_none).contains("exit code N/A"),
            "missing exit code renders N/A: {}",
            plain_text(&n_none)
        );
    }

    #[tokio::test]
    async fn plain_notifications_pass_through_untouched() {
        let h = harness("p3d-sess").await;
        let env = env(&h, "p3d-sess", Mode::Edit);
        let notes = vec![note("bsh_a", 0, None), note("bsh_b", 1, None)];
        let texts = resolve_all(&notes, &env).await;
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], plain_text(&notes[0]));
        assert_eq!(texts[1], plain_text(&notes[1]));
        assert!(
            h.capture.asks.lock().unwrap().is_empty(),
            "no offers, no cards"
        );
    }

    // -- AC1: approval → one-shot unsandboxed rerun, terminal text.

    #[tokio::test]
    async fn approved_offer_reruns_and_reports_new_shell() {
        let session = "p3d-sess";
        let h = harness(session).await;
        let old_id = seed_shell(&h.registry, session, "echo p3d-approved", Some("tu-orig-1")).await;
        let n = note(&old_id, 1, Some(offer("tu-orig-1", EscalationBlock::Write)));
        spawn_resolver(&h, PermissionResponse::AllowOnce);

        let texts = resolve_all(&[n], &env(&h, session, Mode::Edit)).await;

        // Exactly one card, attached back to the ORIGINAL call id.
        let asks = h.capture.asks.lock().unwrap();
        assert_eq!(asks.len(), 1, "one ask card");
        assert_eq!(
            asks[0].tool_use_id, "tu-orig-1",
            "card attaches to the original call"
        );
        drop(asks);
        // Terminal text names the approval + the new shell id.
        assert!(
            texts[0].contains("已经用户批准"),
            "approval framing: {}",
            texts[0]
        );
        assert!(
            texts[0].contains("不沙盒重跑 → bsh_"),
            "rerun pointer: {}",
            texts[0]
        );
        // The rerun itself ran through the registry (its completion
        // notification arrives on a later drain).
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let drained = h.registry.drain_notifications(session).await;
        assert!(
            drained.iter().any(|d| d.shell_session_id != old_id),
            "rerun produced its own notification: {drained:?}"
        );
        // Ask-side audit rows exist (zero new kinds — foreground kinds).
        let ask_rows: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ? AND kind = 'tool_permission_ask'",
        )
        .bind(session)
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert!(ask_rows.0 >= 1, "ask audit row present");
    }

    // -- AC2: deny → failure + guidance, no rerun.

    #[tokio::test]
    async fn denied_offer_keeps_failure_and_appends_guidance() {
        let session = "p3d-sess";
        let h = harness(session).await;
        let old_id = seed_shell(&h.registry, session, "echo p3d-denied", Some("tu-orig-2")).await;
        let n = note(&old_id, 1, Some(offer("tu-orig-2", EscalationBlock::Write)));
        spawn_resolver(
            &h,
            PermissionResponse::Deny {
                reason: String::new(),
            },
        );

        let texts = resolve_all(&[n], &env(&h, session, Mode::Edit)).await;

        assert_eq!(
            h.capture.asks.lock().unwrap().len(),
            1,
            "one card was raised"
        );
        assert!(
            texts[0].contains("未获批准"),
            "denial framing: {}",
            texts[0]
        );
        assert!(
            texts[0].contains("[sandbox]"),
            "mode-aware guidance appended: {}",
            texts[0]
        );
        assert!(
            !texts[0].contains("不沙盒重跑 →"),
            "no rerun happened: {}",
            texts[0]
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let drained = h.registry.drain_notifications(session).await;
        assert!(
            drained.iter().all(|d| d.shell_session_id == old_id),
            "no RERUN notification exists (seed's own completion may be here): {drained:?}"
        );
    }

    // -- AC3: grant-hit → zero cards + ToolAllowed audit; compound
    //        commands never enjoy the grant.

    async fn seed_echo_grant(pool: &SqlitePool, session: &str) {
        sqlx::query(
            "INSERT INTO session_tool_permissions (session_id, tool_name, match_kind, match_value) \
             VALUES (?, 'shell', 'prefix', 'echo')",
        )
        .bind(session)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn prefix_grant_hit_reruns_without_card() {
        let session = "p3d-sess";
        let h = harness(session).await;
        seed_echo_grant(&h.pool, session).await;
        let old_id = seed_shell(&h.registry, session, "echo p3d-grant", Some("tu-orig-3")).await;
        let n = note(&old_id, 1, Some(offer("tu-orig-3", EscalationBlock::Write)));

        let texts = resolve_all(&[n], &env(&h, session, Mode::Edit)).await;

        assert!(
            h.capture.asks.lock().unwrap().is_empty(),
            "grant hit → NO card"
        );
        assert!(
            texts[0].contains("依既有「总是允许」授权"),
            "grant framing: {}",
            texts[0]
        );
        assert!(
            texts[0].contains("不沙盒重跑 → bsh_"),
            "rerun: {}",
            texts[0]
        );
        // Grant-hit branch writes its own ToolAllowed row (foreground
        // `audit_grant_rerun` shape) so the unsandboxed rerun is
        // distinguishable in the audit trail.
        let allowed: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM session_audit_events WHERE session_id = ? AND kind = 'tool_allowed'",
        )
        .bind(session)
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(allowed.0, 1, "exactly the grant-rerun ToolAllowed row");
    }

    #[tokio::test]
    async fn compound_command_does_not_enjoy_grant() {
        let session = "p3d-sess";
        let h = harness(session).await;
        seed_echo_grant(&h.pool, session).await;
        // Structural metacharacters → the prefix would only cover the
        // first segment → the ask path runs instead.
        let old_id = seed_shell(
            &h.registry,
            session,
            "echo one && echo two",
            Some("tu-orig-4"),
        )
        .await;
        let n = note(&old_id, 1, Some(offer("tu-orig-4", EscalationBlock::Write)));
        spawn_resolver(&h, PermissionResponse::AllowOnce);

        let texts = resolve_all(&[n], &env(&h, session, Mode::Edit)).await;

        assert_eq!(h.capture.asks.lock().unwrap().len(), 1, "compound → card");
        assert!(texts[0].contains("已经用户批准"), "{}", texts[0]);
    }

    // -- AC4: Plan never escalates.

    #[tokio::test]
    async fn plan_mode_never_escalates() {
        let session = "p3d-sess";
        let h = harness(session).await;
        let old_id = seed_shell(&h.registry, session, "echo p3d-plan", Some("tu-orig-5")).await;
        let n = note(&old_id, 1, Some(offer("tu-orig-5", EscalationBlock::Write)));
        let expected_base = plain_text(&n);
        let expected_guidance =
            guidance_suffix(&n.escalation.as_ref().unwrap().stderr_evidence, Mode::Plan);

        let texts = resolve_all(&[n], &env(&h, session, Mode::Plan)).await;

        assert!(h.capture.asks.lock().unwrap().is_empty(), "no card in Plan");
        assert_eq!(texts[0], expected_base + &expected_guidance);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let drained = h.registry.drain_notifications(session).await;
        assert!(
            drained.iter().all(|d| d.shell_session_id == old_id),
            "Plan never reruns (seed's own completion may be here): {drained:?}"
        );
    }

    // -- Degrade paths.

    #[tokio::test]
    async fn unknown_entry_degrades_to_plain_text() {
        let h = harness("p3d-sess").await;
        // Offer whose shell entry was never in this registry (swept /
        // wrong id): silent plain-text degradation.
        let n = note(
            "bsh_gone",
            1,
            Some(offer("tu-gone", EscalationBlock::Network)),
        );
        let texts = resolve_all(&[n], &env(&h, "p3d-sess", Mode::Edit)).await;
        assert_eq!(texts[0], plain_text(&note("bsh_gone", 1, None)));
        assert!(h.capture.asks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rerun_spawn_failure_says_so() {
        let session = "p3d-sess";
        let h = harness(session).await;
        // Seed a grant so the approved branch runs. The entry's cwd is
        // a tempdir that is DELETED before the resolve → the rerun
        // spawn fails and the text must say so instead of lying about
        // an approved rerun that never happened. (Seeding directly
        // with a bogus cwd would fail at `start` itself — ENOENT
        // surfaces as SpawnFailed + Err — which is a different path.)
        seed_echo_grant(&h.pool, session).await;
        let cwd_dir = tempdir().unwrap();
        let old_id = h
            .registry
            .start(
                session,
                "echo p3d-spawn-fail".to_string(),
                cwd_dir.path().to_path_buf(),
                None,
                None,
                Some("tu-orig-6".to_string()),
            )
            .await
            .unwrap();
        drop(cwd_dir);
        let n = note(&old_id, 1, Some(offer("tu-orig-6", EscalationBlock::Write)));

        let texts = resolve_all(&[n], &env(&h, session, Mode::Edit)).await;

        assert!(
            h.capture.asks.lock().unwrap().is_empty(),
            "grant hit → NO card"
        );
        assert!(
            texts[0].contains("重跑启动失败"),
            "spawn failure is reported: {}",
            texts[0]
        );
    }
}
