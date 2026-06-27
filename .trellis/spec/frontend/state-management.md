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

### RULE: computed getters must not mutate their own tracked deps

A Vue `computed` re-runs whenever any reactive value it **read** on its
last run changes. If the getter also **writes** one of those same values,
it recursively re-invalidates itself. Vue's scheduler catches this after
100 iterations ("Maximum recursive updates exceeded") and **silently
drops the update** — or, when each re-render is expensive (large lists /
big strings), the runaway re-renders OOM the webview and the window
dies. Both failure modes are brutal to debug because the data is correct;
only the DOM update is lost.

Two real instances shipped and were fixed on 2026-06-24 (commit
`ce25783`):

1. **`streamController.getMessages`** did an "LRU touch"
   (`messagesBySession.delete(k); .set(k, v)`) on every read. The
   `messages` and `currentSessionLatencyTurns` computeds call it inside
   their getter → every streaming event re-triggered the recursion →
   deltas never rendered until a session switch forced a full array
   replacement.
   **Fix:** `getMessages` is a pure read; the touch moved to the
   non-computed callers (`ensureLoaded`, `startRequest`). Locked by a
   regression test in `streamController.test.ts`
   ("getMessages — pure read").

2. **`SubagentDrawer.pendingFirstSeenAt`** was a `reactive(new Map())`
   passed into `pairSections(...)` inside the `toolEntries` computed.
   `pairSections` does `.set/.delete/.get` on it → same recursion,
   amplified by a 100 ms `nowTick` + a worker emitting many sections →
   every tick re-rendered every `DrawerToolCallCard` (including huge
   tool_result content) until the webview OOM'd.
   **Fix:** plain `new Map()` — its accumulation state doesn't need
   Vue tracking (the computed already re-runs on `nowTick`).

**Rule of thumb:** a function called from a `computed` getter must be a
**pure read** of the reactive state it touches. Any write side-effect
(LRU touch, caching into a reactive collection, counter bump) belongs in
a `watch` / `watchEffect` / plain async function — never in the getter.
For per-key accumulation maps that a pure helper mutates, use a plain
`Map` (or `shallowRef` / `markRaw`); reserve `reactive(new Map())` for
maps the template subscribes to directly.

Note: store-level `reactive(new Map())` (`tokenUsageBySession`,
`liveTranscript`, `runSummaryBySession`, ...) is **fine** — those are
mutated by event handlers / actions, not from inside a computed getter.
The bug is specifically *mutating a reactive Map from within a computed
that reads it*.

### Per-run spinner isolation via reactive Map (added L3b PR4 2026-06-27)

For UI surfaces that need to track an in-flight action state **per
key** (e.g. per-run, per-session) — typically a spinner that drives
button `:disabled` — use a top-level `reactive(new Map<key, State>())`
in the store. Mutated from store actions (NOT from inside a computed
getter per the rule above), cleared in `finally`.

```ts
// app/src/stores/subagentRuns.ts (L3b PR4)
type MergeState = { kind: "merge" | "discard"; loading: true };
const mergeStateByRunId = reactive(new Map<string, MergeState>());

async function mergeWorker(runId: string): Promise<MergeResult> {
  if (mergeStateByRunId.has(runId)) {
    // Spinner guard: second click while one is in flight.
    // Button :disabled should already prevent this, but defensive.
    return { kind: "error", message: "another action is already in flight" };
  }
  mergeStateByRunId.set(runId, { kind: "merge", loading: true });
  try {
    await invoke<string>("merge_worker_run", { rid: "merge-pr4", runId });
    // success → mutate row cache (separate Map), no setter cross-talk
    return { kind: "success" };
  } catch (e) {
    // ...
  } finally {
    mergeStateByRunId.delete(runId);  // ALWAYS clear, even on error
  }
}
```

Why a Map (not a single boolean ref): multiple components can mount
the same control with different keys (e.g. 3 subagent drawers open
concurrently, each with its own merge in flight). A single `isLoading`
ref would block all 3 buttons on any one of them. The Map keys on
`runId` so each drawer drives its own spinner independently.

Why `reactive(new Map())` (not `shallowRef` / `markRaw`): the template
subscribes to `store.mergeStateByRunId.get(runId)` directly via a
component computed — Vue needs to track `.has/.get/.set/.delete` on
the Map for re-render. (This is the documented "template subscribes
directly" case from the rule above — fine to mutate from actions,
NOT from inside a computed getter.)

Companion pattern — `getRunCache: reactive(new Map<runId, SubagentRunRow>())`
for the detail cache: same pattern, separate Map. The detail cache and
the spinner Map are intentionally separate so a row mutation
(`getRunCache.set(runId, {...row, worktreePath: null})`) doesn't
disturb the spinner state and vice versa.

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

