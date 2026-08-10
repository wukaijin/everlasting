## System prompt assembly (3-layer, cache-stable)

**Location**: `agent/chat_loop.rs::run_chat_loop` entry calls
`system_prompt::assemble_system_prompt`; layers live in
`agent/behavior_prompt.rs` (`DEFAULT_BEHAVIOR_PROMPT`),
`agent/permissions/mod.rs::mode_system_prefix`, and
`agent/system_prompt.rs::build_system_prompt`.

The system prompt sent to the provider is assembled once per `run_chat_loop`
invocation (reused across turns) from three layers, ordered **stablest-first**
so the upstream prompt-cache prefix stays warm:

| Layer | Source | Mutability |
|---|---|---|
| `behavior_prompt` | `behavior_prompt::DEFAULT_BEHAVIOR_PROMPT` (const) | compile-time — tone, objectivity, tool-usage, code conventions, finishing, git safety, language |
| `mode_prefix` | `mode_system_prefix(Mode)` | per-session (Plan/Edit/Yolo permission boundary) |
| `base_prompt` | `build_system_prompt(...)` | per-invocation (cwd, worktree, HEAD sha) |

Assembled as `behavior_prompt + "\n\n" + mode_prefix + "\n\n" + base_prompt`.

### Layering is complementary, not overlapping

- `mode_prefix` is the **permission boundary** (what the system blocks — e.g.
  Plan can't write). `behavior_prompt`'s `Git safety` is the **model's own
  restraint** (never volunteer a commit). Orthogonal dimensions — keep both.
- Tool visibility is **never** described in any prompt layer. It lives
  exclusively in the `tools[]` array sent to the provider (RULE-E-013,
  2026-06-19: the old inline tool-name list in `build_system_prompt` drifted
  and missed 6 of 13 registered tools). The prompt only states tools are
  available + the path-relative convention.
- User-controlled project guidance (AGENTS.md / CLAUDE.md) is delivered via
  **user-role messages** + `cache_control` (`memory/loader.rs`), not the system
  field — a layer *above* this assembly, not part of it.
- **B6 PR1b (2026-06-19)**: the worker's system prompt is **fully replaced**
  via `subagent::assemble_subagent_prompt(def, task)`, NOT mixed with the
  parent's `behavior_prompt` + `mode_prefix` + `base_prompt` (Claude Code
  convention, see research §5). The worker's permission boundary is enforced
  at the ⑨ layer via `PermissionContext.is_worker` (and via `skip_persist`
  / `skip_session_active` for the surrounding isolation), not via prompt text.

### When modifying the system prompt

- New stable behavior rule → edit `DEFAULT_BEHAVIOR_PROMPT` (keep it a const;
  do not derive per-turn).
- New `Mode` → extend `mode_system_prefix`; assembly order is unchanged.
- Do **not** re-introduce a tool-name list in any prompt string — tool
  visibility stays in `tools[]`.

### Tests

`assemble_system_prompt_orders_layers_behavior_mode_base` pins the order;
`behavior_prompt_content_basics` pins the section set + the
`update_checklist`-not-`TodoWrite` invariant (system-prompt-research §7.2);
`build_system_prompt_no_hardcoded_tool_list` pins RULE-E-013.

---

> 历史 ADR 详见 [IMPLEMENTATION.md §4 2026-06-17 RULE-A-007 / 2026-06-20 RULE-A-015](../../../../docs/IMPLEMENTATION.md)
