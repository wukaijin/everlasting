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
            }
        }),
        None => {
            tracing::warn!(
                session_id = %session_id,
                "build_group_chat_ctx: group_chat session has no metadata; proceeding with no participants",
            );
            GroupChatConfig {
                participants: Vec::new(),
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

    Ok(Some(GroupChatCtx {
        participants: config.participants,
        moderator_model_id,
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
