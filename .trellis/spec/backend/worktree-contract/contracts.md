<!-- Moved from worktree-contract.md 2026-09-19 (doc-split) -->

###3. Contracts

#### Tool result envelope (LLM boundary)

- The tool internals (`app/src-tauri/src/tools/*.rs`) return `String`
 unchanged. None of the existing60+ tool unit tests are modified.
- At the LLM-facing boundary in `agent::chat::chat`, the `ToolResult` event
 payload AND the `ContentBlock::ToolResult.content` stored in the DB
 are wrapped via `tool_result_envelope(content, ctx.worktree_path)`.
- Wire / DB shape: `{"result": "<original content>", "cwd": "<worktree_path>"}`.
- `cwd` is the canonical on-disk path the tool actually ran against
 (`ctx.worktree_path` if the session has a worktree, else
 `project.path`).
- Frontend display: `extractToolResultDisplay(content)` leniently parses
 the envelope and returns the unwrapped `result` field. Falls back to
 the raw string for pre-follow-up data, non-JSON, missing `result`
 key, or parse errors. The wire format to the LLM is preserved.

#### System event injection

- After successful attach/detach/delete, the backend calls
 `db::insert_system_event` with the text format:
 - attach: `[worktree event] attached: <path> on branch session/<id>`
 - detach: `[worktree event] detached from <path> (changes preserved on branch session/<id>)`
 - delete: `[worktree event] deleted: branch session/<id> and dir <path> removed`
- The row is stored with `role='user'` and a structured `metadata`
 marker (`{kind: "worktree_event", event: "attached" | "detached" | "deleted"}`).
- The LLM's next chat loads messages and sees the event in history.
- Frontend `controller.refresh(sessionId)` evicts the cached messages
 and re-loads from DB so the UI bubble list also shows the event
 (rendered as a regular user-role message with the `[worktree event]`
 prefix). A future PR may add a dedicated info-badge rendering.

#### System prompt (Step4 follow-up Bug3)

- The agent loop builds a **session-grounding system prompt** in
 `agent::system_prompt::build_system_prompt(session, project, ctx_root, head_sha)`
 and passes it via `chat_stream_with_tools(config, Some(prompt), ...)`.
 Pre-fix, the request body's `system` field was hard-coded to `None`;
 the LLM honestly answered "no" when asked "does your system prompt
 mention you're in a worktree" because the field was empty. The
 `[worktree event]` user-role messages live in the conversation
 history (see "System event injection" above) and describe
 *transitions*, but the system prompt is what tells the LLM the
 *current* state.
- The prompt is constructed **once per `chat` invocation**, before
 the `for turn in1..=MAX_TURNS` loop. The worktree state can't
 change between turns of the same agent loop (destructive worktree
 commands have a cancel hook that aborts in-flight chats), so
 rebuilding per-turn would be wasteful.
- The prompt shape (always exactly these lines):

```
You are a coding agent. You have access to tools (read_file, write_file,
 edit_file, shell, grep, glob, list_dir). All file paths in tool inputs
 are relative to the session's working directory.

Session context:
- Session ID: <session.id>
- Project: <project.name> (<project.path>)
- Working directory: <ctx_root>
- Worktree: <state phrase>
- Available tool result envelope: {"result": "<content>", "cwd": "<worktree_path>"}
 — `cwd` tells you which root the tool ran against when worktree transitions
 happen mid-session.
```

- `<state phrase>` is one of:
 - `Active`: `ACTIVE on branch 'session/<session.id>' (HEAD <short_sha>)`
 - `Detached`: `DETACHED — was on branch 'session/<session.id>' (HEAD <short_sha>), currently in project root`
 - `None`: `NONE — running in project root`
 - Non-git project (regardless of `worktree_state`): `N/A — non-git project`
- `<short_sha>` = first7 chars of HEAD commit SHA, looked up via
 `lookup_head_sha(ctx_root)`. Best-effort: a non-git path or empty
 repo returns a placeholder (`"not a git repo"` / `"no commits yet"`).
- **Tool result envelope vs system prompt — division of labor**:
 - System prompt is the **persistent declaration** of the session's
 grounding (built once per chat invocation, repeated each request).
 - Tool result envelope's `cwd` field (see "Tool result envelope"
 above) is the **runtime data point** confirming what cwd a
 specific tool actually used. Both are needed: the prompt sets
 the model's mental model; the per-tool `cwd` confirms it after
 each call so the model can detect drift.
