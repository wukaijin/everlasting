<script lang="ts">
// Module-scope IPC cache for `@`-mention file lists. Lives outside
// `<script setup>` so it survives ChatInput remounts (e.g. when the
// chat panel is destroyed and re-created on session switch).
//
// Two independent caches:
//   - `fileCache: Map<projectId, shallow[]>` — per-project shallow
//     (3-layer) walk under `project.path`. Invalidated when
//     `projectsStore.currentProjectId` changes (see the `watch`
//     in `<script setup>`).
//   - `systemRootCache: TriggerMenuItem[] | null` — the literal `/`
//     walk served by `list_files_at`. Project-independent (the
//     filesystem root doesn't change with the project), so it
//     survives project switches and only resets when the app
//     reloads the cache (no automatic invalidation today).
//
// The composable owns the reactive `fileItems` / `fileSystemRootItems`
// refs and the per-mode `loaded` flags; `resetFilePanelState` on
// project switch clears those + the per-project `fileCache` entry.

interface FileCacheEntry {
  shallow?: import("./TriggerMenu.vue").TriggerMenuItem[];
}

const fileCache = new Map<string, FileCacheEntry>();

let systemRootCache: import("./TriggerMenu.vue").TriggerMenuItem[] | null = null;

export {};
</script>

<script setup lang="ts">
// ChatInput — chat composer. A CodeMirror 6 single-line editor that
// auto-grows up to ~200px + a circular Prussian-blue send button on
// the right, with a small hint row below. Matches the spike-003
// reference layout (ui-A.png).
//
// PR1.5 (2026-06-17): the underlying <textarea> was replaced with
// CodeMirror 6. Rationale: CM 6 handles Chinese IME composition
// natively (no manual `isComposing` ref + `compositionstart/end`
// listeners — `view.composing` is the source of truth), and the
// decoration API will let PR-B token-color `/command` / `@file` /
// skill tokens without fighting overlay caret-sync issues.
//
// PR5 (2026-06-17): when `sending` is true, the right-side send
// button morphs into a Stop button. Clicking it emits `stop`; the
// parent calls `chatStore.cancel()`.
//
// Split refactor (2026-06-23, task `06-23-06-23-split-chat-input`):
// the 1834-line monolith was decomposed into:
//   - `app/src/utils/chatInputCodeMirror.ts` — CM 6 composable
//     (host + keymap + IME + `/` + `@` trigger detection +
//     replaceDoc). **0 store import** (ADR-1).
//   - `app/src/components/chat/ChatInputLatencyPopover.vue` — F5
//     latency chip + click popover. **0 store import**, 0 emit.
//   - `app/src/components/chat/ChatInputHintRow.vue` — hint row
//     (latency + token tooltip + ModelSelect). **0 store import**,
//     0 emit.
//
// This component now owns:
//   - Public API: `props.sending` + `props.placeholder` + `send` /
//     `stop` emits (unchanged from before the split).
//   - The <div ref="host"> CM mount element.
//   - Store reads (chatStore / modelsStore / projectsStore) and the
//     few derived computeds (`currentModelContextWindow`,
//     `usageLevel`, `inputRowStyle`).
//   - The dispatch handlers (`onCommandSelect` / `onFileSelect`)
//     that touch Tauri `invoke` + `chatStore.send` — these are
//     NOT in the composable because they need store access (kept
//     per ADR-1: composable 0 store import).
//   - `submit()` (read text, clear CM doc, emit send).
//   - `cycleMode()` (Shift+Tab mode cycle wired through
//     `useKeyboard`).
//
// Public API contract (locked — `ChatPanel.vue` zero modification):
//   props:  { sending: boolean; placeholder?: string }
//   emits:  { send: [text: string]; stop: [] }

import { computed, nextTick, ref, watch, watchEffect } from "vue";
import { transport } from "../../transport";
import { extractErrorMessage } from "../../utils/useErrorBus";
import Icon from "../Icon.vue";
import ModeSelect from "./ModeSelect.vue";
// W1 (Workflow integration, Step 0.2 — 2026-07-08):
// W1 (Workflow integration, Step 2.2 + 2026-07-09
// chip-merge): merged workflow opt-in + plugin picker
// into a single `<PluginSelect>` chip + popover. The
// former `<WorkflowToggle>` chip was deleted (task
// 07-09-07-09-workflow-chip-merge) because it was a
// conditionally-dependent sibling — its presence was
// meaningless when workflow was OFF (PluginSelect hid
// itself) and redundant when ON (two chips reading as
// the same concept). Now this parent just mounts
// PluginSelect, which owns the toggle + popover + IPC
// round-trip and the per-session `hasSession` gate.
import PluginSelect from "./PluginSelect.vue";
import TriggerMenu, { type TriggerMenuItem } from "./TriggerMenu.vue";
import ChatInputHintRow from "./ChatInputHintRow.vue";
import { useChatInputCodeMirror, type FileViewMode } from "../../utils/chatInputCodeMirror";
import { useMessageQueueStore } from "../../stores/messageQueueStore";
import { useChatStore } from "../../stores/chat";
import {
  MODE_CYCLE,
  type HandoffResult,
  type ManualCompactionResult,
  type SessionMode,
  type StagedImage,
} from "../../stores/chat.types";
import { useModelsStore } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";
import { matchBuiltinCommandInput } from "../../utils/slashCommand";
import { colorTagHex, hexToRgba } from "../../utils/colorTag";
import { registerShiftTabCycle } from "../../utils/useKeyboard";
import { useMobileKeyboard } from "../../composables/useMobileKeyboard";

/** B4 (Stretch 2) merged `/`-trigger panel (2026-06-18): wire DTO
 *  from the Rust `commands::panel::PanelItem`. The `source` field is
 *  one of `"builtin"` / `"command"` / `"skill"`. The dispatcher
 *  (`onCommandSelect` further below) reads `source` to pick the
 *  right path:
 *  - `"builtin"` → client-side action (B3 `executeCommand` for
 *    `/help` / `/clear` / `/new`)
 *  - `"command"` → `get_command_body` → user message (B3 path)
 *  - `"skill"` → `get_skill_body` → user message (Stretch 2 path) */
interface PanelItem {
  name: string;
  description: string;
  argument_hint: string | null;
  source: "builtin" | "command" | "skill";
  is_builtin: boolean;
}

const props = defineProps<{
  /** True while the model is generating. Disables the input. */
  sending: boolean;
  /** Placeholder text shown when empty. */
  placeholder?: string;
}>();

const emit = defineEmits<{
  /** B1 (2026-08-16) image-multimodal: the staged paste-image set
   *  rides the send event (ChatInput → ChatPanel → ChatWindow →
   *  `chatStore.send(text, staged)`). The strip state itself lives
   *  on the chat store; this is the same array reference. */
  send: [text: string, staged: StagedImage[]];
  stop: [];
}>();

