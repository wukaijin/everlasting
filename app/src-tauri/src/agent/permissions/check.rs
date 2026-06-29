//! ⑨ 关 entry point — 5-tier `check` + Tier 4 helpers (classify,
//! extract_path_arg, grant checks, sqlite_glob_match,
//! match_value_for_allow_always). Split out of `mod.rs` on
//! 2026-06-23.

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::db::Mode;
use crate::state::ChatEventSink;

use super::ask::ask_path;
use super::audit::{record_audit, AuditKind};
use super::store::PermissionStore;
use super::types::{Decision, PermissionContext};

/// Run the ⑨ 关 5-tier check on one tool_use.
///
/// # Parameters
///
/// - `ctx` — per-call context (session_id + mode + cwd).
/// - `store` — `PermissionStore` for the Tier 4 oneshot bridge.
/// - `db` — SQLite pool (Tier 4 has_tool_permission + Tier 6
///   audit write).
/// - `app` — Tauri AppHandle (for the `permission:ask` emit
///   on Tier 4 and the `tracing` instrumentation).
/// - `tool_name` — the LLM-emitted tool name.
/// - `tool_input` — the LLM-emitted tool input JSON.
/// - `token` — the agent-loop cancellation token (for Tier 4
///   wait interruption on user Stop — does NOT trigger on deny).
///
/// # Returns
///
/// - `Decision::Allow` → execute_tool runs.
/// - `Decision::Deny { reason, critical }` → skip tool, return
///   `(reason, true)` from the agent-loop wrapper.
/// - `Decision::Ask { reason, risk }` is INTERNAL ONLY — the
///   function resolves it internally (await the oneshot or
///   timeout) and returns the final `Allow` / `Deny`.
///
/// # Re-grill 2026-06-13: ordering
///
/// Tier 1 (Hooks) → Tier 2 (Deny) → Tier 3 (Mode) → Tier 4
/// (Path / Prefix / External) → Tier 5 (Allow) → Tier 6 (Audit).
/// The old Tier 3 (always ask) is gone; the new Tier 4 only asks
/// when the path / prefix / external policy says so. Mode
/// check (Plan block writes) was Tier 4 in the old design;
/// moving it to Tier 3 eliminates the "user clicks 始终允许,
/// then gets Mode-denied" bad interaction.
pub async fn check(
    ctx: &PermissionContext,
    store: &PermissionStore,
    db: &SqlitePool,
    sink: &Arc<dyn ChatEventSink>,
    tool_name: &str,
    tool_input: &serde_json::Value,
    tool_use_id: &str,
    token: &tokio_util::sync::CancellationToken,
) -> Decision {
    // ----- Tier 1: Hooks (no-op for MVP — pre-call interface reserved) -----
    // Future PR may insert a hook override point here.

    // ----- Tier 2: Deny rules (hard kill list) -----
    // Yolo 也走这步 — 静默拒绝,不弹窗. Always silent (no Ask path).
    // This is INVARIANT: the 9 regex patterns in `dangerous.rs` are
    // not touched by the re-grill. The re-grill only restructures
    // Tier 3-5 ordering; Tier 2 is the hard wall.
    if let Some(reason) = super::dangerous::is_kill_listed(tool_name, tool_input) {
        let critical = true;
        let kind = if ctx.mode == Mode::Yolo {
            AuditKind::ToolDeniedYolo
        } else {
            AuditKind::ToolDenied
        };
        tracing::warn!(
            session_id = %ctx.session_id,
            mode = %ctx.mode.as_str(),
            tool = %tool_name,
            reason = %reason,
            "permission::check: Tier 2 deny"
        );
        let _ = record_audit( db, ctx, kind, tool_name, tool_input, Some(&reason)).await;
        return Decision::Deny { reason, critical };
    }

    // ----- Tier 3: Mode check (Plan blocks file writes) -----
    // ⑨ 关第 3 道 + ⑧a 三重防御的最后一层. Plan 模式拦截
    // write_file/edit_file (纯写工具, 无歧义, 直接 text error
    // 不弹窗 — 避免 "用户点始终允许 → 仍被 Mode 拒" 的鬼畜交互).
    //
    // **shell 不再在此层拦截 (三档分类 2026-06-14)**: shell 是
    // 异构工具 (git diff 读 / git push 写), 一刀切会把只读命令
    // 也禁掉且无放行口子. shell 的 mode 感知下沉到 Tier 4 的
    // Shell 分支: ReadOnly→Allow, SideEffect/Ask→弹窗 (Plan 下
    // 用户可当场放行). 见 shell_trust.rs 三档分类.
    if matches!(ctx.mode, Mode::Plan) {
        if matches!(tool_name, "write_file" | "edit_file") {
            tracing::info!(
                session_id = %ctx.session_id,
                mode = %ctx.mode.as_str(),
                tool = %tool_name,
                "permission::check: Tier 3 mode block (write tools in read-only mode)"
            );
            let reason = format!(
                "I cannot execute {} in {} mode (read-only session)",
                tool_name,
                ctx.mode.as_str()
            );
            let _ = record_audit(
                db,
                ctx,
                AuditKind::ToolDenied,
                tool_name,
                tool_input,
                Some(&format!("tool blocked in {} mode", ctx.mode.as_str())),
            )
            .await;
            return Decision::Deny {
                reason,
                critical: false,
            };
        }
    }

    // ----- Tier 4: Path / Prefix / External policy -----
    // Re-grill 2026-06-13: was the "always ask" tier. Now split
    // by tool type and bypassed by Yolo.
    //
    // Yolo bypasses the entire tier (Q4: "Yolo bypass 所有 modal").
    // Tier 2 still catches the hard-kill patterns. Tier 3 still
    // catches Plan write tools (we never reach Tier 4 in that case).
    if ctx.mode == Mode::Yolo {
        tracing::info!(
            session_id = %ctx.session_id,
            tool = %tool_name,
            "permission::check: Tier 4 bypassed (Yolo mode)"
        );
        // Tier 6 audit for the Allow path (Tier 2 / Tier 3 deny paths
        // already wrote their own audit rows above).
        let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
        return Decision::Allow;
    }

    // Dispatch by tool type.
    match classify_tool(tool_name) {
        ToolKind::Path => {
            // Path tools: extract the `path` argument (and
            // optionally `cwd` / `working_directory` override),
            // check `is_within_root`, then consult
            // `session_tool_permissions` for a path-glob grant,
            // and emit `permission:ask` if needed.
            let path_str = extract_path_arg(tool_name, tool_input);
            match path_str {
                Some(p) => {
                    // Normalize: the LLM may send relative paths.
                    // For the permission layer, we treat the path as
                    // relative to ctx.cwd unless it's already absolute.
                    let abs_path = if std::path::Path::new(&p).is_absolute() {
                        std::path::PathBuf::from(&p)
                    } else {
                        ctx.cwd.join(&p)
                    };
                    let inside = crate::projects::boundary::is_within_root(&ctx.cwd, &abs_path);
                    // Tier 4.1: check session_tool_permissions
                    // match_kind='path' for a grant. If hit, Allow.
                    if let Ok(true) = check_path_grant(db, &ctx.session_id, tool_name, &abs_path).await {
                        tracing::info!(
                            session_id = %ctx.session_id,
                            tool = %tool_name,
                            path = %abs_path.display(),
                            "permission::check: Tier 4 path grant hit"
                        );
                        let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                        return Decision::Allow;
                    }
                    // 2026-06-26 (task 06-26-subagent-per-run-grant):
                    // worker path — consult the per-run in-memory
                    // grant cache before falling through to ask_path.
                    // Same semantics as the DB grant check above
                    // (sqlite_glob_match on each path-kind row), but
                    // scoped to the worker's run. Parent path has
                    // `run_grants = None` and skips this block.
                    if let Some(cache) = &ctx.run_grants {
                        let candidate = abs_path.to_string_lossy().to_string();
                        if cache.has_run_grant(tool_name, "path", &candidate) {
                            tracing::info!(
                                session_id = %ctx.session_id,
                                tool = %tool_name,
                                path = %abs_path.display(),
                                "permission::check: Tier 4 worker run-grant hit (path)"
                            );
                            let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                            return Decision::Allow;
                        }
                    }
                    if inside {
                        // Inside the project, no grant → silent Allow
                        // (the user trusts the agent to work in the repo).
                        tracing::info!(
                            session_id = %ctx.session_id,
                            tool = %tool_name,
                            path = %abs_path.display(),
                            "permission::check: Tier 4 path inside root, silent Allow"
                        );
                        let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                        return Decision::Allow;
                    }
                    // Outside the project, no grant → modal.
                    let path_owned = abs_path.to_string_lossy().to_string();
                    return ask_path(
                        sink, db, store, ctx,
                        tool_name, tool_input,
                        &path_owned, Some(&path_owned), tool_use_id, token,
                    ).await;
                }
                None => {
                    // Path tool without a `path` arg is a malformed
                    // tool_use — let the tool layer surface the
                    // error (it will produce is_error: true). For
                    // the permission layer, default to Allow
                    // (the tool layer's schema validation is the
                    // real gate).
                    let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                    return Decision::Allow;
                }
            }
        }
        ToolKind::Shell => {
            let cmd = tool_input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            // (a) "始终允许" prefix-grant hit → silent Allow. Closes the
            // old gap: match_value_for_allow_always wrote match_kind='prefix'
            // rows for shell but Tier 4 never queried them — a user's
            // AllowAlways on a shell command now sticks across turns.
            if let Ok(true) = check_prefix_grant(db, &ctx.session_id, &super::shell_trust::first_token_for_allow_always(cmd)).await {
                let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                return Decision::Allow;
            }
            // 2026-06-26 (task 06-26-subagent-per-run-grant): worker
            // path — consult the per-run in-memory grant cache before
            // running classify_prefix. Same prefix semantics as
            // `check_prefix_grant` above (exact-eq on shell first
            // token). Parent path has `run_grants = None` and skips.
            if let Some(cache) = &ctx.run_grants {
                let first_token = super::shell_trust::first_token_for_allow_always(cmd);
                if cache.has_run_grant(tool_name, "prefix", &first_token) {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        tool = %tool_name,
                        first_token = %first_token,
                        "permission::check: Tier 4 worker run-grant hit (prefix)"
                    );
                    let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                    return Decision::Allow;
                }
            }
            // (b) Three-tier classification + per-Mode mapping. shell is
            // heterogenous (git diff vs git push), so the Mode decision
            // lives HERE in Tier 4, not in Tier 3.
            //   Plan: ReadOnly→silent Allow; SideEffect/Ask→modal.
            //   Edit: ReadOnly/SideEffect→silent Allow; Ask→modal.
            //   Yolo never reaches here (Tier 4 bypassed at the top).
            match super::shell_trust::classify_prefix(cmd) {
                super::shell_trust::ShellTrust::ReadOnly => {
                    // Pure read — allow silently in every mode (Plan included).
                    let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                    return Decision::Allow;
                }
                super::shell_trust::ShellTrust::SideEffect => {
                    if ctx.mode == Mode::Plan {
                        // Plan is read-only; surface the side effect to the
                        // user instead of silently allowing it.
                        return ask_path(sink, db, store, ctx, tool_name, tool_input, cmd, None, tool_use_id, token).await;
                    }
                    // Edit: silent Allow (old whitelist behaviour).
                    let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                    return Decision::Allow;
                }
                super::shell_trust::ShellTrust::Ask => {
                    // Asklist / unknown / structurally complex — modal in
                    // every interactive mode. Shell commands are NOT path
                    // tools: the modal renders the command inline via
                    // `toolInput` (no "path scope" row). `path_for_modal =
                    // None` keeps the `path` field OFF the wire so the
                    // frontend's `v-if="hasPath"` does not render a
                    // misleading scope row for a shell ask.
                    return ask_path(sink, db, store, ctx, tool_name, tool_input, cmd, None, tool_use_id, token).await;
                }
            }
        }
        ToolKind::WebFetch => {
            // Web fetch is always external — check
            // session_tool_permissions match_kind='tool' for
            // `web_fetch`. If hit, Allow; else modal.
            if let Ok(true) = check_tool_grant(db, &ctx.session_id, "web_fetch").await {
                let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                return Decision::Allow;
            }
            // 2026-06-26 (task 06-26-subagent-per-run-grant): worker
            // path — consult the per-run in-memory grant cache before
            // falling through to ask_path. Same tool-level semantics
            // as `check_tool_grant` above (tool_name equality;
            // candidate is unused for tool-kind grants). Parent path
            // has `run_grants = None` and skips.
            if let Some(cache) = &ctx.run_grants {
                if cache.has_run_grant("web_fetch", "tool", "") {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        "permission::check: Tier 4 worker run-grant hit (tool: web_fetch)"
                    );
                    let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
                    return Decision::Allow;
                }
            }
            return ask_path(
                sink, db, store, ctx,
                tool_name, tool_input,
                tool_input.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                // Web fetch is always external — the modal renders
                // the URL inline via `toolInput` (no "path scope"
                // row). `path_for_modal = None` keeps the `path` field
                // OFF the wire so the frontend's `v-if="hasPath"` does
                // not render a misleading scope row for a web_fetch
                // ask (it would otherwise show "仓库外" against a URL,
                // which is wrong — the URL is not a filesystem path).
                None, tool_use_id, token,
            ).await;
        }
        ToolKind::GitMutation => {
            // merge_worker / discard_worker — tool-level grant + ask
            // (same shape as WebFetch). The `run_id` is a DB key, not
            // a path, so `path_for_modal = None` keeps the path-scope
            // row off the wire. `check_tool_grant` takes the ACTUAL
            // `tool_name` (not a literal) so a grant on discard_worker
            // is not confused with merge_worker.
            if let Ok(true) = check_tool_grant(db, &ctx.session_id, tool_name).await {
                let _ = record_audit(
                    db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None,
                )
                .await;
                return Decision::Allow;
            }
            if let Some(cache) = &ctx.run_grants {
                if cache.has_run_grant(tool_name, "tool", "") {
                    tracing::info!(
                        session_id = %ctx.session_id,
                        tool = %tool_name,
                        "permission::check: Tier 4 worker run-grant hit (tool: git-mutation)"
                    );
                    let _ = record_audit(
                        db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None,
                    )
                    .await;
                    return Decision::Allow;
                }
            }
            return ask_path(
                sink, db, store, ctx,
                tool_name, tool_input,
                tool_input.get("run_id").and_then(|v| v.as_str()).unwrap_or(""),
                None, tool_use_id, token,
            )
            .await;
        }
        ToolKind::Other => {
            // Unknown / future tool — default Allow (Tier 5).
            // The tool layer's own boundary checks (e.g.
            // ReadGuard for edit_file) are the real gate.
            let _ = record_audit( db, ctx, AuditKind::ToolAllowed, tool_name, tool_input, None).await;
            return Decision::Allow;
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 4 helpers
// ---------------------------------------------------------------------------

/// Tool classification for Tier 4 dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolKind {
    /// Path-based tools (read_file / write_file / edit_file /
    /// list_dir / grep / glob). All extract a `path` argument and
    /// are subject to the path-glob check.
    Path,
    /// Shell tool. Classified by `shell_trust::classify_prefix`.
    Shell,
    /// Web fetch. Always external; uses `tool` match_kind grant.
    WebFetch,
    /// Git-mutating tools (merge_worker / discard_worker). Mirrors
    /// WebFetch: tool-level grant + ask; the `run_id` is a DB key,
    /// not a filesystem path (so the modal renders no path-scope row).
    GitMutation,
    /// Unknown / future tools. Default Allow.
    Other,
}

