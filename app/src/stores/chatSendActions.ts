// chatSendActions — send action(拆分自 chat.ts,08-10-chat-store-split)。
//
// `send` 原样搬迁为 `createSendActions(ctx)` 工厂;ctx 注入共享 state +
// 跨簇 action(cancel / createNewSession)+ 跨簇 helper(toPayloadContent)。
// `cancel`(5 行循环枢纽,被 sessions 簇 / message 簇共用)留 hub。
// 拆分契约见 `.trellis/spec/frontend/state-management.md`
// §Stream Controller Pattern。
import type { ComputedRef, Ref } from "vue";
import { useChecklistStore } from "./checklist";
import { useModelsStore } from "./models";
import type { useStreamControllerStore } from "./streamController";
import type { useProjectsStore } from "./projects";
import { genId, parseForcedDispatchPrefix, type ChatMessagePayload } from "./chat";
import type { ChatMessage, SessionSummary } from "./chat.types";

export interface SendActionsContext {
  currentSessionId: Ref<string | null>;
  forceFollowActive: Ref<boolean>;
  isCurrentSessionStreaming: ComputedRef<boolean>;
  currentSession: ComputedRef<SessionSummary | null>;
  controller: ReturnType<typeof useStreamControllerStore>;
  projectsStore: ReturnType<typeof useProjectsStore>;
  cancel: () => Promise<void>;
  createNewSession: () => Promise<string>;
  toPayloadContent: (m: ChatMessage) => string | ChatMessagePayload["content"];
}

