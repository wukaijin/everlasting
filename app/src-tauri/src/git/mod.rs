//! Git worktree lifecycle for step 4 of the roadmap. Public API:
//!
//! - [`create_worktree`]: create a session worktree on a new
//!   `session/<id>` branch off the project's HEAD.
//! - [`destroy_worktree`]: remove the worktree directory + delete
//!   the session branch.
//! - [`check_clean`]: assert a git working dir has no uncommitted
//!   changes (used by `attach_worktree` and `detach_worktree` to
//!   refuse the destructive operation when there are uncommitted
//!   edits).
//! - [`session_worktree_path`]: canonical on-disk path for a
//!   session worktree.
//! - [`diff::diff_worktree`]: compute the per-file diff between
//!   the session's worktree and the commit the session branch
//!   was created from.
//!
//! See `worktree.rs` for the implementation and
//! `docs/ARCHITECTURE.md §3` for the design rationale. The
//! `git-backend.md` research file under
//! `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/research/`
//! justifies the libgit2 + spawn hybrid.

pub mod diff;
pub mod error;
pub mod worktree;

pub use worktree::{
    check_clean, create as create_worktree, destroy as destroy_worktree,
    worktree_path as session_worktree_path,
};
