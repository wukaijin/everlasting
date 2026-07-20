// httpTransport — Phase 2 stub.
//
// The daemon-split (Phase 2) will implement this over HTTP POST/GET (for
// `invoke`) + a single global EventSource (for `listen`, with an internal
// event-name → handler dispatch table so the public `listen(event, handler)`
// signature is preserved — see design.md §3 / RESEARCH §4.2 方案 B).
//
// Until then every method throws so a mis-selected transport fails loud
// instead of silently no-op'ing.

import type { Transport, UnlistenFn } from "./types";

const NOT_IMPLEMENTED = "httpTransport not implemented (Phase 2)";

export const httpTransport: Transport = {
  invoke: <T = unknown>(
    _cmd: string,
    _args?: Record<string, unknown>,
  ): Promise<T> => {
    throw new Error(NOT_IMPLEMENTED);
  },

  listen: <T = unknown>(
    _event: string,
    _handler: (payload: T) => void,
  ): Promise<UnlistenFn> => {
    throw new Error(NOT_IMPLEMENTED);
  },
};
