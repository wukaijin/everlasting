// Device token storage for pwa-remote mode (S4 task
// `08-11-pairing-and-pwa`, design §2.1 / D3 / D5).
//
// The PWA (mobile / remote-served browser) authenticates to the remote
// daemon with a per-device `device_token` obtained by redeeming a pairing
// code (`remote/routes/pairing.rs`). Its presence in localStorage is the
// single signal that switches `httpTransport` into pwa-remote mode
// (auth header + `/api/v1/proxy` prefix, design §2.2 / D3):
//
//   - browser-local (no token, daemon serves the SPA): direct connect —
//     proxy prefix empty, no Authorization header. Current behavior,
//     unchanged.
//   - pwa-remote (token present, remote serves the SPA): every app
//     command is proxied through `/api/v1/proxy/*path` to the bound PC
//     node, authenticated with `Authorization: Bearer <token>`.
//
// `daemonBase()` is identical across both modes in PROD
// (`location.origin`), so the token — not the origin — is the only
// reliable mode discriminator (design §1.1 / §2.3).
//
// localStorage access is wrapped in try/catch (private mode / disabled
// cookies throw `SecurityError`); callers fall back to `null` / no-op so
// the app never crashes on storage — aligned with the existing
// `stores/config.ts` localStorage pattern.

const TOKEN_KEY = "everlasting_device_token";

/** Read the device token, or `null` if none / storage unavailable. */
export function getDeviceToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

/** Persist the device token. Activates pwa-remote mode on next
 *  `transport.invoke` / `ensureEventSource` (pairing success path should
 *  also call `resetEventSource()` to rebuild the SSE stream with auth). */
export function setDeviceToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    // localStorage may be unavailable (private mode, etc.) — fail
    // silently; pwa-remote mode simply won't activate.
  }
}

/** Clear the device token (logout / 401 invalidation). Subsequent
 *  transport calls fall back to browser-local (direct) behavior. */
export function clearDeviceToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    // fail silently
  }
}

/** True when a device token is present = pwa-remote mode active. Used by
 *  the router guard and PWA UI to decide pairing gating. */
export function hasDeviceToken(): boolean {
  return getDeviceToken() !== null;
}
