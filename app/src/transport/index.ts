// Transport entry point — picks the concrete transport at module load.
//
// Tauri injects `__TAURI_INTERNALS__` on `window`; a plain browser (the
// Phase 2 daemon web client) does not.
//   - 默认(isTauri()):窗口内 → `tauriTransport`,纯浏览器 → `httpTransport`。
//   - `?transport=http`  → 强制 httpTransport(Tauri 窗口内调试 HTTP 链路)。
//   - `?transport=tauri` → 强制 tauriTransport(浏览器里 debug IPC 路径)。
//
// Import `transport` everywhere instead of `@tauri-apps/api/*`:
//   import { transport } from "../transport";
//   await transport.invoke<T>("load_session", { sessionId });
//   const unlisten = await transport.listen<ChatEvent>("chat-event", (p) => ...);

import { tauriTransport } from "./tauri";
import { httpTransport } from "./http";
import type { Transport } from "./types";

const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/// Resolve the concrete transport at module load(P2.3 C7)。
/// 优先级:`?transport=` query 强制 > `isTauri()` 默认判定。
function resolveTransport(): Transport {
  if (typeof window !== "undefined") {
    const q = new URLSearchParams(window.location.search);
    const override = q.get("transport");
    if (override === "http") return httpTransport;
    if (override === "tauri") return tauriTransport;
  }
  return isTauri() ? tauriTransport : httpTransport;
}

export const transport: Transport = resolveTransport();

export type { Transport, UnlistenFn } from "./types";
