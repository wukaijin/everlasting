//! Worker prompt + initial messages assembly.
//!
//! Split out of `agent/subagent/mod.rs` (2026-08-08 batch3).

use std::sync::Arc;

use crate::llm::types::MessageContent;
use crate::llm::{ChatMessage, Role};
use crate::memory::MemoryCache;

use super::registry::SubagentDef;

/// Assemble the worker's system prompt. **Fully replaces** the
/// parent's behavior_prompt + mode_prefix + base_prompt layers —
/// the worker does NOT inherit the main system prompt (Claude Code
/// convention). The mode-specific permission boundary is enforced
/// at the ⑨ 关 layer, not in the prompt.
///
/// **Active since 2026-06-21 (B6 review defect A fix).** The
/// `assemble_subagent_prompt(def, task)` output is now threaded
/// as the 23rd `system_prompt_override` parameter on the
/// `run_chat_loop` nested call (see
/// `agent::subagent::dispatch::run_subagent`); the loop body short-
/// circuits the parent's `assemble_system_prompt(mode_prefix,
/// base_prompt)` step when the override is `Some(_)`. Pre-fix
/// the prompt was effectively dead code (the worker's
/// `SubagentDef.system_prompt` was discarded, and the worker
/// silently received the parent's prompt — contradicting the
/// mode-specific permission behaviour enforced at the ⑨ 关
/// layer). See `docs/review/b6-subagent-assessment.md` §2 +
/// the doc comment on `run_chat_loop.system_prompt_override`
/// for the full rationale.
pub fn assemble_subagent_prompt(def: &SubagentDef, _task: &str) -> String {
    // The task itself is delivered as a user message (see
    // `build_worker_messages`); the system prompt is just the
    // worker's role + behavior guidance. The `_task` is reserved
    // for a future "task summary header" if we want to echo it.
    def.system_prompt.clone()
}

/// Build the worker's initial `messages` Vec.
///
/// Per PRD §Decisions 6 + review #6:
/// 1. `messages[0]` = memory instructions synthetic user message
///    (loaded via `build_instructions_blocks`, banner block carries
///    `cache_control: Some(Ephemeral)` so the worker has its OWN
///    cache breakpoint, independent of the parent).
/// 2. `messages[1]` = delegation task (APPEND, never prepend —
///    see prompt-cache invariant: the memory breakpoint must stay
///    at position 0).
///
/// When the project has no loaded memory layers, only the task
/// message is emitted (the parent's behavior — skip the synthetic
/// message entirely on a fresh install — is preserved).
pub async fn build_worker_messages(
    memory_cache: &Arc<MemoryCache>,
    project_id: &str,
    project_path: &str,
    task: &str,
) -> Vec<ChatMessage> {
    let layers =
        crate::memory::loader::load_for_session(memory_cache, project_id, project_path).await;
    let instructions_blocks = crate::memory::loader::build_instructions_blocks(&layers);
    let mut messages: Vec<ChatMessage> = Vec::with_capacity(2);
    if !instructions_blocks.is_empty() {
        // messages[0] — memory synthetic user message, banner carries
        // cache_control: Ephemeral. Worker's own breakpoint.
        messages.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(instructions_blocks),
            speaker: None,
        });
        // Mirror the parent loop's memory pair: a synthetic assistant
        // ack keeps the Anthropic wire shape happy (user/assistant
        // alternation) and signals the worker has acknowledged the
        // instructions before the task arrives.
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(
                "Understood. I will follow these instructions while working on the \
                 delegated task."
                    .to_string(),
            ),
            speaker: None,
        });
    }
    // The delegation task. APPEND — the memory breakpoint (if any)
    // stays at messages[0]; the task's position is independent of
    // whether memory is loaded. Anthropic accepts a user-role
    // message after an assistant-role message.
    messages.push(ChatMessage {
        role: Role::User,
        content: MessageContent::Text(task.to_string()),
        speaker: None,
    });
    messages
}
