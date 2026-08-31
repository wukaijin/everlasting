//! `create_task` tool — LLM-initiated workflow task creation.
//!
//! 07-10-workflow-task-json-hardening R2 (2026-07-10).
//!
//! The model calls `create_task(title, slug, parent?)` to seed a
//! workflow task. Reuses [`crate::agent::workflow::create_task_init`]
//! (the same writer the `create_task` Tauri IPC in `commands::task`
//! uses), so the on-disk `task.json` + `prd.md` skeleton is
//! schema-correct by construction.
//!
//! # Positioning (NOT the only path)
//!
//! This tool is a **convenience helper**, not a gate. The lenient
//! `read_task` (R1) already tolerates a hand-written task.json, so
//! the LLM MAY also `write_file` one directly — but `create_task`
//! is the lower-effort path (one call vs. hand-rolling JSON) and
//! fills the full template (valid id, created_at/updated_at, prd.md
//! skeleton) that a hand-write tends to drift on. Resilience lives
//! on the read side (R1), not by gating writes.
//!
//! # Visibility
//!
//! Only shown in `workflow_enabled` sessions — see
//! [`crate::tools::filter_tools_for_workflow`]. Non-workflow sessions
//! never see the schema (filter_tools_for_workflow strips it), so the
//! model can't call it where a task has no meaning.
//!
//! # Permission
//!
//! `Risk::Low` (silent Allow). The write lands under
//! `.everlasting/tasks/<slug>/` inside the project root — same
//! boundary `update_checklist`'s persistence path uses. No Tier 4 ask.

use crate::agent::workflow::{create_task_init, load_workflow, TaskError, TaskStatus};
use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// The `create_task` tool definition registered in `builtin_tools()`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "create_task".to_string(),
        description: Some(
            "Create a new workflow task under `.everlasting/tasks/<slug>/`, seeding a \
             schema-correct `task.json` (status=planning) + `prd.md` skeleton. This is \
             the lower-effort path to start a task — prefer it over hand-writing \
             task.json via write_file. (write_file also works because read_task parses \
             leniently, but create_task fills the full template — valid id, timestamps, \
             prd.md — in one call, so it's less error-prone.)\n\n\
             Pass a kebab-case `slug` ([a-z0-9-]{1,64}) and a human `title`. Optional \
             `parent` slug for sub-tasks. Refuses to overwrite an existing task with \
             the same slug.\n\n\
             Only available in workflow sessions."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Human-readable task title (non-empty)."
                },
                "slug": {
                    "type": "string",
                    "description": "Kebab-case task id matching /[a-z0-9-]{1,64}/. Becomes the `.everlasting/tasks/<slug>/` directory name."
                },
                "parent": {
                    "type": "string",
                    "description": "Optional slug of the parent task (for sub-task DAG)."
                }
            },
            "required": ["title", "slug"]
        }),
    }
}

/// Execute `create_task`: parse `title`/`slug`/`parent` →
/// [`create_task_init`] against the session's project root
/// (`ctx.worktree_path`, same path `update_checklist`'s persistence
/// uses) → return a short summary of the fresh task.
///
/// Errors (`InvalidSlug` / `AlreadyExists` / IO) map to
/// `is_error: true` with an actionable message; the in-memory state
/// is untouched on failure.
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let title = match input.get("title").and_then(|v| v.as_str()) {
        Some(t) => t.trim().to_string(),
        None => return ("create_task requires a `title` string".to_string(), true),
    };
    let slug = match input.get("slug").and_then(|v| v.as_str()) {
        Some(s) => s.trim().to_string(),
        None => return ("create_task requires a `slug` string".to_string(), true),
    };
    let parent = input
        .get("parent")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // C5 (2026-07-27): resolve the active plugin's initial state so
    // the seeded task is aligned with that plugin's state machine from
    // creation. A review-plugin session seeds `intake` (not dev's
    // `planning`), so the role gate / breadcrumb / transitions all
    // resolve against the right plugin immediately — no manual
    // task.json status rewrite (the session-04c62fab workaround).
    //
    // C5 (2026-07-28): the same plugin name is also recorded as the
    // task's `workflow_plugin` so role gate / transition keep using
    // the owning plugin's state machine even after the session
    // switches plugins mid-task.
    let project_path_str = ctx.worktree_path.to_string_lossy();
    let plugin_name = ctx
        .workflow_name
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or("dev");
    let initial_status = {
        let initial = load_workflow(plugin_name, &project_path_str).initial;
        TaskStatus::from_str_opt(&initial)
    };

    match create_task_init(
        &ctx.worktree_path,
        &title,
        &slug,
        parent.as_deref(),
        initial_status.clone(),
        plugin_name,
    ) {
        Ok(task) => {
            tracing::info!(slug = %task.slug, "create_task: seeded workflow task");
            (
                format!(
                    "Task created (status: {}):\n\
                     slug: {}\n\
                     title: {}\n\
                     task_dir: .everlasting/tasks/{}/\n\
                     Next: fill prd.md, then call request_task_state_transition when ready.",
                    task.status.as_str(),
                    task.slug,
                    task.title,
                    task.slug
                ),
                false,
            )
        }
        Err(e) => {
            tracing::warn!(slug = %slug, error = %e, "create_task: rejected");
            (
                format!("create_task rejected: {}", map_task_error_msg(&e)),
                true,
            )
        }
    }
}

