# S4 Design — 配对流程 + PWA 壳(transport 增强 + vue-router + RemoteTab)

> **What/Why**:见 [prd.md](./prd.md)。本文是 **How**。
> **决策汇总**:[parent PRD](../08-11-remote-control-epic/prd.md)。
> **S1-S3 契约**:remote 已伺服 PWA 静态文件(`server.rs:53` ServeDir)+ auth 双通道(`auth.rs` Bearer + query)+ proxy 透传(`proxy.rs` `/api/v1/proxy/*path`)+ pairing/nodes API。S4 **不改任何 Rust**,纯前端。
> **代码库事实**(2026-08-12 探查):Vue3 + Pinia + reka-ui,**零路由** SPA;`httpTransport` 默认,`daemonBase()` PROD = `location.origin`;PC 端 4 个 remote IPC 已全就绪(`lib.rs::generate_handler!` + `CMD_TO_DOMAIN`);零 PWA / 零 auth 层 / 零 localStorage token。

---

## 0. 已确认决策(2026-08-12,实施前)

| # | 决策 | 结论 | 依据 |
|---|---|---|---|
| **D1** | PWA 多视图导航 | **vue-router**:三路由 `/pairing` `/nodes` `/chat` + `beforeEach` 守卫查 token | 用户裁决:URL 状态 + 后退键 + 深链。代价:引入 vue-router 依赖 + 现有 AppShell/ChatWindow 包进路由(改动入口) |
| **D2** | PWA tooling | **vite-plugin-pwa**:manifest + SW 自动生成,app-shell precache,`registerType: 'autoUpdate'` | Vite PWA 事实标准;手写 SW 无 MVP 收益且增维护面 |
| **D3** | transport 模式检测 | **`hasDeviceToken()`**:localStorage 有 `everlasting_device_token` = pwa-remote 模式 | 探查结论:`daemonBase()` 三模式 PROD 恒为 `location.origin`,baseURL **无差异**;真实差异是 auth 注入 + proxy 前缀,两者都以"有 token"为前提。原 PRD 的 `isStandalonePWA()` 检测不适用 —— PWA 在配对前无 token 也要工作,且 standalone 状态与"该走 proxy"无因果 |
| **D4** | 命令路由 | **pairing/nodes = 直接 `fetch`**(remote 自身端点);**其余 = `transport.invoke`**(pwa-remote 时加 `/api/v1/proxy` 前缀) | remote 仅挂 proxy 透传 + 自身 pairing/nodes;daemon 命令(sessions/chat…)经 proxy 到 PC |
| **D5** | token 存储 | **localStorage** `everlasting_device_token` | PRD 已定(MVP 接受 XSS;单用户自己的 PWA) |

### Review 修订记录(2026-08-12,design 评审后)

[review.md](./review.md) 6 条意见经**独立核验全部属实**(P1×1 + P2×2 + P3×3),design 各节已就地修订。关键自验(非复述 reviewer):P2-2 自读 `pairing.rs:63` `RedeemedResponse` 确认 `rename_all="camelCase"` wire 为 `{deviceToken, nodeId, nodeDisplayName}`;P3-1 自读 `config.rs:152` 确认 `TunnelStatusPayload` 无 `displayName` 字段;P1-1 推演 daemon ServeDir fallback 把 `POST /api/v1/pairing/redeem` 变 200+HTML → 死局。

| Review 项 | 修正 | 落地位置 |
|---|---|---|
| **P1-1** 守卫把无 token 一律赶 /pairing,锁死 browser-local / Tauri Thin(daemon 无 redeem 路由) | 守卫加 `isRemoteContext()` 前置:复用 bootstrap health 探针(remote 返 `remoteId`,daemon 返 `daemonId`);**仅 remote-served 且无 token 才跳 /pairing**,daemon/Tauri 直放 /chat | 新增 D6 + §5.1 守卫重写 |
| **P2-1** errorBus 只收未捕获异常,调用方系统性 catch+swallow → 401 静默 | 401 拦截改 `http.ts` invoke 的 `!resp.ok` 分支(模块级回调,transport 是唯一 choke point,必过);errorBus Auth toast 保留为提示但不依赖它跳转 | §6.2 重写 |
| **P2-2** redeem/nodes 解构 snake_case,实际 wire camelCase | §2.4 伪码改 camelCase 解构 + 补 nodes 字段清单 | §2.4 |
| **P3-1** RemoteTab display_name 无来源(TunnelStatusPayload 无此字段) | 节点信息区只显 node_id;display_name 删掉(拿不到,加字段 = 改 Rust 违约束) | §3.1 |
| **P3-2** vite-plugin-pwa ^0.20 不兼容 Vite 6 | 改 ^0.21(0.21+ 支持 Vite 6) | 附录 B |
| **P3-3** 完成标准计数 "7 项" 实为 6 + 测试数写死 | 改 "6 项";测试数改"与基线一致(零 Rust 回归)" | implement.md |

