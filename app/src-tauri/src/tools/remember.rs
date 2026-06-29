//! P2 `remember` tool — agent self-writes a long-term memory.
//!
//! The model calls `remember(...)` to persist a piece of cross-session
//! experience into `autonomous_memories`. The row is always written
//! at `status = candidate` (spike-007 §3 state machine; P5 wires the
//! promotion rules). P2's session-start recall surfaces candidate
//! rows too (PRD ADR-lite) so a freshly-remembered memory is
//! immediately recallable in the next session.
//!
//! # Permission
//!
//! **Silent Allow** (does NOT route to Tier 4 ask). Per epic decision
//! "全自主写 + 安全网兜底" — the write-safety net inside
//! `db::memories::insert_memory` (sensitive-content regex, sensitive-
//! /temporary-path deny-list, length caps, path generalization) is
//! the guard; a user-confirmation modal would break the autonomous
//! "see a pitfall, remember it" UX. `risk_for_tool` returns
//! `Risk::Low` (the `_` default); Plan mode keeps the tool (writes
//! land in the DB, not the filesystem).
//!
//! # Frequency control (spike-005 §4.3)
//!
//! - **Same-session ≤ 50 rows** — enforced here via
//!   `count_memories_for_session` BEFORE the insert. Hard reject
//!   above the cap with an actionable message (the model can retry
//!   after the user prunes; or it can overwrite an existing memory
//!   by deleting first — out of scope for P2's tool surface).
//! - **Same-turn ≤ 3 calls** — NOT enforced here. The tool is
//!   stateless (no turn counter in `ToolContext`); a per-turn
//!   counter would require threading new state through
//!   `run_chat_loop`'s 26-param signature, which is out of scope
//!   for P2. The session cap (50) + the write-safety net are the
//!   load-bearing guards; the per-turn rule is a "nice to have"
//!   documented for P5 to wire if write-spam becomes a real
//!   problem. The cap is also self-correcting: 50 candidate rows
//!   from one turn would still all be candidate (never promoted),
//!   so they'd age out under P5's hygiene job.
//!
//! # Pitfall `trigger_key`
//!
//! When `kind = pitfall`, the model SHOULD supply a structured
//! trigger key (`tool_name` + optional `command_pattern` + optional
//! `path_globs`) so P3's pre-tool recall (`find_pitfalls_by_trigger`)
//! can match the pitfall against the live `tool_name + tool_input`
//! in O(1). Non-pitfall kinds ignore these fields.

use crate::db::memories::{
    count_memories_for_session, insert_memory, MemoryInput, MemoryKind, MemoryScope,
    MemoryStatus,
};
use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// Per-session write cap (spike-005 §4.3 "same session ≤ 50"). Above
/// this the `remember` tool returns `is_error: true` with an
/// actionable message; the model is expected to surface this to the
/// user (the user prunes via the MemoryPreview UI in PR3).
pub const REMEMBER_SESSION_CAP: i64 = 50;

/// The `remember` tool definition registered in `builtin_tools()`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "remember".to_string(),
        description: Some(
            "Persist a piece of long-term, cross-session experience memory. Use this when \
             you discover a reusable pitfall, preference, fact, or decision that future \
             sessions would benefit from recalling. The memory is written at `candidate` \
             status and surfaces in future sessions via the session-start recall (matched \
             against the user's latest message). Write CONCISE experience text — one \
             sentence a future you can act on, NOT a log dump.\n\n\
             When to remember:\n\
             - A tool failed ≥ 2 times in a row for the same reason and you eventually \
               worked around it (pitfall).\n\
             - The user explicitly corrected your approach ('no, do it this way') — \
               capture the correction (preference / pitfall).\n\
             - You discovered a non-obvious project convention (build flag, env var, \
               path alias) that isn't in the docs (fact).\n\
             - An architectural / design choice was made that constrains future work \
               (decision).\n\n\
             Do NOT remember:\n\
             - API keys, tokens, secrets, passwords (rejected by the safety net).\n\
             - User PII / home-directory paths (auto-generalized to `~/`).\n\
             - Ephemeral task state (use `update_checklist` instead).\n\
             - Temporary paths like /tmp/ (rejected).\n\n\
             For `kind: pitfall`, supply `tool_name` (+ optional `command_pattern` / \
             `path_globs`) so the pre-tool recall can match it precisely."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short headline (≤ 200 chars). Used for search + display."
                },
                "content": {
                    "type": "string",
                    "description": "The experience text (≤ 500 chars). One sentence a future session can act on."
                },
                "kind": {
                    "type": "string",
                    "enum": ["pitfall", "preference", "fact", "decision"],
                    "description": "Memory category."
                },
                "scope": {
                    "type": "string",
                    "enum": ["user", "project"],
                    "default": "project",
                    "description": "`user` = cross-project (global to you); `project` = scoped to the active project."
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional keyword tags to aid recall."
                },
                "tool_name": {
                    "type": "string",
                    "description": "pitfall only: the tool this pitfall is about (e.g. 'shell'). Enables precise pre-tool recall."
                },
                "command_pattern": {
                    "type": "string",
                    "description": "pitfall only: a substring of the command that triggers the pitfall (e.g. 'cargo test')."
                },
                "path_globs": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "pitfall only: glob patterns (session_tool_permissions-style, `*` does NOT cross `/`) restricting the pitfall to specific paths."
                }
            },
            "required": ["title", "content", "kind"]
        }),
    }
}