const chatStore = useChatStore();
const modelsStore = useModelsStore();
const projectsStore = useProjectsStore();

// === Computed props for HintRow + ChatInput row style ===============

/** The model row backing the current session, or `null` for
 *  sessions that haven't resolved to a model yet (very
 *  early in the app lifecycle, before the catalog loads). The
 *  percentage denominator is `defaultModel.contextWindow` —
 *  the chat command always uses the default model for
 *  resolve-default fallback; a per-session override is also
 *  possible but the user explicitly picks that, and the
 *  percentage uses the same `defaultModel` for visual
 *  stability (a session mid-stream with a per-session override
 *  would still see "X% / 200K" of the default's window). */
const currentModelContextWindow = computed<number>(() => {
  const m = modelsStore.defaultModel;
  return m?.contextWindow ?? 200_000;
});

/** Color threshold for the percentage bar. Matches the
 *  PRD §Q4 decision 6 (50% yellow, 75% red):
 *  - 0-49% → green
 *  - 50-74% → yellow
 *  - 75%+ → red.
 *
 *  The actual band lookup lives in `utils/tokenUsage.ts` so the
 *  boundaries (49/50/74/75) can be unit-tested without spinning
 *  up a Vue renderer + Pinia store. */
const usageLevel = computed<TokenUsageLevel | null>(() => {
  const u = chatStore.currentSessionTokenUsage;
  if (!u) return null;
  // 2026-06-26 snapshot fix: use the cross-provider-normalized
  // `context_input_tokens` (Anthropic: input+cc+cr; OpenAI:
  // prompt_tokens) as the "% of context_window" numerator.
  const pct = u.context_input_tokens / currentModelContextWindow.value;
  return tokenUsageLevel(pct);
});

// D1: conditional background tint on chat-input__row from session color tag.
const inputRowStyle = computed(() => {
  const s = chatStore.sessions.find((x) => x.id === chatStore.currentSessionId);
  if (!s || s.color_tag === null) return {};
  const hex = colorTagHex(s.color_tag);
  if (!hex) return {};
  return { backgroundColor: hexToRgba(hex, 0.2) };
});

// === CodeMirror 6 composable =====================================
//
// The composable owns the CM lifecycle, IME-aware keymap, `/` + `@`
// trigger detection, and panel state. We only need:
//   - `host` (template ref to the <div>)
//   - `sending` / `placeholder` as refs (so the Compartment
//     watchers can reconfigure without rebuilding state)
//   - `onSubmit` callback that reads the current doc, emits `send`,
//     and clears the CM doc
//   - `commandItemsSource` / `fileItemsSource` callbacks that the
//     composable invokes when opening each panel (ADR-2 — keeps the
//     composable free of store imports; the callbacks can call
//     Tauri `invoke` directly).

const host = ref<HTMLDivElement | null>(null);

// S5 移动端软键盘适配:监听 visualViewport 写 --visual-viewport-height CSS
// 变量,AppShell 移动端 height 引用它,软键盘弹起时缩到键盘上方。iOS only
// 机制(Android resize layout viewport,本调用对 Android 无害)。见 design §4.1。
useMobileKeyboard();

// F1 消息队列 (2026-08-25): 经典 session 流式期间解锁输入(可继续
// 打字并排队发送,PRD R1);仅群聊保持锁死(cancel+resend 抢占语义,
// AC4)。`sendingEffective` 驱动 readOnly compartment 与输入框禁用
// 视觉;发送键 disabled 同样看 `sending && isGroupChat`(流式中经典
// session 发送 = 排队),Stop/Esc 看真实 streaming(见下)。
const isGroupChatSession = computed(
  () => chatStore.currentSession?.session_type === "group_chat",
);
const sendingEffective = computed(() => props.sending && isGroupChatSession.value);

const cm = useChatInputCodeMirror({
  host,
  sending: sendingEffective,
  placeholder: computed(() => props.placeholder),
  onSubmit: () => {
    const text = cm.input.value;
    // B1 R2a: pure-image sends pass (empty text + staged images);
    // only an empty text AND empty strip is a no-op.
    // F1: 经典 session 流式中放行走排队路径(chatSendActions 的
    // queueingClassic → 后端入队);群聊保持拦截(cancel+resend 抢占,
    // AC4 —— 输入框本身已被 sendingEffective 锁死,此守卫是兜底)。
    if (
      (!text.trim() && chatStore.stagedImages.length === 0) ||
      (props.sending && isGroupChatSession.value)
    ) {
      return;
    }
    // 08-18-manual-compact-command: typed builtin dispatch. With the
    // palette closed (or Esc'd), `/compact focus…` + Enter must run the
    // command instead of shipping the text to the LLM. Same handler as
    // palette selection (`executeBuiltin`) so the two paths can't drift;
    // `/help` `/clear` `/new` typed directly gain the palette behavior
    // here too (they previously went to the model as plain messages).
    const builtin = matchBuiltinCommandInput(text);
    if (builtin) {
      const v0 = cm.view.value;
      if (v0) {
        const cur = v0.state.doc.toString();
        if (cur.length > 0) {
          v0.dispatch({ changes: { from: 0, to: cur.length, insert: "" } });
        }
      } else {
        cm.input.value = "";
      }
      void executeBuiltin(builtin.name, builtin.rest || undefined);
      return;
    }
    const v = cm.view.value;
    if (v) {
      const cur = v.state.doc.toString();
      if (cur.length > 0) {
        v.dispatch({ changes: { from: 0, to: cur.length, insert: "" } });
      }
    } else {
      cm.input.value = "";
    }
    emit("send", text, chatStore.stagedImages);
  },
  // B1 (2026-08-16): pasted image files go straight to the chat
  // store's staging strip (mime/size/count gates + objectURL
  // lifecycle live there — see chatSendActions).
  onPasteImages: (files) => {
    void chatStore.addStagedImages(files);
  },
  commandItemsSource: async (): Promise<TriggerMenuItem[]> => {
    const projectId = projectsStore.currentProjectId;
    try {
      const list = await transport.invoke<PanelItem[]>("list_panel_items", {
        projectId: projectId ?? null,
      });
      return list.map((c) => ({
        key: `${c.source}:${c.name}`,
        name: c.name,
        description: c.description || undefined,
        argument_hint: c.argument_hint ?? undefined,
        source: c.source,
        is_builtin: c.is_builtin,
      }));
    } catch (e) {
      console.error("list_panel_items failed:", e);
      return [];
    }
  },
  fileItemsSource: async (mode: FileViewMode): Promise<TriggerMenuItem[]> => {
    const projectId = projectsStore.currentProjectId;
    try {
      if (mode === "system_root") {
        return await getOrLoadSystemRoot();
      }
      if (!projectId) return [];
      return await getOrLoadShallow(projectId);
    } catch (e) {
      console.error("list_files failed:", e);
      return [];
    }
  },
  agentItemsSource: async (): Promise<TriggerMenuItem[]> => {
    const projectId = projectsStore.currentProjectId;
    try {
      const list = await transport.invoke<{ name: string; description: string; source: string }[]>(
        "list_subagents",
        { projectId: projectId ?? null },
      );
      return list.map((a) => ({
        key: `${a.source}:${a.name}`,
        name: a.name,
        description: a.description || undefined,
        source: a.source,
      }));
    } catch (e) {
      console.error("list_subagents failed:", e);
      return [];
    }
  },
});

