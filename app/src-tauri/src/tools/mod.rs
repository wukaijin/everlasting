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

pub mod read_file;
pub mod shell;
pub mod write_file;

use std::path::PathBuf;

use crate::llm::types::ToolDef;

/// All built-in tools available in step 2.
pub fn builtin_tools() -> Vec<ToolDef> {
    vec![
        read_file::definition(),
        write_file::definition(),
        shell::definition(),
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
pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool, ToolContextUpdate) {
    match name {
        "read_file" => {
            let (out, is_err) = read_file::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "write_file" => {
            let (out, is_err) = write_file::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default())
        }
        "shell" => shell::execute(input, ctx).await,
        _ => (
            format!("Unknown tool: {}", name),
            true,
            ToolContextUpdate::default(),
        ),
    }
}
