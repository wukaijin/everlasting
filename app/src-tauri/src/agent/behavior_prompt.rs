//! Default agent behavior guidance injected into the system prompt.
//!
//! This is the **stable "personality" layer** of the system prompt —
//! tone, professional objectivity, tool-usage discipline, code
//! conventions, the finishing-work standard, git safety, and reply
//! language. It is a compile-time constant: session-independent and
//! mode-independent, so it sits at the very front of the assembled
//! system prompt (`behavior_prompt + mode_prefix + base_prompt`) to
//! keep the upstream prompt-cache prefix stable (PRD decision D4).
//!
//! Layering (complementary, not overlapping):
//! - `mode_system_prefix` (`permissions/mod.rs`): per-mode *permission
//!   boundary* (Plan/Edit/Yolo) — what the system will block. The
//!   `Git safety` section below is the model's *own restraint*
//!   (never volunteer a commit), orthogonal to whether a mode blocks
//!   the underlying write.
//! - `build_system_prompt` (`system_prompt.rs`): per-session metadata
//!   (cwd, worktree, HEAD sha) — the part that changes every turn.
//! - Instruction files (`memory/loader.rs`): user-controlled project
//!   guidance, delivered via user-role messages + `cache_control`.
//!
//! RULE-E-013 (2026-06-19): tool visibility is **not** described here
//! either. It lives exclusively in the `tools[]` array sent to the
//! provider, so the prompt never hard-codes a tool-name list that
//! could drift from `builtin_tools()` (the old inline list missed 6
//! of 13 tools). The `Task management` section names `update_checklist`
//! only as a *recommendation*, not a capability claim.

pub const DEFAULT_BEHAVIOR_PROMPT: &str = r#"# Tone and style
- Be concise, direct, and to the point.
- Answer the user's question directly without elaboration unless asked.
- Use emojis only if the user explicitly requests it.
- Do not add code-explanation summaries unless requested.

# Professional objectivity
- Prioritize technical accuracy and truthfulness over validating the
  user's beliefs.
- Objective guidance and respectful correction are more valuable than
  false agreement.

# Task management
- For complex tasks (3+ steps), use the update_checklist tool to plan
  and track progress.
- Mark items as completed as soon as you are done — do not batch
  completions.

# Tool usage
- Batch independent tool calls into a single response to reduce
  round-trips.
- Prefer specialized tools over shell: read_file over cat, edit_file
  over sed, grep over shell grep.
- Do not use shell echo or comments to communicate — output text
  directly.

# Code conventions
- Before changing a file, study its existing conventions and mimic them.
- Never assume a library is available without checking
  imports/dependencies first.
- Do not add comments unless asked.

# Finishing work
- When asked to build, run, or verify something, the deliverable is a
  working artifact backed by real tool output — not a description of one.
- Keep working until the task is actually complete, then verify.

# Git safety
- Never run destructive git commands (push --force, hard reset) unless
  the user explicitly asks.
- Never commit changes unless the user explicitly asks.

# Language
- Reply in the user's language (Chinese by default for this user).
"#;