// === `@`-mention file list cache =================================
//
// Two paths through `getOrLoad*`:
// - `getOrLoadShallow(projectId)` → `list_files(projectId, 3)` —
//   the default-`@` 3-layer walk under project root. Cached per
//   project.
// - `getOrLoadSystemRoot()` → `list_files_at("/", 4)` — the literal
//   filesystem root walk served when the user types `@/` (e.g. to
//   mention `/etc/hosts`). Cached globally (project-independent).
//
// `maxDepth: 3` for shallow keeps the typical `tree -L 3` reach —
// enough to surface Cargo.toml / src/* / app/src-tauri/src/* in this
// repo without ever visiting node_modules / target. `maxDepth: 4`
// for system root lets the user reach `/usr/local/bin/*` while
// still hitting the 5000-file cap on the noisy `/usr/share/*` tree.

/** Default shallow depth — project root + 2 nested levels. */
const FILE_SHALLOW_DEPTH = 3;

/** System-root depth — `/` + 3 nested levels. Beyond 4, `/usr/share`
 *  alone exceeds the 5000-file IPC cap and the picker degrades. */
const FILE_SYSTEM_ROOT_DEPTH = 4;

async function getOrLoadShallow(
  projectId: string,
): Promise<TriggerMenuItem[]> {
  const entry = fileCache.get(projectId);
  if (entry?.shallow) return entry.shallow;

  const paths = await transport.invoke<string[]>("list_files", {
    projectId,
    maxDepth: FILE_SHALLOW_DEPTH,
  });
  const items = paths.map((p) => ({ key: p, name: p }));
  fileCache.set(projectId, { shallow: items });
  return items;
}

async function getOrLoadSystemRoot(): Promise<TriggerMenuItem[]> {
  if (systemRootCache) return systemRootCache;
  const paths = await transport.invoke<string[]>("list_files_at", {
    root: "/",
    maxDepth: FILE_SYSTEM_ROOT_DEPTH,
  });
  // Prefix items with `/` so they're rendered as absolute paths
  // (the walk returns root-relative `etc/hosts`, not `/etc/hosts`).
  // TriggerMenu displays `item.name` verbatim.
  const items = paths.map((p) => ({ key: `/${p}`, name: `/${p}` }));
  systemRootCache = items;
  return items;
}

/** On project switch: wipe BOTH the IPC cache (this module's
 *  module-scope `fileCache`) and the composable's reactive panel
 *  state (`fileItems` / `fileSystemRootItems` / loaded flags).
 *  `systemRootCache` is deliberately preserved — the filesystem
 *  root doesn't change with the project, and clearing it forces a
 *  fresh `/` walk (~1s) for no gain. Skipping the composable reset
 *  is what caused the original bug — the IPC cache was cleared but
 *  `cm.fileItems.value` still held the previous project's list, so
 *  the next `@` open rendered stale paths because `shallowLoaded`
 *  was still true and the IPC fetch was skipped. */
watch(
  () => projectsStore.currentProjectId,
  (newId, oldId) => {
    if (oldId && newId !== oldId) {
      fileCache.delete(oldId);
      cm.resetFilePanelState();
    }
  },
);

// === TriggerMenu ref bindings ====================================
//
// The composable needs to call `moveActive` / `confirmActive` on the
// `<TriggerMenu>` instances when the user presses arrow / Tab / Enter.
// We bind them via the standard Vue template-ref pattern; the
// composable reads `commandMenuRef` / `fileMenuRef` reactively.

const triggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
const fileTriggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
// explicit-agent-dispatch (2026-06-30): @@-trigger agent panel.
const agentTriggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);

// Mirror: composable's commandMenuRef should track the local
// triggerMenu ref. The composable's internal ref is exported and
// mutable; we use a watchEffect that copies the value across on
// every change so the CM keymap's `moveActive` / `confirmActive`
// calls land on the current TriggerMenu instance.
watchEffect(() => {
  cm.commandMenuRef.value = triggerMenu.value;
});
watchEffect(() => {
  cm.fileMenuRef.value = fileTriggerMenu.value;
});
watchEffect(() => {
  cm.agentMenuRef.value = agentTriggerMenu.value;
});

// === Send / Stop / Esc ===========================================


// F1 R8「修改」= 排队消息退回输入框:消费 queue store 的回填草稿。
// watch 草稿本身而非 currentSessionId —— 当前 session 内 recall 也
// 要立即回填(原实现仅 session 切换时消费:退回后 composer 不回填,
// 草稿滞留到下次切回才幽灵出现,评审 Round 2 P1 修复)。
watch(
  () => useMessageQueueStore().recallDraft,
  () => {
    const text = useMessageQueueStore().takeRecallDraft(chatStore.currentSessionId);
    if (text != null) cm.input.value = text;
  },
  { immediate: true },
);

function onStop() {
  emit("stop");
}

// B1 R2a: send is enabled for any non-empty text OR a non-empty
// staging strip (pure-image sends).
// F1 单按钮三态(2026-08-25 拍板):有草稿(文本或 staged 图)= 发送键
// (流式中发送 = 排队,AC1 / design §6 P2);无草稿 + sending = Stop
// 变形(PR5「空输入也可打断长流」)。Trade-off:流式中有草稿时无直接
// Stop 入口 —— 先清空草稿按钮即变 Stop(空草稿时 Esc = Stop,P2-5)。
const hasDraft = computed(
  () => cm.input.value.trim().length > 0 || chatStore.stagedImages.length > 0,
);
const showStop = computed(() => props.sending && !hasDraft.value);
const sendDisabled = (): boolean =>
  (props.sending && isGroupChatSession.value) || !hasDraft.value;

function onEscKeydown() {
  if (!props.sending) return;
  // F1 / P2-5 拍板:经典 session 流式中编辑器**非空**时 Esc 不触发
  // Stop —— 防止打好的排队输入被"Stop 清队列"连带丢弃;要停先清空
  // 草稿(按钮随即变 Stop)再点或按 Esc。群聊保持原语义(Esc 即打断)。
  if (!isGroupChatSession.value && cm.input.value.trim().length > 0) return;
  onStop();
}

// === Mode cycle (Shift+Tab, B7 PR2) =============================

