// chat.ts — UI-facing chat store.
//
// PR3 of `06-07-6-ui-bug-markdown-sse`: this file is now a thin
// facade over `streamController.ts`. The controller is the single
// source of truth for in-flight streams and per-session message
// buffers (see that file's top-of-file comment for the rationale).
// What remains here is:
//
//   - Type definitions re-exported for the rest of the app
//     (`ChatMessage`, `ErrorCategory`, `ThinkingBlockInfo`, ...).
//   - UI-side session metadata: the sessions list (sidebar
//     summaries), the active session id / cwd / simplified cwd.
//   - The project-change watcher (cascades `loadSessions` and
//     `ensureLoaded` on tab switch).
//   - Session CRUD delegations: `loadSessions`, `createNewSession`,
//     `switchSession`, `deleteSession`.
//   - `send` / `cancel` thin wrappers that build the wire-format
//     history and forward to the controller's request lifecycle.
//   - Reactive projections over controller state: `messages`,
//     `isCurrentSessionStreaming`, `currentRequestId` — the UI
//     only reads these, never the controller's raw state.
//
// External API surface (consumed by components) is unchanged for
// `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd`,
// `send`, `cancel`, `switchSession`, `createNewSession`,
// `loadSessions`, `deleteSession`. The old global `sending` is
// replaced by `isCurrentSessionStreaming` (per-session); callers
// were updated in the same PR.

