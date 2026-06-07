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

    /// A worktree already exists at the target path. This is a
    /// session-id collision (UUIDs are 128-bit, so this should
    /// never happen in practice) OR a stale leftover from a crashed
    /// previous run.
    #[error("worktree already exists at {path}")]
    WorktreeExists { path: String },

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
}
