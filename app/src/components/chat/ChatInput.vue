<script setup lang="ts">
// ChatInput — chat composer. Single-line textarea (auto-grows up to
// ~200px) + a circular Prussian-blue send button on the right, with
// a small hint row below. Matches the spike-003 reference layout
// (ui-A.png).
//
// IME-safe Enter-to-send: during composition (中文输入法 candidate
// selection) Enter must NOT submit, otherwise typing "你好" can blast
// an unfinished candidate into the model. Same composition gate as
// before.
//
// The component is "dumb" with respect to the chat model — it emits
// `send` with the trimmed text and lets the parent (ChatPanel) decide
// whether to actually call `store.send` (e.g. guard on `sending`,
// project, etc.).
//
// PR5: when `sending` is true, the right-side send button morphs into
// a Stop button. Clicking it emits `stop`; the parent calls
// `chatStore.cancel()`. The disabled-while-streaming state of the
// input itself is unchanged — the user can still see what's being
// streamed; they just can't type a new message until the stream ends
// (or they hit Stop and the stream bails out).
//
// Hint row layout (F5 follow-up):
// - LEFT: LLM cumulative chip (clock icon + "Σ 1.2s" / "—") backed
//   by a CLICKABLE popover that breaks the running total into a
//   per-turn TTFB / Gen / Total list. Replaces the old
//   "⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令" text — the keyboard
//   hints are still documented here in the comment block but the
//   the on-screen real estate now goes to the latency summary (which
//   is useful during streaming, whereas the keyboard hint never
//   changed and just ate horizontal space).
// - CENTER: per-session token usage chip (reka-ui Tooltip on hover,
//   "14.2K · 7% / 200K" with green/yellow/red thresholds and a 4-row
//   breakdown tooltip). Unchanged from A4.
// - RIGHT: model picker popover (ModelSelect, opens UP). Unchanged.
//
// A4 (Token Usage Tracking): the hint row's center token-usage chip
// keeps its 50%/75% color thresholds and the "升级前未统计" fallback
// for pre-A4 sessions (the four columns are NULL). Brand-new sessions
// before their first LLM turn render as "—".
//
// F5 (LLM Latency Tracking) follow-up: the left chip renders "—"
// for pre-F5 / brand-new sessions (currentSessionLatencyTotal ===
// null). For sessions with at least one recorded turn, it shows
// the cumulative Σ totalMs formatted via `abbreviateDuration`. The
// popover (click-triggered, NOT hover) shows the per-turn list
// (TTFB / Gen / Total per assistant message) plus a header with
// 累计 / 轮次 / 平均 three rows. Pre-F5 / no-records sessions
// show the three rows as "—" / 0 / "—" with the "本次 session
// 还没有 LLM 耗时数据" empty footer. Click-outside / Esc closes
// the popover. The popover is hand-written (ModelSelect style)
// instead of reka-ui's `PopoverRoot` because (a) we already have
// the hand-written pattern in the codebase, (b) the layout needs
// a scrollable list with a sticky header, and (c) the reka-ui
// `PopoverRoot` would require an extra import for one user.

