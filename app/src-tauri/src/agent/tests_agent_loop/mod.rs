#![cfg(test)]

// Re-export so cluster files can keep their `use super::tests_common::...`
// imports unchanged from the pre-split single-file layout (pure relocation).
#[allow(unused_imports)]
use super::tests_common;

mod basic;
mod budget;
mod checklist;
mod compaction_summary;
mod error_path;
mod error_persist;
mod handoff;
mod manual_compaction;
mod mock_provider;
mod notifications;
mod parallel_dispatch;
mod recall;
mod resilience;
mod softcap;
mod stub;

/// Helper: flatten a `Vec<ChatMessage>` into a single string for
/// substring assertions. Concatenates every text block in every
/// message — order matters for the ephemeral-injection tests
/// because the checklist block is PREPENDED (so it should appear
/// before the user's "hello" text from `test_messages()`).
pub(super) fn messages_to_text(msgs: &[crate::llm::types::ChatMessage]) -> String {
    let mut out = String::new();
    for m in msgs {
        match &m.content {
            crate::llm::MessageContent::Text(t) => out.push_str(t),
            crate::llm::MessageContent::Blocks(blocks) => {
                for b in blocks {
                    if let crate::llm::ContentBlock::Text { text, .. } = b {
                        out.push_str(text);
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}
