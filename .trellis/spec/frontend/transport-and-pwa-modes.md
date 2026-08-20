# Transport Modes & PWA Navigation (Remote Control)

> Captured from S4 (`08-11-pairing-and-pwa`). The remote-control feature runs the **same SPA bundle** in three contexts; the non-obvious part is how to tell them apart.

## The counterintuitive truth: baseURL is identical across all modes

`daemonBase()` (`transport/http.ts`) resolves to `window.location.origin` in **production for ALL three modes**:

| Mode | Who serves the SPA | `daemonBase()` | Has device_token? |
|---|---|---|---|
| **browser-local** | daemon (sidecar ServeDir) | `location.origin` (= daemon) | No |
| **pwa-remote** | remote (cloud `everlasting-remote` ServeDir) | `location.origin` (= remote) | Yes (after pairing) |
| **tauri escape** (`?transport=tauri`) | n/a (in-process IPC) | n/a | No |

Because daemon and remote **both** same-origin serve the SPA, you **cannot** distinguish "am I on the daemon or the remote?" from the URL. The `isStandalonePWA()` / `display-mode` checks are also wrong here — a desktop browser hitting the remote domain is pwa-remote but not standalone.

## Two independent signals (don't conflate them)

### Signal 1 — Transport routing: `hasDeviceToken()` (localStorage)

Drives **auth injection + proxy prefix** in `httpTransport`:

- Token present → pwa-remote: `invoke` adds `Authorization: Bearer` + URL prefix `/api/v1/proxy`; EventSource appends `?access_token=`.
- No token → direct to daemon (browser-local / Tauri): current behavior, no auth, no prefix.

```ts
const token = getDeviceToken();
const proxyPrefix = token ? "/api/v1/proxy" : "";
const url = `${base}${proxyPrefix}/api/v1/${domain}/${cmd}`;
```

### Signal 2 — Navigation gating: `isRemoteContext()` (health probe)

Drives **whether pairing is required**. The bootstrap health handshake stores the body on `window.__DAEMON_HEALTH__`:

- `remoteId` field present → remote-served → pairing gate applies (no token → `/pairing`).
- `daemonId` / undefined → daemon or Tauri → straight to `/chat` (these never pair; the daemon has no `/api/v1/pairing/redeem` route, so gating them to `/pairing` is a **dead end**).

```ts
function isRemoteContext(): boolean {
  const h = (window as any).__DAEMON_HEALTH__;
  return !!h && "remoteId" in h;
}
```

> **Why two signals?** A user could be on the remote but not yet paired (remote context, no token → show pairing view). Or on the daemon with no token (not remote context → skip pairing entirely). Using only `hasDeviceToken()` for navigation locks out browser-local/Tauri users forever.

## Command routing: two paths, not one

| Command type | Path | Why |
|---|---|---|
| App commands (sessions, chat, projects…) | `transport.invoke()` → proxied when token present | Daemon commands, forwarded to PC via remote proxy |
| Pairing redeem, nodes list | Direct `fetch()` to remote-native endpoints | Different REST shape than `/{domain}/{cmd}`; remote-only, not daemon commands |

Don't shoehorn pairing/nodes into `CMD_TO_DOMAIN` — their URLs don't follow the daemon's `POST /api/v1/{domain}/{cmd}` convention.

## 401 handling: intercept in the transport, not the error bus

`transport.invoke` is the **single choke point** for all app commands. Intercept 401 there (clear token + reset EventSource + fire callback) regardless of whether the calling store catches the error.

The `errorBus` (`window.unhandledrejection`) only fires for **uncaught** errors; existing stores systematically `try/catch + console.error` invoke results, so a 401 would be swallowed silently. Don't rely on errorBus for auth-state transitions.

## Wire field casing

- Remote-native responses (`pairing.rs RedeemedResponse`, `nodes.rs NodeInfo`) use `#[serde(rename_all = "camelCase")]` → **camelCase** wire (`deviceToken`, `nodeId`, `displayName`).
- Request bodies follow the Rust struct's serde default (snake_case for `RedeemRequest`: `device_name`).
- `transport.invoke` auto-converts top-level arg keys camelCase→snake_case (daemon serde default); direct `fetch` does NOT — hand-write the right casing.
- **Command 形状铁律(08-21 质检实证)**:新 IPC 的 `#[tauri::command]` 参数必须**扁平标量**(Tauri 端 JS camelCase 自动映射 snake 参数;HTTP 端顶层 camel→snake 后作 body)——嵌套 `request: SomeStruct` 参数在 HTTP 模式下 body 变成 `{"request": …}` 而 daemon 路由结构体期望顶层字段,Option 字段全体静默 miss(不报错,语义漂移);对应地 **daemon 路由请求结构体用 snake_case 无 rename**(body 已被 invoke 转成 snake)。违例实例:`set_quota_settings` 初版(08-20 任务,质检抓出后改扁平 + snake 结构体)。
