//! Git errors surfaced to Tauri commands. The `String` conversion
//! in `#[tauri::command]` handlers will turn these into user-facing
//! error messages — keep them concise and actionable.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    /// The project's path is not a git repository (no `.git/`
    /// directory or `.git` file at the root). Step 4 requires all
    /// session-creating projects to be git repos; this error is
    /// surfaced when the user tries to create a session against a
    /// non-git project.
    #[error("project is not a git repository: {path}")]
    NotARepo { path: String },

    /// The worktree path's parent directory could not be created or
    /// the worktree itself could not be removed during destroy.
    /// Usually a permission problem; the user can fix it by
    /// clearing the parent dir.
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// libgit2 reported an error. The `git2::Error` carries its own
    /// message and class; we preserve it verbatim so the user can
    /// google / git-log the cause.
    #[error("git2 error: {0}")]
    Git2(#[from] git2::Error),

    /// The target working directory has uncommitted or untracked
    /// changes. Surfaced by `check_clean` and propagated through
    /// helpers that refuse to attach a worktree onto a dirty base
    /// (the new worktree would diverge from the user's WIP).
    /// The `paths` list carries up to 10 offending paths for
    /// an actionable user-facing message.
    #[error("working tree at {path} has uncommitted changes{}", paths_formatted(.paths))]
    Dirty { path: String, paths: Vec<String> },
}

fn paths_formatted(paths: &[String]) -> String {
    if paths.is_empty() {
        String::new()
    } else {
        format!(": {}", paths.join(", "))
    }
}
