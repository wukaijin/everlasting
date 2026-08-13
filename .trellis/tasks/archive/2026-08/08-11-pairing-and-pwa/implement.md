# S4 Implement — 配对流程 + PWA 壳实施顺序

> **What/Why**:[prd.md](./prd.md) · **How**:[design.md](./design.md)
> **已确认决策**(2026-08-12):vue-router(D1)、vite-plugin-pwa(D2)、hasDeviceToken 模式检测(D3)、pairing/nodes 直接 fetch(D4)、localStorage token(D5)。详见 design §0。
> **执行原则**:每步独立 commit + 独立验证。Rust 零改动(纯前端)。Step 2 是核心(transport 接通),Step 3-5 是 UI 层,Step 6 PWA 壳,Step 7 全量验证。
> **硬约束**(design §7):不破 Tauri Thin 逃生 / 不破 browser-local 直连 / 不改任何 Rust / 不改现有 Settings 5 tab 行为。

---

## Step 1:transport/auth.ts + http.ts 增强(核心,D3/D4)

**目的**:让 httpTransport 在 pwa-remote 模式(有 token)注入 auth + 加 proxy 前缀 —— 隧道接通的前提。

**动作**:
1. 新建 `app/src/transport/auth.ts`:`getDeviceToken` / `setDeviceToken` / `clearDeviceToken` / `hasDeviceToken`(localStorage try/catch,design §2.1)。
2. `http.ts` invoke 分支(D4):`const token = getDeviceToken();` → 有 token 加 `Authorization: Bearer` + URL 前缀 `/api/v1/proxy`;无 token 行为不变(design §2.2)。
3. `http.ts` `ensureEventSource`(D4):有 token → URL 加 `/api/v1/proxy` + `?access_token=`;无 token 不变。
4. `http.ts` 新增 `resetEventSource()`(关闭 + 清引用,供 token 变化后重建)。
5. **401 拦截**(P2-1):invoke 的 `!resp.ok` 分支,`status===401 && getDeviceToken()` → `clearDeviceToken()` + `resetEventSource()` + `onAuthFailed?.()`(模块级回调,Step 5 由 App 注册 `router.push("/pairing")`)。暴露 `export function setOnAuthFailed(cb: () => void)` 供注册。
6. 补单测(`app/src/transport/http.test.ts`):有/无 token 的 URL 构造、header 注入、EventSource URL、**401 拦截触发 clearDeviceToken + 回调**(mock fetch 返 401)。

**验证**:
```bash
cd app && pnpm test -- http   # transport 单测
cd app && pnpm exec vue-tsc --noEmit   # 类型检查
```
**期望**:有 token → URL 含 `/api/v1/proxy` + `Authorization` 头;无 token → URL/header 不变(回归);401+有token → 清 token + 回调触发。

**commit**:`feat(transport): pwa-remote 模式 auth 注入 + proxy 前缀(S4 隧道接通)`

---

## Step 2:vue-router + 路由骨架(D1)

**目的**:给零路由 SPA 引入路由,三视图 + token 守卫。现有 app 先原样跑(ChatView 搬 App.vue 内容)。

**动作**:
1. `pnpm add vue-router@4`。
2. 新建 `app/src/router/index.ts`:三路由 `/pairing` `/nodes` `/chat` + `/` redirect + catchAll(design §5.1)。组件用懒 import,但本步先用占位(`<div>TODO</div>`)—— 真视图 Step 3-5 填。
3. `main.ts`:`app.use(router)`。
4. 新建 `app/src/views/ChatView.vue`:1:1 搬 App.vue 的 `<AppShell><ChatWindow/></AppShell>` + streamController onMounted/onUnmounted(design §5.5)。
5. `App.vue` 改为 `<router-view />`(去 AppShell/ChatWindow/streamController,搬 ChatView)。
6. 守卫 `beforeEach`(P1-1/D6):加 `isRemoteContext()`(读 `window.__DAEMON_HEALTH__` 的 `remoteId` 字段)—— **仅 remote 上下文才查 token 门禁**;daemon/Tauri 直放 /chat。selectedNodeId 暂从 localStorage 读(Step 5 store 落地后接 store)。