/**
 * PR2 (B7): Shift+Tab cycle through the per-session Mode.
 *
 * Wired via the `useKeyboard` module so the listener lives at
 * the capture phase on `window` — the default browser
 * behaviour (reverse-tab focus traversal) MUST be suppressed
 * with `e.preventDefault()`, which a per-component listener
 * on the editor can't reliably do once focus has moved
 * elsewhere.
 *
 * The cycle order is `MODE_CYCLE` (Edit → Plan →
 * Yolo → Edit). We delegate the actual IPC + Yolo confirm
 * gate to `chatStore.requestSetMode` so the popover path
 * (`ModeSelect`) and the keyboard path share exactly one
 * orchestrator — Shift+Tab into Yolo will pop the same
 * `YoloConfirmModal` as clicking Yolo in the popover.
 *
 * Streaming gate: the cycle is suppressed while the active
 * session is streaming (matches `ModeSelect`'s `:disabled`
 * contract and the backend rule "mode applies on next turn
 * boundary" — PR1 mode check at ⑧a).
 */
async function cycleMode(): Promise<void> {
  const sid = chatStore.currentSessionId;
  if (!sid) return;
  const summary = chatStore.sessions.find((s) => s.id === sid);
  if (!summary) return;
  const current = (summary.mode as SessionMode) ?? "edit";
  const idx = MODE_CYCLE.indexOf(current);
  if (idx === -1) return;
  const next = MODE_CYCLE[(idx + 1) % MODE_CYCLE.length];
  if (next === current) return;
  await chatStore.requestSetMode(sid, next);
}

registerShiftTabCycle({
  cycle: () => {
    void cycleMode();
  },
  enabled: () => !chatStore.isCurrentSessionStreaming && !!chatStore.currentSessionId,
});

// === TriggerMenu dispatch handlers ==============================
//
// These two stay in ChatInput.vue because they touch Tauri
// `invoke` + `chatStore.send` (the composable is 0 store import
// per ADR-1). The composable exposes `currentSlashToken` /
// `currentAtToken` so we can read the token geometry here.

/** Selected-item dispatcher. Called by TriggerMenu's `@select`.
 *  Three dispatch paths, picked by `item.source` (B4 Stretch 2):
 *  - `builtin` → client-side action (no LLM): `/help` reopens the
 *    panel; `/clear` clears messages; `/new` creates a session.
 *  - `command` → `get_command_body` → sent as a user message (B3).
 *  - `skill` → leave `/skill-name ` in the editor (NOT auto-sent,
 *    NOT body-expanded). The user can append text and send the raw
 *    `/skill-name ...`; the agent then loads the skill body itself
 *    via the `use_skill` tool (L1 progressive disclosure).
 *
 *  builtin + command strip the `/`-token before dispatch (anywhere
 *  on the line via `[slashOffset, tokenEnd)`); skill instead
 *  REPLACES the typed prefix with the canonical `/skill-name ` so
 *  the editor holds a clean reference. */
/** Shared builtin dispatcher — the palette-select path (`onCommandSelect`)
 *  and the typed `/xxx` + Enter interception (`onSubmit`) both land here,
 *  so the two entry points can never drift. `focus` is only consumed by
 *  /compact (rest-of-line 定向说明,prd D2). */
async function executeBuiltin(name: string, focus?: string): Promise<void> {
  const sid = chatStore.currentSessionId;
  switch (name) {
    case "help":
      // `/help` reopens the panel with the full list (filter
      // cleared) — no separate help view in PR2.
      cm.commandPaletteOpen.value = true;
      cm.commandFilter.value = "";
      break;
    case "clear":
      if (!sid) return;
      try {
        await chatStore.clearSessionMessages(sid);
      } catch (e) {
        console.error("/clear failed:", e);
      }
      break;
    case "new":
      try {
        await chatStore.createNewSession();
      } catch (e) {
        console.error("/new failed:", e);
      }
      break;
    case "compact": {
      // 08-18-manual-compact-command: idle-time summary compaction.
      // Backend owns the whole gate chain (group-chat / config switch /
      // in-flight / provider); failures come back as user-readable
      // errors and surface via toast (prd R4, never silent). The
      // summary row appears via the same reload path every DB append
      // uses, so MessageItem's compaction_summary rendering applies.
      if (!sid) return;
      // HUD 按 session 隔离:只在发起会话显示,切走即隐藏(见 chat.ts
      // summaryBusyBySession)。用捕获的 busySid 避免切会话后清错键。
      const busySid = sid;
      chatStore.setSummaryBusy(busySid, "正在压缩上下文…");
      try {
        const r = await transport.invoke<ManualCompactionResult>("compact_session", {
          sessionId: busySid,
          focus: focus ?? null,
        });
        projectsStore.showToast(
          `已压缩：${r.tokens_before.toLocaleString()} → ${r.tokens_after.toLocaleString()} tokens`,
          "info",
          6000,
        );
        await chatStore.reloadSessionMessages(busySid);
      } catch (e) {
        console.error("/compact failed:", e);
        projectsStore.showToast(`压缩失败：${extractErrorMessage(e)}`, "error", 6000);
      } finally {
        chatStore.clearSummaryBusy(busySid);
      }
      break;
    }
    case "handoff": {
      // 08-18-handoff-mechanism: full-coverage summary becomes the
      // FIRST context of a NEW child session. Same backend gate chain
      // as /compact; on success refresh the session list and SWITCH to
      // the child — then wait for user input (no auto first turn,
      // prd D2). Failure is zero-side-effect (backend rolls the shell
      // back), so we just stay in the current session + toast.
      if (!sid) return;
      const busySid = sid;
      chatStore.setSummaryBusy(busySid, "正在生成接力摘要…");
      try {
        const r = await transport.invoke<HandoffResult>("handoff_session", {
          sessionId: busySid,
          focus: focus ?? null,
        });
        // List first so switchSession finds the new summary (cwd
        // lookup) without a redundant load_session IPC.
        if (projectsStore.currentProjectId) {
          await chatStore.loadSessions(projectsStore.currentProjectId);
        }
        await chatStore.switchSession(r.new_session_id);
        projectsStore.showToast(
          `已接力到新会话：${r.tokens_before.toLocaleString()} → ${r.tokens_after.toLocaleString()} tokens`,
          "info",
          6000,
        );
      } catch (e) {
        console.error("/handoff failed:", e);
        projectsStore.showToast(`接力失败：${extractErrorMessage(e)}`, "error", 6000);
      } finally {
        chatStore.clearSummaryBusy(busySid);
      }
      break;
    }
    default:
      console.warn("Unknown builtin command:", name);
  }
}

