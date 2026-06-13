# State Management

> How state is managed in this project.

---

## Overview

<!--
Document your project's state management conventions here.

Questions to answer:
- What state management solution do you use?
- How is local vs global state decided?
- How do you handle server state?
- What are the patterns for derived state?
-->

The chat store is a Pinia store (`app/src/stores/chat.ts`) backed by Tauri IPC
events from the Rust agent core. The state model is: **the wire-format blocks
the agent sends are the source of truth for what's persisted, but the
in-memory representation is denormalized for rendering and event handling.**

---

## State Categories

<!-- Local state, global state, server state, URL state -->

`ChatMessage` carries three categories of state:

1. **Visible fields** — `role`, `text: string[]`, `toolUses`, `toolResults`,
   `isStreaming`. These render directly into the bubble.
2. **Thinking fields** — `thinkingBlocks: ThinkingBlockInfo[]` and
   `redactedThinkingData: string[]`. In-memory only. The visible UI is a
   `<details>` element that defaults to collapsed; on session reload these
   fields are rehydrated from DB and rendered the same way.
3. **Collapse / scroll state** — in-memory only, intentionally NOT persisted.
   Reload resets; the spec is to behave like Claude.ai / Claude Code.

---

## When to Use Global State

<!-- Criteria for promoting state to global -->

Any field that survives a session reload goes in the DB. The DB schema is
documented in `app/src-tauri/src/db.rs`; the round-trip contract is
`MessageContent::Blocks` (a `Vec<ContentBlock>`).

Three persistence rules that came out of step 6:

- **Thinking text + signature go in the DB.** `ContentBlock::Thinking` and
  `ContentBlock::RedactedThinking` are first-class block types and survive
  rehydrate losslessly.
- **The denormalized `text` column does NOT contain thinking text.**
  `MessageContent::to_text()` (Rust) and the corresponding rehydrate logic
  (TS) keep the two streams separate; the bubble shows only the actual reply.
- **Thinking blocks come first in the outbound payload.**
  `toPayloadContent` emits `thinking` / `redacted_thinking` blocks at the
  head of the assistant message — Anthropic requires this order on
  round-trip; emitting them anywhere else → 400. See
  `backend/llm-contract.md` §4 for the full constraint list.

---

## Server State

<!-- How server data is cached and synchronized -->

Event handling contract (from step 6):

| Wire event (`kind`) | Handler | Effect on `ChatMessage` |
|---------------------|---------|-------------------------|
| `"thinking_delta"` | append to `currentThinkingBlock(m).thinking` | UI `<details>` re-renders with new text. |
| `"signature_delta"` | set `currentThinkingBlock(m).signature` | Block is now "closed" — the next `text_delta` or `tool_call` flushes it. |
| `"redacted_thinking_delta"` | append to `m.redactedThinkingData` | UI shows "🔒 N redacted" placeholder. |

The `currentThinkingBlock(m)` helper finds or creates the last open
`thinkingBlocks[i]` so the frontend can handle multiple interleaved thinking
blocks without scattering chunks. It exists because the wire format
`thinking_delta → signature_delta → thinking_delta → signature_delta` (rare
but possible) is the canonical "two thinking blocks in one turn" pattern.

---

## Common Mistakes

<!-- State management mistakes your team has made -->

### Mistake: appending thinking text to the bubble

Thinking is rendered in a separate `<details>` block ABOVE the bubble. Do not
inline it; do not append to `m.text`. `MessageContent::to_text()` (Rust) and
the TS rehydrate path both keep these streams separate.

### Mistake: dropping the signature on rehydrate

`ContentBlock::Thinking { thinking, signature }` → both fields must land in
`m.thinkingBlocks[i]`. If `signature` is empty after rehydrate, the next turn
will 400. The check phase of step 6 added a unit test for the round-trip; any
future change to the DB schema must keep this invariant.

### Mistake: emitting thinking blocks after tool_use in `toPayloadContent`

The outbound payload order is: `thinking → redacted_thinking → text → tool_use → tool_result`.
Anthropic validates this order strictly. See
`backend/llm-contract.md` §4 / §7 for the constraint and a Wrong/Correct pair.

---

## Stream Controller Pattern (added 2026-06-07, PR 06-07-6-ui-bug-markdown-sse)

