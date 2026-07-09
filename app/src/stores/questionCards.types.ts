// questionCards.types.ts — Public type surface for the questionCards store.
//
// This file is the "types layer" of the questionCards store — the
// single source of truth for every type/interface the rest of the
// app imports from `stores/questionCards`. Conventions locked
// (mirrors `subagentRuns.types.ts` + `chat.types.ts`):
//   - MOVE: every `export type/interface` declaration that is part
//     of the public API.
//   - KEEP in questionCards.ts: `useQuestionCardsStore` factory +
//     minimal store actions.
//
// Why split (2026-06-30, Phase C of `06-30-ask-user-question-tool`):
//   the wire types for `tool:question` (Rust `ToolQuestionPayload`)
//   + `tool:question_resolved` IPC are non-trivial — 5+ types
//   (`Question`, `QuestionOption`, `ToolQuestionPayload`,
//   `ToolQuestionAnswer`, `ToolQuestionResolvePayload`,
//   `QuestionCardState`). Inlining them in the store file bloats
//   it; splitting gives a clean public-contract module that
//   consumers (AskUserQuestionCard in Phase D, MessageItem in
//   Phase E, the streamController integration) can import without
//   pulling in the store body.
//
// ⚠️ Cross-layer drift traps (see
//    `.trellis/spec/backend/tool-contract.md` §ask_user_question):
//   1. The Rust `ToolQuestionPayload` struct emits snake_case on the
//      wire (no `rename_all`). The frontend types below mirror
//      snake_case verbatim (`session_id`, `tool_use_id`,
//      `multi_select`, etc.) — do NOT camelCase them. Tauri 2
//      auto-converts JS camelCase args → Rust snake_case on
//      `invoke()`; the `listen<>` payload comes through AS-IS
//      (snake_case from the Rust serde default).
//   2. The `cancelled: true` field on `ToolQuestionResolvePayload`
//      is the "user skipped" marker; the backend derives a
//      `CANCELLED_MARKER` (`{"cancelled": true}`) on the tool_result
//      so the LLM sees a uniform shape regardless of cancel origin
//      (per PRD R5: `{"cancelled": true}` is the canonical wire for
//      "user explicitly chose not to answer"). The IPC payload
//      carries `cancelled?: true` (literal `true`, not a string)
//      to be wire-compatible with the Rust `Option<bool>` payload.
//   3. The `questions` array length is 1..=4; `options` per
//      question is 2..=4; `header` (when present) is ≤12 chars.
//      These bounds are validated server-side in
//      `tools::ask_user_question::execute_blocking`; the frontend
//      mirrors them for client-side early validation (UX hint
//      only — the backend is the source of truth).

/** Tool name for the blocking question tool (mirrors the Rust
 *  `definition().name` in
 *  `app/src-tauri/src/tools/ask_user_question.rs`). Frontend
 *  consumers key on this constant for the MessageItem dispatch
 *  (Phase E): tool_use blocks with `name === ASK_USER_QUESTION_TOOL_NAME`
 *  route to `<AskUserQuestionCard>`, everything else routes to
 *  `<ToolCallCard>`. */
export const ASK_USER_QUESTION_TOOL_NAME = "ask_user_question";

/** Tool name for the blocking mode-change request tool (mirrors
 *  the Rust `definition().name` in
 *  `app/src-tauri/src/tools/request_mode_change.rs`). Frontend
 *  consumers key on this constant for the MessageItem dispatch
 *  (Phase D, 2026-07-07): tool_use blocks with `name === REQUEST_MODE_CHANGE_TOOL_NAME`
 *  route to `<RequestModeChangeCard>`, everything else routes to
 *  `<ToolCallCard>`. Mutually exclusive with
 *  `ASK_USER_QUESTION_TOOL_NAME` — both tools share the same
 *  per-session single-pending mutex on the backend's
 *  QuestionStore (only one pending interaction per session). */
export const REQUEST_MODE_CHANGE_TOOL_NAME = "request_mode_change";

/** Tool name for the blocking workflow state-transition request
 *  tool (mirrors the Rust `definition().name` in
 *  `app/src-tauri/src/tools/request_task_state_transition.rs`).
 *  Frontend consumers key on this constant for the MessageItem
 *  dispatch (2026-07-09, `07-09-workflow-transition-card`):
 *  tool_use blocks with `name === REQUEST_TASK_STATE_TRANSITION_TOOL_NAME`
 *  route to `<RequestTaskStateTransitionCard>`. Mutually exclusive
 *  with the two tools above — all three share the same per-session
 *  single-pending mutex on the backend's QuestionStore. */
