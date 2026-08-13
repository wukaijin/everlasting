# S4 配对流程 + PWA 壳:配对码 UI + PWA manifest/service worker + 节点列表 + transport 切换

> 架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。本任务只细化执行。

## Goal

把 S3 跑通的隧道管线,用前端包装成用户能用的形态:PC 前端配置 remote + 生成配对码;手机 PWA 完整壳(manifest + service worker)+ 配对码输入界面 + 已配对节点列表 + 选节点进入完整前端。

**核心收益兑现点**:本任务做完,手机真的能用了(虽然移动端适配还糙,S5 打磨)。

## Scope

### PC 前端:Settings 加 "Remote" tab

复用现有 Settings 组件结构(参考 Providers/Models/Subagents tab 模式)。新增 `<RemoteTab>`:

- **配置区**:`remote_url` 输入框 + `shared_secret` 输入框 + "保存"按钮 → 调 `set_remote_config` IPC
- **连接状态**:实时显示 tunnel 状态(已连接/重连中/未配置)—— 从 daemon SSE 或轮询获取
- **配对码区**:"生成配对码"按钮 → 调 `generate_pairing_code` IPC → 展示 6 位码 + 60s 倒计时 + "扫码或手动输入"
- **节点信息**:展示本机 node_id + display_name(可选编辑)

### PWA manifest + service worker

- `app/public/manifest.webmanifest`:name / short_name / icons / start_url / display:standalone / theme_color
- `app/public/sw.js`(或 vite-plugin-pwa):最小离线壳 —— 缓存 app shell(HTML/CSS/JS),网络优先 + 离线降级
- `app/index.html`:link manifest + register SW
- **图标**:需要一套(192/512),MVP 用占位图标,V2 再设计

### transport 层增强(`app/src/transport/`)

现有 transport 抽象(tauriTransport / httpTransport)。新增逻辑:

```ts
// 环境检测
function detectTransportMode(): 'tauri' | 'pwa-remote' | 'browser-local' {
  if (isTauri()) return 'tauri';                              // PC Tauri GUI
  if (isStandalonePWA() && hasRemoteBaseURL()) return 'pwa-remote'; // 手机 PWA / 家里电脑浏览器经 remote
  return 'browser-local';                                      // 本地浏览器直连 daemon
}
```

- `pwa-remote` 模式:httpTransport 的 baseURL 指向 remote daemon 域名(从 manifest 或 URL 推断),所有 `invoke` 走 remote
- 配对 token 存储:localStorage `everlasting_device_token`(MVP 接受,V2 评估 httpOnly cookie)

### 手机端:配对码输入界面

- 检测到无 token / token 失效 → 显示配对码输入界面(6 位输入框 + "配对"按钮)
- 调 `POST /api/v1/pairing/redeem` → 拿到 device_token → 存 localStorage → 跳转节点列表

### 手机端:节点列表首页

- `GET /api/v1/nodes`(带 token)→ 已配对节点卡片列表
- 每张卡片:display_name + 在线/离线状态(绿点/灰点)+ 最后在线时间
- 点击在线节点 → 切换 transport baseURL 到该节点 → 进入完整前端(project/session/chat)
- 点击离线节点 → 提示"该 PC 离线"(不白屏)

### 完整前端复用

选节点后,**现有前端代码原样加载**(project tabs / session list / chat / audit / subagent drawer / permission modal 全套)。唯一差异:transport 走 remote。不做功能裁剪(Q11:PWA 全权)。

## 依赖

- **强依赖 S3**(PWA 要真能访问才有意义)
- 但 PWA 壳 / manifest / 配对 UI 可提前做(S3 完成前用 mock remote 测)
- 可与 S3 部分并行:S3 调通前先做壳 + UI,S3 完成后联调

## 验收标准

- [ ] PC 前端 Settings → Remote tab:能配 remote_url + secret + 生成配对码 + 看到连接状态
- [ ] 手机浏览器打开 remote 域名 → 加载 PWA → "添加到主屏幕"成功 → 主屏幕图标启动是 standalone(无浏览器 chrome)
- [ ] 首次打开(无 token)→ 配对码输入界面 → 输入 PC 生成的码 → 配对成功 → 跳转节点列表
- [ ] 节点列表显示已配对 PC + 在线状态;点在线节点进入完整前端
- [ ] 完整前端在手机上功能可用(发消息、看流式、切 session、permission 弹窗)—— 布局适配糙没关系,S5 打磨
- [ ] token 失效(在 remote 吊销)→ 自动跳回配对界面
- [ ] 离线节点点进去显示离线提示,不白屏
- [ ] 家里电脑浏览器(大屏)打开 remote 域名 → 配对 → 完整三栏前端可用(零额外适配)

