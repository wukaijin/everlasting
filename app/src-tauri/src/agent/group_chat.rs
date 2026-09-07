//! Group chat orchestration context (07-29-group-chat).
//!
//! Resolved at IPC entry (mirrors `build_workflow_ctx`'s shape): if
//! the session is `session_type = group_chat`, parse the
//! participants config out of `sessions.metadata` and build a
//! [`GroupChatCtx`] the orchestrator consumes. `None` for classic
//! chat → zero overhead, byte-identical behavior.
//!
//! See `.trellis/tasks/07-29-group-chat/prd.md` (D4/D8) for the
//! data-modeling decisions:
//! - participants config lives in `sessions.metadata` JSON
//!   (`{participants: [{name, model, persona_md?}]}`)
//! - `model` is a model_id (key into `ProviderCatalog`)
//! - persona is inline markdown (D8), not an agent.md file

use serde::Deserialize;
use sqlx::SqlitePool;

// ---------------------------------------------------------------------------
// Config types (deserialized from sessions.metadata JSON)
// ---------------------------------------------------------------------------

/// One participant in a group chat (NOT the moderator — the
/// moderator is the session's own model, `SessionRow.model`).
#[derive(Debug, Clone, Deserialize)]
pub struct ParticipantConfig {
    /// Display name shown in the transcript + injected as the
    /// speaker identity. Must be unique within the session.
    pub name: String,
    /// model_id — the key into `ProviderCatalog`. The orchestrator
    /// resolves `catalog.get(&model)` to dispatch this participant.
    pub model: String,
    /// Inline persona markdown (D8). Becomes the participant's
    /// `system_prompt_override` (fully replaces the parent prompt,
    /// same semantics as subagent's `system_prompt`). Optional.
    #[serde(default)]
    pub persona_md: Option<String>,
    // `order` was removed (08-07-group-chat-review-fixes, R4):
    // round-robin fallback is gone, so the orchestrator no longer
    // reads a display order — the moderator's `nominate_speaker`
    // fully decides turn order. The UI used to expose ↑/↓ reorder
    // buttons that wrote `order`, which misled users into thinking
    // it affected speaking order. serde ignores the unknown key, so
    // existing sessions whose `metadata.participants[].order` still
    // carries a value deserialize fine (the field is dropped).
}

/// Top-level `sessions.metadata` shape for a group_chat session.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupChatConfig {
    pub participants: Vec<ParticipantConfig>,
    /// M4a(09-07-gce-m4a-scheduled-deliberation,评审 P1-4):召集通道
    /// 归因 —— `'script'`(M1 CLI)/ `'mcp'`(MCP server)/ `'scheduled'`
    /// (定时任务 fire)。缺省 = GUI/历史 session。**additive 键**:
    /// 转录自动导出钩子的守卫挂在这上面(终态块全通道共享,无守卫一
    /// 挂就会把 GUI/MCP 场也导出)。
    #[serde(default)]
    pub created_via: Option<String>,
    /// C1.2 token budget (09-08-gc-c1-stoploss): the discussion's total
    /// billed-token ceiling (input + output + cache_creation + cache_read,
    /// summed across every inner LLM turn). `None` (absent key) = unlimited
    /// — default-off keeps existing sessions byte-compatible. Exceeded →
    /// the orchestrator halts at the next round head with
    /// `stop_reason = "budget"`. Declared per discussion in
    /// `sessions.metadata`; the scheduled-task / MCP / CLI creation
    /// channels don't carry it yet (M4 cost governance wires those).
    /// Additive key: serde's default keeps old metadata deserializing.
    #[serde(default)]
    pub token_budget: Option<u64>,
    // 注:metadata 还携带 `scheduled_task_name`(fire 建群时写入)——
    // 转录导出经原始 JSON 读取,不进本结构(serde 忽略未知键)。
}

// ---------------------------------------------------------------------------
// GroupChatCtx
// ---------------------------------------------------------------------------