async function onCommandSelect(item: TriggerMenuItem): Promise<void> {
  const slashTok = cm.currentSlashToken();
  if (!slashTok || slashTok.slashOffset < 0) return;
  const { slashOffset, tokenEnd } = slashTok;
  const doc = cm.input.value;
  const beforeToken = doc.slice(0, slashOffset);
  const afterToken = doc.slice(tokenEnd);
  cm.closeCommandPalette();

  if (item.is_builtin || item.source === "builtin") {
    // /compact & /handoff: the text after the slash token is the
    // optional focus — consume it (editor keeps only the pre-token
    // part). Other builtins keep the leftover text in the editor
    // (pre-existing behavior).
    const takesFocus = item.name === "compact" || item.name === "handoff";
    const focus = takesFocus ? afterToken.trim() : "";
    const keepDoc = takesFocus ? beforeToken : beforeToken + afterToken;
    cm.replaceDoc(keepDoc, beforeToken.length);
    await nextTick();
    cm.view.value?.focus();
    await executeBuiltin(item.name, focus || undefined);
    return;
  }

  const isSkill = item.source === "skill";
  if (isSkill) {
    // 2026-06-18 (option 2): skill 选中后 textarea 只留 `/skill-name`
    // （带一个尾空格），不展开 body、不发送。用户可追加自然语言（如
    // `/review-pr 看下 diff`），发送原文后由 agent 通过 use_skill tool 自行
    // 加载 skill body（L1 渐进披露）。尾空格是必须的，否则
    // currentSlashToken 会重新匹配 `/name`，导致面板立即重开。
    const token = `/${item.name} `;
    const inserted = beforeToken + token + afterToken;
    cm.replaceDoc(inserted, beforeToken.length + token.length);
    await nextTick();
    cm.view.value?.focus();
    return;
  }

  // Custom command: fetch body → send as user message (B3 path).
  const projectId = projectsStore.currentProjectId ?? null;
  let body: string | null = null;
  try {
    body = await transport.invoke<string | null>("get_command_body", {
      name: item.name,
      projectId,
    });
  } catch (e) {
    console.error(`get_command_body "/${item.name}" failed:`, e);
    projectsStore.showToast(`命令 /${item.name} 读取失败: ${extractErrorMessage(e)}`, "error");
    return;
  }
  if (!body || !body.trim()) {
    projectsStore.showToast(`命令 /${item.name} 的模板体为空`, "warn");
    return;
  }
  await chatStore.send(body);
}

/** Replace the `@<filter>` token on the current line with `@<relpath>`
 *  and place the caret right after it. Works anywhere on the line
 *  (Cursor-style): we replace the doc span [`atOffset`, `tokenEnd`)
 *  returned by `currentAtToken`. */
async function onFileSelect(item: TriggerMenuItem): Promise<void> {
  const atTok = cm.currentAtToken();
  if (!atTok || atTok.atOffset < 0) return;
  const { atOffset, tokenEnd } = atTok;
  const doc = cm.input.value;
  const beforeAt = doc.slice(0, atOffset);
  const afterToken = doc.slice(tokenEnd);
  const newDoc = beforeAt + `@${item.name}` + afterToken;
  const caret = atOffset + 1 + item.name.length;
  cm.closeFilePalette();
  cm.replaceDoc(newDoc, caret);
  await nextTick();
  cm.view.value?.focus();
}

/** explicit-agent-dispatch (2026-06-30): @@-trigger agent panel
 *  selection. Strips the `@@` token + any partial name, inserts
 *  `@@<name> ` (trailing space positions the caret for the task
 *  text — the rest of the input becomes the forced-dispatch task). */
async function onAgentSelect(item: TriggerMenuItem): Promise<void> {
  const atAtTok = cm.currentAtAtToken();
  if (!atAtTok || atAtTok.atOffset < 0) return;
  const { atOffset, tokenEnd } = atAtTok;
  const doc = cm.input.value;
  const beforeAt = doc.slice(0, atOffset);
  const afterToken = doc.slice(tokenEnd);
  const newDoc = beforeAt + `@@${item.name} ` + afterToken;
  const caret = atOffset + 2 + item.name.length + 1;
  cm.closeAgentPalette();
  cm.replaceDoc(newDoc, caret);
  await nextTick();
  cm.view.value?.focus();
}
</script>