> **新增 D6**(P1-1 衍生):路由守卫需区分"remote 伺服"(需配对)与"daemon/Tauri 伺服"(不需配对)。判据 = bootstrap health body 的 `remoteId` 字段(仅 remote health 返回)。这补上了 D3 遗留的"origin 类型判定"缺口 —— D3 解决 transport 路由(auth/proxy 前缀用 token 判),D6 解决导航门禁(配对需求用 health 判)。

### 与原 PRD 的偏差

- **D3 修正了 PRD 的 `detectTransportMode()` 三态伪代码**:原伪代码用 `isStandalonePWA()` + `hasRemoteBaseURL()` 判 pwa-remote,但探查发现 `daemonBase()` PROD 恒为 `location.origin`(remote / daemon 都同源伺服 SPA),没有独立的 "remote baseURL" 概念。真实判据是 **token 是否存在**。详见 §2.3。
- PRD 的 `transport 切换 baseURL 到该节点` 说法不成立:手机选某个节点后**不切 baseURL**(baseURL 恒为 remote),token 已绑定 node(配对时绑定);所有请求经 proxy 由 remote 转发到该 node。详见 §2.4。

---

## 1. 架构总览

### 1.1 三种 transport 模式(探查后修正)

```
┌─────────────────────────────────────────────────────────────┐
│ 模式            │ 触发              │ daemonBase() │ auth │ proxy │
├─────────────────────────────────────────────────────────────┤
│ tauri(逃生)    │ ?transport=tauri  │ n/a(IPC)     │  —   │  —    │
│ browser-local   │ 无 token(PROD)   │ location.    │  无  │  无   │
│                 │ daemon 伺服 SPA   │  origin      │      │       │
│ pwa-remote      │ 有 token(PROD)   │ location.    │ Bearer│ /api  │
│                 │ remote 伺服 SPA   │  origin      │ +query│ /v1   │
│                 │                  │              │      │ /proxy│
└─────────────────────────────────────────────────────────────┘
```

**关键洞察**:三种模式 `daemonBase()` 在 PROD **完全相同**(`location.origin`)。差异只有两个开关,都由 `hasDeviceToken()` 驱动:
1. **auth 注入**:有 token → fetch 加 `Authorization: Bearer`;EventSource 加 `?access_token=`。
2. **proxy 前缀**:有 token → app 命令 URL 加 `/api/v1/proxy` 前缀(remote 透传到 PC)。

browser-local 与 pwa-remote 的区别 = `localStorage` 里有没有 token。同一个 SPA bundle,两种行为,运行时按 token 切换 —— **零构建分支,零独立入口**。

### 1.2 PWA 部署拓扑

```
手机 / 家里电脑浏览器                   云服务器 remote daemon            PC daemon
 GET https://remote.example.com         (everlasting-remote)              (sidecar / 独立)
   ├ ServeDir 返回 SPA(同源)            │                                 │
   │  manifest + SW(注册)              │                                 │
   │  vue-router → /pairing             │                                 │
   │  无 token → POST /api/v1/pairing/  │                                 │
   │              redeem ──────────────►│ devices 表 → 绑定 node_id ──────│ (配对码生成
   │  ◄ device_token ───────────────── │                                 │  在 PC 上做)
   │  存 localStorage                   │                                 │
   │  → /nodes                          │                                 │
   │  GET /api/v1/nodes (Bearer) ──────►│ nodes 表(online/offline)       │
   │  ◄ [{display_name, status, ...}]──│                                 │
   │  点在线节点 → /chat                 │                                 │
   │  transport.invoke("list_sessions") │                                 │
   │  POST /api/v1/proxy/api/v1/sessions/list ─►│ proxy → WSS Request ──►│ daemon 处理
   │                                          │ ◄ Response ────────────│
   │  ◄ [...sessions] ◄ Response ──────────────│                                 │
   │  EventSource /api/v1/proxy/api/v1/stream?access_token=... ─►│ proxy │ ◄ SSE chunk
   │  ◄ chat-event 流 ◄────────────────────────│ ← Stream 帧 ─────────────│
```

### 1.3 S4 新写 vs 复用(分界)