import { computed, nextTick, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { TooltipProvider, TooltipRoot, TooltipTrigger, TooltipPortal, TooltipContent, TooltipArrow } from "reka-ui";
import Icon from "../Icon.vue";
import ModelSelect from "./ModelSelect.vue";
import ModeSelect from "./ModeSelect.vue";
import TriggerMenu, { type TriggerMenuItem } from "./TriggerMenu.vue";
import { useChatStore, MODE_CYCLE, type SessionMode } from "../../stores/chat";
import { useModelsStore } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { abbreviateTokens, tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";
import { abbreviateDuration } from "../../utils/duration";
import { colorTagHex, hexToRgba } from "../../utils/colorTag";
import { registerShiftTabCycle } from "../../utils/useKeyboard";

/** B3 `/command` palette (PR2): wire DTO from the Rust
 *  `resource_loader::CommandInfo`. Field names are snake_case to
 *  mirror the Rust struct (BACKLOG §5.2 — TS interface mirrors
 *  Rust verbatim; Tauri command ARGS use camelCase per
 *  HACKING-wsl FU-4, so the `invoke` call below uses `projectId`). */
interface CommandInfo {
  name: string;
  description: string;
  argument_hint: string | null;
  source: string;
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

const input = ref("");
const isComposing = ref(false);
const textareaEl = ref<HTMLTextAreaElement | null>(null);

// A4: per-session token usage — read from the chat store's
// reactive `currentSessionTokenUsage`. The model store provides
// the context window for the percentage denominator. We only
// need the default model's context_window; the model picker
// popover already exposes the selected model and updates this
// value on switch.
const chatStore = useChatStore();
const modelsStore = useModelsStore();
const projectsStore = useProjectsStore();

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

// -----------------------------------------------------------------------
// F5 follow-up: LLM cumulative latency summary chip + clickable popover.
// Mirrors the ModelSelect hand-written popover pattern (open/close
// ref, click-outside + Esc handlers). The chip itself is just a
// clock icon + "Σ 1.2s" label; clicking it opens the popover with
// the per-turn breakdown. The trigger is hidden when no session is
// active (matches the A4 token-usage chip's "no session → don't
// render" rule).
// -----------------------------------------------------------------------

const latencyPopoverOpen = ref(false);
const latencyPopoverRoot = ref<HTMLElement | null>(null);

function toggleLatencyPopover() {
  latencyPopoverOpen.value = !latencyPopoverOpen.value;
}

/** Click outside the latency popover root closes it. Mirrors
 *  `ModelSelect.onDocumentClick` and the worktree dropdown's
 *  pattern. */
function onDocumentClick(e: MouseEvent) {
  if (!latencyPopoverOpen.value) return;
  const target = e.target as Node | null;
  if (
    latencyPopoverRoot.value &&
    target &&
    !latencyPopoverRoot.value.contains(target)
  ) {
    latencyPopoverOpen.value = false;
  }
}

/** Esc closes the latency popover. Bound on `window` because
 *  the trigger button may not have focus when the popover is
 *  open. Same pattern as ModelSelect. */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape" && latencyPopoverOpen.value) {
    latencyPopoverOpen.value = false;
  }
}

if (typeof document !== "undefined") {
  document.addEventListener("click", onDocumentClick);
}
if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
}
onUnmounted(() => {
  if (typeof document !== "undefined") {
    document.removeEventListener("click", onDocumentClick);
  }
  if (typeof window !== "undefined") {
    window.removeEventListener("keydown", onKeyDown);
  }
});

/** Per-turn latency list for the popover breakdown. `null` →
 *  no session active (chip hidden). `[]` → active session but
 *  no turns recorded yet (chip renders "—"). Non-empty → the
 *  popover renders a row per turn. */
const latencyTurns = computed(() => chatStore.currentSessionLatencyTurns);

/** Average totalMs across recorded turns. Computed live from
 *  the per-turn list (no separate counter needed). Returns
 *  `null` when no turns have been recorded. */
const latencyAverage = computed<number | null>(() => {
  const t = latencyTurns.value;
  if (!t || t.length === 0) return null;
  let sum = 0;
  let count = 0;
  for (const x of t) {
    if (typeof x.totalMs === "number") {
      sum += x.totalMs;
      count++;
    }
  }
  return count > 0 ? sum / count : null;
});

/** Auto-grow: reset height so the field shrinks when content is
 *  deleted, then size to scrollHeight (capped via CSS max-height). */
function autosize() {
  const el = textareaEl.value;
  if (!el) return;
  el.style.height = "auto";
  el.style.height = `${el.scrollHeight}px`;
}

function onTextareaInput(e: Event) {
  if (isComposing.value) return;
  input.value = (e.target as HTMLTextAreaElement).value;
  autosize();
  // B3: re-evaluate the command-palette trigger on every input
  //  (open when the current line becomes `/foo`, close when the
  //  user types past the command-name region).
  syncCommandPalette();
}

function onCompositionStart() {
  isComposing.value = true;
}

