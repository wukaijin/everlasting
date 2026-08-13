# Review — S4 配对流程 + PWA 壳(transport 增强 + vue-router + RemoteTab + PWA)

> 评审日期:2026-08-12。评审对象:`prd.md` + `design.md` + `implement.md`(status=planning,实施前评审)。
> 方法:对 design 引用的代码库事实逐条核验(remote `server.rs`/`auth.rs`/`proxy.rs`/`pairing.rs`/`nodes.rs`/`health.rs` 的契约与行号、前端 `http.ts`/`daemonBase`/`CMD_TO_DOMAIN`/errorBus/`SettingsModal`/`main.ts`/`App.vue`/`streamController`/package.json/vite.config、PC 侧 4 个 remote IPC);对跨任务契约(S1-S3 ↔ S4)与三文档一致性(prd ↔ design ↔ implement)做交叉核对。
> 关联评审:[S2 review](../08-11-tunnel-client/review.md)(P1-3 SSE query 认证契约,S4 直接消费,已核验落地)。

## 总体评价

design 质量高——**remote 侧契约掌握准确(ServeDir+SPA fallback、auth 双通道、proxy 前缀+剥 token、SSE 裸字节透传、redeem/nodes wire shape 全部属实)**,D1-D5 决策链完整、D3 对 PRD 三态伪代码的修正正确(baseURL 三模式 PROD 恒同,token 是唯一可靠信号),"Rust 零改动"约束现实可行(核验确认 S1-S3 已铺好全部后端前提),S4 新写 vs 复用的分界清晰,implement Step 1-7 与 design/PRD 验收逐条对应。

**结论:可批准进入实施,但 P1-1 必须先修**——路由守卫把「无 token」一律赶到 `/pairing`,会锁死现有 browser-local / Tauri Thin 模式(它们永远没有 token,且 daemon 无 redeem 路由),与 design §7.1/§7.2 的"现状不变"自相矛盾;另有 2 个 P2(wire 契约字段名错误、401 拦截位置)建议实施前修订。

## ✅ 核验通过(证据确凿)