export const REQUEST_TASK_STATE_TRANSITION_TOOL_NAME =
  "request_task_state_transition";

/** Tauri event channel name (backend → frontend). Distinct from
 *  `tool:call` / `tool:result` / `permission:ask` so the listener
 *  can be wired independently (per the design's §5.4 routing). */
export const TOOL_QUESTION_EVENT = "tool:question";

/** Tauri command name (frontend → backend). Routes to
 *  `commands::question::resolve_tool_question`, which calls
 *  `QuestionStore.resolve(session_id, response)`. */
export const RESOLVE_TOOL_QUESTION_CMD = "resolve_tool_question";

/** Tauri command name (frontend → backend). Routes to
 *  `commands::question::get_pending_question`, which calls
 *  `QuestionStore.get(session_id)`. Returns `Option<ToolQuestionPayload>`
 *  (snake_case payload), `null` when no pending question exists
 *  for the session. The streamController calls this on
 *  `ensureLoaded` to fetch the authoritative backend state (the
 *  QuestionStore lives in `AppState`, NOT in the LRU-bounded
 *  `messagesBySession`, so it survives session-switch reloads
 *  intact — see design §5.4 source-of-truth rationale). */
export const GET_PENDING_QUESTION_CMD = "get_pending_question";

// -----------------------------------------------------------------------
// Wire types (mirrors Rust `ToolQuestionPayload` + children, snake_case)
// -----------------------------------------------------------------------

/** Single option inside a question. Mirrors the Rust `Option` struct
 *  in `ask_user_question.rs`: `label: String` (required),
 *  `description: Option<String>`, `preview: Option<String>`.
 *  Wire is snake_case (no `rename_all` on the Rust struct). */
export interface QuestionOption {
  label: string;
  /** Free-text description rendered under the label. Optional. */
  description?: string;
  /** Markdown body rendered in a collapsible preview panel. Optional. */
  preview?: string;
}

/** One question in the agent's blocking prompt. Mirrors the Rust
 *  `Question` struct: `question: String` (required), `header:
 *  Option<String>` (≤12 chars on the wire — backend validates),
 *  `options: Vec<Option>` (2..=4), `multi_select: bool` (default
 *  false). Wire is snake_case. */
export interface Question {
  question: string;
  /** ≤12 chars — backend schema check rejects longer. Optional. */
  header?: string;
  /** 2..=4 options (backend validates). */
  options: QuestionOption[];
  /** Multi-select checkbox vs single-select radio. Backend default
   *  is `false` when omitted (Rust `Option<bool>` defaults to None
   *  → false on the JSON wire). */
  multi_select: boolean;
}

/** `tool:question` event payload (backend → frontend). Mirrors the
 *  Rust `ToolQuestionPayload` struct verbatim (snake_case).
 *  Emitted by `tools::ask_user_question::execute_blocking` AFTER
 *  the QuestionStore.register + oneshot setup, BEFORE the
 *  `tokio::select!{cancel, oneshot.recv()}` wait. The streamController
 *  listener receives this payload and routes it into the
 *  `questionCards` store.
 *
 *  Field-by-field:
 *  - `session_id` — the session the pending question belongs to.
 *    Per-session routing (matches `permission:ask`'s `sessionId`).
 *  - `tool_use_id` — the LLM-assigned id matching the assistant
 *    `ToolUse(ask_user_question)` block. Phase E's MessageItem
 *    dispatch uses this to pair the card with its tool_use row
 *    in the message stream (so the card sits BELOW the right
 *    `ToolCallCard` / `ToolUseBlock`).
 *  - `questions` — 1..=4 question objects (per schema).
 *  - `ts` — unix ms timestamp (frontend ordering tiebreaker;
 *    `pendingBySession` keys by session_id, but `ts` lets the
 *    card show "asked at X.Xs ago" in a future polish PR). */
export interface ToolQuestionPayload {
  session_id: string;
  tool_use_id: string;
  questions: Question[];
  ts: number;
}

/** One question's answer on the way back to the backend. Mirrors
 *  the Rust `QuestionAnswer` struct verbatim (snake_case).
 *  Carried inside `ToolQuestionResolvePayload.answer` (an array,
 *  one entry per `questions[i]`, preserving the order).
 *
 *  `options` is the array of selected labels (string-array, NOT
 *  indices) — single-select → 1 element; multi-select → N elements.
 *  Backend accepts the labels verbatim (no ID lookup needed; the
 *  card renders labels, the LLM context sees labels). */
