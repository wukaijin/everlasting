## Pattern: Worker Worktree Override (`worktree_override` param, L3b PR1, 2026-06-27)

L3b PR1 introduces two new parameters to `run_chat_loop`:

- **25th `worktree_override: Option<PathBuf>`** — when `Some(path)`, the loop uses `path` as the worker's worktree root INSTEAD of `loaded_session.session.worktree_path` (which is the parent session's worktree — the root cause of worker reuse of the parent's checkout, see `git/diff.rs::diff_against_branch` for the diff-side contract).
- **26th `app_data_dir: PathBuf`** — pass-through to the dispatch_subagent interceptor (the agent loop body itself does NOT read it; only `run_subagent` does, when creating the worker worktree path).

Mirrors the existing `system_prompt_override` (23rd param) override pattern: per-call clarity, no config struct, thread the override at the `ToolContext` construction site (line ~452). When `None`, the loop builds `worktree_path` + `cwd` from the session row as before (production chat + test path, AND the non-isolated worker path).

### `worktree_override` interaction with `current_cwd`

When `worktree_override.is_some()`, `current_cwd` defaults to `worktree_path` (the override). The parent session's `current_cwd` history is meaningless for a worker (it points at a path inside the parent's checkout, not the worker's). When `None`, `current_cwd` falls through to `loaded_session.session.current_cwd` (legacy behavior, unchanged).

### Why 26 parameters (and not a config struct)?

Same argument as the existing 23-param `run_chat_loop` (see "Why 23 parameters" section above). The 2 added parameters follow the same precedent — per-call overrides are clearer than a config struct that grows every time. Tradeoff: marginal cost (each new override = 1 more param) vs one-time refactor cost.

### Interaction with `STRUCTURALLY_DISABLED` + worker nesting gate

The L3b override does NOT change the no-nesting invariant: `dispatch_subagent` is still stripped from worker's toolset via `STRUCTURALLY_DISABLED` + `effective_is_worker` gate (L3d PR3 lesson, see "Pattern: Worker Subagent" above). A worker's worker (depth 2+) cannot happen. PR1's worker worktree is depth 1 only.

### Tests Required (L3b PR1 additions to `agent/tests_agent_loop.rs` + `agent/tests_subagent.rs`)

The 6 `agent_loop_*` integration tests in `tests_agent_loop.rs` thread `None, h.app_data_dir.clone()` (production-style caller). The B6 worker tests in `tests_subagent.rs` gain:

| Test | Asserts |
|---|---|
| `l3b_worker_with_isolation_runs_in_worker_worktree` | dispatch_subagent with isolation=true → worker's tool calls observe a different `ToolContext.worktree_path` than the parent's session row |
| `l3b_worker_with_isolation_false_runs_in_parent_worktree` | dispatch_subagent with isolation=false → worker's tool calls run in parent session's worktree (legacy behavior preserved) |
| `resolve_isolation_truth_table` | 4-row merge semantics from `tool-contract.md §dispatch_subagent isolation` table |

## Pattern: Forced dispatch — `@@` explicit dispatch (2026-06-30)

**Problem**: `dispatch_subagent` is an LLM-owned tool — the model decides whether + which worker to run. The user had no way to force a specific agent (e.g. `@@spec-auditor 审一下 X.md`) without the LLM judging whether to comply.

**Mechanism** (`06-30-explicit-agent-dispatch`): a `@@<agent> <task>` input prefix is parsed by the frontend (`chat.ts send()`) into a `ForcedDispatch { subagent, task }`, threaded through the `chat` Tauri command → `run_chat_loop`'s 24th param `forced_dispatch: Option<ForcedDispatch>`.

**Turn-1 short-circuit** (sits AFTER the user-message persist site, BEFORE `for turn in 1..=turn_limit`): when `Some(fd)`, the loop:
1. emits `ChatEvent::Start` + a synthetic `dispatch_subagent` `tool:call` (`tool_use_id = "forced_{rid}-{seq}"`);
2. calls `run_subagent(...)` directly — **`provider.stream` is NOT called** (this is the load-bearing invariant: the parent LLM contributes zero calls);
3. emits the worker's summary as `tool:result` + `ChatEvent::Delta` (assistant text) + `ChatEvent::Done`;
4. persists the assistant turn (`Blocks = [ToolUse(dispatch), Text(summary)]`) + returns. **Forced dispatch runs exactly ONE turn** — no follow-up LLM loop.

