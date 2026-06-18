// useChecklistStore — B12 Checklist agent self-tracking (PR2 frontend).
//
// The backend `update_checklist` virtual tool (PR1, commit `994db84`)
// atomically replaces a per-request `Vec<ChecklistItem>` held inside
// the agent loop's run scope. The frontend derives the current
// checklist for display from the tool_use INPUT (not the tool_result,
// which is rendered text for the LLM).
//
// Data flow:
//   1. Live streaming — `handleToolCall` is invoked by the
//      streamController when a `tool:call` event for `update_checklist`
//      arrives. We parse `input.items` (array of `{content, status}`),
//      re-coerce at-most-one `in_progress` client-side (mirroring
//      PR1's `coerce_at_most_one_in_progress` pure fn), and store as
//      the session's current checklist. We do NOT wait for the
//      matching `tool:result` — the input is the source of truth and
//      the result text is purely for LLM consumption.
//   2. Reload — on session switch / load, the chat store rehydrates
//      messages from DB. We scan those messages for the LAST
//      `update_checklist` tool_use whose paired tool_result has
//      `is_error === false`. The `is_error` filter is critical:
//      a cancelled `update_checklist` (RULE-A-004) gets a synthetic
//      `is_error: true` tool_result ("Tool execution was interrupted
//      ..."); rendering it would show interrupt text as checklist
//      state.
//
// Per-session storage: `checklistBySession: Map<sessionId, ChecklistItem[]>`.
// Switching sessions shows that session's checklist (or hides if none).
//
// Reset semantics: B12 is per-request lifetime. The backend wipes
// the loop-local Vec on every new `run_chat_loop` invocation; the
// frontend mirrors this on `send()` — see `clearForNewRun` wired from
// chat.ts. Reload from DB derives from history so the card shows on
// rehydrate.

import { defineStore } from "pinia";
import { reactive } from "vue";

/** Tool name (mirrors PR1's `definition().name`). */
export const CHECKLIST_TOOL_NAME = "update_checklist";

/** Status of a checklist item. Mirrors the Rust `ChecklistStatus`
 *  enum's `#[serde(rename_all = "snake_case")]` wire values. */
export type ChecklistStatus = "pending" | "in_progress" | "done";

/** One checklist item. Mirrors the Rust `ChecklistItem` struct's
 *  JSON shape `{ content, status }`. */
export interface ChecklistItem {
  content: string;
  status: ChecklistStatus;
}

/** Re-coerce at-most-one `in_progress` client-side.
 *
 *  Mirrors PR1's `coerce_at_most_one_in_progress` in
 *  `app/src-tauri/src/tools/update_checklist.rs`: keep the LAST
 *  `in_progress` item (by array order) and demote any earlier ones
 *  to `pending`. Pure function — does NOT mutate the input.
 *
 *  We replicate this client-side because the live `tool:call` event
 *  carries the model's RAW input (pre-coerce) — the coerce happens
 *  inside the Rust `execute()` body. Displaying the raw input would
 *  momentarily show multiple `in_progress` items before the
 *  `tool:result` arrives; coercing here keeps the card consistent
 *  with the post-coerce state the LLM actually sees. */
export function coerceAtMostOneInProgress(
  items: ReadonlyArray<ChecklistItem>,
): ChecklistItem[] {
  let lastInProgress = -1;
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].status === "in_progress") {
      lastInProgress = i;
      break;
    }
  }
  const out: ChecklistItem[] = [];
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (item.status === "in_progress" && i !== lastInProgress) {
      out.push({ content: item.content, status: "pending" });
    } else {
      out.push({ content: item.content, status: item.status });
    }
  }
  return out;
}

/** Parse a raw `input.items` value (from a tool_use block or a live
 *  `tool:call` event payload) into a coerced `ChecklistItem[]`.
 *
 *  Defensive parsing mirrors PR1's `parse_and_coerce`:
 *  - Non-array `items` → empty Vec.
 *  - An item missing `content` (or non-string) → skipped.
 *  - An item with unrecognized `status` → coerced to `pending`.
 *  - Then at-most-one-`in_progress` coercion runs.
 *
 *  Exported for the store + the vitest. */
