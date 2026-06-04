//! `read_file` tool — read a file's contents.

use crate::llm::types::ToolDef;

/// Max output before truncation (matches ARCHITECTURE.md §2.5.3).
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "read_file".to_string(),
        description: Some(
            "Read the contents of a file at the given path. Returns the file content as a string."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to read."
                }
            },
            "required": ["path"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value) -> (String, bool) {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true),
    };

    match tokio::fs::read_to_string(path).await {
        Ok(content) => (truncate_output(content), false),
        Err(e) => (format!("Failed to read file '{}': {}", path, e), true),
    }
}

/// Truncate output exceeding MAX_OUTPUT_BYTES (head + tail, omit middle).
fn truncate_output(content: String) -> String {
    if content.len() <= MAX_OUTPUT_BYTES {
        return content;
    }
    let head_end = 25 * 1024;
    let tail_start = content.len() - 25 * 1024;
    let omitted = content.len() - MAX_OUTPUT_BYTES;
    format!(
        "{}\n<truncated: omitted {} bytes>\n{}",
        &content[..head_end],
        omitted,
        &content[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "read_file");
    }

    #[test]
    fn definition_schema_requires_path() {
        let schema = &definition().input_schema;
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("path")));
    }

    #[tokio::test]
    async fn execute_reads_real_file() {
        let input = serde_json::json!({"path": "/etc/hostname"});
        let (content, is_error) = execute(&input).await;
        assert!(!is_error);
        assert!(!content.is_empty());
    }

    #[tokio::test]
    async fn execute_missing_path_param() {
        let input = serde_json::json!({});
        let (content, is_error) = execute(&input).await;
        assert!(is_error);
        assert!(content.contains("Missing"));
    }

    #[tokio::test]
    async fn execute_nonexistent_file() {
        let input = serde_json::json!({"path": "/nonexistent/file/xyz.txt"});
        let (content, is_error) = execute(&input).await;
        assert!(is_error);
        assert!(content.contains("Failed to read"));
    }
}
