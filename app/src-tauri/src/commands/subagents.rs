//! 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 3):
//! Settings-UI-facing IPCs for per-subagent model configuration.
//!
//! Two commands:
//!
//! - [`list_subagents_with_model`] — extension of the existing
//!   `list_subagents` (in `panel.rs`) that ALSO carries the
//!   resolved model per row. The Settings UI's `SubagentsTab`
//!   consumes this directly. The DB override is layered on top of
//!   the cache's frontmatter-declared value per the
//!   `DB > frontmatter > parent` priority chain (the chain itself
//!   lives in `agent::subagent::dispatch::resolve_final_model`;
//!   this IPC reuses the same lookup to keep both surfaces in
//!   lockstep).
//!
//! - [`set_subagent_model`] — write-side. Dispatches by `source`:
//!   `builtin` → DB override table; `user` / `project` →
//!   `write_frontmatter_model` on the loader-resolved file path.
//!   `model_id = None` means "inherit parent" — clears the DB
//!   row for builtin OR removes the `model:` line for
//!   user/project. Returns the post-update row (so the frontend
//!   can refresh without a follow-up `list_subagents_with_model`
//!   roundtrip).
//!
//! Both commands return `Result<T, AppCommandError>` per the
//! project's IPC convention (see `error.rs` + A5 task). DB
//! errors and IO errors are wrapped as `Server`; user-input
//! errors (invalid name, no fence, builtin file path miss) are
//! `InvalidRequest` so the frontend's typed error handler can
//! show the right toast.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::agent::subagent::SubagentSource;
use crate::agent::subagent::{locate_agent_file, write_frontmatter_model};
use crate::db;
use crate::db::subagent_overrides::{
    clear_subagent_model_override, get_subagent_model_override,
    list_subagent_model_overrides as db_list_overrides, set_subagent_model_override,
};
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// list_subagents_with_model — UI-facing list of all subagents with
// their resolved model (DB override > frontmatter > None=inherit)
// ---------------------------------------------------------------------------

/// One row in the `list_subagents_with_model` response. Mirrors
/// `SubagentInfo` (the existing `list_subagents` panel IPC) with
/// 4 extra model fields + a `writable` flag.
///
/// `writable` is `false` for `source=builtin` (no frontmatter
/// file exists, so the UI must route writes to the DB override
/// table). `user` / `project` rows are writable via the
/// `write_frontmatter_model` IO helper.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentWithModelRow {
    pub name: String,
    pub description: String,
    /// `"builtin"` | `"user"` | `"project"`.
    pub source: String,
    pub tools: Vec<String>,
    /// Final model id after `DB > frontmatter > None` resolution.
    /// `None` = inherit parent (no DB override AND no frontmatter
    /// `model:` declared). The frontend renders a "继承父级" chip
    /// on null.
    pub resolved_model_id: Option<String>,
    /// `models.display_name` for the resolved id, fetched via a
    /// single `list_models` projection. `None` when
    /// `resolved_model_id` is `None` OR the model has been
    /// deleted (catalog miss) — the UI shows the raw id + a red
    /// "model 已删除" badge in the latter case.
    pub resolved_model_display: Option<String>,
    /// Raw `model:` value from the frontmatter (before DB
    /// overlay). For debug / "what does the file say" tooltip;
    /// the resolved value is the user-facing one.
    pub declared_model_id: Option<String>,
    /// `true` iff the DB override table has a row for this name.
    /// Drives the "(DB override)" chip in the UI.
    pub has_db_override: bool,
    /// `true` for `source=user|project` (frontmatter is writable).
    /// `false` for `source=builtin` (writes route to the DB table).
    pub writable: bool,
}