| 声明(design) | 核验结果 |
|---|---|
| remote 伺服 PWA 静态文件 + SPA fallback(`server.rs:53`) | **属实**(`server.rs:53` `resolve_dist_dir()` match,57 `ServeDir.not_found_service(index.html)`;`EVERLASTING_REMOTE_DIST_DIR` env 覆盖) |
| remote health 兼容协议门禁(`health.rs:26` `apiVersions: ["v1"]`) | **精确**(`health.rs:26` `SUPPORTED_API_VERSIONS = &["v1"]`,camelCase `remoteId/remoteVersion/apiVersions/uptimeSeconds`) |
| auth 双通道(Bearer header + `?access_token=` query) | **属实**(`auth.rs:45` `device_token_from_request`,header 优先;无效/吊销 → 401 `category: "Auth"`) |
| proxy `/api/v1/proxy/*path` 透传 + SSE 桥接 + 取消 | **属实**(`proxy.rs:75` catch-all + `require_device_token`;SSE 按 `Accept: text/event-stream` 分流;`frame_path` 剥 `access_token` query 后转发,测试断言 path 纯净;60s pending / 30s 首帧 / 离线 502 `node_offline`;慢手机剔除 + End 取消信号) |
| pairing redeem 契约 + 限速 | **属实**(`pairing.rs`:`POST /api/v1/pairing/redeem {code, device_name}` → 200 `{deviceToken, nodeId, nodeDisplayName}`;400 `invalid_or_expired_code` / 429 `RateLimit` per-IP 10/min;**wire 为 camelCase** —— design 伪码字段名错误,见 P2-2) |
| nodes API(带 token)→ 节点卡片列表 | **属实**(`nodes.rs`:`GET /api/v1/nodes` → `[{nodeId, displayName, status, lastSeenAt}]` camelCase;`online`/`offline` 由 WSS 心跳维护) |
| `daemonBase()` PROD 恒为 `location.origin` | **属实**(`http.ts:212`;DEV 才走 `localhost:7456`,`?daemonUrl=` 可覆盖)→ D3 对 PRD 三态伪代码的修正正确 |
| 前端 SSE 单全局 EventSource(`http.ts:247`) | **属实**(`http.ts:244-255`;行号与 design 引用一致)—— 附带确认:**Last-Event-ID 回放经 proxy 天然可用**(`forward_headers` 只剥 Authorization/Host/Content-Length,S3 断线重连能力手机端成立) |
| 前端 fetch 零 auth(`http.ts:278`) | **属实**(`http.ts:277-282` 无 Authorization 头)—— S4 核心改动点定位准确 |
| PC 4 个 remote IPC 已就绪 + `CMD_TO_DOMAIN` 注册 | **属实**(`commands/config.rs` `get_remote_config`/`set_remote_config`/`get_tunnel_status` + `commands/pairing.rs` `generate_pairing_code`;daemon routes 同注册;`http.ts:64-68` CMD_TO_DOMAIN 已含 4 条;`generate_pairing_code` 返 `{code, expiresIn}` camelCase —— design §3.1 伪码 `r.expiresIn` 正确) |
| `set_remote_config` 校验已落地 | **属实**(`config.rs:204-208` `normalize_remote_url` + `InvalidRequest` 不写库;S2 review P2-2 闭环) |
| 现有 errorBus 有 Auth category 路由(`AppShell.vue:76`) | **属实**(`utils/useErrorBus.ts` `routeByCategory` 4 类 → toast)—— 但只收**未捕获**异常,见 P2-1 |
| SettingsModal 5 tab + `default-value="providers"` | **属实**(`SettingsModal.vue:33` + 5 对 TabsTrigger/TabsContent)→ RemoteTab 加第 6 tab 的模式可行 |
| 零路由 SPA / 零 PWA / 零 token 存储 | **属实**(package.json 无 vue-router/vite-plugin-pwa;无 `views/`/`router/` 目录) |
| App.vue 27 行、1:1 搬 ChatView 可行 | **属实**(`App.vue` 恰好 27 行:AppShell+ChatWindow+streamController start/stop) |
| `streamController.start/stop` 视图级启停 | **属实**(`streamController.ts:663-757`:`listenerWired` 幂等,stop 后 start 可重臂 —— ChatView onMounted/onUnmounted 方案成立) |
| `main.ts` bootstrap `awaitDaemonHealthy` 不变 | **属实**(`main.ts:38-54` fail-loud overlay;remote health 兼容) |
| 前端 localStorage try/catch 惯例(`stores/config.ts`) | **属实**(`config.ts:67-82` 私有模式容错)→ `transport/auth.ts` 对齐的写法正确 |
| 单测基建(Step 1 补测可行) | **属实**(`http.test.ts` `vi.resetModules()` + mock fetch/EventSource,module 级状态隔离) |
| ws.rs 内部 RPC 已接(`/internal/pairing/generate`) | **属实**(`routes/ws.rs:252` `handle_internal_rpc` 分派 + S2 daemon 侧 `generate_pairing_code` 镜像) |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 路由守卫把「无 token」一律赶去 `/pairing`,锁死 browser-local / Tauri Thin 模式(与 design §7.1/§7.2 自相矛盾)

**位置**:design §5.1 `beforeEach`(`if (!hasDeviceToken()) return { name: "pairing" }`)+ `/` redirect 同判据;implement Step 2 验证("无 token → /pairing")。

**问题**:守卫对**所有模式**生效,而 browser-local(浏览器直连 daemon ServeDir)与 Tauri Thin GUI(WebView,默认 httpTransport)是**无 token 常态**——它们永远不会经过配对流程,却被一律重定向到 `/pairing`。而 `/pairing` 的 redeem 是 remote 专属端点(daemon 无此路由,核验 `daemon/routes/` 只有 `generate_pairing_code` 镜像);且 daemon 的 ServeDir SPA fallback 会把 `POST /api/v1/pairing/redeem` 也回退成 200 + index.html(`resp.ok === true` → `resp.json()` 抛错 → 显示误导性的"码无效")。**结果:S4 上线 = 现有 PC GUI / browser-local 全部不可用,死局**。design §7.1("无 token → 行为 = 现状 ✅")/§7.2("无 token → 直连 daemon(现状)✅")因此不成立。