export function createSendActions(ctx: SendActionsContext) {
  const {
    currentSessionId,
    forceFollowActive,
    isCurrentSessionStreaming,
    currentSession,
    controller,
    projectsStore,
    cancel,
    createNewSession,
    toPayloadContent,
  } = ctx;

  async function send(text: string) {
    const trimmed = text.trim();
    // Empty input is always rejected.
    if (!trimmed) return;
    // Bug 6 fix (PR3): the old guard was a single global `sending`
    // ref. The new guard is per-session: the user can have multiple
    // sessions streaming concurrently, but they can't fire a second
    // message into the SAME session while it's still streaming.
    //
    // Group chat (Phase 4 / D9-Q4): preemptive interrupt. In a
    // group_chat session a human message while the host/participant
    // is streaming is *preemptive* — we cancel the in-flight turn
    // first (the backend `run_group_chat_loop` checks the cancel
    // token every round and breaks), then continue the normal send
    // path so the new message lands in the DB and the host re-enters
    // turn-taking (its reload observes the human interrupt). We do
    // NOT loosen the guard for ordinary `chat` sessions: there the
    // original "can't interject while streaming" semantics stay.
    if (isCurrentSessionStreaming.value) {
      if (currentSession.value?.session_type !== "group_chat") return;
      await cancel();
    }
    const projectId = projectsStore.currentProjectId;
    if (!projectId) {
      throw new Error("send: no current project");
    }

    // explicit-agent-dispatch (2026-06-30): detect a `@@<agent>
    // <task>` prefix. When present, strip it from the user message
    // body (the body becomes the task) and thread a `forcedDispatch`
    // payload through the `chat` IPC so the backend short-circuits
    // the LLM and dispatches the named subagent directly. An unknown
    // agent name is NOT rejected here — the backend's `run_subagent`
    // surfaces it as an error tool_result (cache.lookup miss). An
    // empty task after the prefix is rejected (no dispatch without a
    // brief). Only one leading `@@` prefix is honored.
    //
    // B6+ B (2026-07-07): an optional `--model=<X>` flag may appear
    // BETWEEN the agent name and the task (git/cargo flag semantics):
    //   `@@<agent> --model=<X> <task>`
    // `<X>` may be a model id or display_name; `resolveModelInput`
    // reverse-resolves display_name→id via `useModelsStore`. A `--model=`
    // flag appearing in the task body (not in the flag position) is NOT
    // extracted — it stays part of the task text. An unresolved name
    // yields `model_id: undefined` (the dispatch falls back to the
    // agent's configured default); the raw `--model=` text remains
    // visible in the input so the user can correct it. The wire field
    // is `model_id` (snake_case) to match the backend `ForcedDispatch`
    // struct (no serde rename — nested IPC struct fields pass through
    // verbatim, unlike top-level Tauri command args which auto-camel).
    const models = useModelsStore().models;
    const parsed = parseForcedDispatchPrefix(trimmed, models);
    if (parsed === null) return; // empty task after prefix
    let forcedDispatch = parsed.forcedDispatch;
    let body = parsed.body;

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

    // B12 Checklist (PR2 frontend, 2026-06-19): per-request
    // lifetime — a new user message starts a fresh run with a
    // fresh empty checklist. Mirror the backend's fresh
    // `Vec<ChecklistItem>` in each `run_chat_loop` invocation.
    // The controller's `reloadAfterFinalize` at the end of THIS
    // run will re-derive from history if any update_checklist
    // fires; for the duration of the stream the card stays
    // hidden until the first update_checklist tool_use arrives.
    useChecklistStore().clearForNewRun(sessionId);

    // B2 PR3 (bug fix 2026-06-17): compute the seq the
    // backend's `chat_loop` will assign to the user row.
    // The agent loop's `next_seq` counter starts at
    // `max(messages.seq) + 1` from `load_session` — the
    // same value we read off the rehydrated `msgs` here.
    // We stamp the user placeholder with this seq (and the
    // assistant placeholder with `nextSeq + 1`) so the
    // `ChatEvent::FileInjections` handler in
    // `streamController.ts` can locate the user message by
    // `m.seq === event.message_seq`. The rehydrated
    // messages all carry `seq` (set in
    // `rehydrateMessages` from `MessageRow.seq`); the
    // pre-stamping matters for the live path because the
    // freshly-pushed user/assistant placeholders are
    // not yet in the DB and so have no `seq` to read back.
    // Without this stamp, the live path silently drops
    // every `FileInjections` event.
    const nextSeq = msgs.reduce(
      (acc, m) => (typeof m.seq === "number" && m.seq > acc ? m.seq : acc),
      -1,
    ) + 1;

    // F2: activate force-follow mode so the chat stays scrolled to
    // bottom for the entire duration of the stream.
    forceFollowActive.value = true;

    const userMsg: ChatMessage = {
      id: genId(),
      // B2 PR3 (bug fix 2026-06-17): stamp the user message
      // with the seq the backend's `chat_loop` will assign.
      // The agent loop computes `next_seq = max(messages.seq)
      // + 1` from `load_session` at startup, and that value
      // is the seq the user row gets on `persist_turn`
      // (line 295 of `app/src-tauri/src/agent/chat_loop.rs`).
      // Without this, the `ChatEvent::FileInjections` handler
      // in `streamController.ts` does `msgs.find(m => m.role
      // === "user" && m.seq === event.message_seq)` and
      // NEVER finds the user message (its `seq` is undefined),
      // so the hint row under the user bubble never appears
      // during live streaming. Reload-after-DB-persist works
      // because `rehydrateMessages` reads `seq` from
      // `MessageRow.seq` and stamps it on every rehydrated
      // message — but the live path needs an explicit stamp
      // here.
      seq: nextSeq,
      role: "user",
      content: body,
    };
    const assistantMsg: ChatMessage = {
      id: genId(),
      // Assistant placeholder takes the next seq so
      // `case "turn_complete"` and `case "file_injections"`
      // both have a stable seq to key on. The agent loop
      // bumps seq after each `persist_turn` (user row →
      // assistant row → tool_result row), so the assistant
      // row seq is `userSeq + 1`.
      seq: nextSeq + 1,
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
      forcedDispatch,
      // 08-04 follow-up (群聊逐轮流式): group-chat sessions keep the
      // request alive across the inner per-speaker `Done`s and only
      // finalize on `Done { stop_reason: "group_chat_end" }`.
      groupChat: currentSession.value?.session_type === "group_chat",
    });
  }

  return { send };
}
