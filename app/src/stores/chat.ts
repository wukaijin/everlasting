// chat.ts — UI-facing chat store.
//
// PR3 of `06-07-6-ui-bug-markdown-sse`: this file is now a thin
// facade over `streamController.ts`. The controller is the single
// source of truth for in-flight streams and per-session message
// buffers (see that file's top-of-file comment for the rationale).
// What remains here is:
//
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
//   - The `useChatStore` factory itself.
//
// Public type/interface declarations (`ChatMessage`,
// `ErrorCategory`, `ThinkingBlockInfo`, `SessionSummary`,
// `SessionMode`, `FileDiff`, etc.) live in `./chat.types` —
// this file imports them back. Splitting the public contract
// into its own module keeps `chat.ts` focused on the store
// body (see PRD 06-23-06-23-split-chat-types).
//
// External API surface (consumed by components) is unchanged for
// `sessions`, `currentSessionId`, `currentCwd`, `simplifiedCwd`,
// `send`, `cancel`, `switchSession`, `createNewSession`,
// `loadSessions`, `deleteSession`. The old global `sending` is
// replaced by `isCurrentSessionStreaming` (per-session); callers
// were updated in the same PR.

import { defineStore } from "pinia";
import { computed, reactive, ref, watch } from "vue";
import { transport } from "../transport";

import { useProjectsStore } from "./projects";
import { useConfigStore } from "./config";
import { useStreamControllerStore } from "./streamController";
import { type ModelWithProvider } from "./models";
import { createSessionActions } from "./chatSessionActions";
import { createModeActions } from "./chatModeActions";
import { createMessageActions } from "./chatMessageActions";
import { createSendActions } from "./chatSendActions";
import { simplifyPath } from "../utils/path";
import {
  type ChatMessage,
  type DiffResult,
  type FileDiff,
  type LatencyInfo,
  type ParticipantConfig,
  type SessionSummary,
  type SessionTokenUsage,
  type ThinkingBlockInfo,
} from "./chat.types";

type Role = "user" | "assistant";

/**
 * B6+ B (task 07-06-b6plus-b-dispatch-model-arg): resolve a
 * `--model=<X>` flag value (from `@@agent --model=<X> <task>`) to a
 * model id. `<X>` may be a model id (passthrough) or a display_name
 * (reverse-lookup via the loaded models list). Exported for unit
 * testing (pure over the `models` arg).
 *
 * - Exact id match first.
 * - Miss → display_name match (first wins; display_name should be
 *   unique but the DB does not enforce it). On duplicate, logs a
 *   `console.warn` so the ambiguity is visible in devtools.
 * - Not found → `undefined` (the caller omits `modelId`, so the
 *   dispatch falls back to the agent's configured default). No
 *   thrown error — the raw `--model=` text stays in the input so the
 *   user can correct it.
 */
export function resolveModelInput(
  raw: string,
  models: ModelWithProvider[],
): string | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;
  // ① exact id match.
  const byId = models.find((m) => m.id === trimmed);
  if (byId) return byId.id;
  // ② display_name reverse lookup (first match wins).
  const matches = models.filter((m) => m.displayName === trimmed);
  if (matches.length === 1) return matches[0].id;
  if (matches.length > 1) {
    console.warn(
      `[resolveModelInput] multiple models share display_name "${trimmed}"; using the first match (${matches[0].id})`,
    );
    return matches[0].id;
  }
  // ③ not found.
  console.warn(
    `[resolveModelInput] no model matches id or display_name "${trimmed}"; ignoring --model override`,
  );
  return undefined;
}

/** Shape of a forced-dispatch payload threaded through the `chat`
 *  IPC. Field names are snake_case to match the backend
 *  `ForcedDispatch` struct (nested IPC struct fields pass through
 *  serde verbatim — no Tauri arg auto-camel). */
export interface ForcedDispatchPayload {
  subagent: string;
  task: string;
  model_id?: string;
}