For anything related to **in-flight SSE streams from the Rust agent loop**, the
single source of truth is `useStreamControllerStore()` in
`app/src/stores/streamController.ts` — NOT `useChatStore()`. The chat store
is a thin facade that projects controller state for the UI to read.

### Why a separate store

The old design put messages, `streamingSessionId`, `currentRequestId`, and
the SSE listener all inside `useChatStore()`. That broke the moment a user
switched sessions mid-stream: the listener filtered events by
`currentSessionId`, so `done` events for the now-non-current stream were
dropped, leaving the red dot, the "stop" button, and `sending` all stuck.
The streaming message itself was also lost when `switchSession` rehydrated
the new session's messages and overwrote `messages.value`.

### The split

| Concern | Owns | API |
|---|---|---|
| Per-session message buffer | **streamController** | `messagesBySession: Map<sessionId, ChatMessage[]>` (LRU 20) |
| Active in-flight requests | **streamController** | `activeRequests: Map<requestId, RequestState>` |
| SSE listener registration | **streamController** (singleton) | `start()` / `stop()` in `App.vue` lifecycle |
| `streamingSessionIds` / `streamingProjectIds` | **streamController** | `ComputedRef<Set<string>>` — UI subscribes |
| Sessions list, currentSessionId, currentCwd | **chatStore** (UI state) | `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd` |
| Session CRUD (`createNewSession`, `switchSession`, `deleteSession`) | **chatStore** (delegates to controller) | wires UI → controller's `ensureLoaded` / `evict` |
| Wire-format history construction (`toPayloadContent`) | **chatStore** (the only place that needs `ChatMessage` for outbound) | passed to `controller.startRequest` |

### Rules of thumb for new code

- **Never** register an SSE listener outside `streamController.start()`. One
  global listener routes by `request_id` (not by current session) — that's
  the whole point.
- **Never** mutate `messagesBySession` directly from a component. Use
  `chatStore.send()` (which calls `controller.startRequest` and pushes
  user/assistant placeholders into the correct session's array) or
  `controller.ensureLoaded(id)` for the DB-backed load path.
- **Pin streaming sessions in the LRU.** `controller.startRequest` calls
  `pinnedSessions.add(sessionId)`; `finalizeRequest` removes it. Don't
  hand-evict a session whose `activeRequests` map has an entry for it.
- **`isCurrentSessionStreaming` is per-session.** Use it for the chat
  input's stop button. Use `streamController.streamingSessionIds` directly
  for the sidebar (PR4) so non-current sessions can show their own
  streaming indicator.

### Reactive bridge caveat

`streamController.streamingSessionIds` is a `ComputedRef<Set<string>>`.
Pinia auto-unwraps refs on store-proxy access, so components read it as a
plain `Set<string>` (no `.value`). The `Set` itself is recomputed on every
`activeRequests` mutation; the `v-if` binding on the session card flips
automatically. No manual triggers needed.

### Worktree transition invalidation (added 2026-06-08, step 4 follow-up)

After any worktree state change (`attachWorktree` / `detachWorktree` /
`deleteWorktree`), the chat store calls `controller.refresh(sessionId)` to
evict the cached messages and reload from DB. This is the only way the LLM's
next `send()` payload can include the freshly-injected `[worktree event]`
system event.

```typescript
// app/src/stores/chat.ts (excerpt)
async function attachWorktree(sessionId: string) {
  // ... Tauri invoke ...
  controller.refresh(sessionId);  // ← mandatory, do not skip
}
```

`controller.refresh` is a thin wrapper around `ensureLoaded` that first
evicts the LRU entry and then re-loads from DB. Without it:

- The cached `messagesBySession.get(sessionId)` is stale.
- The next `send()` builds `toPayloadContent` from the cache.
- The LLM's payload omits the system event, and the model reasons on
  the old worktree state.

Backend stores the system event in the same `messages` table; the
frontend does not need a special branch in the wire event handler.
The system event renders as a regular user-role message with the
`[worktree event]` prefix.

### Send completion invalidation (added 2026-06-08, step 4 follow-up — 2013 wire invariant)

Every `send()` must end with `controller.finalizeRequest` clearing the
in-memory `messagesBySession` entry **and** the chat store's
`diffCache` entry for the same session. The cleanest place to do
both is the controller's own `finalizeRequest` (the function the
`done` / `error` / catch-error paths all route through) — pairing
`evict(sessionId)` with `useChatStore().invalidateDiff(sessionId)`
in one call.

