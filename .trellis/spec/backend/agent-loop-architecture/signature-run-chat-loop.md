## Signature: `run_chat_loop`

**Location**: `app/src-tauri/src/agent/chat_loop.rs`

```rust
#[allow(clippy::too_many_arguments)]
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,                                                          // 1
    provider: Arc<dyn Provider>,                                                       // 2
    context_window: u32,                                                               // 3
    rid: String,                                                                       // 4
    session_id: String,                                                                // 5
    messages: Vec<ChatMessage>,                                                        // 6
    sink: Arc<dyn ChatEventSink>,                                                      // 7
    db: SqlitePool,                                                                    // 8
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,                     // 9
    session_active_request: Arc<Mutex<HashMap<String, String>>>,                       // 10
    read_guard: ReadGuard,                                                             // 11
    memory_cache: Arc<MemoryCache>,                                                    // 12
    skill_cache: Arc<SkillCache>,                                                      // 13
    permission_asks: crate::agent::permissions::PermissionStore,                       // 14
    token: CancellationToken,                                                          // 15
    // D3 PR3 (2026-06-17): resend context. When `Some(seq)`, the
    // user-message persist site writes a `resend_message` audit row
    // pointing at the original user message's seq. `None` for normal
    // first-time sends. Best-effort.
    resend_seq: Option<i64>,                                                           // 16
    // L1a (2026-06-19): cross-request background-shell registry.
    // Threaded into `ToolContext` so the 3 L1a tools can call into it.
    // The agent loop itself reads it once per turn (after C3 compaction,
    // before `provider.send`) to drain completion notifications.
    background_shells: crate::background_shell::DefaultRegistry,                        // 17
    // B6 PR1a (2026-06-19): worker turn cap. `None` = default 50 (MAX_TURNS).
    // `let turn_limit = max_turns.unwrap_or(MAX_TURNS); for turn in 1..=turn_limit`.
    // Production + 9 base tests pass `None`; the worker path passes `Some(SUBAGENT_MAX_TURNS)` (= `Some(200)`).
    max_turns: Option<usize>,                                                          // 18
    // B6 PR1b (2026-06-19): when `true`, `CancellationGuard::drop` skips
    // `session_active_request.remove(session_id)`. The worker path uses this
    // so its Drop does NOT evict the parent's `session_active_request[parent_session_id]`
    // entry (REVIEW-SUBAGENT-PRD #2 / RULE-E-005). Production + tests pass `false`.
    skip_session_active: bool,                                                         // 19
    // B6 PR1b (2026-06-19): when `true`, all DB writes inside `run_chat_loop`
    // (persist_turn / update_message_metadata / touch_session / add_token_usage /
    // record_*_audit / persist_turn_cwd — 18 sites total) are skipped. The
    // worker path uses this so its intermediate turns stay in-memory only
    // (the `SubagentBufferSink` transcript captures them; PR2 persists into
    // `subagent_runs`). Skipping also avoids the UNIQUE-constraint collision
    // with the parent's own `persist_turn` calls — both loops would otherwise
    // write to the same `messages` table keyed by `(session_id, seq)`.
    //
    // **PR2a correction (2026-06-20, RULE-A-015)**: the 18 site count is
    // actually 16 in the implementation as merged. PR2a fixed 2 over-broad
    // gates that PR1b introduced: (a) `add_token_usage` — token-usage
    // metadata belongs to the `sessions` table, not the `messages` table, so
    // it should NOT be gated by `skip_persist` (worker must still stream its
    // per-turn token usage into the parent's `sessions` accumulator so the
    // parent's UI shows live total cost); (b) the terminal `Done` event
    // emit — the `SubagentBufferSink` was the BOTH the consumer of the
    // terminal `Done` and the data source for `transcript_snapshot()`, so
    // gating it killed the worker's `was_cancelled` tracking. Both are
    // now outside the gate. See "Pattern: PR2a corrected PR1 over-broad
    // skip_persist gate (RULE-A-015)" below.
    //
    // **2026-06-26 reversal (task 06-26-fix-token-usage-snapshot)**: item (a)
    // (`add_token_usage`) is REVERSED — worker token now stays OUT of the
    // parent's sessions totals. Rationale: worker reuses parent session_id,
    // so streaming its per-turn usage into the parent's accumulator polluted
    // the parent's "context occupancy %" (subagent turns summed in → 1.7M /
    // 100% blowup). Token usage switched to a per-turn **snapshot**
    // (`update_last_turn_usage`, overwrites not accumulates) and is gated
    // back inside `!skip_persist`; worker token lives in
    // `subagent_runs.token_usage_json` only. Item (b) (terminal Done emit)
    // stays outside the gate — that part of PR2a stands.
    skip_persist: bool,                                                                // 20
    // B6 PR2b (2026-06-20, RULE-A-014): when `Some(true)`, the
    // `PermissionContext` built inside the loop carries `is_worker: true`,
    // which makes the Tier 4 `ask_path` / `ask_shell` branches collapse
    // to `Decision::Deny` instead of emitting a `permission:ask` (workers
    // have no UI sink — a permission modal would hang forever on the
    // oneshot). `None` falls back to the session-row mode's natural
    // default (production-style = `false`, since no parent process is a
    // worker). The worker path passes `Some(true)`; production + 35
    // `agent_loop_*` integration tests pass `Some(false)` to make the
    // production default explicit at the call site.
    is_worker: Option<bool>,                                                           // 21
    // B6 PR3 (2026-06-20, PR2 hotfix): optional Tauri `AppHandle` used
    // ONLY by `run_subagent` to construct the worker's
    // `SubagentBufferSink` with a live IPC emit path (the
    // `subagent:event` channel). The agent loop body itself does NOT
    // read this parameter — only `run_subagent` does, when building
    // the worker sink. Production passes `Some(app.clone())` from the
    // `chat` Tauri command; tests pass `None` (no Tauri runtime, the
    // worker's IPC emit becomes a no-op). `AppHandle` is an `Arc`
    // internally so the clone is cheap.
    app_handle: Option<tauri::AppHandle>,                                              // 22
    // 2026-06-21 fix (B6 review defect A): worker system prompt
    // override. When `Some(p)`, the loop uses `p` directly as the
    // system prompt (skipping `assemble_system_prompt(mode_prefix,
    // base_prompt)`); when `None`, the loop builds the prompt from
    // the project + session row. `run_subagent` (worker nested call)
    // passes `Some(assemble_subagent_prompt(def, &task))`; the
    // production `chat` command + 36 `agent_loop_*` integration
    // tests pass `None` (parent path). 4 指令文件 prompt caching is
    // unaffected — the 4 instructions live in a separate user-role
    // synthetic message with its own `cache_control: Ephemeral`
    // breakpoint (see `build_instructions_blocks`), independent of
    // the system role.
    system_prompt_override: Option<String>,                                            // 23
) { ... }
```

> **Warning — Entry invariant: "the tail user message is a fresh, not-yet-persisted send".**
>
> `run_chat_loop`'s entry user-message persist site
> (`chat_loop.rs`, "Persist the most recent user message before the agent
> loop runs") **unconditionally re-persists** `messages`'s tail user-role
> message. This relies on an implicit invariant: that message is a fresh
> frontend send, not an already-persisted DB row.
>
> **Violating it is the root cause of the group-chat 400 death-loop**
> (`08-04-group-chat-orchestration-rewrite`): `messages` has
> `UNIQUE(session_id, seq)` but `run_chat_loop` recomputes `seq = max+1`
> on every entry, so re-persisting a reloaded `tool_result` (role=user)
> writes a NEW row with the same content — no UNIQUE collision → a
> duplicate `tool_result` with no matching `tool_calls` → OpenAI 400 /
> Anthropic 2013 on every subsequent request. DB forensics in
> `.trellis/tasks/08-04-group-chat-orchestration-rewrite/research/db-evidence.md`
> show the same `tool_use_id` accumulating 30+ rows.
>
> **Guard (D-D, 08-04 rewrite)** — the persist site skips re-writing when
> ALL of:
> 1. `group_chat_state.is_some()` (a group-chat speaker — ordinary chat
>    short-circuits, byte-identical behavior);
> 2. the tail user message content-matches **any** user-role row in
>    `loaded_session.messages` (`user_message_matches`: `tool_result`
>    by `tool_use_id`, plain text by byte equality).
>
> Do NOT restore a `messages.len() == loaded_session.messages.len()`
> length criterion (the pre-08-04 heuristic, D-F): it is always false
> after the memory/skill injection pass inserts synthetic user rows into
> `messages`, and it misfires on filtered participant views (fewer rows
> than the DB). Known cosmetic boundary (documented in the guard
> comment): a human re-sending the EXACT same text in a group chat is
> judged already-persisted and skipped — one text row lost, no tool-pair
> breakage, no 400.

### Why 23 parameters (and not a config struct)?

The 23 parameters look excessive, but they are the **exact set of state pieces
the agent loop body needs**, and grouping them into a config struct would:

1. Hide the dependency surface (a struct named `RunChatLoopArgs` would tempt
   callers to add fields that are *only* test-internal)
2. Add a layer of indirection without adding safety (Rust's borrow checker
   already enforces "use what you need")
3. Obscure the 1:1 correspondence between production and test call sites
   (the 36 `agent_loop_*` integration tests, including the 5 B6 worker tests
   and 1 B6 PR2b end-to-end test, pass them in the same order, with the same
   types — a config struct would let them diverge silently)

`#[allow(clippy::too_many_arguments)]` is the deliberate cost of keeping the
dependency surface explicit. **Do not refactor this into a struct** without
re-running all 36 integration tests + cargo check.

