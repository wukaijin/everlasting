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
- `TranscriptKind` snake_case: `"chat_event" | "tool_call" | "tool_result" | "permission_ask"`.
  **Must mirror the Rust `#[serde(rename_all = "snake_case")]` exactly — drift is a cross-layer bug.**

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
