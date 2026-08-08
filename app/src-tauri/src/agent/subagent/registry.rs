//! `SubagentDef` + the builtin subagent registry.
//!
//! Split out of `agent/subagent/mod.rs` (2026-08-08 batch3).

/// One built-in subagent definition. MVP ships 2 (`researcher` +
/// `general-purpose`); a future PR will load these from Markdown
/// frontmatter (`.everlasting/agents/*.md`, mirroring `.claude/agents/*.md`).
///
/// - `tools` is an **allowlist** — the worker only sees the tools
///   named here. The interceptor additionally strips the
///   structural-disabled set (see `filter_tools_for_subagent`) so
///   even if a future frontmatter definition named
///   `update_checklist` / `dispatch_subagent` / the L1a triple,
///   they would still be removed.
/// - `system_prompt` **fully replaces** the parent's behavior_prompt
///   layer — the worker does NOT inherit the main system prompt
///   (Claude Code convention, see PRD §Decisions 6 + research §5).
#[derive(Clone, Debug)]
pub struct SubagentDef {
    pub name: String,
    /// User-facing description. Consumed by L3d PR3's
    /// `definition_with_cache` to render the per-subagent source
    /// tag + summary in the `dispatch_subagent` tool description
    /// (so the LLM sees builtin + user + project agents with their
    /// provenance). Also kept on the struct so the frontmatter
    /// loader (PR2) can populate it from the Markdown front-matter.
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    /// L3b (2026-06-27): per-agent default for worktree isolation.
    /// When `Some(true)`, workers dispatched under this subagent
    /// run in an isolated git worktree (independent checkout +
    /// `worker/<run_id>` branch) unless the dispatch-time
    /// `isolation` input parameter explicitly overrides to `false`.
    /// `Some(false)` or `None` keeps the legacy shared-cwd behavior
    /// (worker reuses the parent session's worktree). Builtin
    /// `general-purpose` ships with `Some(true)` (write-capable
    /// workers benefit most from isolation); `researcher` ships
    /// with `None` (read-only workers don't need a separate
    /// checkout — saves the per-dispatch checkout cost).
    ///
    /// The final isolation decision is the merge of this default
    /// with the dispatch-time override; see
    /// [`resolve_isolation`] in `dispatch.rs`.
    pub isolation: Option<bool>,
    /// Per-agent model override (task 07-03-subagent-frontmatter-model,
    /// 2026-07-03). When `Some(model_id)`, a worker dispatched under this
    /// subagent resolves its `Arc<dyn Provider>` from the process catalog
    /// (`models.id` → provider) instead of inheriting the parent's
    /// provider — enabling cross-model adversarial review (e.g. a
    /// `reviewer` agent bound to a stronger / different-family model). The
    /// worker's `context_window` follows the target model. `None` (the
    /// builtin + legacy default) inherits the parent provider +
    /// context_window, preserving current behavior. Value is the catalog
    /// key (`models.id`, a UUID); if absent from the catalog at dispatch
    /// time (model deleted / provider api_key empty → `build_provider`
    /// skipped), `run_subagent` logs `warn!` and falls back to the parent.
    /// Display-name resolution is deferred to ROADMAP `B6+` C (UI picker);
    /// MVP writes the raw `models.id`.
    pub model: Option<String>,
}

