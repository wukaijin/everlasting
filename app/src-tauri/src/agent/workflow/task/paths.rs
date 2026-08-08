//! Slug validation + task path-layout helpers.
//!
//! Split out of `agent/workflow/task.rs` (2026-08-08 batch3). Pure
//! path construction, no IO.

use std::path::{Path, PathBuf};

use super::types::{TaskError, TaskResult};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Project namespace shared across all four subsystems
/// (commands / agents / skills / outputs). Same constant
/// used by every `.everlasting/*` loader — kept here only
/// as a convenience re-export so callers building task
/// paths don't need to import the `PROJ_NS` from three
/// different modules. The single source of truth lives in
/// the agent directories (e.g. `agent::subagent::loader`
/// `PROJ_NS = ".everlasting"`); when those constants
/// change, this one follows.
pub(crate) const PROJ_NS_TASKS_DIR: &str = ".everlasting/tasks";

/// Maximum slug length. ASCII `[a-z0-9-]` only — any other
/// char (spaces / accents / emoji) is rejected. Generous
/// bound to leave headroom for "session-2026-07-08-abc".
const MAX_SLUG_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Slug validation
// ---------------------------------------------------------------------------

/// Strict slug validator — `^[a-z0-9-]{1,64}$` ASCII,
/// non-empty, no leading / trailing hyphen. The hyphen
/// guard stops `--foo` / `foo--` confusion in shell
/// auto-complete; the lowercase guard stops
/// `My-Feature` vs `my-feature` directory confusion on
/// case-insensitive filesystems.
pub fn validate_slug(slug: &str) -> TaskResult<()> {
    if slug.is_empty() || slug.len() > MAX_SLUG_LEN {
        return Err(TaskError::InvalidSlug(slug.to_string()));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(TaskError::InvalidSlug(slug.to_string()));
    }
    for c in slug.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(TaskError::InvalidSlug(slug.to_string()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Path layout
// ---------------------------------------------------------------------------

/// `<project>/.everlasting/tasks/<slug>/` — does NOT create
/// directories; use [`super::write_task`] for the create-on-write
/// path or call `fs::create_dir_all` yourself.
pub fn task_dir(project_path: &Path, slug: &str) -> PathBuf {
    project_path.join(PROJ_NS_TASKS_DIR).join(slug)
}

/// `<project>/.everlasting/tasks/<slug>/task.json`.
pub fn task_json_path(project_path: &Path, slug: &str) -> PathBuf {
    task_dir(project_path, slug).join("task.json")
}

/// `<project>/.everlasting/tasks/<slug>/prd.md`.
pub fn task_prd_path(project_path: &Path, slug: &str) -> PathBuf {
    task_dir(project_path, slug).join("prd.md")
}