import { defineStore } from "pinia";
import { computed, reactive, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import { useProjectsStore } from "./projects";
import { useConfigStore } from "./config";
import { useStreamControllerStore } from "./streamController";
import { simplifyPath } from "../utils/path";

type Role = "user" | "assistant";
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

/** A4 (Token Usage Tracking): per-session cumulative token usage.
 *  Mirrors Rust `llm::types::TokenUsage` (snake_case) and the four
 *  `*_total` columns in the `sessions` table. All four fields are
 *  `null` for sessions that have never sent a message (pre-A4
 *  rows, fresh sessions before their first LLM turn). */
export interface SessionTokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
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

/** Chat message with optional tool call/result/thinking metadata. */
export interface ChatMessage {
  id: string;
  role: Role;
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
   *  rows keep NULL until their first post-upgrade turn). The
   *  ChatInput hint reads these to render the
   *  "14.2K · 7% / 200K" line. */
  input_tokens_total: number | null;
  output_tokens_total: number | null;
  cache_creation_total: number | null;
  cache_read_total: number | null;
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
 *  Yolo → Edit (forward). 3 档化 2026-06-13: Review 移除, Yolo
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

/** Wire-format content sent to the Rust `chat` command. Mirrors
 *  Rust's `MessageContent`: a plain string for text-only messages,
 *  or an array of `ContentBlock` (snake_case tag + fields) when
 *  the message carries tool_use / tool_result / thinking /
 *  redacted_thinking blocks. */
type ContentBlockPayload =
  | { type: "text"; text: string }
  | { type: "thinking"; thinking: string; signature: string }
  | { type: "redacted_thinking"; data: string }
  | {
      type: "tool_use";
      id: string;
      name: string;
      input: Record<string, unknown>;
    }
  | {
      type: "tool_result";
      tool_use_id: string;
      content: string;
      is_error: boolean;
    };

interface ChatMessagePayload {
  role: Role;
  content: string | ContentBlockPayload[];
}

const genId = () =>
  Math.random().toString(36).slice(2) + Date.now().toString(36);

/** Concatenate the streamed summary text of all thinking blocks for
 *  display in the UI's thinking section. Newlines separate blocks so
 *  multiple blocks (interleaved thinking) read coherently. */
export function thinkingBlocksToText(blocks: ThinkingBlockInfo[] | undefined): string {
  if (!blocks || blocks.length === 0) return "";
  return blocks.map((b) => b.text).join("\n\n");
}

export const useChatStore = defineStore("chat", () => {
  // -----------------------------------------------------------------------
  // UI-side state (sessions list + active session metadata)
  // -----------------------------------------------------------------------

  const sessions = ref<SessionSummary[]>([]);
  const currentSessionId = ref<string | null>(null);
  const currentCwd = ref<string>("");

  // -----------------------------------------------------------------------
  // A4 (Token Usage Tracking): per-session running totals.
  //
  // The Map is keyed by session id; the value is the cumulative
  // token usage as of the most recent LLM turn Done event. The
  // data flow is:
  //
  //   Anthropic / OpenAI stream ends
  //     → ChatEvent::Done { usage: Some(t) }
  //     → streamController.handleChatEvent("done")
  //     → useChatStore().accumulateTokenUsage(sid, t)
  //     → tokenUsageBySession.get(sid) gets t added in place
  //     → currentSessionTokenUsage computed re-evaluates
  //     → ChatInput.vue re-renders the hint area
  //
  // The Map is also seeded from the `SessionSummary` returned by
  // `list_sessions` / `load_session` so a fresh page reload
  // shows the totals from the DB (the user sees the cumulative
  // value, not "—" + reset). Subsequent per-turn increments are
  // additive on top of the seeded totals.
  //
  // `null` (not `0`) for sessions that have never sent a turn —
  // the ChatInput hint renders this as "—" with the
  // "升级前未统计" tooltip.
  // -----------------------------------------------------------------------
  const tokenUsageBySession = reactive(
    new Map<string, SessionTokenUsage | null>(),
  );

  /** Reactive getter for the current session's running token
   *  totals. `null` when no session is active, or when the
   *  active session has not yet sent its first turn (pre-A4
   *  data or brand-new session). The ChatInput.vue hint area
   *  reads this; the threshold coloring is computed inline in
   *  the component (keeps the store API single-purpose). */
  const currentSessionTokenUsage = computed<SessionTokenUsage | null>(
    () => {
      const sid = currentSessionId.value;
      if (!sid) return null;
      return tokenUsageBySession.get(sid) ?? null;
    },
  );

  /** Add a per-turn usage report to the running session total.
   *  Called by `streamController.handleChatEvent` on every
   *  `done` event that carries a `usage` payload. Add-or-init
   *  semantics: a first call seeds from 0; subsequent calls
   *  add field-wise. */
  function accumulateTokenUsage(
    sessionId: string,
    usage: SessionTokenUsage,
  ): void {
    const existing = tokenUsageBySession.get(sessionId);
    if (!existing) {
      tokenUsageBySession.set(sessionId, { ...usage });
    } else {
      existing.input_tokens += usage.input_tokens;
      existing.output_tokens += usage.output_tokens;
      existing.cache_creation_input_tokens +=
        usage.cache_creation_input_tokens;
      existing.cache_read_input_tokens += usage.cache_read_input_tokens;
    }
  }

  // -----------------------------------------------------------------------
  // F5 (LLM Latency Tracking): per-session cumulative latency.
  //
  // The Map is keyed by session id; the value is the running
  // total of `total_ms` across all assistant turns in the
  // session, displayed in the ChatPanel footer ("本次 session
  // LLM 累计耗时"). The data flow mirrors A4's token usage:
  //
  //   streamController.handleChatEvent("done")
  //     → compute { ttfbMs, genMs, totalMs } for the assistant turn
  //     → update_message_latency IPC to persist per-message columns
  //     → sessionTotalLatencyMs map += totalMs (cumulative)
  //
  // The Map is also seeded from `load_session` so a fresh page
  // reload shows the cumulative value (not "—"). The seed
  // sums `Σ total_ms WHERE role = 'assistant' AND total_ms IS
  // NOT NULL` — the controller does the sum during rehydrate
  // and hands the value to `accumulateLatency` via
  // `add-latency` (the per-message increments then stack on
  // top).
  //
  // The sessionTotalLatencyMs is also exposed as a
  // `currentSessionLatencyTotal` computed (mirroring
  // `currentSessionTokenUsage`) for the ChatPanel footer to
  // read.
  // -----------------------------------------------------------------------
  const sessionTotalLatencyMs = reactive(
    new Map<string, number>(),
  );

  /** Reactive getter for the current session's running latency
   *  total. `null` when no session is active OR when the
   *  active session has not yet recorded a `total_ms` value
   *  (pre-F5 data or brand-new session). The ChatPanel footer
   *  reads this; "—" is rendered for `null`. */
  const currentSessionLatencyTotal = computed<number | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    return sessionTotalLatencyMs.get(sid) ?? null;
  });

  /** Add a per-turn latency report to the running session
   *  total. Called by `streamController.handleChatEvent` on
   *  every `done` event that resolved a `totalMs`. A first
   *  call seeds the map (overwriting any prior seed value
   *  from rehydrate). Subsequent calls add. The caller is
   *  responsible for NOT firing this on cancel / error paths
   *  that have no `totalMs`. */
  function accumulateLatency(sessionId: string, totalMs: number): void {
    const existing = sessionTotalLatencyMs.get(sessionId);
    if (existing === undefined) {
      sessionTotalLatencyMs.set(sessionId, totalMs);
    } else {
      sessionTotalLatencyMs.set(sessionId, existing + totalMs);
    }
  }

  /** F5 follow-up: per-turn latency list for the active
   *  session, in chronological order (oldest first). The
   *  ChatInput popover renders this as a row-by-row breakdown
   *  (TTFB / Gen / Total per turn). Derived purely from the
   *  controller's in-memory messages — no separate Map needed,
   *  because the streaming `done` / `error` handler writes
   *  `latency` onto the assistant message in place, and
   *  rehydrated rows carry the values from `messages.total_ms`
   *  via `rehydrateMessages`. Returns `null` when no session
   *  is active; empty array when the session has messages
   *  but none of them recorded a latency (pre-F5 data, or a
   *  fresh session before its first turn). The render layer
   *  distinguishes "no session" (`null` → "—") from "no
   *  latency yet" (`[]` → "0.0s · 0 turns" / similar) so the
   *  user gets a stable label across the three states. */
  const currentSessionLatencyTurns = computed<LatencyInfo[] | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    const msgs = controller.getMessages(sid);
    if (!msgs) return [];
    const out: LatencyInfo[] = [];
    for (const m of msgs) {
      if (m.role !== "assistant") continue;
      if (!m.latency) continue;
      out.push(m.latency);
    }
    return out;
  });

  // -----------------------------------------------------------------------
  // Stream controller — single source of truth for messages + active
  // requests. Owned by a separate Pinia store; this file only projects
  // the controller's state into the shape the components expect.
  // -----------------------------------------------------------------------
  const controller = useStreamControllerStore();

  // F2: when true, auto-scroll follows every delta regardless of
  // user position. Set on send(), cleared on stream-done or when
  // the user manually scrolls up.
  const forceFollowActive = ref(false);

  // F4: true while switchSession is loading messages (IPC pending).
  const sessionLoading = ref(false);

  // F4: incremented after reloadAfterFinalize replaces messages, so
  // MessageList can re-scroll to bottom. The value is a counter, not a
  // boolean, to guarantee Vue detects the change.
  const scrollAfterReload = ref(0);

  // -----------------------------------------------------------------------
  // Reactive projections over the controller's state. Components read
  // these and never touch the controller directly.
  // -----------------------------------------------------------------------

  /** Messages for the currently active session. Touches the
   *  controller's LRU on every read so the active session stays MRU
   *  (and therefore won't be evicted mid-view). Returns `[]` when
   *  no session is active. The LRU side effect is the intended
   *  behavior — see `streamController.getMessages`. */
  const messages = computed<ChatMessage[]>(() => {
    const sid = currentSessionId.value;
    if (!sid) return [];
    return controller.getMessages(sid) ?? [];
  });

  /** True if the CURRENT session has an in-flight stream.
   *  Per-session independence (PR3 / bug 6): a stream in session A
   *  does not make this true while the user is looking at session
   *  B. Use the controller's `streamingSessionIds` directly for
   *  the full picture (e.g. session card streaming indicators in
   *  PR4).
   *
   *  Note: Pinia auto-unwraps refs/computeds when you read them
   *  off a store proxy, so `controller.streamingSessionIds` is
   *  the `Set<string>` itself (no `.value`). The reactive Set
   *  triggers our computed to re-run when the controller's
   *  `activeRequests` map changes. */
  const isCurrentSessionStreaming = computed<boolean>(() => {
    const sid = currentSessionId.value;
    if (!sid) return false;
    return controller.streamingSessionIds.has(sid);
  });

  /** The request id of the current session's active stream, or
   *  `null` if it isn't streaming. Replaces the old chat-store
   *  `currentRequestId` writable ref — the controller owns the
   *  actual request state, this is just a per-session lookup. */
  const currentRequestId = computed<string | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    return controller.currentRequestId(sid);
  });

  // PR3 (BACKLOG §5.1): the chat panel header displays the cwd with
  // the user's home prefix shortened to `~`. The computed is reactive
  // so when the home-dir cache finishes loading after the chat store
  // is first read, the UI re-renders without extra wiring. The
  // `configStore` reference is captured lazily — the computed body
  // only runs on first `.value` access, by which time the line
  // below has been initialized.
  const simplifiedCwd = computed<string>(() =>
    simplifyPath(currentCwd.value, configStore.homeDir),
  );

  // -----------------------------------------------------------------------
  // Cross-store coordination: react to project changes
  // -----------------------------------------------------------------------

  const projectsStore = useProjectsStore();
  const configStore = useConfigStore();

  watch(
    () => projectsStore.currentProjectId,
    async (newId) => {
      // Persist last-active project to localStorage. The config
      // store's own watcher writes to localStorage; we just update
      // its ref. Done here (not in the projects store) so the
      // persistence lives next to the read path (config.load) for
      // cohesion.
      configStore.lastActiveProjectId = newId;
      await onProjectChange(newId);
    },
    { immediate: true },
  );

  // PR3 self-check fix: the old `done` handler in chat.ts ran
  // `loadSessions(currentProjectId)` after each turn so the sidebar
  // would reflect the new `updated_at` / auto-generated title. With
  // the listener owned by the controller, that side effect moved
  // out of the event handler — but we still need it. Watch the
  // controller's `activeRequests.size` for any shrink (a request
  // ended via done or error) and refresh sessions for the project
  // the user is currently viewing. Cross-project case (stream
  // finishes in project A while user views B) is naturally covered
  // by `onProjectChange` reloading on next switch.
  watch(
    () => controller.activeRequests.size,
    (newSize, oldSize) => {
      if (newSize < oldSize && projectsStore.currentProjectId) {
        void loadSessions(projectsStore.currentProjectId);
      }
    },
  );

  async function onProjectChange(newId: string | null): Promise<void> {
    if (newId === null) {
      sessions.value = [];
      currentSessionId.value = null;
      currentCwd.value = "";
      return;
    }
    await loadSessions(newId);
    // A4: seed the per-session token usage map from the
    // SessionSummary so the ChatInput hint area renders
    // the right number on a fresh page reload (without this,
    // the user would see "—" until they sent another turn).
    for (const s of sessions.value) {
      if (
        s.input_tokens_total !== null &&
        s.output_tokens_total !== null
      ) {
        tokenUsageBySession.set(s.id, {
          input_tokens: s.input_tokens_total,
          output_tokens: s.output_tokens_total,
          cache_creation_input_tokens: s.cache_creation_total ?? 0,
          cache_read_input_tokens: s.cache_read_total ?? 0,
        });
      }
    }
    // Default to the most-recently-updated session if any exist;
    // otherwise leave the chat area in its empty state.
    if (sessions.value.length > 0) {
      // F1: prefer per-project last active session over sessions[0].
      const lastId = configStore.readLastSession(newId);
      const target =
        lastId && sessions.value.some((s) => s.id === lastId)
          ? sessions.value.find((s) => s.id === lastId)!
          : sessions.value[0];
      currentSessionId.value = target.id;
      currentCwd.value = target.current_cwd ?? "";
      // F1: persist the selected session as last active for this project.
      configStore.writeLastSession(newId, target.id);
      // Seed the controller's cache for the new active session so
      // the `messages` computed and the controller's per-session
      // event routing have something to look at on first render.
      await controller.ensureLoaded(target.id);
    } else {
      currentSessionId.value = null;
      currentCwd.value = "";
    }
  }

  // -----------------------------------------------------------------------
  // Session management
  // -----------------------------------------------------------------------

  async function loadSessions(projectId: string | null): Promise<void> {
    if (!projectId) {
      sessions.value = [];
      return;
    }
    sessions.value = await invoke<SessionSummary[]>("list_sessions", {
      projectId: projectId,
    });
  }

  /** Create a new session under the current project. Throws if no
   *  project is active — the caller (the chat area) is expected to
   *  be visible only when a project is selected (Q2 in dispatch
   *  prompt: the empty state hides the input, so send/create is
   *  unreachable from the UI). */
  async function createNewSession(): Promise<string> {
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("createNewSession: no current project");
    }
    const project = projectsStore.projectById(projectId);
    const initialCwd = project?.path ?? "";
    const session = await invoke<{
      id: string;
      title: string;
      created_at: string;
      updated_at: string;
      model: string;
      project_id: string;
      current_cwd: string;
    }>("create_session", {
      projectId: projectId,
      initialCwd: initialCwd,
    });
    currentSessionId.value = session.id;
    currentCwd.value = session.current_cwd ?? "";
    // Seed the controller's cache with an empty buffer for the new
    // session. `ensureLoaded` will do an IPC `load_session` call
    // (returning an empty message list for a fresh session) — the
    // only public way to put a value into the controller's LRU.
    await controller.ensureLoaded(session.id);
    await loadSessions(projectId);
    return session.id;
  }

  async function switchSession(sessionId: string) {
    // Per-session independence (PR3 / bug 6 fix): switching
    // sessions mid-stream is now a first-class operation. The
    // in-flight request keeps running on the backend; the
    // controller's listener routes events to the matching
    // `request_id` regardless of the user's current view. When
    // the user returns to the streaming session, the
    // `messages` computed re-evaluates and the in-flight
    // message is right there — no DB reload, no `done`-event
    // loss.
    //
    // F4: set loading state for spinner display. Cleared after
    // ensureLoaded completes.
    sessionLoading.value = true;
    try {
      await controller.ensureLoaded(sessionId);
      currentSessionId.value = sessionId;
      // F1: persist per-project last active session.
      if (projectsStore.currentProjectId) {
        configStore.writeLastSession(
          projectsStore.currentProjectId,
          sessionId,
        );
      }
      // Pull cwd from the session summary (the controller doesn't
      // expose session metadata; `list_sessions` already has the
    // value in memory). Avoids a redundant `load_session` IPC.
      const summary = sessions.value.find((s) => s.id === sessionId);
      currentCwd.value = summary?.current_cwd ?? "";
    } finally {
      sessionLoading.value = false;
    }
  }

  async function deleteSession(sessionId: string) {
    await invoke("delete_session", { sessionId });
    // Evict from the controller's cache (and unpin, just in case)
    // so the in-memory buffer doesn't keep a stale entry alive
    // past the DB row's deletion.
    controller.evict(sessionId);
    // Drop any cached diff for this session — the worktree it
    // referenced is now gone, so the diff is meaningless.
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      currentSessionId.value = null;
      currentCwd.value = "";
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  // D1: rename + color tag
  async function renameSession(sessionId: string, newTitle: string) {
    await invoke("rename_session", { sessionId, newTitle });
    const s = sessions.value.find((x) => x.id === sessionId);
    if (s) s.title = newTitle.slice(0, 80);
  }

  async function setSessionColor(sessionId: string, colorTag: number | null) {
    await invoke("set_session_color", { sessionId, colorTag: colorTag });
    const s = sessions.value.find((x) => x.id === sessionId);
    if (s) s.color_tag = colorTag;
  }

  // -----------------------------------------------------------------------
  // Step 4 follow-up: opt-in worktree actions
  //
  // Three Tauri commands, three Pinia actions. Each one (a) calls
  // the backend, (b) invalidates the local diff cache for the
  // session (the on-disk state has changed), and (c) refreshes the
  // sessions list so the sidebar chip updates. Errors are surfaced
  // via `projectsStore.showToast` so the user sees a single
  // consistent error path.
  // -----------------------------------------------------------------------

  async function attachWorktree(sessionId: string): Promise<void> {
    try {
      await invoke("attach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`attach worktree 失败: ${String(e)}`, "error");
      throw e;
    }
    // Invalidate cached diff (the on-disk worktree is now
    // different from the session baseline) and refresh the list.
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      // Re-load messages from the DB so the system event the
      // backend just inserted (REQ-17) is in the cache. The
      // next `send()` builds history from the cache; without
      // this refresh the LLM would not see the worktree
      // transition event.
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  async function detachWorktree(sessionId: string): Promise<void> {
    try {
      await invoke("detach_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`detach worktree 失败: ${String(e)}`, "error");
      throw e;
    }
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      // Re-fetch the session metadata + messages so currentCwd,
      // the session's new state, and the system event the
      // backend just injected are all visible immediately. Use
      // `refresh` (not `ensureLoaded`) so the cache picks up
      // the new system event row.
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  async function deleteWorktree(sessionId: string): Promise<void> {
    try {
      await invoke("delete_worktree", { sessionId });
    } catch (e) {
      projectsStore.showToast(`delete worktree 失败: ${String(e)}`, "error");
      throw e;
    }
    diffCache.value.delete(sessionId);
    if (currentSessionId.value === sessionId) {
      await controller.refresh(sessionId);
    }
    if (projectsStore.currentProjectId) {
      await loadSessions(projectsStore.currentProjectId);
    }
  }

  // -----------------------------------------------------------------------
  // Diff (step 4 / PR3) — fetch and cache the session's worktree
  // diff. The IPC call is read-only and cheap (libgit2 walks the
  // tree, no remote I/O), but we still cache to avoid recomputing
  // for repeated clicks on the same session. The cache is keyed by
  // session id and is invalidated on session switch (so a stale
  // "diff from a different session" can't bleed through) and on
  // session delete.
  // -----------------------------------------------------------------------

  const diffCache = ref<Map<string, DiffResult>>(new Map());

  /** Reactive getter: cached diff for a session, or `null` if
   *  not yet fetched. Vue consumers should call `fetchDiff`
   *  first; this is just the read-side of the cache. */
  function getDiff(sessionId: string): DiffResult | null {
    return diffCache.value.get(sessionId) ?? null;
  }

  /** Fetch the session's worktree diff. Cached after the first
   *  call until the session is deleted. Errors propagate to the
   *  caller (the UI surfaces them in the popover). */
  async function fetchDiff(sessionId: string): Promise<DiffResult> {
    const cached = diffCache.value.get(sessionId);
    if (cached) {
      return cached;
    }
    const result = await invoke<DiffResult>("diff_worktree", { sessionId });
    diffCache.value.set(sessionId, result);
    // Force reactivity for the new Map reference (Pinia tracks
    // Map.set on the proxy but consumers reading `.get` want a
    // fresh snapshot).
    diffCache.value = new Map(diffCache.value);
    return result;
  }

  /** BUG FIX (06-08-06-08 step-4 follow-up — 2013 wire invariant):
   *  drop a single session's entry from the diff cache so the next
   *  reader (the worktree chip in `ChatPanel.vue` or a
   *  `diffWorktree` modal open) takes the cache-miss path and
   *  re-invokes the backend `diff_worktree` IPC. Called from
   *  `streamController.finalizeRequest` right after a `chat`
   *  request ends, so the worktree chip reflects post-send state
   *  (e.g. a `git commit` run inside the worktree drops the
   *  "diff (N)" counter immediately) instead of staying on the
   *  pre-send snapshot. The map replacement (`new Map(...)`) is
   *  the same reactivity trick `fetchDiff` uses — Vue tracks
   *  Map.set on the proxy but downstream `computed` consumers
   *  want a fresh reference. No-op if the session isn't cached.
   *
   *  Note: this does NOT touch `loadedFromDb` or the in-memory
   *  message buffer — that's `streamController.evict`, called in
   *  the same `finalizeRequest` so the two stay paired. */
  function invalidateDiff(sessionId: string): void {
    if (diffCache.value.has(sessionId)) {
      diffCache.value.delete(sessionId);
      diffCache.value = new Map(diffCache.value);
    }
  }

  /** Filter a session's diff down to a single file path. Returns
   *  `null` if the file isn't in the diff (either not changed in
   *  this session, OR the session diff hasn't been fetched yet). */
  function getFileDiff(sessionId: string, filePath: string): FileDiff | null {
    const result = diffCache.value.get(sessionId);
    if (!result) return null;
    return result.files.find((f) => f.path === filePath) ?? null;
  }

  // -----------------------------------------------------------------------
  // Send / Cancel
  // -----------------------------------------------------------------------

  /** Build the wire-format content for a history message: plain string
   *  for text-only / thinking-only messages, or an array of blocks when
   *  the turn carries tool_use / tool_result data. Backend's
   *  `MessageContent` deserializer accepts both shapes.
   *
   *  CRITICAL: thinking blocks (incl. signatures) and redacted_thinking
   *  data are emitted verbatim in their original streaming order. The
   *  Anthropic API requires the exact signature blob on the next turn —
   *  omitting or rewriting it produces 400. */
  function toPayloadContent(m: ChatMessage): string | ContentBlockPayload[] {
    // CRITICAL: tool_result blocks belong ONLY on user-role messages
    // (Anthropic Messages API contract). `rehydrateMessages` (in the
    // controller) attaches the following user message's tool_results
    // onto the assistant message *for UI grouping* (per-message "done /
    // running" lookup); here we MUST NOT echo them onto the wire when
    // role=assistant or Anthropic returns 2013 ("tool result's tool id
    // ... not found") because the assistant message itself isn't
    // allowed to contain tool_result blocks. Same for `content` text
    // emitted onto a ghost user message: only the assistant's text
    // counts.
    if (m.role === "assistant") {
      const hasTools = !!m.toolCalls?.length;
      const hasThinking =
        !!m.thinkingBlocks?.length || !!m.redactedThinkingData?.length;
      if (!hasTools && !hasThinking) {
        return m.content;
      }
      const blocks: ContentBlockPayload[] = [];
      // Thinking blocks come first (Anthropic convention: reasoning
      // before any visible text in the same turn).
      for (const tb of m.thinkingBlocks ?? []) {
        blocks.push({
          type: "thinking",
          thinking: tb.text,
          signature: tb.signature,
        });
      }
      if (m.content) {
        blocks.push({ type: "text", text: m.content });
      }
      for (const tc of m.toolCalls ?? []) {
        blocks.push({
          type: "tool_use",
          id: tc.id,
          name: tc.name,
          input: tc.input,
        });
      }
      for (const data of m.redactedThinkingData ?? []) {
        blocks.push({ type: "redacted_thinking", data });
      }
      // Intentionally omit `m.toolResults` — they're for the UI, not
      // the wire. The matching user-role message in the array
      // carries the canonical tool_result blocks.
      return blocks;
    }

    // user role: emit tool_result blocks + any text/thinking/redacted.
    // The rehydrated user message (formerly tool_result-only "ghost")
    // and the live user-typed message both pass through here.
    const hasTools = !!m.toolResults?.length;
    const hasThinking =
      !!m.thinkingBlocks?.length || !!m.redactedThinkingData?.length;
    if (!hasTools && !hasThinking) {
      return m.content;
    }
    const blocks: ContentBlockPayload[] = [];
    for (const tb of m.thinkingBlocks ?? []) {
      blocks.push({
        type: "thinking",
        thinking: tb.text,
        signature: tb.signature,
      });
    }
    if (m.content) {
      blocks.push({ type: "text", text: m.content });
    }
    for (const tr of m.toolResults ?? []) {
      blocks.push({
        type: "tool_result",
        tool_use_id: tr.toolUseId,
        content: tr.content,
        is_error: tr.isError,
      });
    }
    for (const data of m.redactedThinkingData ?? []) {
      blocks.push({ type: "redacted_thinking", data });
    }
    return blocks;
  }

  async function send(text: string) {
    const trimmed = text.trim();
    // Bug 6 fix (PR3): the old guard was a single global `sending`
    // ref. The new guard is per-session: the user can have multiple
    // sessions streaming concurrently, but they can't fire a second
    // message into the SAME session while it's still streaming.
    if (!trimmed || isCurrentSessionStreaming.value) return;
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("send: no current project");
    }

    // Lazily create a session if there isn't one yet. `createNewSession`
    // throws if no project is active, so the chat area is expected
    // to be visible only when a project is selected (Q2 in dispatch
    // prompt: the empty state hides the input, so send/create is
    // unreachable from the UI).
    if (!currentSessionId.value) {
      await createNewSession();
    }
    // After createNewSession, `currentSessionId` is set; we
    // re-read in case the project's `last_cwd` is different from
    // the previous session's, etc.
    const sessionId = currentSessionId.value!;

    // Make sure the controller's cache has an entry for this
    // session (in case the user hits send immediately after a
    // project switch before `ensureLoaded` has run, or after a
    // long-idle eviction). `ensureLoaded` is a no-op for cached
    // sessions and an IPC call for evicted ones.
    const msgs = await controller.ensureLoaded(sessionId);

    // F2: activate force-follow mode so the chat stays scrolled to
    // bottom for the entire duration of the stream.
    forceFollowActive.value = true;

    const userMsg: ChatMessage = {
      id: genId(),
      role: "user",
      content: trimmed,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
      role: "assistant",
      content: "",
    };
    // The controller's event handlers look up `last` on this
    // array, so the assistant placeholder MUST be the final
    // entry before the stream starts. Pushing in this order also
    // matches the order the UI renders (user message first,
    // assistant placeholder right after).
    msgs.push(userMsg, assistantMsg);

    // Build history — keep tool_use / tool_result / thinking /
    // redacted_thinking blocks intact so the LLM has full context
    // across turns and across session switches. The agent loop
    // also constructs a matching assistant message from the
    // streaming events and persists it before the next LLM call,
    // so the history we send here will line up with what's in the
    // DB.
    const history: ChatMessagePayload[] = msgs
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

    // `startRequest` registers the active request, pins the session
    // in the LRU, and invokes the backend `chat` IPC. The
    // controller owns the listener, the request state, the
    // message routing, and the cleanup on `done` / `error` /
    // cancel. This call returns once the IPC completes (the
    // backend stream continues independently; events route back
    // via the global listener).
    await controller.startRequest({
      sessionId,
      projectId,
      userMsg,
      assistantMsg,
      history,
    });
  }

  /** PR5: cancel an in-flight chat request. The backend's agent
   *  loop notices on the next event boundary, bails out, persists
   *  whatever it has, and emits a `done` event with
   *  `stop_reason: "cancelled"`. That `done` flows through the
   *  controller's `handleChatEvent` → `finalizeRequest`, which
   *  clears the active request and unpins the session — so this
   *  call is fire-and-forget IPC; the actual state reset happens
   *  via the `done` event. */
  async function cancel() {
    const rid = currentRequestId.value;
    if (!rid) return;
    await controller.cancel(rid);
  }

  // -----------------------------------------------------------------------
  // A2 + B7 (PR2 front-end): per-session Mode changes via the
  // `set_session_mode` Tauri command. Both the popover entry
  // (`ModeSelect.vue`) and the keyboard entry (`Shift+Tab` in
  // `ChatInput.vue` via `useKeyboard`) call this so the Yolo
  // confirm modal flow can live in exactly one place. The
  // component-side handlers (`ModeSelect.onModePick`,
  // `ChatInput.cycleMode`) just route here.
  //
  // We deliberately do NOT ship the Yolo confirm modal as a
  // store-managed thing — the modal is visual chrome and a
  // store shouldn't own a `<Teleport>` target. Instead, the
  // store exposes:
  //   - `pendingYoloConfirm`: a reactive boolean the modal
  //     mounts against (`v-if`).
  //   - `requestSetMode(sessionId, mode)`: the orchestrator
  //     that flips the Yolo gate for non-Chat modes and
  //     short-circuits when the gate is already open.
  //   - `confirmYolo()` / `cancelYolo()`: confirm / cancel the
  //     pending modal (the modal calls these on its buttons).
  //
  // `ModeSelect` reads `pendingYoloConfirm` to render the modal
  // (it owns the modal mount today; the store only holds the
  // boolean). `ChatInput`'s `cycleMode` calls `requestSetMode`
  // — the Yolo transition will surface in `ModeSelect`'s
  // mounted modal because both UIs share the same store state.
  // -----------------------------------------------------------------------

  /** True while the Yolo confirm modal should be mounted. Both
   *  UI entry points (`ModeSelect` popover + `ChatInput`
   *  Shift+Tab) flip this through `requestSetMode`. The modal
   *  is unmounted via `v-if` when this flips false. */
  const pendingYoloConfirm = ref(false);

  /** Orchestrator for a mode change. The caller passes the
   *  target mode; this method handles the Yolo gate. Returns
   *  `true` if the mode was applied (or already current),
   *  `false` if the call was deferred to the modal. Errors
   *  propagate to the caller via the `invoke` throw. */
  async function requestSetMode(
    sessionId: string,
    mode: SessionMode,
  ): Promise<boolean> {
    if (!sessionId) return false;
    if (isCurrentSessionStreaming.value) return false;

    // No-op when the mode is already current. The optimistic
    // local update below is also a no-op, but we skip the IPC
    // round-trip to keep Shift+Tab snappy.
    const summary = sessions.value.find((s) => s.id === sessionId);
    if (summary && summary.mode === mode) return true;

    // Yolo always requires the confirm ceremony. We stage the
    // modal mount and let `confirmYolo` fire the IPC.
    if (mode === "yolo") {
      pendingYoloConfirm.value = true;
      return false;
    }

    // Non-Yolo mode: apply directly.
    try {
      await invoke("set_session_mode", { sessionId, mode });
      if (summary) {
        (summary as { mode: string }).mode = mode;
      }
      return true;
    } catch (e) {
      console.error("Failed to update session mode:", e);
      return false;
    }
  }

  /** Called by `YoloConfirmModal`'s confirm button. Fires the
   *  pending IPC, optimistic-updates the session row, and
   *  closes the modal. No-op when no session is active or
   *  streaming kicked in while the modal was open. */
  async function confirmYolo(): Promise<void> {
    pendingYoloConfirm.value = false;
    const sid = currentSessionId.value;
    if (!sid) return;
    if (isCurrentSessionStreaming.value) return;
    try {
      await invoke("set_session_mode", { sessionId: sid, mode: "yolo" });
      const summary = sessions.value.find((s) => s.id === sid);
      if (summary) {
        (summary as { mode: string }).mode = "yolo";
      }
    } catch (e) {
      console.error("Failed to confirm Yolo:", e);
    }
  }

  /** Cancel the pending Yolo confirm — no mode change. */
  function cancelYolo(): void {
    pendingYoloConfirm.value = false;
  }

  return {
    // Reactive state (computed projections)
    messages,
    isCurrentSessionStreaming,
    currentRequestId,
    // A4: per-session running token totals. The ChatInput
    // hint area reads `currentSessionTokenUsage`; the Map is
    // exposed for tests / future per-session UIs.
    currentSessionTokenUsage,
    tokenUsageBySession,
    // F5: per-session running latency total. The ChatPanel
    // footer reads `currentSessionLatencyTotal`; the Map is
    // exposed for tests.
    currentSessionLatencyTotal,
    sessionTotalLatencyMs,
    // F5 follow-up: per-turn latency list for the popover
    // breakdown. Derived from the controller's in-memory
    // messages (no separate Map — see the computed's doc
    // comment for the rationale). `null` when no session
    // is active; `[]` when the active session has no
    // latency data yet.
    currentSessionLatencyTurns,
    // UI-side state (refs)
    sessions,
    currentSessionId,
    currentCwd,
    simplifiedCwd,
    diffCache,
    // F2/F4: scroll follow mode + session loading
    forceFollowActive,
    sessionLoading,
    scrollAfterReload,
    // Methods
    send,
    cancel,
    loadSessions,
    createNewSession,
    switchSession,
    deleteSession,
    renameSession,
    setSessionColor,
    attachWorktree,
    detachWorktree,
    deleteWorktree,
    fetchDiff,
    getDiff,
    getFileDiff,
    invalidateDiff,
    // A4: hook called by streamController.handleChatEvent on
    // every `done` event that carries a usage payload.
    accumulateTokenUsage,
    // F5: hook called by streamController.handleChatEvent on
    // every `done` event that resolved a `totalMs`. Adds the
    // per-turn `totalMs` to the running session total.
    accumulateLatency,
    // A2 + B7 (PR2): per-session Mode setters. The Yolo gate
    // is held in `pendingYoloConfirm` and consumed by the
    // YoloConfirmModal mounted by `ModeSelect.vue`.
    pendingYoloConfirm,
    requestSetMode,
    confirmYolo,
    cancelYolo,
  };
});