`SessionSummary.mode: "edit" | "plan" | "yolo" | "background"`
is the per-session mode override. The wire field is snake_case
(untyped on the Rust side via `Option<String>` in
`db::models::SessionRow`) and serializes as the lowercase `Mode`
enum string. The `Background` variant is reserved in the enum for
schema stability but never appears in the UI; the
`SessionMode = "edit" | "plan" | "yolo"` subset is the
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
    /** Re-grill 2026-06-13 (PR2): path scope field. Backend
     *  serializes with `#[serde(skip_serializing_if =
     *  "Option::is_none")]` so the field is truly absent for
     *  shell / web_fetch — the modal checks
     *  `typeof ask.path === "string"` to decide whether to
     *  render the path range row. Only set for path tools
     *  (read_file / write_file / edit_file / list_dir /
     *  grep / glob). */
    path?: string;
  }

  // Client → Server: invoke("permission_response", { rid, decision })
  type PermissionDecision = "allow_once" | "allow_always" | "deny";
  ```

- **Path range row** (re-grill 2026-06-13 PR2): `<PermissionModal>`
  reads `ask.path` and computes the in-repo / out-of-repo badge
  against the session's `currentCwd` via the
  `isPathInRoot(target, root)` helper in `app/src/utils/path.ts`.
  The helper mirrors the Rust `projects/boundary::is_within_root`
  predicate (component-wise lexical match, see
  `.trellis/spec/backend/project-cwd-boundary.md §6`). When
  `path` is absent (shell / web_fetch) the row is entirely
  hidden via `v-if="hasPath"` — no empty placeholder, no layout
  shift. The badge color reuses existing tool-color tokens:
  in-repo = `--color-tool-write` (emerald), out-of-repo =
  `--color-tool-shell` (amber). See the
  `PermissionModal: path range row` case study in
  `popover-pattern.md` for visual + behavioral details.

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

#### Worker ask routing — dual map (added 2026-06-22, RULE-FrontSubagent-003)

> Worker (subagent) asks now flow through the same `permission:ask`
> channel as main-chat asks (backend PR1.5 dual-emits — see
> [permission-layer.md §5b](../backend/permission-layer.md)). The
> store must keep them in a **separate map** from main-chat asks.

**Why a separate map** (`pendingWorkerByRunId`, NOT `pendingBySession`):

A parent session can have its OWN main-chat ask pending (user paused on
a parent tool_use) WHILE a worker it spawned is ALSO waiting on approval.
If both keyed into `pendingBySession[sessionId]`, the worker ask would
overwrite the parent's pending slot (replacement semantics) — the parent's
modal would vanish mid-confirmation. Two maps avoid the collision:

```typescript
const pendingBySession = reactive(new Map<string, PermissionAsk>());      // main-chat (workerRunId absent)
const pendingWorkerByRunId = reactive(new Map<string, PermissionAsk>());  // worker (workerRunId present)
```

`setPending(ask)` branches on `ask.workerRunId`:
- present → `pendingWorkerByRunId.set(ask.workerRunId, ask)`
- absent → `pendingBySession.set(ask.sessionId, ask)` (unchanged)

`respond(rid)` scans **both** maps (rid is unique per ask, routes correctly
regardless of origin). `stop()` clears both + their timers.

**Accessors**:
- `getPendingByRid(rid)` — scans both maps. Used by `<SubagentDrawer>`'s
  `isPermissionAskLive(rid)` reconciliation: a transcript ask entry is
  rendered as **interactive** (Allow/Deny) while its rid is live-pending,
  else **historical**. Same rid appears in both the transcript (via
  `subagent:event`) and the live store (via `permission:ask`); the drawer
  renders exactly ONE card, mode-flipping on store state.
- `pendingWorkerCountForSession(parentSessionId)` — counts worker asks
  whose `sessionId === parentSessionId`. Drives `<WorkerAskBanner>`:
  `v-if="count > 0"`. The worker ask's `sessionId` field carries the
  **parent** session id (not the composite `worker:{runId}` — that's
  backend-internal only), so the banner groups worker asks under their
  parent session correctly.
- `pendingWorkerRunIdsForSession(parentSessionId)` — the runId list;
  banner click opens the drawer for the most-recent pending run.

**Worker card hides "始终允许"**: `<DrawerPermissionAskCard>` passes
`hideAllowAlways = !!ask.workerRunId` to `<PermissionAskBody>`. Backend
worker `AllowAlways` is treated as `AllowOnce` (no `session_tool_permissions`
write — workers don't persist across the permission boundary), so the button
would be misleading. Main-chat `<PermissionModal>` still shows all 3 buttons.

**No global modal for worker asks** (design lock): worker approval UI lives
inside `<SubagentDrawer>` (interactive card) + the non-blocking
`<WorkerAskBanner>` in `<ChatPanel>` header. Never a top-level modal —
multi-session concurrency would race + obscure attribution.

### D3 PR2 (2026-06-17): inline message edit (user messages only)

The UI half of the session message edit / resend feature
(PR1 landed the backend `edit_user_message` Tauri command in
commit `308d277`; PR2 wires the frontend). The user-initiated
edit flow lives entirely in the chat store + `MessageItem.vue`
+ a new `<MessageActionsMenu>` component. Resend is left as a
disabled placeholder pending PR3.

#### Chat store API additions

- `editingMessageSeq: Ref<number | null>` — the message seq
  currently in inline edit mode (`null` = no row). Lives on the
  store, not as a local ref, because `MessageList` remounts
  on session switch and would lose a local `ref`. A single
  nullable scalar is the right shape: only one row can be in
  edit mode at a time. Cleared on Save success, on Cancel, and
  on session switch.
- `editMessage(sessionId, messageSeq, newContent)` — bridge to
  the backend `edit_user_message` IPC. The flow:
  1. Cancel any in-flight stream on the session (current session
     uses the existing `cancel()` wrapper; cross-session edits
     go through `controller.cancel(rid)` directly with the
     resolved `requestId`).
  2. `await invoke<void>("edit_user_message", { sessionId,
     messageSeq, newContent })`. The backend's `Result<(), String>`
     becomes a JS rejection on failure — we let it propagate
     so the caller (`MessageItem.vue`'s Save handler) can
     toast and keep edit mode active for retry.
  3. `await controller.refresh(sessionId)` — evict + re-load
     the in-memory buffer from DB. The rehydrated messages
     carry the new `content` / `text` columns + the trimmed
     tail (cascade DELETE).

The `newContent` is a plain `string` (matches the backend
`MessageContent::Text` variant). The `MessageContent` Rust type
also has a `Blocks` variant for richer content (e.g. images,
tool blocks), but PR2 only edits plain text — richer
content editing is a future enhancement.

Multi-listener safety: `editMessage` only mutates the
controller's per-session buffer (via `controller.refresh`),
never the `sessions` list directly, and never
`currentSessionId`. The SessionList / project tab subscribers
see the title's `updated_at` advance via the existing
`controller.activeRequests.size` watcher (which fires
`loadSessions` on any shrink). Pinia deep proxies mean no
listener sees a "reset" event — the new content lands in
place on the reactive array.

#### `<MessageActionsMenu>` component

A new `app/src/components/chat/MessageActionsMenu.vue` renders
the hover-triggered ⋯ button + reka-ui `DropdownMenu` with
three items:

| Item | Enabled when | Handler |
|---|---|---|
| Edit | `role==="user"` AND `!isEditing` AND `!isStreaming` | `emit("edit", seq)` → parent enters edit mode |
| Resend | always disabled in PR2 | no-op (tooltip "PR3 待实施") |
| Copy | always | `navigator.clipboard.writeText(content)` + toast |

Why reka-ui `DropdownMenu` (not the hand-rolled
`popover-pattern.md`): the message-hover ⋯ trigger is
per-row ephemeral (appears on hover, hides on leave). Reka-ui
gives keyboard arrow / Esc / focus-return a11y out of the box,
and `DropdownMenuTrigger` `as-child` lets us style the existing
button without wrapping in a new element. Trade-off (acknowledged
in `popover-pattern.md`): we now have two popover
implementations. Future work could extract `usePopover`.

Trigger placement: `position: absolute; top: -8px; right: 4px`
inside the parent `.msg <li>`. The parent's `position: relative`
gives the trigger an anchor. Opacity transitions drive the
hover-in / hover-out feel — the trigger is `opacity: 0` by
default and the parent's `.msg:hover .msg-actions` rule
flips it to `1`. `:focus-within` keeps it visible when
keyboard focus arrives on the wrapper.

Stream race: when `isStreaming` is true (per-session, read from
`controller.streamingSessionIds`), the trigger is `disabled` and
the `.msg-actions--streaming` class drops `pointer-events: none`
+ `opacity: 0`. The same gate is applied at the menu item level
(Edit requires `!isStreaming`), so even a stray click can't
fire Edit mid-stream.

#### `MessageItem.vue` edit mode

When `chatStore.editingMessageSeq === message.seq`, the bubble
is hidden and replaced with:

- `<textarea>` — autosize 2-20 rows, follows the bubble's width.
  v-model bound to a local `editBuffer: Ref<string>` (NOT to
  `props.message.content` directly — the controller's streaming
  `delta` handler also mutates that field, and a live v-model
  would race the streaming text append).
- Save / Cancel buttons — Save fires the chat store's
  `editMessage`; Cancel clears `editingMessageSeq` and resets
  the buffer. Both are disabled while the IPC is in-flight
  (`isSaving` ref).
- Inline error row — if the IPC rejects, the error message
  is shown in-place (so the user knows what to fix) AND a
  toast fires via `projectsStore.showToast`. Edit mode stays
  active so the user can retry without retyping.

The local `editBuffer` is re-seeded whenever edit mode opens
for this row OR the underlying `message.content` changes
(so a streaming turn that ends mid-edit re-seeds the buffer
with the final content rather than the stale pre-stream text).

The `displayContent` computed pauses the markdown pipeline
while the textarea is open (`isEditingThisMessage ? "" :
props.message.content`) so the debounced renderer doesn't
clobber the user's edits. The watcher on
`[editingMessageSeq, message.content]` re-schedules the render
on cancel / save so the bubble re-renders with the new
content.

The `<li>` carries a `.msg--editing` class while in edit mode
to give the row a subtle accent border + tinted background
(analogous to `.tool-card--pending`). The user can still see
the surrounding context but the row is clearly demarcated.

#### D3 PR3 (2026-06-17): Resend pipeline + "(edited)" label

The Resend pipeline is the second half of D3's user-message
edit feature. PR1 + PR2 covered Edit (in-place content update
+ cascade delete); PR3 wires Resend (no content mutation, just
re-fire the existing user prompt). The frontend half of PR3
plugs into the same `<MessageActionsMenu>` PR2 built, so no
new UI components are needed — only the disabled placeholder
is replaced with the real handler.

##### Chat store API addition

`resendMessage(sessionId, messageSeq, contentText)` — bridge
to the backend `chat` IPC with the `resendSeq` flag. The flow
mirrors `send()` (placeholder user + assistant messages +
`controller.startRequest`), with one difference: the
`resendSeq` flag in the IPC arg tells the backend this is a
re-fire of an existing prompt (the backend writes a
`resend_message` audit row at the user-message persist site,
no content mutation, no cascade). The `contentText` parameter
is the original user message's `content` (verbatim — Resend
re-runs the same prompt).

The stream-race guard is identical to `editMessage`: cancel
any in-flight stream on the session first (current-session
fast path via `cancel()`, cross-session via
`controller.cancel(rid)`), then fire the new stream.

##### `<MessageActionsMenu>` Resend wired

The PR2 placeholder (`canResend = () => false` + "PR3 待实施"
tooltip) is replaced with `canResend = () => role === 'user'
&& !isEditing && !isStreaming` — the same gate as Edit
(because Resend on an assistant message has no defined
semantics). The `resend` emit bubbles to `MessageItem.vue`,
which calls `chatStore.resendMessage(sessionId, messageSeq,
content)`. Failure surfaces via `projectsStore.showToast`
(same pattern as Edit's catch path).

##### "(edited)" label render

D3 PR3 also surfaces the `(edited)` affordance on edited
message rows. The render reads `message.metadata.edited_at`
(plain string) from the in-memory `ChatMessage.metadata`
field, which is populated by `rehydrateMessages` in
`streamController.ts` from the `MessageRow.metadata` JSON
column. When the field is present AND the row is not
streaming AND not in edit mode, a small grey italic
`(edited)` label renders inline at the bottom-right of the
bubble (analogous to the F5 latency chip's position, but
inside the bubble so it doesn't collide with the chip).
Hover surfaces the precise edit timestamp via the
`title` attribute (the audit log carries the same field
in a structured way; the inline label is just a hint).

The metadata field is intentionally a free-form
`Record<string, unknown>` (not a discriminated union) so
future metadata fields (e.g. `original_content` for an
undo affordance, if a later PR adds it) don't require
touching the `ChatMessage` interface. The rehydrate path
parses the raw JSON object and assigns it verbatim; the
renderer only reads the fields it cares about
(`edited_at`). Defensive: a missing or non-string
`edited_at` is treated as "not edited" (no label).

The `(edited)` label is rendered on both user AND assistant
messages, defensively — D3 PR1 only allows user edits in
practice, but the render path is generic (any row with
`metadata.edited_at` shows the label). If a future PR adds
"edit assistant message" support, the label will Just Work.

---

### B12 Checklist store (2026-06-19)

The frontend half of B12 (PR1 backend `update_checklist` committed `994db84`; PR2
frontend committed `1896470`). The store derives the current checklist for display
from the `update_checklist` tool_use **INPUT** (not the tool_result — that is rendered
text for the LLM). Single source of truth: `useChecklistStore()` in
`app/src/stores/checklist.ts`.

#### State shape

```typescript
const checklistBySession = reactive(new Map<string, ChecklistItem[]>());
```

- **Absent key** = "no `update_checklist` seen yet this run" → the `<ChecklistCard>`
  overlay hides.
- **`[]` (present empty)** = "the model cleared the list" → the card renders the empty
  state. (Distinct from absent — do NOT conflate; `getChecklist(sessionId)` returns
  `null` for absent, `[]` for cleared.)

#### Types (mirror PR1's Rust wire)

```typescript
export type ChecklistStatus = "pending" | "in_progress" | "done"; // Rust #[serde(rename_all="snake_case")]
export interface ChecklistItem { content: string; status: ChecklistStatus }
```

#### Store API

| Method | Called by | Effect |
|---|---|---|
| `handleToolCall(sessionId, toolName, input)` | `streamController.handleToolCall` on a `tool:call` for `update_checklist` | parse `input.items` → coerce → set as current (**live** path; does NOT wait for `tool:result`) |
| `rehydrateFromMessages(sessionId, messages)` | `streamController.ensureLoaded` + `reloadAfterFinalize` | scan history for the last committed `update_checklist` (**reload** path) |
| `clearForNewRun(sessionId)` | `chat.ts` `send()` + `resendMessage()` | per-request reset (mirror backend's fresh Vec each run) |
| `clearSession(sessionId)` | `chat.ts` `deleteSession` + `clearSessionMessages` | drop on session removal |

#### Client-side coerce — cross-layer mirror (load-bearing)

`coerceAtMostOneInProgress` / `parseAndCoerceItems` are pure TS fns that **re-implement
PR1's Rust `coerce_at_most_one_in_progress` / `parse_and_coerce`** line-by-line (keep last
`in_progress` via reverse scan, demote earlier to `pending`; unknown status → `pending`;
missing `content` → skip). Why duplicate: the live `tool:call` event carries the model's
RAW input (pre-coerce — the Rust `execute()` body does the coerce). Rendering raw input
would flash multiple `in_progress` items before the `tool:result` lands; client-coerce keeps
the card consistent with the post-coerce state the LLM sees. The two deterministic coerces
produce identical output. **A drift between the TS and Rust coerce is a cross-layer bug** —
`trellis-check` verifies line-by-line parity; keep them in sync on any change to either side.

#### Reload: `is_error` filter (RULE-A-004 contract)

`findLastCommittedChecklist(messages)` scans for the LAST `update_checklist` tool_use whose
paired tool_result has `is_error === false`. A tool_use with no result, or with
`is_error === true` (the cancel path's synthetic "Tool execution was interrupted…" result),
is skipped. Without this filter a cancelled update would freeze the card on the moment of
interruption. Returns `null` when no committed checklist exists → the Map key is deleted.

#### Tool-card suppression

`MessageItem.vue` keeps a local `VIRTUAL_TOOLS = new Set(["update_checklist"])` and a
`visibleToolCalls` computed that filters it out of the `ToolCallCard` stream —
`update_checklist` is represented by the floating `<ChecklistCard>`, not a per-call card.
The suppression is **render-only**: `toPayloadContent` still walks the raw `toolCalls` /
`toolResults`, so the `tool_use` + `tool_result` blocks remain in the LLM-facing wire
payload. `use_skill` is NOT in the set (it renders as a normal `ToolCallCard`). If more
virtual tools accumulate, lift the set to a shared module.

#### `<ChecklistCard>` overlay

Mounted in `ChatPanel.vue` (which gained `position: relative` to anchor it). `position:
absolute`, bottom-right offset above the input bar, `z-index: 50` (below modals, which
Teleport to `<body>` at 1000+). Two states: expanded (full list + the single `in_progress`
item gets a pulse/spinner) ⇄ minimized floating ball (`done/total` count, breathes when an
`in_progress` is active). Toggle is local UI state. Empty/absent checklist → hidden.

---

### subagentRuns store + SubagentDrawer (B6 PR3, 2026-06-20)

The frontend half of B6 PR3 (worker subagent live transcript + drawer).
The backend PR2 hotfix added a live `subagent:event` IPC stream (one
emit per `SubagentBufferSink::emit_*` call) + PR3a added two Tauri
commands for list/get. The store is the reactive wrapper that feeds
the `<SubagentDrawer>` side panel; the `ToolCallCard` for
`dispatch_subagent` collapses to a click-target that opens the
drawer instead of expanding an inline transcript.

Single source of truth: `useSubagentRunsStore()` in
`app/src/stores/subagentRuns.ts`. State shape:

```typescript
const runSummaryBySession = reactive(new Map<string, SubagentRunSummary[]>());  // list cache
const getRunCache          = reactive(new Map<string, SubagentRunRow>());        // detail cache
const liveTranscript       = reactive(new Map<string, TranscriptEntry[]>());    // live stream (debounced)
const openRunId            = ref<string | null>(null);                          // drawer open state
// non-reactive (debounce stage):
const liveTranscriptBuffer = new Map<string, TranscriptEntry[]>();
const debounceTimers       = new Map<string, ReturnType<typeof setTimeout>>();
```

#### Store API

| Method | Effect |
|---|---|
| `fetchForSession(sessionId)` | invoke `list_subagent_runs_by_session` → write `runSummaryBySession`. Failure log+swallow (best-effort). |
| `fetchRun(runId)` | invoke `get_subagent_run` → write `getRunCache` + seed `liveTranscript` IF it's empty (don't overwrite in-flight streaming). |
| `openDrawer(runId)` | set `openRunId` + `fetchRun` if uncached. |
| `closeDrawer()` | clear `openRunId` only (caches intact for instant reopen). |
| `getSummaryByToolUseId(sessionId, toolUseId)` | match `summary.parentRequestId.endsWith("-sub-" + toolUseId)` (backend formats worker rid as `"{parent_rid}-sub-{tool_use_id}"`). Returns `undefined` when uncached. |
| `start()` / `stop()` | mount/teardown the `subagent:event` listener; `stop()` flushes any pending debounce buffer so the last batch isn't lost. Wired from `ChatWindow.vue` `onMounted`/`onUnmounted` (same lifecycle slot as `permissionsStore.start`). |

#### IPC contract

- `listen<SubagentEventPayload>("subagent:event", ...)` payload:
  `{ runId, sessionId, kind: TranscriptKind, payload: Record<string, unknown>, timestamp }`.
- `TranscriptKind` snake_case: `"chat_event" | "tool_call" | "tool_result" | "permission_ask" | "permission_ask_resolved"`.
  **Must mirror the Rust `#[serde(rename_all = "snake_case")]` exactly — drift is a cross-layer bug.**
  - `permission_ask_resolved` (2026-06-22, RULE-WorkerAsk-001) is a **pass-through** kind in the live stream — the store never renders it as a standalone section; `pairSections` (PR5 reducer) pre-scans these entries into a `Map<rid, outcome>` and surfaces `outcome` on the matching `PermissionAskSection`. Resolved entries themselves are dropped from the section list (consumed by pairing).