- **Privacy / surface area**: only `session.id`, `project.name`,
 `project.path`, `ctx_root`, and the short HEAD SHA are emitted. No
 user messages, tool inputs, or DB rows are echoed into the prompt.
 Future additions (e.g. "the project's main branch is `main`") go in
 `build_system_prompt`; do not scatter prompt-building across the
 agent loop.

#### In-flight cancel hook

- `chat` spawn fills `session_active_request: session_id -> request_id`.
- `CancellationGuard` (Drop on every agent-loop exit path) removes the entry.
- The3 destructive paths (`delete_session` / `detach_worktree` /
 `delete_worktree`) call `cancel_inflight_for_session` at entry:
1. Look up the active `request_id` for `session_id`.
2. If found, fetch the matching `CancellationToken` from
 `cancellations` and `.cancel()`.
3. Take the matching exit `oneshot::Receiver` out of `inflight_exits`
 (single-consumer) and return it.
4. The caller `await_inflight_exit(rx, label)` — awaits the signal
 (10 s timeout backstop) before the destructive work runs, so the
 agent loop's in-flight tool can't write into a just-deleted worktree.
- **Ordering invariant (RULE-E-005, updated 2026-06-15)**: cancel →
 **await agent-loop exit** → destructive execute → system event
 injection (if any). The cancel token only *sets* the flag; the agent
 loop checks it at stream-event boundaries and *after* the current
 tool (`chat_loop.rs::run_chat_loop`), so one in-flight tool may still run before
 the loop returns. Without the await, that tool could write into a
 just-deleted worktree (ENOENT / panic / orphaned fingerprint). The
 destructive caller `await_inflight_exit(rx, label)` (10 s defensive
 timeout backstop) closes the race; the system event still lands BEFORE
 the LLM's next turn.
- Frontend guard: `detach` / `delete worktree` menu items are
 `:disabled="chatStore.isStreaming"`. This is a UX guard, not the
 safety net — the backend cancel hook covers the in-flight IPC case.

#### Rehydrate + outbound payload

- The system event is a `role='user'` message, so `toPayloadContent`
 passes it through as a text block. No new block type needed.
- Order: system events appear at the position of their original
 insertion (typically after the latest assistant turn). Anthropic
 accepts interleaved user messages without re-ordering.

#### Synthetic `tool_result` on cancel (BUG FIX2013 tool_use orphan)

- **Trigger**: the agent loop's cancel branch fires after one or more
 `tool_use` blocks have been streamed and accumulated but before any
 tool has executed (PR5 `Stop`, `attach_worktree`'s in-flight cancel
 hook, network drop, or any path that returns from the agent loop
 with `cancelled = true` and `tool_calls` non-empty).
- **Required behavior**: the cancel branch must persist a synthetic
 `user`-role `ChatMessage` carrying one `ContentBlock::ToolResult`
 per `(id, name, _input)` triple, then emit `done { stop_reason:
 "cancelled" }` and return. Pre-fix, the cancel branch returned
 immediately after persisting the assistant turn, leaving the DB
 with an orphan `tool_use` and no matching `tool_result` — the
 next `send()` then built a malformed history and the Anthropic API
 returned2013 ("tool call result does not follow tool call").