/** B6+ B: parse a `@@<agent> [--model=<X>] <task>` prefix from the
 *  trimmed input text. Pure over `(trimmed, models)` so it is
 *  unit-testable without the pinia store.
 *
 *  Returns:
 *  - `{ forcedDispatch: {...}, body }` when a valid `@@` prefix is
 *    present and the task is non-empty.
 *  - `null` when a `@@` prefix is present but the task is empty
 *    (the caller should abort the send — no dispatch without a
 *    brief).
 *  - `{ forcedDispatch: undefined, body: trimmed }` when NO `@@`
 *    prefix is present (a normal message).
 *
 *  The `--model=<X>` flag is optional and must sit BETWEEN the agent
 *  name and the task (git/cargo flag semantics); a `--model=` token
 *  elsewhere in the task body is NOT extracted. An unresolved `<X>`
 *  yields no `model_id` (the dispatch falls back to the agent's
 *  configured default); the raw `--model=` text stays in the input.
 */
export function parseForcedDispatchPrefix(
  trimmed: string,
  models: ModelWithProvider[],
):
  | { forcedDispatch: ForcedDispatchPayload; body: string }
  | { forcedDispatch: undefined; body: string }
  | null {
  const atAt = trimmed.match(
    /^@@([A-Za-z0-9_-]+)[ \t]+(?:--model=(\S+)[ \t]+)?([\s\S]+)$/,
  );
  if (!atAt) {
    return { forcedDispatch: undefined, body: trimmed };
  }
  const task = atAt[3].trim();
  if (!task) return null;
  const rawModel = atAt[2]; // undefined when no --model= flag
  const modelId = rawModel ? resolveModelInput(rawModel, models) : undefined;
  const payload: ForcedDispatchPayload = {
    subagent: atAt[1],
    task,
    ...(modelId ? { model_id: modelId } : {}),
  };
  return { forcedDispatch: payload, body: task };
}

/** Wire-format content sent to the Rust `chat` command. Mirrors
 *  Rust's `MessageContent`: a plain string for text-only messages,
 *  or an array of `ContentBlock` (snake_case tag + fields) when
 *  the message carries tool_use / tool_result / thinking /
 *  redacted_thinking blocks. */
export type ContentBlockPayload =
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

export interface ChatMessagePayload {
  role: Role;
  content: string | ContentBlockPayload[];
  /** B1 (2026-08-16) image-multimodal: image refs riding this user
   *  message. The backend rebuilds `ContentBlock::ImageRef` blocks
   *  from them each request (DB rows stay text-only; the metadata
   *  manifest is the persistent source of truth). Omitted for
   *  messages without attachments. See `AttachmentWireRef`. */
  attachments?: AttachmentWireRef[];
}

/** B1: one image attachment in the `chat` IPC's nested
 *  `messages[].attachments` array. Mirrors the Rust
 *  `llm::types::chat::AttachmentRef` serde shape — the struct has
 *  NO `rename_all`, so the wire fields are snake_case
 *  (`media_type` / `tokens_est`) and ride the nested message
 *  object verbatim on BOTH transports (httpTransport's
 *  camelCase→snake_case pass only rewrites top-level command
 *  params; Tauri's auto-camel only applies to command params
 *  too — nested struct fields deserialize with their declared
 *  serde names). */
export interface AttachmentWireRef {
  file: string;
  media_type: string;
  source: string;
  tokens_est?: number;
}