## Notes

- **token 存 localStorage 是 XSS 风险点**,MVP 接受(单用户 + 自己的 PWA + 自己的 remote)。V2 评估 httpOnly cookie 方案(需要 remote 侧 session 化,工作量大)。
- 家里电脑浏览器场景**白嫖**:大屏直接用现有三栏布局,不需要 S5 的移动端适配。S5 只针对手机宽度。
- 配对码 60s 倒计时 UI 要清晰,过期了引导重新生成。

## 实施前决策与探查结论(2026-08-12,design 前 + review 后修订)

基于前端骨架探查(Vue3/Pinia/reka-ui)+ remote crate 代码核验(S1-S3),裁决如下。详见 [design.md §0](./design.md)。

| # | 问题 | 裁决 | 依据 |
|---|---|---|---|
| **D1** | PWA 多视图导航方式 | **vue-router**:三路由 `/pairing` `/nodes` `/chat` + 守卫查 token | 用户决策(URL 状态 + 后退键 + 深链);现有零路由 SPA 需引入路由依赖 |
| **D2** | PWA tooling | **vite-plugin-pwa**:manifest + SW 自动生成,app-shell precache | Vite PWA 事实标准;手写 SW 无 MVP 收益 |
| **D3** | transport 模式检测 | **`hasDeviceToken()`**(localStorage 存在 token = pwa-remote 模式) | 探查:`daemonBase()` 在 PROD 恒为 `location.origin`,tauri/browser-local/pwa-remote 三者 baseURL 无差异 —— 真实差异是 auth 注入 + proxy 前缀,而这两者都以"有 token"为前提 |
| **D4** | remote-native vs proxied 命令路由 | **pairing/nodes 用直接 `fetch`**(remote 自身端点);**其余走 `transport.invoke`**(pwa-remote 时加 `/api/v1/proxy` 前缀) | remote 只挂 `/api/v1/proxy/*path` 透传 + `/api/v1/pairing` + `/api/v1/nodes`;daemon 命令(sessions/chat…)经 proxy 透传到 PC |
| **D5** | token 存储 | **localStorage**(PRD 已定,MVP 接受 XSS 风险) | 单用户 + 自己的 PWA + 自己的 remote;V2 评估 httpOnly cookie |
| **D6** | 路由守卫的"需配对"判定 | **`isRemoteContext()`**:bootstrap health body 有 `remoteId` = remote 伺服(需配对);daemon 返 `daemonId` = daemon/Tauri(直放) | P1-1 修订:守卫不能只查 token —— browser-local/Tauri Thin 永远无 token 却不需配对;daemon 无 redeem 路由,强制跳 /pairing = 死局 |

**已核验的代码库事实**(无需再探查):

- **PC 端 4 个 IPC 已就绪**:`get_remote_config` / `set_remote_config` / `get_tunnel_status` / `generate_pairing_code` 已在 `lib.rs::generate_handler!` + daemon HTTP 路由 + `CMD_TO_DOMAIN` 注册 —— RemoteTab 是纯 UI 工作。
- **remote 已伺服 PWA 静态文件**:`server.rs:53` `ServeDir + not_found_service(index.html)` SPA fallback —— PWA 同源托管在 remote,`daemonBase()` 的 `location.origin` 分支天然指向 remote。
- **remote health 兼容协议门禁**:`health.rs:26` `apiVersions: ["v1"]`(camelCase shape 对齐 daemon)—— `awaitDaemonHealthy()` 在 pwa-remote 模式天然通过,bootstrap 无需特殊处理。
- **auth 双通道已就绪**:`auth.rs` `Authorization: Bearer`(fetch)+ `?access_token=`(EventSource 无法设 header 的 SSE 通道)。
- **transport 当前零 auth**:`http.ts:278` fetch 无 Authorization 头,`http.ts:247` EventSource URL 无 token query —— S4 核心改动点。
- **无 router / 无 PWA / 无 auth 层**:全为 greenfield(100% 新写)。