<template>
  <footer class="chat-input" @keydown.escape.prevent="onEscKeydown">
    <!-- B1 (2026-08-16) R2a: paste-image staging strip. Horizontal
         thumbnail row above the input; each cell is a 56px-tall
         thumb with a ✕ remove button. Renders only when the strip
         is non-empty (state lives on the chat store —
         `chatStore.stagedImages`). -->
    <div v-if="chatStore.stagedImages.length > 0" class="chat-input__staged">
      <div
        v-for="(img, idx) in chatStore.stagedImages"
        :key="img.url"
        class="chat-input__staged-item"
      >
        <img
          class="chat-input__staged-thumb"
          :src="img.url"
          :alt="`待发送图片 ${idx + 1}(${img.tokensEst} tokens 估算)`"
        />
        <!-- 08-21-b1-image-followups R1:压缩标注(D3)。title 给出
             原始→结果对照;未压缩的图零变化。 -->
        <span
          v-if="img.compressed"
          class="chat-input__staged-compressed"
          :title="`已压缩:${img.origW}×${img.origH} ${Math.round((img.origBytes ?? 0) / 1024)}KB → ${img.w}×${img.h}`"
        >已压缩</span>
        <button
          type="button"
          class="chat-input__staged-remove btn btn--muted btn--circle"
          aria-label="移除此图片"
          :title="`移除此图片(估算 ${img.tokensEst} tokens)`"
          @click="chatStore.removeStagedImage(idx)"
        >
          ✕
        </button>
      </div>
    </div>
    <div
      class="chat-input__row"
      :class="{ 'chat-input__row--streaming': sending }"
      :style="inputRowStyle"
    >
      <!-- PR2 (B7): per-session Mode picker. Placed on the LEFT
           of the input row (same line as the editor), NOT in
           the hint row, per Q4 P2 in the 2026-06-13 mode-redesign
           grill-with-docs session. -->
      <ModeSelect />
      <!-- W1 (Workflow integration, Step 2.2 + 2026-07-09
           chip-merge): per-session workflow + plugin
           picker merged into a single chip + popover.
           Sibling to `<ModeSelect>` on the same flex row;
           reads as "B7 Mode + W1 Workflow" together form
           the session's configuration surface on the chat
           input row. The component itself owns its
           visibility (only renders when there's a current
           session) and the toggle + plugin IPC round-trips;
           this parent just mounts the tag. The former
           `<WorkflowToggle />` chip was removed in task
           07-09-07-09-workflow-chip-merge. -->
      <PluginSelect />
      <!-- B3 (PR2) + B4 (Stretch 2, 2026-06-18): merged
           command + skill palette. Anchored to the input row
           (position: relative on the row makes it the
           offsetParent); opens UPWARD above the editor when the
           user types `/` at the start of the current line. The
           TriggerMenu component is a reusable skeleton (see its
           top-of-file comment) — B2 (@file) reuses it with a
           different trigger char + data source. The data source
           switched from `list_commands` (B3) to `list_panel_items`
           (B4 Stretch 2) so the same panel surfaces builtins +
           custom commands + skills; the `source` chip on each row
           tells the user which type they're picking.
           `:trigger-el` points at the CM `.cm-editor` DOM node
           (view.dom) so click-to-reposition-caret inside CM
           doesn't close the panel. -->
      <TriggerMenu
        ref="triggerMenu"
        :open="cm.commandPaletteOpen.value"
        :items="cm.commandItems.value"
        :filter="cm.commandFilter.value"
        trigger="/"
        header-label="命令"
        empty-label="无匹配命令"
        :trigger-el="cm.view.value?.dom ?? null"
        @select="onCommandSelect"
        @close="cm.closeCommandPalette"
      />
      <!-- B2 (PR1): @文件 palette. Second <TriggerMenu> caller —
           trigger="@", fuzzysort (fuzzy prop), #row slot renders a
           file icon + relative path. Mutually exclusive with the
           command palette above (a line starts with `/` XOR `@`). -->
      <TriggerMenu
        ref="fileTriggerMenu"
        :open="cm.filePaletteOpen.value"
        :items="cm.panelItems.value"
        :filter="cm.fileFilter.value"
        trigger="@"
        header-label="文件"
        empty-label="无匹配文件"
        fuzzy
        wide
        :max-rows="200"
        :trigger-el="cm.view.value?.dom ?? null"
        @select="onFileSelect"
        @close="cm.closeFilePalette"
      >
        <template #row="{ item }">
          <span class="chat-input__file-row">
            <Icon name="document" :size="12" />
            <code class="chat-input__file-path">{{ item.name }}</code>
          </span>
        </template>
      </TriggerMenu>
      <!-- explicit-agent-dispatch (2026-06-30): @@-trigger agent
           palette. Third <TriggerMenu> caller — trigger="@@",
           sources from `list_subagents` (builtin + user + project).
           Uses the default row slot (not #row) so each row renders
           name + description + the source chip (builtin/user/project)
           via TriggerMenu's built-in chrome. Selecting inserts
           `@@<name> `; the rest of the input becomes the
           forced-dispatch task (parsed by `chat.ts send()`). -->
      <TriggerMenu
        ref="agentTriggerMenu"
        :open="cm.agentPaletteOpen.value"
        :items="cm.agentItems.value"
        :filter="cm.agentFilter.value"
        trigger="@@"
        header-label="Agent"
        empty-label="无匹配 agent"
        :trigger-el="cm.view.value?.dom ?? null"
        @select="onAgentSelect"
        @close="cm.closeAgentPalette"
      />
      <!-- PR1.5: CodeMirror 6 host div. The EditorView mounts into
           this element via the composable's onMounted hook and
           owns all internal DOM (`.cm-editor`, `.cm-scroller`,
           `.cm-content`). Vue MUST NOT render children here —
           CM is the sole owner of the host's subtree. -->
      <div
        ref="host"
        class="chat-input__field"
        :class="{ 'chat-input__field--disabled': sendingEffective }"
        :aria-disabled="sendingEffective ? 'true' : undefined"
      />
      <!-- PR5 + F1 (2026-08-25) 单按钮三态:同一位置按状态变形 ——
           有草稿 → 发送键(流式中发送 = 排队,AC1);无草稿 + 流式 →
           Stop(空输入也可打断长流);无草稿 + 空闲 → 发送键 disabled。
           群聊流式中输入框已锁无草稿,自然落到 Stop(cancel+resend 抢占,
           AC4);`showStop`/`sendDisabled` 判定见 script 侧 F1 注释。 -->
      <button
        v-if="showStop"
        class="chat-input__action chat-input__stop btn btn--danger btn--circle"
        aria-label="停止生成"
        @click="onStop"
      >
        <span class="chat-input__stop-glyph" aria-hidden="true"></span>
      </button>
      <button
        v-else
        class="chat-input__action chat-input__send btn btn--primary btn--circle"
        :disabled="sendDisabled()"
        aria-label="发送"
        @click="cm.submit"
      >
        <Icon name="arrow-up" :size="16" />
      </button>
    </div>
    <!-- Hint row: latency chip + token usage chip + ModelSelect.
         Extracted into a self-contained sub-component
         (`ChatInputHintRow.vue`) — 0 store import, props-only. -->
    <ChatInputHintRow
      :token-usage="chatStore.currentSessionTokenUsage"
      :context-window="currentModelContextWindow"
      :usage-level="usageLevel"
      :current-session-id="chatStore.currentSessionId"
      :total-ms="chatStore.currentSessionLatencyTotal"
      :turns="chatStore.currentSessionLatencyTurns"
    />
  </footer>
</template>

<style scoped>
.chat-input {
  padding: 12px 20px 16px;
  background: var(--color-bg-app);
  flex-shrink: 0;
}

/* B1 R2a: staging strip — horizontal scrollable thumbnail row above
   the input row. 56px cells (thumb 48px + a little chrome), ✕ badge
   overlapping the top-right corner of each cell. Zero new
   dependencies: plain CSS + the store's objectURLs. */
.chat-input__staged {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-bottom: 8px;
  padding: 4px 2px;
  overflow-x: auto;
  scrollbar-width: thin;
}

.chat-input__staged-item {
  position: relative;
  flex-shrink: 0;
  width: 56px;
  height: 56px;
}

.chat-input__staged-thumb {
  width: 100%;
  height: 100%;
  object-fit: cover;
  border-radius: var(--radius-md);
  border: 1px solid var(--color-bg-border);
  display: block;
  background: var(--color-bg-elevated);
}

/* 08-24 btn-family:18px 圆形删除钮,本体由 muted·circle 家族承载;
   本地保留定位/固定几何 + 裸 10px 字号落 --text-2xs token +
   hover 红字(删除语义,家族 muted hover 是 accent 转向不匹配)。 */
.chat-input__staged-remove {
  position: absolute;
  top: -6px;
  right: -6px;
  width: 18px;
  height: 18px;
  padding: 0;
  line-height: 1;
  font-size: var(--text-2xs);
}

/* 08-21-b1-image-followups R1:压缩标注 —— 缩略图左下角小 chip。
   (非按钮元素,不迁 .btn;裸 10px 字号落 --text-2xs token。) */
.chat-input__staged-compressed {
  position: absolute;
  left: 2px;
  bottom: 2px;
  padding: 0 4px;
  border-radius: var(--radius-sm);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  color: var(--color-text-secondary);
  font-size: var(--text-2xs);
  line-height: 14px;
  pointer-events: none;
  user-select: none;
}

.chat-input__staged-remove:hover:not(:disabled) {
  color: var(--color-tool-error-text, #e5484d);
  background: var(--color-bg-app);
}

.chat-input__row {
  position: relative;
  display: flex;
  align-items: flex-end;
  gap: 8px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-xl);
  padding: 6px 6px 6px 14px;
  transition: border-color var(--duration-base) var(--ease-out), box-shadow var(--duration-base) var(--ease-out);
}

.chat-input__row:focus-within {
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}

/* SSE streaming ring (2026-08-21): while `sending`, the input row's
   border is replaced by a slowly rotating gradient ring (accent blue →
   cyan → violet, all existing tokens — no new --color-*), plus a soft
   breathing glow. The row is the only element guaranteed visible for
   the whole stream (the message list scrolls), and the border is the
   slot the idle/focus states already use, so streaming reads as the
   third state in idle → focused → working rather than a new visual
   language. Technique: ::before ring via the canonical mask-composite
   gradient-border recipe; the conic angle animates through a
   registered @property (WebKitGTK 2.44+/WebView2 both support it —
   older engines degrade to a static gradient ring at 0deg).
   `--duration-pulse * 2` (3.6s/rev) is deliberately slower than the
   1.8s subagent breathing so the two never beat against each other. */
@property --chat-input-stream-angle {
  syntax: "<angle>";
  initial-value: 0deg;
  inherits: false;
}

.chat-input__row--streaming,
.chat-input__row--streaming:focus-within {
  /* Hide the static border + focus ring — the animated ::before ring
     owns the border visual while streaming. box-shadow is driven by
     the glow animation below (animations override the static
     --shadow-ring). */
  border-color: transparent;
  animation: chat-input-stream-glow var(--duration-pulse) ease-in-out infinite;
}

.chat-input__row--streaming::before {
  content: "";
  position: absolute;
  inset: -1px;
  border-radius: inherit;
  padding: 1px;
  background: conic-gradient(
    from var(--chat-input-stream-angle, 0deg),
    var(--color-accent-text),
    var(--color-tool-read) 33%,
    var(--color-tool-thinking) 66%,
    var(--color-accent-text)
  );
  /* Ring mask: subtract the padding-box rect from the content-box
     rect, leaving a 1px frame. #fff is structural (any opaque color
     works), not a visual color. */
  -webkit-mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  -webkit-mask-composite: xor;
  mask:
    linear-gradient(#fff 0 0) content-box,
    linear-gradient(#fff 0 0);
  mask-composite: exclude;
  pointer-events: none;
  animation: chat-input-stream-rotate calc(var(--duration-pulse) * 2) linear infinite;
}

@keyframes chat-input-stream-rotate {
  to {
    --chat-input-stream-angle: 360deg;
  }
}

@keyframes chat-input-stream-glow {
  0%,
  100% {
    box-shadow: 0 0 10px color-mix(in srgb, var(--color-accent) 10%, transparent);
  }
  50% {
    box-shadow: 0 0 16px color-mix(in srgb, var(--color-accent) 22%, transparent);
  }
}

@media (prefers-reduced-motion: reduce) {
  .chat-input__row--streaming,
  .chat-input__row--streaming:focus-within,
  .chat-input__row--streaming::before {
    animation: none;
  }
}

/* PR1.5: CodeMirror 6 host. The EditorView creates `.cm-editor`
   inside this div; we style it through `:deep()` because CM
   injects its own DOM (scoped CSS `data-v-xxx` doesn't apply to
   imperative children — same gotcha as reka-ui portal children,
   see `.trellis/spec/frontend/reka-ui-usage.md`). Visual contract
   matches the old `<textarea>`: flex:1 to fill the row, 14px sans
   body, 6/0 vertical/horizontal padding, max-height 200px with
   internal scroller. */
.chat-input__field {
  flex: 1;
  min-width: 0;
  min-height: 28px;
  display: flex;
  flex-direction: column;
  justify-content: center;
}

:deep(.chat-input__field .cm-editor) {
  background: transparent;
  color: var(--color-text-primary);
  font-family: var(--font-sans);
  font-size: var(--text-md);
  line-height: 1.5;
  max-height: 200px;
}

:deep(.chat-input__field .cm-editor .cm-scroller) {
  font-family: inherit;
  overflow: auto;
  padding: 6px 0;
}

:deep(.chat-input__field .cm-editor .cm-content) {
  padding: 0;
  caret-color: var(--color-text-primary);
}

:deep(.chat-input__field .cm-editor.cm-focused) {
  /* CodeMirror 自绘光标;外层 .chat-input__field 已有 focus-within --shadow-ring */
  outline: none;
}

:deep(.chat-input__field .cm-editor .cm-cursor) {
  border-left-color: var(--color-text-primary);
}

:deep(.chat-input__field .cm-editor .cm-placeholder) {
  color: var(--color-text-muted);
}

.chat-input__field--disabled {
  cursor: not-allowed;
}

:deep(.chat-input__field--disabled .cm-editor) {
  color: var(--color-text-muted);
}

:deep(.chat-input__field--disabled .cm-editor .cm-content) {
  caret-color: var(--color-text-muted);
}

/* PR1.5 PR-B: token coloring. The marks are added by the
   `tokenHighlightPlugin` in chatInputTokens.ts as CSS classes on
   inline `<span>`s inside `.cm-content`. Colors reuse existing
   design tokens (design-tokens.md: "Don't add a new `--color-*`
   token for a one-off use"). */
:deep(.chat-input__field .cm-editor .cm-content .cm-token-command) {
  color: var(--color-accent-text);
  font-weight: var(--weight-semibold);
}

:deep(.chat-input__field .cm-editor .cm-content .cm-token-file) {
  color: var(--color-tool-read);
  font-weight: var(--weight-semibold);
}

:deep(.chat-input__field .cm-editor .cm-content .cm-token-skill) {
  color: var(--color-tool-thinking);
  font-weight: var(--weight-semibold);
}

/* explicit-agent-dispatch (2026-06-30): @@agent token. Uses the
   thinking color (violet) — the agent dispatch is a directive layer
   akin to skills, distinct from file (read cyan) + command (accent). */
:deep(.chat-input__field .cm-editor .cm-content .cm-token-agent) {
  color: var(--color-tool-thinking);
  font-weight: var(--weight-semibold);
}

/* Shared shape for both the Send and Stop action buttons. PR5
   factored the common width/height/border-radius/padding out of
   the old `.chat-input__send` rule so the new Stop variant can
   reuse it without duplicating pixel values.
   08-24 btn-family:本体由 primary·circle(send)/ danger·circle(stop)
   家族承载;本地仅保留 32px 固定几何 + opacity 进 transition(输入行
   disabled 渐隐,家族 transition 未含 opacity)。 */
.chat-input__action {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  padding: 0;
  transition: background var(--duration-base) var(--ease-out), opacity var(--duration-base) var(--ease-out);
}

.chat-input__send:disabled {
  background: var(--color-bg-elevated);
  color: var(--color-text-muted);
}

/* PR5 Stop button. Uses a different background so the visual cue
   "this will halt the stream" is unambiguous, and the square
   glyph differentiates it from the up-arrow Send icon.
   2026-08-21: slow red halo breathing (same --duration-pulse rhythm
   as the subagent breathing + input-row glow) so the button reads
   alive while streaming. Deliberately NOT part of the gradient ring
   treatment — red is the semantic "stop" color and must stay pure.
   08-24 btn-family:tool-error 实底由 danger 家族承载;红晕呼吸动画
   本地保留;原 80%+#000 混色 hover 删,落家族 brightness(1.1)。 */
.chat-input__stop {
  animation: chat-input-stop-breathe var(--duration-pulse) ease-in-out infinite;
}

@keyframes chat-input-stop-breathe {
  0%,
  100% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--color-tool-error) 0%, transparent);
  }
  50% {
    box-shadow: 0 0 10px 1px color-mix(in srgb, var(--color-tool-error) 40%, transparent);
  }
}

