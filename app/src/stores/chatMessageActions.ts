// chatMessageActions — edit / resend / retry actions(拆分自 chat.ts,
// 08-10-chat-store-split)。
//
// 3 个 action + ERROR_MARKER_LOCAL 常量原样搬迁为
// `createMessageActions(ctx)` 工厂;ctx 注入共享 state + 跨簇 helper
// (cancel / toPayloadContent)。拆分契约见
// `.trellis/spec/frontend/state-management.md` §Stream Controller Pattern。
import type { ComputedRef, Ref } from "vue";
import { transport } from "../transport";
import { useChecklistStore } from "./checklist";
import type { useStreamControllerStore } from "./streamController";
import type { useProjectsStore } from "./projects";
import { genId, type AttachmentWireRef, type ChatMessagePayload } from "./chat";
import type { ChatMessage, SessionSummary } from "./chat.types";

export interface MessageActionsContext {
  currentSessionId: Ref<string | null>;
  forceFollowActive: Ref<boolean>;
  isCurrentSessionStreaming: ComputedRef<boolean>;
  currentSession: ComputedRef<SessionSummary | null>;
  controller: ReturnType<typeof useStreamControllerStore>;
  projectsStore: ReturnType<typeof useProjectsStore>;
  cancel: () => Promise<void>;
  toPayloadContent: (m: ChatMessage) => string | ChatMessagePayload["content"];
  /** B1 (2026-08-16): resend / retry rebuild the wire history from
   *  the in-memory (rehydrated) buffer — image-bearing user rows
   *  must round-trip their `metadata.attachments` refs so the
   *  backend re-attaches the Image blocks (design §6 D3). */
  toPayloadAttachments: (m: ChatMessage) => AttachmentWireRef[];
}

