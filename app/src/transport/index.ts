// Transport entry point — picks the concrete transport at module load.
//
// Tauri injects `__TAURI_INTERNALS__` on `window`; a plain browser (the
// Phase 2 daemon web client) does not. Today the browser path throws (stub),
// so all shipping code runs `tauriTransport`.
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

export const transport: Transport = isTauri() ? tauriTransport : httpTransport;

export type { Transport, UnlistenFn } from "./types";
