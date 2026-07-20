// tauriTransport — the in-process Tauri IPC forwarder.
//
// Pure forwarding: `invoke` delegates straight to `@tauri-apps/api/core`;
// `listen` delegates to `@tauri-apps/api/event` and unwraps the `Event<T>`
// envelope to `payload` so call sites receive the same shape the future
// HTTP transport will hand them.
//
// This is the ONLY non-test module (besides the transport module itself and
// TitleBar's window/os usage) allowed to import `@tauri-apps/api/*`.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import type { Transport, UnlistenFn } from "./types";

export const tauriTransport: Transport = {
  invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) =>
    tauriInvoke<T>(cmd, args),

  listen: <T = unknown>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<UnlistenFn> =>
    // Tauri hands the callback an `Event<T>` (`{ event, id, payload }`);
    // the transport contract is "unwrapped payload", so pass `e.payload`.
    tauriListen<T>(event, (e) => handler(e.payload)),
};
