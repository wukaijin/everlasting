# Worktree Contract — attach/detach/delete + Cancel + System Prompt

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后) + 2026-08-10 (3 个 worker Pattern 拆出子文件)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + 反模式汇总(Extended Thinking 已拆至 [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md))
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) (本文) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **worker Pattern 子文件**(2026-08-10 拆出):
> - [worktree-contract/worker-variant.md](./worktree-contract/worker-variant.md) — `create_worker` / `destroy_worker` 变体(L3b PR1)
> - [worktree-contract/worker-sweep.md](./worktree-contract/worker-sweep.md) — 启动期 stale worker worktree sweep(L3b PR3)
> - [worktree-contract/lazy-auto-attach.md](./worktree-contract/lazy-auto-attach.md) — merge 后懒 attach(06-30 follow-up)
>
> **何时读本文**:涉及 `attach_worktree` / `detach_worktree` / `delete_worktree` / `cancel_inflight_for_session` / `build_system_prompt` / synthetic `tool_result` 时。worker 隔离/sweep/merge-attach 见上述子文件。

---

## Scenario: Worktree State Transparency + LLM Cancel (Step4 Follow-up,2026-06-08)

###1. Scope / Trigger

- Trigger: Decoupling worktree from session lifecycle. After this change the
 user can `attach_worktree` / `detach_worktree` / `delete_worktree` mid-session
 (previously worktree was auto-created at session create and never changed).
 Three risks emerged that need code-spec depth:
1. **LLM confusion across worktree views** — the same session's tool results
 can come from two different on-disk roots (worktree vs project root) within
 one chat. Without explicit signalling the LLM will see "I read S1" then
 "I read S2" with no reason for the change.
2. **Stale conversation history** — if the frontend keeps its cached messages
 after a worktree transition, the LLM's next turn won't see the new
 `[worktree event]` system event and will reason on outdated context.
3. **Destructive path racing an in-flight chat** — `delete_session` /
 `detach_worktree` / `delete_worktree` can run while the agent loop is
 still streaming. Without a cancel hook the in-flight turn's `INSERT`
 fails on a deleted row, or a tool call writes to a worktree that no
 longer exists.
- Why code-spec depth: mandatory — new Tauri command surface, new message
 shape on the wire (envelope), new row kind in the `messages` table
 (system event), and a new cross-layer ordering constraint
 (cancel → destructive → system event → next LLM turn).

> **分篇**(2026-09-19):本文保留 §1 Scope、§6 Tests Required 与尾部三个 Pattern stub(Worker Variant / Sweep / Lazy Auto-Attach);§2-§5、§7 已按 tool-contract 模式拆至 `worktree-contract/` 子目录(原锚点以 stub 保留)。

###2. Signatures

> **已拆出**(2026-09-19 doc-split):完整签名(Tauri commands / DB schema / helpers / AppState field)见 [`worktree-contract/signatures.md`](./worktree-contract/signatures.md)。

###3. Contracts

> **已拆出**(2026-09-19 doc-split):完整契约(tool result envelope / system event injection / system prompt / in-flight cancel hook / rehydrate / synthetic tool_result on cancel / orphan tool_use repair / in-memory mirror)+ §4 Validation & Error Matrix 见 [`worktree-contract/contracts.md`](./worktree-contract/contracts.md)。

###4. Validation & Error Matrix

> **已拆出**(2026-09-19 doc-split):见 [`worktree-contract/contracts.md`](./worktree-contract/contracts.md)。

###5. Good / Base / Bad Cases

> **已拆出**(2026-09-19 doc-split):完整用例见 [`worktree-contract/cases-and-wrong-vs-correct.md`](./worktree-contract/cases-and-wrong-vs-correct.md)。
###6. Tests Required

