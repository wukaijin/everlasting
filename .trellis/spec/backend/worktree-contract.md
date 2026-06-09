# Worktree Contract — attach/detach/delete + Cancel + System Prompt

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + Extended Thinking + 反模式汇总
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) (本文) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `attach_worktree` / `detach_worktree` / `delete_worktree` / `cancel_inflight_for_session` / `build_system_prompt` / synthetic `tool_result` 时。

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

###2. Signatures

#### New Tauri commands (`app/src-tauri/src/commands/worktree.rs`)

```rust
#[tauri::command]
async fn attach_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn detach_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn delete_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;
```

Each is registered in `invoke_handler!` and exposed to the frontend.

#### New DB schema (`app/src-tauri/src/db/sessions.rs`)

```sql
ALTER TABLE sessions ADD COLUMN worktree_state TEXT NOT NULL DEFAULT 'none';
ALTER TABLE sessions ADD COLUMN last_worktree_path TEXT;
-- One-shot backfill on startup (idempotent, see migration helper):
UPDATE sessions
SET worktree_state = 'active'
WHERE worktree_path IS NOT NULL
 AND worktree_state = 'none';
```

Valid `worktree_state` values (snake_case strings; serialized as
`#[serde(rename_all = "snake_case")]`):

| Value | Meaning | `worktree_path` | `last_worktree_path` |
|-------|---------|-----------------|----------------------|
| `"none"` | Never had a worktree, or worktree was deleted. | `NULL` | `NULL` (never used) or last value preserved |
| `"active"` | A worktree is currently bound to this session. | `Some(<path>)` | `NULL` (or previous value preserved) |
| `"detached"` | Had a worktree, now unbound. Directory may still exist on disk. | `NULL` | `Some(<previous path>)` |

#### New helpers

```rust
// app/src-tauri/src/git/worktree.rs
pub fn check_clean(repo_path: &Path) -> Result<(), GitError>;
// Uses libgit2 `Repository::open(repo_path)?.status()?` to detect modified
// tracked files + untracked files. Ignores .gitignore'd files.
// Rejects when status is non-empty.

// app/src-tauri/src/db/sessions.rs
pub async fn set_worktree_state(
 pool: &SqlitePool, session_id: &str, state: WorktreeState,
 last_worktree_path: Option<&str>,
) -> Result<(), sqlx::Error>;

pub async fn insert_system_event(
 pool: &SqlitePool, session_id: &str, text: &str,
) -> Result<(), sqlx::Error>;
// Inserts a row into `messages` with role='user' and content=text.
// seq = max(seq)+1 for the session.

// app/src-tauri/src/agent/helpers.rs
pub fn tool_result_envelope(content: String, worktree_path: &Path) -> String;
// Returns: {"result": "<content>", "cwd": "<worktree_path>"}
// Lives in agent::helpers (NOT in the tool modules) so the existing60+ tool
// unit tests are unchanged.

pub async fn cancel_inflight_for_session(
 cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
 session_active_request: &Arc<Mutex<HashMap<String, String>>>,
 session_id: &str,
) -> Option<String>;
// Returns the cancelled request_id, or None if no in-flight request.
```

#### New AppState field

```rust
struct AppState {
 // ... existing fields ...
 session_active_request: Arc<Mutex<HashMap<String, String>>>,
 // Maps session_id -> currently active request_id.
 // Inserted by `chat` on spawn; cleared by CancellationGuard on Drop.
 // Read by the3 destructive paths to find the request_id to cancel.
}
```

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
3. Return; the agent loop's `tokio::select!` notices and exits
 cleanly, which clears the map entry via the `CancellationGuard`.
- **Ordering invariant**: cancel → destructive execute → system event
 injection (if any). The system event must land AFTER the cancel
 completes (so the event reflects the post-cancel state) and BEFORE
 the LLM's next turn (so the LLM sees it).
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

###5. Good / Base / Bad Cases

#### Good: attach → LLM sees event + envelope

