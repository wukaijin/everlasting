// chatInputCodeMirror — CodeMirror 6 composable for the chat
// composer.
//
// Extracted from `ChatInput.vue` (split refactor 2026-06-23). The
// composable owns the CodeMirror 6 host lifecycle, IME-aware keymap,
// `/` + `@` trigger detection (command + file palettes), and panel
// state (open / items / filter / loaded flags). The parent
// (`ChatInput.vue`) only owns:
//   - The <div ref="host"> element
//   - The dispatch handlers (`onCommandSelect` / `onFileSelect`)
//     which touch Tauri `invoke` + `chatStore.send` (composable is
//     **0 store import** by ADR-1)
//   - The submit wrapper (`onSubmit`) that emits `send` after
//     reading the current doc
//   - Public-facing refs (`replaceDoc`, `view.dom` for TriggerMenu's
//     `:trigger-el`)
//
// **Panel data source (ADR-2)**: the composable internally manages
// `commandPaletteOpen / commandItems / commandFilter / commandsLoaded`
// and the matching file-palette state. The parent passes
// `opts.commandItemsSource` + `opts.fileItemsSource` callbacks; the
// composable invokes them inside `syncCommandPalette` /
// `syncFilePalette` to populate `commandItems` / `fileItems`. This
// keeps the composable free of store dependencies (the callbacks
// can be arrow functions that read whatever they need).
//
// **IME safety**: `submit()` checks `view.composing` BEFORE delegating
// to `opts.onSubmit()`. If composing, `submit()` returns true
// (intercept) so CM doesn't insert a newline during IME composition.
// The dispatch to `opts.onSubmit()` clears the CM doc and emits
// `send` with the pre-clear text.
//
// **Trigger detection (unchanged)**: `currentSlashToken` and
// `currentAtToken` walk left from the caret for the trigger char,
// enforce the boundary rule (prev char must be line-start or
// whitespace), and compute the token span. These were inlined in
// `ChatInput.vue` (lines 599-654 + 884-944) and are moved here
// verbatim — no behavior change.

import { ref, shallowRef, onMounted, onUnmounted, watch, type Ref, type ShallowRef } from "vue";
import { EditorState, Compartment, Prec } from "@codemirror/state";
import {
  EditorView,
  keymap,
  placeholder as cmPlaceholder,
  type ViewUpdate,
} from "@codemirror/view";
import { tokenHighlightPlugin } from "../components/chat/chatInputTokens";
import type { TriggerMenuItem } from "../components/chat/TriggerMenu.vue";

// === Public API surface (locked by PRD §Technical Approach) ===

export interface ChatInputCodeMirrorApi {
  /** The CM EditorView instance (or null before mount). shallowRef —
   *  EditorView is mutable; deep reactivity would be wrong. */
  view: ShallowRef<EditorView | null>;
  /** The current doc text mirrored as a Vue ref. The parent reads
   *  this for `sendDisabled` and to gate `submit()`. Updated from
   *  CM via the updateListener (`docChanged` → mirror back). */
  input: Ref<string>;
  /** Replace the entire CM doc; optionally set caret. Used by the
   *  parent's command/file panel selection handlers to strip the
   *  trigger token before dispatch. */
  replaceDoc: (newDoc: string, caret?: number) => void;
  /** Read the `/` token at the caret, anywhere on the current line.
   *  Returns null when not on a `/` trigger line. */
  currentSlashToken: () => SlashToken | null;
  /** Read the `@` token at the caret, anywhere on the current line.
   *  Returns null when not on an `@` trigger line. */
  currentAtToken: () => AtToken | null;
  /** Cheap trigger check used by the composable's own sync loop. */
  detectCommandTrigger: () => { trigger: boolean; filter: string };
  detectFileTrigger: () => { trigger: boolean; filter: string };
  /** Pull the current slash/at token into the panel items + filter.
   *  Invokes the parent's `commandItemsSource` / `fileItemsSource`
   *  callbacks to refresh items, then opens the panel if a trigger
   *  is active. Called from the updateListener on every doc change. */
  syncCommandPalette: () => void;
  syncFilePalette: () => void;
  /** Force-close panels (e.g. on Esc). */
  closeCommandPalette: () => void;
  closeFilePalette: () => void;
  /** Enter handler. Returns true if Enter was consumed by CM
   *  (composing or trigger panel confirm). The parent's
   *  `onSubmit` is invoked when Enter is a plain submit. */
  submit: () => boolean;
  /** Reference to the command palette's `<TriggerMenu>` instance,
   *  so the parent can call `moveActive` / `confirmActive` from
   *  the CM keymap. `null` until the parent mounts the TriggerMenu. */
  commandMenuRef: Ref<{ moveActive: (d: number) => void; confirmActive: () => void } | null>;
  /** Reference to the file palette's `<TriggerMenu>` instance. */
  fileMenuRef: Ref<{ moveActive: (d: number) => void; confirmActive: () => void } | null>;
  /** Reactive flags used by the parent to pass `:open` to the
   *  TriggerMenu panels. */
  commandPaletteOpen: Ref<boolean>;
  filePaletteOpen: Ref<boolean>;
  /** Reactive filter strings used by the parent to pass `:filter`
   *  to the TriggerMenu panels. */
  commandFilter: Ref<string>;
  fileFilter: Ref<string>;
  /** Reactive items used by the parent to pass `:items` to the
   *  TriggerMenu panels. */
  commandItems: Ref<TriggerMenuItem[]>;
  fileItems: Ref<TriggerMenuItem[]>;
}