| 能力 | 状态 | 位置 |
|---|---|---|
| remote 静态伺服 + SPA fallback | ✅ S1 已做 | `remote/server.rs:53` |
| remote pairing redeem / nodes API | ✅ S1 已做 | `remote/routes/{pairing,nodes}.rs` |
| remote auth 双通道(Bearer + query) | ✅ S1 已做 | `remote/auth.rs` |
| remote proxy 透传 + SSE 桥接 + 取消 | ✅ S1/S3 已做 | `remote/routes/{proxy,ws}.rs` |
| PC 端 4 个 remote IPC | ✅ S2 已做 | `commands/{config,pairing}.rs` + `CMD_TO_DOMAIN` |
| **transport auth 注入 + proxy 前缀** | 🔴 S4 新写 | `app/src/transport/http.ts` |
| **transport token 存取** | 🔴 S4 新写 | `app/src/transport/auth.ts`(新) |
| **vue-router + 三视图** | 🔴 S4 新写 | `app/src/router/` + `views/` |
| **PC RemoteTab** | 🔴 S4 新写(纯 UI) | `app/src/components/settings/RemoteTab.vue` |
| **PWA 壳(manifest + SW + icons)** | 🔴 S4 新写 | `vite.config.ts` + `app/public/` |
| **PairingView / NodeListView** | 🔴 S4 新写 | `app/src/views/` |

**不改的东西**:任何 Rust 代码 / agent core / SseRegistry / 现有 store 的业务逻辑 / 现有 Settings 的 5 个 tab / Tauri 构建配置(tauri.conf.json)。

---

## 2. transport 增强(核心)

### 2.1 token 存取(`transport/auth.ts`,新文件)

```ts
const TOKEN_KEY = "everlasting_device_token";

export function getDeviceToken(): string | null {
  try { return localStorage.getItem(TOKEN_KEY); } catch { return null; }
}
export function setDeviceToken(token: string): void {
  try { localStorage.setItem(TOKEN_KEY, token); } catch {}
}
export function clearDeviceToken(): void {
  try { localStorage.removeItem(TOKEN_KEY); } catch {}
}
export function hasDeviceToken(): boolean {
  return getDeviceToken() !== null;
}
```

localStorage 访问 try/catch(隐私模式 / 禁用 cookie 时抛 SecurityError,fallback null 不 crash —— 对齐现有 `stores/config.ts` 的 localStorage 访问惯例)。

### 2.2 `http.ts` 改动:auth 注入 + proxy 前缀

**invoke 改动**(diff 视角,非整文件):

```ts
// 现状(http.ts:270-282)
const domain = CMD_TO_DOMAIN[cmd];
const url = `${daemonBase()}/api/v1/${domain}/${cmd}`;
const resp = await fetch(url, {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(transformArgsTopLevel(args)),
});

// S4 后
const domain = CMD_TO_DOMAIN[cmd];
const base = daemonBase();
const token = getDeviceToken();
// pwa-remote(token 存在):app 命令经 proxy 透传到 PC。
// browser-local(无 token):直连 daemon(现状不变)。
const proxyPrefix = token ? "/api/v1/proxy" : "";
const url = `${base}${proxyPrefix}/api/v1/${domain}/${cmd}`;
const headers: Record<string, string> = { "Content-Type": "application/json" };
if (token) headers["Authorization"] = `Bearer ${token}`;
const resp = await fetch(url, { method: "POST", headers, body: ... });
```

**EventSource 改动**(diff 视角):

```ts
// 现状(http.ts:244-247)
function ensureEventSource(): void {
  if (eventSource) return;
  const url = `${daemonBase()}/api/v1/stream`;
  eventSource = new EventSource(url);
}

// S4 后
function ensureEventSource(): void {
  if (eventSource) return;
  const base = daemonBase();
  const token = getDeviceToken();
  // pwa-remote:SSE 经 proxy + access_token query(EventSource 不能设 header,
  // remote auth.rs 的 query 通道);browser-local:直连 daemon(现状)。
  const url = token
    ? `${base}/api/v1/proxy/api/v1/stream?access_token=${encodeURIComponent(token)}`
    : `${base}/api/v1/stream`;
  eventSource = new EventSource(url);
}
```

**EventSource 重建时机**:token 变化(配对成功 / 登出)后,旧 EventSource(无 token 的直连)需关闭重建。`ensureEventSource` 当前是"已存在则跳过"。S4 加一个 `resetEventSource()` 供配对/登出后调用:close 现有 + 清引用 → 下次 `listen` 触发 `ensureEventSource` 用新 token 重建。

```ts
// 供 pairing/nodes store 在 token 变化后调
export function resetEventSource(): void {
  if (eventSource) { eventSource.close(); eventSource = null; }
}
```

**为什么 proxy 前缀只加在 app 命令,不加在 pairing/nodes**:pairing/nodes 是 remote 自身端点(`remote/routes/{pairing,nodes}.rs`),不经过 proxy(D4)。但它们不通过 `transport.invoke` 调用 —— 见 §2.4,用直接 fetch。所以 `transport.invoke` 里的 proxy 前缀对 pairing/nodes 无影响(它们不走这条路)。

