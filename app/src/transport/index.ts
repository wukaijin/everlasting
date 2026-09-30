// Transport entry point — exports the concrete transport.
//
// de-Tauri（2026-09-30, task `09-30-de-tauri`）: `httpTransport` is the
// only implementation (the historical `tauriTransport` and its
// `?transport=tauri` escape hatch died with the GUI bin — the daemon
// serves the SPA same-origin, so HTTP + SSE carries everything). The
// facade stays: every component imports `transport` from here and the
// test suite mocks this boundary (`vi.mock("../../transport")`), which
// is cheaper to keep than a repo-wide rewrite to direct `http.ts`
// imports.
//
// Import `transport` everywhere instead of reaching into `./http`:
//   import { transport } from "../transport";
//   await transport.invoke<T>("load_sessions", { projectId });
//   const unlisten = await transport.listen<ChatEvent>("chat-event", (p) => ...);

import { httpTransport } from "./http";
import type { Transport } from "./types";

export const transport: Transport = httpTransport;

export type { Transport, UnlistenFn } from "./types";
