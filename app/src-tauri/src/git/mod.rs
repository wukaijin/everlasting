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
//! - [`checkpoint`]: turn-boundary file snapshots for the N2
//!   revert loop — dangling snapshot commits + umbrella refs
//!   under `refs/everlasting/<session_id>`, zero-touch on the
//!   user's branches / index / workdir (restore is explicit).
//!
//! See `worktree.rs` for the implementation and
//! `docs/ARCHITECTURE.md §3` for the design rationale. The
//! `git-backend.md` research file under
//! `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/research/`
//! justifies the libgit2 + spawn hybrid.

pub mod checkpoint;
pub mod diff;
pub mod error;
pub mod tests_worktree;
pub mod worktree;

pub use worktree::{check_clean, destroy as destroy_worktree};