### 2.3 模式检测为何用 token 而非 `isStandalonePWA()`

原 PRD 伪代码:
```ts
if (isStandalonePWA() && hasRemoteBaseURL()) return 'pwa-remote';
```

**不成立的原因**:
1. **`hasRemoteBaseURL()` 无从定义**:`daemonBase()` PROD 恒为 `location.origin`。daemon(browser-local)和 remote(pwa-remote)都同源伺服 SPA,`location.origin` 无法区分"这是 daemon 还是 remote"。
2. **`isStandalonePWA()` 与 proxy 路由无因果**:家里电脑浏览器(非 standalone display-mode)打开 remote 域名也是 pwa-remote(需 auth + proxy)。`isStandalonePWA()` 只影响"能否添加到主屏幕"的提示,不影响 transport 路由。
3. **配对前 PWA 无 token 也要工作**:配对码输入界面调 `/api/v1/pairing/redeem`(直接,无 auth)。此时无 token → transport 走 browser-local 行为(直连)—— 但 redeem 本就是直接 fetch,不经过 transport.invoke,所以无冲突。

**token 是唯一可靠的 pwa-remote 信号**:有 token = 已配对 = 经 remote 代理 = 需要 auth + proxy。配对前(无 token)= 等同 browser-local 行为(redeem 用直接 fetch)。

`isStandalonePWA()` 保留一个用途(§4):PWA 安装提示 UI(非 transport 路由)。

### 2.4 pairing / nodes 用直接 fetch(不走 transport.invoke)

remote-native 端点 shape 与 daemon 命令不同,且 URL 不经 proxy:

```ts
// app/src/stores/pairing.ts(新)
async function redeem(code: string, deviceName: string): Promise<string> {
  const resp = await fetch(`${daemonBase()}/api/v1/pairing/redeem`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, device_name: deviceName }),  // 请求体 snake_case(pairing.rs:57 RedeemRequest serde 默认)
  });
  if (!resp.ok) throw new Error(...);
  // 响应 wire 是 camelCase(pairing.rs:63 RedeemedResponse rename_all="camelCase")
  const { deviceToken, nodeId, nodeDisplayName } = await resp.json();
  setDeviceToken(deviceToken);   // ← 触发 pwa-remote 模式
  resetEventSource();             // ← 旧 ES(无 token)关闭,下次 listen 重建
  return nodeId;
}
```

```ts
// app/src/stores/nodes.ts(新)
// wire shape(nodes.rs:26 NodeInfo rename_all="camelCase"):
//   [{ nodeId, displayName, status, lastSeenAt }]  status: "online"|"offline"
interface NodeInfo {
  nodeId: string;
  displayName: string;
  status: "online" | "offline";
  lastSeenAt: number;  // unix epoch ms
}
async function loadNodes(): Promise<NodeInfo[]> {
  const token = getDeviceToken();
  const resp = await fetch(`${daemonBase()}/api/v1/nodes`, {
    headers: token ? { "Authorization": `Bearer ${token}` } : {},
  });
  if (!resp.ok) throw new Error(...);
  return resp.json();
}
```

**为什么不用 transport.invoke**:`CMD_TO_DOMAIN` 是 daemon 命令 → domain 的映射;`redeem` / `list_nodes` 在 remote 上是独立 REST 端点(`/api/v1/pairing/redeem` / `/api/v1/nodes`),与 daemon 的 `/api/v1/{domain}/{cmd}` 结构不同。硬塞进 transport.invoke 会污染映射表 + URL 拼接逻辑。直接 fetch 更清晰,且这两个 store 是 S4 新增,不碰现有 store。

---

## 3. PC RemoteTab(Settings 新 tab)

### 3.1 结构(照抄 MemoryTab/SubagentsTab 模式)

`components/settings/RemoteTab.vue` + `stores/remoteConfig.ts`(新 Pinia store):

```
RemoteTab.vue
├ 配置区
│  ├ remote_url 输入框(预填 get_remote_config 的值)
│  ├ shared_secret 输入框(type=password)
│  └ 保存按钮 → transport.invoke("set_remote_config", { remoteUrl, sharedSecret })
├ 连接状态区
│  ├ 状态指示(已连接🟢 / 重连中🟡 / 未配置⚪ / 认证失败🔴)
│  └ 轮询 get_tunnel_status(或 onMounted 起 2s 间隔轮询,组件卸载停)
├ 配对码区
│  ├ "生成配对码"按钮 → transport.invoke("generate_pairing_code")
│  ├ 6 位码大字显示 + 60s 倒计时
│  └ "在手机上打开 {remote_url} 并输入此码"提示
└ 节点信息区
   └ node_id(只读,从 get_tunnel_status;P3-1:TunnelStatusPayload 只有
     {connected, remoteUrl, nodeId, lastError},无 displayName —— 显示名
     只在 remote nodes 表,S4 纯前端拿不到;加字段 = 改 Rust 违约束,留 V2)
```

