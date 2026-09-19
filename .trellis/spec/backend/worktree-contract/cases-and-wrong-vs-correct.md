<!-- Moved from worktree-contract.md 2026-09-19 (doc-split) -->

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
emit_tool_result(&ctx, wire); // emits on the `tool:result` IPC channel (state.rs ChatEventSink trait)
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