export function parseAndCoerceItems(raw: unknown): ChecklistItem[] {
  if (!Array.isArray(raw)) return [];
  const parsed: ChecklistItem[] = [];
  for (const entry of raw) {
    if (!entry || typeof entry !== "object") continue;
    const content = (entry as { content?: unknown }).content;
    if (typeof content !== "string") continue;
    const statusRaw = (entry as { status?: unknown }).status;
    let status: ChecklistStatus;
    if (statusRaw === "in_progress") status = "in_progress";
    else if (statusRaw === "done") status = "done";
    else status = "pending"; // unknown / missing / "pending" / anything else
    parsed.push({ content, status });
  }
  return coerceAtMostOneInProgress(parsed);
}

/** A rehydrated message in the minimal shape we need for the
 *  reload scan. This is a structural subset of the production
 *  `ChatMessage` interface in `chat.ts` — the chat store passes
 *  its own `ChatMessage[]` which satisfies this shape (we read
 *  `role`, `toolCalls`, `toolResults` only). Keeping the interface
 *  local avoids cross-store type coupling and lets the vitest
 *  build minimal fixtures. */
export interface ChecklistRehydrateMessage {
  role: "user" | "assistant";
  /** Assistant-side tool_use blocks. Each entry's `input` carries
   *  the model's raw `items` array; we filter by `name ===
   *  "update_checklist"` to find candidates. The rehydrate step
   *  in streamController parses the wire-format `tool_use` blocks
   *  into this shape. */
  toolCalls?: ReadonlyArray<{
    id: string;
    name: string;
    input: Record<string, unknown>;
  }>;
  /** UI-grouped tool results. The rehydrate step copies the
   *  following user message's `tool_result` blocks onto the
   *  preceding assistant for the UI's "done / running" lookup;
   *  `isError` is the post-rehydrate flag. We use this to decide
   *  whether a candidate tool_use was "committed" (isError ===
   *  false) or "cancelled / interrupted" (isError === true). */
  toolResults?: ReadonlyArray<{
    toolUseId: string;
    isError: boolean;
  }>;
}

/** Find the last committed `update_checklist` tool_use in a
 *  rehydrated message list and return its coerced items.
 *
 *  "Committed" = the paired tool_result exists AND has
 *  `is_error === false`. A cancelled update gets a synthetic
 *  `is_error: true` tool_result ("Tool execution was interrupted
 *  ..."); rendering its raw input as state would freeze the card
 *  on the moment of interruption. By skipping `is_error === true`
 *  results we lock in "the last update that actually landed".
 *
 *  Returns `null` when no committed `update_checklist` exists.
 *
 *  Exported for the vitest (the store delegates to this on
 *  `rehydrateFromMessages`). */
export function findLastCommittedChecklist(
  messages: ReadonlyArray<ChecklistRehydrateMessage>,
): ChecklistItem[] | null {
  // Collect every assistant tool_use for `update_checklist`, in
  // order. We use the UI-side `toolCalls` array (parsed from the
  // raw `tool_use` blocks by `rehydrateMessages`).
  interface CandidateUse {
    id: string;
    input: Record<string, unknown>;
  }
  const candidates: CandidateUse[] = [];
  for (const m of messages) {
    if (m.role !== "assistant") continue;
    if (!m.toolCalls) continue;
    for (const tc of m.toolCalls) {
      if (tc.name !== CHECKLIST_TOOL_NAME) continue;
      if (typeof tc.id !== "string") continue;
      if (!tc.input || typeof tc.input !== "object") continue;
      candidates.push({ id: tc.id, input: tc.input });
    }
  }
  if (candidates.length === 0) return null;

  // Build a map of tool_use_id → is_error, scanning every message's
  // UI-grouped toolResults. Last write wins; in practice there is
  // exactly one result per id. A tool_use with NO result at all is
  // uncommitted (cancel mid-tool before the synthetic result was
  // spliced in, etc.) and skipped.
  const resultErrorById = new Map<string, boolean>();
  for (const m of messages) {
    if (!m.toolResults) continue;
    for (const tr of m.toolResults) {
      if (typeof tr.toolUseId !== "string") continue;
      resultErrorById.set(tr.toolUseId, !!tr.isError);
    }
  }

  // Walk candidates from LAST to FIRST; the first one with a
  // committed (is_error === false) result wins.
  for (let i = candidates.length - 1; i >= 0; i--) {
    const c = candidates[i];
    const err = resultErrorById.get(c.id);
    if (err === undefined) continue; // no result at all → uncommitted
    if (err) continue; // is_error === true → cancelled / interrupted
    return parseAndCoerceItems(c.input.items);
  }
  return null;
}

