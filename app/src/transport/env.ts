// Environment detection helpers (远程访问 Phase 2, 2026-07-23).
//
// The frontend runs in two contexts that share one codebase:
//   1. Tauri webview — `@tauri-apps/api/*` (window controls, OS plugin,
//      IPC) is available; `window.__TAURI_INTERNALS__` is injected by
//      the Tauri runtime.
//   2. Plain browser — the daemon serves the SPA over HTTP; NO Tauri
//      runtime is present, so any synchronous `@tauri-apps/api` call
//      made at component setup time (e.g. `getCurrentWindow()`) throws
//      and crashes the component subtree.
//
// These helpers let components guard Tauri-only code paths. The check
// mirrors what the test suite already relies on (see comments in
// `components/chat/*Card.test.ts` referencing `window.__TAURI_INTERNALS__`).
//
// NOTE: transport selection (`transport/index.ts`) defaults to
// `httpTransport` in BOTH contexts (P2.4 D3); this helper is about the
// *Tauri runtime chrome* (window controls / drag region / OS plugin),
// not about which transport carries `invoke`/`listen`.

/**
 * True when the current document is running inside a Tauri webview
 * (i.e. the Tauri runtime injected `window.__TAURI_INTERNALS__`).
 *
 * Use this to gate Tauri-only chrome — e.g. `TitleBar`'s window-control
 * buttons / drag region, or `@tauri-apps/plugin-os` calls. In a plain
 * browser these APIs throw; guard them with this so component setup
 * doesn't crash and a browser-mode fallback (`BrowserHeader`) renders
 * instead.
 */
export function isTauriWebview(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