pub(crate) fn classify_tool(tool_name: &str) -> ToolKind {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "list_dir" | "grep" | "glob" => {
            ToolKind::Path
        }
        // L1a: `run_background_shell` runs the SAME `sh -c <command>` shape
        // as `shell`, so the Tier 4 shell branch (kill-list + 3-tier
        // classify_prefix + prefix grants) applies uniformly. Routing it to
        // `ToolKind::Shell` instead of `Other` is what makes "始终允许"
        // grants on `cargo` work for both sync and async forms.
        "shell" | "run_background_shell" => ToolKind::Shell,
        "web_fetch" => ToolKind::WebFetch,
        // L3b PR3: merge_worker / discard_worker mutate the parent
        // session's git branch. Routed to `GitMutation` (NOT `Shell`)
        // because they take a `run_id`, not a `command` — Shell's
        // prefix-grant match (first-token of `command`) would collide
        // on the empty token and let an "always allow" bleed across
        // empty-token shell commands. GitMutation uses tool-level
        // grants, mirroring WebFetch.
        "merge_worker" | "discard_worker" => ToolKind::GitMutation,
        _ => ToolKind::Other,
    }
}

/// Extract the `path` argument from a path-tool's input. Most
/// tools use `path`; a future tool may use `cwd` or
/// `working_directory`. Returns the **string** (not PathBuf)
/// because the caller may need to send it on the wire as part
/// of the PermissionAskPayload.
pub(crate) fn extract_path_arg(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    // Read tools / write_file / edit_file / list_dir / grep / glob
    // all use `path` (the schema is uniform across them — see
    // `tools/*.rs::definition()`).
    let _ = tool_name; // silence unused warning; reserved for future
    let p = input
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("cwd").and_then(|v| v.as_str()))
        .or_else(|| input.get("working_directory").and_then(|v| v.as_str()))?;
    Some(p.to_string())
}