1. User clicks "attach worktree" in the chat panel.
2. Backend `attach_worktree` runs:
 - `cancel_inflight_for_session` (no-op, not streaming).
 - `check_clean(project_root)` passes.
 - `git::worktree::create` builds the worktree.
 - `set_worktree_state(sid, Active, None)` updates the row.
 - `insert_system_event` writes the `[worktree event] attached: ...` row.
 - Returns the new `SessionRow`.
3. Frontend `attachWorktree` action calls `controller.refresh(sid)` to
 evict + reload the cached messages (so the bubble list shows the event).
4. User sends a new message.
5. LLM loads messages, sees the event in history, then calls `read_file`.
6. Backend emits `ToolResult` with envelope `{"result": "...", "cwd": "<worktree_path>"}`.
7. LLM sees `cwd` and knows which root it just operated on.

#### Good: LLM aware of worktree (Step4 follow-up Bug3)

1. User has an active worktree on session `abc-123`. User sends:
 "where am I right now? am I in a worktree?".
2. Frontend `send()` posts the message; backend `chat` spawns the
 agent loop. Before the `for turn in1..=MAX_TURNS`:
 - `lookup_head_sha(worktree_path)` returns `"e3f4567"`.
 - `build_system_prompt(session, project, worktree_path, "e3f4567")`
 produces a prompt that includes:
```
- Working directory: /home/carlos/.local/share/everlasting/worktrees/<pid>/abc-123
- Worktree: ACTIVE on branch 'session/abc-123' (HEAD e3f4567)
```
3. `chat_stream_with_tools(config, Some(prompt), messages, tools)`
 sends the request body with the `system` field populated.
4. The LLM's reply correctly states "you are in a worktree at
 `/home/carlos/.local/share/everlasting/worktrees/.../abc-123`
 on branch `session/abc-123` (HEAD `e3f4567`)" — quoting the
 prompt verbatim if asked.
5. Pre-fix counterpart: with `system: None`, the LLM would answer
 "I don't see any worktree in my system prompt" — which is
 honest but useless. The `[worktree event]` row in history says
 "user told me X happened", which the LLM treats as user speech,
 not authoritative grounding.

#### Base: detach clean

1. `worktree_state = 'active'`, worktree is clean.
2. User clicks "detach worktree".
3. `detach_worktree` runs: `check_clean(worktree_path)` passes, then
 `set_worktree_state(sid, Detached, Some(previous_path))`.
4. `insert_system_event` writes the detach event.
5. Next tool call: `ctx.worktree_path` is now `project.path` (the
 fallback), so the envelope's `cwd` reflects project root.
6. LLM sees the event, knows the worktree is gone, and continues
 operating in project root.

#### Bad: ToolCallCard displays the envelope literally

1. Backend correctly emits envelope `{"result": "hello", "cwd": "/worktree"}`.
2. Frontend `ToolCallCard.vue` does `{{ result.content }}` directly.
3. User sees a JSON blob in the tool card instead of `hello`.
4. Fix: `ToolCallCard.vue` uses `extractToolResultDisplay(result.content)`
 for both `outputSize` and the rendered `<pre>`.

#### Bad: missing controller.refresh

1. `attachWorktree` action mutates the row but does NOT evict the
 cached messages.
2. User clicks send. `controller.startRequest` reads from
 `messagesBySession.get(sid)` (the cache, not the DB).
3. LLM's payload does not include the new system event.
4. LLM reasons on stale context, may try to `cd` into a worktree path
 it doesn't know exists, etc.
5. Fix: `controller.refresh(sid)` at the end of `attachWorktree` /
 `detachWorktree` / `deleteWorktree` actions in `chat.ts`.

#### Bad: missing cancel hook in delete_session

1. User clicks delete session.
2. `lib.rs::delete_session` immediately starts cleanup of
 shell outputs dir + worktree + DB row.
3. LLM is mid-stream; the agent loop is about to `INSERT` a
 `ContentBlock::Text` row referencing the about-to-be-deleted
 session_id.
4. INSERT fails with FK violation; the message is lost.
5. Fix: `cancel_inflight_for_session(sid)` at the top of
 `delete_session` (and the other two destructive paths).

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