**store 模式**(对齐 `providers.ts` 组合式 API):
```ts
const remoteConfigStore = defineStore("remoteConfig", () => {
  const config = ref<{remoteUrl?: string; sharedSecret?: string} | null>(null);
  const status = ref<TunnelStatus | null>(null);
  const pairingCode = ref<{code: string; expiresAt: number} | null>(null);
  async function load() { config.value = await transport.invoke("get_remote_config"); }
  async function save(remoteUrl: string, sharedSecret: string) { ... }
  async function refreshStatus() { status.value = await transport.invoke("get_tunnel_status"); }
  async function generateCode() { const r = await transport.invoke("generate_pairing_code"); pairingCode.value = {code: r.code, expiresAt: Date.now() + r.expiresIn*1000}; }
  return { config, status, pairingCode, load, save, refreshStatus, generateCode };
});
```

### 3.2 挂载(SettingsModal.vue 加 tab)

```vue
<!-- SettingsModal.vue,照抄现有 TabsTrigger/TabsContent 模式 -->
<TabsTrigger value="remote">Remote</TabsTrigger>
...
<TabsContent value="remote"><RemoteTab /></TabsContent>
```

import + 两行模板。`default-value` 不改(仍 `providers`)。

### 3.3 仅在 daemon 模式可见

RemoteTab 只在 PC daemon(Tauri Thin / 浏览器连 daemon)有意义 —— 手机 PWA 不配 remote。但 Settings 模态在 PWA 上也不会被打开(手机走 PairingView/NodeListView,不进 AppShell)。故无需条件隐藏;若担心,加 `v-if="!hasDeviceToken()"` 守卫(手机 token 存在时不显示 Remote tab)。

---

## 4. PWA 壳(vite-plugin-pwa)

### 4.1 vite.config.ts 加插件

```ts
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    vue(),
    tailwindcss(),
    VitePWA({
      registerType: "autoUpdate",        // SW 更新自动激活
      injectRegister: "auto",            // index.html 自动注入注册脚本
      manifest: {
        name: "Everlasting",
        short_name: "Everlasting",
        description: "远程遥控你的 AI agent",
        theme_color: "#1a1a1a",
        background_color: "#1a1a1a",
        display: "standalone",
        start_url: "/",
        icons: [
          { src: "/icons/192.png", sizes: "192x192", type: "image/png" },
          { src: "/icons/512.png", sizes: "512x512", type: "image/png" },
          { src: "/icons/512-maskable.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
        ],
      },
      workbox: {
        // app-shell precache:HTML/CSS/JS/字体离线可加载。
        // 数据(API/SSE)永远 network-only(应用本质在线)。
        globPatterns: ["**/*.{js,css,html,woff2,png,svg}"],
        navigateFallback: "/index.html",   // SPA history fallback
        navigateFallbackDenylist: [/^\/api\//],  // API 不走 fallback(返真实 4xx)
        runtimeCaching: [],  // MVP 不做 runtime cache(数据全在线)
      },
      devOptions: { enabled: false },  // dev 不启用 SW(避免缓存干扰 HMR)
    }),
  ],
  // base 不设(默认 "/")—— remote 根路径部署。子路径部署时运维设 base。
});
```

**为什么 `navigateFallbackDenylist: [/^\/api\//]`**:SW 的 navigateFallback 会把所有导航请求 fallback 到 index.html。但 API 请求(`/api/v1/...`)不能被 fallback(否则 404 变成返 index.html HTML,前端拿到 HTML 而非 JSON)。denylist 排除 API 路径。

### 4.2 icons

`app/public/icons/` 放 3 个占位图标(192/512/512-maskable)。MVP 用简单 monogram(首字母"E"深色底);V2 设计师出正式图标。生成方式:用 sharp/imagemin 从一个 SVG 生成多尺寸,或手放占位 PNG。

### 4.3 index.html

vite-plugin-pwa 的 `injectRegister: "auto"` 自动注入 SW 注册脚本,无需手改 index.html。但手动加 PWA meta 提升体验(iOS Safari 的 standalone 支持):

```html
<head>
  <meta name="theme-color" content="#1a1a1a">
  <meta name="apple-mobile-web-app-capable" content="yes">
  <meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">
  <link rel="apple-touch-icon" href="/icons/192.png">
</head>
```

### 4.4 SW 更新提示

`registerType: "autoUpdate"` 下,新版本 SW 会在 `controllerchange` 时激活。MVP 不做"有更新请刷新"提示(autoUpdate 静默接管);V2 加 `useRegisterSW` 的更新提示弹窗。

