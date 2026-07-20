// Transport abstraction (远程访问 Phase 1, 2026-07-20).
//
// Goal: collapse every Tauri IPC call (`invoke` / `listen`) behind a single
// `Transport` interface so that Phase 2 (daemon split) can swap the Tauri
// in-process IPC for an HTTP + SSE transport WITHOUT touching the 20 call
// sites across stores / utils / components. The Tauri build's behavior is
// unchanged — `tauriTransport` is a pure forwarder.
//
// See `.trellis/tasks/07-20-remote-access-transport-abstraction/design.md §1`.

/**
 * Cancel handle returned by `Transport.listen`. Structurally identical to
 * Tauri's `UnlistenFn` (`() => void`) — re-exported here so call sites can
 * drop their `@tauri-apps/api/event` import entirely and depend only on the
 * transport module.
 */
export type UnlistenFn = () => void;

export interface Transport {
  /**
   * Call a backend command (a `#[tauri::command]` today, an HTTP handler in
   * Phase 2).
   *
   * @param cmd  command name, e.g. `"chat"` / `"load_session"`.
   * @param args argument object; field names stay snake_case to match the
   *             Rust side + `AppCommandError` wire shape.
   * @throws the deserialized backend error (an `AppCommandError`-shaped
   *         object for the daemon path; a rejected Tauri promise today).
   */
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /**
   * Subscribe to a backend event (an `app.emit` channel).
   *
   * @param event   event name, e.g. `"chat-event"` / `"permission:ask"`.
   * @param handler callback invoked with the **already-unwrapped** payload
   *                (NOT Tauri's `Event<T>` envelope — the transport layer
   *                unwraps `event.payload` so both transports present the
   *                same shape to call sites).
   * @returns an unlisten function.
   *
   * Semantics note: Tauri broadcasts globally by event name; the (future)
   * HTTP transport subscribes per-session. That divergence is absorbed
   * INSIDE `httpTransport` (single global EventSource + event-name dispatch
   * table) — the interface stays `listen(event, handler)` so the
   * streamController's requestId-based routing is untouched (RESEARCH §4.2
   * 方案 B).
   */
  listen<T = unknown>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<UnlistenFn>;
}
