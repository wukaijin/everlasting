//! Task file IO: read / write / create-init.
//!
//! Split out of `agent/workflow/task.rs` (2026-08-08 batch3). Pure sync
//! file IO (no DB, no LLM, no tokio).

use std::fs;
use std::path::Path;

use chrono::Utc;

use super::paths::{task_dir, task_json_path, task_prd_path, validate_slug};
use super::types::{TaskError, TaskJson, TaskResult, TaskStatus};

/// Read and parse `task.json` from disk. Used by Step 0.5's
/// chat_loop per-turn injection. Returns
/// `Err(TaskError::NotFound)` when the file is missing
/// (NOT `Err(Io)` so the caller's "no task yet" branch is
/// unambiguous).
pub fn read_task(project_path: &Path, slug: &str) -> TaskResult<TaskJson> {
    let path = task_json_path(project_path, slug);
    if !path.exists() {
        return Err(TaskError::NotFound(task_dir(project_path, slug)));
    }
    let bytes = fs::read(&path).map_err(|e| TaskError::Io(path.clone(), e))?;
    let task: TaskJson = serde_json::from_slice(&bytes)
        .map_err(|e| TaskError::MalformedJson(path.clone(), e.to_string()))?;
    Ok(task)
}

/// Serialize and atomically write `task.json`. Writes to
/// `<file>.tmp` first then renames onto the final path so
/// a partial write can never be observed by a concurrent
/// reader (Step 0.5's chat_loop + the Phase 2 B12 writer
/// may both touch the file).
pub fn write_task(project_path: &Path, task: &TaskJson) -> TaskResult<()> {
    validate_slug(&task.slug)?;
    let dir = task_dir(project_path, &task.slug);
    fs::create_dir_all(&dir).map_err(|e| TaskError::Io(dir.clone(), e))?;
    let final_path = dir.join("task.json");
    let tmp_path = dir.join("task.json.tmp");
    let bytes = serde_json::to_vec_pretty(task)
        .map_err(|e| TaskError::MalformedJson(final_path.clone(), e.to_string()))?;
    fs::write(&tmp_path, &bytes).map_err(|e| TaskError::Io(tmp_path.clone(), e))?;
    fs::rename(&tmp_path, &final_path).map_err(|e| TaskError::Io(final_path.clone(), e))?;
    Ok(())
}

/// `create_task` body — fills a fresh v1 template, writes
/// the `task.json` + `prd.md` skeleton, returns the
/// resulting `TaskJson`. Called by the Tauri IPC
/// (`commands::task::create_task`).
///
/// **Idempotency**: refuses to overwrite an existing
/// `task.json` (returns `Err(AlreadyExists)`). The frontend
/// is expected to surface "task already exists" + a "open
/// existing" affordance rather than silently overwrite.
///
/// The new task starts at `status = Planning` (matches
/// `default_workflow().initial`); Phase 0 has no
/// pre-flight checks on the `initial` state.
pub fn create_task_init(
    project_path: &Path,
    title: &str,
    slug: &str,
    parent: Option<&str>,
    initial_status: TaskStatus,
    workflow_plugin: &str,
) -> TaskResult<TaskJson> {
    validate_slug(slug)?;
    if title.trim().is_empty() {
        return Err(TaskError::InvalidSlug(
            "title must not be empty".to_string(),
        ));
    }
    let dir = task_dir(project_path, slug);
    let json_path = dir.join("task.json");
    if json_path.exists() {
        return Err(TaskError::AlreadyExists(dir));
    }

    // Compose the v1 template. The `id` is a v4 UUID —
    // uniqueness is global so two projects can't
    // accidentally collide on the same slug (rare but
    // possible when the user opens parallel workspaces).
    let now = Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();
    let task = TaskJson {
        id,
        title: title.trim().to_string(),
        slug: slug.to_string(),
        // C5 (2026-07-27): caller picks the initial status so a
        // review-plugin session seeds `intake` (not dev's
        // `planning`), keeping the task aligned with the active
        // plugin's state machine from creation.
        status: initial_status,
        // C5 (2026-07-28): record the owning plugin so role gate /
        // transition / breadcrumb resolve the state machine via
        // the task, not the (switchable) session plugin.
        workflow_plugin: workflow_plugin.to_string(),
        created_at: now.clone(),
        updated_at: now,
        parent: parent.map(str::to_string),
        summary: String::new(),
        items: Vec::new(),
        // Step 3.3: completed_at is set later by
        // archive_task_init. Always None at creation time.
        completed_at: None,
    };

    write_task(project_path, &task)?;

    // Best-effort `prd.md` skeleton. The user / agent
    // can fully rewrite it; this is just a starting
    // prompt so the file always exists at create time
    // (matches `.everlasting/commands/*/cmd.md` and
    // `.everlasting/skills/*/SKILL.md` skeletons).
    let prd_path = task_prd_path(project_path, slug);
    let prd_body = format!(
        "# {title}\n\
         \n\
         > task slug: `{slug}`\n\
         > created: {now}\n\
         > spec: pull from `WorkflowCtx` once Step 0.5 wires it.\n\
         \n\
         ## Goal\n\
         \n\
         <fill>\n\
         \n\
         ## Acceptance criteria\n\
         \n\
         - [ ] <fill>\n",
        title = task.title,
        slug = task.slug,
        now = task.created_at,
    );
    fs::write(&prd_path, prd_body.as_bytes()).map_err(|e| TaskError::Io(prd_path, e))?;

    Ok(task)
}
