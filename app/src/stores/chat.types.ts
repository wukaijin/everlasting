// chat.types.ts — Public type surface for the chat store.
//
// This file is the "types layer" of the chat store. It is the single
// source of truth for every type/interface/const the rest of the
// app imports from `stores/chat` (other than the `useChatStore`
// factory function itself, which stays in `chat.ts`).
//
// Why split:
//   `chat.ts` was 1640 lines (a Pinia store facade + ~245 lines of
//   type/interface declarations + a handful of internal helpers).
//   The 14 exported types + `MODE_CYCLE` const are pure compile-time
//   declarations; inlining them with the ~1400 lines of store body
//   makes the file hard to read. Splitting types out gives a clean
//   "public contract" module that consumers (components, other
//   stores, utils) can import without pulling in the store body.
//
// Conventions locked (see PRD 06-23-06-23-split-chat-types):
//   - MOVE: every `export type/interface` declaration + the
//     `MODE_CYCLE` const that are part of the public API.
//   - KEEP in chat.ts: `type Role` (private), the
//     `ContentBlockPayload` / `ChatMessagePayload` private types
//     used only inside the `send` payload builder, the `genId`
//     helper, the `thinkingBlocksToText` export function (a
//     runtime helper, not a type), and the `useChatStore` factory.
//   - No behavior change — pure file/import reorganization.

/** Error category surfaced by LLM calls. Mirrors the Rust
 *  `llm::error::ErrorCategory` enum's `#[serde(rename_all =
 *  "snake_case")]` serialization. The display layer maps this to
 *  a Chinese user-facing message via the toast helper. */
export type ErrorCategory =
  | "auth"
  | "rate_limit"
  | "invalid_request"
  | "server"
  | "network";

/** Tool call info displayed in the UI. */
export interface ToolCallInfo {
  id: string;
  name: string;
  input: Record<string, unknown>;
}

/** Tool result info displayed in the UI. */
export interface ToolResultInfo {
  toolUseId: string;
  content: string;
  isError: boolean;
  /** F5 (LLM Latency Tracking): wall-clock duration of this
   *  specific tool invocation in milliseconds, measured as
   *  `tool:result_at - tool:call_at` by the frontend. `null`
   *  for pre-F5 rows (the field is embedded in the persisted
   *  `tool_result` block — see PRD R2 + ADR-lite decision 1).
   *  The ToolCallCard displays "0.3s" next to the status text
   *  when this is set. */
  durationMs?: number;
}

/** One thinking content block. The model can produce multiple blocks per
 *  turn (interleaved thinking with tool calls); each must be preserved
 *  in order and round-tripped back to the LLM verbatim, otherwise the
 *  next turn 400s. `text` is the streamed summary (or empty under
 *  `display: "omitted"`); `signature` is the opaque, encrypted blob. */
export interface ThinkingBlockInfo {
  text: string;
  signature: string;
}

/** 2026-06-26 (token-usage snapshot fix): per-session LAST-TURN
 *  token usage snapshot (NOT cumulative). Mirrors Rust
 *  `llm::types::TokenUsage` (snake_case) and the five `last_*`
 *  columns in the `sessions` table. `currentSessionTokenUsage` is
 *  `null` for sessions that have never sent a message
 *  (pre-snapshot rows, fresh sessions before their first LLM
 *  turn).
 *
 *  `context_input_tokens` is the cross-provider-normalized total
 *  input — the canonical numerator the ChatInput hint uses for
 *  "% of context_window" (Anthropic: input+cc+cr; OpenAI:
 *  prompt_tokens). The other four fields are the provider-native
 *  breakdowns rendered in the tooltip detail rows.
 *
 *  The legacy `*_total` cumulative fields (frozen, no longer
 *  written by production code) remain on `SessionSummary` for
 *  non-destructive migration but are not read by the UI. */
export interface SessionTokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  context_input_tokens: number;
}

/** F5 (LLM Latency Tracking): per-message latency breakdown
 *  measured by the frontend around the SSE event boundaries of
 *  one chat invocation. Mirrors the `MessageRow.ttfb_ms` /
 *  `gen_ms` / `total_ms` columns in the DB and the Rust
 *  `MessageLatency` struct in `db::sessions`.
 *
 *  All three fields are optional because:
 *  - Pre-F5 rows keep NULL → UI shows "—"
 *  - Cancel / error paths may only know `totalMs` (no `delta`
 *    was ever received → no `ttfbMs`)
 *  - Rehydrated messages from the DB inherit NULL when the row
 *    was inserted before the F5 columns existed.
 *
 *  `totalMs` is the only field the UI always renders (when
 *  present); `ttfbMs` and `genMs` are surfaced in the hover
 *  tooltip breakdown. */
export interface LatencyInfo {
  ttfbMs?: number;
  genMs?: number;
  totalMs?: number;
}