#### Cross-layer drift traps (locked by tests)

1. **`SubagentRunRow.status` is a raw `string`, NOT the typed enum
   that `SubagentRunSummary.status` carries.** The Rust struct
   behind `SubagentRunRow` does NOT derive the enum-typed status
   (only `SubagentRunSummary` does). The store exports a
   `coerceStatus(raw: string): SubagentStatus` helper that coerces
   both shapes into the typed union (`"running" | "completed" |
   "cancelled" | "error"`); unknown strings fall back to
   `"running"` (mirrors the Rust `SubagentStatusDb::from_str_opt`
   lenient default). Always run `Row.status` through `coerceStatus`
   before comparing.
2. **`TranscriptEntry` keeps snake_case `payload_json`** (the Rust
   struct has NO `rename_all`). The live `subagent:event` payload
   wraps the body as camelCase `payload`. The store unifies on the
   **storage shape** internally: when the listener receives a live
   event, it converts `event.payload` → `entry.payload_json` before
   buffering. The drawer reads a single `TranscriptEntry[]` and
   never has to know which source it came from. Parsing
   `transcriptJson` (DB storage shape, from `get_subagent_run`) uses
   `parseTranscriptJson()` which reads `payload_json`.

#### 200ms debounce (self-implemented, no lodash)