/** Geometry of the `/`-trigger token under the caret. Mirrors the
 *  shape the parent uses to strip the token from the doc on
 *  selection. `slashOffset` and `tokenEnd` are doc offsets; the
 *  replacement slice is `[slashOffset, tokenEnd)`. */
export interface SlashToken {
  line: number;
  from: number;
  to: number;
  /** True when the `/` token is active (panel should be open). */
  trigger: boolean;
  /** Text typed after the `/` (used for the filter). */
  filter: string;
  /** Doc offset of the `/` char (inclusive). -1 when not triggered. */
  slashOffset: number;
  /** Doc offset ONE past the last token char (exclusive). -1 when
   *  not triggered. */
  tokenEnd: number;
}

/** Geometry of the `@`-trigger token under the caret. */
export interface AtToken {
  line: number;
  from: number;
  to: number;
  trigger: boolean;
  filter: string;
  /** Doc offset of the `@` char (inclusive). -1 when not triggered. */
  atOffset: number;
  /** Doc offset ONE past the last token char (exclusive). -1 when
   *  not triggered. */
  tokenEnd: number;
}

export interface UseChatInputCodeMirrorOpts {
  /** The host element the EditorView mounts into. Provided by the
   *  parent via a template ref. The composable waits for this to
   *  become non-null before creating the view (onMounted). */
  host: Ref<HTMLDivElement | null>;
  /** Reactive `sending` flag. When true, the editor is readOnly —
   *  dispatches from the trigger-panel selection handlers still
   *  work because we use `EditorState.readOnly` (blocks user input)
   *  rather than `EditorView.editable.of(false)` (which only blocks
   *  DOM events). */
  sending: Ref<boolean>;
  /** Reactive placeholder text. Reconfigures the placeholder
   *  Compartment without rebuilding the whole state. */
  placeholder: Ref<string | undefined>;
  /** Called when Enter is pressed and the editor is NOT composing
   *  and no trigger panel is open. The parent's `onSubmit` reads
   *  the current doc text, dispatches a clear, and emits `send`. */
  onSubmit: () => void;
  /** Optional callback invoked from `syncCommandPalette` to refresh
   *  the command panel's items (e.g. fetch from `list_panel_items`
   *  IPC + map to `TriggerMenuItem[]`). Returning items updates
   *  `commandItems` reactively. If absent, the composable leaves
   *  `commandItems` empty (panel still opens with empty list). */
  commandItemsSource?: () => TriggerMenuItem[] | Promise<TriggerMenuItem[]>;
  /** Optional callback invoked from `syncFilePalette` to refresh
   *  the file panel's items. */
  fileItemsSource?: () => TriggerMenuItem[] | Promise<TriggerMenuItem[]>;
}

/** Composable factory. The parent calls this in `<script setup>`
 *  and uses the returned API surface. */