export const useChecklistStore = defineStore("checklist", () => {
  /** Per-session current checklist. Keyed by sessionId so switching
   *  sessions shows that session's checklist. An empty array means
   *  "checklist exists but is empty" (the model cleared it); the
   *  key being ABSENT means "no update_checklist seen yet this run"
   *  (the card hides). */
  const checklistBySession = reactive(new Map<string, ChecklistItem[]>());

  /** Read the current checklist for a session. Returns `null` when
   *  no checklist exists for the session (card hides); returns
   *  `[]` when the model has cleared it (card still renders the
   *  empty state). */
  function getChecklist(sessionId: string): ChecklistItem[] | null {
    return checklistBySession.get(sessionId) ?? null;
  }

  /** Live streaming hook: called by `streamController.handleToolCall`
   *  when a `tool:call` for `update_checklist` arrives. Parses the
   *  raw `input.items` and stores the coerced result as the
   *  session's current checklist.
   *
   *  Idempotent — calling twice with the same input produces the
   *  same state (the second call's parse+coerce is deterministic).
   *  We deliberately do NOT wait for the matching `tool:result`
   *  event: the input is the source of truth and the result text
   *  is purely LLM-facing. */
  function handleToolCall(
    sessionId: string,
    toolName: string,
    input: Record<string, unknown> | undefined,
  ): void {
    if (toolName !== CHECKLIST_TOOL_NAME) return;
    const items = parseAndCoerceItems(input?.items);
    // Empty-items means "model cleared the list" — we still write
    // the empty array so the card can transition through the empty
    // state. Only an absent Map key means "no checklist yet".
    checklistBySession.set(sessionId, items);
  }

  /** Reload hook: scan rehydrated messages and reconstruct the
   *  session's checklist from the last committed `update_checklist`.
   *  Called by the chat store / streamController after `ensureLoaded`
   *  / `reloadAfterFinalize` produces a fresh `ChatMessage[]`.
   *
   *  `messages` accepts the production `ChatMessage[]` shape; we
   *  only read `role`, `contentBlocks` (raw), and `toolResults`
   *  (UI-grouped). The production ChatMessage stores raw content
   *  blocks only in the backend's `content` column — the frontend
   *  `ChatMessage` interface doesn't expose `contentBlocks`. To
   *  avoid coupling this scan to that internal shape, callers can
   *  pass EITHER:
   *    - a list of `{role, toolCalls, toolResults}` (the chat store
   *      shape — we use `toolCalls` to find update_checklist uses
   *      and `toolResults` to check the paired result), OR
   *    - a list of `{role, contentBlocks}` (the raw DB shape).
   *  We support both via the loose `ChecklistRehydrateMessage`
   *  interface; the chat store passes its own `ChatMessage[]`
   *  which satisfies the structural subset. */
  function rehydrateFromMessages(
    sessionId: string,
    messages: ReadonlyArray<ChecklistRehydrateMessage>,
  ): void {
    const items = findLastCommittedChecklist(messages);
    if (items === null) {
      // No committed checklist in history → drop any prior live
      // state (e.g. a stale entry from a prior run).
      checklistBySession.delete(sessionId);
    } else {
      checklistBySession.set(sessionId, items);
    }
  }

  /** Per-request reset hook: B12 lifetime is per-run. Called by
   *  the chat store on `send()` / `resendMessage()` before the new
   *  stream starts, mirroring the backend's fresh `Vec` in each
   *  `run_chat_loop` invocation. */
  function clearForNewRun(sessionId: string): void {
    checklistBySession.delete(sessionId);
  }

  /** Explicit drop (e.g. on session delete). Mirrors the
   *  controller's `evict`. */
  function clearSession(sessionId: string): void {
    checklistBySession.delete(sessionId);
  }

  return {
    checklistBySession,
    getChecklist,
    handleToolCall,
    rehydrateFromMessages,
    clearForNewRun,
    clearSession,
  };
});