Live events accumulate in the non-reactive `liveTranscriptBuffer`
(per-runId); a per-runId `setTimeout(SUBAGENT_EVENT_DEBOUNCE_MS =
200)` flushes them into the reactive `liveTranscript`. The 200ms
cadence keeps the drawer lively (a human-perceptible update rate)
without re-rendering on every SSE delta (which can be 10+ per second
during a busy worker). Tests use `vi.useFakeTimers()` to control the
flush deterministically. `stop()` flushes any pending buffer so the
last batch isn't lost on unmount.

#### Drawer data-source priority (R6)

```
store.liveTranscript.get(openRunId)                       // 1. live stream (worker running)
  ?? parseTranscriptJson(getRunCache.get(openRunId)?.transcriptJson)  // 2. DB cache (worker done)
  ?? []                                                    // 3. empty state ("Worker is starting...")
```

#### `<SubagentDrawer>` UX mode

Right-side fixed panel (~480px wide, full-height), slides in from
the right (180ms transform). Click-outside / Esc / X close button
clears `openRunId`. Header: status badge (color per status:
running=shell amber / completed=write green / cancelled=muted /
error=red) + subagentName + startedAt + finishedAt (if any) +
summary (if any). Body: scrollable transcript list; per entry: kind
badge (chat=gray / call=blue / result=green / perm=orange) +
`JSON.stringify(payload_json, null, 2)`. Filter row: "Show chat
events" toggle (chat_event entries default hidden — they're verbose
LLM deltas; tool_call/tool_result/permission_ask always visible) +
"原 transcript 已截断 (head + tail)" notice when
`transcriptTruncated !== 0`. Empty state: "Worker is starting...".