/// Parse the LLM-supplied JSON into the `MemoryInput` write bundle.
///
/// - `title` / `content` / `kind` are required; missing → `Err`.
/// - `scope` defaults to `project` (the most common case). The
///   project_id binding is resolved from `ctx` (see `execute`).
/// - `tags` is JSON-encoded into the `tags` TEXT column (empty array
///   if absent).
/// - `tool_name` / `command_pattern` / `path_globs` are pitfall-only;
///   for other kinds they're ignored (left `None`).
///
/// Returns `Err(message)` on any validation failure; the caller
/// surfaces the message as `is_error: true` tool_result.
fn build_input(
    input: &serde_json::Value,
    project_id: Option<String>,
) -> Result<MemoryInput, String> {
    let title = input
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("remember requires a `title` string")?
        .trim()
        .to_string();
    if title.is_empty() {
        return Err("`title` must not be empty".to_string());
    }
    let content = input
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("remember requires a `content` string")?
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("`content` must not be empty".to_string());
    }
    let kind = match input.get("kind").and_then(|v| v.as_str()) {
        Some("pitfall") => MemoryKind::Pitfall,
        Some("preference") => MemoryKind::Preference,
        Some("fact") => MemoryKind::Fact,
        Some("decision") => MemoryKind::Decision,
        Some(other) => return Err(format!("unknown `kind` '{}'", other)),
        None => return Err("remember requires a `kind` string".to_string()),
    };
    let scope = match input.get("scope").and_then(|v| v.as_str()) {
        Some("user") => MemoryScope::User,
        // Default + "project" + any unrecognized → project (the
        // most common scope; lenient parse matches the DB-side
        // enum's `from_str_opt`).
        _ => MemoryScope::Project,
    };

    // scope/project_id interaction (H2): user scope MUST NOT carry a
    // project_id (insert_memory rejects it); project scope MUST have
    // one. We enforce at this layer so the error message is
    // actionable BEFORE the DB call.
    let project_id = match (scope, project_id) {
        (MemoryScope::User, _) => None,
        (MemoryScope::Project, Some(id)) => Some(id),
        (MemoryScope::Project, None) => {
            return Err(
                "remember: `scope=project` requires an active project; the session has none"
                    .to_string(),
            );
        }
    };

    // tags: JSON-encode the array (or "[]" if absent / malformed).
    let tags = match input.get("tags").and_then(|v| v.as_array()) {
        Some(arr) => {
            let strs: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            serde_json::to_string(&strs).unwrap_or_else(|_| "[]".to_string())
        }
        None => "[]".to_string(),
    };

    // pitfall-only structured trigger key.
    let (tool_name, command_pattern, path_globs) = if matches!(kind, MemoryKind::Pitfall) {
        let tool_name = input
            .get("tool_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let command_pattern = input
            .get("command_pattern")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let path_globs = input.get("path_globs").and_then(|v| v.as_array()).map(|arr| {
            let strs: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            serde_json::to_string(&strs).unwrap_or_else(|_| "[]".to_string())
        });
        (tool_name, command_pattern, path_globs)
    } else {
        (None, None, None)
    };

    Ok(MemoryInput {
        scope,
        project_id,
        kind,
        // P2 always writes candidate; P5 wires promotion.
        status: MemoryStatus::Candidate,
        title,
        content,
        tags,
        tool_name,
        command_pattern,
        path_globs,
        // source_session_id is filled by `execute` (it has the
        // session id); source_ref is left None for P2 (the
        // remember tool has no turn/tool_call id context).
        source_session_id: None,
        source_ref: None,
    })
}

