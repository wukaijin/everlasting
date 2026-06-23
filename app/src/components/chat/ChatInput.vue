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

import { computed, nextTick, ref, watchEffect } from "vue";
import { invoke } from "@tauri-apps/api/core";
import Icon from "../Icon.vue";
import ModeSelect from "./ModeSelect.vue";
import TriggerMenu, { type TriggerMenuItem } from "./TriggerMenu.vue";
import ChatInputHintRow from "./ChatInputHintRow.vue";
import { useChatInputCodeMirror } from "../../utils/chatInputCodeMirror";
import { useChatStore } from "../../stores/chat";
import { MODE_CYCLE, type SessionMode } from "../../stores/chat.types";
import { useModelsStore } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";
import { colorTagHex, hexToRgba } from "../../utils/colorTag";
import { registerShiftTabCycle } from "../../utils/useKeyboard";

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
  send: [text: string];
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
  const pct = u.input_tokens / currentModelContextWindow.value;
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

const cm = useChatInputCodeMirror({
  host,
  sending: computed(() => props.sending),
  placeholder: computed(() => props.placeholder),
  onSubmit: () => {
    const text = cm.input.value;
    if (!text.trim() || props.sending) return;
    const v = cm.view.value;
    if (v) {
      const cur = v.state.doc.toString();
      if (cur.length > 0) {
        v.dispatch({ changes: { from: 0, to: cur.length, insert: "" } });
      }
    } else {
      cm.input.value = "";
    }
    emit("send", text);
  },
  commandItemsSource: async (): Promise<TriggerMenuItem[]> => {
    const projectId = projectsStore.currentProjectId;
    try {
      const list = await invoke<PanelItem[]>("list_panel_items", {
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
  fileItemsSource: async (): Promise<TriggerMenuItem[]> => {
    const projectId = projectsStore.currentProjectId;
    try {
      const paths = await invoke<string[]>("list_files", {
        projectId: projectId ?? null,
      });
      return paths.map((p) => ({ key: p, name: p }));
    } catch (e) {
      console.error("list_files failed:", e);
      return [];
    }
  },
});

// === TriggerMenu ref bindings ====================================
//
// The composable needs to call `moveActive` / `confirmActive` on the
// `<TriggerMenu>` instances when the user presses arrow / Tab / Enter.
// We bind them via the standard Vue template-ref pattern; the
// composable reads `commandMenuRef` / `fileMenuRef` reactively.

const triggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
const fileTriggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);

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

// === Send / Stop / Esc ===========================================

function onStop() {
  emit("stop");
}

const sendDisabled = (): boolean => props.sending || !cm.input.value.trim();

function onEscKeydown() {
  if (props.sending) {
    onStop();
  }
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
async function onCommandSelect(item: TriggerMenuItem): Promise<void> {
  const slashTok = cm.currentSlashToken();
  if (!slashTok || slashTok.slashOffset < 0) return;
  const { slashOffset, tokenEnd } = slashTok;
  const doc = cm.input.value;
  const beforeToken = doc.slice(0, slashOffset);
  const afterToken = doc.slice(tokenEnd);
  cm.closeCommandPalette();

  const sid = chatStore.currentSessionId;

  if (item.is_builtin || item.source === "builtin") {
    cm.replaceDoc(beforeToken + afterToken, beforeToken.length);
    await nextTick();
    cm.view.value?.focus();
    switch (item.name) {
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
      default:
        console.warn("Unknown builtin command:", item.name);
    }
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
    body = await invoke<string | null>("get_command_body", {
      name: item.name,
      projectId,
    });
  } catch (e) {
    console.error(`get_command_body "/${item.name}" failed:`, e);
    projectsStore.showToast(`命令 /${item.name} 读取失败: ${String(e)}`, "error");
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
</script>

<template>
  <footer class="chat-input" @keydown.escape.prevent="onEscKeydown">
    <div class="chat-input__row" :style="inputRowStyle">
      <!-- PR2 (B7): per-session Mode picker. Placed on the LEFT
           of the input row (same line as the editor), NOT in
           the hint row, per Q4 P2 in the 2026-06-13 mode-redesign
           grill-with-docs session. -->
      <ModeSelect />
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
        :items="cm.fileItems.value"
        :filter="cm.fileFilter.value"
        trigger="@"
        header-label="文件"
        empty-label="无匹配文件"
        fuzzy
        wide
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
      <!-- PR1.5: CodeMirror 6 host div. The EditorView mounts into
           this element via the composable's onMounted hook and
           owns all internal DOM (`.cm-editor`, `.cm-scroller`,
           `.cm-content`). Vue MUST NOT render children here —
           CM is the sole owner of the host's subtree. -->
      <div
        ref="host"
        class="chat-input__field"
        :class="{ 'chat-input__field--disabled': sending }"
        :aria-disabled="sending ? 'true' : undefined"
      />
      <!-- PR5: morph the send button into a Stop button while
           `sending` is true. The button is always enabled (even
           when the input is empty) so the user can interrupt a
           long stream with no draft. -->
      <button
        v-if="sending"
        class="chat-input__action chat-input__stop"
        aria-label="停止生成"
        @click="onStop"
      >
        <span class="chat-input__stop-glyph" aria-hidden="true"></span>
      </button>
      <button
        v-else
        class="chat-input__action chat-input__send"
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

.chat-input__row {
  position: relative;
  display: flex;
  align-items: flex-end;
  gap: 8px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 12px;
  padding: 6px 6px 6px 14px;
  transition: border-color 0.15s, box-shadow 0.15s;
}

.chat-input__row:focus-within {
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
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
  font-size: 14px;
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
  color: var(--color-accent);
  font-weight: 600;
}

:deep(.chat-input__field .cm-editor .cm-content .cm-token-file) {
  color: var(--color-tool-read);
  font-weight: 600;
}

:deep(.chat-input__field .cm-editor .cm-content .cm-token-skill) {
  color: var(--color-tool-thinking);
  font-weight: 600;
}

/* Shared shape for both the Send and Stop action buttons. PR5
   factored the common width/height/border-radius/padding out of
   the old `.chat-input__send` rule so the new Stop variant can
   reuse it without duplicating pixel values. */
.chat-input__action {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  border-radius: 50%;
  border: none;
  background: var(--color-accent);
  color: #ffffff;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  font-family: inherit;
  padding: 0;
  transition: background 0.15s, opacity 0.15s;
}

.chat-input__send:hover:not(:disabled) {
  background: var(--color-accent-hover);
}

.chat-input__send:disabled {
  background: var(--color-bg-elevated);
  color: var(--color-text-muted);
  cursor: not-allowed;
  opacity: 0.6;
}

/* PR5 Stop button. Uses a different background so the visual cue
   "this will halt the stream" is unambiguous, and the square
   glyph differentiates it from the up-arrow Send icon. */
.chat-input__stop {
  background: var(--color-tool-error);
}

.chat-input__stop:hover {
  background: color-mix(in srgb, var(--color-tool-error) 80%, #000 20%);
}

.chat-input__stop-glyph {
  display: block;
  width: 10px;
  height: 10px;
  background: #ffffff;
  border-radius: 2px;
}

.chat-input__spinner {
  animation: chat-input-spin 1s linear infinite;
}

@keyframes chat-input-spin {
  to {
    transform: rotate(360deg);
  }
}

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
  font-size: 12px;
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
</style>