**验证**:
```bash
cd app && pnpm exec vue-tsc --noEmit
cd app && pnpm build          # 构建过(路由懒 chunk 生成)
```
**期望**:构建过;`/` redirect 到 `/pairing`(无 token);ChatView 在 `/chat` 渲染现有 app(streamController 正常)。

**commit**:`feat(router): vue-router 骨架 + ChatView(现有 app 包入路由)`

---

## Step 3:PC RemoteTab(纯 UI,design §3)

**目的**:PC Settings 加 Remote tab —— 配 remote + 看状态 + 生成配对码。

**动作**:
1. 新建 `app/src/stores/remoteConfig.ts`(Pinia 组合式,design §3.1):config / status / pairingCode + load/save/refreshStatus/generateCode。
2. 新建 `app/src/components/settings/RemoteTab.vue`:配置区 + 状态区(2s 轮询 status,组件卸载停)+ 配对码区(60s 倒计时)+ 节点信息区。照抄 MemoryTab/SubagentsTab 的 reka-ui 风格。
3. `SettingsModal.vue`:加 `<TabsTrigger value="remote">` + `<TabsContent value="remote"><RemoteTab/></TabsContent>` + import。
4. 配对码倒计时:`setInterval` 每秒减,过期清空 + 提示重新生成。

**验证**:
```bash
cd app && pnpm exec vue-tsc --noEmit
# 手测(需跑 daemon):cd app && pnpm dev:daemon & pnpm dev → Settings → Remote tab
```
**期望**:能配 remote_url + secret 保存;状态轮询显示;生成配对码 + 倒计时。

**commit**:`feat(settings): RemoteTab(remote 配置 + 配对码 + 状态)`

---

## Step 4:PairingView + pairing store(手机配对码输入,design §5.3/§6.1)

**目的**:手机首次打开 → 输码 → 拿 token → 跳节点列表。

**动作**:
1. 新建 `app/src/stores/pairing.ts`:`redeem(code, deviceName)` → 直接 `fetch POST /api/v1/pairing/redeem`(D4,不走 transport.invoke)→ 解构 **camelCase** `{ deviceToken, nodeId, nodeDisplayName }`(P2-2:wire 是 `rename_all="camelCase"`,非 snake_case)→ `setDeviceToken` + `resetEventSource`。
2. 新建 `app/src/views/PairingView.vue`:6 格 OTP 输入 + 设备名 + 配对按钮。成功 → `router.push("/nodes")`;失败 → 中文提示。
3. redeem 的错误处理:400(码无效/过期)/ 404 / 网络错误 → 各自友好提示。

**验证**:
```bash
cd app && pnpm exec vue-tsc --noEmit
# 手测:启 remote(指向 daemon dist)+ 启 PC daemon 配 remote → PC 生成码 → 浏览器开 remote 域名 → 输码 → 跳 /nodes
```
**期望**:输码成功 → localStorage 有 token → 跳 /nodes。

**commit**:`feat(pwa): PairingView + 配对码 redeem→token 流`

---

## Step 5:NodeListView + nodes store + 401 全局处理(design §5.4/§6.2)

**目的**:节点列表 + 选节点进 chat + token 失效自动跳配对。

**动作**:
1. 新建 `app/src/stores/nodes.ts`:`loadNodes()`(直接 `fetch GET /api/v1/nodes` + Bearer,D4)+ `selectNode(id)`(记 selectedNodeId + localStorage)+ 登出(clearDeviceToken + resetEventSource + 清 selectedNode)。
2. 新建 `app/src/views/NodeListView.vue`:节点卡片(状态点 + display_name + 相对时间)+ 点击 online→selectNode→`/chat`,offline→toast。登出按钮。
3. 401 注册(P2-1):App.vue onMounted 调 `setOnAuthFailed(() => router.push("/pairing"))` —— 把 Step 1 的 transport 回调接到 router。**不放 errorBus**(只收未捕获异常,调用方 catch 会吞掉)。
4. router 守卫接 nodes store 的 selectedNodeId(替换 Step 2 的 localStorage 直读)。

**验证**:
```bash
cd app && pnpm exec vue-tsc --noEmit
# 手测:配对后 → 节点列表显示 PC + 在线 → 点进 chat → 现有前端可用
#       token 失效:remote 侧吊销 token(devtools 改 localStorage 为无效值)→ 下次操作 → 跳配对
```
**期望**:节点列表正确;选节点进 chat;401 自动跳配对。