@media (prefers-reduced-motion: reduce) {
  .chat-input__stop {
    animation: none;
  }
}

.chat-input__stop-glyph {
  display: block;
  width: 10px;
  height: 10px;
  background: #ffffff;
  border-radius: 2px;
}

/* .chat-input__spinner/@keyframes chat-input-spin 已删
   (08-23-spinner-skeleton-primitive):样式存在但模板零引用(grep 实证),
   属遗留死代码;全局原语在 style.css(.app-spinner/.icon-spin)。 */

/* B2 @文件 palette row (rendered via <TriggerMenu>'s #row slot). The
   slot content is parent-scoped, so these rules live here (not in
   TriggerMenu.vue). Occupies the full row width (the panel's grid is
   `1fr auto`; a file row has no meta column). Monospace path + ellipsis
   for long relative paths; the document icon matches the read_file
   tool family visually. */
.chat-input__file-row {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  grid-column: 1 / -1;
  color: var(--color-text-secondary);
}

.chat-input__file-path {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  /* 2026-06-18: 长相对路径看不到文件名 —— 让 <code> 在 inline-flex 父里可
     收缩（min-width:0 + flex），并从左侧省略。direction:rtl 把 ellipsis
     落到视觉左侧、内容右对齐，于是溢出时保留尾部文件名 + 近端目录段。
     unicode-bidi:isolate 让纯 ASCII 路径整体当 LTR run，字符顺序不变。 */
  min-width: 0;
  flex: 1 1 auto;
  direction: rtl;
  unicode-bidi: isolate;
}

