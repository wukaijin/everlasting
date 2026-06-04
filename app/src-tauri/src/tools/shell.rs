//! `shell` tool — execute a shell command.

use crate::llm::types::ToolDef;

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
const MAX_OUTPUT_BYTES: usize = 50 * 1024;
/// Command timeout in seconds (matches ARCHITECTURE.md §2.5.2).
const TIMEOUT_SECS: u64 = 300;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "shell".to_string(),
        description: Some(
            "Execute a shell command and return its stdout and stderr. Runs via `sh -c`."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                }
            },
            "required": ["command"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value) -> (String, bool) {
    let command = match input.get("command").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return ("Missing required parameter: command".to_string(), true),
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(TIMEOUT_SECS),
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr]\n");
                result.push_str(&stderr);
            }

            let exit_code = output.status.code().unwrap_or(-1);
            if !result.is_empty() {
                result.push_str(&format!("\n[exit code: {}]", exit_code));
            } else {
                result = format!("[exit code: {}]", exit_code);
            }

            let is_error = !output.status.success();
            (truncate_output(result), is_error)
        }
        Ok(Err(e)) => (format!("Failed to execute command: {}", e), true),
        Err(_) => (
            format!("Command timed out after {} seconds", TIMEOUT_SECS),
            true,
        ),
    }
}

/// Truncate output exceeding MAX_OUTPUT_BYTES (head + tail, omit middle).
fn truncate_output(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    let head_end = 25 * 1024;
    let tail_start = s.len() - 25 * 1024;
    let omitted = s.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &s[..head_end],
        omitted,
        &s[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "shell");
    }

    #[tokio::test]
    async fn execute_echo() {
        let input = serde_json::json!({"command": "echo hello"});
        let (content, is_error) = execute(&input).await;
        assert!(!is_error);
        assert!(content.contains("hello"));
        assert!(content.contains("[exit code: 0]"));
    }

    #[tokio::test]
    async fn execute_stderr_command() {
        // `false` is a shell builtin that exits with code 1.
        let input = serde_json::json!({"command": "echo error >&2 && false"});
        let (content, is_error) = execute(&input).await;
        assert!(is_error); // non-zero exit
        assert!(content.contains("error"));
    }

    #[tokio::test]
    async fn execute_missing_command_param() {
        let input = serde_json::json!({});
        let (msg, is_error) = execute(&input).await;
        assert!(is_error);
        assert!(msg.contains("Missing required parameter"));
    }
}