/** B2 PR3: per-token `@relpath` injection verdict for one user
 *  turn. `path` is the raw `@relpath` text (the `@` is stripped
 *  for storage so the row reads like a relative path, matching
 *  the placeholder wording "✓ 注入 48 行"). `action` is the
 *  per-token outcome — Injected (text file with line count),
 *  Degraded (image / PDF / Office / binary placeholder), or
 *  Skipped (out-of-root / missing / unreadable).
 *
 *  The shape mirrors the Rust `agent::at_file::InjectionAction`
 *  enum (see `app/src-tauri/src/agent/at_file.rs`). The
 *  wire-format enum is `tag = "kind", rename_all = "snake_case"`,
 *  matching the `tool_call` / `tool_result` discriminated-union
 *  convention in `streamController.ts`.
 *
 *  Field naming follows the snake_case wire:
 *  - `Injected` carries a `lines: number` field.
 *  - `Degraded` carries a `file_kind: "image" | "pdf" | "office"
 *    | "binary"` field (the Rust `FileKind` enum's
 *    `rename_all = "snake_case"` makes the JSON values lowercase,
 *    so `"image"` / `"pdf"` / `"office"` / `"binary"`).
 *  - `Skipped` carries a `reason: "out_of_root" | "missing" |
 *    "unreadable"` field (the Rust `SkipReason` enum's
 *    `rename_all = "snake_case"` makes the JSON values
 *    `out_of_root` / `missing` / `unreadable`).
 *
 *  The frontend maps these to the human-readable Chinese
 *  labels in `FileInjectionsHint.vue`. An unknown variant
 *  falls back to the raw string / "未知".
 *
 *  Stored in `messages.metadata` (JSON column) — the
 *  rehydrate path parses it back here on session load. Also
 *  pushed live via `ChatEvent::FileInjections` so the hint
 *  row appears before the assistant's first delta. */
export type InjectionRecord =
  | { kind: "injected"; lines: number }
  | {
      kind: "degraded";
      file_kind: "image" | "pdf" | "office" | "binary";
    }
  | {
      kind: "skipped";
      reason: "out_of_root" | "missing" | "unreadable";
    };

/** B2 PR3: the visible shape stored on the message — `path`
 *  lives on the record wrapper so each entry is self-contained
 *  in the hint row. Mirrors the Rust `InjectionRecord` struct
 *  (path + action). The array is empty/undefined for user
 *  messages that never had `@relpath` tokens, AND for
 *  non-user messages (assistant / system) — the agent loop
 *  only attaches the manifest to the last user turn. */
export interface InjectionEntry {
  path: string;
  action: InjectionRecord;
}

/** Chat message with optional tool call/result/thinking metadata. */
export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string; // accumulated text content
  streaming?: boolean;
  error?: { message: string; category: ErrorCategory };
  toolCalls?: ToolCallInfo[];
  toolResults?: ToolResultInfo[];
  /** All thinking blocks emitted by the model for this message, in
   *  streaming order. Empty/missing for messages without thinking. */
  thinkingBlocks?: ThinkingBlockInfo[];
  /** Each entry is the opaque `data` payload of a `redacted_thinking`
   *  block — preserved verbatim for round-trip, never displayed. */
  redactedThinkingData?: string[];
  /** F5 (LLM Latency Tracking): per-message latency breakdown
   *  (TTFB / gen / total in ms). Rehydrated from the
   *  `messages.ttfb_ms` / `gen_ms` / `total_ms` columns on
   *  session load; the frontend `streamController` populates
   *  it during streaming (via `Date.now()` deltas) and
   *  fires `update_message_latency` IPC at `done` to persist.
   *  Missing for pre-F5 / user-role / system-event rows. */
  latency?: LatencyInfo;
  /** F5: the seq the agent loop assigned to this row when it
   *  was persisted. Used by the `update_message_latency` IPC
   *  to look up the SQLite id via `find_message_id_by_seq`.
   *  Set during rehydrate (from `messages.seq`); the
   *  controller's streaming path tracks the assistant
   *  placeholder's seq in `RequestState` instead. */
  seq?: number;
  /** F5 follow-up: how long the model spent in the thinking
   *  phase for this turn (drives the "Thought for X.Xs"
   *  header in `ThinkingBlock.vue`, replacing the prior
   *  "X tokens" estimate). Captured by the streaming
   *  `streamController` from `RequestState.thinkingDurationMs`
   *  on the first non-thinking boundary (text `delta`,
   *  `tool:call`, `done`, or `error`). In-memory only for
   *  now — no DB column — so a page reload loses it and the
   *  header falls back to "—". A future F5-follow-up can
   *  add a `messages.thinking_ms` column + an
   *  `update_message_thinking` IPC, mirroring the
   *  latency-tracking pattern, if reload-survival matters. */
  thinkingDurationMs?: number;
  /** B2 PR3: per-user-turn `@relpath` injection manifest,
   *  parallel to `toolCalls` / `toolResults` (one message
   *  can have BOTH tool cards AND a hint row when the
   *  assistant decides to use tools after seeing the
   *  injected files). Empty/missing for messages without
   *  `@relpath` tokens. Set by:
   *
   *  - The streaming `streamController` on
   *    `ChatEvent::FileInjections` (live path).
   *  - The rehydrate path (`rehydrateMessages` in
   *    `streamController.ts`) on session load, parsing
   *    `MessageRow.metadata` JSON.
   *
   *  The MessageItem hint row renders when
   *  `msg.role === "user" && msg.injections?.length` —
   *  assistant rows never have `@` references and the
   *  field stays undefined. */
  injections?: InjectionEntry[];
  /** D3 PR3 (2026-06-17): per-message metadata JSON,
   *  parsed from `MessageRow.metadata`. Currently used
   *  for the D3 "edited" affordance — when the row has
   *  `metadata.edited_at`, the MessageItem renders a small
   *  "(edited)" grey label next to the bubble. Rehydrated
   *  on session load by `rehydrateMessages` (parses the
   *  same JSON column B2 PR3 uses for the injection
   *  manifest). In-memory only — mutations stay on the
   *  backend via `edit_user_message`. The shape is
   *  loosely typed (a free-form JSON object) so future
   *  metadata fields (e.g. `original_content` for undo)
   *  don't require touching this interface. */
  metadata?: Record<string, unknown>;
}

