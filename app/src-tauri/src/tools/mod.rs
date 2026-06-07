//! Tool definitions and execution for the agent.
//!
//! Step 2 defines 3 built-in tools: `read_file`, `write_file`, `shell`.
//! Each tool has a `definition()` (for the LLM request) and an `execute()`
//! (for the agent runtime).
//!
//! Step 3b-1 introduces [`ToolContext`]: a per-turn struct injected
//! into every tool call that carries the active project's root and
//! the session's current working directory. The boundary check
//! (`projects::boundary::assert_within_root`) is the single source of
//! truth for "is this path inside the project?" — see
//! `.trellis/spec/backend/project-cwd-boundary.md` for the contract.
//!
//! Step toolset-extension adds 4 more tools: `edit_file`, `grep`,
//! `glob`, `list_dir`. `edit_file` requires a `ReadGuard` (Tauri
//! State) and a session id; the other 3 are pure functions like the
//! step 2 tools.

pub mod edit_file;
pub mod glob;
pub mod grep;
pub mod list_dir;
pub mod read_file;
pub mod read_guard;
pub mod shell;
pub mod write_file;

use std::path::PathBuf;

use crate::llm::types::ToolDef;
use crate::tools::read_guard::ReadGuard;

/// All built-in tools available as of step 2 + the toolset extension.
pub fn builtin_tools() -> Vec<ToolDef> {
    vec![
        read_file::definition(),
        write_file::definition(),
        edit_file::definition(),
        shell::definition(),
        grep::definition(),
        glob::definition(),
        list_dir::definition(),
    ]
}

/// Per-turn context passed to every tool execution. Built once per
/// agent turn (in `lib.rs::chat`) from the active project / session
/// state, then handed (immutably) to each tool call.
///
/// - `project_root`: canonical absolute path of the active project
///   (resolved via `boundary::assert_within_root` at turn start).
/// - `cwd`: canonical absolute path of the session's current working
///   directory. This is the cwd the agent "lives in" between shell
///   tool calls; LLM-supplied `working_directory` overrides are
///   validated against `project_root` and, if accepted, returned
///   through the [`ToolContextUpdate`] the shell tool can emit.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub project_root: PathBuf,
    pub cwd: PathBuf,
}

/// Optional per-tool update to the tool context. The shell tool uses
/// this to report a new `cwd` (e.g. after a successful `cd`); the
/// agent loop tracks the latest one and writes the final value to
/// `sessions.current_cwd` once at the end of the turn (see
/// `docs/PROPOSAL-project-binding-and-top-tabs.md` §4.4 "turn 结束
/// 一次性写").
#[derive(Debug, Clone, Default)]
pub struct ToolContextUpdate {
    pub new_cwd: Option<PathBuf>,
}

/// Execute a tool by name. Returns `(content_string, is_error, ctx_update)`.
///
/// `ctx` is injected (not held) — see `PROPOSAL §4.4` / GLM review
/// §1.2. Tools are pure functions, testable in isolation.
///
/// The `edit_file` and `read_file` tools additionally take a
/// `ReadGuard` and `session_id`; the dispatch here routes the right
/// combination of arguments. The guard is a Tauri-managed `State`
/// cloned in by `lib.rs::chat` so the dispatch signature stays
/// uniform for tools that don't need it.
pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: Option<&ReadGuard>,
    session_id: Option<&str>,
) -> (String, bool, ToolContextUpdate) {
    match name {
        "read_file" => {
            let (out, is_err) = read_file::execute(input, ctx, guard, session_id).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "write_file" => {
            let (out, is_err) = write_file::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "edit_file" => match (guard, session_id) {
            (Some(g), Some(sid)) => {
                let (out, is_err) = edit_file::execute(input, ctx, g, sid).await;
                (out, is_err, ToolContextUpdate::default())
            }
            _ => (
                "edit_file called without a ReadGuard / session_id; this is a bug."
                    .to_string(),
                true,
                ToolContextUpdate::default(),
            ),
        },
        "shell" => {
            let (out, is_err, update) = shell::execute(input, ctx, session_id).await;
            (out, is_err, update)
        }
        "grep" => {
            let (out, is_err) = grep::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "glob" => {
            let (out, is_err) = glob::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "list_dir" => {
            let (out, is_err) = list_dir::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        _ => (
            format!("Unknown tool: {}", name),
            true,
            ToolContextUpdate::default(),
        ),
    }
}