export interface ToolQuestionAnswer {
  question: string;
  /** Echo of the question's `header` (≤12 chars). Optional —
 *    present iff the question had a header in the original payload. */
  header?: string;
  /** Selected option labels (1 element for single-select, N for
 *    multi-select). Backend schema requires `length >= 1`. */
  options: string[];
  /** Echo of the question's `multi_select` flag. Backend uses
 *    this to validate the answer shape (single-select → 1 label;
 *    multi-select → N labels). */
  multi_select: boolean;
}

/** `tool:question_resolved` IPC payload (frontend → backend).
 *  Routes to `commands::question::resolve_tool_question` →
 *  `QuestionStore.resolve(session_id, response)`.
 *
 *  Mutually exclusive union: `answer` is set on a real answer;
 *  `cancelled` is set on user-initiated skip. The backend accepts
 *  both; setting both is a malformed payload (frontend invariant
 *  guarantees exclusive). Snake_case wire.
 *
 *  `tool_use_id` MUST match the original payload's `tool_use_id`
 *  for the backend to route the response to the right oneshot
 *  (QuestionStore keys by session_id only; the tool_use_id is
 *  echoed back as a sanity check — the `rid` from `permission:ask`
 *  is NOT used here because questions are session-singleton, not
 *  rid-keyed). */
export interface ToolQuestionResolvePayload {
  session_id: string;
  tool_use_id: string;
  /** Real answer — one entry per `questions[i]` in the original
   *  payload, in the same order. Backend coerces → tool_result
   *  block format (PRD R4). */
  answer?: ToolQuestionAnswer[];
  /** User explicitly skipped. Wire is literal `true` (Rust
   *  `Option<bool>` serializes to JSON `true` / omitted). */
  cancelled?: true;
}

// -----------------------------------------------------------------------
// Frontend card state (frontend-only, NOT on the wire)
// -----------------------------------------------------------------------

/** Per-card UI state, driven by the
 *  `streamController.handleToolQuestion` listener + the card's
 *  own submit / skip actions. Frontend-only — never crosses the
 *  IPC boundary. The card itself owns the selection state
 *  (Phase D) and uses this enum to gate the bottom buttons:
 *  "pending" → show 提交 / 跳过; "answered" → show 已选项摘要 +
 * 展开保留; "cancelled" → show 已跳过 + 展开保留. */
export type QuestionCardState = "pending" | "answered" | "cancelled";

/** The store's internal record (per pending question). Frontend-only
 *  — the `tool_use_id` lets Phase E's MessageItem dispatch pair
 *  the card with its tool_use row; the `payload` is the original
 *  `ToolQuestionPayload` (preserved for re-renders). The
 *  `selectedAnswer` is `undefined` while the user hasn't
 *  submitted; populated on submit (Phase D) so the card can render
 *  the "answered" state with selected highlights. */
export interface PendingQuestion {
  sessionId: string;
  toolUseId: string;
  questions: Question[];
  /** Echo of the original payload's `ts` (unix ms). Frontend can
   *  use it for sort ordering if multiple questions arrive for
   *  the same session in a race (single pending mutex — see
   *  QuestionStore.register — makes this rare but possible
   *  during a transient cross-session race). */
  ts: number;
}

// -----------------------------------------------------------------------
// `request_mode_change` tool wire types (07-07-07-07, 2026-07-07)
// -----------------------------------------------------------------------

/** `mode:change:request` event payload (backend → frontend).
 *  Mirrors the Rust `ModeChangePayload` struct verbatim
 *  (snake_case, no `rename_all`). The backend's
 *  `request_mode_change::execute_blocking` emits this AFTER
 *  QuestionStore.register + BEFORE the `tokio::select!{cancel,
 *  oneshot.recv()}` wait — same interception pattern as the
 *  question tool. The streamController listener routes it into
 *  the `questionCards` store wrapped as
 *  `{ kind: "mode_change", payload: ModeChangePayload }`.
 *
 *  Field-by-field:
 *  - `session_id` — the session the pending change belongs to.
 *  - `tool_use_id` — the LLM-assigned id matching the assistant
 *    `ToolUse(request_mode_change)` block. MessageItem dispatch
 *    uses this to pair the card with its tool_use row.
 *  - `target_mode` — the mode the LLM wants (`"edit" | "plan" |
 *    "yolo"`). Backend validates against `VALID_MODES` enum.
 *  - `current_mode` — optional snapshot at tool-invocation time
 *    (used to render the "当前: X → 目标: Y" comparison pill;
 *    `null` is reserved for future "session pre-load not yet
 *    resolved" edge cases — production always populates).
 *  - `reason` — LLM-supplied explanation (≤500 chars). Optional.
 *  - `ts` — unix ms (display ordering, "asked Xs ago"). */
