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
- transport 检测要稳健:`isStandalonePWA()` 检查 `window.matchMedia('(display-mode: standalone)')` 或 `navigator.standalone`。
- 配对码 60s 倒计时 UI 要清晰,过期了引导重新生成。
