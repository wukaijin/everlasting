//! D2② `search_history` tool — agent-driven cross-session full-text
//! search (ROADMAP D2's second driver, `08-17-agent-search-history-
//! tool`; the first driver is the user-facing SearchModal from
//! `08-17-cross-session-search`).
//!
//! The model calls `search_history({query})` to find PAST
//! conversation messages across ALL projects (plus session titles) —
//! "上次怎么解的 X / 之前讨论过 Y 吗" lookups that neither the
//! autonomous-memory recall (distilled experiences) nor the
//! filesystem tools can answer.
//!
//! # Layering
//!
//! Thin wrapper over the shared query layer [`crate::db::search::
//! search_messages`] (zero SQL here): input parsing + LLM-facing
//! text formatting. The ≥3-chars FTS / <3-chars LIKE dispatch,
//! `bm25` ranking, title-hit rider, and snippet cutting all live in
//! the db layer and are inherited as-is.
//!
//! # Permission
//!
//! **Silent Allow** (Tier 5 via `ToolKind::Other` default) — same
//! model as `remember`: a read-only DB query with no side effects,
//! so no Tier 4 ask. `risk_for_tool` returns `Risk::Low` (the `_`
//! default); Plan mode keeps the tool (`filter_tools_for_mode` only
//! strips write-class tools); not in the kill list. Not in
//! `STRUCTURALLY_DISABLED`, so serial general-purpose workers get
//! it; `READONLY_TOOL_ALLOWLIST` carries it so concurrent read-only
//! workers keep it too. NOT a C7D stub candidate (3-param schema).
//!
//! # Limits
//!
//! Agent-side cap (50) is deliberately tighter than the modal's 200
//! — every hit line lands in the LLM context; ~20 lines ≈ 3k tok is
//! the right single-call budget. The two caps are independent
//! constants by design (modal pages for humans, tool budgets for
//! models).

use crate::db::search::{search_messages, MessageSearchHit, SearchHitKind};
use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// Default page size for the agent surface (modal uses its own 50).
pub const DEFAULT_LIMIT: u32 = 20;
/// Agent-side upper bound. Deliberately ≠ `db::search`'s MAX_LIMIT
/// (200) — see module doc "Limits".
pub const MAX_LIMIT: u32 = 50;

/// The `search_history` tool definition registered in
/// `builtin_tools()` (appended last — order feeds the provider
/// prefix cache; appending never shifts the existing prefix).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "search_history".to_string(),
        description: Some(
            "Search the full text of past conversation messages across all projects \
             (all sessions) plus session titles. Use when the user asks about earlier \
             discussions, past decisions, or how something was solved before. Returns \
             one line per hit: date, project / session title, #seq, role, snippet. \
             ≥3-char queries use full-text match; shorter use substring (2-char CJK \
             words work)."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search text (trimmed). Empty is rejected."
                },
                "scope": {
                    "type": "string",
                    "enum": ["all", "current_project"],
                    "default": "all",
                    "description": "`all` = every project; `current_project` = only the active project."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 20,
                    "description": "Max hits returned."
                }
            },
            "required": ["query"]
        }),
    }
}

/// Search scope — the agent reasons in terms of "everywhere" vs
/// "this project", not project UUIDs (which it never sees).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    CurrentProject,
}

/// Parse + validate the LLM input. Strict on `scope` (an unknown
/// value changes WHAT is searched — fail loud, don't guess), lenient
/// on `limit` (a bad number doesn't change search direction — fall
/// back to the default).
fn parse_args(input: &serde_json::Value) -> Result<(String, Scope, u32), String> {
    let query = input
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("search_history requires a `query` string")?
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("`query` must not be empty or whitespace".to_string());
    }
    let scope = match input.get("scope").and_then(|v| v.as_str()) {
        None | Some("all") => Scope::All,
        Some("current_project") => Scope::CurrentProject,
        Some(other) => {
            return Err(format!(
                "unknown `scope` '{}' (expected \"all\" or \"current_project\")",
                other
            ))
        }
    };
    let limit = match input.get("limit").and_then(|v| v.as_u64()) {
        Some(n) if n >= 1 => u32::try_from(n).unwrap_or(MAX_LIMIT).min(MAX_LIMIT),
        // Absent, 0, negative, or non-integer → default. Not an error.
        _ => DEFAULT_LIMIT,
    };
    Ok((query, scope, limit))
}

/// Execute: parse → shared query layer → format. `session_id` (the
/// caller's own session, threaded by `execute_tool_inner`) only
/// powers the `(this session)` marker on hits the model already has
/// in context — nudging it to skip those instead of re-reading them.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    session_id: Option<&str>,
) -> (String, bool) {
    let (query, scope, limit) = match parse_args(input) {
        Ok(args) => args,
        Err(e) => return (e, true),
    };
    let project_filter = match scope {
        Scope::All => None,
        Scope::CurrentProject => Some(ctx.project_id.as_str()),
    };
    match search_messages(&ctx.db, &query, project_filter, Some(limit)).await {
        Ok(hits) => (format_hits(&hits, &query, scope, session_id), false),
        Err(e) => {
            tracing::warn!(error = %e, query = %query, "search_history: db query failed");
            (format!("search_history failed: {}", e), true)
        }
    }
}

fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::All => "scope: all projects",
        Scope::CurrentProject => "scope: current project",
    }
}

/// Render hits as compact one-per-line text for the LLM (NOT the
/// `MessageSearchHit` JSON — the ① design already fixed the split:
/// SQL shared, presentation per driver).
fn format_hits(
    hits: &[MessageSearchHit],
    query: &str,
    scope: Scope,
    current_session: Option<&str>,
) -> String {
    if hits.is_empty() {
        return format!(
            "No history hits for \"{}\" ({}). Try a longer, more distinctive phrase, \
             or a different wording of the same concept.",
            query,
            scope_label(scope)
        );
    }
    let mut out = format!(
        "Found {} hits for \"{}\" ({}):\n",
        hits.len(),
        query,
        scope_label(scope)
    );
    for (i, h) in hits.iter().enumerate() {
        let date = h.updated_at.get(..10).unwrap_or(h.updated_at.as_str());
        let project = h.project_name.as_deref().unwrap_or("(unnamed project)");
        let this = if Some(h.session_id.as_str()) == current_session {
            " (this session)"
        } else {
            ""
        };
        match h.kind {
            SearchHitKind::Title => out.push_str(&format!(
                "{}. [title match] [{}] {} / {}{}\n",
                i + 1,
                date,
                project,
                h.session_title,
                this
            )),
            SearchHitKind::Content => {
                let seq = h.seq.map(|s| s.to_string()).unwrap_or_default();
                let role = h.role.as_deref().unwrap_or("?");
                let speaker = match h.speaker.as_deref() {
                    Some(s) => format!(" ({})", s),
                    None => String::new(),
                };
                let snippet = h.snippet.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "{}. [{}] {} / {} · #{} {}{}{}: {}\n",
                    i + 1,
                    date,
                    project,
                    h.session_title,
                    seq,
                    role,
                    speaker,
                    this,
                    snippet
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    use crate::db::projects::create_project;
    use crate::db::sessions::{create_session, persist_turn};
    use crate::llm::types::{ContentBlock, MessageContent, Role};

    /// Fresh in-memory pool + migrations per test (seeds must not
    /// leak across tests — mirrors `db/search_tests.rs::test_pool`).
    async fn make_ctx() -> (ToolContext, sqlx::SqlitePool) {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
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
            project_id: String::new(), // caller overwrites per test
            data_dir: std::path::PathBuf::from("/repo"),
            workflow_name: None,
            mode: crate::db::Mode::Edit,
        };
        (ctx, pool)
    }

    async fn say(pool: &sqlx::SqlitePool, session_id: &str, role: Role, text: &str, seq: i64) {
        let content = MessageContent::Blocks(vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }]);
        persist_turn(pool, session_id, role, &content, seq, None, None)
            .await
            .unwrap();
    }

    // ---- definition ----

    #[test]
    fn definition_has_correct_name_and_required_query_only() {
        let def = definition();
        assert_eq!(def.name, "search_history");
        let required: Vec<&str> = def
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["query"]);
    }

    #[test]
    fn definition_scope_enum_and_limit_max() {
        let schema = definition().input_schema;
        let scope_enum: Vec<&str> = schema
            .pointer("/properties/scope/enum")
            .and_then(|v| v.as_array())
            .expect("scope enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(scope_enum, vec!["all", "current_project"]);
        assert_eq!(
            schema
                .pointer("/properties/limit/maximum")
                .and_then(|v| v.as_u64()),
            Some(MAX_LIMIT as u64)
        );
    }

    // ---- parse_args ----

    #[test]
    fn parse_args_defaults_scope_all_limit_20() {
        let (q, scope, limit) = parse_args(&serde_json::json!({"query": "worktree"})).unwrap();
        assert_eq!(q, "worktree");
        assert_eq!(scope, Scope::All);
        assert_eq!(limit, DEFAULT_LIMIT);
    }

    #[test]
    fn parse_args_trims_and_clamps_limit() {
        let (q, scope, limit) = parse_args(&serde_json::json!({
            "query": "  worktree  ", "scope": "current_project", "limit": 999
        }))
        .unwrap();
        assert_eq!(q, "worktree");
        assert_eq!(scope, Scope::CurrentProject);
        assert_eq!(limit, MAX_LIMIT);
        // limit 0 / garbage → default (lenient, see parse_args doc)
        let (_, _, l0) = parse_args(&serde_json::json!({"query": "x", "limit": 0})).unwrap();
        assert_eq!(l0, DEFAULT_LIMIT);
    }

    #[test]
    fn parse_args_rejects_empty_query_and_unknown_scope() {
        assert!(parse_args(&serde_json::json!({"query": "  "})).is_err());
        assert!(parse_args(&serde_json::json!({})).is_err());
        let err =
            parse_args(&serde_json::json!({"query": "x", "scope": "everywhere"})).unwrap_err();
        assert!(
            err.contains("everywhere"),
            "error names the bad value: {err}"
        );
    }

    // ---- execute (integration against the real query layer) ----

    #[tokio::test]
    async fn execute_formats_content_and_title_hits() {
        let (mut ctx, pool) = make_ctx().await;
        let p = create_project(&pool, "proj-a", "/tmp/pa", false, None)
            .await
            .unwrap();
        ctx.project_id = p.id.clone();
        let s = create_session(
            &pool,
            &Uuid::new_v4().to_string(),
            &p.id,
            "/tmp/pa",
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        say(
            &pool,
            &s.id,
            Role::Assistant,
            "we solved the worktree attach failure via lazy auto-attach",
            3,
        )
        .await;

        let (out, is_err) =
            execute(&serde_json::json!({"query": "worktree"}), &ctx, Some(&s.id)).await;
        assert!(!is_err);
        assert!(out.contains("Found"), "{out}");
        assert!(out.contains("proj-a"), "project name: {out}");
        assert!(out.contains("#3"), "seq: {out}");
        assert!(out.contains("assistant"), "role: {out}");
        assert!(out.contains("(this session)"), "marker: {out}");
        assert!(out.contains("lazy auto-attach"), "snippet: {out}");
        // Session auto-title won't necessarily match; title-hit line
        // shape is covered by the dedicated test below.
    }

    #[tokio::test]
    async fn execute_scope_current_project_filters_other_projects() {
        let (mut ctx, pool) = make_ctx().await;
        let pa = create_project(&pool, "proj-a", "/tmp/pa", false, None)
            .await
            .unwrap();
        let pb = create_project(&pool, "proj-b", "/tmp/pb", false, None)
            .await
            .unwrap();
        ctx.project_id = pa.id.clone();
        for p in [&pa, &pb] {
            let s = create_session(
                &pool,
                &Uuid::new_v4().to_string(),
                &p.id,
                "/tmp/x",
                "GLM-4.7",
                None,
                None,
                None,
            )
            .await
            .unwrap();
            say(&pool, &s.id, Role::User, "migration docsize backfill", 0).await;
        }

        let (all, _) = execute(&serde_json::json!({"query": "docsize"}), &ctx, None).await;
        assert!(all.contains("proj-a") && all.contains("proj-b"), "{all}");

        let (scoped, _) = execute(
            &serde_json::json!({"query": "docsize", "scope": "current_project"}),
            &ctx,
            None,
        )
        .await;
        assert!(scoped.contains("proj-a"), "{scoped}");
        assert!(!scoped.contains("proj-b"), "scope filters: {scoped}");
    }

    #[tokio::test]
    async fn execute_title_hit_line_and_two_char_cjk_fallback() {
        let (mut ctx, pool) = make_ctx().await;
        let p = create_project(&pool, "proj-a", "/tmp/pa", false, None)
            .await
            .unwrap();
        ctx.project_id = p.id.clone();
        // Title containing the query; body text uses a 2-char CJK
        // word so the LIKE fallback path (agent side inherits it
        // transparently) is exercised in the same test.
        let s = create_session(
            &pool,
            &Uuid::new_v4().to_string(),
            &p.id,
            "/tmp/pa",
            "GLM-4.7",
            None,
            None,
            None,
        )
        .await
        .unwrap();
        crate::db::sessions::rename_session(&pool, &s.id, "权限系统设计讨论")
            .await
            .unwrap();
        say(&pool, &s.id, Role::User, "权限不足导致失败", 0).await;

        let (out, is_err) = execute(&serde_json::json!({"query": "权限"}), &ctx, None).await;
        assert!(!is_err);
        assert!(out.contains("[title match]"), "title hit line: {out}");
        assert!(out.contains("权限系统设计讨论"), "{out}");
        // 2-char CJK rides the LIKE fallback → content hit present.
        assert!(out.contains("#0"), "content hit via LIKE: {out}");
    }

    #[tokio::test]
    async fn execute_zero_hits_is_not_error_and_empty_query_is() {
        let (ctx, _pool) = make_ctx().await;
        let (out, is_err) = execute(
            &serde_json::json!({"query": "definitely-no-such-phrase"}),
            &ctx,
            None,
        )
        .await;
        assert!(!is_err);
        assert!(out.starts_with("No history hits"), "{out}");

        let (out, is_err) = execute(&serde_json::json!({"query": "  "}), &ctx, None).await;
        assert!(is_err);
        assert!(out.contains("query"), "{out}");
    }

    /// AC6: the read-only strip must KEEP `search_history` — it is a
    /// read-only DB query, so concurrent read-only workers retain it.
    #[test]
    fn readonly_allowlist_keeps_search_history() {
        assert!(crate::agent::subagent::READONLY_TOOL_ALLOWLIST.contains(&"search_history"));
    }
}
