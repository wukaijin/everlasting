//! `end_discussion` — group-chat moderator tool (07-29-group-chat).
//!
//! The moderator LLM calls this to terminate the discussion. The
//! agent loop intercepts the tool_use (same pattern as
//! `nominate_speaker`) and sets `discussion_ended = true` in the
//! shared [`GroupChatTurnState`]; the outer `run_group_chat_loop`
//! orchestrator reads it and stops the turn-taking loop.
//!
//! Like `nominate_speaker` this is a SIGNAL tool, not a BLOCKING
//! tool — it returns immediately.

use crate::llm::types::ToolDef;
use crate::tools::nominate_speaker::SharedTurnState;

/// Tool name — MUST match the interception branch in `chat_loop.rs`.
pub const END_DISCUSSION_TOOL_NAME: &str = "end_discussion";

/// Schema. An optional `summary` lets the moderator close with a
/// final remark (persisted as the tool_result, visible in the
/// transcript).
pub fn definition() -> ToolDef {
    ToolDef {
        name: END_DISCUSSION_TOOL_NAME.to_string(),
        description: Some(
            "Group chat only: end the discussion. Call this when the participants have \
             covered the topic sufficiently and no further turns are needed. An optional \
             `summary` captures the moderator's closing remark (shown in the transcript)."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Optional closing remark shown in the transcript."
                }
            }
        }),
    }
}

/// Async execution used by the chat_loop interception handler. Sets
/// the `discussion_ended` flag + returns the summary (or a default)
/// as the `tool_result` content.
pub async fn execute_intercept(
    state: &SharedTurnState,
    input: &serde_json::Value,
) -> (String, bool) {
    let summary = input
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("Discussion ended.");
    let mut st = state.lock().await;
    st.discussion_ended = true;
    drop(st);
    (summary.to_string(), false)
}
