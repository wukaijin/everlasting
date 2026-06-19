//! Per-session system prompt construction.
//!
//! Step 4 follow-up Bug 3: the request body's `system` field is
//! built once per chat invocation and describes the session's
//! project, working directory, and worktree state so the model is
//! grounded on every turn. Pre-fix, the system field was
//! hard-coded to `None` and the only worktree signal the model had
//! was the post-hoc `[worktree event]` user-role message in
//! history.

/// Step 4 follow-up Bug 3: read the HEAD commit SHA of a git
/// working directory and return the first 7 characters (the
/// classic git short-SHA). Returns a placeholder string when the
/// path is not a git repo, libgit2 fails to open it, or the repo
/// has no commits yet (e.g. a freshly-`git init`'d empty repo).
///
/// Best-effort by design: this is consumed only by
/// `build_system_prompt` as a hint to the LLM about the current
/// HEAD; we never want a transient git error to surface as a
/// chat failure.
pub fn lookup_head_sha(path: &std::path::Path) -> String {
    if !path.join(".git").exists() {
        return "not a git repo".to_string();
    }
    let repo = match git2::Repository::open(path) {
        Ok(r) => r,
        Err(_) => return "not a git repo".to_string(),
    };
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return "no commits yet".to_string(),
    };
    let commit = match head.peel_to_commit() {
        Ok(c) => c,
        Err(_) => return "no commits yet".to_string(),
    };
    let full = commit.id().to_string();
    // Classic git short-SHA: first 7 chars.
    full.chars().take(7).collect()
}

/// Step 4 follow-up Bug 3: construct the per-session system
/// prompt the LLM sees at the top of every chat request. The
/// prompt describes the session's project, working directory, and
/// worktree state so the model is grounded on every turn.
///
/// Three worktree-state phrasings:
/// - `Active` → "ACTIVE on branch 'session/<id>' (HEAD <short_sha>)"
/// - `Detached` → "DETACHED — was on branch 'session/<id>'
///   (HEAD <short_sha>), currently in project root"
/// - `None` → "NONE — running in project root"
///
/// **Privacy**: only the `session_id`, `project.name`,
/// `project.path`, `ctx_root`, and short HEAD SHA are emitted. No
/// user messages or tool inputs are echoed.
pub fn build_system_prompt(
    session: &crate::db::SessionRow,
    project: &crate::projects::ProjectRow,
    ctx_root: &std::path::Path,
    head_sha: &str,
) -> String {
    let branch = crate::git::worktree::branch_name(&session.id);
    let worktree_line = if !project.is_git_repo {
        "N/A — non-git project".to_string()
    } else {
        match session.worktree_state {
            crate::db::WorktreeState::Active => {
                format!("ACTIVE on branch '{}' (HEAD {})", branch, head_sha)
            }
            crate::db::WorktreeState::Detached => format!(
                "DETACHED — was on branch '{}' (HEAD {}), currently in project root",
                branch, head_sha
            ),
            crate::db::WorktreeState::None => "NONE — running in project root".to_string(),
        }
    };

    format!(
        "You are a coding agent. You have access to the tools defined in this \
request. All file paths in tool inputs are relative to the session's \
working directory.\n\
\n\
Session context:\n\
- Session ID: {session_id}\n\
- Project: {project_name} ({project_path})\n\
- Working directory: {cwd}\n\
- Worktree: {worktree_line}\n\
- Available tool result envelope: {{\"result\": \"<content>\", \"cwd\": \"<worktree_path>\"}} \
— `cwd` tells you which root the tool ran against when worktree transitions happen mid-session.",
        session_id = session.id,
        project_name = project.name,
        project_path = project.path,
        cwd = ctx_root.display(),
        worktree_line = worktree_line,
    )
}

/// Assemble the full system prompt from its three layers, in
/// cache-stability order: the stable behavior guidance first, then
/// the mode prefix, then the per-turn base prompt. Stablest layer
/// first keeps the upstream prompt-cache prefix warm across turns.
/// See the [`behavior_prompt`] module for the layering rationale.
pub fn assemble_system_prompt(mode_prefix: &str, base_prompt: &str) -> String {
    format!(
        "{}\n\n{}\n\n{}",
        crate::agent::behavior_prompt::DEFAULT_BEHAVIOR_PROMPT,
        mode_prefix,
        base_prompt,
    )
}