/// `project_path` is the canonical worktree path string (the same
/// one `SubagentCache::list` uses as its scan key). The frontend
/// should pass `useChatStore.currentCwd` (canonicalized by the
/// backend via the existing `resolve_project_path` helper in
/// `panel.rs`).
pub async fn list_subagents_with_model_inner(
    project_path: String,
    state: &Arc<AppState>,
) -> Result<Vec<SubagentWithModelRow>, AppCommandError> {
    // Step 1: scan the cache for builtin + user + project.
    let loaded = state.subagent_cache.list(&project_path).await;

    // Step 2: load all DB overrides in a single query (cheaper
    // than per-agent lookup) + load the models catalog for the
    // display_name projection.
    let overrides: HashMap<String, String> = db_list_overrides(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_subagents_with_model: list overrides failed: {}", e))?
        .into_iter()
        .collect();
    let models = db::models::list_models(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_subagents_with_model: list models failed: {}", e))?;
    // `ModelWithProvider` is `#[serde(flatten)]` on `model: ModelRow`
    // (the flatten is serialization-only; the Rust struct still
    // nests the fields under `model`). Project the (id →
    // display_name) map the UI consumes.
    let model_display: HashMap<String, String> = models
        .into_iter()
        .map(|m| (m.model.id, m.model.display_name))
        .collect();

    // Step 3: per-row priority chain (DB > frontmatter > None).
    // The dispatch path's `resolve_final_model` reads the DB
    // + returns `Some(<model_id>)`; we then look up the display
    // name from the catalog. We re-implement the priority chain
    // here (rather than calling `resolve_final_model` per row)
    // because the IPC layer already has the override map loaded
    // — calling `resolve_final_model` per row would re-query
    // the DB N times. The chain is identical to the dispatch
    // path's (the rule is "DB row wins; else frontmatter; else
    // None"), so the two surfaces stay in lockstep.
    let mut out = Vec::with_capacity(loaded.len());
    for l in loaded {
        let db_override = overrides.get(&l.def.name).cloned();
        let resolved = db_override.clone().or_else(|| l.def.model.clone());
        let resolved_display = resolved
            .as_ref()
            .and_then(|mid| model_display.get(mid))
            .cloned();
        out.push(SubagentWithModelRow {
            name: l.def.name,
            description: l.def.description,
            source: l.source.as_str().to_string(),
            tools: l.def.tools,
            resolved_model_id: resolved,
            resolved_model_display: resolved_display,
            declared_model_id: l.def.model,
            has_db_override: db_override.is_some(),
            writable: !matches!(l.source, SubagentSource::Builtin),
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn list_subagents_with_model(

    project_path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<SubagentWithModelRow>, AppCommandError> {
    list_subagents_with_model_inner(project_path, &state).await
}

// ---------------------------------------------------------------------------
// set_subagent_model — write-side, dispatched by `source`
// ---------------------------------------------------------------------------

/// Set or clear a subagent's model. `model_id = Some(mid)` sets
/// the model; `None` clears (builtin → DB row DELETE;
/// user/project → frontmatter `model:` line removed, restoring
/// the "inherit parent" state on the next dispatch).
///
/// Returns the post-update `SubagentWithModelRow` so the frontend
/// can refresh the row locally (the cost of a follow-up
/// `list_subagents_with_model` IPC is wasted bandwidth; this
/// avoids it).
///
/// **Atomicity**: each branch (DB write OR file write) is a
/// single operation; we don't wrap the two in a transaction
/// (the dispatch path always reads at most one of them per
/// agent, so partial failures are bounded to the agent + the
/// current call — the rest of the table / filesystem is
/// untouched).
pub async fn set_subagent_model_inner(
    name: String,
    source: String,
    project_path: String,
    model_id: Option<String>,
    state: &Arc<AppState>,
) -> Result<SubagentWithModelRow, AppCommandError> {
    // Validate the agent name first (cheap; avoids DB / IO
    // round-trips on obvious garbage).
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AppCommandError::new(
            ErrorCategory::InvalidRequest,
            format!("invalid agent name: '{}'", name),
        ));
    }
    let source_enum = match source.as_str() {
        "builtin" => SubagentSource::Builtin,
        "user" => SubagentSource::User,
        "project" => SubagentSource::Project,
        other => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!(
                    "invalid source: '{}' (expected builtin/user/project)",
                    other
                ),
            ));
        }
    };

    // Dispatch by source. Builtin → DB override; user / project →
    // file. The IPC's source string is the authority; we do NOT
    // re-validate against the cache (a UI race could legitimately
    // pass a slightly stale source if the user changed the file
    // out-of-band; the user's intent is honored).
    match source_enum {
        SubagentSource::Builtin => match &model_id {
            Some(mid) => {
                set_subagent_model_override(&state.db, &name, mid)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("set_subagent_model: write DB override failed: {}", e)
                    })?;
            }
            None => {
                clear_subagent_model_override(&state.db, &name)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("set_subagent_model: clear DB override failed: {}", e)
                    })?;
            }
        },
        SubagentSource::User | SubagentSource::Project => {
            let path = locate_agent_file(source_enum, &name, &project_path).map_err(|e| {
                AppCommandError::new(
                    ErrorCategory::InvalidRequest,
                    format!("set_subagent_model: cannot locate agent file: {}", e),
                )
            })?;
            write_frontmatter_model(&path, model_id.as_deref()).map_err(|e| {
                AppCommandError::new(
                    ErrorCategory::Server,
                    format!(
                        "set_subagent_model: write frontmatter failed for '{}': {}",
                        path.display(),
                        e
                    ),
                )
            })?;
        }
        // Step 2.3 (`07-08-workflow-integration`): plugin
        // agent model is not yet writable from the UI — the
        // plugin layer is currently read-only (mirrors the
        // skills plugin layer's read-only contract).
        // Surfaces as `InvalidRequest` so the frontend's
        // optimistic-update rollback can fire cleanly rather
        // than silently no-op'ing.
        SubagentSource::Plugin => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                "set_subagent_model: plugin agents are read-only; edit the .md under <project>/.everlasting/workflow/<wf>/agents/ directly",
            ));
        }
        // 07-09-workflow-builtin-plugin: 内置 plugin agents 是
        // `include_str!` 编译期常量,无磁盘路径。要覆盖需在项目 plugin
        // 目录放同名 .md(从而走 SubagentSource::Plugin 分支)。
        SubagentSource::BuiltinPlugin => {
            return Err(AppCommandError::new(
                ErrorCategory::InvalidRequest,
                "set_subagent_model: builtin-plugin agents are read-only compile-time constants; override by placing a same-named .md in <project>/.everlasting/workflow/dev/agents/",
            ));
        }
    }

    // Re-read the cache so the returned row reflects the
    // post-write state. The cache's mtime-fence means a freshly
    // written file is picked up on this very call (the fence
    // stat-dirs check sees the new mtime; re-scan is triggered).
    let loaded = state.subagent_cache.list(&project_path).await;
    let after = loaded
        .into_iter()
        .find(|l| l.def.name == name)
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::Server,
                format!(
                    "set_subagent_model: agent '{}' disappeared after write",
                    name
                ),
            )
        })?;

    // Re-resolve the priority chain for the response so the
    // frontend sees the canonical "after" state. The DB row is
    // already written (builtin case) or the file is already
    // updated (user / project case); we re-read both to project
    // the resolved value. `resolve_final_model` does the DB
    // read inline; for the file we use the cache's `def.model`
    // (the mtime-fence has already picked up the write).
    let db_override = get_subagent_model_override(&state.db, &name)
        .await
        .map_err(|e| anyhow::anyhow!("set_subagent_model: re-read override failed: {}", e))?;
    let resolved = db_override.clone().or_else(|| after.def.model.clone());
    let models = db::models::list_models(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("set_subagent_model: list models failed: {}", e))?;
    let resolved_display = resolved
        .as_ref()
        .and_then(|mid| models.iter().find(|m| &m.model.id == mid))
        .map(|m| m.model.display_name.clone());

    Ok(SubagentWithModelRow {
        name: after.def.name,
        description: after.def.description,
        source: after.source.as_str().to_string(),
        tools: after.def.tools,
        resolved_model_id: resolved,
        resolved_model_display: resolved_display,
        declared_model_id: after.def.model,
        has_db_override: db_override.is_some(),
        writable: !matches!(after.source, SubagentSource::Builtin),
    })
}

#[tauri::command]
pub async fn set_subagent_model(

    name: String,
    source: String,
    project_path: String,
    model_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<SubagentWithModelRow, AppCommandError> {
    set_subagent_model_inner(name, source, project_path, model_id, &state).await
}