**Implementation note (reka-ui version pin)**: the PRD specified
reka-ui `Sheet`, but reka-ui@2.9.9 (this project's pinned version
per `reka-ui-usage.md`) does NOT ship the `Sheet` primitive. We
compose the drawer out of the existing `Dialog*` primitives +
side-panel CSS (fixed right + full height + slide-in transition).
Functionally identical to a Sheet for our use case.

#### Drawer vs popover-pattern.md

The drawer is a **click-triggered + persistent + side-panel**
surface. This contrasts with `popover-pattern.md`'s
**hover-triggered + ephemeral + anchored** surfaces (tooltips,
per-row hover menus). The two patterns don't conflict — each owns
its own trigger / lifecycle / positioning strategy. Drawer state
lives in `subagentRuns.openRunId`; popover state lives in the
host component's local refs. The drawer is single-instance (opening
run B closes run A — no nesting, per PRD Out of Scope).

#### `<ToolCallCard>` dispatch_subagent branch (R7)

A `dispatch_subagent` `tool_use` card collapses to a single row
(clickable affordance on the root `.tool-card`). Clicking anywhere
on the card calls `subagentRuns.openDrawer(runId)` (resolved via
`getSummaryByToolUseId`); the card does NOT expand an inline
transcript. The card also lazy-fetches
`subagentRuns.fetchForSession(currentSessionId)` on mount (so the
summary lookup has data to read). The default input/output
`<details>` are suppressed for this tool — the drawer carries all
the worker state. Lazy-fetch is idempotent (the store replaces the
cache on every call, so multiple dispatch_subagent cards in the
same session just re-fetch the same data).