---

## 5. vue-router + 三视图

### 5.1 路由表

```ts
// app/src/router/index.ts(新)
import { createRouter, createWebHistory } from "vue-router";
import { hasDeviceToken } from "../transport/auth";

// D6(P1-1):区分"remote 伺服"(需配对)与"daemon/Tauri 伺服"(不需配对)。
// 判据 = bootstrap health body:remote 返 remoteId(`health.rs:44`),
// daemon 返 daemonId。main.ts 把 health 存进 window.__DAEMON_HEALTH__。
// ?transport=tauri 逃生跳过 health → undefined → 视为 daemon 上下文(直放 /chat)。
function isRemoteContext(): boolean {
  const h = (window as unknown as { __DAEMON_HEALTH__?: Record<string, unknown> }).__DAEMON_HEALTH__;
  return !!h && "remoteId" in h;
}

const routes = [
  { path: "/pairing", name: "pairing", component: () => import("../views/PairingView.vue") },
  { path: "/nodes", name: "nodes", component: () => import("../views/NodeListView.vue") },
  { path: "/chat", name: "chat", component: () => import("../views/ChatView.vue") },
  { path: "/", redirect: () => {
      if (!isRemoteContext()) return "/chat";     // daemon/Tauri:直进应用(现状)
      if (!hasDeviceToken()) return "/pairing";    // remote 未配对
      return "/nodes";                             // remote 已配对未选节点
    }},
  { path: "/:catchAll(.*)", redirect: "/" },
];

export const router = createRouter({ history: createWebHistory(), routes });

// 守卫(D6):配对门禁仅对 remote 上下文生效 —— browser-local / Tauri Thin
// 永远无 token 也直放(P1-1:否则 daemon 无 redeem 路由 = 死局)。
router.beforeEach((to) => {
  if (to.name === "pairing") return true;               // 配对页始终可达
  if (!isRemoteContext()) {
    // daemon/Tauri 上下文:不需配对,直进 chat(现状行为)
    return to.name === "chat" ? true : { name: "chat" };
  }
  // remote 上下文:token + selectedNode 门禁
  if (!hasDeviceToken()) return { name: "pairing" };
  if (to.name === "chat" && !useNodesStore().selectedNodeId) return { name: "nodes" };
  return true;
});
```

### 5.2 main.ts + App.vue 改动

```ts
// main.ts 改动:加 router
app.use(createPinia());
app.use(router);          // ← 新增
// bootstrap() 不变(awaitDaemonHealthy 仍跑;remote health 兼容,§0 已确认)
```

```vue
<!-- App.vue 改动:AppShell+ChatWindow 移到 ChatView,根挂 router-view -->
<script setup lang="ts">
// 全局错误处理 / SSE 生命周期不再放这里(移到 ChatView,§5.4)
</script>
<template>
  <router-view />
</template>
```

**为什么 streamController 从 App.vue 移到 ChatView**:SSE 流只在聊天视图需要。配对/节点列表时不应挂 stream(浪费隧道连接 + 权限)。ChatView 的 onMounted 启动、onUnmounted 停止。

### 5.3 PairingView

```
PairingView.vue
├ 居中卡片
│  ├ 标题"配对你的设备"
│  ├ 6 位码输入框(6 格 OTP 风格,自动聚焦下一格)
│  ├ 设备名输入框(可选,默认 "手机" / navigator.userAgent 推断)
│  ├ "配对"按钮 → pairingStore.redeem(code, deviceName)
│  │   ├ 成功 → setDeviceToken + resetEventSource → router.push("/nodes")
│  │   └ 失败 → 红色提示(码无效 / 已过期)
│  └ 提示"在 PC 的 Settings → Remote → 生成配对码"
└ (若 isStandalonePWA() 为 false)非手机环境提示"可添加到主屏幕"
```

redeem 失败的 400/404 → 友好中文提示;网络错误 → "无法连接到服务器"。

### 5.4 NodeListView

```
NodeListView.vue
├ 顶栏(标题"选择设备" + 登出按钮 → clearDeviceToken → router.push("/pairing"))
└ 节点卡片列表(nodesStore.nodes)
   每张卡片:
   ├ display_name
   ├ 状态点(online 🟢 / offline ⚪)
   ├ 最后在线时间(相对时间,"3 分钟前")
   └ 点击:
      ├ online → nodesStore.selectNode(nodeId) → router.push("/chat")
      └ offline → toast "该 PC 离线,无法连接"
```

**selectNode 做什么**(D4 修正):**不切 baseURL**(baseURL 恒为 remote)。token 已在配对时绑定 node_id(remote `devices` 表);所有请求经 proxy 由 remote 按 token → node_id → WSS 转发。selectNode 只记 `selectedNodeId`(store 状态,供守卫 + ChatView 知道用户选了谁)+ 跳路由。