```typescript
// app/src/stores/streamController.ts (excerpt)
function finalizeRequest(requestId: string, sessionId: string, _errored: boolean): void {
  activeRequests.delete(requestId);
  pinnedSessions.delete(sessionId);
  evict(sessionId);
  useChatStore().invalidateDiff(sessionId);  // ← paired, mandatory
}
```

Why both:

- **In-memory `messagesBySession`** is the *streaming-accumulation*
  shape — a single `assistantMsg` placeholder that absorbed every
  `delta` / `tool_call` / `tool_result` / `thinking_delta` event
  across all turns of the `chat` invocation. The DB stores one
  assistant message per agent-loop turn (per turn the Rust side
  persists in `lib.rs:chat`). If we leave the cache after a
  successful send, the next `ensureLoaded` for that session takes
  the in-memory fast path and the wire-format history sent to the
  LLM has an assistant turn whose `tool_use` block is followed by
  a user-text message (the next typed prompt) with no `tool_result`
  in between — Anthropic Messages API returns 2013 ("tool call
  result does not follow tool call"). Evicting forces the next
  `ensureLoaded` to re-read from DB, where the per-turn split
  shape puts the `tool_result` in the correct following
  user-role message.
- **Chat store `diffCache`** holds the worktree diff result so the
  chip's "diff (N)" counter doesn't re-fetch on every render. A
  `git commit` run inside the worktree during the just-finished
  send should drop the counter immediately; without invalidation
  the chip keeps showing the pre-send snapshot until the next
  `attachWorktree` / `detachWorktree` / `deleteWorktree` happens to
  trigger an `invalidateDiff` (coincidentally — that's why the
  earlier 2026-06-07 step-4 follow-up worked for the first send
  but regressed on the second).

A failure mode that this invariant prevents: a refactor that splits
`finalizeRequest` into "evict" and "invalidate" helpers but only
calls one of them would silently break one of the two bugs above.
The two actions are paired, not independent — the
`streamController.test.ts` `finalizeRequest` describe block has a
`both actions fire on the same finalizeRequest call (paired
invariant)` test that locks this.

### Per-session Mode field (added 2026-06-13, PR2 of A2 + B7)

`SessionSummary.mode: "chat" | "plan" | "review" | "yolo" | "background"`
is the per-session mode override. The wire field is snake_case
(untyped on the Rust side via `Option<String>` in
`db::models::SessionRow`) and serializes as the lowercase `Mode`
enum string. The `Background` variant is reserved in the enum for
schema stability but never appears in the UI; the
`SessionMode = "chat" | "plan" | "review" | "yolo"` subset is the
user-facing surface (`MODE_CYCLE` constant in `app/src/stores/chat.ts`).

The store-level orchestrator is `requestSetMode(sessionId, mode)`
in `chat.ts`. Both UI entry points (popover via `ModeSelect.vue`
and `Shift+Tab` cycle in `ChatInput.vue`) call this single method,
which:

- Short-circuits when the target mode matches current.
- Refuses the call while the session is streaming (the `:disabled`
  contract; mirrors `ModelSelect.vue`).
- For `Yolo`, flips `pendingYoloConfirm = true` (the modal mounts
  via `v-if`; the modal's confirm button calls `confirmYolo()`
  which fires the actual `set_session_mode` IPC).

### Permissions store + PermissionModal IPC bridge (added 2026-06-13, PR3 of A2 + B7)

For anything related to the ⑨ 关 user-confirmation IPC, the single
source of truth is `usePermissionsStore()` in
`app/src/stores/permissions.ts`. The backend's
`agent/permissions::check` emits a `permission:ask` event when a
tool_use lands on Tier 3 of the 5-tier decision layer; the store
listens for that event and exposes the pending payload via
`pendingPermission`. `<PermissionModal>` (mounted in
`ChatPanel.vue`) renders whenever the slot is non-null.

The store mirrors the backend's behavior closely. Key design
points:

- **Single slot, replacement semantics**: `pendingPermission`
  holds exactly one ask at a time. When the backend emits a new
  `permission:ask` (e.g. the next tool_use in a multi-tool turn),
  the listener overwrites `pendingPermission`. The modal's
  `:key` binding should be on `pendingPermission.rid` so it
  remounts on every replace (resets focus + scroll).
- **120s timer** (`ASK_TIMEOUT_MS = 120_000`): the store arms a
  client-side timer that mirrors the backend's
  `tokio::time::sleep(ASK_TIMEOUT)`. Duplication is intentional:
  it lets us (a) close the modal at the same moment the backend
  resolves the oneshot, and (b) surface a
  "权限询问已超时,已自动拒绝" toast via `useProjectsStore.showToast`.
  Both paths converge to a deny — race-guard via `timerRid.value`
  catches "user clicked just as timer fired".
- **IPC wire shape** (matches `agent::permissions::PermissionAskPayload`
  via `#[serde(rename_all = "camelCase")]`):

  ```typescript
  // Server → Client: emit("permission:ask", payload)
  interface PermissionAsk {
    rid: string;                         // UUID, ties to the oneshot
    toolName: string;
    toolInput: Record<string, unknown>;
    risk: "low" | "medium" | "high" | "critical";
    reason?: string;
  }

  // Client → Server: invoke("permission_response", { rid, decision })
  type PermissionDecision = "allow_once" | "allow_always" | "deny";
  ```

- **Lifecycle**: `permissionsStore.start(toast)` is called from
  `ChatWindow.vue`'s `onMounted` (passes the projects-store
  `showToast` for the timeout path). `stop()` is reserved for
  future hot-reload / test scenarios; the listener lives for
  the lifetime of the Tauri process in practice.