export function createMessageActions(ctx: MessageActionsContext) {
  const {
    currentSessionId,
    forceFollowActive,
    isCurrentSessionStreaming,
    currentSession,
    controller,
    projectsStore,
    cancel,
    toPayloadContent,
    toPayloadAttachments,
  } = ctx;

  // -----------------------------------------------------------------------
  // D3 PR2 (2026-06-17): user message edit + cascade delete
  //
  // Mirrors the backend `edit_user_message` Tauri command (PR1,
  // commit `308d277`): in-place update the row's content, cascade-
  // delete every strictly-later message in the session, append an
  // audit row. The frontend flow is:
  //
  //   1. Cancel any in-flight stream on the session — the backend
  //      `edit_user_message` command also cancels as a defense in
  //      depth (cancel_inflight_for_session + await_inflight_exit),
  //      but doing it on the frontend too means the in-memory
  //      `streaming` flag on the placeholder message clears via
  //      the same `done` event path, and the user sees the input
  //      row's send button re-enable in the same tick.
  //   2. Fire the IPC. The backend's `Result<(), String>` becomes
  //      a JS rejection on failure (Tauri's IPC contract) — we
  //      let it propagate to the caller, which surfaces it via a
  //      toast and keeps the parent in edit mode for retry.
  //   3. Refresh the controller's per-session message buffer
  //      from the DB. `refresh` evicts + re-loads, so the
  //      rehydrated buffer shows the new content (the new
  //      `content` / `text` columns + the bumped
  //      `metadata.edited_at`) AND the trimmed tail (the cascade
  //      DELETE). The Vue computed `messages` re-evaluates and
  //      the <MessageList> re-renders.
  //
  // The `Resend` half is intentionally NOT wired in PR2. The
  // backend doesn't have a `Resend` IPC yet (needs a new
  // `ChatEvent::Resend` variant + an audit kind + the spec for
  // the cancel-vs-resend race), and the dispatch prompt's "DoD"
  // lists it under "留 PR3". The UI menu item stays disabled with
  // a "PR3 待实施" tooltip.
  //
  // Multi-listener safety: this method only mutates the
  // controller's per-session buffer (via `controller.refresh`),
  // never the `sessions` list directly, and never the
  // `currentSessionId` ref. The SessionList / project tab
  // subscribers see the title's `updated_at` advance via the
  // existing `controller.activeRequests.size` watcher (which
  // fires `loadSessions` on any shrink). Pinia deep proxies
  // mean no listener sees a "reset" event — the new content
  // lands in place on the reactive array.
  // -----------------------------------------------------------------------
  async function editMessage(
    sessionId: string,
    messageSeq: number,
    newContent: string,
  ): Promise<void> {
    if (!sessionId) {
      throw new Error("editMessage: sessionId is required");
    }
    if (typeof messageSeq !== "number") {
      throw new Error("editMessage: messageSeq is required");
    }
    if (typeof newContent !== "string") {
      throw new Error("editMessage: newContent must be a string");
    }
    // 1. Stream race — cancel any in-flight stream on this
    // session. The chat store's `cancel` is per-current-session
    // (it reads `currentRequestId`); for cross-session edits
    // (e.g. user edits a message in session A while session B is
    // streaming) we use the controller's lower-level `cancel`
    // with the resolved requestId. The current session's case
    // is the common one and goes through the existing wrapper.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    } else {
      const rid = controller.currentRequestId(sessionId);
      if (rid) {
        await controller.cancel(rid);
      }
    }
    // 2. Fire the IPC. The backend takes `newContent` as a
    // plain string and wraps it in `MessageContent::Text`
    // (mirrors the wire shape the `chat` command's
    // `toPayloadContent` emits for a plain text message). The
    // Rust side serializes the new content to `messages.content`
    // (JSON `Vec<ContentBlock>` form) and the `text` denormalized
    // column. On error, the backend's `Result::Err(String)`
    // surfaces here as a rejected promise — we let it propagate
    // so the caller (`MessageItem.vue`'s Save handler) can
    // toast and keep the edit mode active.
    await transport.invoke<void>("edit_user_message", {
      sessionId,
      messageSeq,
      newContent,
    });
    // 3. Refresh the per-session message buffer. We always
    // refresh, even if the user is currently viewing a
    // different session — the rehydrated buffer lives in the
    // controller's LRU keyed by sessionId and will surface
    // correctly when the user navigates back. The `refresh`
    // helper does evict + ensureLoaded, so the new content +
    // trimmed tail are read from the DB and the in-memory
    // `messagesBySession` is replaced atomically (no blank
    // page flash — see the BUG FIX comment in
    // `finalizeRequest` for the same invariant).
    await controller.refresh(sessionId);
  }

  // -----------------------------------------------------------------------
  // D3 PR3 (2026-06-17): user message Resend — re-fire the
  // existing user prompt (no content mutation) by re-calling
  // `chat` with the same messages payload + a `resendSeq`
  // flag pointing at the original user message's seq. The
  // backend's agent loop detects the flag and writes a
  // `resend_message` audit row at the user-message persist
  // site (best-effort; see `app/src-tauri/src/agent/chat.rs`
  // `chat` command signature).
  //
  // Diff vs `editMessage`:
  // - No IPC `edit_user_message` call — content is unchanged.
  // - `chat` IPC receives an extra `resendSeq` parameter (the
  //   seq the user clicked Resend on). Backend audit fires at
  //   persist site; otherwise the request is identical to a
  //   normal send.
  // - No `controller.refresh` — the in-flight stream will
  //   stream into the same placeholder, and `finalizeRequest`
  //   will evict the buffer + `load_session` rehydrates
  //   including the (newly created) re-sent user message row.
  //
  // Stream race: same as `editMessage` — cancel any in-flight
  // stream first. The cancel order matters: the user clicks
  // Resend, we cancel the old stream, then we re-fire chat.
  // If the user clicks Resend twice in quick succession, the
  // second click cancels the first Resend's stream (which is
  // mid-flight) and starts yet another — the second
  // `resend_message` audit row will overwrite the first one's
  // role (the latest is the only one the user sees anyway).
  // -----------------------------------------------------------------------
  async function resendMessage(
    sessionId: string,
    messageSeq: number,
    contentText: string,
  ): Promise<void> {
    if (!sessionId) {
      throw new Error("resendMessage: sessionId is required");
    }
    if (typeof messageSeq !== "number") {
      throw new Error("resendMessage: messageSeq is required");
    }
    if (typeof contentText !== "string") {
      throw new Error("resendMessage: contentText must be a string");
    }
    // 1. Stream race — cancel any in-flight stream on this
    // session, mirroring `editMessage`'s defensive pattern.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    } else {
      const rid = controller.currentRequestId(sessionId);
      if (rid) {
        await controller.cancel(rid);
      }
    }
    // 2. Re-fire `chat` with the same messages payload + a
    // `resendSeq` flag. We mirror `send()`'s placeholder
    // construction (push a fresh userMsg + assistantMsg so the
    // controller's `case "delta"` finds the assistant message
    // to mutate), but the `resendSeq` flag tells the backend
    // this is a re-fire (audit at the user-message persist
    // site). The user message content is identical to the
    // original — we're re-running the same prompt.
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("resendMessage: no current project");
    }
    const msgs = await controller.ensureLoaded(sessionId);
    // B12 Checklist: per-request lifetime — resend starts a
    // fresh run; drop any prior checklist state. The new run's
    // first update_checklist will repopulate the card.
    useChecklistStore().clearForNewRun(sessionId);
    // Compute next seq for the new placeholders, same logic
    // as `send()`. The agent loop will use `max(loaded.seq)
    // + 1` for the actual persist, but we stamp the
    // in-memory placeholder so the controller's
    // `FileInjections` / `TurnComplete` events can key on it.
    const nextSeq = msgs.reduce(
      (acc: number, m: ChatMessage) =>
        typeof m.seq === "number" && m.seq > acc ? m.seq : acc,
      -1,
    ) + 1;
    forceFollowActive.value = true;
    const userMsg: ChatMessage = {
      id: genId(),
      seq: nextSeq,
      role: "user",
      content: contentText,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
      seq: nextSeq + 1,
      role: "assistant",
      content: "",
    };
    msgs.push(userMsg, assistantMsg);
    const history: ChatMessagePayload[] = msgs
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => {
        // B1: rehydrated user rows round-trip their image refs.
        const attachments = toPayloadAttachments(m);
        return {
          role: m.role,
          content: toPayloadContent(m),
          ...(attachments.length > 0 ? { attachments } : {}),
        };
      });
    // 3. Start the request with the `resendSeq` flag. Backend
    // audit fires at user-message persist site; otherwise the
    // request is identical to a normal send.
    await controller.startRequest({
      sessionId,
      projectId,
      userMsg,
      assistantMsg,
      history,
      // D3 PR3 (2026-06-17): mark this request as a resend
      // of the original user message at `messageSeq`. The
      // backend's agent loop reads this and writes a
      // `resend_message` audit row at the user-message
      // persist site (best-effort).
      resendSeq: messageSeq,
      // 08-04 follow-up (群聊逐轮流式): see the send() call site.
      groupChat: currentSession.value?.session_type === "group_chat",
    });
  }

  // -----------------------------------------------------------------------
  // A5 R2 (2026-07-17): chat-stream retry — re-fire the same
  // history after a `ChatEvent::Error` terminal event, "in place"
  // at the errored assistant message's row. Mirrors the structure
  // of `resendMessage` but with key differences:
  //
  //   - **NO new placeholders**: the errored assistant row is
  //     mutated (error → cleared, streaming → true). No new
  //     user/assistant pair is pushed.
  //   - **NO `resendSeq` flag**: this is a USER-LEVEL retry, not a
  //     user-message edit. The backend has no audit row for it
  //     (the original user message's persist already produced its
  //     audit). The agent loop runs on the same history up to and
  //     including the original user prompt.
  //   - **`ERROR_MARKER` strip**: the assistant's persisted text
  //     is the partial turn the Rust side flushed before the
  //     Error terminal ("[生成出错中断]" appended per RULE-A-007).
  //     We strip the marker (and any preceding blank line) so
  //     the new stream's first delta reads cleanly against the
  //     partial-turn text. The DB row stays unchanged; the
  //     in-memory placeholder is what the user sees during the
  //     live stream. After `finalizeRequest`, `reloadAfterFinalize`
  //     re-hydrates from the DB and overwrites the in-memory
  //     message — the partial-turn text + new content becomes the
  //     canonical row.
  //
  // Diff vs `resendMessage`:
  //
  //   | Dimension       | resendMessage           | retryChat                |
  //   |-----------------|-------------------------|--------------------------|
  //   | placeholder     | push new                | mutate existing errored  |
  //   | seq             | use next seq (max+1)    | same errored row seq     |
  //   | resendSeq flag  | yes (audit)             | no (no audit)            |
  //   | ERROR_MARKER    | n/a (new content)       | stripped                 |
  //
  // The error-cancel guard at the top mirrors `resendMessage` —
  // if the user double-tapped "↻ 重试" while a retry was already
  // in flight, the second call cancels the first request and
  // starts fresh (the second request's `done` wins the race).
  // -----------------------------------------------------------------------
  const ERROR_MARKER_LOCAL = "[生成出错中断]";

  async function retryChat(
    sessionId: string,
    messageSeq: number,
  ): Promise<void> {
    if (!sessionId) {
      throw new Error("retryChat: sessionId is required");
    }
    if (typeof messageSeq !== "number") {
      throw new Error("retryChat: messageSeq is required");
    }
    // 1. Stream race — cancel any in-flight stream on this
    //    session. Defensive: a mid-stream retry would race the
    //    in-flight stream's `done` event against the new request.
    if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
      await cancel();
    } else {
      const rid = controller.currentRequestId(sessionId);
      if (rid) {
        await controller.cancel(rid);
      }
    }
    // 2. ensureLoaded + clearForNewRun, mirror send/resend.
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("retryChat: no current project");
    }
    const msgs = await controller.ensureLoaded(sessionId);
    useChecklistStore().clearForNewRun(sessionId);
    // 3. Locate the errored assistant message + the preceding
    //    user message (the prompt that triggered the failed
    //    turn). If we can't find both, surface a clear error —
    //    a retry button clicked against a missing row means a
    //    stale UI (e.g. cleared session) and we want to refuse
    //    silently rather than fire a stranded stream.
    const errored = msgs.find(
      (m) => m.role === "assistant" && m.seq === messageSeq && m.error,
    );
    if (!errored) {
      throw new Error(
        `retryChat: errored assistant message at seq ${messageSeq} not found`,
      );
    }
    const userIdx = msgs.findIndex(
      (m) => m.role === "user" && typeof m.seq === "number" && m.seq === messageSeq - 1,
    );
    if (userIdx < 0) {
      throw new Error(
        `retryChat: user message at seq ${messageSeq - 1} not found (cannot re-fire without the prompt)`,
      );
    }
    // 4. Mutate the errored assistant in place:
    //    - clear the error
    //    - strip the trailing ERROR_MARKER (and any blank-line
    //      separator) so the first delta reads cleanly
    //    - mark streaming so the UI re-renders with cursor
    //    - clear toolCalls / toolResults / thinkingBlocks so
    //      the bubble + cards re-render empty
    errored.error = undefined;
    errored.streaming = true;
    errored.toolCalls = [];
    errored.toolResults = [];
    errored.thinkingBlocks = [];
    errored.redactedThinkingData = [];
    errored.latency = undefined;
    // Strip trailing ERROR_MARKER (`<text>\n\n[生成出错中断]`)
    // and any blank-line separator preceding it. The marker is
    // on its own line per the backend's `flush_pending_*`
    // pattern (helpers.rs:307); strip the marker + any
    // preceding `\n` repetition to recover the partial text.
    let cleaned = errored.content;
    if (cleaned.endsWith(ERROR_MARKER_LOCAL)) {
      cleaned = cleaned.slice(0, -ERROR_MARKER_LOCAL.length).replace(
        /(\r?\n)+$/,
        "",
      );
    } else if (cleaned.includes(ERROR_MARKER_LOCAL)) {
      // marker mid-text (rare, defensive) — strip from marker-onward
      cleaned = cleaned.slice(
        0,
        cleaned.lastIndexOf(ERROR_MARKER_LOCAL),
      );
    }
    errored.content = cleaned;
    // 5. The history we send to the backend is the in-memory
    //    buffer trimmed to the errored row (inclusive). The
    //    backend will reuse the persisted rows up to that point
    //    (no DB write from us). The `assistantMsg.id` /
    //    `userMsg.id` ids we pass to `startRequest` are existing
    //    rows — `startRequest` stamps them on the active request
    //    so the delta handler routes events to the in-place
    //    message (NOT a brand-new placeholder).
    const assistantMsg = errored;
    const userMsg = msgs[userIdx];
    // F2: activate force-follow mode so the chat stays scrolled
    // to bottom for the duration of the new stream (mirrors send
    // + resend).
    forceFollowActive.value = true;
    // 6. `clear-content` semantics for the wire: build history
    //    up to and INCLUDING the user prompt. The errored assistant
    //    itself is NOT included in the payload — the backend
    //    rebuilds its own assistant message from the streaming
    //    events and persists at the next turn boundary (RULE-A-007
    //    flush path).
    const historyMsgs = msgs.slice(0, userIdx + 1);
    const history: ChatMessagePayload[] = historyMsgs.map((m) => {
      // B1: retry re-fires the same history — image-bearing user
      // rows keep their attachment refs.
      const attachments = toPayloadAttachments(m);
      return {
        role: m.role,
        content: toPayloadContent(m),
        ...(attachments.length > 0 ? { attachments } : {}),
      };
    });
    await controller.startRequest({
      sessionId,
      projectId,
      userMsg,
      assistantMsg,
      history,
      // NO resendSeq: this is a user-level retry, not a message
      // edit. The backend does not write a `resend_message`
      // audit row.
      resendSeq: undefined,
      // 08-04 follow-up (群聊逐轮流式): see the send() call site.
      groupChat: currentSession.value?.session_type === "group_chat",
    });
  }

  return {
    editMessage,
    resendMessage,
    retryChat,
  };
}