export const genId = () =>
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

  // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8):
  // the active session's full SessionSummary (or null if no
  // session is selected). The header UI uses this to decide
  // whether to render the group-chat indicator + "编辑参与者"
  // button (`session_type === "group_chat"`).
  const currentSession = computed<SessionSummary | null>(() => {
    const sid = currentSessionId.value;
    if (!sid) return null;
    return sessions.value.find((s) => s.id === sid) ?? null;
  });

  // Group chat (Phase 4 TODO-E8): the current session's
  // participant roster, parsed from `sessions.metadata.participants[]`.
  // `null` for non-group-chat sessions OR when the host hasn't
  // refreshed the session list yet (no metadata yet). The
  // GroupChatConfigModal re-edit path feeds this as
  // `initialParticipants`.
  const currentSessionParticipants = computed<ParticipantConfig[] | null>(() => {
    const cs = currentSession.value;
    if (!cs || cs.session_type !== "group_chat") return null;
    const meta = cs.metadata;
    if (!meta || typeof meta !== "object") return null;
    const raw = (meta as Record<string, unknown>).participants;
    if (!Array.isArray(raw)) return null;
    // Defensive: skip malformed entries. The session panel's
    // `updateGroupChatConfig` overwrites the array wholesale,
    // so a stale shape is self-healing on the next edit.
    const out: ParticipantConfig[] = [];
    for (const r of raw) {
      if (
        r &&
        typeof r === "object" &&
        typeof (r as { name?: unknown }).name === "string" &&
        typeof (r as { model?: unknown }).model === "string"
      ) {
        const e = r as ParticipantConfig;
        out.push({
          name: e.name,
          model: e.model,
          persona_md: e.persona_md,
        });
      }
    }
    return out;
  });

  /** 2026-06-26 (token-usage snapshot fix): OVERWRITE the
   *  per-session last-turn usage snapshot with this turn's
   *  `usage`. Called by `streamController.handleChatEvent` on
   *  every `done` event that carries a `usage` payload. Snapshot
   *  semantics: the value reflects the LLM's LAST request (not a
   *  running total) — matching Anthropic's statusline convention. */
  function setLastTurnUsage(
    sessionId: string,
    usage: SessionTokenUsage,
  ): void {
    tokenUsageBySession.set(sessionId, { ...usage });
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

  // D3 PR2 (2026-06-17): the message seq currently in inline edit
  // mode (`null` = no row is being edited). Stored on the chat store
  // rather than as a local ref in MessageItem because (a) MessageList
  // remounts on session switch and would lose a local ref, and
  // (b) only one row can be in edit mode at a time, so a single
  // nullable scalar is the right shape. The MessageItem reads it as
  // a computed and the parent flips it via the
  // `<MessageActionsMenu>`'s `edit` emit. Cleared on Save success
  // (the IPC + refresh has finished) and on Cancel.
  const editingMessageSeq = ref<number | null>(null);

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

  // Diff cache (declared here so the session-actions factory below
  // can capture it before the watchers fire). See `chatSessionActions`.
  const diffCache = ref<Map<string, DiffResult>>(new Map());

  // Session CRUD + worktree actions (08-10-chat-store-split: 拆出
  // chatSessionActions.ts,工厂 + ctx 注入,函数体原样保留)。
  const sessionActions = createSessionActions({
    sessions,
    currentSessionId,
    currentCwd,
    sessionLoading,
    diffCache,
    isCurrentSessionStreaming,
    controller,
    projectsStore,
    configStore,
    cancel,
  });
  const {
    loadSessions,
    createNewSession,
    updateGroupChatConfig,
    switchSession,
    openSessionInProject,
    deleteSession,
    clearSessionMessages,
    renameSession,
    setSessionColor,
    attachWorktree,
    detachWorktree,
    publishSessionToMain,
    deleteWorktree,
  } = sessionActions;

  watch(
    () => projectsStore.currentProjectId,
    async (newId) => {
      // Persist last-active project to localStorage. The config
      // store's own watcher writes to localStorage; we just update
      // its ref. Done here (not in the projects store) so the
      // persistence lives next to the read path (config.load) for
      // cohesion.
      //
      // Guard against null: this watcher is `immediate`, so it fires
      // once synchronously at store creation — before
      // `ChatWindow.onMounted` has restored the saved project. At
      // that moment `currentProjectId` is still null, and writing
      // null here would overwrite the just-restored
      // `lastActiveProjectId` and (via the config store's own
      // watcher) delete the localStorage key, so every cold start
      // fell back to the first project. Only persist real selections.
      if (newId !== null) {
        configStore.lastActiveProjectId = newId;
      }
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
    // 2026-06-26 snapshot fix: seed the per-session token usage
    // map from the SessionSummary's LAST-TURN snapshot (NOT the
    // legacy cumulative `*_total`). The判定 field is
    // `last_context_input_tokens` (the cross-provider-normalized
    // numerator) — if it's NULL, the session has no snapshot
    // (pre-snapshot legacy row or fresh session before first
    // turn) and the ChatInput hint renders "—".
    for (const s of sessions.value) {
      if (s.last_context_input_tokens !== null) {
        tokenUsageBySession.set(s.id, {
          input_tokens: s.last_input_tokens ?? 0,
          output_tokens: s.last_output_tokens ?? 0,
          cache_creation_input_tokens: s.last_cache_creation ?? 0,
          cache_read_input_tokens: s.last_cache_read ?? 0,
          context_input_tokens: s.last_context_input_tokens,
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
  // Diff (step 4 / PR3) — fetch and cache the session's worktree
  // diff. The IPC call is read-only and cheap (libgit2 walks the
  // tree, no remote I/O), but we still cache to avoid recomputing
  // for repeated clicks on the same session. The cache is keyed by
  // session id and is invalidated on session switch (so a stale
  // "diff from a different session" can't bleed through) and on
  // session delete.
  // -----------------------------------------------------------------------

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
    const result = await transport.invoke<DiffResult>("diff_worktree", { sessionId });
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

  /** B1 (2026-08-16) image-multimodal: map a message's
   *  `metadata.attachments` manifest onto the wire `attachments`
   *  array. Tolerant of BOTH shapes the manifest appears in (see
   *  `AttachmentView`): the optimistic camelCase form written by
   *  `chatSendActions.send` (entries carry `file` + `localUrl`) and
   *  the rehydrated snake_case form from the DB. Entries without a
   *  server `file` name (e.g. a pre-upload localUrl-only entry)
   *  are filtered out — nothing to fetch on the backend. */
  function toPayloadAttachments(m: ChatMessage): AttachmentWireRef[] {
    const raw = m.metadata?.attachments;
    if (!Array.isArray(raw)) return [];
    const out: AttachmentWireRef[] = [];
    for (const r of raw) {
      if (!r || typeof r !== "object") continue;
      const o = r as Record<string, unknown>;
      if (typeof o.file !== "string" || !o.file) continue;
      const mediaType =
        typeof o.media_type === "string"
          ? o.media_type
          : typeof o.mediaType === "string"
            ? o.mediaType
            : "";
      if (!mediaType) continue;
      const source = typeof o.source === "string" ? o.source : "paste";
      const tokensRaw = o.tokens_est ?? o.tokensEst;
      const tokens =
        typeof tokensRaw === "number" && Number.isFinite(tokensRaw)
          ? Math.max(0, Math.round(tokensRaw))
          : undefined;
      out.push({
        file: o.file,
        media_type: mediaType,
        source,
        ...(tokens !== undefined ? { tokens_est: tokens } : {}),
      });
    }
    return out;
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

  // Send action (08-10-chat-store-split: 拆出 chatSendActions.ts,
  // 工厂 + ctx 注入,函数体原样保留。cancel 5 行循环枢纽留 hub,
  // 经 ctx 注入给 sessions / message / send 三簇)。
  const sendActions = createSendActions({
    currentSessionId,
    forceFollowActive,
    isCurrentSessionStreaming,
    currentSession,
    controller,
    projectsStore,
    cancel,
    createNewSession,
    toPayloadContent,
    toPayloadAttachments,
  });
  const { send, stagedImages, addStagedImages, removeStagedImage, discardStagedImages } = sendActions;

  // B1 (2026-08-16) image-multimodal: staged paste images are
  // per-session in-memory state — drop them (revoking their
  // objectURLs) whenever the active session changes, including the
  // project-switch resets that flow through `currentSessionId`.
  watch(currentSessionId, () => {
    discardStagedImages();
  });

  // Edit / resend / retry actions (08-10-chat-store-split: 拆出
  // chatMessageActions.ts,工厂 + ctx 注入,函数体原样保留)。
  const messageActions = createMessageActions({
    currentSessionId,
    forceFollowActive,
    isCurrentSessionStreaming,
    currentSession,
    controller,
    projectsStore,
    cancel,
    toPayloadContent,
    toPayloadAttachments,
  });
  const { editMessage, resendMessage, retryChat } = messageActions;

  // Mode / yolo / workflow actions (08-10-chat-store-split: 拆出
  // chatModeActions.ts,工厂 + ctx 注入,函数体原样保留)。
  const modeActions = createModeActions({
    sessions,
    currentSessionId,
  });
  const {
    pendingYoloConfirm,
    pendingResolveRequest,
    requestSetMode,
    confirmYolo,
    cancelYolo,
    requestSetWorkflowEnabled,
    requestSetPluginName,
    listWorkflowPlugins,
  } = modeActions;

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
    // Group chat (07-29-group-chat, Phase 4 Step 3 TODO-E8):
    // the active session's full summary + the parsed
    // participants roster. The chat header uses these to
    // render the group-chat indicator + "编辑参与者" button.
    currentSession,
    currentSessionParticipants,
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
    // D3 PR2: the message seq currently in inline edit mode.
    // Written by `<MessageActionsMenu>`'s `edit` emit, cleared
    // on Save success / Cancel. UI consumers read it via
    // `chatStore.editingMessageSeq` (a `number | null`).
    editingMessageSeq,
    // Methods
    send,
    cancel,
    // B1 (2026-08-16) image-multimodal: paste-staging strip state +
    // actions. Owned by the send cluster so the send / clear /
    // session-switch lifecycle is store-owned (design §5.1);
    // ChatInput.vue renders the strip and forwards pasted Files.
    stagedImages,
    addStagedImages,
    removeStagedImage,
    discardStagedImages,
    loadSessions,
    createNewSession,
    updateGroupChatConfig,
    switchSession,
    // D2 (08-17-cross-session-search): search-modal "open in main
    // window" — project-aware switch (see chatSessionActions).
    openSessionInProject,
    deleteSession,
    // B3 (PR2): `/clear` — wipe messages, keep session row.
    clearSessionMessages,
    renameSession,
    setSessionColor,
    attachWorktree,
    detachWorktree,
    publishSessionToMain,
    deleteWorktree,
    fetchDiff,
    getDiff,
    getFileDiff,
    invalidateDiff,
    // 2026-06-26 snapshot fix: hook called by
    // streamController.handleChatEvent on every `done` event
    // that carries a usage payload. OVERWRITES the per-session
    // last-turn snapshot.
    setLastTurnUsage,
    // F5: hook called by streamController.handleChatEvent on
    // every `done` event that resolved a `totalMs`. Adds the
    // per-turn `totalMs` to the running session total.
    accumulateLatency,
    // A2 + B7 (PR2): per-session Mode setters. The Yolo gate
    // is held in `pendingYoloConfirm` and consumed by the
    // YoloConfirmModal mounted by `ModeSelect.vue`.
    pendingYoloConfirm,
    pendingResolveRequest,
    requestSetMode,
    confirmYolo,
    cancelYolo,
    // W1 (Workflow integration, Step 0.2 + 2026-07-09
    // chip-merge): per-session workflow opt-in toggle.
    // Wired by `<PluginSelect>`'s top-of-popover toggle
    // row (the former `<WorkflowToggle>` chip was deleted
    // in task 07-09-07-09-workflow-chip-merge and folded
    // into PluginSelect); optimistic-update + rollback on
    // IPC failure, no streaming guard, no Yolo gate.
    requestSetWorkflowEnabled,
    // W1 (Workflow integration, Step 2.2 + 2026-07-09
    // chip-merge): per-session active workflow plugin
    // name flip. Wired by `<PluginSelect>`'s popover
    // plugin list (sits below the toggle row in the same
    // popover); mirrors `requestSetWorkflowEnabled`'s
    // optimistic-update + rollback contract. Plugin rows
    // are disabled in the UI when workflow is OFF — the
    // store action itself does NOT guard on
    // `workflow_enabled` because the backend
    // `set_session_plugin_name` IPC intentionally accepts
    // name writes independent of the workflow flag
    // (lets a future flow pre-stage a plugin name before
    // turning workflow on).
    requestSetPluginName,
    // W1 Step 2.2: discover available plugins under
    // `<project>/.everlasting/workflow/`. Backs the
    // `PluginSelect.vue` popover data source.
    listWorkflowPlugins,
    // D3 PR2 (2026-06-17): user message edit + cascade delete
    // bridge to the backend `edit_user_message` IPC. Called by
    // `MessageItem.vue`'s Save handler; the parent catches
    // errors and keeps the edit mode active for retry.
    editMessage,
    // D3 PR3 (2026-06-17): re-fire an existing user message
    // (no content mutation). Called by `MessageActionsMenu`'s
    // `resend` emit; `MessageItem.vue` builds the
    // `contentText` from `message.content` and the chat
    // store fires the new stream with the `resendSeq` flag.
    resendMessage,
    // A5 R2 (2026-07-17): re-fire the chat stream in place at
    // an errored assistant row. Mirrors `resendMessage`'s
    // structure (cancel + ensureLoaded + startRequest) but
    // mutates the errored row instead of pushing new
    // placeholders, strips the `ERROR_MARKER` tail, and
    // does NOT carry a `resendSeq` flag (no audit). Called by
    // `MessageItemFooter`'s `↻ 重试` button when
    // `categoryRetryable(category)` resolves true.
    retryChat,
  };
});
