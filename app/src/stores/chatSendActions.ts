// chatSendActions — send action(拆分自 chat.ts,08-10-chat-store-split)。
//
// `send` 原样搬迁为 `createSendActions(ctx)` 工厂;ctx 注入共享 state +
// 跨簇 action(cancel / createNewSession)+ 跨簇 helper(toPayloadContent)。
// `cancel`(5 行循环枢纽,被 sessions 簇 / message 簇共用)留 hub。
// 拆分契约见 `.trellis/spec/frontend/state-management.md`
// §Stream Controller Pattern。
//
// B1 (2026-08-16) image-multimodal: this cluster additionally owns
// the paste-staging strip state (`stagedImages` + add/remove/discard
// actions) so the send / clear / session-switch lifecycle lives with
// the send flow it feeds (design §5.1) — ChatInput.vue only collects
// pasted Files and renders the strip.
import type { ComputedRef, Ref } from "vue";
import { ref } from "vue";
import { transport } from "../transport";
import { compressImage } from "../utils/imageCompress";
import { extractErrorMessage } from "../utils/useErrorBus";
import { useChecklistStore } from "./checklist";
import { useModelsStore } from "./models";
import type { useStreamControllerStore } from "./streamController";
import type { useProjectsStore } from "./projects";
import { genId, parseForcedDispatchPrefix, type AttachmentWireRef, type ChatMessagePayload } from "./chat";
import type { ChatMessage, SessionSummary, StagedImage } from "./chat.types";

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
  toPayloadAttachments: (m: ChatMessage) => AttachmentWireRef[];
}

// B1 paste-staging gates (PRD R6: aligned with the strictest of the
// two provider APIs; the backend re-checks both server-side).
/** png / jpg / webp only — gif / bmp / tiff / heic rejected. */
const IMAGE_MIME_WHITELIST = new Set(["image/png", "image/jpeg", "image/webp"]);
/** ≤10 staged images per turn. */
const MAX_STAGED_IMAGES = 10;
/** ≤5MB per image. */
const MAX_IMAGE_BYTES = 5 * 1024 * 1024;

/** Chunked Uint8Array → base64. `String.fromCharCode(...bytes)` on a
 *  5MB array blows the arg-count limit; 32KiB chunks don't. */
function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () =>
      reject(new Error(`read image failed: ${file.name || "unnamed"}`));
    reader.onload = () => {
      try {
        resolve(bytesToBase64(new Uint8Array(reader.result as ArrayBuffer)));
      } catch (e) {
        reject(e);
      }
    };
    reader.readAsArrayBuffer(file);
  });
}

/** Read pixel dimensions off the File via a throwaway objectURL.
 *  Resolves `{w:0,h:0}` on decode failure (jsdom, truncated file) —
 *  the caller's tokensEst floors at 1, mirroring the backend's
 *  fixed-pad fallback. Always revokes the objectURL. */