#### Evolution log (parameter count grew with new features)

| Date | Count | PR / task | New param | Why |
|---|---|---|---|---|
| 2026-06-15 | 14 | `06-15-unify-chat-loop-dispatch` (RULE-A-006 closure) | — | baseline after production migrated through `run_chat_loop` |
| 2026-06-17 | 15 | D3 PR3 | `resend_seq: Option<i64>` | resend audit row at user-message persist site |
| 2026-06-19 | 17 | L1a | `background_shells: DefaultRegistry` | cross-request registry threaded into `ToolContext` + per-turn notification drain |
| 2026-06-19 | 18 | B6 PR1a | `max_turns: Option<usize>` | worker turn cap; production + tests pass `None` |
| 2026-06-19 | 19 | B6 PR1b | `skip_session_active: bool` | worker guard Drop skips `session_active_request.remove` |
| 2026-06-19 | 20 | B6 PR1b | `skip_persist: bool` | persist-site gates inside the function body (PR1 spec: 18 sites; PR2a actual: 16 — see RULE-A-015) |
| 2026-06-20 | 21 | B6 PR2b (RULE-A-014) | `is_worker: Option<bool>` | thread `is_worker` to nested `run_chat_loop` so Tier 4 `ask_path` / `ask_shell` collapses to `Deny` on the worker path (workers have no UI sink) |
| 2026-06-20 | 22 | B6 PR3 (PR2 hotfix) | `app_handle: Option<tauri::AppHandle>` | thread the parent's `AppHandle` through so `run_subagent` can wire the worker's `SubagentBufferSink` with a live `subagent:event` IPC emit path (live transcript streaming for the PR3b `<SubagentDrawer>`); tests pass `None` |
| 2026-06-21 | 23 | `06-21-fix-worker-system-prompt-dead-code` (B6 review defect A) | `system_prompt_override: Option<String>` | thread the worker's `SubagentDef.system_prompt` through as the override (pre-fix `_worker_system_prompt` was dead code; the worker silently inherited the parent's `assemble_system_prompt` output, causing prompt/permission contradictions in Edit/Plan mode); production + 36 `agent_loop_*` tests pass `None`, the worker nested call passes `Some(assemble_subagent_prompt(def, &task))` |
| 2026-06-30 | 24 | `06-30-explicit-agent-dispatch` | `forced_dispatch: Option<ForcedDispatch>` | user `@@<agent> <task>` prefix → turn-1 short-circuit bypasses `provider.stream` (parent LLM zero calls) + reuses `run_subagent` directly; production passes the parsed `ForcedDispatch` (or `None`), worker nested + all tests pass `None` |
| 2026-07-03 | 25 | `07-03-subagent-per-agent-model-ui` (B6+ C) | (no new `run_chat_loop` param) | per-agent model priority chain `DB override > frontmatter > parent` lives in `agent::subagent::dispatch::resolve_final_model`, called by `run_subagent` BEFORE `resolve_worker_provider`; the resolver itself is **unchanged** (6 existing tests stay green); the new `subagent_model_overrides` table (`db::subagent_overrides`) is the global DB row that wins over the file-declared `model:` |
| 2026-07-07 | 26 | `07-06-b6plus-b-dispatch-model-arg` (B6+ B) | (no new `run_chat_loop` param) | per-dispatch model override extends the priority chain to **`dispatch > DB > frontmatter > parent`**. The overlay is one line in `run_subagent`: `final_model = dispatch_model.or(resolved_lower)` where `dispatch_model` is parsed from `input.model` (LLM path sends a display_name from the schema enum; user `@@agent --model=` path sends an id resolved by the frontend). A new `resolve_model_by_name_or_id(db, input)` does display_name→id reverse-lookup (id passthrough first, then `list_models` first-match on display_name; miss → `None` → falls through to `resolve_final_model`). `resolve_final_model` / `resolve_worker_provider` are **unchanged** (their tests stay green). The `dispatch_subagent` schema gains a `model` property with a dynamic enum built from a per-`run_chat_loop` `list_models` snapshot (`Vec<ModelBrief>`, display_name values — the system prompt does not list models so the enum is the LLM's only discovery channel). `ForcedDispatch` gains `model_id: Option<String>` (`#[serde(default)]`, wire snake_case). |

The B6 cluster (PR1a + PR1b + PR2b + PR3, adding 5 params across 4 sub-PRs in a
single 2-week window) is the largest single jump. It is justified because
the worker is a **structural** extension of the agent loop (it re-uses
`run_chat_loop` recursively via `Box::pin`, see "Pattern: Worker Subagent"
below), and the 5 params are the minimal surface needed to keep
production + worker behavior isolated (session mapping cleanup + DB
isolation + turn cap + Tier 4 collapse without hang + IPC emit path).
The follow-up `system_prompt_override` param (2026-06-21, B6 review defect A)
is a one-shot fix for a dead-code bug in the worker path — it restores the
worker to its `SubagentDef.system_prompt` after PR1b's nested call had
silently inherited the parent's `assemble_system_prompt` output (causing
prompt / permission contradictions in Edit/Plan mode). The 6 total B6 params
across 5 PRs remain the minimum surface needed.