/// Check `session_tool_permissions` for a path-glob grant on
/// the given path. Returns `Ok(true)` if any row's
/// `match_value` (a sqlite GLOB) matches the path. Used by
/// Tier 4 to short-circuit the modal for "始终允许 path".
async fn check_path_grant(
    db: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    path: &std::path::Path,
) -> Result<bool, sqlx::Error> {
    // Pull all `path` match_kind rows for this session+tool.
    // The path-glob uses sqlite GLOB syntax:
    // - `*` matches any sequence of characters NOT crossing `/`
    // - `?` matches exactly one character
    // - `**` is NOT supported (sqlite GLOB is single-asterisk
    //   only). The re-grill PRD explicitly accepts this
    //   limitation (§"Out of Scope").
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT match_value FROM session_tool_permissions
        WHERE session_id = ? AND tool_name = ? AND match_kind = 'path'
        "#,
    )
    .bind(session_id)
    .bind(tool_name)
    .fetch_all(db)
    .await?;
    let path_str = path.to_string_lossy();
    for (glob,) in rows {
        // sqlite GLOB matcher (inlined). We use a simple
        // recursive matcher that respects the GLOB rule that
        // `*` does NOT cross `/`. The crate `glob` would also
        // work but the dependency was deemed overkill for
        // one-line matching.
        if sqlite_glob_match(&glob, &path_str) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Match a path against a sqlite-style GLOB pattern. Supports
/// `*` (zero or more non-`/` characters) and `?` (exactly one
/// non-`/` character). All other characters match literally.
/// Backslash-escape is NOT supported (sqlite GLOB doesn't
/// support it either). Case-sensitive (sqlite GLOB is
/// case-sensitive; the column was stored verbatim from
/// `Path::display()`).
pub(crate) fn sqlite_glob_match(pattern: &str, text: &str) -> bool {
    // Recursive matcher. We track `pi` (pattern index) and
    // `ti` (text index) and use a small stack of
    // backtrack positions for `*`.
    let pbytes = pattern.as_bytes();
    let tbytes = text.as_bytes();
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ti: usize = 0;
    while ti < tbytes.len() {
        if pi < pbytes.len() {
            match pbytes[pi] {
                b'*' => {
                    // Record backtrack position and try matching
                    // zero chars first.
                    star_pi = Some(pi);
                    star_ti = ti;
                    pi += 1;
                    continue;
                }
                b'?' => {
                    // Single char; sqlite GLOB `?` does NOT cross `/`.
                    // We just hit a `/` — the backtrack-on-`*` block
                    // that the original 2026-06-14 code carried here
                    // was unreachable (the inner `if tbytes[ti] == b'/'`
                    // check was guaranteed-true by the outer one above,
                    // so the `if let Some(sp) = star_pi` branch never
                    // fired — see DEBT.md RULE-B-003, closed 2026-06-24).
                    // Fail directly.
                    if tbytes[ti] == b'/' {
                        return false;
                    }
                    pi += 1;
                    ti += 1;
                    continue;
                }
                c if c == tbytes[ti] => {
                    pi += 1;
                    ti += 1;
                    continue;
                }
                _ => {
                    // Literal mismatch — backtrack on `*` if any.
                    if let Some(sp) = star_pi {
                        pi = sp;
                        ti = star_ti + 1;
                        star_ti += 1;
                        // But `*` cannot cross `/`; if we just stepped
                        // past a `/`, fail.
                        if ti > 0 && tbytes[ti - 1] == b'/' {
                            return false;
                        }
                        continue;
                    }
                    return false;
                }
            }
        } else {
            // Pattern exhausted — backtrack on `*` if any.
            if let Some(sp) = star_pi {
                pi = sp;
                ti = star_ti + 1;
                star_ti += 1;
                if ti > 0 && tbytes[ti - 1] == b'/' {
                    return false;
                }
                continue;
            }
            return false;
        }
    }
    // Pattern may have trailing `*`s — consume them.
    while pi < pbytes.len() && pbytes[pi] == b'*' {
        pi += 1;
    }
    pi == pbytes.len()
}

/// Check `session_tool_permissions` for an exact-tool grant.
/// Returns `Ok(true)` if any row has
/// `match_kind = 'tool'` + `tool_name = ?` + `match_value IS NULL`.
async fn check_tool_grant(
    db: &SqlitePool,
    session_id: &str,
    tool_name: &str,
) -> Result<bool, sqlx::Error> {
    crate::db::has_tool_permission(db, session_id, tool_name).await
}

/// Check `session_tool_permissions` for a shell-prefix grant.
/// Returns `Ok(true)` if any row has `tool_name='shell'`,
/// `match_kind='prefix'`, and `match_value = first_token` (exact
/// match — prefix grants store the bare command name like
/// `cargo`, not a glob).
///
/// Closes the old gap where `match_value_for_allow_always` wrote
/// `match_kind='prefix'` rows for shell but Tier 4 never queried
/// them: a user's "始终允许" on a shell command now sticks.
async fn check_prefix_grant(
    db: &SqlitePool,
    session_id: &str,
    first_token: &str,
) -> Result<bool, sqlx::Error> {
    if first_token.is_empty() {
        return Ok(false);
    }
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM session_tool_permissions
        WHERE session_id = ?
          AND tool_name = 'shell'
          AND match_kind = 'prefix'
          AND match_value = ?
        LIMIT 1
        "#,
    )
    .bind(session_id)
    .bind(first_token)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Compute the `(match_kind, match_value)` pair to write to
/// `session_tool_permissions` on a user's "始终允许" click.
/// Re-grill Q6: wire the 3 match_kind variants. Q8: path
/// uses parent-directory + `*` glob (sqlite GLOB `*` does
/// not cross `/`).
///
/// **Path tool**: parent directory + `/*` (Q8). E.g.
/// `/Users/me/Documents/notes.md` → `match_value = '/Users/me/Documents/*'`.
///
/// **Shell**: first whitespace token (Q7). E.g.
/// `cargo test` → `match_value = 'cargo'`.
///
/// **Web fetch**: tool match (Q6 "web_fetch 始终允许 = 整 tool");
/// per-domain persistence is OOS for the re-grill (deferred
/// to PR3+).
pub(crate) fn match_value_for_allow_always(
    tool_name: &str,
    _tool_input: &serde_json::Value,
    path_or_cmd: &str,
) -> (&'static str, Option<String>) {
    match classify_tool(tool_name) {
        ToolKind::Path => {
            // parent + /*  glob
            let p = std::path::Path::new(path_or_cmd);
            let glob = match p.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    format!("{}/*", parent.display())
                }
                _ => format!("{}/*", path_or_cmd),
            };
            ("path", Some(glob))
        }
        ToolKind::Shell => {
            // first token
            let prefix = super::shell_trust::first_token_for_allow_always(path_or_cmd);
            ("prefix", Some(prefix))
        }
        ToolKind::WebFetch => {
            // tool-level grant (per-domain deferred to PR3+)
            ("tool", None)
        }
        ToolKind::GitMutation => {
            // tool-level grant (merge_worker / discard_worker).
            ("tool", None)
        }
        ToolKind::Other => {
            // Future tool — fall back to `tool` match (no glob).
            ("tool", None)
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 1 Hooks: pre-tool pitfall recall (P3, 2026-06-29)
// ---------------------------------------------------------------------------

/// Pre-tool pitfall recall — the Tier 1 Hooks side of the ⑨ layer.
///
/// **Scope**: hooks the `permissions/check.rs` Tier 1 site (currently
/// no-op per the 5-tier design) with a `find_pitfalls_by_trigger`
/// probe. When `active` pitfalls match the current `(tool_name,
/// tool_input)`, the function builds a footnote string that the
/// chat loop prepends to the `tool_result.content`. The tool
/// execution itself is NEVER blocked (this is the **active 注脚**
/// tier per spike-007 §4 + P3 PRD; the verified soft-intercept tier
/// is OUT OF SCOPE here — `P5`).
///
/// **Why separate from `check()`**: `check()` returns a `Decision`
/// (Allow/Deny/resolved-Ask); injecting a "soft footnote" would
/// pollute that contract (Deny is silent, Ask goes through
/// oneshot, neither carries text). The chat loop already has a
/// clear "after check returns Allow, before execute_tool" seam,
/// so the recall runs there as its own pure-data step. This keeps
/// `check()` 5-tier-pure and the loop structure untouched (PRD
/// hard rule).
///
/// **Behavior contract** (locked by P3 acceptance criteria):
/// 1. Resolves `(command_pattern, path)` from `tool_input` based
///    on tool kind (Path → `path`/`cwd`/`working_directory`;
///    Shell → `command`; WebFetch → `url`; other → `(None, None)`).
/// 2. Calls `db::memories::find_pitfalls_by_trigger` — exact-match
///    `tool_name`, substring `command_pattern` (when supplied by
///    the caller), `path_globs` glob match (when supplied).
/// 3. Filters to `status == 'active'` rows only (verified soft-
///    intercept is P5 scope; see spike-007 §4 tier table).
/// 4. Builds a multi-line footnote: one bullet per matching pitfall
///    with title + content. Token budget is loose (P3 doesn't cap;
///    P5 will re-derive alongside the verified soft-intercept).
/// 5. Fires `db::memories::bump_hit_count` per hit, fire-and-forget
///    on a `tokio::spawn` so the recall step stays sync-fast
///    (matches the audit-write pattern: best-effort metadata, never
///    blocks the hot path).
/// 6. Any DB error → `tracing::warn!` + return `Ok(None)` (recall
///    failure MUST NOT block tool execution; PRD acceptance
///    criterion).
///
/// **Returns**: `Ok(Some(footnote))` on active hit, `Ok(None)` on
/// miss / DB-error / out-of-scope. The chat loop prepends the
/// footnote to `content` before envelope wrapping. The footnote is
/// deliberately plain text (no `cache_control`, no Anthropic
/// metadata) — it travels inside the tool_result `content` string
/// alongside the tool's normal output.
///
/// **Wire / cross-layer**: this function is called from
/// `agent/chat_loop.rs` between `permissions::check` returning
/// `Allow` and `execute_tool`. It does NOT mutate the agent loop
/// state machine, does NOT touch cancel/audit maps, does NOT alter
/// the persisted message history (the tool_result already travels
/// through the normal path).
pub async fn recall_pitfall_footnote(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Result<Option<String>, sqlx::Error> {
    // Step 1: extract the relevant probe string from tool_input.
    let (command_pattern, path) = extract_probe_args(tool_name, tool_input);

    // Step 2: probe find_pitfalls_by_trigger.
    let rows = crate::db::memories::find_pitfalls_by_trigger(
        db,
        tool_name,
        command_pattern.as_deref(),
        path.as_deref(),
    )
    .await?;

    // Step 3: filter to active rows only (verified → P5).
    let active_rows: Vec<_> = rows
        .into_iter()
        .filter(|r| r.status == "active")
        .collect();

    if active_rows.is_empty() {
        return Ok(None);
    }

    // Step 5: bump_hit_count fire-and-forget per hit (best-effort,
    // never blocks the recall step).
    for row in &active_rows {
        let pool = db.clone();
        let mid = row.memory_id.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::db::memories::bump_hit_count(&pool, &mid).await {
                tracing::warn!(
                    memory_id = %mid,
                    error = %e,
                    "recall_pitfall_footnote: bump_hit_count failed (non-fatal)"
                );
            }
        });
    }

    // Step 4: build the multi-line footnote. Imperative, pitfall-
    // style phrasing per spike-007 §4 "active 注脚" tier (the
    // soft hint that doesn't interrupt execution).
    let mut out = String::from("⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n");
    for row in &active_rows {
        // Title + content, one pitfall per line. Use the bullet
        // marker `•` so the LLM can pick the relevant one out of
        // multiple hits without losing alignment.
        out.push_str(&format!("• [{}] {}\n", row.title, row.content));
    }
    Ok(Some(out))
}

/// Resolve the `(command_pattern, path)` probe arguments from a
/// tool's input JSON, dispatching by tool kind. Returns
/// `(None, None)` for tool kinds that don't carry a probe-able
/// argument (e.g. dispatch_subagent — irrelevant for pitfall
/// recall).
///
/// **Why a per-tool dispatch**: pitfall rows store their
/// `command_pattern` (substring match) and `path_globs` (glob
/// match) as separate fields. The probe must extract the right
/// fields per tool so the underlying
/// `find_pitfalls_by_trigger` SQL filter does the right thing:
/// - Shell: `command` → substring probe.
/// - Path tools: `path` (with `cwd`/`working_directory` fallback) →
///   glob probe via `path_globs`.
/// - WebFetch: `url` → substring probe (matches pitfalls that
///   trigger on a domain or URL pattern).
/// - Other / unknown: no probe (no recall possible).
///
/// **Mirrors `extract_path_arg`'s precedence** for the path key
/// (`path` > `cwd` > `working_directory`), so the recall probe
/// uses the same canonical path the Tier 4 path-glob check
/// would resolve.
fn extract_probe_args(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    match classify_tool(tool_name) {
        ToolKind::Path => {
            // path-globs probe.
            (None, extract_path_arg(tool_name, tool_input))
        }
        ToolKind::Shell => {
            // substring probe on the full command. The underlying
            // `find_pitfalls_by_trigger` will further check
            // `command_pattern` substring containment inside
            // this value (the writer sets `command_pattern` to a
            // distinctive substring like "cargo test").
            let cmd = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (cmd, None)
        }
        ToolKind::WebFetch => {
            // URL substring probe. Most pitfalls store a host
            // substring (e.g. "api.example.com") that the full
            // URL contains.
            let url = tool_input
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (url, None)
        }
        ToolKind::GitMutation | ToolKind::Other => {
            // No probe-able field — recall returns empty.
            (None, None)
        }
    }
}
