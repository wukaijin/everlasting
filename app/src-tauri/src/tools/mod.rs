//! Tool definitions and execution for the agent.
//!
//! Step 2 defines 3 built-in tools: `read_file`, `write_file`, `shell`.
//! Each tool has a `definition()` (for the LLM request) and an `execute()`
//! (for the agent runtime).

pub mod read_file;
pub mod shell;
pub mod write_file;

use crate::llm::types::ToolDef;

/// All built-in tools available in step 2.
pub fn builtin_tools() -> Vec<ToolDef> {
    vec![
        read_file::definition(),
        write_file::definition(),
        shell::definition(),
    ]
}

/// Execute a tool by name. Returns `(content_string, is_error)`.
pub async fn execute_tool(name: &str, input: &serde_json::Value) -> (String, bool) {
    match name {
        "read_file" => read_file::execute(input).await,
        "write_file" => write_file::execute(input).await,
        "shell" => shell::execute(input).await,
        _ => (format!("Unknown tool: {}", name), true),
    }
}