**B6+ C (2026-07-03, task `07-03-subagent-per-agent-model-ui`) does NOT
add a new `run_chat_loop` param** — the per-agent model priority chain
`DB override > frontmatter > parent` is enforced **upstream** of
`resolve_worker_provider` (a new `resolve_final_model` helper in
`agent::subagent::dispatch` collapses the two priority arms into a
single `Option<model_id>`; the resolver itself is unchanged so its
6 existing tests stay green). The chain lives entirely in `run_subagent`
body, which already had the `catalog` (added by task 07-03) and now
also reads from the `subagent_model_overrides` table via a single
DB call. The Settings-UI surface (`list_subagents_with_model` +
`set_subagent_model` commands in `commands::subagents`) is the only
new IPC pair. **`run_chat_loop`'s 24-param signature stays unchanged**
in this task — the upstream priority resolution is the architectural
decision; the alternative (adding a `model_override` param to
`run_chat_loop` so the parent path could also benefit from per-session
overrides) is deferred to a follow-up.

**B6+ B (2026-07-07, task `07-06-b6plus-b-dispatch-model-arg`) also
does NOT add a `run_chat_loop` param** — it extends the C priority
chain upward by one tier to `dispatch > DB > frontmatter > parent`.
The overlay is `final_model = dispatch_model.or(resolved_lower)` in
`run_subagent` (one line; `dispatch_model=None` collapses to C's
behavior, so A/C zero-regression). `dispatch_model` is parsed from
`input.get("model")` — both entry points converge on this field:
the LLM-driven `dispatch_subagent({model})` path sends a display_name
(the schema `model` enum lists display_names), and the user `@@agent
--model=<X>` path sends an id (the frontend's `resolveModelInput`
reverse-resolves display_name→id before IPC). The display_name→id
reverse-lookup is `resolve_model_by_name_or_id(db, input)` (id
passthrough first, then `list_models` first-match; miss → `None` →
graceful degrade to `resolve_final_model`). A miss / typo does NOT
fail the dispatch — it logs `warn!` and falls through to the agent's
configured default. The `dispatch_subagent` schema `model` enum is
fed by a per-`run_chat_loop` snapshot of `list_models` projected into
`Vec<ModelBrief>` (display_name values; built once outside the turn
loop — CRUD during a session reflects next session + catalog-miss
fallback covers the lag).