function onCompositionEnd(e: CompositionEvent) {
  isComposing.value = false;
  input.value = (e.target as HTMLTextAreaElement).value;
  autosize();
  // B3: an IME commit may have inserted a `/` (or removed it);
  // re-evaluate the trigger state now that composition is over.
  syncCommandPalette();
}

function onKeydown(e: KeyboardEvent) {
  // B3: when the command palette is open, ArrowUp / ArrowDown /
  //  Enter / Escape belong to the palette, not the textarea.
  //  Enter MUST NOT submit while the palette is open (otherwise
  //  selecting `clear` would also send `/clear` as a chat
  //  message). We intercept here, before the existing
  //  Enter-to-submit branch below.
  if (commandPaletteOpen.value) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      triggerMenu.value?.moveActive(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      triggerMenu.value?.moveActive(-1);
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
      e.preventDefault();
      triggerMenu.value?.confirmActive();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      closeCommandPalette();
      return;
    }
  }
  if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
    e.preventDefault();
    submit();
  }
}

function onSubmit() {
  submit();
}

function onStop() {
  emit("stop");
}

/**
 * PR2 (B7): Shift+Tab cycle through the per-session Mode.
 *
 * Wired via the `useKeyboard` module so the listener lives at
 * the capture phase on `window` — the default browser
 * behaviour (reverse-tab focus traversal) MUST be suppressed
 * with `e.preventDefault()`, which a per-component listener
 * on the textarea can't reliably do once focus has moved
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

// -----------------------------------------------------------------------
// B3 `/command` palette (PR2).
//
// The TriggerMenu is a reusable skeleton (TriggerMenu.vue); this
// block owns the B3-specific wiring:
//   - detection: open the panel when the textarea's CURRENT LINE
//     starts with `/` AND the line is otherwise empty (matches
//     Claude Code's "type / to see commands" UX). Multi-line
//     drafts only look at the cursor's line, so `/help` on line 2
//     of a draft still triggers.
//   - IME safety: never open during composition (Chinese IME
//     candidates include `/` as a literal char in some schemas).
//   - data source: lazy `list_commands` IPC the first time the
//     panel opens in a session; the backend's mtime-fence cache
//     makes subsequent opens free.
//   - keyboard routing: when the panel is open, ArrowUp / ArrowDown
//     / Enter / Escape are intercepted HERE (before the textarea's
//     own Enter handler) and routed to the TriggerMenu. Enter no
//     longer submits while the panel is open.
//   - dispatch: builtins (`/help` `/clear` `/new`) run client-side
//     (no LLM round-trip); custom commands are PR3 territory —
//     PR2 leaves a console breadcrumb so the wiring is obvious.
// -----------------------------------------------------------------------

const triggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
const commandPaletteOpen = ref(false);
const commandItems = ref<TriggerMenuItem[]>([]);
/** Text the user typed AFTER the `/`. Empty string = "show all".
 *  Computed from the textarea's current line on every input. */
const commandFilter = ref("");
/** Marker so we don't refetch `list_commands` on every keystroke.
 *  Cleared when the panel closes (so a future edit to a command
 *  file is picked up on the next open — the backend's mtime fence
 *  makes the IPC cheap anyway, but the round-trip itself is not
 *  free). */
let commandsLoaded = false;

/** The cursor's current line in the textarea. Used for trigger
 *  detection (must START with `/` and be otherwise empty) and
 *  for extracting the filter text. Splits on `\n` and indexes by
 *  `selectionStart`. */
function currentLineInfo(): { line: string; lineStart: number } {
  const el = textareaEl.value;
  if (!el) return { line: "", lineStart: 0 };
  const pos = el.selectionStart ?? input.value.length;
  const upto = input.value.slice(0, pos);
  const lineStart = upto.lastIndexOf("\n") + 1;
  const lineEnd = input.value.indexOf("\n", pos);
  const line =
    lineEnd === -1
      ? input.value.slice(lineStart)
      : input.value.slice(lineStart, lineEnd);
  return { line, lineStart };
}

/** True when the cursor's current line is exactly `/` optionally
 *  followed by command-name characters ([a-z0-9_-]). The "starts
 *  with `/` AND nothing else on the line beyond command chars"
 *  rule matches Claude Code: typing `/he` opens the panel filtered
 *  to `help`; typing `/hello world` (space) closes it because the
 *  line is no longer a command-name shape. */
