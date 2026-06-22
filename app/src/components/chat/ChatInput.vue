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
// Migration notes:
//   - v-model bridge: `updateListener` emits `update:modelValue` on
//     docChanged (guarded against echo); a `watch` on the prop
//     dispatches external resets back into the CM doc (also guarded).
//   - autosize: CSS-only (`.cm-editor { max-height: 200px }` +
//     `.cm-scroller { overflow: auto }`). The old JS `autosize()`
//     that poked at `el.scrollHeight` is gone — CM's contenteditable
//     `.cm-content` grows natively with the doc.
//   - IME: CM owns composition state. Enter is wired through CM's
//     `keymap`; while `view.composing` is true, Enter is intercepted
//     (returns true) and never reaches `submit()`.
//   - trigger panel routing: ArrowUp / ArrowDown / Enter / Tab / Esc
//     are routed via `Prec.highest(keymap.of([...]))` to whichever
//     TriggerMenu (command or file) is open. The `/` + `@` trigger
//     geometry reads `view.state.doc.lineAt(head)` (via
//     `currentSlashToken` / `currentAtToken`) instead of a textarea
//     `selectionStart`.
//   - TriggerMenu `:trigger-el` is bound to `view.dom` (the
//     `.cm-editor` element) so click-to-reposition-caret inside CM
//     does NOT close the panel (same pattern as the old textarea
//     binding).
//
// The component is "dumb" with respect to the chat model — it emits
// `send` with the trimmed text and lets the parent (ChatPanel) decide
// whether to actually call `store.send` (e.g. guard on `sending`,
// project, etc.).
//
// PR5: when `sending` is true, the right-side send button morphs into
// a Stop button. Clicking it emits `stop`; the parent calls
// `chatStore.cancel()`. The disabled-while-streaming state of the
// input itself is handled by swapping CM into `readOnly` via a
// Compartment — the user can still see what's being streamed; they
// just can't type a new message until the stream ends (or they hit
// Stop and the stream bails out).
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

