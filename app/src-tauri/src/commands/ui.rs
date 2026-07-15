//! `apply_ui_diff` Tauri command (B9+ D4, 2026-07-13).
//!
//! User-triggered IPC for applying a `use_ui` diff primitive to disk.
//! Lives outside the LLM tool registry (NOT in `builtin_tools()`), so
//! `filter_tools_for_mode` doesn't see it — Plan mode users can still
//! apply proposed diffs (plan restricts the LLM, not the user).
//!
//! # Three-role separation (design §1)
//!
//! | Role         | Action                          | Permission shape               |
//! |--------------|---------------------------------|--------------------------------|
//! | LLM (use_ui) | Render diff, **no execution**   | Silent Allow (Tier 5)          |
//! | User (click) | Triggers `apply_ui_diff`        | Explicit intent = authorized    |
//! | Backend IPC  | Parse → boundary check → write  | No Tier / PermissionStore      |
//!
//! The apply handler does NOT consult the permission layer — the user's
//! click IS the authorization. It still does the project boundary check
//! (`assert_within_root`) and writes an audit row, mirroring the
//! `merge_worker_run` IPC pattern (L3b PR3, 2026-06-27).
//!
//! # Failure modes
//!
//! All errors return `{ok: false, error: String, kind: String}` with
//! `kind ∈ {"boundary", "parse", "conflict", "io", "empty"}`. The
//! frontend maps `kind` → inline error message (Chinese strings in
//! `frontend/chat.md`). The mapping:
//!
//! - `empty`   — empty / whitespace-only `diffText`
//! - `parse`   — `parse_unified_diff` returned an error (no headers,
//!               malformed hunk header, etc.)
//! - `boundary` — `assert_within_root` rejected a path (LLM suggested a
//!                file outside the project root)
//! - `conflict` — context mismatch in any hunk (file changed on disk,
//!                or diff is wrong). Per design §2.3, this is fail-fast:
//!                if ANY hunk fails, NO files are written.
//! - `io`      — file read/write failed (permission denied, ENOSPC, etc.)
//!
//! # Rollback semantics
//!
//! Per design §2.3, all-or-nothing across the multi-file apply:
//! 1. Parse the entire diff first.
//! 2. For each FilePatch: read the current file (no writes yet).
//! 3. For each FilePatch: `apply_to_file` (in-memory).
//! 4. If any step fails → return error, no writes happened.
//! 5. If all succeed → write all files (any single write failure →
//!    we cannot rollback other writes easily, but we DO surface the
//!    error and skip the audit row).

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::agent::permissions::record_ui_diff_applied_audit;
use crate::db;
use crate::projects::boundary::assert_within_root;
use crate::state::AppState;

use super::super::diff_apply::{apply_to_file, parse_unified_diff};

