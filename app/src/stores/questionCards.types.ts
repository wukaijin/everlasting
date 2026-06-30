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