import { computed, nextTick, onMounted, onUnmounted, ref, shallowRef, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { TooltipProvider, TooltipRoot, TooltipTrigger, TooltipPortal, TooltipContent, TooltipArrow } from "reka-ui";
import { EditorState, Compartment, Prec } from "@codemirror/state";
import {
  EditorView,
  keymap,
  placeholder as cmPlaceholder,
  type ViewUpdate,
} from "@codemirror/view";
import Icon from "../Icon.vue";
import ModelSelect from "./ModelSelect.vue";
import ModeSelect from "./ModeSelect.vue";
import TriggerMenu, { type TriggerMenuItem } from "./TriggerMenu.vue";
// PR1.5 PR-B: token-coloring ViewPlugin for `/command` + `@file`
// (skill token is pre-staged for B4). See chatInputTokens.ts for the
// regex boundaries + IME-safety rationale.
import { tokenHighlightPlugin } from "./chatInputTokens";
import { useChatStore } from "../../stores/chat";
import { MODE_CYCLE, type SessionMode } from "../../stores/chat.types";
import { useModelsStore } from "../../stores/models";
import { useProjectsStore } from "../../stores/projects";
import { abbreviateTokens, tokenUsageLevel, type TokenUsageLevel } from "../../utils/tokenUsage";
import { abbreviateDuration } from "../../utils/duration";
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

const input = ref("");

// === CodeMirror 6 host ===========================================
//
// `host` is the <div> the EditorView mounts into. `view` holds the
// EditorView instance (shallowRef — EditorView is a mutable class
// instance; deep reactivity is unnecessary + would be wrong). Two
// Compartments let us reconfigure extensions without rebuilding the
// whole state: `editableCompartment` flips readOnly on/off when
// `sending` toggles, and `placeholderCompartment` updates the
// placeholder text when the prop changes.

const host = ref<HTMLDivElement | null>(null);
const view = shallowRef<EditorView | null>(null);
const editableCompartment = new Compartment();
const placeholderCompartment = new Compartment();

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

// === CM v-model bridge + keymap ==================================

/**
 * The single updateListener wires three behaviors off every CM
 * transaction (only when the doc actually changed — IME
 * composition transactions don't fire `docChanged` until commit):
 *   1. Mirror the new doc text into the local `input` ref so
 *      `sendDisabled()` and the trigger-panel detection read
 *      live state. The `!==` guard avoids redundant re-assignments
 *      when the watch-handler dispatched the same text back.
 *   2. Re-run both trigger palettes (replaces the old
 *      `onTextareaInput` body). CM fires `docChanged` once at the
 *      end of a composition commit (NOT during the composition),
 *      so this is naturally IME-safe — no manual `isComposing` gate.
 *
 * NOTE: `input` is NOT a v-model prop — the component owns it
 * internally and exposes the typed text only via the `send`
 * event. The CM doc is the source of truth for what's in the
 * editor; `input.value` is the Vue-side mirror used by
 * `sendDisabled()` and the trigger detection helpers.
 */
function onEditorUpdate(u: ViewUpdate): void {
  if (u.docChanged) {
    const next = u.state.doc.toString();
    // Keep the local ref in sync for sendDisabled + trigger
    // detection. The !== guard avoids re-entering when the
    // watch(modelValue) handler dispatched the same text back.
    if (next !== input.value) {
      input.value = next;
    }
    // CM → Vue emit (v-model bridge). Same guard: the prop value
    // mirrors `input.value` (it's our own internal state, not a
    // parent prop), but we keep the guard symmetric for clarity.
    syncCommandPalette();
    syncFilePalette();
  }
}

/** IME-safe Enter handler. While `view.composing` is true the
 *  keymap returns `true` (intercept) without submitting — this is
 *  the CM-native replacement for the old `isComposing` ref gate.
 *  Without it, typing Chinese via pinyin and pressing Enter to
 *  pick a candidate would also fire `submit()` and send the
 *  half-composed text. */
function handleEnter(): boolean {
  const v = view.value;
  if (!v) return false;
  // CM composition state — true while an IME candidate window is
  // open. Returning true intercepts the key so CM doesn't also
  // treat it as a newline insertion.
  if (v.composing) return true;
  // Either trigger panel open → confirm selection instead of submit.
  if (commandPaletteOpen.value || filePaletteOpen.value) {
    const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
    menu?.confirmActive();
    return true;
  }
  // Plain Enter (no Shift) → submit. Shift+Enter is NOT bound here
  // so CM falls through to its default (insert newline) — that's
  // the expected "Shift+Enter = newline" behavior.
  submit();
  return true;
}

/** Build the keymap extension. Uses `Prec.highest` so our Enter /
 *  ArrowUp / ArrowDown / Tab / Esc bindings outrank any other
 *  keymap that might be added later. We do NOT install CM's
 *  `defaultKeymap` (we don't want its history / indentWithTab /
 *  cursor-movement defaults in this single-line-ish composer),
 *  so `Prec.highest` here is mostly future-proofing — if a
 *  future PR adds `defaultKeymap` for undo/redo, our Enter /
 *  ArrowUp / Down / Tab / Esc still win. */
function buildKeymap() {
  return Prec.highest(
    keymap.of([
      {
        key: "ArrowDown",
        run: () => {
          if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
          const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
          menu?.moveActive(1);
          return true;
        },
      },
      {
        key: "ArrowUp",
        run: () => {
          if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
          const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
          menu?.moveActive(-1);
          return true;
        },
      },
      // Enter handled via handleEnter (composition-aware submit).
      { key: "Enter", run: handleEnter },
      {
        // Non-Shift Tab when a panel is open = confirm (same as
        // Enter). Without a panel open, return false so CM falls
        // through to its default (no-op in a single-line editor).
        key: "Tab",
        run: () => {
          if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
          const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
          menu?.confirmActive();
          return true;
        },
      },
      {
        // Esc closes whichever panel is open. If neither is open,
        // return false so the outer `<footer @keydown.escape>` handler
        // (Stop-on-Esc-while-sending) still fires.
        key: "Escape",
        run: () => {
          if (filePaletteOpen.value) {
            closeFilePalette();
            return true;
          }
          if (commandPaletteOpen.value) {
            closeCommandPalette();
            return true;
          }
          return false;
        },
      },
    ]),
  );
}

onMounted(() => {
  if (!host.value) return;
  const initialState = EditorState.create({
    doc: input.value,
    extensions: [
      EditorView.lineWrapping,
      placeholderCompartment.of(cmPlaceholder(props.placeholder ?? "问点什么,或输入 / 调出命令…")),
      editableCompartment.of(EditorState.readOnly.of(props.sending ? true : false)),
      EditorView.updateListener.of(onEditorUpdate),
      buildKeymap(),
      // PR1.5 PR-B: color `/command` + `@file` tokens in the editor.
      // Pure-decoration plugin — never dispatches / mutates doc /
      // selection, so it's invisible to the input / IME / caret flow.
      tokenHighlightPlugin,
    ],
  });
  view.value = new EditorView({
    state: initialState,
    parent: host.value,
  });
});

onUnmounted(() => {
  view.value?.destroy();
  view.value = null;
});

// Vue → CM: when the parent changes the placeholder prop, reconfigure
// the placeholder Compartment without rebuilding the whole state.
watch(
  () => props.placeholder,
  (next) => {
    const v = view.value;
    if (!v) return;
    v.dispatch({
      effects: placeholderCompartment.reconfigure(
        cmPlaceholder(next ?? "问点什么,或输入 / 调出命令…"),
      ),
    });
  },
);

// Vue → CM: when `sending` toggles, flip readOnly via the editable
// Compartment. Replaces the old `:disabled="sending"` textarea
// binding. We use `EditorState.readOnly` (prevents user-driven doc
// mutations) rather than `EditorView.editable.of(false)` (which
// only prevents the DOM from receiving user input but still allows
// programmatic dispatches) so the trigger-panel dispatches
// (replaceDoc in onCommandSelect / onFileSelect) + the submit
// clear-dispatch still work while a stream is in flight.
watch(
  () => props.sending,
  (sending) => {
    const v = view.value;
    if (!v) return;
    v.dispatch({
      effects: editableCompartment.reconfigure(EditorState.readOnly.of(sending)),
    });
  },
);

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

// -----------------------------------------------------------------------
// B3 `/command` palette (PR2).
//
// The TriggerMenu is a reusable skeleton (TriggerMenu.vue); this
// block owns the B3 + B4 Stretch 2 wiring:
//   - detection: open the panel when a `/`-prefixed command-name
//     token sits under the caret, ANYWHERE on the current line
//     (2026-06-18: was line-start only; `/skill` now triggers
//     mid-line too). `2026/06/18` and `a/b` are excluded by the
//     boundary + char-class rules in `currentSlashToken`.
//   - IME safety: CM fires `docChanged` only at composition commit,
//     so the panel never opens mid-composition.
//   - data source: lazy `list_panel_items` IPC the first time the
//     panel opens in a session; the backend's mtime-fence cache
//     (CommandCache + SkillCache) makes subsequent opens free.
//   - keyboard routing: when the panel is open, ArrowUp / ArrowDown
//     / Enter / Escape are intercepted by the CM keymap (see
//     buildKeymap above) and routed to the TriggerMenu. Enter no
//     longer submits while the panel is open.
//   - dispatch: builtins (`/help` `/clear` `/new`) run client-side
//     (no LLM round-trip); custom commands fetch their template body
//     via `get_command_body` and skills fetch their SKILL.md body
//     via `get_skill_body` — both are then sent as a user message
//     to the LLM (B4 Stretch 2 path; agent loop unchanged).
// -----------------------------------------------------------------------

const triggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
const commandPaletteOpen = ref(false);
const commandItems = ref<TriggerMenuItem[]>([]);
/** Text the user typed AFTER the `/`. Empty string = "show all".
 *  Computed from the editor's current line on every doc change. */
const commandFilter = ref("");
/** Marker so we don't refetch `list_panel_items` on every keystroke.
 *  Cleared when the panel closes (so a future edit to a command
 *  or skill file is picked up on the next open — the backend's
 *  mtime fence makes the IPC cheap anyway, but the round-trip
 *  itself is not free). */
let commandsLoaded = false;

// B2 @文件 palette. Symmetrical to the B3 command block above: a second
// <TriggerMenu> caller with trigger="@" + fuzzysort. The two palettes
// are mutually exclusive (a line can't start with both `/` and `@`),
// so only one is open at a time; the CM keymap routes keys to
// whichever is open. `filesLoaded` is cleared on close so the next
// `@` re-fetches the file list (source trees churn — no backend
// mtime cache).
const fileTriggerMenu = ref<InstanceType<typeof TriggerMenu> | null>(null);
const filePaletteOpen = ref(false);
const fileItems = ref<TriggerMenuItem[]>([]);
const fileFilter = ref("");
let filesLoaded = false;

/** Locate the `/`-trigger token under the caret, ANYWHERE on the
 *  current line (Cursor-style — mirrors `currentAtToken` for `@`).
 *
 *  2026-06-18 change: the command/skill palette used to require `/`
 *  at the start of the current line. The user asked for `/skill` to
 *  trigger mid-line too (so it can be appended after other text),
 *  so the `/` panel now shares the `@` panel's anywhere-on-line
 *  geometry. The boundary + char-class rules below keep it from
 *  firing on dates (`2026/06/18`) or inline word-slashes (`a/b`).
 *
 *  Rules:
 *  - Walk left from the caret for the nearest `/`. A space stops the
 *    walk (a `/` separated from the caret by a space is not "live").
 *  - The char before `/` must be line-start or whitespace.
 *  - The span `/`→caret must be command-name chars `[a-zA-Z0-9_-]`
 *    only. A space or punctuation there means the user finished the
 *    token → panel closes.
 *  - `tokenEnd` extends past the caret to the first non-name char, so
 *    selecting from the panel strips the whole typed prefix (e.g.
 *    `/he` when picking `help`), not just up to the caret.
 *
 *  Same return shape as `currentAtToken` so `onCommandSelect` strips
 *  the token via `[slashOffset, tokenEnd)` exactly like `onFileSelect`
 *  does for `@`. */
function currentSlashToken(): {
  trigger: boolean;
  filter: string;
  slashOffset: number;
  tokenEnd: number;
} {
  const v = view.value;
  if (!v) return { trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
  const head = v.state.selection.main.head;
  const line = v.state.doc.lineAt(head);
  const lineText = line.text;
  const caretCol = head - line.from;
  // Walk left for the nearest `/` on this line.
  let slashCol = -1;
  for (let i = caretCol - 1; i >= 0; i--) {
    const ch = lineText[i];
    if (ch === " ") break;
    if (ch === "/") {
      slashCol = i;
      break;
    }
  }
  if (slashCol === -1) {
    return { trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
  }
  // Boundary check: char before `/` must be line-start or whitespace
  // (defends against `2026/06/18`, `a/b`, etc.).
  const prevCh = slashCol > 0 ? lineText[slashCol - 1] : "";
  const prevIsBoundary = slashCol === 0 || /\s/.test(prevCh);
  if (!prevIsBoundary) {
    return { trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
  }
  // `/`→caret must be command-name chars only.
  const afterSlash = lineText.slice(slashCol + 1, caretCol);
  if (afterSlash.length > 0 && !/^[a-zA-Z0-9_-]+$/.test(afterSlash)) {
    return { trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
  }
  const slashOffset = line.from + slashCol;
  // tokenEnd = exclusive end, bounded by the first non-name char to
  // the RIGHT of the caret (or the line end).
  let endCol = caretCol;
  for (let i = caretCol; i < lineText.length; i++) {
    if (!/[a-zA-Z0-9_-]/.test(lineText[i])) {
      endCol = i;
      break;
    }
    endCol = i + 1;
  }
  const tokenEnd = line.from + endCol;
  return { trigger: true, filter: afterSlash, slashOffset, tokenEnd };
}

/** `/`-trigger detection. Delegates to `currentSlashToken` — the
 *  panel opens whenever a live `/`-prefixed command-name token sits
 *  under the caret, line-start OR mid-line (2026-06-18 change). */
function detectCommandTrigger(): { trigger: boolean; filter: string } {
  const { trigger, filter } = currentSlashToken();
  return { trigger, filter };
}

/** Fetch the merged `/`-trigger panel (builtin + custom command +
 *  skill) from the backend. B4 Stretch 2: the single `list_panel_items`
 *  IPC replaces the B3-only `list_commands` so the same TriggerMenu
 *  surfaces commands AND skills (the dispatcher in `onCommandSelect`
 *  reads `item.source` to pick the right body-fetch IPC). The backend
 *  already enforces the cross-type priority rules (builtin always
 *  wins; skill covers custom command; project covers user). */
async function loadCommands(): Promise<void> {
  if (commandsLoaded) return;
  const projectId = projectsStore.currentProjectId;
  try {
    const list = await invoke<PanelItem[]>("list_panel_items", {
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
    console.error("list_panel_items failed:", e);
    commandItems.value = [];
    commandsLoaded = true;
  }
}

/** Open the panel + lazy-load the command list. Called from the
 *  update listener when `detectCommandTrigger` flips to true. */
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

/** Re-evaluate trigger state on every doc change. Open the panel
 *  when the cursor enters command shape; close it when the user
 *  types past the command-name region (space, punctuation, newline)
 *  or deletes the leading `/`. Called from `onEditorUpdate` only
 *  when `u.docChanged` is true — never during IME composition. */
function syncCommandPalette(): void {
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

/** Replace the doc text and reset the selection back to the same
 *  position (after the inserted text). Used by onCommandSelect /
 *  onFileSelect to strip the trigger token before dispatch.
 *  Dispatches a single CM transaction; the resulting `docChanged`
 *  flips through onEditorUpdate which re-syncs `input.value`. */
function replaceDoc(newDoc: string, caret?: number): void {
  const v = view.value;
  if (!v) {
    input.value = newDoc;
    return;
  }
  const cur = v.state.doc.toString();
  if (cur === newDoc) return;
  v.dispatch({
    changes: { from: 0, to: cur.length, insert: newDoc },
    selection: caret !== undefined ? { anchor: caret } : undefined,
    scrollIntoView: true,
  });
}

/** Selected-item dispatcher. Called by TriggerMenu's `@select`.
 *  Three dispatch paths, picked by `item.source` (B4 Stretch 2):
 *  - `builtin` → client-side action (no LLM): `/help` reopens the
 *    panel; `/clear` clears messages; `/new` creates a session.
 *  - `command` → `get_command_body` → sent as a user message (B3).
 *  - `skill` → leave `/skill-name ` in the editor (NOT auto-sent,
 *    NOT body-expanded). The user can append text and send the raw
 *    `/skill-name ...`; the agent then loads the skill body itself via
 *    the `use_skill` tool (L1 progressive disclosure). 2026-06-18
 *    option 2: a skill selection is an explicit reference the agent
 *    resolves, not a prompt the frontend expands.
 *
 *  builtin + command strip the `/`-token before dispatch (anywhere on
 *  the line via `[slashOffset, tokenEnd)`); skill instead REPLACES the
 *  typed prefix with the canonical `/skill-name ` so the editor holds
 *  a clean reference. */
async function onCommandSelect(item: TriggerMenuItem): Promise<void> {
  // Strip the `/`-prefixed token from ANYWHERE on the current line.
  // The token geometry comes from `currentSlashToken`; we slice the
  // doc by `[slashOffset, tokenEnd)` so the whole typed prefix (e.g.
  // `/he` when picking `help`) is removed, leaving surrounding text
  // intact. Mirrors `onFileSelect`'s `@`-token strip.
  const { slashOffset, tokenEnd } = currentSlashToken();
  const doc = input.value;
  const hasToken = slashOffset >= 0 && tokenEnd >= 0;
  const beforeToken = hasToken ? doc.slice(0, slashOffset) : doc;
  const afterToken = hasToken ? doc.slice(tokenEnd) : "";
  // Close the panel first so the dispatch side-effects don't race
  // with a still-mounted panel.
  closeCommandPalette();

  const sid = chatStore.currentSessionId;

  // B4 Stretch 2: source-aware dispatch. The `source` field on the
  // panel item is set by the backend `list_panel_items` IPC; we
  // mirror its three-value union literally here.
  if (item.is_builtin || item.source === "builtin") {
    // Builtin commands don't reach the LLM. Strip the token first so
    // the input box is clean (e.g. selecting `/clear` empties the box).
    replaceDoc(beforeToken + afterToken, beforeToken.length);
    await nextTick();
    view.value?.focus();
    switch (item.name) {
      case "help":
        // `/help` reopens the panel with the full list (filter
        // cleared) — no separate help view in PR2.
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

  const isSkill = item.source === "skill";
  if (isSkill) {
    // 2026-06-18 (option 2): skill 选中后 textarea 只留 `/skill-name`
    // （带一个尾空格），不展开 body、不发送。用户可追加自然语言（如
    // `/review-pr 看下 diff`），发送原文后由 agent 通过 use_skill tool 自行
    // 加载 skill body（L1 渐进披露，body 作 tool_result），区别于 command 的
    // "展开即发送"。后端 build_skill_listing_block 的提示已强化：LLM 看到
    // `/name` 显式引用会调 use_skill。
    //
    // 尾空格是必须的：`/name` 本身会被 currentSlashToken 重新匹配（`/` + 命令
    // 名字符），导致选中后面板立即重开。加尾空格后光标位于空格右侧，向左找
    // `/` 时遇空格 break → token 不再 live → 面板保持关闭。
    const token = `/${item.name} `;
    const inserted = beforeToken + token + afterToken;
    replaceDoc(inserted, beforeToken.length + token.length);
    await nextTick();
    view.value?.focus();
    return;
  }

  // 自定义命令：fetch body → 作 user message 发送（B3 行为；send 内部清空输入框）。
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

// -----------------------------------------------------------------------
// B2 `@文件` palette (PR1).
//
// Symmetrical to the B3 block above but for the `@` trigger char. The
// file list comes from the backend `list_files` command (gitignore +
// built-in excludes + depth/count caps), fetched once per open; the
// <TriggerMenu>'s built-in fuzzysort (fuzzy prop) narrows it on each
// keystroke. Selecting a file replaces the `@filter` token on the
// current line with `@<relpath>` and leaves the caret right after it
// (PR1: the token is a path hint; PR2 will resolve + inject content in
// the agent loop).
// -----------------------------------------------------------------------

/** Detect `@` anywhere on the current line (Cursor-style trigger). The
 *  trigger fires when the nearest `@` to the LEFT of the caret is
 *  preceded by line-start or whitespace (defends against emails like
 *  `name@host`), AND the span between that `@` and the caret contains
 *  no whitespace (the token hasn't been closed yet). Returns the full
 *  geometry so `detectFileTrigger` (open/close the panel) and
 *  `onFileSelect` (replace the token) share ONE source of truth — no
 *  duplicated scan logic, no off-by-one between the two call sites.
 *
 *  - `trigger`: whether the panel should be open right now.
 *  - `filter`: the text typed after `@` (path prefix, may include `/`).
 *  - `atOffset`: doc offset of the `@` char (inclusive); -1 when not
 *    triggered. Used as the LEFT edge of the replace span.
 *  - `tokenEnd`: doc offset ONE past the last token char (exclusive);
 *    -1 when not triggered. Used as the RIGHT edge of the replace
 *    span — bounded by the first whitespace, line end, or the caret,
 *    whichever comes first. */
function currentAtToken(): {
  trigger: boolean;
  filter: string;
  atOffset: number;
  tokenEnd: number;
} {
  const v = view.value;
  if (!v) return { trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
  const head = v.state.selection.main.head;
  const line = v.state.doc.lineAt(head);
  const lineText = line.text;
  const caretCol = head - line.from;
  // Walk left from the caret looking for the nearest `@` on this line.
  let atCol = -1;
  for (let i = caretCol - 1; i >= 0; i--) {
    const ch = lineText[i];
    if (ch === " ") break; // whitespace stops the search — no `@`
    // is part of the current token anymore.
    if (ch === "@") {
      atCol = i;
      break;
    }
  }
  if (atCol === -1) {
    return { trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
  }
  // Boundary check: the char before `@` must be line-start or
  // whitespace. Otherwise this is an inline word like `name@host`.
  const prevCh = atCol > 0 ? lineText[atCol - 1] : "";
  const prevIsBoundary = atCol === 0 || /\s/.test(prevCh);
  if (!prevIsBoundary) {
    return { trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
  }
  // The token spans `@` up to the caret OR the first whitespace after
  // `@`, whichever is closer. (Cursor is always <= first whitespace
  // because the left-walk above already bailed on whitespace; we still
  // compute the explicit boundary for the replace span.)
  const afterAt = lineText.slice(atCol + 1, caretCol);
  if (afterAt.includes(" ")) {
    return { trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
  }
  const atOffset = line.from + atCol;
  // tokenEnd = exclusive end of the token. Either the first whitespace
  // after `@` on the rest of the line, or the caret (user is still
  // typing into the token), or the line end.
  let endCol = caretCol;
  for (let i = caretCol; i < lineText.length; i++) {
    if (/\s/.test(lineText[i])) {
      endCol = i;
      break;
    }
    endCol = i + 1;
  }
  const tokenEnd = line.from + endCol;
  return { trigger: true, filter: afterAt, atOffset, tokenEnd };
}

/** Detect `@` anywhere on the current line (Cursor-style). The filter
 *  is whatever the user typed after `@` (a path prefix, may contain
 *  `/`); a space ends the token. See `currentAtToken` for the full
 *  geometry + boundary rules. */
function detectFileTrigger(): { trigger: boolean; filter: string } {
  const { trigger, filter } = currentAtToken();
  return { trigger, filter };
}

/** Fetch the project file list. Re-runs on every open (filesLoaded is
 *  cleared on close) so newly-added files show up — no backend mtime
 *  cache, since source trees churn and the frontend only fires this
 *  once per `@` open. */
async function loadFiles(): Promise<void> {
  if (filesLoaded) return;
  const projectId = projectsStore.currentProjectId;
  try {
    const paths = await invoke<string[]>("list_files", {
      projectId: projectId ?? null,
    });
    fileItems.value = paths.map((p) => ({ key: p, name: p }));
    filesLoaded = true;
  } catch (e) {
    console.error("list_files failed:", e);
    fileItems.value = [];
    filesLoaded = true;
  }
}

async function openFilePalette(filter: string): Promise<void> {
  fileFilter.value = filter;
  filePaletteOpen.value = true;
  await loadFiles();
}

function closeFilePalette(): void {
  filePaletteOpen.value = false;
  filesLoaded = false;
  fileItems.value = [];
}

/** Re-evaluate the `@` trigger on every doc change — mirror of
 *  syncCommandPalette. Opens when the line becomes `@foo`, closes when
 *  the user types a space / deletes the `@`. */
function syncFilePalette(): void {
  const { trigger, filter } = detectFileTrigger();
  if (trigger) {
    if (!filePaletteOpen.value) {
      void openFilePalette(filter);
    } else {
      fileFilter.value = filter;
    }
  } else if (filePaletteOpen.value) {
    closeFilePalette();
  }
}

/** Replace the `@<filter>` token on the current line with `@<relpath>`
 *  and place the caret right after it. Works anywhere on the line
 *  (Cursor-style): we replace the doc span [`atOffset`, `tokenEnd`)
 *  returned by `currentAtToken` — that's the `@` + the filter the user
 *  typed, regardless of whether the `@` sits at the line start or
 *  mid-sentence. Any text before the `@` and after the token is left
 *  intact. */
async function onFileSelect(item: TriggerMenuItem): Promise<void> {
  const { atOffset, tokenEnd } = currentAtToken();
  if (atOffset < 0 || tokenEnd < 0) return;
  const doc = input.value;
  const beforeAt = doc.slice(0, atOffset);
  const afterToken = doc.slice(tokenEnd);
  const newDoc = beforeAt + `@${item.name}` + afterToken;
  const caret = atOffset + 1 + item.name.length;
  closeFilePalette();
  replaceDoc(newDoc, caret);
  await nextTick();
  view.value?.focus();
}

function submit() {
  const text = input.value;
  if (!text.trim() || props.sending) return;
  // Clear the CM doc. Dispatching triggers onEditorUpdate which
  // mirrors the empty doc back into `input.value`, so no manual
  // `input.value = ""` here is necessary (kept for belt-and-
  // suspenders safety in case `view` is mid-teardown).
  const v = view.value;
  if (v) {
    const cur = v.state.doc.toString();
    if (cur.length > 0) {
      v.dispatch({ changes: { from: 0, to: cur.length, insert: "" } });
    }
  } else {
    input.value = "";
  }
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
           of the input row (same line as the editor), NOT in
           the hint row, per Q4 P2 in the 2026-06-13 mode-redesign
           grill-with-docs session. Rationale: mode = "input
           context" — physically adjacent to the input box. Same
           popover pattern as `ModelSelect` (upward-opening,
           hand-rolled) but separate visual position. The trigger
           shows the current Mode label (Edit / Plan / Yolo).
           Shift+Tab cycles Mode via `useKeyboard`. -->
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
           tells the user which type they're picking. The
           `onCommandSelect` dispatcher routes by `item.source`:
           builtin → client action, command → get_command_body,
           skill → get_skill_body.
           `:trigger-el` points at the CM `.cm-editor` DOM node
           (view.dom) so click-to-reposition-caret inside CM
           doesn't close the panel. -->
      <TriggerMenu
        ref="triggerMenu"
        :open="commandPaletteOpen"
        :items="commandItems"
        :filter="commandFilter"
        trigger="/"
        header-label="命令"
        empty-label="无匹配命令"
        :trigger-el="view?.dom ?? null"
        @select="onCommandSelect"
        @close="closeCommandPalette"
      />
      <!-- B2 (PR1): @文件 palette. Second <TriggerMenu> caller —
           trigger="@", fuzzysort (fuzzy prop), #row slot renders a
           file icon + relative path. Mutually exclusive with the
           command palette above (a line starts with `/` XOR `@`). -->
      <TriggerMenu
        ref="fileTriggerMenu"
        :open="filePaletteOpen"
        :items="fileItems"
        :filter="fileFilter"
        trigger="@"
        header-label="文件"
        empty-label="无匹配文件"
        fuzzy
        wide
        :trigger-el="view?.dom ?? null"
        @select="onFileSelect"
        @close="closeFilePalette"
      >
        <template #row="{ item }">
          <span class="chat-input__file-row">
            <Icon name="document" :size="12" />
            <code class="chat-input__file-path">{{ item.name }}</code>
          </span>
        </template>
      </TriggerMenu>
      <!-- PR1.5: CodeMirror 6 host div. The EditorView mounts into
           this element on `onMounted` and owns all internal DOM
           (`.cm-editor`, `.cm-scroller`, `.cm-content`). Vue MUST
           NOT render children here — CM is the sole owner of the
           host's subtree (rendering v-html/v-text inside would
           destroy CM's DOM bookkeeping). -->
      <div
        ref="host"
        class="chat-input__field"
        :class="{ 'chat-input__field--disabled': sending }"
        :aria-disabled="sending ? 'true' : undefined"
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
  /* The .cm-scroller inside handles overflow:auto when content
     exceeds 200px — mirrors the old textarea's max-height +
     overflow-y:auto pair. */
}

:deep(.chat-input__field .cm-editor .cm-scroller) {
  font-family: inherit;
  overflow: auto;
  padding: 6px 0;
}

:deep(.chat-input__field .cm-editor .cm-content) {
  /* Match the old textarea's 6px vertical padding baseline so the
     caret + text sit at the same Y as the Mode/Model popovers
     around it. CM's default content padding is 4px; we override
     to 0 because the scroller above already provides 6px. */
  padding: 0;
  caret-color: var(--color-text-primary);
}

:deep(.chat-input__field .cm-editor.cm-focused) {
  /* No double focus ring — the .chat-input__row:focus-within rule
     above already draws the accent ring on the outer container. */
  outline: none;
}

:deep(.chat-input__field .cm-editor .cm-cursor) {
  border-left-color: var(--color-text-primary);
}

/* Placeholder — CM injects `.cm-placeholder` via the placeholder()
   extension. Match the old `::placeholder` muted color. */
:deep(.chat-input__field .cm-editor .cm-placeholder) {
  color: var(--color-text-muted);
}

/* Disabled (sending) state — mirrors the old
   `.chat-input__field:disabled { color: muted; cursor: not-allowed }`.
   CM's `EditorState.readOnly` doesn't add a `:disabled` pseudo, so
   we toggle a class on the host and dim the content + change the
   cursor. We don't disable pointer events entirely so the user
   can still select text to copy mid-stream. */
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
   inline `<span>`s inside `.cm-content`. We scope them under
   `.chat-input__field` (consistent with the other CM `:deep()` rules
   above) so the styling can't leak if a second CM instance ever
   mounts elsewhere in the app. Colors reuse existing design tokens
   (design-tokens.md: "Don't add a new `--color-*` token for a
   one-off use"):
     - `/command` → --color-accent (matches B3 command palette family)
     - `@file`    → --color-tool-read (matches read_file tool family)
     - skill      → --color-tool-thinking (violet, pre-staged for B4)
   font-weight: 600 makes the tokens pop visually without needing a
   brighter color. */
:deep(.chat-input__field .cm-editor .cm-content .cm-token-command) {
  color: var(--color-accent);
  font-weight: 600;
}

:deep(.chat-input__field .cm-editor .cm-content .cm-token-file) {
  color: var(--color-tool-read);
  font-weight: 600;
}

/* B4 skill token — pre-staged. The class is not applied by any regex
   today (chatInputTokens.ts has no skill entry in TOKEN_KINDS yet),
   but the CSS rule is ready so B4 only needs to add the match logic. */
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
     落到视觉左侧、内容右对齐，于是溢出时保留尾部文件名 + 近端目录段（如
     `…nents/chat/ChatInput.vue`）。unicode-bidi:isolate 让纯 ASCII 路径整体
     当 LTR run，字符顺序不变，只翻转省略方向。 */
  min-width: 0;
  flex: 1 1 auto;
  direction: rtl;
  unicode-bidi: isolate;
}

/* file-row 里的 Icon 固定不收缩，确保 file-path 吃掉所有剩余宽度去 ellipsis。 */
.chat-input__file-row :deep(svg) {
  flex: 0 0 auto;
}
</style>