export function useChatInputCodeMirror(
  opts: UseChatInputCodeMirrorOpts,
): ChatInputCodeMirrorApi {
  const input = ref("");
  const view = shallowRef<EditorView | null>(null);
  const editableCompartment = new Compartment();
  const placeholderCompartment = new Compartment();

  // Panel state (internally managed per ADR-2; the composable owns
  // these refs so the parent only reads them to pass down as props).
  const commandPaletteOpen = ref(false);
  const commandItems = ref<TriggerMenuItem[]>([]);
  const commandFilter = ref("");
  const filePaletteOpen = ref(false);
  const fileItems = ref<TriggerMenuItem[]>([]);
  const fileFilter = ref("");
  // Marker so we don't refetch on every keystroke. Cleared on close.
  let commandsLoaded = false;
  let filesLoaded = false;

  // TriggerMenu ref handles — the parent sets these via
  // `setCommandMenuRef` / `setFileMenuRef` (typically through a
  // template ref binding).
  const commandMenuRef = ref<{ moveActive: (d: number) => void; confirmActive: () => void } | null>(null);
  const fileMenuRef = ref<{ moveActive: (d: number) => void; confirmActive: () => void } | null>(null);

  // === Trigger detection (verbatim from ChatInput.vue) ============

  function currentSlashToken(): SlashToken | null {
    const v = view.value;
    if (!v) return null;
    const head = v.state.selection.main.head;
    const line = v.state.doc.lineAt(head);
    const lineText = line.text;
    const caretCol = head - line.from;
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
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
    }
    const prevCh = slashCol > 0 ? lineText[slashCol - 1] : "";
    const prevIsBoundary = slashCol === 0 || /\s/.test(prevCh);
    if (!prevIsBoundary) {
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
    }
    const afterSlash = lineText.slice(slashCol + 1, caretCol);
    if (afterSlash.length > 0 && !/^[a-zA-Z0-9_-]+$/.test(afterSlash)) {
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", slashOffset: -1, tokenEnd: -1 };
    }
    const slashOffset = line.from + slashCol;
    let endCol = caretCol;
    for (let i = caretCol; i < lineText.length; i++) {
      if (!/[a-zA-Z0-9_-]/.test(lineText[i])) {
        endCol = i;
        break;
      }
      endCol = i + 1;
    }
    const tokenEnd = line.from + endCol;
    return {
      line: line.number,
      from: line.from,
      to: tokenEnd,
      trigger: true,
      filter: afterSlash,
      slashOffset,
      tokenEnd,
    };
  }

  function detectCommandTrigger(): { trigger: boolean; filter: string } {
    const t = currentSlashToken();
    if (!t) return { trigger: false, filter: "" };
    return { trigger: t.trigger, filter: t.filter };
  }

  function currentAtToken(): AtToken | null {
    const v = view.value;
    if (!v) return null;
    const head = v.state.selection.main.head;
    const line = v.state.doc.lineAt(head);
    const lineText = line.text;
    const caretCol = head - line.from;
    let atCol = -1;
    for (let i = caretCol - 1; i >= 0; i--) {
      const ch = lineText[i];
      if (ch === " ") break;
      if (ch === "@") {
        atCol = i;
        break;
      }
    }
    if (atCol === -1) {
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
    }
    const prevCh = atCol > 0 ? lineText[atCol - 1] : "";
    const prevIsBoundary = atCol === 0 || /\s/.test(prevCh);
    if (!prevIsBoundary) {
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
    }
    const afterAt = lineText.slice(atCol + 1, caretCol);
    if (afterAt.includes(" ")) {
      return { line: line.number, from: 0, to: 0, trigger: false, filter: "", atOffset: -1, tokenEnd: -1 };
    }
    const atOffset = line.from + atCol;
    let endCol = caretCol;
    for (let i = caretCol; i < lineText.length; i++) {
      if (/\s/.test(lineText[i])) {
        endCol = i;
        break;
      }
      endCol = i + 1;
    }
    const tokenEnd = line.from + endCol;
    return {
      line: line.number,
      from: line.from,
      to: tokenEnd,
      trigger: true,
      filter: afterAt,
      atOffset,
      tokenEnd,
    };
  }

  function detectFileTrigger(): { trigger: boolean; filter: string } {
    const t = currentAtToken();
    if (!t) return { trigger: false, filter: "" };
    return { trigger: t.trigger, filter: t.filter };
  }

  // === Doc replace ================================================

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

  // === Panel sync (ADR-2) ========================================

  async function loadCommands(): Promise<void> {
    if (commandsLoaded || !opts.commandItemsSource) return;
    try {
      const items = await opts.commandItemsSource();
      commandItems.value = items;
      commandsLoaded = true;
    } catch (e) {
      console.error("commandItemsSource failed:", e);
      commandItems.value = [];
      commandsLoaded = true;
    }
  }

  async function openCommandPalette(filter: string): Promise<void> {
    commandFilter.value = filter;
    commandPaletteOpen.value = true;
    await loadCommands();
  }

  function closeCommandPalette(): void {
    commandPaletteOpen.value = false;
    commandsLoaded = false;
    commandItems.value = [];
  }

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

  async function loadFiles(): Promise<void> {
    if (filesLoaded || !opts.fileItemsSource) return;
    try {
      const items = await opts.fileItemsSource();
      fileItems.value = items;
      filesLoaded = true;
    } catch (e) {
      console.error("fileItemsSource failed:", e);
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

  // === Update listener (CM → Vue mirror + palette sync) ==========

  function onEditorUpdate(u: ViewUpdate): void {
    if (u.docChanged) {
      const next = u.state.doc.toString();
      if (next !== input.value) {
        input.value = next;
      }
      syncCommandPalette();
      syncFilePalette();
    }
  }

  // === IME-safe Enter / ArrowUp / ArrowDown / Tab / Esc keymap ===

  function handleEnter(): boolean {
    const v = view.value;
    if (!v) return false;
    if (v.composing) return true;
    if (commandPaletteOpen.value || filePaletteOpen.value) {
      const menu = filePaletteOpen.value ? fileMenuRef.value : commandMenuRef.value;
      menu?.confirmActive();
      return true;
    }
    submit();
    return true;
  }

  function submit(): boolean {
    // IME guard — handled at the keymap level (handleEnter checks
    // `view.composing` first). Here we just delegate to the
    // parent's onSubmit, which is responsible for reading the
    // current text, emitting `send`, and clearing the doc.
    opts.onSubmit();
    return true;
  }

  function buildKeymap() {
    return Prec.highest(
      keymap.of([
        {
          key: "ArrowDown",
          run: () => {
            if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
            const menu = filePaletteOpen.value ? fileMenuRef.value : commandMenuRef.value;
            menu?.moveActive(1);
            return true;
          },
        },
        {
          key: "ArrowUp",
          run: () => {
            if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
            const menu = filePaletteOpen.value ? fileMenuRef.value : commandMenuRef.value;
            menu?.moveActive(-1);
            return true;
          },
        },
        { key: "Enter", run: handleEnter },
        {
          key: "Tab",
          run: () => {
            if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
            const menu = filePaletteOpen.value ? fileMenuRef.value : commandMenuRef.value;
            menu?.confirmActive();
            return true;
          },
        },
        {
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

  // === Lifecycle =================================================

  onMounted(() => {
    if (!opts.host.value) return;
    const initialState = EditorState.create({
      doc: input.value,
      extensions: [
        EditorView.lineWrapping,
        placeholderCompartment.of(cmPlaceholder(opts.placeholder.value ?? "问点什么,或输入 / 调出命令…")),
        editableCompartment.of(EditorState.readOnly.of(opts.sending.value ? true : false)),
        EditorView.updateListener.of(onEditorUpdate),
        buildKeymap(),
        tokenHighlightPlugin,
      ],
    });
    view.value = new EditorView({
      state: initialState,
      parent: opts.host.value,
    });
  });

  onUnmounted(() => {
    view.value?.destroy();
    view.value = null;
  });

  // === Reconfigure watchers ======================================

  watch(
    () => opts.placeholder.value,
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

  watch(
    () => opts.sending.value,
    (sending) => {
      const v = view.value;
      if (!v) return;
      v.dispatch({
        effects: editableCompartment.reconfigure(EditorState.readOnly.of(sending)),
      });
    },
  );

  return {
    view,
    input,
    replaceDoc,
    currentSlashToken,
    currentAtToken,
    detectCommandTrigger,
    detectFileTrigger,
    syncCommandPalette,
    syncFilePalette,
    closeCommandPalette,
    closeFilePalette,
    submit,
    commandMenuRef,
    fileMenuRef,
    commandPaletteOpen,
    filePaletteOpen,
    commandFilter,
    fileFilter,
    commandItems,
    fileItems,
  };
}