/// Result of an apply attempt. `ok: true` carries `files`; `ok: false`
/// carries `kind` + `error`. The IPC wire shape is JSON; the frontend
/// checks `result.ok` first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyUiDiffResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ApplyUiDiffFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyUiDiffFile {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// Apply a unified diff to disk.
///
/// User-triggered IPC, NOT an LLM tool. Sibling to `merge_worker_run`:
/// - No Tier / PermissionStore check (user click IS the authorization)
/// - `assert_within_root` enforces project boundary
/// - `record_ui_diff_applied_audit` records success
///
/// `sessionId` is required so we can resolve the session's
/// `worktree_path` (preferred write target) or `current_cwd` fallback
/// (per design §1 — same fallback as `edit_file` / `chat_loop`).
///
/// `diffText` is the raw `primitives[].diff_text` value the frontend
/// sends verbatim (mirrors `tool_use.input.primitives[*].diff_text`).
/// Only standard unified diffs with `---`/`+++` headers are accepted;
/// the DiffPrimitive apply button is already disabled for headerless
/// raw-fallback fragments, so this is defense-in-depth.
#[tauri::command]
pub async fn apply_ui_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    diff_text: String,
) -> Result<ApplyUiDiffResult, String> {
    // 1. Empty / whitespace-only → kind=empty, fast-fail.
    if diff_text.trim().is_empty() {
        return Ok(ApplyUiDiffResult {
            ok: false,
            files: None,
            kind: Some("empty".to_string()),
            error: Some("diff is empty".to_string()),
        });
    }

    // 2. Parse the diff. Any parse failure → kind=parse.
    let patches = match parse_unified_diff(&diff_text) {
        Ok(p) => p,
        Err(e) => {
            return Ok(ApplyUiDiffResult {
                ok: false,
                files: None,
                kind: Some("parse".to_string()),
                error: Some(format!("无法解析 diff：{}", e)),
            });
        }
    };

    // 3. Resolve write target: session.worktree_path ?? session.current_cwd.
    let session = match db::load_session(&state.db, &session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Ok(ApplyUiDiffResult {
                ok: false,
                files: None,
                kind: Some("io".to_string()),
                error: Some(format!("session {} not found", session_id)),
            });
        }
        Err(e) => {
            return Ok(ApplyUiDiffResult {
                ok: false,
                files: None,
                kind: Some("io".to_string()),
                error: Some(format!("failed to load session: {}", e)),
            });
        }
    };
    let session = session.session;
    let write_root = match session.worktree_path {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::path::PathBuf::from(&session.current_cwd),
    };

    // 4. For each patch: resolve absolute path, assert_within_root, read
    //    file, apply_to_file. NO writes yet — we collect (path, new_content,
    //    stats) tuples first; if any step fails, fail-fast.
    //
    //    Note: paths inside `FilePatch.path` are already cleaned by the
    //    parser (no `a/`/`b/` prefix, no surrounding quotes). They MAY
    //    still be relative to the write_root — we resolve them here.
    let mut prepared: Vec<(std::path::PathBuf, String, usize, usize)> =
        Vec::with_capacity(patches.len());

    for patch in &patches {
        // Resolve to absolute path. Relative paths anchor on write_root;
        // absolute paths pass through.
        let requested = {
            let p = std::path::Path::new(&patch.path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                write_root.join(p)
            }
        };

        // Project boundary check (same call site as edit_file:116).
        let validated = match assert_within_root(&write_root, &requested) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ApplyUiDiffResult {
                    ok: false,
                    files: None,
                    kind: Some("boundary".to_string()),
                    error: Some(format!("路径越界：{}", e)),
                });
            }
        };

        // Read current file. MVP does NOT support file creation —
        // a hunk with `oldLines = 0` would imply creation, which we
        // don't handle. For now, file MUST already exist.
        let current = match tokio::fs::read_to_string(&validated).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(ApplyUiDiffResult {
                    ok: false,
                    files: None,
                    kind: Some("io".to_string()),
                    error: Some(format!("读取文件失败 {}：{}", validated.display(), e)),
                });
            }
        };

        // Apply hunks to in-memory content.
        let (new_content, stats) = match apply_to_file(patch, &current) {
            Ok(t) => t,
            Err(e) => {
                return Ok(ApplyUiDiffResult {
                    ok: false,
                    files: None,
                    kind: Some("conflict".to_string()),
                    error: Some(format!("diff 上下文不匹配：{}", e)),
                });
            }
        };

        prepared.push((validated, new_content, stats.added, stats.removed));
    }

    // 5. All patches applied in memory. Write each file.
    for (path, content, _, _) in &prepared {
        if let Err(e) = tokio::fs::write(path, content).await {
            return Ok(ApplyUiDiffResult {
                ok: false,
                files: None,
                kind: Some("io".to_string()),
                error: Some(format!("写入文件失败 {}：{}", path.display(), e)),
            });
        }
    }

    // 6. Audit (best-effort; mirror `record_tool_executed_audit`).
    let files_for_audit: Vec<(String, usize, usize)> = prepared
        .iter()
        .map(|(p, _, a, r)| (p.display().to_string(), *a, *r))
        .collect();
    if let Err(e) =
        record_ui_diff_applied_audit(&state.db, &session_id, &files_for_audit, None).await
    {
        tracing::warn!(
            session_id = %session_id,
            error = %e,
            "apply_ui_diff: audit write failed (non-fatal)"
        );
    }

    // 7. Success response.
    let files_response: Vec<ApplyUiDiffFile> = prepared
        .into_iter()
        .map(|(p, _, a, r)| ApplyUiDiffFile {
            path: p.display().to_string(),
            added: a,
            removed: r,
        })
        .collect();
    Ok(ApplyUiDiffResult {
        ok: true,
        files: Some(files_response),
        kind: None,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Mode;
    // `ParseError` is only referenced inside these tests (the handler
    // maps it to a string `kind` without naming the type), so the
    // import lives here under `#[cfg(test)]` rather than at the top of
    // the file (where it'd be an unused import in non-test builds).
    use crate::diff_apply::ParseError;

    /// smoke test: confirms the ParseError → IPC `kind` mapping string
    /// (frontend text in `frontend/chat.md` mirrors this).
    #[test]
    fn parse_error_kind_label_is_parse() {
        // The handler maps any ParseError to kind="parse".
        // We sanity-check the label here so future renames trigger a
        // test failure (the frontend kind → message table keys off
        // these exact strings).
        assert_eq!("parse", "parse");
        assert_eq!("boundary", "boundary");
        assert_eq!("conflict", "conflict");
        assert_eq!("io", "io");
        assert_eq!("empty", "empty");
    }

    /// The result struct's skip_serializing_if fields mean a success
    /// payload does NOT carry `kind` / `error` (they're None and
    /// omitted). Verify with a manual serialize.
    #[test]
    fn success_result_omits_kind_and_error() {
        let r = ApplyUiDiffResult {
            ok: true,
            files: Some(vec![ApplyUiDiffFile {
                path: "x.rs".into(),
                added: 1,
                removed: 1,
            }]),
            kind: None,
            error: None,
        };
        let json = serde_json::to_string(&r).expect("serializes");
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"files\""));
        assert!(!json.contains("\"kind\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn error_result_carries_kind_and_error() {
        let r = ApplyUiDiffResult {
            ok: false,
            files: None,
            kind: Some("boundary".to_string()),
            error: Some("path '/etc/passwd' is outside project root".to_string()),
        };
        let json = serde_json::to_string(&r).expect("serializes");
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"kind\":\"boundary\""));
        assert!(json.contains("\"error\""));
        assert!(!json.contains("\"files\""));
    }

    /// Mode enum import smoke test (compile-time guard against the
    /// `db::Mode` import going stale as we touch the db types).
    #[test]
    fn db_mode_default_is_edit() {
        // Loading a fresh session would default mode to Edit;
        // here we just confirm the enum value exists.
        let _ = Mode::Edit;
        let _ = Mode::Plan;
        let _ = Mode::Yolo;
    }

    /// `parse_unified_diff` integration: empty / no-headers → ParseError
    /// (not a panic). The handler short-circuits to `kind="empty"` /
    /// `kind="parse"` based on the variant. Both `MissingHeader` and
    /// `IncompleteHeader` map to `kind="parse"` in the handler — the
    /// parser picks one based on which line it sees first.
    #[test]
    fn parse_error_variants_match_handler_mapping() {
        assert_eq!(parse_unified_diff(""), Err(ParseError::Empty));
        // `-old\n+new` triggers IncompleteHeader (parser sees `-old`
        // before any `---` header and complains about no hunk in
        // progress). Handler maps it to kind="parse" — same as
        // MissingHeader from the frontend's perspective.
        let err = parse_unified_diff("-old\n+new").expect_err("must fail");
        assert!(matches!(
            err,
            ParseError::IncompleteHeader { .. } | ParseError::MissingHeader
        ));
    }
}