export interface ModeChangePayload {
  session_id: string;
  tool_use_id: string;
  target_mode: "edit" | "plan" | "yolo";
  current_mode?: "edit" | "plan" | "yolo" | null;
  reason?: string;
  ts: number;
}

/** `resolve_mode_change` IPC payload (frontend → backend). Routes
 *  to `commands::question::resolve_mode_change` →
 *  `QuestionStore.resolve(session_id, InteractionResponse)`.
 *
 *  `allow` is the user's decision: `true` = allow + apply mode +
 *  write `mode_change_allowed` audit; `false` = deny + write
 *  `mode_change_denied` audit + tool_result becomes
 *  `{ cancelled_by_user: true }`. The backend resolves the
 *  oneshot AFTER applying the mode change (see
 *  `commands::question::resolve_mode_change` for the ordering
 *  rationale) — the frontend doesn't need to await any second
 *  IPC.
 *
 *  `targetMode` is sent separately (not extracted from the
 *  original payload) because the backend's `resolve_mode_change`
 *  IPC applies the mode internally via
 *  `set_session_mode_internal` — it doesn't fetch the pending
 *  payload first. This keeps the IPC self-contained.
 *
 *  CamelCase at the JS layer (Tauri auto-translates to Rust
 *  snake_case at the IPC boundary). */
export interface ModeChangeResolvePayload {
  sessionId: string;
  toolUseId: string;
  targetMode: "edit" | "plan" | "yolo";
  allow: boolean;
}

/** The four workflow state-machine states (mirrors the Rust
 *  `WorkflowDef.states` for the dev plugin +
 *  `TaskStatus::as_str`). Used by the task-state-transition
 *  payload + card props. The literal union lets vue-tsc catch a
 *  typo at the dispatch site (vs a bare `string`). */
export type WorkflowState = "planning" | "implement" | "check" | "done";

/** Backend → frontend payload emitted on the
 *  `task:state:transition:request` channel (and returned inside
 *  `PendingInteractionEntry.payload` by `get_pending_interaction`).
 *  Mirrors the Rust `TaskStateTransitionPayload`
 *  (`agent/question_store.rs`) which has NO `rename_all` — fields
 *  are snake_case on the wire (shared IPC camelCase exemption,
 *  same posture as `ModeChangePayload`).
 *
 *  Emitted by the `request_task_state_transition` blocking tool
 *  when it registers a pending transition in the QuestionStore +
 *  fires the event. The streamController listener wraps it into
 *  the store's tagged-union `{ kind: "task_state_transition",
 *  payload }`.
 *
 *  Field-by-field:
 *  - `session_id` — the workflow session the transition belongs to.
 *  - `tool_use_id` — matches the assistant
 *    `ToolUse(request_task_state_transition)` block; MessageItem
 *    dispatch pairs the card with its tool_use row via this.
 *  - `target_state` — the state the agent wants to move to
 *    (`planning`/`implement`/`check`/`done`). Backend validates
 *    against the workflow's states.
 *  - `current_state` — optional snapshot at tool-invocation time
 *    (rendered in the "当前 → 目标" comparison row; omitted on the
 *    wire when the backend had no workflow task resolved).
 *  - `slug` — the task's slug (locates
 *    `.everlasting/tasks/<slug>/task.json`). REQUIRED by the
 *    resolve IPC (the handler has no WorkflowCtx, so it re-reads
 *    `from` off disk via this slug). Omitted on the event wire only
 *    when the backend couldn't resolve a slug (rare; the card
 *    falls back to disabling allow).
 *  - `reason` — agent-supplied explanation (≤500 chars). Optional.
 *  - `ts` — unix ms (display ordering). */
export interface TaskStateTransitionPayload {
  session_id: string;
  tool_use_id: string;
  target_state: WorkflowState;
  current_state?: WorkflowState | null;
  slug?: string;
  reason?: string;
  ts: number;
}

/** `resolve_task_state_transition` IPC payload (frontend → backend).
 *  Routes to `commands::question::resolve_task_state_transition` →
 *  `workflow::set_task_state` (the single writer for task.json.status
 *  + the `from → to` Rust hook, e.g. Check→Done triggers spec
 *  distillation).
 *
 *  `allow` is the user's decision: `true` = apply the transition
 *  (write task.json + dispatch hook) + write
 *  `task_state_transition_allowed` audit; `false` = skip
 *  `set_task_state` entirely + write `..._denied` audit +
 *  tool_result becomes `{ cancelled_by_user: true }`.
 *
 *  Differs from `ModeChangeResolvePayload` in ONE field: **`slug`**
 *  is required here because the IPC handler has no WorkflowCtx and
 *  must locate `<project>/.everlasting/tasks/<slug>/task.json` to
 *  read the current `from` state off disk. There is NO `fromState`
 *  arg — the backend reads it fresh (the card does not echo it).
 *
 *  CamelCase at the JS layer (Tauri auto-translates to Rust
 *  snake_case at the IPC boundary). */