**commit**:`feat(pwa): NodeListView + 节点选择 + 401 全局跳配对`

---

## Step 6:PWA 壳(vite-plugin-pwa,design §4)

**目的**:manifest + SW + icons → 可安装 + 离线 app shell。

**动作**:
1. `pnpm add -D vite-plugin-pwa`。
2. `vite.config.ts` 加 `VitePWA({...})`(design §4.1):manifest + workbox(navigateFallback + denylist `/api/`)+ devOptions.enabled=false。
3. `app/public/icons/`:放 192/512/512-maskable 占位图标(从一个 SVG 生成)。
4. `index.html`:加 PWA meta(theme-color / apple-mobile-web-app-capable / apple-touch-icon)。
5. `pnpm build` 验证 SW + manifest 生成在 dist/。

**验证**:
```bash
cd app && pnpm build   # 期望 dist/ 有 sw.js + manifest.webmanifest + registerSW.js
cd app && pnpm preview # 浏览器 devtools → Application → Manifest 显示 + SW 注册
```
**期望**:manifest 可识别;SW 注册;手机"添加到主屏幕"→ standalone 启动。

**commit**:`feat(pwa): manifest + service worker + icons(vite-plugin-pwa)`

---

## Step 7:全量验证 + 回归

**目的**:零回归 + 类型/lint 全绿 + 端到端手测。

**动作**:
```bash
cd app && pnpm exec vue-tsc --noEmit           # 类型检查全绿
cd app && pnpm test                              # 前端单测全绿(含 Step 1 transport 新测)
cd app && pnpm build                             # 构建过(SW + 路由懒 chunk)
# 后端回归(确认 Rust 未被碰)
cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib  # 与基线一致(零 Rust 回归)
# 端到端手测(完整链路):
#   1. remote 起(指向 dist)+ PC daemon 配 remote + 生成码
#   2. 手机/浏览器开 remote 域名 → PWA → 配对 → 节点列表 → chat
#   3. 发消息 → 看流式 → 切 session → permission 弹窗
#   4. 断 SSE → agent 不停(S3 行为,前端视角验证)
#   5. 手机断网重连 → 流恢复
```

**commit**(若有零散修):`chore(pwa): S4 收尾(fmt/lint/手测修正)`

---

## S4 完成标准(对应 PRD 验收)

- [ ] Step 1-7 全部 commit
- [ ] design §1.3 表里 6 项 🔴 S4 新写全部落地
- [ ] PRD 8 条验收全过(附录 A 映射)
- [ ] `pnpm exec vue-tsc --noEmit` + `pnpm test` + `pnpm build` 全绿
- [ ] `cargo test --lib` 与实施前基线一致(零 Rust 回归,S4 不碰 Rust)
- [ ] Tauri Thin 逃生(`?transport=tauri`)仍工作
- [ ] browser-local(无 token 直连 daemon)行为不变

## 实施顺序总结

```
Step 1 (transport 核心) → Step 2 (router 骨架)
  → Step 3 (PC RemoteTab) → Step 4 (PairingView)
  → Step 5 (NodeListView + 401) → Step 6 (PWA 壳)
  → Step 7 (全量验证)
```

Step 1 是隧道接通的前提(无它手机请求到不了 PC);Step 2 是结构骨架;Step 3-5 是 UI 层(可并行:PC 端 vs 手机端);Step 6 PWA 壳独立;Step 7 串联验证。Step 1 完成后即可用 curl/手测验证 transport 路由,不必等 UI。

## 风险点 / 回滚

- **vue-router 接入碰现有 AppShell**:若 AppShell 内部有假设"无 router"的逻辑(如自己的导航),Step 2 暴露。回滚:App.vue 恢复直接挂 AppShell,router 独立分支。
- **vite-plugin-pwa 与现有 manualChunks 冲突**:Step 6 构建若报 chunk 冲突,调 workbox.globPatterns 或禁用 injectManifest 改用 generateSW。
- **EventSource 401 不重连**:design §6.2 已说明(MVP 靠 invoke 401 兜底)。若手测发现流式体验差,Step 5 补 EventSource onerror 探活。
