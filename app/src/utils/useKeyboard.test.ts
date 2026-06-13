// Unit tests for `useKeyboard` — the global keybinding registry
// used by PR2 (B7) for the Shift+Tab mode cycle.
//
// We test the pure registration / dispatch logic; the lifecycle
// hook (`onUnmounted` auto-dispose) is implicit and is exercised
// indirectly through Vue's lifecycle. The point of these tests
// is to lock the contract:
//   1. `registerShiftTabCycle` fires the cycle handler on
//      Shift+Tab AND calls `preventDefault()` (otherwise the
//      browser would also reverse-tab the focus).
//   2. `registerShiftTabCycle` does NOT fire on plain Tab.
//   3. The `enabled()` gate short-circuits the handler.
//   4. `dispose()` removes the listener.
//   5. The base `registerKeybinding` matches keys
//      case-insensitively and respects modifiers per spec.
//   6. Modifiers are additive: `shiftKey: true` requires Shift
//      but does NOT require Ctrl/Meta to be absent (consumers
//      who want stricter matching opt into `noModifier`).
//
// `window` is jsdom's global; the test harness already runs in
// jsdom (vitest.config.ts sets `environment: "jsdom"`).

import { describe, it, expect, vi } from "vitest";
import { effectScope } from "vue";
import { registerKeybinding, registerShiftTabCycle } from "./useKeyboard";

/** Dispatch a KeyboardEvent on `window` so the capture-phase
 *  listener registered by `registerKeybinding` sees it. The
 *  default jsdom `Event` constructor won't accept `key` so we
 *  use the full KeyboardEvent class. */
function pressKey(opts: {
  key: string;
  shiftKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}): KeyboardEvent {
  const ev = new KeyboardEvent("keydown", {
    key: opts.key,
    shiftKey: !!opts.shiftKey,
    ctrlKey: !!opts.ctrlKey,
    metaKey: !!opts.metaKey,
    altKey: !!opts.altKey,
    bubbles: true,
    cancelable: true,
  });
  window.dispatchEvent(ev);
  return ev;
}

describe("useKeyboard — registerKeybinding (base API)", () => {
  it("matches key case-insensitively", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      registerKeybinding({ key: "Tab", shiftKey: true, handler });
      // jsdom keeps the case of `key` as passed; we lowercase
      // before comparing so both 'Tab' and 'TAB' should match.
      pressKey({ key: "TAB", shiftKey: true });
      expect(handler).toHaveBeenCalledOnce();
    });
    scope.stop();
  });

  it("requires shiftKey when binding declares shiftKey=true", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      registerKeybinding({ key: "Tab", shiftKey: true, handler });
      pressKey({ key: "Tab" });
      expect(handler).not.toHaveBeenCalled();
    });
    scope.stop();
  });

  it("does NOT call preventDefault (caller's responsibility)", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      registerKeybinding({ key: "Tab", shiftKey: true, handler });
      const ev = pressKey({ key: "Tab", shiftKey: true });
      expect(handler).toHaveBeenCalledOnce();
      // The base API does not auto-preventDefault — that's the
      // caller's responsibility. `registerShiftTabCycle` is the
      // wrapper that DOES preventDefault for Shift+Tab cycles.
      expect(ev.defaultPrevented).toBe(false);
    });
    scope.stop();
  });

  it("respects the enabled() gate", () => {
    const scope = effectScope();
    scope.run(() => {
      let allowed = false;
      const handler = vi.fn();
      registerKeybinding({
        key: "Tab",
        shiftKey: true,
        handler,
        enabled: () => allowed,
      });

      pressKey({ key: "Tab", shiftKey: true });
      expect(handler).not.toHaveBeenCalled();

      allowed = true;
      pressKey({ key: "Tab", shiftKey: true });
      expect(handler).toHaveBeenCalledOnce();
    });
    scope.stop();
  });

  it("dispose() removes the listener", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      const handle = registerKeybinding({ key: "Tab", shiftKey: true, handler });
      pressKey({ key: "Tab", shiftKey: true });
      expect(handler).toHaveBeenCalledOnce();

      handle.dispose();
      pressKey({ key: "Tab", shiftKey: true });
      // No additional call after dispose.
      expect(handler).toHaveBeenCalledOnce();
    });
    scope.stop();
  });

  it("requires ctrlOrMeta when binding declares ctrlOrMeta=true", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      registerKeybinding({
        key: "k",
        ctrlOrMeta: true,
        handler,
      });
      // Plain "k" — no modifier — should not fire.
      pressKey({ key: "k" });
      expect(handler).not.toHaveBeenCalled();
      // Ctrl+k — fires.
      pressKey({ key: "k", ctrlKey: true });
      expect(handler).toHaveBeenCalledOnce();
      // Meta+k — also fires (macOS Cmd).
      pressKey({ key: "k", metaKey: true });
      expect(handler).toHaveBeenCalledTimes(2);
    });
    scope.stop();
  });

  it("requires noModifier when binding declares noModifier=true", () => {
    const scope = effectScope();
    scope.run(() => {
      const handler = vi.fn();
      registerKeybinding({
        key: "Enter",
        noModifier: true,
        handler,
      });
      // Plain Enter — fires.
      pressKey({ key: "Enter" });
      expect(handler).toHaveBeenCalledOnce();
      // Shift+Enter — should NOT fire.
      pressKey({ key: "Enter", shiftKey: true });
      expect(handler).toHaveBeenCalledOnce();
    });
    scope.stop();
  });
});

describe("useKeyboard — registerShiftTabCycle", () => {
  it("calls the cycle fn on Shift+Tab and preventDefaults", () => {
    const scope = effectScope();
    scope.run(() => {
      const cycle = vi.fn();
      registerShiftTabCycle({ cycle });
      const ev = pressKey({ key: "Tab", shiftKey: true });
      expect(cycle).toHaveBeenCalledOnce();
      // preventDefault must be called so the browser doesn't
      // also reverse-tab the focus.
      expect(ev.defaultPrevented).toBe(true);
    });
    scope.stop();
  });

  it("skips when enabled() returns false", () => {
    const scope = effectScope();
    scope.run(() => {
      const cycle = vi.fn();
      registerShiftTabCycle({ cycle, enabled: () => false });
      pressKey({ key: "Tab", shiftKey: true });
      expect(cycle).not.toHaveBeenCalled();
    });
    scope.stop();
  });

  it("does NOT fire on plain Tab", () => {
    const scope = effectScope();
    scope.run(() => {
      const cycle = vi.fn();
      registerShiftTabCycle({ cycle });
      pressKey({ key: "Tab" });
      expect(cycle).not.toHaveBeenCalled();
    });
    scope.stop();
  });
});