/** Session summary shown in the sidebar. Snake_case to match PR1's
 *  Rust serialization (no `#[serde(rename_all = "camelCase")]`). */
export interface SessionSummary {
  id: string;
  title: string;
  updated_at: string;
  preview: string;
  project_id: string;
  current_cwd: string;
  /** Worktree path (or `null` for sessions in `none` / `detached`
   *  state). Step 4 follow-up: the worktree is now opt-in. */
  worktree_path: string | null;
  /** Tri-state worktree state: `none` (never attached), `active`
   *  (currently bound), or `detached` (was active, now unbound,
   *  but the branch + directory are still on disk for re-attach).
   *  See `db::WorktreeState` in the Rust source. */
  worktree_state: "none" | "active" | "detached";
  /** Path of the most recently detached worktree. Used to
   *  re-attach or just for display in the "上次 worktree" chip. */
  last_worktree_path: string | null;
  /** PR4 of multi-model: per-session model override. `null` when the
   *  session uses the global default model. Soft FK to `models.id`. */
  model_id: string | null;
  /** A4 (Token Usage Tracking): cumulative per-session token
   *  totals as of the last LLM turn. `null` for pre-A4
   *  sessions (the migration is non-destructive, so legacy
   *  rows keep NULL until their first post-upgrade turn).
   *  FROZEN 2026-06-26 (snapshot fix): no longer written by
   *  production code; kept for non-destructive migration.
   *  The ChatInput hint reads `last_*` instead. */
  input_tokens_total: number | null;
  output_tokens_total: number | null;
  cache_creation_total: number | null;
  cache_read_total: number | null;
  /** 2026-06-26 (token-usage snapshot fix): per-session LAST-TURN
   *  token usage snapshot (NOT cumulative). The ChatInput hint
   *  reads `last_context_input_tokens` for the "% of
   *  context_window" line; the tooltip detail reads the four
   *  provider-native breakdowns. All five are `null` on
   *  pre-snapshot sessions (the migration is non-destructive). */
  last_context_input_tokens: number | null;
  last_input_tokens: number | null;
  last_output_tokens: number | null;
  last_cache_creation: number | null;
  last_cache_read: number | null;
  /** D1 (Color Tag): palette index 0-7, null = no mark. */
  color_tag: number | null;
  /** A2 + B7 (Permission system + per-session Mode, 2026-06-13;
   *  3 档化 2026-06-13): the session's current mode. The Rust
   *  side serializes the `Mode` enum as its lowercase string
   *  (`edit` / `plan` / `yolo` / `background`); `Background` is
   *  reserved in the enum and never appears in the UI. PR2
   *  wires the frontend ModeSelect to flip this via
   *  `set_session_mode` IPC. See
   *  `app/src-tauri/src/db/types.rs::Mode`. */
  mode: "edit" | "plan" | "yolo" | "background";
}

/** User-facing mode subset — the three modes the MVP UI exposes.
 *  Excludes `Background` (reserved in the backend enum for
 *  schema stability but never shown to the user). */
export type SessionMode = "edit" | "plan" | "yolo";

/** Cycle order for `useKeyboard` Shift+Tab iteration. Matches
 *  Claude Code's `Shift+Tab` cycle convention: Edit → Plan →
 *  Yolo → Edit (forward). 3 档化 2026-06-13: Review 移除， Yolo
 *  紧跟 Plan 后面。 */
export const MODE_CYCLE: SessionMode[] = ["edit", "plan", "yolo"];

/** One file in the worktree diff (step 4 / PR3). Mirror of the
 *  Rust `git::diff::FileDiff` struct. Field names are
 *  intentionally snake_case to match the IPC payload. */
export interface FileDiff {
  path: string;
  status: string;
  added: number;
  removed: number;
  diff_text: string;
}

/** The full diff for a session: the file list plus a structured
 *  per-file payload. `files` is empty when the worktree matches
 *  the base (no edits yet, OR pre-step-4 session with no
 *  worktree). */
export interface DiffResult {
  files: FileDiff[];
}