/// Execute `remember`: parse → frequency check → insert → return
/// the new memory_id. `session_id` is threaded from the agent loop
/// (the `execute_tool` dispatch passes it for every tool); it
/// becomes the row's `source_session_id` for frequency-control
/// accounting + future audit.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool) {
    // Resolve the project_id from the ToolContext (the session's
    // `projects.id` UUID, threaded from chat_loop). This is the same
    // identifier the session-start recall filters by, so a written
    // memory is immediately recallable. `scope=user` memories don't
    // carry a project_id (they're cross-project); `scope=project`
    // binds this id.
    let project_id_for_input = Some(ctx.project_id.clone());

    let mut mem_input = match build_input(input, project_id_for_input) {
        Ok(inp) => inp,
        Err(e) => return (e, true),
    };
    // Stamp source_session_id for frequency control.
    if let Some(sid) = session_id {
        mem_input.source_session_id = Some(sid.to_string());
    }

    // Frequency control (spike-005 §4.3): same-session ≤ 50.
    if let Some(sid) = session_id {
        let count = count_memories_for_session(&ctx.db, sid).await;
        if count >= REMEMBER_SESSION_CAP {
            return (
                format!(
                    "Memory cap reached: this session already has {} memories (limit {}). \
                     Delete one via the Memory UI before remembering more.",
                    count, REMEMBER_SESSION_CAP
                ),
                true,
            );
        }
    }

    match insert_memory(&ctx.db, &mem_input).await {
        Ok(row) => {
            tracing::info!(
                memory_id = %row.memory_id,
                kind = %row.kind,
                scope = %row.scope,
                "remember: wrote candidate memory",
            );
            (
                format!(
                    "Remembered (status: candidate, will surface in future sessions):\n\
                     id: {}\n\
                     title: {}\n\
                     kind: {}\n\
                     scope: {}",
                    row.memory_id, row.title, row.kind, row.scope
                ),
                false,
            )
        }
        Err(e) => {
            tracing::warn!(error = %e, "remember: insert rejected by safety net");
            (format!("remember rejected: {}", e), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    /// Build a ToolContext backed by a FRESH in-memory pool (with
    /// migrations + FK pragma). Each test gets its own pool so
    /// writes don't leak across tests. We can't use the
    /// process-wide `test_default_pool` helper here because (a)
    /// it's a `OnceLock` so migrations would only run once across
    /// the whole test binary and (b) the remember tool IS a DB
    /// writer, unlike the other tools the helper was designed for.
    async fn make_ctx() -> (ToolContext, SqlitePool) {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory connect");
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .expect("FK pragma");
        crate::db::migrations::run_migrations(&pool)
            .await
            .expect("migrations");
        let ctx = ToolContext {
            worktree_path: std::path::PathBuf::from("/repo/proj"),
            cwd: std::path::PathBuf::from("/repo/proj"),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: pool.clone(),
            project_id: "/repo/proj".to_string(),
            data_dir: std::path::PathBuf::from("/repo"),
        };
        (ctx, pool)
    }

    // ---- definition ----

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "remember");
    }

    #[test]
    fn definition_schema_requires_title_content_kind() {
        let schema = definition().input_schema;
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array present");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"title"));
        assert!(names.contains(&"content"));
        assert!(names.contains(&"kind"));
    }

    #[test]
    fn definition_schema_kind_enum_covers_four() {
        let schema = definition().input_schema;
        let strs: Vec<&str> = schema
            .pointer("/properties/kind/enum")
            .and_then(|v| v.as_array())
            .expect("kind enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(strs, vec!["pitfall", "preference", "fact", "decision"]);
    }

    // ---- build_input ----

    #[test]
    fn build_input_happy_path_project_scope() {
        let v = serde_json::json!({
            "title": "WSL cargo test",
            "content": "set PKG_CONFIG_PATH before cargo test in WSL",
            "kind": "pitfall",
            "tool_name": "shell",
            "command_pattern": "cargo test",
            "tags": ["wsl", "cargo"]
        });
        let inp = build_input(&v, Some("/repo/proj".into())).unwrap();
        assert_eq!(inp.kind, MemoryKind::Pitfall);
        assert_eq!(inp.scope, MemoryScope::Project);
        assert_eq!(inp.project_id.as_deref(), Some("/repo/proj"));
        assert_eq!(inp.status, MemoryStatus::Candidate);
        assert_eq!(inp.tool_name.as_deref(), Some("shell"));
        assert_eq!(inp.command_pattern.as_deref(), Some("cargo test"));
        assert_eq!(inp.tags, "[\"wsl\",\"cargo\"]");
    }

    #[test]
    fn build_input_user_scope_strips_project_id() {
        let v = serde_json::json!({
            "title": "prefer absolute paths",
            "content": "the user prefers absolute paths in tool calls",
            "kind": "preference",
            "scope": "user"
        });
        let inp = build_input(&v, Some("/repo/proj".into())).unwrap();
        assert_eq!(inp.scope, MemoryScope::User);
        assert!(inp.project_id.is_none(), "user scope strips project_id");
    }

    #[test]
    fn build_input_project_scope_without_project_errors() {
        let v = serde_json::json!({
            "title": "x", "content": "y", "kind": "fact"
        });
        let err = build_input(&v, None).unwrap_err();
        assert!(err.contains("active project"), "{}", err);
    }

    #[test]
    fn build_input_missing_title_errors() {
        let v = serde_json::json!({"content": "y", "kind": "fact"});
        assert!(build_input(&v, Some("/repo".into())).is_err());
    }

    #[test]
    fn build_input_unknown_kind_errors() {
        let v = serde_json::json!({"title": "t", "content": "c", "kind": "rule"});
        assert!(build_input(&v, Some("/repo".into())).is_err());
    }

    #[test]
    fn build_input_non_pitfall_ignores_trigger_fields() {
        let v = serde_json::json!({
            "title": "t", "content": "c", "kind": "fact",
            "tool_name": "shell", "command_pattern": "cargo"
        });
        let inp = build_input(&v, Some("/repo".into())).unwrap();
        assert!(inp.tool_name.is_none());
        assert!(inp.command_pattern.is_none());
    }

    // ---- execute (roundtrip + safety net + frequency) ----

    #[tokio::test]
    async fn execute_writes_candidate_roundtrip() {
        let (ctx, pool) = make_ctx().await;
        let v = serde_json::json!({
            "title": "prefer tabs",
            "content": "the user prefers tabs over spaces",
            "kind": "preference"
        });
        let (out, is_err) = execute(&v, &ctx, Some("sess-A")).await;
        assert!(!is_err, "{}", out);
        assert!(out.contains("status: candidate"));
        // Row exists in DB.
        let rows = crate::db::memories::list_memories(
            &pool,
            Some(MemoryScope::Project),
            Some("/repo/proj"),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "prefer tabs");
        assert_eq!(rows[0].status, "candidate");
        assert_eq!(rows[0].source_session_id.as_deref(), Some("sess-A"));
    }

    #[tokio::test]
    async fn execute_rejects_sensitive_content() {
        let (ctx, _pool) = make_ctx().await;
        let v = serde_json::json!({
            "title": "leaked key",
            "content": "the api_key is sk-abc123",
            "kind": "fact"
        });
        let (out, is_err) = execute(&v, &ctx, Some("sess-A")).await;
        assert!(is_err, "sensitive content rejected");
        assert!(out.contains("rejected"), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_when_session_cap_reached() {
        let (ctx, pool) = make_ctx().await;
        // Seed REMEMBER_SESSION_CAP rows attributed to sess-full.
        for i in 0..REMEMBER_SESSION_CAP {
            let inp = MemoryInput {
                scope: MemoryScope::User,
                project_id: None,
                kind: MemoryKind::Fact,
                status: MemoryStatus::Candidate,
                title: format!("seed {}", i),
                content: "seed content cargo".into(),
                tags: "[]".into(),
                tool_name: None,
                command_pattern: None,
                path_globs: None,
                source_session_id: Some("sess-full".into()),
                source_ref: None,
            };
            insert_memory(&pool, &inp).await.unwrap();
        }
        // Next write from the same session → rejected.
        let v = serde_json::json!({
            "title": "over cap",
            "content": "should be rejected",
            "kind": "fact",
            "scope": "user"
        });
        let (out, is_err) = execute(&v, &ctx, Some("sess-full")).await;
        assert!(is_err, "over-cap write rejected");
        assert!(out.contains("cap"), "{}", out);
        assert!(out.contains("50"));
    }

    #[tokio::test]
    async fn execute_other_session_under_cap_succeeds() {
        let (ctx, pool) = make_ctx().await;
        // sess-full at cap; sess-other has 0 → write succeeds.
        for i in 0..REMEMBER_SESSION_CAP {
            let inp = MemoryInput {
                scope: MemoryScope::User,
                project_id: None,
                kind: MemoryKind::Fact,
                status: MemoryStatus::Candidate,
                title: format!("seed {}", i),
                content: "seed content cargo".into(),
                tags: "[]".into(),
                tool_name: None,
                command_pattern: None,
                path_globs: None,
                source_session_id: Some("sess-full".into()),
                source_ref: None,
            };
            insert_memory(&pool, &inp).await.unwrap();
        }
        let v = serde_json::json!({
            "title": "ok",
            "content": "writes from a different session succeed",
            "kind": "fact",
            "scope": "user"
        });
        let (out, is_err) = execute(&v, &ctx, Some("sess-other")).await;
        assert!(!is_err, "{}", out);
    }
}
