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
//!
//! C2 证据链(2026-09-09,任务 09-09-gc-c2-evidence-summary):除
//! `summary` 叙事外,工具增可选结构化参数 `conclusions`(claim +
//! file:line 锚点 + stance 实证/推测/争议)与 `open_questions`;
//! 编排器落库前对锚点做后校验后持久化为 `sessions.discussion_detail`。
//! 全参数可选 = 只发 `summary` 的旧行为零变更。

use crate::agent::discussion_detail::{self, DiscussionDetail};
use crate::llm::types::ToolDef;
use crate::tools::nominate_speaker::SharedTurnState;

/// Tool name — MUST match the interception branch in `chat_loop.rs`.
pub const END_DISCUSSION_TOOL_NAME: &str = "end_discussion";

/// Schema. An optional `summary` lets the moderator close with a
/// final remark (persisted as the tool_result, visible in the
/// transcript). Optional structured `conclusions` / `open_questions`
/// (C2 证据链) carry per-claim evidence anchors + stance for the
/// first-class `discussion_detail` column.
pub fn definition() -> ToolDef {
    ToolDef {
        name: END_DISCUSSION_TOOL_NAME.to_string(),
        description: Some(
            "Group chat only: end the discussion. Call this when the participants have \
             covered the topic sufficiently and no further turns are needed. An optional \
             `summary` captures the moderator's closing remark (shown in the transcript). \
             Prefer also passing structured `conclusions` (each: one-sentence claim, \
             `anchors` = [{path, line}] evidence you actually read, `stance` = \
             verified|inferred|disputed) and `open_questions` (unresolved items) — \
             they become the machine-readable consensus record."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Optional closing remark shown in the transcript."
                },
                "conclusions": {
                    "type": "array",
                    "description": "Structured consensus: one entry per conclusion.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "claim": {
                                "type": "string",
                                "description": "One-sentence conclusion."
                            },
                            "anchors": {
                                "type": "array",
                                "description": "Evidence references you actually read (project-relative path + optional 1-based line).",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "path": {"type": "string"},
                                        "line": {"type": "integer"}
                                    },
                                    "required": ["path"]
                                }
                            },
                            "stance": {
                                "type": "string",
                                "enum": ["verified", "inferred", "disputed"],
                                "description": "verified = you or a participant read the evidence; inferred = reasoning without direct evidence; disputed = no consensus."
                            }
                        },
                        "required": ["claim"]
                    }
                },
                "open_questions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Unresolved questions left open by the discussion."
                }
            }
        }),
    }
}

/// Async execution used by the chat_loop interception handler. Sets
/// the `discussion_ended` flag + returns the summary (or a default)
/// as the `tool_result` content.
///
/// GC7 (2026-09-05, BUGLIST-group-chat): the summary is ALSO captured
/// into `end_summary` so the orchestrator persists it to the session
/// row's first-class `discussion_summary` column at loop exit.
///
/// C2 (2026-09-09): the structured `conclusions` / `open_questions`
/// subtree is parsed tolerantly into `end_detail`; any parse failure
/// degrades to「无结构化产物」and never fails the closing turn.
pub async fn execute_intercept(
    state: &SharedTurnState,
    input: &serde_json::Value,
) -> (String, bool) {
    let summary = input
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("Discussion ended.");
    let end_detail: Option<DiscussionDetail> = discussion_detail::parse_from_input(input);
    let mut st = state.lock().await;
    st.discussion_ended = true;
    st.end_summary = Some(summary.to_string());
    st.end_detail = end_detail;
    drop(st);
    (summary.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn capture(
        input: &serde_json::Value,
    ) -> (String, crate::tools::nominate_speaker::GroupChatTurnState) {
        let state: SharedTurnState = Default::default();
        let (text, _) = execute_intercept(&state, input).await;
        let st = state.lock().await;
        (
            text,
            crate::tools::nominate_speaker::GroupChatTurnState {
                next_speaker: st.next_speaker.clone(),
                discussion_ended: st.discussion_ended,
                end_summary: st.end_summary.clone(),
                end_detail: st.end_detail.clone(),
            },
        )
    }

    #[tokio::test]
    async fn legacy_summary_only_input_keeps_detail_none() {
        let (text, st) = capture(&json!({"summary": "结论叙事"})).await;
        assert_eq!(text, "结论叙事");
        assert!(st.discussion_ended);
        assert_eq!(st.end_summary.as_deref(), Some("结论叙事"));
        assert!(st.end_detail.is_none());
    }

    #[tokio::test]
    async fn structured_input_captures_detail() {
        let input = json!({
            "summary": "叙事",
            "conclusions": [{"claim": "A", "anchors": [{"path": "a.rs", "line": 1}], "stance": "verified"}],
            "open_questions": ["q1"]
        });
        let (_, st) = capture(&input).await;
        let d = st.end_detail.unwrap();
        assert_eq!(d.conclusions.len(), 1);
        assert_eq!(d.open_questions, vec!["q1"]);
    }

    #[tokio::test]
    async fn empty_input_uses_default_summary() {
        let (text, st) = capture(&json!({})).await;
        assert_eq!(text, "Discussion ended.");
        assert!(st.end_detail.is_none());
    }
}