.chat-input__file-row :deep(svg) {
  flex: 0 0 auto;
}
/* S5 移动端 ChatInput 适配(08-11-mobile-adaptation, Step 6)。桌面样式
   块零改动(全在 @media max-width:767px 内)。 */
@media (max-width: 767px) {
  /* safe-area:底部 padding 叠加 Home Indicator 高度(env() 桌面=0 无害)。
     原 padding 12px 20px 16px 的 16px 底部保留,加 safe-area-bottom。 */
  .chat-input {
    padding-bottom: calc(16px + var(--safe-area-bottom));
  }
  /* iOS Safari 字号 <16px 触发整页自动缩放。CodeMirror host 必须 16px。 */
  :deep(.chat-input__field .cm-editor) {
    font-size: 16px;
  }
  /* send / stop 按钮触摸目标放大(Apple HIG 44px)。原 32×32 桌面不动。 */
  .chat-input__action {
    width: 44px;
    height: 44px;
  }
}

/* S6a 底部输入区移动端适配(08-13-mobile-chat-view)+ 08-14 ux-polish-r1
   WP1 1.4(评审 A4:Edit/wf chips 挤压输入区)。
   重排:移动端 row 切 grid,ModeSelect / PluginSelect 两 chip 移到
   编辑框上方独立一行(桌面 flex 同行布局零改动)。
   - grid 三列 auto/1fr/auto:chips 行 = col1+col2(start 对齐、随内容宽),
     编辑框 = r2 c1/c3 跨两列,发送/停止按钮 = r2 c3(align-self:end 与
     桌面 flex-end 对齐习惯一致)。
   - chips 行间距用 margin-bottom(而非 row-gap):无 session 时两 chip 根
     节点 v-if 不渲染,但 field 显式落在行 2 仍会让 grid 建出行 1(空行,
     高度 0)—— 任何 row-gap 都会在编辑框上方垫出幽灵行距;margin 只挂在
     chip 上,不渲染即不产生。
   - 横向间距走 column-gap 8px(与桌面 flex gap 同值):编辑框(r2 跨
     c1-c2)与发送/停止按钮(r2 c3)之间、chips 行 c1/c2 之间都由它提供
     (plugin-select 无需再 margin-left)。
   - TriggerMenu 关闭时 v-if 无 DOM,打开时 position:absolute 脱离
     grid 流,不占 cell,锚定行为不受影响(row 仍是 offsetParent)。
   - chip 保持 32px 高紧凑档(DEC-6:chip 不拉 44px,上移只是为了把
     编辑框让出来);CM 16px 是 iOS 防缩放底线,发送/停止 44px 是主操作
   底线,均不动。 */
@media (max-width: 767px) {
  .chat-input__row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: end;
    /* row-gap 归零(空 grid 行 1 也会垫出幽灵行距,见上方注释;行间距由
       chips 的 margin-bottom 提供),column-gap 8px 承担全部横向间距 ——
       修复:gap 整体归零时编辑框与发送按钮 0px 贴死(桌面/S6a 是 8/6px)。
       padding 沿用 S6a 的移动端收窄值。 */
    row-gap: 0;
    column-gap: var(--space-2);
    padding: 6px 6px 6px 10px;
  }
  :deep(.mode-select) {
    grid-area: 1 / 1;
    margin-bottom: var(--space-2);
  }
  :deep(.plugin-select) {
    grid-area: 1 / 2;
    justify-self: start;
    margin-bottom: var(--space-2);
  }
  .chat-input__field {
    grid-area: 2 / 1 / 3 / 3;
  }
  .chat-input__action {
    grid-area: 2 / 3;
    align-self: end;
  }
  :deep(.mode-select__trigger),
  :deep(.plugin-select__chip) {
    /* 真机迭代(2026-08-13):两 chip 固定高 32px + padding 归零,高度一致
       (桌面靠 3px 8px 内边距撑高,高度 ~23px 参差)。字号保持 13px
       (--text-base)。 */
    height: 32px;
    padding-left: 8px;
    padding-right: 8px;
    padding-top: 0;
    padding-bottom: 0;
  }
  /* 占位文案窄屏不换行(ellipsis 截断),避免 D6 里占位换行的怪观感。 */
  :deep(.chat-input__field .cm-editor .cm-placeholder) {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
}
</style>