**Reuses `run_subagent` verbatim** — the 19 params mirror the LLM-driven interceptor at `chat_loop.rs:2374` (force_readonly=false, parallel=false → single serial dispatch; isolation falls back to the subagent's frontmatter default via `resolve_isolation`). The permission chain (worker inherits parent Mode → `WorkerAskBanner`) is unchanged.

**Why this isn't a new dispatch path**: it's the same `run_subagent` the LLM-driven interceptor uses, just triggered by a user prefix instead of an LLM `tool_use`. The only thing the prefix skips is `provider.stream` on the parent's turn 1.

**Test**: `agent_loop_forced_dispatch_runs_worker_without_llm` asserts `mock.call_count() == 1` (only the worker's single turn — the parent contributed zero LLM calls).
| `builtin_*_defaults_to_isolated` | `general-purpose.isolation == Some(true)`, `researcher.isolation == None` |
| `probe_worker_changes_*` (3 tests in tests_dispatch.rs) | empty worktree → no changes; tracked edit → changes; untracked file → changes |

---

## Pattern: LLM retry_open wrapper (A5+, 2026-07-05)

`run_chat_loop` turn 内 provider 阶段的 `provider.send(...)` 由 `llm::retry::retry_open(...)` 包装(`chat_loop.rs` 调用点)。retry_open 处理**首字节前**的可重试失败(`Network`/`Server`/`RateLimit`):Full Jitter 退避 + retry-after advisory + 双向熔断(`max_retries` × `budget`)。**首字节后**任何错误交回 chat_loop per-event loop(had_error 路径,不回 retry)。

**为何是 turn 内 wrapper 而非 Provider trait 内**:Provider 专注协议转换不感知 retry;wrapper 可见 chat_loop 的 `token`(取消)与 `sink`(前端 Retrying 事件);Provider trait 签名零改动。

返回 `OpenOutcome::Stream(first_byte chained with rest)` 或 `Cancelled` — chat_loop 拿到 Stream 后用既有 per-event select loop 消费,**select loop 零改动**。两个 select(首字节 await / backoff sleep)都 `biased` 第一位 `token.cancelled()`,sleep 中取消立即响应。

完整契约(retryable 分类 / Full Jitter 公式 / retry-after 解析 / `LlmError` headers 字段扩展 / 前端 Retrying 事件 / 测试矩阵)见 [llm-contract.md Scenario: LLM Retry / Backoff (A5+)](./llm-contract.md)。决策见 [IMPLEMENTATION §4 2026-07-05](../../../docs/IMPLEMENTATION/decisions.md)。

---

## Pattern: E2 trace pipeline — emit + persist at 4 harness write points (E2, 2026-07-14)

> **Source**: E2 trace viewer task `07-14-e2-harness-trace-viewer`
> (child-1 `e2-backend-trace-pipeline`).

The trace pipeline is a **double-write best-effort contract** that
hooks into 4 existing harness write points (NOT a separate agent
loop). Each hook emits a `ChatEvent` for the live panel AND upserts
the corresponding `turn_trace` column for history. The schema lives
in [database-guidelines.md Pattern: per-turn trace UPSERT
accumulation](./database-guidelines.md); the event shapes live in
[llm-contract.md Scenario: E2 trace ChatEvent variants](./llm-contract.md).

**The 4 write points** (all inside the turn loop, after the
existing signal is produced):

| Signal | Hook location | Event variant | turn_trace column |
|---|---|---|---|
| C3 compaction | `compact_messages` return — both normal and `StillOver` branches | `ContextCompacted` | `compaction_json` |
| C2 soft hint (1-2 hits) | right after `verdict_kind` is determined, gated `loop_hit_count < 3` (the `≥3` path keeps writing the existing `loop_intervention` audit) | `LoopHint` | `loop_hint_json` |
| Workflow breadcrumb | right after `append_workflow_breadcrumb` returns `true` (S-B guard passed) | `WorkflowBreadcrumb` | `breadcrumb_json` |
| Per-turn token | alongside `update_last_turn_usage` inside the `!skip_persist` gate (reuses `ChatEvent::Done { usage }`) | (reuses `Done`) | `token_usage_json` |

**The trace helpers (`agent/trace.rs`)** are deliberately small:
each takes the same `(sink, db, rid, session_id, seq, ...)` shape,
emits the `ChatEvent` first, then upserts the column; DB failures
are `warn!`-logged and swallowed (the agent loop must NEVER break
on a trace write — same contract as `record_*_audit`).

**Per-turn token worker gate (RULE-A-015 alignment).** The
`upsert_turn_trace_token` call lives **inside** the `if !skip_persist
{ ... }` block that also wraps `update_last_turn_usage`. Worker
subagent turns reuse the parent's `session_id` + `seq`; without
the gate, a worker's `Done{usage}` would overwrite the parent's
trace row token. By reusing the same gate, the parent token and
worker token stay in lockstep with the snapshot columns on the
`sessions` table.

**The other 3 hooks intentionally do NOT gate on `skip_persist`.**
C3 compaction and C2 soft hint are rare in worker paths (worker
turns typically have lower message volume and are less prone to
loop detection); workflow breadcrumb is suppressed in workers by
the `append_workflow_breadcrumb` S-B guard before the trace call.
Mixing worker trace rows into the parent's `turn_trace` is the
documented MVP behavior (see design §7 risk); an `is_worker` column
is Phase 2 OOS.

**`PermissionContext.turn_seq` is the audit-alignment hook.**
Inside the turn loop, the agent loop updates `permission_ctx.turn_seq
= Some(seq)` at the top of each turn. `record_audit` reads
`ctx.turn_seq` and passes it to `record_audit_event`, so audit rows
land in the same turn group as the trace row. Outside the turn
loop (`commands/question.rs` resolve_* handlers,
`db::sessions::edit_user_message`, etc.) the call site passes
`None` and the audit row stays un-grouped. This is a single,
explicit, grep-able seam — preferred over a thread-local turn
context (would not match Rust idiom).

## Pattern: Group-chat transcript view (08-04-group-chat-orchestration-rewrite)

**Problem**: `run_group_chat_loop`(群聊编排)reloads the full DB history
and feeds it verbatim into every speaker's `run_chat_loop`. Two defects
follow: (1) the previous speaker's persisted `tool_result` becomes the
next speaker's tail user message and is re-persisted (see the entry
invariant warning above → 400 death-loop); (2) a participant sees the
moderator's `nominate_speaker` / `end_discussion` tool interaction and
mistakes itself for the moderator (DB evidence: a participant's thinking
was "I need to respond as the moderator").

**Solution** — give each speaker a purpose-built transcript view, and
rely on the D-D entry guard (above) instead of per-persist-site patches.
Since 08-07-group-chat-role-history-isolation, the view is a **per-role
isolated history** (`role_history`); the pre-isolation `participant_view`
only stripped arbitration pairs and left every speaker's assistant rows
(including thinking + signature) in one context — the 多身份 assistant
串台 root cause:

```rust
// group_chat_prompts.rs (08-07-large-file-splitting: prompt/role_history 纯函数拆出)
// View (every speaker, moderator + participants): role_history builds
// an isolated LLM context — ONLY the speaker's own assistant rows
// verbatim (incl. thinking + signature, Anthropic round-trip safe) +
// other speakers' remarks rewritten as role:user; other speakers'
// thinking / tool pairs (incl. the moderator's arbitration pairs) are
// dropped (tool results are NOT shared).
let full = if round == 0 { messages.clone() }          // tail = new human msg
           else { reload_messages(&db, &session_id).await };
let history = role_history(&full, current_role);       // "moderator" | participant.name
run_chat_loop(/* … */ history, /* … */);
```

**Key contracts**:

1. **Per-role history isolation (08-07-group-chat-role-history-isolation
   R1)**: `role_history(full, current_role)` is a one-pass state machine
   over the reloaded transcript. A speaker sees ONLY its own assistant
   rows verbatim (`role:assistant` + all blocks, incl. thinking +
   signature — the Anthropic round-trip contract), other speakers'
   utterances rewritten as `role:user` (single Text block, content
   WITHOUT `@` prefix, `speaker` field preserved — attribution is the
   wire layer's job: Anthropic `apply_speaker_prefix` adds `@name:`,
   OpenAI fills the native `name` field; a content-embedded `@` would
   double-prefix). Other speakers' Thinking/RedactedThinking blocks are
   DROPPED (their signatures are bound to *their* generation context)
   and their tool_use↔tool_result pairs stripped whole (tool results
   are NOT shared — relayed only via text remarks). The moderator's
   arbitration pairs are just another "other speaker's tool pair" from
   a participant's view; the moderator keeps its own (跨轮连贯). The
   strip is **atomic per pair** (llm-contract.md §469): no orphan
   `tool_use` / `tool_result` survives; persisted pairs are adjacent in
   `full` (one speaker turn), so a one-pass state machine suffices.
2. **Scope via the shared turn-state**: participants pass
   `Some(turn_state)` (same Arc as the moderator) so the D-D guard's
   `group_chat_state.is_some()` holds for them too — otherwise the
   round-0 human message would be re-persisted. Arbitration safety is
   unaffected: `group_chat_tool_defs(.., false)` does not list the
   arbitration tools for participants, so the interception branch can
   never fire.
3. **Moderator runs `max_turns=Some(1)` (single turn, 08-04 follow-up)**:
   each moderator round is exactly ONE `provider.send` — remark +
   `nominate_speaker` / `end_discussion` `ToolCall` in one stream, then
   the turn ends right after the tool_result (no second send). The
   earlier `Some(3)` burned a second call on first-person arbitration
   filler ("已把话筒交给 X，等待发言…") which weak participants
   misread as their own voice (identity confusion — DB session
   `b144cc2a`, seq 3-4); single-turn removes it. A mock script must
   script one send per moderator tool round.
4. **Reload is retained (D-B)**: `run_chat_loop` returns `()`; a reload
   between speakers is the only resync. It is safe only because the
   entry guard prevents re-persisting already-persisted rows.
5. **D-D guard speaker 短路 (08-07-group-chat-role-history-isolation
   R2.5, P0-1/P0-3)**: inside `group_chat_state.is_some()` scope, a
   tail user row carrying `speaker` is treated as already persisted and
   skipped — such a row can ONLY be a `role_history` rewrite product
   (group-chat human prompts, tool_results and synthetic tool_results
   all carry `speaker == None`, so the signal cannot misfire); the seq
   anchors on the tail-most user row and `last_user_snapshot` returns
   `None` so the at_file `@file` injection pass is skipped for rewrite
   rows (they are another speaker's remark, not a human input). Without
   this, every rewrite row would be re-persisted each round → DB
   pollution + frontend ghost rows.
6. **Identity-guard prompt (08-04 follow-up)**: the wire-layer speaker
   label (OpenAI `name` / Anthropic `@name:` prefix) is NOT enough —
   weak models ignore it and adopt the moderator's first-person voice
   (DB evidence: participant M3 replied as `@moderator:` and wrote "I am
   prompting the conversation"). `participant_system_prompt` therefore
   appends an explicit role-boundary block ("The moderator's messages are
   NOT yours", "never start your reply with your OWN name or role") to
   BOTH the persona and the default template; `moderator_system_prompt`
   carries the same no-self-label rule. Do NOT showcase an `@`-prefixed
   example in the guard block — naming `@moderator:` self-primed weak
   models into writing `@moderator:` / `@M3:` self-labels (DB sessions
   `2bbc0d55` / `7bb0c351` show `@M3:  @M3:  @D4F`-style noise).
   @-mentioning ANOTHER participant in the reply body is allowed and
   desirable ("@D4F，你说得对…"). Moderator single-turn (bullet 3)
   removes the identity-confusing filler text at the source.
7. **Terminal signal (08-04 follow-up "终止事件 + 逐轮流式")**: the
   orchestrator shares ONE `request_id` across every inner
   `run_chat_loop`; the frontend cannot know when the discussion has
   actually ENDED from the inner per-speaker `Done`s. The orchestrator
   therefore emits a dedicated terminal
   `Done { stop_reason: "group_chat_end" }` after the outer loop ends
   (NOT on cancel — the cancelled inner turn's `Done { cancelled }` is
   the terminal there). The frontend keeps the request alive across the
   inner `Done`s and only finalizes on `group_chat_end` / `cancelled`.
8. **Live speaker identity (08-04 follow-up "实时 speaker 标识")**: the
   per-speaker wire events (`Delta` / `Done`) carry no speaker, so the
   orchestrator emits `ChatEvent::Speaker { speaker }` right before
   each inner turn ("moderator" or the participant name). The frontend
   stashes it (`req.pendingSpeaker`) and the next `start` stamps it on
   the freshly-pushed placeholder's `speaker` field — the existing
   MessageItem speaker chip then renders the name live. `Speaker` is
   orchestrator-only: the agent loop's per-event stream match drops it
   defensively if a provider ever re-emits it. Test: the integration
   test asserts one `Speaker` event per inner turn, in turn order.
9. **Tool whitelist, not blacklist (08-07-group-chat-toolset-and-identity R1)**:
   group-chat speakers receive ONLY `group_chat_tool_defs(tool_defs,
   is_moderator)` — a whitelist of research tools
   (`read_file`/`grep`/`glob`/`list_dir`/`web_fetch`) shared by both
   roles, plus the two arbitration tools for the moderator. This replaced
   the pre-R1 `participant_tool_defs` blacklist ("strip the two
   arbitration tools"), which leaked `use_skill` / `update_checklist` /
   `shell` / `write_file` into group chat. DB session `8be4687f` showed a
   participant abusing `use_skill` (hallucinated
   `"group-chat-director"` — no `<available-skills>` block is injected
   under `system_prompt_override`) and `update_checklist` (self-built
   speaker rotation) to hijack the moderator. The whitelist is
   **exhaustive**: a newly added `builtin_tools` entry does NOT enter
   group chat unless explicitly added to the whitelist, so this leak
   class can't recur. Tests:
   `group_chat_tool_defs_moderator_has_research_plus_arbitration` +
   `group_chat_tool_defs_participant_has_research_only`.
10. **No nominate-streak counter (08-07-group-chat-toolset-and-identity R2)**:
   when the moderator ends a turn without calling
   `nominate_speaker`/`end_discussion`, the orchestrator simply retries
   the moderator turn (`continue`) — there is NO "stuck" detection, NO
   `moderator_nudge`, NO `MAX_NO_NOMINATE_STREAK`. The only bound is
   `MAX_ORCHESTRATION_ROUNDS` (→ terminal `Done { stop_reason:
   "max_rounds" }`). This replaced the 08-07 streak mechanism, which
   couldn't distinguish "moderator is legitimately researching" (DB
   `8be4687f` seq 1/3/5) from "stuck" and mis-killed the former.
   Pacing guidance now lives in the moderator prompt (contract: the
   "research is a MEANS, hand the floor after a short look" block), not
   in a dynamically-appended nudge. The `moderator_stuck` stop_reason is
   gone; the finalize whitelist is `{ group_chat_end, cancelled,
   max_rounds }`.

**When to apply**: any orchestrator that drives multiple `run_chat_loop`
calls over one shared session — give each callee a per-role isolated
history (`role_history`), never the raw reload, never patch the persist
sites heuristically (the D-D guard speaker 短路 is the single seam), and
scope each callee's tool list to a whitelist (never the full
`builtin_tools`).

**Tests**: `.trellis/spec/backend/…` → `app/src-tauri/src/agent/tests_group_chat.rs`
(integration: full multi-round flow, no `ChatEvent::Error`, one
`tool_result` row per `tool_use_id`, participant histories free of
arbitration blocks, system-prompt identity, one `group_chat_end` +
one `Speaker` per turn) + `role_history` unit
tests in `tests_group_chat_prompts.rs` (own rows verbatim incl. signature
round-trip, other-speaker rewrite as user without `@` prefix +
`speaker` preserved, other thinking dropped, other tool pairs stripped
whole, own tool pairs preserved, human prompt preserved, arbitration
dropped for participants / kept for the moderator, multi-turn same-role
preserved, wire no-double-prefix) + `dd_guard_*` unit tests in
`chat_loop.rs` (speaker-carrying tail skipped in group-chat scope,
classic-chat behavior unchanged, rewrite rows skip at_file injection,
seq anchors on the tail-most user row) +
`app/src/stores/streamController.test.ts` (group-chat streaming:
inner `done` doesn't finalize, `start` pushes a new placeholder, the
