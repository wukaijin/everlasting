// Transport entry point — picks the concrete transport at module load.
//
// **P2.4 D3 (2026-07-22)**: the default is now `httpTransport` in ALL
// cases (Tauri webview AND plain browser). The Tauri GUI spawns the
// `everlasting-daemon` sidecar and talks to it over same-origin HTTP
// + SSE; the daemon also serves the SPA via `ServeDir`, so there is no
// cross-origin hop in production. `tauriTransport` is retained ONLY as
// the `?transport=tauri` emergency escape hatch (when the daemon is
// broken, reload the window with that query to fall back to the legacy
// in-process IPC — the Rust side must be in Full GUI mode for this,
// see `lib.rs::run` + `sidecar::GuiMode::resolve`).
//
//   - 默认(无 query): `httpTransport`(sidecar / 同源 daemon)。
//   - `?transport=http`  → 显式 `httpTransport`(等价默认,留作可读自文档)。
//   - `?transport=tauri` → `tauriTransport`(逃生:Rust 侧须 Full 模式)。
//
// Import `transport` everywhere instead of `@tauri-apps/api/*`:
//   import { transport } from "../transport";
//   await transport.invoke<T>("load_session", { sessionId });
//   const unlisten = await transport.listen<ChatEvent>("chat-event", (p) => ...);

import { tauriTransport } from "./tauri";
import { httpTransport } from "./http";
import type { Transport } from "./types";

/// Resolve the concrete transport at module load(P2.3 C7 + P2.4 D3)。
/// 优先级:`?transport=tauri` 显式逃生 > 默认 `httpTransport`。
function resolveTransport(): Transport {
  if (typeof window !== "undefined") {
    const q = new URLSearchParams(window.location.search);
    const override = q.get("transport");
    if (override === "tauri") return tauriTransport;
    // `?transport=http` 与默认一致 —— 显式写出仅作自文档,无行为差。
  }
  return httpTransport;
}

export const transport: Transport = resolveTransport();

export type { Transport, UnlistenFn } from "./types";
