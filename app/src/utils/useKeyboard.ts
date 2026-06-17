// useKeyboard — global keyboard shortcut registry.
//
// PR2 (B7 front-end): one centralised place for app-wide
// keybindings. The first consumer is Shift+Tab mode cycling
// (Edit → Plan → Yolo → Edit), wired by the
// `registerShiftTabModeCycle` helper below. Future PRs can add
// Ctrl+K / Cmd+P / etc. without scattering `addEventListener`
// calls across components.
//
// Why a global registry (not per-component):
// - `Shift+Tab` is the canonical browser "reverse-tab" key.
//   Default browser behavior moves focus to the previous
//   focusable element. To make it cycle mode instead we MUST
//   intercept the event before the browser does — capture
//   phase + `preventDefault()`. Per-component listeners can
//   miss the key when focus is elsewhere; a single capture-
//   phase listener on `window` guarantees the key is caught.
// - Centralised `enabled` gating: the cycle is suppressed
//   while a stream is in flight (matches the `ModelSelect`
//   `:disabled="isStreaming"` contract; the backend also
//   defers mid-stream mode changes to the next turn).
// - Single mount/unmount lifecycle avoids the bug where
//   multiple components each register their own listener and
//   the order of execution depends on registration order.
//
// Capture-phase gotcha (per `.trellis/spec/frontend/popover-pattern.md`):
// we MUST `addEventListener(..., { capture: true })` and call
// `e.preventDefault()` to override the default focus-shift
// behavior. Bubble-phase listeners run after the browser
// default and would require manual `tabindex` / focus math —
// not worth it.

import { onUnmounted } from "vue";

/** A single keybinding registration. The matcher decides if
 *  the event applies; the handler decides what to do. */
export interface KeyBinding {
  /** Lowercase `KeyboardEvent.key` value (e.g. `"tab"`,
   *  `"escape"`, `"k"`). Matched case-insensitively (we
   *  lowercase before comparing). */
  key: string;
  /** When true, the binding fires only when `Shift` is held. */
  shiftKey?: boolean;
  /** When true, the binding fires only when `Ctrl` (or
   *  `Meta` on macOS) is held. */
  ctrlOrMeta?: boolean;
  /** When true, the binding fires only when no modifier
   *  (no Shift/Ctrl/Meta/Alt) is held. Default `true` for
   *  simple keys like "tab" but `false` is implied when
   *  `shiftKey` / `ctrlOrMeta` is set. */
  noModifier?: boolean;
  /** Optional gate. Returning `false` short-circuits without
   *  calling `handler`. The simplest use is "skip while
   *  streaming" — the consumer closes over the relevant
   *  reactive state. */
  enabled?: () => boolean;
  /** What to do when the binding fires. Call `e.preventDefault()`
   *  inside if the default browser behavior is unwanted. */
  handler: (e: KeyboardEvent) => void;
}

/** Result returned by `registerGlobalKeybindings`. Call
 *  `dispose()` to unregister (auto-called on component
 *  unmount). */
export interface KeyBindingHandle {
  dispose(): void;
}

/** Register a single keybinding on `window` at the capture
 *  phase. Returns a handle with `dispose()`. Also auto-
 *  disposes on Vue unmount if a component scope is active. */
export function registerKeybinding(binding: KeyBinding): KeyBindingHandle {
  const onKeyDown = (e: KeyboardEvent) => {
    // Match key (case-insensitive).
    if (e.key.toLowerCase() !== binding.key.toLowerCase()) return;

    // Match modifiers. Order:
    // 1. shiftKey required: must be held
    // 2. ctrlOrMeta required: Ctrl or Meta held
    // 3. noModifier required: no Shift/Ctrl/Meta/Alt held
    // 4. Otherwise, no modifier constraint — the consumer
    //    is happy with whatever combination the user pressed.
    if (binding.shiftKey && !e.shiftKey) return;
    if (binding.ctrlOrMeta && !(e.ctrlKey || e.metaKey)) return;
    if (binding.noModifier) {
      if (e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    }

    // Consumer-side gate (e.g. "skip while streaming").
    if (binding.enabled && !binding.enabled()) return;

    binding.handler(e);
  };

  if (typeof window !== "undefined") {
    window.addEventListener("keydown", onKeyDown, { capture: true });
  }

  const dispose = () => {
    if (typeof window !== "undefined") {
      window.removeEventListener("keydown", onKeyDown, { capture: true } as EventListenerOptions);
    }
  };

  onUnmounted(dispose);
  return { dispose };
}

/** Convenience: register Shift+Tab as a "cycle through a list"
 *  trigger. The `cycle` callback receives no args and the
 *  binding calls `e.preventDefault()` to override the default
 *  focus-shift behavior. `enabled` is the streaming gate.
 *
 *  PR1.5 (2026-06-17, B2): the handler also calls
 *  `e.stopPropagation()` so the event does NOT continue
 *  propagating to a CodeMirror 6 `EditorView` host. The current
 *  ChatInput CM instance does NOT install `defaultKeymap`, so
 *  Shift+Tab has no CM-side binding today — but if a future PR
 *  adds `defaultKeymap` (e.g. for undo/redo), its Shift+Tab
 *  "inverse-indent" command would otherwise run AFTER our
 *  capture-phase cycle and double-handle the key. The capture-
 *  phase stopPropagation is a small, defensive choke point that
 *  future-proofs the cycle without coupling to whether CM has
 *  a Shift+Tab binding.
 *
 *  This is the only consumer of `registerKeybinding` today;
 *  future PRs can add their own bindings or call this helper
 *  with a different `cycle` fn (e.g. "cycle projects" with
 *  Ctrl+Tab). */
export function registerShiftTabCycle(opts: {
  cycle: () => void;
  enabled?: () => boolean;
}): KeyBindingHandle {
  return registerKeybinding({
    key: "Tab",
    shiftKey: true,
    handler: (e) => {
      e.preventDefault();
      // Stop the event from reaching any target/bubble-phase
      // listener (e.g. a future CodeMirror `defaultKeymap`
      // Shift+Tab "inverse-indent" command). Capture-phase
      // listeners fire BEFORE the target, so stopPropagation
      // here is the correct choke point. Cheap + defensive.
      e.stopPropagation();
      opts.cycle();
    },
    enabled: opts.enabled,
  });
}