**建议(三选一,推荐 a)**:

1. **(推荐)守卫加"remote-served"前置条件**:bootstrap 的 `awaitDaemonHealthy` 已拿到 health body,`health.ts:35-38` `DaemonHealth` 有 `daemonId`,而 remote health 返 `remoteId` —— 用"是否 remote 伺服"区分。`isRemoteServed && !hasDeviceToken() → /pairing`;browser-local / Thin 放行 `/chat`(现状)。零额外请求(复用握手),且顺带补上 D3 遗留的"origin 类型判定"缺口。
2. **反向设计**:无 token 默认放行 `/chat`(browser-local 现状);pwa-remote 首次打开由第一个 invoke 的 401 兜底(P2-1 的 transport 层拦截)自动跳 `/pairing`。零探测,但首开会闪一下 chat UI。
3. 守卫只拦"有 token 但缺 selectedNode"类,**配对页不进路由**(`/pairing` 仅当用户显式访问);pwa-remote 的强制配对靠 401 兜底。改动面更大,不推荐。

**注意**:不能直接用 `isStandalonePWA()` 判定(design §2.3 已正确论证其不适用)—— remote-served 判据必须来自健康探针。

### 🟡 P2-1 — 401 全局处理推荐挂 errorBus,但 errorBus 只收「未捕获」异常,现有调用点系统性 catch+swallow

**位置**:design §6.2("**推荐 errorBus handler**(main.ts 注册全局 Auth 错误处理器,检测 401 → 清 token → 跳 pairing)")。

**问题**:`main.ts:20-30` 的 errorBus 只从 `window.error` / `window.unhandledrejection` 收事件;而现有调用点(如 `ProvidersTab.vue:132-146`)对 store 的 invoke 调用是 `try/catch + console.error` **吞掉**的。401 从 `transport.invoke` 抛出后大概率到不了 unhandledrejection → 全局 Auth 处理器收不到 → token 失效(remote 吊销)后用户只会看到静默失败,不跳回配对页 —— **PRD 验收第 6 条失效**。

**建议**:选 design 自己列的备选——**在 `http.ts` invoke 的 `!resp.ok` 分支做 401 拦截,触发模块级回调**(由 router/App 注册:`clearDeviceToken` + `resetEventSource` + `router.push("/pairing")`)。transport 是全部 app 命令的唯一 choke point,无论调用方 catch 与否都必过。errorBus 的 Auth toast 保留(提示用户),但**跳转**不依赖它。design §6.2 的"或"选项应升为推荐项。

### 🟡 P2-2 — redeem 响应解构用 snake_case,实际 wire 是 camelCase(照抄必炸)

**位置**:design §2.4 `stores/pairing.ts` 伪码:`const { device_token, node_id, node_display_name } = await resp.json();`。

**问题**:`pairing.rs:63-68` `RedeemedResponse` 是 `#[serde(rename_all = "camelCase")]`,实际 wire 为 `{deviceToken, nodeId, nodeDisplayName}`(测试断言 `body["nodeId"]` / `body["nodeDisplayName"]`)。伪码照抄 → 解构出 3 个 `undefined` → 存的 token 是 `undefined` → `hasDeviceToken()` 判真但请求全 401。`stores/nodes.ts` 的 `NodeInfo[]` 同理(`{nodeId, displayName, status, lastSeenAt}`)。

**建议**:§2.4 伪码改为 camelCase 解构(`{ deviceToken, nodeId, nodeDisplayName }`);§2.4/§5.4 补 nodes 响应字段清单。手测(implement Step 4)会暴露,但文档是"可信源",应先行修正。

### 🟢 P3-1 — RemoteTab 节点信息区的 `display_name` 无来源

**位置**:design §3.1 节点信息区("display_name(只读,从 get_tunnel_status)")。