function detectCommandTrigger(): { trigger: boolean; filter: string } {
  const { line } = currentLineInfo();
  const trimmed = line.trimStart();
  if (!trimmed.startsWith("/")) return { trigger: false, filter: "" };
  // After the `/`, allow command-name chars only. Any space or
  // punctuation means the user moved on from autocomplete.
  const rest = trimmed.slice(1);
  if (rest.length > 0 && !/^[a-zA-Z0-9_-]+$/.test(rest)) {
    return { trigger: false, filter: "" };
  }
  return { trigger: true, filter: rest };
}

/** Fetch the command list (builtin + user + project) from the
 *  backend. The backend's `AppState.command_cache` is mtime-fenced
 *  so this is a cheap read after the first scan. We map the wire
 *  `CommandInfo` to the panel's `TriggerMenuItem` here so the
 *  panel stays data-source-agnostic (B2 will source from a file
 *  walker, not this function). */
async function loadCommands(): Promise<void> {
  if (commandsLoaded) return;
  const projectId = projectsStore.currentProjectId;
  try {
    const list = await invoke<CommandInfo[]>("list_commands", {
      projectId: projectId ?? null,
    });
    commandItems.value = list.map((c) => ({
      key: `${c.source}:${c.name}`,
      name: c.name,
      description: c.description || undefined,
      argument_hint: c.argument_hint ?? undefined,
      source: c.source,
      is_builtin: c.is_builtin,
    }));
    commandsLoaded = true;
  } catch (e) {
    console.error("list_commands failed:", e);
    commandItems.value = [];
    commandsLoaded = true;
  }
}

/** Open the panel + lazy-load the command list. Called from the
 *  input watcher when `detectCommandTrigger` flips to true. */
async function openCommandPalette(filter: string): Promise<void> {
  commandFilter.value = filter;
  commandPaletteOpen.value = true;
  await loadCommands();
}

function closeCommandPalette(): void {
  commandPaletteOpen.value = false;
  // Drop the cached list so the next open re-scans. The backend's
  // mtime fence makes this nearly free; the round-trip is the only
  // cost, and a user editing their `~/.config/everlasting/commands/`
  // between two opens expects the new file to show up.
  commandsLoaded = false;
  commandItems.value = [];
}

/** Re-evaluate trigger state on every input. Open the panel when
 *  the cursor enters command shape; close it when the user types
 *  past the command-name region (space, punctuation, newline) or
 *  deletes the leading `/`. */
function syncCommandPalette(): void {
  if (isComposing.value) return;
  const { trigger, filter } = detectCommandTrigger();
  if (trigger) {
    if (!commandPaletteOpen.value) {
      void openCommandPalette(filter);
    } else {
      commandFilter.value = filter;
    }
  } else if (commandPaletteOpen.value) {
    closeCommandPalette();
  }
}

/** Selected-item dispatcher. Called by TriggerMenu's `@select`.
 *  Builtins run client-side; custom commands are PR3 territory.
 *
 *  The `/` prefix and any filter the user typed are stripped from
 *  the textarea BEFORE dispatch — so selecting `clear` from the
 *  panel leaves the input empty (ready for the next message),
 *  matching Claude Code. */