/// Resolved group-chat context. Carries the parsed participants +
/// the moderator's model_id (the session's own model, resolved to a
/// catalog key). Built once at IPC entry by
/// [`build_group_chat_ctx`]; consumed by `run_group_chat_loop`.
#[derive(Debug, Clone)]
pub struct GroupChatCtx {
    /// Parsed participants (excludes the moderator).
    pub participants: Vec<ParticipantConfig>,
    /// The moderator's model_id (the session's own model, which
    /// `SessionRow.model` / `model_id` resolves to). The orchestrator
    /// dispatches the moderator turn through this.
    pub moderator_model_id: String,
    /// The session's working directory, injected into both speaker
    /// prompts (2026-09-06, live-run lesson). Group-chat prompts fully
    /// replace the classic system prompt — which is where the
    /// `- Working directory:` line lives — so without this the speakers
    /// have NO in-band path knowledge and guess absolute roots (live:
    /// a moderator burned 5 × 120s permission asks on a hallucinated
    /// `/home/user/everlasting` before giving up on research).
    pub project_root: Option<String>,
    /// M4a:召集通道归因(metadata `created_via`,缺省 None = GUI/
    /// 历史 session)。终态转录导出钩子守卫
    /// `created_via == Some("scheduled")` 读它 —— 见
    /// [`GroupChatConfig::created_via`]。
    pub created_via: Option<String>,
    /// C1.2 (09-08-gc-c1-stoploss): the declared per-discussion token
    /// ceiling, `None` = unlimited. Read by the orchestrator's round
    /// head; see [`GroupChatConfig::token_budget`].
    pub token_budget: Option<u64>,
}

// ---------------------------------------------------------------------------
// GroupChatControl(09-06-gc-p0-preempt-min-semantics)
// ---------------------------------------------------------------------------

/// Per-live-discussion control channel: the inject buffer + the preempt
/// signal. One entry per session in `AppState::group_chat_controls`,
/// registered by `chat_inner` when it spawns a group-chat orchestration
/// (same timing as the `cancellations` registration — GC1: one entry
/// covers the whole discussion) and removed by the orchestrator's Drop
/// guard on every exit path.
///
/// Two writers, one reader:
/// - `chat_inner`'s routing critical section **pushes** user messages
///   into `pending_injects` when the session is busy (R1 inject —
///   replacing the old legacy 3a path that cancelled the whole
///   discussion);
/// - `preempt_group_chat(session_id)` (commands layer) sets
///   `preempt_requested` (R2);
/// - the orchestrator **drains** at each round head (persisting injects
///   while no inner loop holds a seq cursor — see
///   `db::sessions::insert_user_inject`) and clears `preempt_requested`
///   when it acts on it.
///
/// Lock discipline: `group_chat_controls` is ALWAYS acquired LAST
/// (after `message_queues` → `session_active_request`).
#[derive(Debug, Default)]
pub struct GroupChatControlInner {
    /// User messages arrived while the discussion is busy. In-memory
    /// only (same risk posture as the F1 queue — lost on daemon
    /// restart); persisted by the orchestrator at the round head.
    pub pending_injects: Vec<crate::llm::types::ChatMessage>,
    /// R2 preempt: stop the discussion at the next round boundary —
    /// let the in-flight speaker finish, then one moderator wrap-up
    /// turn (`end_discussion` → summary), terminal
    /// `stop_reason = "preempted"`.
    pub preempt_requested: bool,
}

pub type GroupChatControl = std::sync::Arc<tokio::sync::Mutex<GroupChatControlInner>>;

/// GCE P1a (2026-09-06, task 09-06-gc-p1a-checkpoint-resume): resume
/// context for `run_group_chat_loop`. Built by the `resume_group_chat`
/// command from the persisted checkpoint row. `start_round` is the
/// round the interrupted discussion reached — the loop enters there,
/// so the total round budget (`MAX_ORCHESTRATION_ROUNDS`) stays
/// capped across crash→resume cycles. The GC5 error streak is
/// deliberately NOT carried: a human intervening resets it to 0.
#[derive(Debug, Clone, Copy)]
pub struct GroupChatResume {
    pub start_round: usize,
}