/// The two MVP subagent definitions, keyed by name. Used by
/// `run_subagent` to resolve the LLM-supplied `subagent` argument.
pub fn builtin_subagents() -> &'static [SubagentDef] {
    // `OnceLock<Vec<SubagentDef>>` holds the registry; `get_or_init`
    // builds it exactly once on first read. The `'static` borrow is
    // sound because the OnceLock itself lives in a `static`.
    static REGISTRY: std::sync::OnceLock<Vec<SubagentDef>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            SubagentDef {
                name: "researcher".to_string(),
                description: "Read-only research subagent. Can read files, grep, glob, list \
                              directories, and fetch web pages, but cannot edit, write, or run \
                              shells. Use for focused code exploration or web research where \
                              the verbose search output would otherwise pollute the main \
                              conversation."
                    .to_string(),
                system_prompt: "You are a read-only research subagent dispatched by the main \
                                agent to investigate a focused question. You have access to \
                                `read_file`, `grep`, `glob`, `list_dir`, and `web_fetch` — use \
                                them to \
                                answer the task as completely as you can. You CANNOT edit, \
                                write, or run shell commands, and you CANNOT dispatch further \
                                subagents (no nesting). When you have gathered enough, write a \
                                concise final summary of what you found — the summary will be \
                                returned to the main agent verbatim as the tool_result of the \
                                dispatch_subagent call, so it should be self-contained. Keep \
                                the summary focused: the main agent has its own full context \
                                and does not need your intermediate tool logs.\n\nReply in the \
                                user's language."
                    .to_string(),
                tools: vec![
                    "read_file".to_string(),
                    "grep".to_string(),
                    "glob".to_string(),
                    "list_dir".to_string(),
                    "web_fetch".to_string(),
                ],
                // L3b (2026-06-27): researcher is read-only, so it
                // does not benefit from a separate worktree (no
                // write conflicts to isolate). Leaving isolation
                // `None` keeps the legacy shared-cwd behavior and
                // saves the per-dispatch checkout cost.
                isolation: None,
                // task 07-03-subagent-frontmatter-model: builtin
                // inherits the parent provider (model: None).
                model: None,
            },
            SubagentDef {
                name: "general-purpose".to_string(),
                description: "General-purpose subagent. Has the full toolset minus the \
                              structural-disabled set (dispatch_subagent, update_checklist, \
                              background-shell tools). Use for self-contained sub-tasks that \
                              would benefit from isolated context (e.g. a focused refactor, \
                              a full test+fix loop, a multi-file search-and-edit)."
                    .to_string(),
                system_prompt: "You are a general-purpose subagent dispatched by the main \
                                agent to work on a self-contained sub-task in your own \
                                isolated context. You have access to the full toolset minus \
                                `dispatch_subagent` (no nesting), `update_checklist`, and the \
                                background-shell tools. The main agent's conversation history \
                                is NOT visible to you — work only from the task prompt you \
                                were given. When you finish, write a concise summary of what \
                                you did (what files you changed, what commands you ran, any \
                                failures) — the summary will be returned to the main agent \
                                verbatim as the tool_result of the dispatch_subagent call, so \
                                it should be self-contained.\n\nReply in the user's language."
                    .to_string(),
                // Empty Vec = "inherit builtin_tools() minus structural-disabled".
                // `filter_tools_for_subagent` reads `tools.is_empty()` as "full set
                // minus disabled"; this keeps the general-purpose subagent's tool
                // list self-maintaining as new tools are added to builtin_tools().
                tools: vec![],
                // L3b (2026-06-27): general-purpose is write-capable, so it
                // benefits most from worktree isolation — concurrent workers
                // can each land writes in their own checkout without racing.
                // B (2026-06-30): default changed to `None` (shared) so
                // a single serial dispatch reuses the parent cwd — zero
                // merge, matches Claude Code's default. Concurrent
                // dispatch is force-isolated in `chat_loop.rs`'s
                // `DispatchBatch::Concurrent` branch (gated by
                // `worker_is_writable`), so concurrent-write safety no
                // longer relies on this default being `Some(true)`.
                isolation: None,
                // task 07-03-subagent-frontmatter-model: builtin
                // inherits the parent provider (model: None).
                model: None,
            },
        ]
    })
}

/// Resolve a built-in subagent by name. Returns `None` for unknown
/// names (the interceptor synthesizes an error tool_result).
///
/// **L3d PR3 (2026-06-25)**: production code now resolves subagents
/// via `SubagentCache::lookup` (which merges builtin + user + project
/// with precedence). This function is retained for the unit tests
/// in this module + `tests_subagent.rs` that want a direct builtin
/// lookup without spinning up a `SubagentCache`. The
/// `#[allow(dead_code)]` silences the "never used" warning from the
/// production build (the function is only called from `#[cfg(test)]`
/// code).
#[allow(dead_code)]
pub fn lookup_subagent(name: &str) -> Option<&'static SubagentDef> {
    builtin_subagents().iter().find(|s| s.name == name)
}