async function onCommandSelect(item: TriggerMenuItem): Promise<void> {
  // Strip the `/`-prefixed command token from the current line.
  // We remove the leading `/` + the command name (+ any filter
  // chars the user had typed). Leaves any surrounding text intact
  // (rare — the trigger only fires when the line is a bare
  // command-name shape).
  const { lineStart } = currentLineInfo();
  const beforeLine = input.value.slice(0, lineStart);
  const afterToken = input.value.slice(lineStart);
  // The user may have typed a prefix (e.g. `/he` for `help`); we
  // remove the entire typed prefix, not just the matched name, so
  // no leftover `lp` stays in the box after selecting `help`.
  const tokenEnd = afterToken.search(/[\s\n]/) === -1 ? afterToken.length : afterToken.search(/[\s\n]/);
  const newAfter = afterToken.slice(tokenEnd);
  input.value = beforeLine + newAfter;
  // Close the panel first so the dispatch side-effects (modal,
  // invoke) don't race with a still-mounted panel.
  closeCommandPalette();
  // Refocus + autosize so the cursor lands back in the textarea.
  await nextTick();
  const el = textareaEl.value;
  if (el) {
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
    el.focus();
  }

  const sid = chatStore.currentSessionId;

  if (item.is_builtin) {
    // Dispatch builtin commands. None of these go to the LLM.
    switch (item.name) {
      case "help":
        // `/help` is a no-op dispatch: the panel already shows the
        // full list when open, and there's no separate help view in
        // PR2. Re-open the panel so the user sees the full list
        // again (the close above was so the dispatch could run; for
        // help we want the list visible). Filter is cleared.
        await openCommandPalette("");
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

  // Custom command — PR3 territory. PR2 leaves a console
  // breadcrumb and a user-visible toast so the wiring is
  // obvious during development. The body is NOT fetched here
  // (`get_command_body` is PR3); the user message is not sent.
  console.info(
    `[B3] custom command "/${item.name}" selected — body expansion is PR3 (not yet wired).`,
  );
  projectsStore.showToast(
    `用户命令 /${item.name} 的模板展开将在 PR3 实现`,
    "info",
  );
}

function submit() {
  const text = input.value;
  if (!text.trim() || props.sending) return;
  input.value = "";
  // Reset height on send so an emptied field collapses to a single
  // line immediately rather than snapping to 0 on the next input.
  const el = textareaEl.value;
  if (el) el.style.height = "auto";
  emit("send", text);
}

const sendDisabled = (): boolean => props.sending || !input.value.trim();

function onEscKeydown() {
  if (props.sending) {
    onStop();
  }
}
</script>

<template>
  <footer class="chat-input" @keydown.escape.prevent="onEscKeydown">
    <div class="chat-input__row" :style="inputRowStyle">
      <!-- PR2 (B7): per-session Mode picker. Placed on the LEFT
           of the input row (same line as the textarea), NOT in
           the hint row, per Q4 P2 in the 2026-06-13 mode-redesign
           grill-with-docs session. Rationale: mode = "input
           context" — physically adjacent to the input box. Same
           popover pattern as `ModelSelect` (upward-opening,
           hand-rolled) but separate visual position. The trigger
           shows the current Mode label (Edit / Plan / Yolo).
           Shift+Tab cycles Mode via `useKeyboard`. -->
      <ModeSelect />
      <!-- B3 (PR2): command palette. Anchored to the input row
           (position: relative on the row makes it the
           offsetParent); opens UPWARD above the textarea when the
           user types `/` at the start of the current line. The
           TriggerMenu component is a reusable skeleton (see its
           top-of-file comment) — B2 (@file) and B4 (skill) will
           reuse it with a different trigger char + data source. -->
      <TriggerMenu
        ref="triggerMenu"
        :open="commandPaletteOpen"
        :items="commandItems"
        :filter="commandFilter"
        trigger="/"
        header-label="命令"
        empty-label="无匹配命令"
        :trigger-el="textareaEl"
        @select="onCommandSelect"
        @close="closeCommandPalette"
      />
      <textarea
        ref="textareaEl"
        :value="input"
        class="chat-input__field"
        rows="1"
        :placeholder="placeholder ?? '问点什么,或输入 / 调出命令…'"
        :disabled="sending"
        @input="onTextareaInput"
        @compositionstart="onCompositionStart"
        @compositionend="onCompositionEnd"
        @keydown="onKeydown"
      />
      <!-- PR5: morph the send button into a Stop button while
           `sending` is true. We use the same accent color for
           visual continuity; the stop glyph is a CSS-rendered
           square (no extra icon import — heroicons 2.x has no
           StopIcon). The button is always enabled (even when the
           input is empty) so the user can interrupt a long
           stream with no draft. -->
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
        @click="onSubmit"
      >
        <Icon name="arrow-up" :size="16" />
      </button>
    </div>
    <div class="chat-input__hint">
      <!-- F5 follow-up: LLM cumulative latency chip (LEFT).
           Renders the Σ totalMs of every recorded assistant turn
           in the active session. Clicking opens a popover with a
           per-turn breakdown (TTFB / Gen / Total). Pre-F5 / no
           session / no recorded turns → "—". -->
      <div
        v-if="chatStore.currentSessionId"
        ref="latencyPopoverRoot"
        class="chat-input__latency"
      >
        <button
          type="button"
          class="chat-input__latency-chip"
          :class="{
            'chat-input__latency-chip--open': latencyPopoverOpen,
          }"
          :aria-haspopup="'dialog'"
          :aria-expanded="latencyPopoverOpen"
          :title="
            chatStore.currentSessionLatencyTotal !== null
              ? '点击查看本次 session LLM 累计耗时明细'
              : '本次 session 还没有 LLM 耗时数据'
          "
          @click="toggleLatencyPopover"
        >
          <Icon name="clock" :size="11" />
          <span class="chat-input__latency-label">LLM</span>
          <span class="chat-input__latency-value">
            {{
              chatStore.currentSessionLatencyTotal !== null
                ? abbreviateDuration(chatStore.currentSessionLatencyTotal)
                : "—"
            }}
          </span>
        </button>
        <Transition name="chat-input-latency-popover">
          <div
            v-if="latencyPopoverOpen"
            class="chat-input__latency-popover"
            role="dialog"
            aria-label="LLM 累计耗时明细"
          >
            <div class="chat-input__latency-popover-header">
              <Icon name="clock" :size="11" />
              <span>本次 session LLM 累计耗时</span>
            </div>
            <div class="chat-input__latency-popover-summary">
              <div class="chat-input__latency-popover-row">
                <span>累计</span>
                <span class="chat-input__latency-popover-strong">
                  {{
                    chatStore.currentSessionLatencyTotal !== null
                      ? abbreviateDuration(chatStore.currentSessionLatencyTotal)
                      : "—"
                  }}
                </span>
              </div>
              <div class="chat-input__latency-popover-row">
                <span>轮次</span>
                <span>{{ latencyTurns?.length ?? 0 }}</span>
              </div>
              <div class="chat-input__latency-popover-row">
                <span>平均</span>
                <span>
                  {{ latencyAverage !== null ? abbreviateDuration(latencyAverage) : "—" }}
                </span>
              </div>
            </div>
            <div
              v-if="latencyTurns && latencyTurns.length > 0"
              class="chat-input__latency-popover-list"
            >
              <div
                v-for="(turn, i) in latencyTurns"
                :key="i"
                class="chat-input__latency-popover-turn"
              >
                <div class="chat-input__latency-popover-turn-head">
                  <span>turn {{ i + 1 }}</span>
                  <span class="chat-input__latency-popover-strong">
                    {{ turn.totalMs !== undefined ? abbreviateDuration(turn.totalMs) : "—" }}
                  </span>
                </div>
                <div class="chat-input__latency-popover-turn-detail">
                  <span>TTFB</span>
                  <span>{{ turn.ttfbMs !== undefined ? abbreviateDuration(turn.ttfbMs) : "—" }}</span>
                </div>
                <div class="chat-input__latency-popover-turn-detail">
                  <span>gen</span>
                  <span>{{ turn.genMs !== undefined ? abbreviateDuration(turn.genMs) : "—" }}</span>
                </div>
              </div>
            </div>
            <div v-else class="chat-input__latency-popover-empty">
              本次 session 还没有 LLM 耗时数据
            </div>
          </div>
        </Transition>
      </div>
      <!-- A4: token usage chip. Render-mode depends on
           whether the session has accumulated any usage:
           - null → "—" with the "升级前未统计" tooltip
           - non-null → the percentage line; tooltip breaks
             the four counters down.
           Color thresholds are 50% (yellow) and 75% (red);
           see `usageLevel` computed above. -->
      <TooltipProvider>
        <TooltipRoot>
          <TooltipTrigger
            as-child
          >
            <span
              class="chat-input__token-usage"
              :class="{
                [`chat-input__token-usage--${usageLevel}`]: usageLevel,
              }"
            >
              <template v-if="chatStore.currentSessionTokenUsage">
                {{ abbreviateTokens(chatStore.currentSessionTokenUsage.input_tokens) }}
                ·
                {{
                  Math.min(
                    100,
                    Math.round(
                      (chatStore.currentSessionTokenUsage.input_tokens /
                        currentModelContextWindow) *
                        100,
                    ),
                  )
                }}% / {{ abbreviateTokens(currentModelContextWindow) }}
              </template>
              <template v-else>—</template>
            </span>
          </TooltipTrigger>
          <TooltipPortal>
            <TooltipContent class="chat-input__token-tooltip" :side-offset="6">
              <template v-if="chatStore.currentSessionTokenUsage">
                <div class="chat-input__token-tooltip-row">
                  <span>input</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>cache_read</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.cache_read_input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>cache_creation</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.cache_creation_input_tokens) }}</span>
                </div>
                <div class="chat-input__token-tooltip-row">
                  <span>output</span>
                  <span>{{ abbreviateTokens(chatStore.currentSessionTokenUsage.output_tokens) }}</span>
                </div>
              </template>
              <template v-else>
                <div class="chat-input__token-tooltip-empty">升级前未统计</div>
              </template>
              <TooltipArrow class="chat-input__token-tooltip-arrow" :size="6" />
            </TooltipContent>
          </TooltipPortal>
        </TooltipRoot>
      </TooltipProvider>
      <!-- PR5: model picker popover (upward-opening) attached to
           the right edge of the hint row. Replaces the
           bottom-of-content `StatusBar` from PR4. -->
      <ModelSelect />
    </div>
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