**问题**:`TunnelStatusPayload` 只有 `{connected, remoteUrl, nodeId, lastError}`(`config.rs:150-157`),**没有 display_name**。display_name 只存在于 remote 的 nodes 表(PC 连 WSS 时注册),S4 纯前端拿不到;给 `TunnelStatusPayload` 加字段 = 改 Rust,违背"零 Rust 改动"。

**建议**:节点信息区只显示 `node_id`(从 get_tunnel_status,属实);display_name 行删掉或标注"V2(需 Rust 侧加字段)"。

### 🟢 P3-2 — `vite-plugin-pwa ^0.20` 与 Vite ^6 不兼容

**位置**:design 附录 B 依赖清单。

**问题**:vite-plugin-pwa **0.21.0 起才支持 Vite 6**(0.20 的 peer 范围是 Vite 5);项目 Vite 是 `^6.0.3`(`package.json`)。按 `^0.20` 安装会 peer 冲突或解析到 0.21+。

**建议**:附录 B 改 `vite-plugin-pwa (^0.21)`(实施时以 pnpm 实际解析 + `pnpm build` 通过为准)。

### 🟢 P3-3 — implement 完成标准计数笔误("7 项 🔴"实为 6 项)

**位置**:implement.md 完成标准第 2 条("design §1.3 表里 7 项 🔴 S4 新写全部落地")。

**问题**:design §1.3 表 🔴 行共 **6** 条(transport auth 注入 / auth.ts / router / RemoteTab / PWA 壳 / PairingView+NodeListView)。另:完成标准里 `cargo test --lib 仍 1689 全绿` 是写死的数字,测试数随 S 增长会漂移(AGENTS.md 基线 1657)。

**建议**:改"6 项";测试数改"与实施前基线一致(零 Rust 回归)"。

## 🟦 其他备注(可不动)

- **D3 决策成立,但判据不可滥用**:`hasDeviceToken()` 作 transport 开关(§2.2)完全正确;作路由守卫(§5.1)则引出 P1-1。同一判据、两处语义,文档应显式区分"transport 路由"与"导航门禁"。
- **SSE 断线回放能力手机端成立**(前表已述):Last-Event-ID 经 proxy 透传,S3 的"断 SSE 不停 agent / 重连回放"在 PWA 场景无额外工作——这是 S3 流式铺底的直接红利,实施时手测确认即可。
- **dev 模式手机联调有约束**:`daemonBase()` DEV 恒指 `localhost:7456`,手机 PWA 无法直接连 vite dev server;联调需走 remote 伺服 prod build(implement Step 4 验证已按此设计,正确)。不必改。
- **`check.jsonl` / `implement.jsonl` 仍是 seed 状态**(各 254B):若按 workflow 1.4 在 `task.py start` 前需要 curated 清单(sub-agent 平台),记得补;inline 平台可跳过。
- **pairing/nodes 直接 fetch 绕过 CMD_TO_DOMAIN 的决策正确**(remote-native 端点 shape 不同于 daemon 命令,硬塞会污染映射表)——与 S2 review 对"5 处改动"的计数教训同理,保持两张表分离是对的。
- **proxy 前缀 URL 拼接已由 remote 测试锁定**(`/api/v1/proxy/api/v1/sessions/list` → path 剥为 `/api/v1/sessions/list`),design §2.2 的拼法 `{base}/api/v1/proxy/api/v1/{domain}/{cmd}` 与契约一致,无需实现期再验证。

## 复评建议

1. **修 P1-1**(§5.1 守卫加 remote-served 判定,复用 bootstrap health 探针)→ 修完即可 `task.py start`
2. **修 P2-1**(§6.2 401 拦截改 http.ts 模块级回调)+ **P2-2**(§2.4 伪码改 camelCase)
3. P3-1/P3-2/P3-3 合并修订(单 commit doc 修)
4. Step 1 单测记得覆盖 P2-1 的 401 拦截分支(transport 层新行为,现有 `http.test.ts` 基建可直接扩展)