/// Map [`TaskError`] to a plain user/LLM-facing message. Mirrors
/// `commands::task::map_task_error`'s vocabulary but returns a
/// `String` (the tool layer surfaces `is_error` text, not
/// `AppCommandError` categories).
fn map_task_error_msg(e: &TaskError) -> String {
    match e {
        TaskError::InvalidSlug(msg) => format!("invalid slug/title: {}", msg),
        TaskError::AlreadyExists(path) => format!(
            "task already exists at {} — open the existing one instead of recreating",
            path.display()
        ),
        TaskError::NotFound(path) => format!("task not found at {}", path.display()),
        TaskError::AlreadyArchived(path) => format!("already archived at {}", path.display()),
        TaskError::NotInDoneStatus(s) => format!("not in done (current status: {})", s),
        TaskError::MalformedJson(path, msg) => {
            format!("malformed task.json at {}: {}", path.display(), msg)
        }
        TaskError::Io(path, src) => format!("io error at {}: {}", path.display(), src),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::{read_task, TaskStatus};
    use crate::tools::ToolContext;
    use std::path::PathBuf;

    fn ctx_at(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            escalation: Default::default(),
            worktree_path: tmp.path().to_path_buf(),
            cwd: tmp.path().to_path_buf(),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "test".to_string(),
            data_dir: PathBuf::from("/tmp"),
            workflow_name: Some("dev".to_string()),
            mode: crate::db::Mode::Edit,
        }
    }

    /// C5: a review-plugin ToolContext — mirrors `ctx_at` but with
    /// `workflow_name = "review"` so `execute` resolves the plugin's
    /// initial state (`intake`) instead of dev's `planning`.
    fn ctx_at_review(tmp: &tempfile::TempDir) -> ToolContext {
        let mut ctx = ctx_at(tmp);
        ctx.workflow_name = Some("review".to_string());
        ctx
    }

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "create_task");
    }

    #[tokio::test]
    async fn execute_seeds_schema_correct_task_json() {
        // The whole point of R2: create_task produces a task.json
        // that read_task parses cleanly (unlike a hand-write that
        // may drift on id/created_at/updated_at).
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let input = serde_json::json!({"title": "My Feature", "slug": "my-feature"});
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(!is_err, "{}", out);
        assert!(out.contains("my-feature"));
        let task = read_task(tmp.path(), "my-feature").expect("read_task parses the seeded file");
        assert_eq!(task.title, "My Feature");
        assert_eq!(task.status, TaskStatus::Planning);
        assert!(
            !task.created_at.is_empty(),
            "create_task_init fills created_at"
        );
        // prd.md skeleton exists (write_file hand-write wouldn't make it).
        assert!(tmp
            .path()
            .join(".everlasting/tasks/my-feature/prd.md")
            .exists());
    }

    /// C5 (2026-07-27): a review-plugin session must seed the task
    /// in `intake` (review's initial state), NOT dev's `planning`.
    /// Otherwise the role gate reads `planning`, finds no matching
    /// entry in review's `roles_by_state`, and denies every dispatch
    /// — the session-04c62fab dead-lock.
    #[tokio::test]
    async fn execute_review_plugin_seeds_intake_status() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at_review(&tmp);
        let input = serde_json::json!({"title": "Review Me", "slug": "review-me"});
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(!is_err, "{}", out);
        // The success message reports the resolved status.
        assert!(
            out.contains("status: intake"),
            "review-plugin create_task must report intake status; got: {}",
            out
        );
        let task = read_task(tmp.path(), "review-me").expect("read_task parses the seeded file");
        assert_eq!(
            task.status,
            crate::agent::workflow::TaskStatus::Custom("intake".to_string()),
            "review-plugin task must be in intake (Custom(\"intake\")), got {:?}",
            task.status
        );
    }

    #[tokio::test]
    async fn execute_rejects_duplicate_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let input = serde_json::json!({"title": "First", "slug": "dup"});
        let (_out, first_err) = execute(&input, &ctx).await;
        assert!(!first_err);
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(is_err, "duplicate slug rejected");
        assert!(out.contains("already exists"), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_bad_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let input = serde_json::json!({"title": "Bad", "slug": "UPPER"});
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(is_err);
        assert!(out.contains("invalid slug"), "{}", out);
    }

    #[tokio::test]
    async fn execute_missing_title_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let input = serde_json::json!({"slug": "x"});
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(is_err);
        assert!(out.contains("title"), "{}", out);
    }

    #[tokio::test]
    async fn execute_optional_parent_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let input =
            serde_json::json!({"title": "Sub", "slug": "sub-task", "parent": "parent-task"});
        let (out, is_err) = execute(&input, &ctx).await;
        assert!(!is_err, "{}", out);
        let task = read_task(tmp.path(), "sub-task").expect("read");
        assert_eq!(task.parent.as_deref(), Some("parent-task"));
    }
}
