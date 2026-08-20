//! Subagent dispatch 的模型/提供方/project 解析簇(拆分自 dispatch.rs,
//! 08-07-large-file-splitting)。纯函数 + DB 只读查询,零 IO 副作用。

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::db::subagent_overrides::get_subagent_model_override;
use crate::llm::Provider;
use crate::state::ProviderCatalog;

/// Truth table (matches the PRD's "已闭合" merge semantics):
///
/// | frontmatter default | dispatch `isolation` | result |
/// |---------------------|----------------------|--------|
/// | `Some(true)`        | not specified        | isolated |
/// | `Some(true)`        | `Some(false)`        | shared (LLM opted out) |
/// | `Some(false)`/`None`| `Some(true)`         | isolated (LLM opted in) |
/// | `Some(false)`/`None`| not specified        | shared (legacy behavior) |
/// | `Some(false)`/`None`| `Some(false)`        | shared |
/// | `Some(true)`        | `Some(true)`         | isolated |
///
/// Precedence: **dispatch input > frontmatter default > not isolated**.
/// The dispatch input is the LLM's per-call override (`dispatch_subagent`'s
/// `isolation` parameter); the frontmatter default is the SubagentDef's
/// `isolation` field (builtin `general-purpose` = `Some(true)`,
/// `researcher` = `None`).
/// Truth table (matches the PRD's "已闭合" merge semantics):
///
/// | frontmatter default | dispatch `isolation` | result |
/// |---------------------|----------------------|--------|
/// | `Some(true)`        | not specified        | isolated |
/// | `Some(true)`        | `Some(false)`        | shared (LLM opted out) |
/// | `Some(false)`/`None`| `Some(true)`         | isolated (LLM opted in) |
/// | `Some(false)`/`None`| not specified        | shared (legacy behavior) |
/// | `Some(false)`/`None`| `Some(false)`        | shared |
/// | `Some(true)`        | `Some(true)`         | isolated |
///
/// Precedence: **dispatch input > frontmatter default > not isolated**.
/// The dispatch input is the LLM's per-call override (`dispatch_subagent`'s
/// `isolation` parameter); the frontmatter default is the SubagentDef's
/// `isolation` field (builtin `general-purpose` = `Some(true)`,
/// `researcher` = `None`).
pub fn resolve_isolation(frontmatter_default: Option<bool>, dispatch_input: Option<bool>) -> bool {
    // Dispatch input wins if present; otherwise the frontmatter
    // default; otherwise `false` (legacy shared-cwd behavior).
    dispatch_input.or(frontmatter_default).unwrap_or(false)
}

/// task 07-03-subagent-per-agent-model-ui: priority chain
/// `DB override > frontmatter > parent`. Pure (modulo the DB call
/// for the override lookup) so unit tests can cover the merge
/// without spinning up a full `run_subagent` fixture.
///
/// Decision: read the DB override FIRST (always — even if the
/// frontmatter is `Some(...)`); the DB row is the user-managed
/// "set this agent to this model" affordance, which by design
/// overrides anything the `.md` file declares. If the DB row
/// doesn't exist OR the lookup fails, fall through to the
/// frontmatter `model:` value. If both are absent, the returned
/// `None` lets [`resolve_worker_provider`] handle the
/// parent-inheritance fallback.
///
/// **Catalog-miss decision (NOT a fallback chain)**: when the DB
/// override is `Some(mid)` but the catalog later misses (model
/// was deleted / provider's `api_key` is empty), the
/// `resolve_worker_provider` path already logs `warn!` + falls
/// back to the parent provider (NOT to the frontmatter — the
/// frontmatter is a *declaration* of intent, not a *fallback*).
/// The DB override is the highest-priority declaration; the
/// frontmatter is the second-priority declaration; both are
/// declarations of "which model to use", and the parent
/// inheritance is the catch-all when there's no declaration.
/// This matches the design's stated priority chain (DB > fm >
/// parent) — a missing highest-priority declaration does NOT
/// silently defer to the second-priority declaration; it errors
/// to the parent. Settings UI surfaces invalid overrides with a
/// red "model 已删除" badge so the user can fix them.
///
/// Failure mode: a transient DB error during the override
/// lookup logs `warn!` (in the caller) + falls through to the
/// frontmatter `model:` (NOT to the parent). The Settings UI
/// works on a stable DB; a transient error is rare and the
/// frontmatter is a sensible "default to file" fallback for
/// the duration of the error.
/// B6+ B (task 07-06-b6plus-b-dispatch-model-arg): resolve a model
/// id from either an id (passthrough) or a display_name (reverse
/// lookup). Serves the LLM-driven dispatch path where the
/// `dispatch_subagent` schema's `model` enum values are
/// display_names (human-readable; the LLM has no other way to learn
/// which models exist — `build_system_prompt` does not list models).
///
/// - Exact id match first (`get_model`, O(1)).
/// - Miss → `list_models` reverse-lookup on `display_name`; first
///   match wins (display_name should be unique but DB does not
///   enforce it — the rare ambiguity takes the first row, which is
///   deterministic for a given DB state).
/// - Empty / whitespace-only input → `Ok(None)`.
/// - Not found → `Ok(None)` (NOT an error): the caller treats `None`
///   as "no dispatch override" and falls through to
///   `resolve_final_model`, so a deleted model / typo degrades
///   gracefully to the agent's configured default.
///
/// Returns the resolved model id (catalog key) for the caller to
/// feed into [`resolve_worker_provider`].
pub(crate) async fn resolve_model_by_name_or_id(
    db: &SqlitePool,
    input: &str,
) -> Result<Option<String>, sqlx::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // ① exact id match (passthrough — the LLM may legitimately send
    //    an id it learned from another tool's description).
    if let Some(row) = crate::db::models::get_model(db, trimmed).await? {
        return Ok(Some(row.id));
    }
    // ② display_name reverse lookup (first match wins).
    let models = crate::db::list_models(db).await?;
    Ok(models
        .into_iter()
        .find(|m| m.model.display_name == trimmed)
        .map(|m| m.model.id))
}