---

### subagentRuns RunAccumulator (B6 redesign PR2, 2026-06-21)

The frontend half of the B6 subagent-drawer redesign. PR1 (`86a81b2`)
added the DB columns `task` + `final_text` on `subagent_runs`; PR2
(this section) replaces the raw `liveTranscript: Map<runId,
TranscriptEntry[]>` surface with a **per-runId `RunAccumulator`**
that collapses the noisy `chat_event` SSE delta stream into a
denormalized `liveSections: Map<runId, TranscriptSection[]>` for
the drawer to render. The visual rewrite of `<SubagentDrawer>` into
the 5-segment grouped view is **PR5** — PR2 is the data layer only;
the drawer still reads `liveTranscript` for now and the visual
consumer migration is a follow-up.

#### Why an accumulator (not just `transcript: Ref<TranscriptEntry[]>`)

The worker can emit 200+ chat_event SSE chunks per second during a
busy turn. Storing every chunk as its own `TranscriptEntry` is both
verbose (the drawer has to filter them out via the `showChatEvents`
toggle) and a perf liability (Vue deep-proxies each object).
Collapsing the chunk stream into one or two `Thinking` / `Text`
segments (one per content block) cuts the drawer's render set by
~100x and matches the main panel's mental model (the main panel
already aggregates `chat_event` deltas into `ChatMessage.text[]` and
`thinkingBlocks[]`; the drawer was the odd one out exposing raw
chunks).

#### `TranscriptSection` discriminated union (replaces raw `TranscriptEntry` for UI)

```typescript
// 7 section kinds — the drawer's render surface. tool_call /
// tool_result / permission_ask are renamed in PR5's visual layer
// (the rename is render-only — wire shapes stay snake_case).
// `PermissionAskResolved` is a pass-through kind: emitted by the
// sink (RULE-WorkerAsk-001) and consumed by pairSections — never
// rendered as a standalone card.
export type TranscriptSectionKind =
  | "Thinking"            // collapsed chat_event thinking_delta (Anthropic SSE)
  | "Text"                // collapsed chat_event delta (text)
  | "FinalText"           // terminal text from subagent_runs.final_text (PR1 column)
  | "ToolCall"
  | "ToolResult"
  | "PermissionAsk"       // carries optional `outcome` from paired PermissionAskResolved
  | "PermissionAskResolved"; // pass-through; consumed by pairSections pairing

export type PermissionAskOutcome = "allow" | "deny" | "timeout" | "cancel";

export interface TranscriptSection {
  kind: TranscriptSectionKind;
  // Free-form body — schema varies per kind. Drawing from the
  // original TranscriptEntry.payload_json shape (snake_case) for
  // tool_call/tool_result/permission_ask; a flat text string for
  // Thinking/Text/FinalText. `PermissionAskResolved` carries
  // `{ rid, outcome }` (the pairing input).
  body: string | Record<string, unknown>;
  // Live phase only — flags redacted thinking blocks (🔒 marker).
  redacted?: boolean;
  // Live phase only — char counter for the section header chip.
  chars?: number;
  // PermissionAsk only — set by pairSections when a paired
  // PermissionAskResolved entry is found (same `rid`).
  outcome?: PermissionAskOutcome;
}
```

The accumulator keeps `transcript: ShallowRef<TranscriptSection[]>`
as the source of truth for its runId; the store exposes this as
`liveSections.set(runId, acc.transcript.value)` on debounce flush.
Drawer reads `store.liveSections.get(openRunId)` (PR5 migration;
PR2 still exposes `liveTranscript` for backward compat).

#### `RunAccumulator` class API