The store never owns visual chrome — `<PermissionModal>` is the
component that owns the DOM. This matches the pattern in
`ModeSelect.vue` / `YoloConfirmModal.vue`: stores hold reactive
state, components handle rendering. The Modal's three buttons
each call `store.respond(rid, decision)`; cancel/Esc/X/backdrop
also route through `respond(rid, "deny")` per spec Q6.

#### Acceptance criteria covered (PermissionModal 14 条)

| AC | Where it's enforced |
|---|---|
| 居中 + 4px backdrop-blur | `<PermissionModal>` template + CSS `:deep(.permission-modal-backdrop)` |
| 56x56 shield icon 容器,risk tint bg | `iconTintStyle` computed, `.permission-modal__icon` |
| Critical 3px 红左 border + shield-x icon | `:deep(.permission-modal--critical)` + `RISK_META.critical.iconName` |
| `JSON.stringify(_, null, 2)` 渲染在 `<pre>` | `formattedInput` computed |
| terminal icon (左) + copy icon (右) | `.permission-modal__preview-icon` + `.permission-modal__copy` |
| "工具类别: X · 风险等级: Y" + risk 颜色点 | `.permission-modal__risk` + `.permission-modal__risk-dot` |
| 3 按钮等宽 33%,顺序 拒绝/仅一次/始终允许 | `.permission-modal__actions` + grid |
| Critical Enter 改 拒绝 | `defaultDecision` computed |
| Esc / X / 遮罩 = 拒绝 | `onKeyDown` + `@click.self="onCancel"` |
| 始终允许 → `INSERT INTO session_tool_permissions` | Backend `check()` Tier 3 `AllowAlways` (PR1 已有) |
| 仅一次 → 不写表 | Backend `check()` Tier 3 `AllowOnce` (PR1 已有) |
| 拒绝 → 后续 tool_use 继续按 ⑨ 关 | Backend `check()` Tier 3 `Deny` (PR1 已有) |
| Stop(C1) → CancellationToken + audit 区分 | Backend `check()` `tokio::select!` cancel branch (PR1 已有) |
| 不渲染 checkbox | 没有 checkbox in template |
| 同一 turn 多 tool_use 串行 | Backend `for tool_use in turn.tool_uses` (PR1 已有) |
| reka-ui DialogContent `:deep()` gotcha | 模板不用 reka-ui (手写 `<Teleport>` + `:deep()`) |
| 120s 超时 → 自动 deny | `startAskTimer` + toast |