pub(crate) async fn resolve_final_model(
    db: &SqlitePool,
    agent_name: &str,
    frontmatter_model: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    // ① DB override (highest priority).
    if let Some(mid) = get_subagent_model_override(db, agent_name).await? {
        return Ok(Some(mid));
    }
    // ② Frontmatter declaration (lowest priority declaration).
    Ok(frontmatter_model.map(str::to_string))
}

/// task 07-03-subagent-frontmatter-model: resolve the worker's
/// provider / context_window / display_name from `def.model`. Pure over
/// (catalog, db) so it's unit-testable without spinning up
/// `run_chat_loop` — the caller (`run_subagent`) holds the catalog read
/// lock and passes `&ProviderCatalog` here.
///
/// - `def_model=None` (or empty after trim) → inherit parent provider + ctx.
/// - `def_model=Some(mid)` + catalog hit → worker provider from catalog;
///   ctx/display from `get_model(mid)` (one DB roundtrip; ctx falls back
///   to parent if the row vanished between catalog build + now).
/// - `def_model=Some(mid)` + catalog miss → `warn!` + inherit parent.
///
/// 08-20-turn-usage-event-quota-view WP2: 第 4 返回值 = provider 行 id
/// (`turn_trace.provider_id` 归因)。catalog 命中分支 `get_model` 已
/// fetch,顺手取 `provider_id`;inherit / miss 分支返回 None,由
/// caller(`resolve_worker`)按父 session 模型回填 —— 回填块本来就在
/// `get_model` 父模型,零额外查询。
pub(crate) async fn resolve_worker_provider(
    def_model: Option<&str>,
    parent_provider: &Arc<dyn Provider>,
    parent_ctx: u32,
    catalog: Option<&ProviderCatalog>,
    db: &SqlitePool,
) -> (Arc<dyn Provider>, u32, Option<String>, Option<String>) {
    let mid = match def_model.map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) => m,
        None => return (parent_provider.clone(), parent_ctx, None, None),
    };
    let hit: Option<Arc<dyn Provider>> = catalog.and_then(|c| c.get(mid).cloned());
    match hit {
        Some(p) => {
            let model_row = crate::db::models::get_model(db, mid).await.ok().flatten();
            let ctx = model_row
                .as_ref()
                .map(|m| m.context_window)
                .unwrap_or(parent_ctx);
            let disp = model_row.as_ref().map(|m| m.display_name.clone());
            let pid = model_row.map(|m| m.provider_id);
            (p, ctx, disp, pid)
        }
        None => {
            tracing::warn!(
                model = mid,
                "subagent model not in catalog (deleted / provider api_key missing); \
                 falling back to parent provider"
            );
            (parent_provider.clone(), parent_ctx, None, None)
        }
    }
}

/// Resolve the project_id for a session. Best-effort DB lookup of
/// `sessions.project_id` — the worker's memory loader needs the
/// project_id to slot into the right MemoryCache entry.
pub(crate) async fn resolve_project_id(db: &SqlitePool, session_id: &str) -> String {
    match crate::db::load_session(db, session_id).await {
        Ok(Some(loaded)) => loaded.session.project_id,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                "run_subagent: failed to load session for project_id; falling back to empty"
            );
            String::new()
        }
    }
}

/// Resolve the project's MAIN repo path (the directory containing
/// `.git/`) for a session. L3b (2026-06-27): used by
/// `create_worker` / `destroy_worker` which need the main repo to
/// open libgit2 + manage linked worktrees.
///
/// This is distinct from `current_ctx.worktree_path` (which is the
/// PARENT SESSION's worktree — a linked worktree, NOT the main
/// repo). The project row's `path` field is the main repo path.
pub(crate) async fn resolve_project_main_path(db: &SqlitePool, session_id: &str) -> String {
    let project_id = resolve_project_id(db, session_id).await;
    if project_id.is_empty() {
        return String::new();
    }
    match crate::db::get_project(db, &project_id).await {
        Ok(Some(p)) => p.path,
        _ => {
            tracing::warn!(
                session_id = %session_id,
                project_id = %project_id,
                "run_subagent: failed to load project for main path; falling back to empty"
            );
            String::new()
        }
    }
}