| Test | Asserts |
|------|---------|
| `tool_result_envelope_round_trip` | Output has exactly2 keys: `result` + `cwd` |
| `tool_result_envelope_handles_special_chars` | Newline / quote / backslash correctly JSON-escaped |
| `git::worktree::check_clean::rejects_modified` | Modified tracked file → `Err(GitError::Dirty)` |
| `git::worktree::check_clean::rejects_untracked` | Untracked file → `Err` |
| `git::worktree::check_clean::ignores_gitignore` | `.gitignore`d file does not change verdict |
| `db::set_worktree_state::active` | Row updated; `last_worktree_path` cleared or preserved per call |
| `db::set_worktree_state::detached` | Row updated; `last_worktree_path` set |
| `db::insert_system_event::role_user` | New row in `messages`; `role='user'`; content contains `[worktree event]` |
| `attach_worktree::uncommitted_project_root` | Returns `Err` with "uncommitted changes" message |
| `detach_worktree::uncommitted_worktree` | Returns `Err` with same shape |
| `lib::cancel_inflight_for_session::finds_active` | In-flight token cancelled; entry removed after `CancellationGuard` drop |
| `lib::cancel_inflight_for_session::no_active` | Returns `None`; proceeds silently |
| `vitest extractToolResultDisplay::unwraps_envelope` | `{"result":"X","cwd":"Y"}` → `X` |
| `vitest extractToolResultDisplay::falls_back_raw` | Plain string → same string |
| `vitest extractToolResultDisplay::handles_empty` | `""` → `""` |
| `vitest extractToolResultDisplay::handles_non_json` | `"not json"` → `"not json"` |
| `db::migration::backfill_legacy_active` | Pre-follow-up sessions with `worktree_path IS NOT NULL` get `state='active'` |
| `lib::build_system_prompt_active_worktree` | Active state → prompt contains `ACTIVE on branch 'session/<id>'`, HEAD SHA, and worktree path as cwd |
| `lib::build_system_prompt_detached_worktree` | Detached → `DETACHED — was on branch ... currently in project root`; cwd = project root |
| `lib::build_system_prompt_no_worktree` | None → `NONE — running in project root`; no branch / SHA leakage |
| `lib::build_system_prompt_non_git_project` | Non-git project → `Worktree: N/A — non-git project`; no `session/<id>` reference |
| `llm::client::chat_request_system_field_serializes_when_some` | `ChatRequest { system: Some(s), .. }` serializes with the `system` field present at the top level |
| `lib::synthetic_tool_result_message_mirrors_tool_calls` | One `tool_call` → one `ToolResult` block with matching `tool_use_id`, `is_error: true`, tool name in content |
| `lib::synthetic_tool_result_message_preserves_order_for_multi_call` |3 `tool_call`s →3 `ToolResult` blocks in the same order, all `is_error: true` |
| `lib::synthetic_tool_result_message_empty_when_no_tool_calls` | Empty input → empty `Blocks` (no stray user message) |
| `lib::synthetic_tool_result_message_serializes_to_anthropic_wire_shape` | `serde_json::to_string(msg)` round-trip produces `{"role":"user","content":[{"type":"tool_result","tool_use_id":"X","content":"...","is_error":true}]}` |
| `vitest rehydrateMessages::splices_synthetic_user_tool_result_after_orphan_assistant` | Orphan `tool_use` → synthetic `user(tool_result)` spliced in at `i+1` |
| `vitest rehydrateMessages::does_not_splice_when_paired` | Normal `assistant(tool_use)` + `user(tool_result)` pair → no extra synthetic |
| `vitest rehydrateMessages::repairs_every_orphan_in_same_assistant` | Multi-call orphan → all `tool_use` ids covered by the spliced synthetic |
| `vitest rehydrateMessages::synthetic_id_is_unique` | Synthetic message's `id` ≠ the orphan assistant's `id` (won't collide with `send()` placeholder) |
| `vitest rehydrateMessages::orphan_at_end_of_array_repaired` | Last-message orphan → synthetic still spliced in (loop must not underflow) |
| `vitest rehydrateMessages::empty_messages_array_does_not_crash` | Defensive: `load_session` returning `[]` rehydrates to `[]` |
| `vitest rehydrateMessages::merge_step_preserved` | Pre-existing merge step (`user.toolResults` → preceding `assistant.toolResults`) is not regressed by the orphan-repair refactor |

Total:~17 new tests added in this round; backend suite now180+ tests,
frontend vitest44+. As of step4 follow-up:**182 backend tests +44 frontend
vitest =226 tests pass**. Bug3 (system prompt) adds **+5 backend tests**
(`193 total` with the Bug1/2 self-heal counts not included here — see
the round-2 task PRD for those).

#### Frontend

- `pnpm build` (vue-tsc strict) must pass. The4 worktree actions
 (`attachWorktree` / `detachWorktree` / `deleteWorktree` /
 `controller.refresh`) live in `app/src/stores/chat.ts` and
 `app/src/stores/streamController.ts`; any new field on `SessionSummary`
 must round-trip end-to-end.
- Manual smoke test (acceptance A29):
1. `cd app && pnpm tauri dev`.
2. Open a session, attach worktree, observe the chip flips to
 "diff (0) ▼" and the dropdown shows copy + detach + delete.
3. Send a chat, observe the LLM's next response references the
 worktree path (the `cwd` it sees in the tool result envelope).
4. Detach, send another chat, observe the response references the
 project root and the bubble list shows the
 `[worktree event] detached from ...` message.
5. While streaming, click "detach worktree" — observe the disabled
 state, and verify that a quick race (IPC mid-flight) still
 results in a clean cancel via the backend hook.


###7. Wrong vs Correct

> **已拆出**(2026-09-19 doc-split):完整对照见 [`worktree-contract/cases-and-wrong-vs-correct.md`](./worktree-contract/cases-and-wrong-vs-correct.md)。

## Pattern: Worker Worktree Variant (`create_worker` / `destroy_worker`, L3b PR1, 2026-06-27)

> **已拆出**(2026-08-10 doc-split):完整 Pattern(签名 / 与 session 变体差异 / behavior matrix /
> Tests Required)见 [worktree-contract/worker-variant.md](./worktree-contract/worker-variant.md)。

## Pattern: Worker Worktree Sweep (L3b PR3, 2026-06-27)

> **已拆出**(2026-08-10 doc-split):完整 Pattern(签名 / behavior / Tests Required / startup
> wiring / env override)见 [worktree-contract/worker-sweep.md](./worktree-contract/worker-sweep.md)。

## Pattern: Lazy Auto-Attach on Merge (06-30 follow-up)

> **已拆出**(2026-08-10 doc-split):完整 Pattern(helper contract / layering / IPC return value /
> validation & error matrix)见 [worktree-contract/lazy-auto-attach.md](./worktree-contract/lazy-auto-attach.md)。