/// Parse the session's metadata + resolve the moderator model.
///
/// Mirrors `build_workflow_ctx`:
/// - returns `Ok(None)` for non-group-chat sessions (the gate),
/// - is defensive: a missing/malformed metadata or an unresolvable
///   moderator model is logged + downgraded rather than aborting
///   the chat (a corrupt config must NOT break the session — it
///   falls back to classic chat behavior).
pub async fn build_group_chat_ctx(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Option<GroupChatCtx>, sqlx::Error> {
    let loaded = match crate::db::load_session(db, session_id).await? {
        Some(l) => l,
        None => {
            tracing::warn!(
                session_id = %session_id,
                "build_group_chat_ctx: session not found; treating as non-group-chat"
            );
            return Ok(None);
        }
    };
    // Gate: only group_chat sessions enter the orchestration.
    if loaded.session.session_type != crate::db::SessionType::GroupChat {
        return Ok(None);
    }

    // Parse participants from metadata JSON. A missing/empty/
    // malformed config is downgraded to an empty participant list
    // (logged) — the moderator can still speak, but no participants
    // means the discussion can't proceed past the opening.
    let config: GroupChatConfig = match loaded.session.metadata.as_ref() {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_else(|e| {
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "build_group_chat_ctx: malformed metadata; proceeding with no participants",
            );
            GroupChatConfig {
                participants: Vec::new(),
                created_via: None,
                token_budget: None,
            }
        }),
        None => {
            tracing::warn!(
                session_id = %session_id,
                "build_group_chat_ctx: group_chat session has no metadata; proceeding with no participants",
            );
            GroupChatConfig {
                participants: Vec::new(),
                created_via: None,
                token_budget: None,
            }
        }
    };

    // Resolve the moderator's model_id. Prefer the session's
    // `model_id` (the catalog key); fall back to `model` (display
    // name) — the orchestrator will try both against the catalog.
    let moderator_model_id = loaded
        .session
        .model_id
        .clone()
        .unwrap_or_else(|| loaded.session.model.clone());

    let project_root = {
        let cwd = loaded.session.current_cwd;
        (!cwd.trim().is_empty()).then_some(cwd)
    };

    Ok(Some(GroupChatCtx {
        participants: config.participants,
        moderator_model_id,
        project_root,
        created_via: config.created_via,
        token_budget: config.token_budget,
    }))
}

impl GroupChatCtx {
    /// Look up a participant config by display name (case-sensitive).
    /// Used by the orchestrator to resolve the moderator's
    /// `nominate_speaker` target.
    pub fn participant_by_name(&self, name: &str) -> Option<&ParticipantConfig> {
        self.participants.iter().find(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R4 regression (08-07-group-chat-review-fixes): the `order`
    /// field was removed from `ParticipantConfig`. Existing group-chat
    /// sessions persisted `order` into `sessions.metadata.participants[]`
    /// before the removal, so on first load after upgrade the metadata
    /// JSON still carries the key. serde's default behavior is to IGNORE
    /// unknown fields (neither `ParticipantConfig` nor `GroupChatConfig`
    /// sets `deny_unknown_fields`), so those sessions must deserialize
    /// cleanly — the stale `order` value is silently dropped. This test
    /// locks that forward-compat contract so a future `deny_unknown_fields`
    /// addition doesn't silently break every pre-upgrade group chat.
    #[test]
    fn participant_config_ignores_legacy_order_field() {
        let metadata = serde_json::json!({
            "participants": [
                {
                    "name": "M1",
                    "model": "m1",
                    "persona_md": "you are M1",
                    "order": 0
                },
                {
                    "name": "M2",
                    "model": "m2",
                    "order": 1
                }
            ]
        });
        let config: GroupChatConfig =
            serde_json::from_value(metadata).expect("legacy metadata with order must deserialize");
        assert_eq!(config.participants.len(), 2);
        assert_eq!(config.participants[0].name, "M1");
        assert_eq!(
            config.participants[0].persona_md.as_deref(),
            Some("you are M1")
        );
        assert_eq!(config.participants[1].name, "M2");
        assert!(config.participants[1].persona_md.is_none());
    }
}
