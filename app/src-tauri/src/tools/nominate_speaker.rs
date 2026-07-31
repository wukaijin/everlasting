//! `nominate_speaker` — group-chat moderator tool (07-29-group-chat).
//!
//! The moderator LLM calls this to hand the floor to a specific
//! participant. The agent loop intercepts the tool_use (same
//! interception pattern as `ask_user_question` / `request_mode_change`)
//! and records the nominee in the shared [`GroupChatTurnState`]; the
//! outer `run_group_chat_loop` orchestrator reads it after the
//! moderator's `run_chat_loop` returns and dispatches that participant
//! next.
//!
//! Unlike `ask_user_question` this is a SIGNAL tool, not a BLOCKING
//! tool — it does NOT wait on a oneshot for a frontend response. It
//! returns immediately with a confirmation string (so the moderator's
//! turn ends cleanly: `should_continue` sees no further tool_use and
//! `run_chat_loop` returns at the natural turn boundary).

use crate::llm::types::ToolDef;

/// Shared, mutable turn state threaded through `run_group_chat_loop`.
/// The moderator's `run_chat_loop` writes here via the
/// `nominate_speaker` / `end_discussion` interception; the orchestrator
/// reads after each `run_chat_loop` returns.
///
/// `Arc<Mutex<Option<...>>>` so the interception handler (inside the
/// spawned loop) and the orchestrator (outside) can share it across
/// the async boundary.
#[derive(Debug, Default)]
pub struct GroupChatTurnState {
    /// The participant name the moderator nominated, or `None` if
    /// the moderator hasn't called `nominate_speaker` yet this round.
    pub next_speaker: Option<String>,
    /// `true` once the moderator calls `end_discussion`. The
    /// orchestrator stops the turn-taking loop.
    pub discussion_ended: bool,
}

pub type SharedTurnState = std::sync::Arc<tokio::sync::Mutex<GroupChatTurnState>>;

/// Tool name — MUST match the interception branch in `chat_loop.rs`.
pub const NOMINATE_SPEAKER_TOOL_NAME: &str = "nominate_speaker";

/// Schema. The moderator picks ONE participant by `name` (must match
/// a configured participant). `reason` is free-text rationale shown
/// in the transcript for traceability.
pub fn definition() -> ToolDef {
    ToolDef {
        name: NOMINATE_SPEAKER_TOOL_NAME.to_string(),
        description: Some(
            "Group chat only: hand the floor to a named participant for their turn. \
             Call this after you (the moderator) have spoken / summarized, to pick \
             who speaks next. The participant will see the full conversation so far \
             and respond. Use the participant's exact name.\n\n\
             Pair with `end_discussion` when the discussion has run its course."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "The exact name of the participant who should speak next."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional one-line rationale (shown in transcript for traceability)."
                }
            },
            "required": ["name"]
        }),
    }
}

/// Async execution used by the chat_loop interception handler.
/// Records the nominee in the shared turn state + returns a
/// confirmation string that becomes the `tool_result` (so the
/// moderator's turn ends at a clean boundary). `is_error=true`
/// short-circuits (the moderator gets the error and can self-correct
/// next turn).
pub async fn execute_intercept(
    state: &SharedTurnState,
    input: &serde_json::Value,
) -> (String, bool) {
    let name = match input.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => {
            return (
                "nominate_speaker: missing or empty `name`. Pick a configured participant."
                    .to_string(),
                true,
            );
        }
    };
    let mut st = state.lock().await;
    st.next_speaker = Some(name.clone());
    drop(st);
    (format!("Floor handed to {}.", name), false)
}