```ts
// stores/nodes.ts
const selectedNodeId = ref<string | null>(null);
function selectNode(nodeId: string) {
  selectedNodeId.value = nodeId;
  try { localStorage.setItem("everlasting_selected_node", nodeId); } catch {}
}
// 初始化时恢复(localStorage 持久化上次选择,免每次重选)
```

### 5.5 ChatView(现有 app 包入路由)

```vue
<!-- app/src/views/ChatView.vue(新,搬 App.vue 的内容) -->
<script setup lang="ts">
import { onMounted, onUnmounted } from "vue";
import AppShell from "../components/layout/AppShell.vue";
import ChatWindow from "../components/ChatWindow.vue";
import { useStreamControllerStore } from "../stores/streamController";

const streamController = useStreamControllerStore();
onMounted(() => void streamController.start());
onUnmounted(() => streamController.stop());
</script>
<template>
  <AppShell><ChatWindow /></AppShell>
</template>
```

现有 App.vue 的 27 行逻辑 1:1 搬到这里。App.vue 只剩 `<router-view />`。

---

## 6. token 生命周期

### 6.1 配对(redeem)成功流

```
PairingView → POST /api/v1/pairing/redeem
  ◄ { device_token, node_id, node_display_name }
  → setDeviceToken(token)         // localStorage 写入 → pwa-remote 模式激活
  → resetEventSource()             // 关旧 ES(若有)
  → router.push("/nodes")          // 守卫放行(hasDeviceToken=true)
```

### 6.2 token 失效(吊销 / 过期)

remote `auth.rs` 对无效/吊销 token 返 401(`category: "Auth"`)。两类捕获:

1. **transport.invoke 收到 401**(P2-1 修订:拦截放 transport 层,不依赖 errorBus):在 `http.ts` invoke 的 `!resp.ok` 分支,**401 时先做副作用再抛错** —— `clearDeviceToken()` + `resetEventSource()` + 触发模块级 `onAuthFailed` 回调(由 App 注册:`router.push("/pairing")`)。transport 是全部 app 命令的唯一 choke point,无论调用方 catch 与否都必过此处。

   ```ts
   // http.ts invoke 的 !resp.ok 分支(P2-1)
   if (resp.status === 401 && getDeviceToken()) {
     clearDeviceToken();
     resetEventSource();
     onAuthFailed?.();  // 模块级回调,App 注册 → router.push("/pairing")
   }
   throw new TransportError(resp.status, body);
   ```

   **为什么不挂 errorBus**(P2-1):`main.ts` 的 errorBus 只从 `window.error`/`unhandledrejection` 收事件;现有调用点系统性 `try/catch + console.error` 吞掉 invoke 错误(如 ProvidersTab.vue)→ 401 到不了 unhandledrejection → 全局 Auth 处理器收不到 → 静默失败。transport 层拦截保证必触发。errorBus 的 Auth toast 保留(给用户提示),但**跳转不依赖它**。

2. **EventSource 收到 401**:浏览器 EventSource 遇 HTTP 错误码**直接 fail 且不重连**(S3 design P3-1 已记录)。`eventSource.onerror` 触发 → S4 加:onerror 时探测 token 有效性(GET /api/v1/nodes 带 token → 401 = 失效)→ 清 token → 跳 pairing。**MVP 简化**:onerror 只 console.warn(现状),token 失效靠下次 invoke 401 捕获 —— 因为 EventSource 401 后流已停,用户下次操作必然触发 invoke。

### 6.3 登出

NodeListView 登出按钮 → `clearDeviceToken()` + `resetEventSource()` + 清 selectedNode + `router.push("/pairing")`。守卫拦截后续导航。

---

## 7. 兼容性(不破现状)

### 7.1 Tauri GUI(Thin 模式)

- `?transport=tauri` 逃生:不受影响(tauriTransport 不读 token / 不加 proxy)。
- Thin 默认 httpTransport:无 token(localStorage 空)→ proxy 前缀为空 + 无 auth header → 行为 = 现状(直连 daemon)。✅
- RemoteTab 新增:Settings 多一个 tab,不影响其他 tab。

### 7.2 家里电脑浏览器(browser-local)

- 无 token → 直连 daemon(现状)。✅
- 若用户在 remote 域名(非 daemon)打开:同 pwa-remote 流程(配对 → token → proxy)。这是 PRD 验收"家里电脑浏览器经 remote"的场景 —— 大屏直接用三栏布局,零额外适配。

### 7.3 现有测试

- `transport/health.ts` + `http.ts` 有单测,改动后需补:proxy 前缀(有/无 token)、auth header(有/无 token)、EventSource URL(token query)。
- 新增 `router/` 无测试负担(MVP 手测;vue-router 守卫逻辑简单)。