- **Synthetic block shape** (must match the wire contract):
 - `type: "tool_result"`
 - `tool_use_id: <id>` (mirrors the corresponding `tool_use` block's id)
 - `content: "Tool execution was interrupted: the user stopped the
 request or the session was cancelled before the tool could run.
 The tool <name> did not run."` (English + tool name per the
 `HACKING-llm.md` "陷阱3" decision)
 - `is_error: true` (the Anthropic schema's strong signal that the
 tool failed; combined with the content wording this usually
 causes the model to retry the tool_use on the next turn rather
 than reason on the empty result)
- **Helper**: `build_synthetic_tool_result_message(tool_calls: &[(String, String, serde_json::Value)]) -> ChatMessage` in `agent::helpers.rs`. Pure function over `tool_calls`; no DB / Tauri deps. Extracted as a free function (not inlined in the cancel branch) so the invariants are unit-testable in isolation.
- **Order invariants** (must hold after this fix lands):
1. `assistant(tool_use)` is persisted FIRST, then the synthetic
 `user(tool_result)` is persisted. Persisting in the other order
 would still be malformed (Anthropic rejects `tool_result` blocks
 not preceded by a matching `tool_use` in the immediately-prior
 assistant message).
2. `seq` is strictly increasing across the two rows.
3. The synthetic message is NOT emitted as a `tool:result` Tauri
 event (the event is for UX feedback on actually-executed tools;
 no tool ran here, and emitting it would confuse the frontend's
 streaming pipeline).
4. `stop_reason: "cancelled"` is still emitted on `done` — the
 frontend's cancel path doesn't change.

#### Orphan tool_use repair on rehydrate (BUG FIX2013, frontend side)

- **Trigger**: any historical session row in the `messages` table
 where an `assistant` turn contains `tool_use` blocks with no
 matching `tool_result` in the immediately-following `user` turn.
 These can predate the synthetic-tool_result fix above (cancel /
 network drop in the old code) and can also come from a future
 bug that re-introduces the gap.
- **Required behavior**: the frontend's `rehydrateMessages` in
 `app/src/stores/streamController.ts` must splice in a synthetic
 `user`-role `ChatMessage` with one `tool_result` block per orphan
 `tool_use` id, immediately after the orphan assistant message.
 Without this, the next `send()` pushes a malformed history and
 the API returns2013.
- **Detection rules** (applied to the post-merge-step message array):
 - An `assistant` message is considered to have an orphan `tool_use`
 when `toolCalls[i].id` is not in the union of:
1. The assistant's own `toolResults[*].toolUseId` (set by the
 merge step from a later user message).
2. The immediately-following `user` message's
 `toolResults[*].toolUseId`.
 - Loop direction: **reverse scan** (i = out.length-1 down to0)
 so that `splice(i+1,0, syntheticMsg)`'s index shift doesn't
 affect the next iteration.
- **Synthetic block shape on rehydrate** must match the backend's
 cancel-path synthetic exactly (same wording, same `is_error: true`,
 same tool name in content). The two repair paths must stay in lockstep;
 if they ever diverge, the LLM will see inconsistent recovery
 behavior depending on whether a session was repaired by the
 backend or the frontend.
- **Wire-effect**: the spliced synthetic message participates in
 `toPayloadContent` exactly like a real user-role `tool_result`
 message — `assistant(tool_use)` and `user(tool_result)` are
 emitted as two adjacent messages, satisfying the Anthropic
 contract.
- **Tests required** (locked in `app/src/stores/streamController.test.ts`):
 - Orphan `tool_use` with no following user → spliced synthetic
 - Multiple orphan `tool_use` in the same assistant → all repaired
 - Paired `tool_use` + `tool_result` (normal case) → NOT touched
 - Orphan at end-of-array (no following user at all) → spliced
 - Existing merge step (user.toolResults → preceding assistant) is
 preserved by the refactor

#### In-memory must mirror DB on send completion (BUG FIX2026-06-08,2013 reappears in normal-completion path)

- **Trigger**: any `chat` IPC that **completes** (not just cancels)
 while the agent loop ran at least one tool. The pre-fix behavior
 kept the in-memory `streamController.messagesBySession` cache
 alive after `done` so the user could keep viewing the session;
 the in-memory shape is the *streaming-accumulation* shape
 (single `assistantMsg` placeholder that absorbed every `delta`
 / `tool_call` / `tool_result` / `thinking_delta` event across
 all turns), while the DB shape is one assistant message per
 agent-loop turn (per `agent::chat::chat`).
- **Failure mode**: a subsequent `send()` for the same session
 hits `ensureLoaded`'s in-memory fast path, the cache is the
 accumulation shape, `toPayloadContent` for `assistant` role
 emits `tool_use` (per the Anthropic contract: `tool_result`
 blocks only go on user-role messages), and the next wire
 message after the assistant turn is a user-text prompt with
 no `tool_result` in between. Anthropic Messages API returns
2013 ("tool call result does not follow tool call").
- **Required behavior**: `streamController.finalizeRequest` (the
 function the `done` / `error` / catch-error paths all route
 through) must:
1. `evict(sessionId)` — clears `messagesBySession`,
 `loadedFromDb`, and `pinnedSessions` for the session, so
 the next `ensureLoaded` takes the re-load-from-DB path and
 gets the per-turn split shape.
