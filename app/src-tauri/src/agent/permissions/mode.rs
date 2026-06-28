//! ⑧a Mode check helpers — per-turn system prompt prefix + tool
//! list filter. Used by `agent/chat.rs` / `agent/chat_loop.rs`
//! before every turn. Split out of `mod.rs` on 2026-06-23.

use crate::db::Mode;

/// Per-turn system prompt prefix for the active mode. Injected
/// at the head of the system prompt so the LLM is grounded on
/// the mode's behavioral contract on every request (this is the
/// "per-turn system prompt" layer of the ⑧a triple defense — the
/// other two are tool-list filtering and runtime intercept).
pub fn mode_system_prefix(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "\
You are in Plan mode. You may read files, search, and run readonly \
commands (cat / grep / git log / etc.) to understand the codebase, \
but you CANNOT execute any write tool (write_file, edit_file, shell \
with side effects). If the user asks for an edit, propose the \
change as a diff and ask them to switch to Edit mode to apply it.",
        Mode::Yolo => "\
You are in Yolo mode. All user-confirmation modals are \
automatically skipped. Hard-deny rules (rm -rf /, mkfs, dd if=, \
fork bombs, write-to-disk, chmod 777 /, force-push to protected \
branches, curl|bash) are STILL enforced and will be silently \
denied. Operate with care.",
        Mode::Background => "\
You are in Background mode. (Reserved — not currently exposed in \
the UI.)",
        Mode::Edit => "\
You are in Edit mode (the default). You have full access to all \
tools. Destructive shell commands are silently denied; other \
commands trigger a one-time confirmation modal the first time the \
user sees them per session.",
    }
}

/// ⑧a tool list filter: Plan drops the write tools, Edit/Yolo
/// keep the full set. Returns the filtered tool list to pass
/// to `ChatRequest.tools`. Plan mode still emits the full
/// tool list to the LLM in some Claude-Code-like designs; we
/// choose the explicit filter per audit §2 recommendation
/// (saves a turn + reduces confusion). 3 档化 2026-06-13:
/// Review 移除, 只剩 Plan 一个只读 mode。
pub fn filter_tools_for_mode(
    tools: Vec<crate::llm::ToolDef>,
    mode: Mode,
) -> Vec<crate::llm::ToolDef> {
    match mode {
        Mode::Plan => tools
            .into_iter()
            .filter(|t| !matches!(
                t.name.as_str(),
                "write_file" | "edit_file" | "shell" | "run_background_shell"
                    | "merge_worker" | "discard_worker"
            ))
            .collect(),
        _ => tools,
    }
}