export interface TaskStateTransitionResolvePayload {
  sessionId: string;
  toolUseId: string;
  targetState: WorkflowState;
  slug: string;
  allow: boolean;
}

/** Tagged union carrying a `ToolQuestionPayload` (the
 *  `ask_user_question` flow), a `ModeChangePayload` (the
 *  `request_mode_change` flow), or a `TaskStateTransitionPayload`
 *  (the `request_task_state_transition` flow). Mirrors the Rust
 *  `PendingInteraction` enum's wire shape — `#[serde(tag =
 *  "kind", rename_all = "snake_case")]` — so the JSON arrives as
 *  `{ kind: "question", ... }` / `{ kind: "mode_change", ... }` /
 *  `{ kind: "task_state_transition", ... }` with the variant's
 *  fields flattened alongside `kind`. Single-pending-mutex
 *  semantics (only one variant per session) come from the
 *  backend's QuestionStore; this type is the frontend's mirror.
 *
 *  The `pendingBySession` Pinia cache stores these directly —
 *  callers read `getPending(sid)?.kind` to dispatch to the right
 *  card component (`<AskUserQuestionCard>` /
 *  `<RequestModeChangeCard>` / `<RequestTaskStateTransitionCard>`). */
export type PendingInteraction =
  | { kind: "question"; payload: ToolQuestionPayload }
  | { kind: "mode_change"; payload: ModeChangePayload }
  | { kind: "task_state_transition"; payload: TaskStateTransitionPayload };

/** IPC return shape of `get_pending_interaction`. Mirrors the
 *  Rust `PendingInteractionEntry` struct (top-level `kind` +
 *  nested tagged `payload`). The outer `kind` lets callers
 *  short-circuit on `entry.kind === "mode_change"` (etc.) without
 *  parsing the tagged enum first; the `payload` carries the typed
 *  variant (dispatch on `kind`, read matching fields).
 *
 *  Why a wrapper (not just `PendingInteraction` directly)? The
 *  Rust side's `get_payload` returns
 *  `PendingInteractionEntry { kind, payload }` so the IPC caller
 *  can `entry.kind` without consuming the payload. The frontend
 *  mirrors this 1:1. */
export interface PendingInteractionEntry {
  kind: "question" | "mode_change" | "task_state_transition";
  payload: PendingInteraction;
}

/** Tauri event channel name for `request_mode_change` (backend →
 *  frontend). Distinct from `tool:question` so the listener can
 *  be wired independently. Mirrors the Rust `app.emit("mode:change:request",
 *  payload)` site in `state.rs::AppHandleSink::emit_mode_change_request`. */
export const MODE_CHANGE_EVENT = "mode:change:request";

/** Tauri command name for `resolve_mode_change` (frontend →
 *  backend). Routes to `commands::question::resolve_mode_change`,
 *  which calls `QuestionStore.resolve(session_id, response)`
 *  AFTER applying the mode via `set_session_mode_internal`. */
export const RESOLVE_MODE_CHANGE_CMD = "resolve_mode_change";

/** Tauri event channel name for `request_task_state_transition`
 *  (backend → frontend, 2026-07-09). Distinct from the two channels
 *  above so the listener wires independently. Mirrors the Rust
 *  `app.emit("task:state:transition:request", payload)` site in
 *  `state.rs::AppHandleSink::emit_task_state_transition`. */
export const TASK_STATE_TRANSITION_EVENT = "task:state:transition:request";

/** Tauri command name for `resolve_task_state_transition` (frontend
 *  → backend). Routes to
 *  `commands::question::resolve_task_state_transition`, which calls
 *  `workflow::set_task_state` (BEFORE resolving the oneshot) when
 *  allowed — the single writer for task.json.status + the
 *  `from → to` Rust hook. */
export const RESOLVE_TASK_STATE_TRANSITION_CMD =
  "resolve_task_state_transition";

/** Tauri command name for `get_pending_interaction` (frontend →
 *  backend). Routes to `commands::question::get_pending_interaction`,
 *  which calls `QuestionStore.get_payload(session_id)`. Returns
 *  `Option<PendingInteractionEntry>` (snake_case payload), `null`
 *  when no pending interaction exists for the session. The
 *  streamController calls this on `ensureLoaded` to fetch the
 *  authoritative backend state. */
export const GET_PENDING_INTERACTION_CMD = "get_pending_interaction";