// attachmentUrl — B1 (2026-08-16) image-multimodal: the URL an
// `<img>` / `window.open` uses to fetch a session attachment from
// the daemon's GET route (`daemon/routes/attachments.rs`:
// `GET /api/v1/attachments/:session_id/:file`, the daemon's first
// binary GET route).
//
// Three transport modes resolve to two URL shapes (design §3.3):
//
//   - browser-local PROD: `daemonBase()` === `location.origin`
//     (sidecar serves the SPA, same origin as the daemon) → plain
//     absolute URL.
//   - DEV browser (vite 1420): `daemonBase()` ===
//     `http://localhost:7456` → same absolute-URL form; the cross
//     origin is why we always build from `daemonBase()` instead of
//     emitting a relative `/api/...` path (vite has a dev `/api`
//     proxy as a belt-and-braces fallback, but absolute URLs keep
//     all three modes on one code path).
//   - pwa-remote (device token present): `<img>` cannot send an
//     `Authorization: Bearer` header, so the GET goes through the
//     remote's proxy path with the same query-token channel the
//     SSE `EventSource` uses (`?access_token=`, see
//     `everlasting-remote/src/auth.rs` — header first, query
//     fallback). The remote strips the token before forwarding the
//     clean GET to the bound PC daemon.

import { daemonBase } from "../transport/http";
import { currentDeviceToken } from "../transport/auth";

/** Absolute URL for one attachment's bytes. `file` is the
 *  server-generated name from `save_attachment` (uuid + extension),
 *  NOT a user path — both components are encoded defensively. */
export function attachmentUrl(sessionId: string, file: string): string {
  const base = daemonBase().replace(/\/+$/, "");
  const sid = encodeURIComponent(sessionId);
  const name = encodeURIComponent(file);
  // 08-26 多节点:与 invoke/SSE 同源,取"当前选中节点"的 token。
  const token = currentDeviceToken();
  if (token) {
    // pwa-remote: proxy + query token (GET binary pass-through).
    return `${base}/api/v1/proxy/api/v1/attachments/${sid}/${name}?access_token=${encodeURIComponent(token)}`;
  }
  return `${base}/api/v1/attachments/${sid}/${name}`;
}