### Production + test call site parity

- **Production**: `app/src-tauri/src/agent/chat.rs::chat` Tauri command's
  `tauri::async_runtime::spawn` body, after pre-flight (provider lookup +
  cancel token registration + sink build). The call site passes `None` for
  `resend_seq` + `max_turns`, `false` for both `skip_session_active` and
  `skip_persist`, `Some(false)` for `is_worker` (production is never a
  worker; the explicit `Some(false)` makes the production-style default
  obvious at the call site, matching PR2b's contract),
  `Some(app.clone())` for `app_handle` (PR2 hotfix: production threads a
  real `AppHandle` so the worker's `SubagentBufferSink` can emit
  `subagent:event` to the frontend), and `None` for
  `system_prompt_override` (production is never a worker, so the parent's
  `assemble_system_prompt(mode_prefix, base_prompt)` path runs unchanged).
- **Tests**: `app/src-tauri/src/agent/tests.rs::agent_loop_basic_text_only_completes`
  and 35 sibling tests pass a `MockProvider` + `MockEmitter` for the
  `Arc<dyn Provider>` and `Arc<dyn ChatEventSink>` parameters. Other
  parameters are real (test DB, real `MemoryCache`, real `PermissionStore`,
  real `ReadGuard`). Tests pass `Some(false)` for `is_worker` to make the
  non-worker test surface explicit, `None` for `app_handle` (no Tauri
  runtime — the worker's IPC emit path becomes a no-op; the worker's
  `SubagentBufferSink` is constructed via
  `SubagentBufferSink::new_without_app_handle` so transcript accumulation
  still works), and `None` for `system_prompt_override` (the production +
  test path runs through `assemble_system_prompt(mode_prefix,
  base_prompt)` unchanged).

The 23-parameter signature is **production-ready as written** — no test-only
gating (no `#[cfg(test)]`), no compile-time `dead_code` allowance, no
runtime branching on `cfg!(test)`.

---