2. `useChatStore().invalidateDiff(sessionId)` — clears the
 worktree diff cache for the same session, so the worktree
 chip's "diff (N)" counter re-fetches on the next read
 (after a `git commit` ran inside the worktree, etc.).
- **Why both, paired**: the in-memory shape and the diff cache
 are owned by different stores (`streamController` vs `chat`),
 but they're both stale on send completion for the same root
 reason. A refactor that only calls one of the two would
 silently re-introduce one of the two bugs above. The
 `streamController.test.ts` `finalizeRequest` describe block
 has a `both actions fire on the same finalizeRequest call
 (paired invariant)` test that locks this.
- **Relation to the cancel-path fix** (`Synthetic tool_result
 on cancel` above, c35c384): the two fixes address
 *different*2013 paths. The cancel-path fix prevents the DB
 from developing an orphan `tool_use` when the user stops
 mid-stream. The in-memory-mirror fix prevents the wire-format
 history from having an apparent orphan `tool_use` even when
 the DB is fully self-consistent. Both must stay in place —
 removing either re-opens2013 under a different repro path.
- **Tests required** (locked in
 `app/src/stores/streamController.test.ts`,
 `finalizeRequest` describe block):
 - `evicts the in-memory message buffer and unloads from DB
 cache` (after `done` / after `error`)
 - `invalidates the chat store's diff cache for the same
 session` (after `done` / after `error`)
 - `both actions fire on the same finalizeRequest call (paired
 invariant)` — paired test, not two independent tests

###4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `attach_worktree` on non-git project | Reject: `"project <name> is not a git repository"` |
| `attach_worktree` while project root has uncommitted changes | Reject: `"project root has uncommitted changes; commit or stash before attaching"` |
| `attach_worktree` while worktree_path already exists | Reject: libgit2 error (mirrors PR1 behavior) |
| `detach_worktree` while `worktree_state != 'active'` | Reject: `"no active worktree to detach"` |
| `detach_worktree` while worktree has uncommitted changes | Reject: `"worktree <path> has uncommitted changes; commit/stash before detach"` |
| `delete_worktree` while `worktree_state != 'active'` | Reject: same as detach |
| Destructive path with active in-flight chat | Cancel in-flight first, then proceed |
| `extractToolResultDisplay` receives non-JSON string | Fallback: return raw string |
| `extractToolResultDisplay` receives JSON without `result` key | Fallback: return raw string |
| `insert_system_event` fails (e.g. DB locked) | `tracing::warn!`; the destructive operation has already succeeded — the system event is best-effort |
| `cancel_inflight_for_session` finds no active request | Returns `None`; destructive proceeds |
| `cancel_inflight_for_session` finds request_id but no token (rare race) | `tracing::warn!`; destructive proceeds |
| `lookup_head_sha` on a non-git `ctx_root` | Returns `"not a git repo"`; prompt embeds the placeholder |
| `lookup_head_sha` on an empty (no-commits) repo | Returns `"no commits yet"`; prompt embeds the placeholder |
| `build_system_prompt` for a non-git project | Worktree line is `N/A — non-git project` regardless of `worktree_state` |
| `chat_stream_with_tools` called with `system: None` | Request omits the `system` field (skip_serializing_if=None); backward compat with the pre-fix call sites |
| `chat` cancel branch with empty `tool_calls` | Returns immediately (no synthetic message persisted; `seq` not incremented) |
| `chat` cancel branch with non-empty `tool_calls` | Persists `assistant(Blocks{tool_use...})` THEN synthetic `user(Blocks{tool_result...})`; both rows must have strictly-increasing `seq` |
| Synthetic `tool_result` content / `is_error` field | `serde_json::Value` round-trip preserves `type: "tool_result"`, `tool_use_id`, `content` (with tool name), `is_error: true` (the `is_false` skip filter only drops `false`) |
| Rehydrate orphan `tool_use` (frontend) | Spliced synthetic message has `role: "user"`, `content: ""` (no text), `toolResults: [{toolUseId, content, isError: true}]`; message's `id` is `<assistant.id>-orphan-repair` |
| Rehydrate paired `tool_use` (frontend) | Output array length unchanged (no synthetic inserted) |