.chat-input__field {
  flex: 1;
  resize: none;
  border: none;
  background: transparent;
  color: var(--color-text-primary);
  font-family: var(--font-sans);
  font-size: 14px;
  line-height: 1.5;
  outline: none;
  padding: 6px 0;
  min-height: 28px;
  max-height: 200px;
  overflow-y: auto;
}

.chat-input__field::placeholder {
  color: var(--color-text-muted);
}

.chat-input__field:disabled {
  color: var(--color-text-muted);
  cursor: not-allowed;
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
   glyph differentiates it from the up-arrow Send icon. The
   `warn` tool-error color (a warm orange) reads as "danger,
   cancel" without being as harsh as the actual error red. */
.chat-input__stop {
  background: var(--color-tool-error);
}

.chat-input__stop:hover {
  background: color-mix(in srgb, var(--color-tool-error) 80%, #000 20%);
}

/* Tiny centered square — the universal "stop" pictogram. 10×10
   in a 32px button reads as a solid stop block on both standard
   and high-DPI displays. */
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

.chat-input__hint {
  margin-top: 8px;
  padding: 0 6px;
  font-size: 11px;
  color: var(--color-text-muted);
  user-select: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

/* F5 follow-up: LLM cumulative latency chip (LEFT of hint row).
   Shape matches the existing token-usage chip and the A4 color
   thresholds family, but it's a real clickable button (cursor
   pointer) that opens a popover. Uses the same `color-bg-elevated`
   base + `color-bg-border` outline as the worktree chip and the
   `ModelSelect` trigger, so the visual family is consistent. */
.chat-input__latency {
  position: relative;
  display: inline-flex;
  flex-shrink: 0;
}

.chat-input__latency-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--color-text-muted);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  cursor: pointer;
  user-select: none;
  font: inherit;
  font-family: var(--font-mono);
  font-size: 11px;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}

.chat-input__latency-chip:hover {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-chip--open {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
  color: var(--color-text-primary);
}

.chat-input__latency-label {
  color: var(--color-text-secondary);
}

.chat-input__latency-value {
  color: var(--color-text-primary);
  font-weight: 600;
}

/* The latency popover (F5 follow-up). Hand-written like
   ModelSelect's `.model-select__menu` — opens UPWARD because the
   trigger sits at the bottom of the chat panel; opening down
   would clip under the next sibling. Width is enough to fit the
   longest "0.0s · 0.0s · 0.0s" line without overflow. The list
   area scrolls when there are too many turns (rare, but a 50-turn
   session shouldn't break the layout). */
.chat-input__latency-popover {
  position: absolute;
  bottom: calc(100% + 4px);
  top: auto;
  left: 0;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  min-width: 220px;
  max-width: 280px;
  max-height: 320px;
  z-index: 200;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 11px;
  color: var(--color-text-primary);
  font-family: var(--font-mono);
}

.chat-input__latency-popover-header {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--color-text-muted);
  padding-bottom: 4px;
  border-bottom: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-summary {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.chat-input__latency-popover-row {
  display: flex;
  justify-content: space-between;
  gap: 16px;
}

.chat-input__latency-popover-row > span:first-child {
  color: var(--color-text-secondary);
}

.chat-input__latency-popover-strong {
  color: var(--color-text-primary);
  font-weight: 600;
}

.chat-input__latency-popover-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  max-height: 200px;
  padding-top: 4px;
  border-top: 1px solid var(--color-bg-border);
}

.chat-input__latency-popover-turn {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 4px 0;
}

.chat-input__latency-popover-turn + .chat-input__latency-popover-turn {
  border-top: 1px dashed var(--color-bg-border);
}

.chat-input__latency-popover-turn-head {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  font-weight: 500;
}

.chat-input__latency-popover-turn-detail {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  color: var(--color-text-secondary);
  padding-left: 8px;
}

.chat-input__latency-popover-empty {
  color: var(--color-text-muted);
  text-align: center;
  padding: 6px 0;
}

/* Open/close animation. The popover opens UP, so it slides
   from translateY(4px) (slightly below the final position) up
   into place. Exit reverses. Matches the ModelSelect pattern. */
.chat-input-latency-popover-enter-active,
.chat-input-latency-popover-leave-active {
  transition: opacity 150ms ease-out, transform 150ms ease-out;
  transform-origin: bottom left;
}

.chat-input-latency-popover-enter-from,
.chat-input-latency-popover-leave-to {
  opacity: 0;
  transform: translateY(4px);
}

.chat-input-latency-popover-leave-active {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}

/* A4 (Token Usage Tracking): the per-session token usage
   chip in the hint row. The chip is a TooltipTrigger
   (reka-ui); the trigger itself has no role, the span is
   the visual target. The three color states map to the
   threshold ladder:
   - ok (0-49%): subtle green tint, still readable on dark
   - warn (50-74%): amber, calls attention
   - alert (75%+): red, stops the eye */
.chat-input__token-usage {
  display: inline-flex;
  align-items: center;
  padding: 0 6px;
  font-size: 11px;
  font-family: var(--font-mono);
  white-space: nowrap;
  cursor: help;
  border-radius: 4px;
  color: var(--color-text-muted);
  transition: color 0.15s;
  user-select: none;
}

.chat-input__token-usage--ok {
  color: #4ade80; /* green-400 — readable on dark, doesn't shout */
}

.chat-input__token-usage--warn {
  color: #fbbf24; /* amber-400 — matches --color-tool-shell family */
}

.chat-input__token-usage--alert {
  color: var(--color-tool-error);
}

/* Tooltip content (reka-ui `TooltipContent` portal to body
   — must use :deep() per `.trellis/spec/frontend/reka-ui-usage.md`
   gotcha). The popover floats above the trigger (default
   side is "top" since the chat input is at the bottom of the
   viewport). */
:deep(.chat-input__token-tooltip) {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
  padding: 8px 10px;
  min-width: 180px;
  z-index: 3000;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  animation: chat-input-tooltip-enter 150ms ease-out;
}

:deep(.chat-input__token-tooltip-row) {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-row span:first-child) {
  color: var(--color-text-secondary);
}

:deep(.chat-input__token-tooltip-empty) {
  color: var(--color-text-muted);
  text-align: center;
  padding: 2px 0;
}

:deep(.chat-input__token-tooltip-arrow) {
  fill: var(--color-bg-surface);
  stroke: var(--color-bg-border);
}

@keyframes chat-input-tooltip-enter {
  from {
    opacity: 0;
    transform: translateY(2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}
</style>