```typescript
class RunAccumulator {
  private thinkingSegment: TextSegment | null = null;
  private textSegment: TextSegment | null = null;
  private rawEventsShallow: ShallowRef<ChatEventPayload[]> =
    shallowRef(markRaw([]));  // R21 — see below
  readonly transcript: ShallowRef<TranscriptSection[]>;

  /** Live phase — called per SSE event (debounced 200ms upstream).
   *  O(1) per call: route into the active thinking or text segment
   *  and mutate its fields. Never rebuilds the array. */
  feed(entry: TranscriptEntry): void { ... }

  /** Worker finished path — replaces the in-memory transcript with
   *  the authoritative DB-cached version. Linear walk over the
   *  parsed JSON. Called from fetchRun() after the IPC resolves. */
  rebuildFromCache(transcriptJson: string | null,
                   finalText: string | null): void { ... }
}
```

A `Map<runId, RunAccumulator>` lives on the store
(`accumulators`). `clearSession()` drops the entries (paired with
`liveSections.delete(runId)` + the existing debounce buffer +
timers cleanup).

#### chat_event discriminator (drift trap 3 — NEW)

`chat_event` is the wire-level catch-all for LLM streaming chunks.
The accumulator inspects `payload_json.kind` (Anthropic SSE inner
discriminator) to route into the right section. **The switch must
be exhaustive** — adding a new Anthropic SSE event type without
updating this switch is a silent data-loss bug.

| `payload_json.kind` (Anthropic SSE) | Section mutation |
|---|---|
| `"thinking_delta"` | append `text` to `thinkingSegment.text`; bump `chars` |
| `"signature_delta"` | set `thinkingSegment.signature` (closes the block — next `delta` or `tool_call` flushes it) |
| `"redacted_thinking_delta"` | mark `thinkingSegment.redacted = true`, render 🔒 |
| `"delta"` (regular text) | append to `textSegment.text`; bump `chars` |
| `"start"` / `"done"` / `"error"` | dropped (control events) |
| anything else | logged + dropped (defensive — see Common Mistake below) |

`signature_delta` flushes the thinking segment by pushing the
current `thinkingSegment` into `transcript.value` and nulling the
field. The same flush happens implicitly when a `tool_call` or
non-thinking `chat_event` arrives after a thinking block.

The accumulator reads `payload_json.kind` after `routeEvent` has
already translated the live camelCase `payload` to the storage
snake_case `payload_json` — see `wire-shape-contract.md` §"Transcript
Entry" and the B6 PR3 drift trap 2 above. **Don't skip the
translation** — `routeChatEvent` will not find `kind` on a
camelCase object and silently drop every event.

#### R20 invariant: live feed is O(1) per event

`feed()` must NOT rebuild `transcript.value` on every call. The
correct pattern is to mutate the active segment's fields in place
and push a new section object only when the segment *closes*
(signature_delta → flush; tool_call arrival → flush text segment;
new thinking block start → flush prior text). Vue reactivity on
`ShallowRef` will pick up the mutation via the inner field
accessors, not via array reallocation.

##### Common Mistake: O(N²) array spread on every feed (pre-PR2 fix)

The first PR2 implementation did:

```typescript
// BAD — O(N) per event, O(N²) cumulative
feed(entry: TranscriptEntry): void {
  this.rawEventsShallow.value =
    markRaw([...this.rawEventsShallow.value, entry]);
  // ... route into segment ...
}
```

For 20k events this measured **1317ms** in a synthetic perf test —
violating the AC "20000 events transcript 冷启动 <500ms" ceiling
when the same pattern is applied to the live path. Fixed by
removing the spread entirely from the live path: `rawEvents` is
populated only by `rebuildFromCache` (the cold-cache fetch path,
which is the actual AC ceiling — measured 13.6ms for 20k).
Post-fix: 20k live events = 1.5ms (880x improvement).

The lesson: **`rawEvents` is a test-only handle, not a runtime
buffer.** If you need to inspect the live raw stream in a test,
seed via `rebuildFromCache` instead of `feed()`. The markRaw lock
test (`__v_skip` symbol assertion) was updated accordingly — it's
still real, just reached via the cold path.

#### R21 invariant: raw events wrapped in `markRaw()`

`rawEventsShallow` is initialized as `shallowRef(markRaw([]))`.
Vue's `__v_skip` symbol is set on the **array root** when
`markRaw` is applied — that's what stops proxy creation on the
root. The inner `TranscriptEntry` objects inside the array are
NOT deep-marked (and don't need to be — they're read-only data).
The test `rawEvents wraps in markRaw` asserts
`(rawEvents as any).__v_skip === true`; if a refactor accidentally
drops the `markRaw`, this test catches it.

#### R22 invariant: live = mutate segments, finished = full rebuild

```
subagent:finished (IPC)
  └→ flushBuffer(runId)              # commit any 200ms-debounced events
  └→ fetchRun(runId)                 # get_subagent_run → getRunCache
       └→ acc.rebuildFromCache(      # linear walk over authoritative
            row.transcriptJson,       # transcriptJson + final_text
            row.finalText)            # from PR1's column
            └→ liveSections.set(runId, acc.transcript.value)  # replace
```