#### Wrong: envelope added to a tool's return type

```rust
// BAD — modifies the tool's signature, breaks60+ tests
pub async fn execute(...) -> Result<serde_json::Value, GitError> {
 Ok(json!({ "result": output, "cwd": ctx.worktree_path }))
}
```

LLM transparency logic is now baked into the business logic of every
tool. Future tools must remember to do the same. The60+ existing
unit tests have to be rewritten.

#### Correct: envelope applied at the LLM-facing boundary only

```rust
// GOOD — tool internals unchanged
pub async fn execute(...) -> Result<String, GitError> { ... }

// agent::chat::chat at the agent-loop boundary
let wire = tool_result_envelope(output, &ctx.worktree_path);
emit(ChatEvent::ToolResult { content: wire, ... });
persist(ContentBlock::ToolResult { content: wire, ... });
```

The `tool_result_envelope` function lives in `agent::helpers` and is the
single place that knows about the envelope shape. Tools remain pure
data producers; the agent loop is the formatter.

#### Wrong: destructive path without cancel

```rust
// BAD — race against an in-flight chat
async fn delete_session(state, sid) -> Result<()> {
 cleanup_outputs_dir(...).await; // shells out
 destroy_worktree(...).await; // libgit2
 db::delete_session(...).await; // FK cascades
}
```

If the agent loop is mid-turn, its `db::insert_message(...)` or
`db::update_session(...)` may fire after the row is gone, hitting FK
violations or silently dropping the message. The LLM's next-turn
history is also missing the messages that didn't land.

#### Correct: cancel, then proceed

```rust
// GOOD — cancel hook at the top
async fn delete_session(state, sid) -> Result<()> {
 cancel_inflight_for_session(
 &state.cancellations,
 &state.session_active_request,
 sid,
 ).await;
 // CancellationGuard's Drop will remove the map entry once the
 // agent loop exits; we don't wait for it explicitly — the next
 // operation in this function does not need the entry removed.
 cleanup_outputs_dir(...).await;
 destroy_worktree(...).await;
 db::delete_session(...).await;
}
```

The destructive path is bounded by the cancel; the agent loop's
`tokio::select!` will exit on the next event boundary, and the guard
cleans the map.

#### Wrong: signal worktree state only via user-role event messages

```rust
// BAD — system field hard-coded to None; rely on [worktree event]
// rows in history as the only signal
chat_stream_with_tools(config.clone(), messages.clone(), tools.clone());
// (system: None inside the function)
```

The model honestly answers "no" when asked "does your system prompt
mention you're in a worktree" — because the `system` field IS empty.
The `[worktree event]` user-role messages tell the model "the user
told me X happened", but the model treats them as user assertions,
not authoritative grounding. Worse, the events describe
*transitions*; if the user attaches a worktree and then sends5
unrelated messages, the relevant `[worktree event]` is buried in
history and the model has no compact, always-present statement of
its current state.

#### Correct: build system prompt once per chat, pass via `system:`

```rust
// GOOD — build_system_prompt fills the Anthropic `system` field
let head_sha = lookup_head_sha(&worktree_path);
let system_prompt = build_system_prompt(
 &loaded_session.session,
 &project,
 &worktree_path,
 &head_sha,
);
for turn in1..=MAX_TURNS {
 let stream = chat_stream_with_tools(
 config.clone(),
 Some(system_prompt.clone()),
 messages.clone(),
 tool_defs.clone(),
 );
 // ...
}
```

The system prompt is the model's *current state*; the `[worktree
event]` messages remain in history so the model can recall the
*transition* (useful if the model wants to explain "I just attached
the worktree"). Both work together:

- **System prompt** = persistent declaration. Built once per chat
 invocation. Lives in `agent::system_prompt::build_system_prompt`, single source
 of truth.
- **`[worktree event]`** = transition log. Injected at attach /
 detach / delete time as a user-role message, persisted in the
 `messages` table.
- **Tool result envelope `cwd`** = runtime data point per tool
 call. Confirms what cwd the specific tool actually ran against.