function readImageDimensions(file: File): Promise<{ w: number; h: number }> {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(file);
    const img = new Image();
    const done = (w: number, h: number) => {
      URL.revokeObjectURL(url);
      resolve({ w, h });
    };
    img.onload = () => done(img.naturalWidth || 0, img.naturalHeight || 0);
    img.onerror = () => done(0, 0);
    img.src = url;
  });
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
    toPayloadAttachments,
  } = ctx;

  // ---------------------------------------------------------------------
  // B1 paste-staging strip (R2a). In-memory only — switching sessions
  // or reloading drops unsent images (nothing hits the disk until
  // `save_attachment` runs inside `send`).
  // ---------------------------------------------------------------------
  const stagedImages = ref<StagedImage[]>([]);

  /** Stage pasted image Files after the per-file gates (mime
   *  whitelist / compress / 5MB-on-compressed / ≤10). Non-image
   *  entries are ignored (the paste & drop handlers only forward
   *  `image/*`, this is defensive); rejected files toast and DON'T
   *  block the rest of the batch.
   *
   *  08-21-b1-image-followups R1: compression runs BEFORE the 5MB
   *  gate (D3「压后判定」) — an oversized-but-compressible image
   *  gets downscaled/re-encoded and passes if the product fits;
   *  w/h/tokensEst all reflect the compressed file. Compression is
   *  fail-open: on any decode/encode failure the original file
   *  flows through and the old gates apply unchanged. */
  async function addStagedImages(files: File[]): Promise<void> {
    for (const f of files) {
      if (!f.type.startsWith("image/")) continue;
      if (!IMAGE_MIME_WHITELIST.has(f.type)) {
        projectsStore.showToast("仅支持 png / jpg / webp 图片", "warn");
        continue;
      }
      const result = await compressImage(f);
      const staged = result.file;
      if (staged.size > MAX_IMAGE_BYTES) {
        projectsStore.showToast("单张图片不能超过 5MB", "warn");
        continue;
      }
      if (stagedImages.value.length >= MAX_STAGED_IMAGES) {
        projectsStore.showToast(`单轮最多 ${MAX_STAGED_IMAGES} 张图片`, "warn");
        continue;
      }
      const { w, h } = result.w > 0 ? { w: result.w, h: result.h } : await readImageDimensions(staged);
      stagedImages.value.push({
        url: URL.createObjectURL(staged),
        file: staged,
        w,
        h,
        tokensEst: Math.max(1, Math.round((w * h) / 750)),
        ...(result.compressed
          ? {
              compressed: true,
              origW: result.origW,
              origH: result.origH,
              origBytes: result.origBytes,
            }
          : {}),
      });
    }
  }

  /** Drop one staged image (its ✕ button) — revoke its objectURL. */
  function removeStagedImage(index: number): void {
    const item = stagedImages.value[index];
    stagedImages.value.splice(index, 1);
    if (item) URL.revokeObjectURL(item.url);
  }

  /** Drop the whole strip WITH revoke — session switch / project
   *  change. NOT used after a successful send: the optimistic
   *  message still renders from the localUrls (see `send`). */
  function discardStagedImages(): void {
    for (const s of stagedImages.value) URL.revokeObjectURL(s.url);
    stagedImages.value = [];
  }

  /** Upload one staged image set via `save_attachment`; returns the
   *  per-image server refs. Any failure rejects — the caller aborts
   *  the whole send and keeps the strip (no partial sends). */
  async function uploadStagedImages(
    sessionId: string,
    staged: StagedImage[],
  ): Promise<Array<{ file: string; localUrl: string; mediaType: string; tokensEst: number }>> {
    const out: Array<{ file: string; localUrl: string; mediaType: string; tokensEst: number }> = [];
    for (const s of staged) {
      const dataBase64 = await fileToBase64(s.file);
      const resp = await transport.invoke<{ file: string }>(
        "save_attachment",
        {
          sessionId,
          mediaType: s.file.type,
          dataBase64,
        },
      );
      if (!resp || typeof resp.file !== "string" || !resp.file) {
        throw new Error("save_attachment: response missing `file`");
      }
      out.push({
        file: resp.file,
        localUrl: s.url,
        mediaType: s.file.type,
        tokensEst: s.tokensEst,
      });
    }
    return out;
  }

  async function send(text: string, staged?: StagedImage[]) {
    const stagedForTurn = staged ?? stagedImages.value;
    const trimmed = text.trim();
    // Empty input is always rejected — EXCEPT a pure-image send
    // (B1 R2a: empty text + staged images goes through so 看图问答
    // doesn't force typing).
    if (!trimmed && stagedForTurn.length === 0) return;
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
    // F1 消息队列 (2026-08-25): 经典 session 流式中不再丢弃发送 ——
    // 走排队路径(后端入队,当前轮结束后批量注入续轮)。群聊保持
    // 抢占语义(cancel + resend)不变。@@ 前缀在流式中直接拒绝(D8):
    // 强制派发与延迟注入组合语义不清,MVP 不做。
    const isGroupChat =
      currentSession.value?.session_type === "group_chat";
    const queueingClassic = isCurrentSessionStreaming.value && !isGroupChat;
    if (queueingClassic && trimmed.startsWith("@@")) {
      projectsStore.showToast(
        "流式期间不支持 @@ 派发：请等当前轮结束或先停止",
        "warn",
      );
      return;
    }
    if (isCurrentSessionStreaming.value && isGroupChat) {
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
    const modelsStore = useModelsStore();
    const parsed = parseForcedDispatchPrefix(trimmed, modelsStore.models);
    if (parsed === null) return; // empty task after prefix
    let forcedDispatch = parsed.forcedDispatch;
    let body = parsed.body;

    // Lazily create a session if there isn't one yet. `createNewSession`
    // throws if no project is active, so the chat area is expected to
    // be visible only when a project is selected (Q2 in dispatch
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
    // F1: 排队发送不开新轮,不重置在途轮的 checklist 视图。
    if (!queueingClassic) useChecklistStore().clearForNewRun(sessionId);

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

    // B1 (2026-08-16) image upload pass — runs AFTER the session id
    // exists (`save_attachment` needs it) and BEFORE the optimistic
    // message is pushed (its metadata carries the server file names
    // so `toPayloadAttachments` maps them onto the wire).
    let uploaded: Array<{ file: string; localUrl: string; mediaType: string; tokensEst: number }> = [];
    if (stagedForTurn.length > 0) {
      // R3 soft notice: model lacks vision → toast but still send
      // (the wire layer degrades Image blocks to a text placeholder
      // the model can read). Unresolvable model → skip the notice.
      const sessionModelId = currentSession.value?.model_id;
      const model =
        (sessionModelId ? modelsStore.byId(sessionModelId) : undefined) ??
        modelsStore.defaultModel;
      if (model && model.supportsImages === false) {
        projectsStore.showToast(
          "当前模型不支持图片，将以占位符发送",
          "warn",
        );
      }
      try {
        uploaded = await uploadStagedImages(sessionId, stagedForTurn);
      } catch (e) {
        // P1-3: any upload failure aborts the WHOLE send (no partial
        // sends) and the strip is kept so the user can retry / prune.
        projectsStore.showToast(
          `图片上传失败，已取消发送：${extractErrorMessage(e)}`,
          "error",
        );
        return;
      }
    }

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
      // B1: optimistic attachment manifest — camelCase + `localUrl`
      // for the pre-DB render (`MessageImages.vue` reads localUrl
      // first); `file` (server name) makes `toPayloadAttachments`
      // map it onto the wire history entry. The finalize reload
      // replaces this whole object with the backend's snake_case
      // manifest.
      ...(uploaded.length > 0
        ? {
            metadata: {
              attachments: uploaded.map((u) => ({
                file: u.file,
                localUrl: u.localUrl,
                mediaType: u.mediaType,
                source: "paste",
                tokensEst: u.tokensEst,
              })),
            },
          }
        : {}),
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
    //
    // B1: user rows carry their `attachments` refs (current turn's
    // optimistic manifest + rehydrated history manifests) so the
    // backend rebuilds Image blocks every request.
    const history: ChatMessagePayload[] = msgs
      .filter((m) => m.id !== assistantMsg.id)
      .map((m) => {
        const attachments = toPayloadAttachments(m);
        return {
          role: m.role,
          content: toPayloadContent(m),
          ...(attachments.length > 0 ? { attachments } : {}),
        };
      });

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

    // B1: release the staging strip. The objectURLs are NOT revoked
    // here — the optimistic user message still renders from
    // `metadata.attachments[].localUrl` until the finalize reload
    // replaces it with server-file refs. TODO(B1 follow-up): revoke
    // on message replacement (needs a hook into
    // `reloadAfterFinalize`); until then the URLs live until
    // session switch / page unload.
    stagedImages.value = [];
  }

  return {
    send,
    stagedImages,
    addStagedImages,
    removeStagedImage,
    discardStagedImages,
  };
}