`rebuildFromCache` parses `transcriptJson` (snake_case
`payload_json`) via `parseTranscriptJson()`, walks the entries
once, and assembles a fresh `TranscriptSection[]`. It also appends
a `FinalText` section from `row.finalText` (the PR1 column) when
non-null. The `publishAccumulator` helper then writes
`acc.transcript.value` into `liveSections`, which is the surface
the drawer (and eventually PR5's visual layer) reads.

`fetchRun` is the only entry point that calls
`rebuildFromCache`. The live `subagent:event` listener never does
— it calls `acc.feed()` only.

#### Performance contract (locked by test)

`app/src/stores/subagentRuns.test.ts` has a
`rebuilds 20k events in <500ms` benchmark that:
1. Generates 20k synthetic `chat_event` deltas + tool_call/result
   pairs (realistic mix).
2. Calls `acc.rebuildFromCache(jsonString, null)`.
3. Asserts the elapsed `performance.now()` is `< 500`.

Measured post-fix: **13.6ms** (36x headroom). The same test does
NOT exercise `feed()` in a 20k loop — feed is per-event and
debounced 200ms upstream, so 20k feeds are spread across 100s of
wall time and never bunch up. If a future change adds a bulk
`feedMany()` method, add a 20k benchmark for it too.

#### Dead code (flag for cleanup, not blocking)

The PR2 commit exports `chatEventSignature`, `appendFinalText`,
and `closeTextSegment` from `RunAccumulator` for future flows:

- `chatEventSignature` — will be used by a future "verify
  thinking-block signature on rehydrate" feature (mirrors main
  panel's signature-check on outbound payload). Currently unused.
- `appendFinalText` / `closeTextSegment` — will be used when
  streaming final text into the `Text` segment as the worker
  emits its terminal reply (today, `finalText` is set wholesale
  via `rebuildFromCache` from the DB column — no live path).
  Currently unused.

A future PR can either land consumers for these or remove the
exports. Keep them until then — they encode the intended public
API of the accumulator even if no caller exists yet.

---

### Per-Pin store lifecycle + worker transition (planned, PR5 / PR6)

`runAccumulator` instances and the `liveSections` Map entries
must be cleared when the parent session is deleted, when the
worker is detached (worktree transition), and on `chat-mode`
switch (so a stale runId from a previous session doesn't bleed
into a new chat). `clearSession()` handles the first case (paired
with `accumulators.delete(runId)` + `liveSections.delete(runId)`).
Worktree transitions and chat-mode switches are PR5 / PR6
concerns — flag here so the next reviewer doesn't miss them.

---

### subagentRuns PR3 reusable pieces (B6 redesign PR3, 2026-06-21)

PR3 ships two **independent, generic** building blocks that PR5
consumes but are not subagent-drawer-specific. They live under
`app/src/components/common/` and `app/src/utils/` (the project's
`use*-prefix-in-utils/` convention — `useKeyboard.ts` precedent —
NOT a separate `composables/` directory).

#### `MarkdownDetailModal.vue` (reka-ui 6-piece pattern)

```typescript
// app/src/components/common/MarkdownDetailModal.vue
interface Props {
  open: boolean;             // v-model:open
  title: string;
  markdown: string;          // full body — no truncation here
  source?: 'prompt' | 'reply' | 'worker' | null;  // header chip hint
}
```

6 reka-ui primitives + 1 `Icon` chip + `renderMarkdown()` body,
mirrors the existing `MemoryModal.vue` / `SettingsModal.vue`
pattern. `<style scoped>` requires `:deep(...)` selectors for
`DialogContent`'s portaled children (the `:deep()` gotcha from
`reka-ui-usage.md` applies). z-index 2000/2001 — above the
drawer's 1000 so the modal stacks correctly when both are open.
`source="worker"` is reserved for a future drawer surface
(PR5 only needs `prompt` + `reply`).

**Mount at chat-panel or App level, not inside the drawer** —
the drawer's v-if remounts the subtree on close, and the
modal's portal outlives the drawer anyway. PR5 will route the
`:open` ref through a Pinia slot (or co-locate with the
drawer's `openRunId`).

#### `useTruncate.ts` (pure markdown-safe truncation)

```typescript
// app/src/utils/useTruncate.ts — pure function, not a composable
export function truncate(
  text: string,
  maxChars: number,
  suffix: string = "…",  // single Unicode ellipsis (U+2026)
): string;
```

Single linear O(N) scan tracking two backtick states: a fence
toggled by runs of `>= 3` backticks, and an inline toggled by
runs of exactly 1 backtick. If the cut boundary lands inside
an open code region, backtrack to the most recent opener; fall
back to a hard cut at `maxChars` when the only safe boundary is
index 0 (prevents infinite-loop on degenerate input like a
string of backticks). Links (`[text](url)`) are allowed to be
cut at the boundary — only fenced / inline code regions get
the "push to safe boundary" treatment.

**Default budgets** (documented in file header, PR5 should not
need to dig into the implementation to find them):

| Source | `maxChars` | Suffix |
|---|---|---|
| `task` (worker prompt) | 120 | `…` |
| `finalText` (worker reply) | 280 | `…` |

The function is exported as a plain function — not wrapped in
`ref` / not a Vue composable. Pure transforms don't need
reactivity. Callers can wrap if they want; most won't.

**Common Mistake**: don't try to regex-parse markdown for
boundary detection. Markdown's grammar is context-sensitive
(links contain `[`, code contains backticks, fences can be
inside list items, etc.); regex would either over-trim
(false positives on `[` in code) or under-trim (false
negatives on `>3` backticks). The single linear scan with
explicit backtick-run state is the simplest correct approach.

**Performance contract** (locked by test): 100k chars + embedded
fences complete in <50ms. PR5's realistic input (`finalText`
typically <2k, `task` typically <500) is far below the stress
ceiling. If a future caller needs to truncate a megabyte of
markdown, the algorithm is still O(N) — no quadratic
backtracking.

#### Test gotchas (from memory)

- `Icon` stub has empty `textContent` (the rendered `<icon-stub
  name="..." />` is an SVG, not text). Assert on the inner
  `.markdown-detail-modal__title-text` span, not on the
  wrapper's `textContent` includes.
- reka-ui `DialogContent` portals to body; `wrapper.unmount()`
  doesn't clean up the portal. `beforeEach` must remove
  `.markdown-detail-modal` + `.markdown-detail-modal__overlay`
  from `document.body` to prevent cross-test leak.

---