### 7.4 构建产物

vite-plugin-pwa 在 `pnpm build` 时生成 SW(`sw.js` + `workbox-*.js`)+ manifest 注入。这些进 `app/dist/`,remote 的 `ServeDir` 伺服它们(daemon 的 ServeDir 也伺服,无冲突 —— Tauri GUI 加载同一 bundle,SW 在非 PWA 上下文注册也无害,但为干净可 `devOptions.enabled=false` + 仅 prod 生效)。

---

## 8. 关键权衡

| 决策 | 选择 | 理由 | 否决项 |
|---|---|---|---|
| 导航 | vue-router | URL 状态 + 后退键 + 深链(用户裁决 D1) | gate 条件渲染:无 URL 状态,后退键不作用于视图切换 |
| PWA tooling | vite-plugin-pwa | 事实标准,manifest+SW 自动 | 手写 SW:增维护面,无 MVP 收益 |
| 模式检测 | hasDeviceToken() | baseURL 三模式无差异,token 是唯一可靠信号 | isStandalonePWA():与 proxy 路由无因果,家里浏览器非 standalone 也需 proxy |
| pairing/nodes | 直接 fetch | remote-native 端点 shape 不同于 daemon 命令 | 塞 transport.invoke:污染 CMD_TO_DOMAIN + URL 拼接 |
| SSE 生命周期 | ChatView onMounted/onUnmounted | 配对/节点列表不需挂 stream | App.vue 全局常驻:浪费隧道连接 |
| SW 数据缓存 | 不做(runtimeCaching:[]) | 应用本质在线,缓存 API 数据有一致性风险 | runtime cache:复杂度高,过期/失效处理 MVP 不做 |
| 401 全局处理 | http.ts invoke 401 拦截(模块级回调)→ 清 token 跳 pairing | transport 是唯一 choke point,调用方 catch 与否都必过(P2-1) | errorBus:只收未捕获异常,调用方系统性 catch → 静默失败 |

### D1(vue-router)的代价

引入 vue-router 依赖(~15KB gz)+ 入口重构(App.vue → router-view + ChatView)。现有 AppShell/ChatWindow 需包进路由组件。这是用户选的"做正"路线 —— 换来真移动端导航体验(后退键、深链、URL 可分享)。S5 移动适配时路由已就位,无需补。

---

## 9. 运营 / 回滚

### 9.1 部署

1. `cd app && pnpm build` → `app/dist/`(含 SW + manifest + icons)。
2. scp `dist/` + remote 二进制到云服务器(同目录)。
3. remote 启动 → `resolve_dist_dir()` 找到 `./dist` → ServeDir 伺服 PWA。
4. PC daemon 配 remote_url + secret → 生成配对码 → 手机输入。

### 9.2 回滚

删 S4 前端改动:回退 `http.ts`(去 auth/proxy)、删 `transport/auth.ts`、`router/`、`views/`、`RemoteTab.vue`、`vite-plugin-pwa` 配置 + `public/`。Rust 零改动,S1-S3 隧道管线保留(非流式 + SSE 全通,只是手机 PWA 不可用)。

---

## 附录 A:S4 验收 checklist(对应 PRD)

| PRD 验收项 | S4 实现位置 |
|---|---|
| PC Settings → Remote tab:配 remote_url + secret + 生成配对码 + 看连接状态 | RemoteTab.vue + remoteConfig store(§3) |
| 手机打开 remote 域名 → 加载 PWA → 添加到主屏幕 → standalone | vite-plugin-pwa manifest(§4)+ remote ServeDir(S1) |
| 首次(无 token)→ 配对码输入 → 配对成功 → 跳节点列表 | PairingView + pairing store redeem(§5.3/§6.1) |
| 节点列表显示已配对 PC + 在线状态;点在线节点进完整前端 | NodeListView + nodes store(§5.4) |
| 完整前端在手机上功能可用 | ChatView 复用 AppShell+ChatWindow(§5.5);布局适配留 S5 |
| token 失效 → 自动跳回配对 | errorBus 401 handler(§6.2) |
| 离线节点点进去显示离线提示 | NodeListView click handler(§5.4) |
| 家里电脑浏览器经 remote → 配对 → 三栏前端 | 同 pwa-remote 流程,大屏用现有布局(§7.2) |

## 附录 B:依赖新增

- `vue-router` (^4) — D1 导航
- `vite-plugin-pwa` (^0.21) — D2 PWA 壳(devDependency,仅构建期;P3-2:0.21+ 才支持 Vite 6,项目 vite ^6.0.3)
- 无新增 Rust 依赖 / 无新增 runtime 前端依赖(vue-router 是唯一 runtime dep)
