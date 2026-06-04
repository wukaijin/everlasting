//! `write_file` tool — write content to a file.

use crate::llm::types::ToolDef;

pub fn definition() -> ToolDef {
    ToolDef {
        name: "write_file".to_string(),
        description: Some(
            "Write content to a file. Creates the file (and parent directories) if they do not exist, overwrites if they do."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file."
                }
            },
            "required": ["path", "content"]
        }),
    }
}

/// Execute the tool. Returns `(content, is_error)`.
pub async fn execute(input: &serde_json::Value) -> (String, bool) {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return ("Missing required parameter: path".to_string(), true),
    };
    let content = match input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return ("Missing required parameter: content".to_string(), true),
    };

    // Create parent directories if needed.
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return (format!("Failed to create parent directories: {}", e), true);
            }
        }
    }

    match tokio::fs::write(path, content).await {
        Ok(()) => (
            format!("Successfully wrote {} bytes to {}", content.len(), path),
            false,
        ),
        Err(e) => (format!("Failed to write file '{}': {}", path, e), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "write_file");
    }

    #[tokio::test]
    async fn execute_writes_and_reads_back() {
        let dir = std::env::temp_dir().join("everlasting_test_write");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test.txt");
        let path_str = path.to_str().unwrap();

        let input = serde_json::json!({"path": path_str, "content": "hello world"});
        let (msg, is_error) = execute(&input).await;
        assert!(!is_error);
        assert!(msg.contains("Successfully wrote"));

        // Read back to verify.
        let content = tokio::fs::read_to_string(path_str).await.unwrap();
        assert_eq!(content, "hello world");

        // Cleanup.
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn execute_missing_content_param() {
        let input = serde_json::json!({"path": "/tmp/x.txt"});
        let (msg, is_error) = execute(&input).await;
        assert!(is_error);
        assert!(msg.contains("Missing required parameter: content"));
    }
}
