# 远程访问 E2E 部署与验收手册

> 从零部署到手机看到 agent 实时响应的完整 runbook。综合 S1(remote daemon)
 > + S2(PC tunnel client)+ S4(配对/PWA)+ S5(移动适配)。
>
> - 部署细节见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md)(remote 单端运维)
> - 架构决策见 [`.trellis/tasks/08-11-remote-control-epic/prd.md`](../.trellis/tasks/08-11-remote-control-epic/prd.md)(SoT)
> - 远期规划见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md)

## 0. 拓扑与角色

```
手机 PWA(浏览器)                PC(公司/家里,常开)
  │ HTTPS                          │
  │ ① 打开 https://remote.dom      │
  ▼                                │
┌──────────────────────┐    WSS    │
│ 云服务器 remote       │◄──────────┤ PC daemon(tunnel client)
│ everlasting-remote   │  ② PC 主动│  - agent core + 本地 SQLite
│  - WSS 服务端         │   outbound│  - 本地功能零依赖 remote
│  - 配对码 + devices  │   穿 NAT  │  - 跑 API :7456 + 前端
│  - 反向代理 + SSE 桥 │           │
│  - ServeDir 伺服 PWA │    HTTP   │
│  - SQLite(只存 token)│──────────►│ ③ 手机请求经 WSS 转发
└──────────────────────┘ ④ proxy   │   → daemon loopback :7456
                          到 PC    │   → agent 响应 → SSE 回流手机
```

**三个角色,三台机器**:

| 角色 | 跑什么 | 端口 | 谁访问 |
|---|---|---|---|
| **云服务器** | `everlasting-remote`(独立二进制)+ nginx + 前端 dist | 443(nginx)→ 7457(remote) | 手机 + PC 都连它 |
| **PC** | `everlasting-daemon`(agent core + tunnel client + 前端) | 7456 | PC 自己 + 经 remote 被手机访问 |
| **手机** | 浏览器(Safari/Chrome) | — | 访问 remote 域名 |

**关键不变量**:PC daemon 本地功能**完全不依赖 remote**。remote 挂了/隧道断了/没配,PC 本地照常工作,只是手机暂时连不上。

---

## 1. 前置:编译两份产物

在开发机(WSL / 本地)上一次性产出,再分别部署到服务器和 PC。

### 1.1 remote 二进制(部署到云服务器)

```bash
# 仓库根目录
cargo build --release -p everlasting-remote
# 产物:target/release/everlasting-remote(ubuntu ELF,零系统库依赖)
```

> remote **不需要** `PKG_CONFIG_PATH` / webkit2gtk / gdk-pixbuf(对比 daemon)。
> 服务器是 ubuntu x86_64 就能直接跑,无 docker 无 runtime。

### 1.2 前端 dist(PC 和 remote 各要一份)

```bash
cd app
pnpm install        # 首次或依赖变更后
pnpm build          # vue-tsc 类型检查 + vite build
# 产物:app/dist/(index.html + assets/*)
```

**两份 dist 用途不同**:
- **PC 的 dist**:PC daemon 本地伺服(给 PC 浏览器配置 remote 用)。dev 模式可省略,用 `pnpm dev`(vite HMR)代替。
- **remote 的 dist**:remote 伺服给手机访问的 PWA。**必须部署到服务器**,否则手机打开 remote 域名只拿到 API、无前端页面(空白)。

---

## 2. Step 1 — 部署 remote 到云服务器

> 单端运维细节(编译/参数/CLI/nginx/安全模型)见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md)。本节只讲 E2E 必经路径。

### 2.1 上传产物到服务器

```bash
# 本地 → 服务器(scp 或 rsync)
scp target/release/everlasting-remote user@yourserver:/opt/everlasting/
scp -r app/dist user@yourserver:/opt/everlasting/dist
```

服务器目录最终长这样:
```
/opt/everlasting/
├── everlasting-remote       # 二进制
└── dist/                    # 前端产物(index.html + assets/)
```

### 2.2 配置 nginx(HTTPS + WSS,证书自理)

```nginx
server {
    listen 443 ssl;
    server_name remote.yourdomain.com;

    ssl_certificate     /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    # 手机 PWA + 普通 API(remote 自己伺服前端,ServeDir)
    location / {
        proxy_pass http://127.0.0.1:7457;
    }

    # /ws 必须单独 location:WebSocket 升级头要透传。
    # proxy_read_timeout 300s 必加(心跳 30s × 倍数,否则空闲 60s 被 nginx 默认掐断)。
    # access_log off:?secret 在 query 里,别记进服务器日志。
    location /ws {
        proxy_pass http://127.0.0.1:7457;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 300s;
        access_log off;
    }
}
```

> 证书用 Let's Encrypt / Caddy / 你自己的都行,remote 不挑。

### 2.3 启动 remote(指向前端 dist)

```bash
# 服务器上(推荐 systemd 托管,这里演示前台)
cd /opt/everlasting
export EVERLASTING_REMOTE_SECRET="<你定的共享密钥>"
export EVERLASTING_REMOTE_DIST_DIR="/opt/everlasting/dist"   # 指向前端产物
./everlasting-remote --port 7457
```

**关键参数**:

| 参数 / env | 必填 | 说明 |
|---|---|---|
| `--shared-secret` / `EVERLASTING_REMOTE_SECRET` | **是** | PC 连 WSS 的握手密钥;缺了启动直接 panic(防裸跑) |
| `EVERLASTING_REMOTE_DIST_DIR` | 否(但 E2E 必设) | 前端 dist 路径;不设则找 CWD 下 `./dist`;都没有则纯 API(手机空白) |
| `--port` | 否(默认 7457) | nginx 反代到此端口 |
| `--db-path` | 否(默认 `~/.local/share/dev.everlasting.remote/remote.db`) | remote 的 SQLite,只存 token/devices/配对码 |

### 2.4 验证 remote 起来了

```bash
# 服务器本地
curl http://localhost:7457/health
# 期望:{"remoteId":"everlasting-remote/..."}  ← remoteId 字段是手机判断"我在 remote 上"的信号

# 从外网(手机/任意机器)
curl https://remote.yourdomain.com/health
# 期望:同上。若失败 → nginx / 证书 / 防火墙问题

curl https://remote.yourdomain.com/
# 期望:返回 index.html(<html>...</html>)。若 404/空 → dist 没部署对(EVERLASTING_REMOTE_DIST_DIR)
```

**Step 1 完成的标志**:`/health` 返回含 `remoteId` 的 JSON + `/` 返回前端 index.html。

---

## 3. Step 2 — PC 侧配置 tunnel

### 3.1 启动 PC daemon

```bash
# 方式 A:release daemon(serve 前端 + API,生产形态)
./scripts/daemon.sh bg              # 编译 release + 后台跑,默认 :7456

# 方式 B:dev 模式(vite HMR 前端 + daemon API,开发态)
cd app && pnpm dev:all              # vite(:1420) + daemon(:7456) 并行
```

PC 浏览器打开 daemon:
- 方式 A:`http://localhost:7456`
- 方式 B:`http://localhost:1420`

> PC daemon 本地零依赖 remote —— 即使 Step 1 没做,PC 本地功能照常。配置 remote 是 opt-in 附加层。

### 3.2 配置 Remote(Settings → Remote tab)

PC 浏览器里:

1. 左侧 Sidebar 底部 **设置** 齿轮图标 → 打开 Settings 弹窗
2. 切到 **Remote** tab
3. 填两项:
   - **Remote URL (wss://)**:`wss://remote.yourdomain.com`(对应 nginx 的 443 + /ws location)
   - **Shared Secret**:Step 2.3 里 `EVERLASTING_REMOTE_SECRET` 设的那个值
4. 点 **保存**

### 3.3 确认 tunnel 连上

保存后看 **连接状态** 区(每 2s 轮询),状态徽章应变绿:

| 徽章 | 含义 | 处理 |
|---|---|---|
| 🟢 已连接 | tunnel 握手成功,PC 已注册到 remote | ✅ 继续 Step 3 |
| 🟡 重连中 | 连接断开,指数退避重试 | 等;持续黄看 §7 故障排查 |
| 🔴 认证失败 | shared_secret 与 remote 不匹配 | 检查 Step 2.3 的 secret vs Step 3.2 填的 |
| ⚪ 未配置 | remote_url 为空 | 没保存成功,重填 |

**节点信息** 区会显示 `Node ID`(PC 在 remote 的唯一标识)。

### 3.4 生成配对码

1. 在 Remote tab 的 **配对码** 区,点 **生成配对码**
2. 显示 6 位数字码 + 60s 倒计时(一次性,过期作废)
3. 提示文案会指明"在手机上打开 `wss://remote.yourdomain.com`" —— 注意手机用 **https://** 不是 wss://(wss 是 PC tunnel 用的,手机浏览器走 https)

**Step 2 完成的标志**:连接状态 🟢 已连接 + 配对码已生成(60s 内有效)。

---

## 4. Step 3 — 手机配对

### 4.1 打开 remote 域名

手机浏览器(Safari/Chrome)访问:

```
https://remote.yourdomain.com
```

> **不是** PC 的 localhost,也**不是** wss://。就是 remote 的 https 域名。

**会发生什么**(自动,无需操作):
1. remote ServeDir 返回 PWA 前端(index.html + assets)
2. 前端 bootstrap 探测 `GET /health` → 响应含 `remoteId` → 前端判定"我在 remote 上"(isRemoteContext)
3. 前端查 localStorage 无 device_token → router 自动跳 `/pairing`

**若没自动跳 /pairing**:看 §7(可能是 health 探测失败,或 daemon 模式误判)。

### 4.2 输入配对码

`/pairing` 页面:
1. **配对码** 输入框:填 PC 屏幕上显示的 6 位码(自动大写)
2. **设备名(可选)**:填个识别名,如"Carlos 的 iPhone"(方便以后在 remote nodes 表里认)
3. 点 **配对**

**会发生什么**:
1. 前端 POST `https://remote.yourdomain.com/api/v1/pairing/redeem` `{code, device_name}`
2. remote 校验码(未过期 + 未用 + 绑定正确 PC)→ 签发 64 位 hex `device_token` + 落 devices 表
3. 前端把 `device_token` 存 localStorage → 跳 `/nodes`

**失败**:
- "配对码无效或过期" → 码过期/输错/已用过(一次性),回 PC 重新生成
- "请求过于频繁" → 同 IP 限速 10/min,等一分钟

### 4.3 选节点进 chat

`/nodes` 页面显示已配对的 PC 卡片:
- 🟢 在线(PC tunnel 在)→ 点进去
- 🔴 离线(PC daemon 没跑 / 断网)→ 显示离线提示,不白屏

点在线节点 → 跳 `/chat`(完整前端:抽屉导航 + 会话列表 + 对话)。

**Step 3 完成的标志**:手机进到 /chat,看到 PC 上的会话列表。

---

## 5. Step 4 — 验证效果(对照 epic 验收标准)

在手机 /chat 里逐项验证(对应 parent PRD 的 epic 验收 + S5 移动适配):

### 5.1 全链路读写

- [ ] 选一个会话 → 输入消息 → 发送
- [ ] 看到 agent **流式响应**(字符逐个出现,SSE 经 remote 桥接到手机)
- [ ] markdown 代码块横向可滚(不撑破布局)
- [ ] tool_use / tool_result 卡片可读(长字段自动换行)

### 5.2 permission 交互

- [ ] 触发一个需要权限的工具(如写文件)→ agent 发 `permission:ask`
- [ ] 手机上看到权限弹窗(全屏居中,按钮可点)
- [ ] 点允许 → agent 继续

### 5.3 S5 移动适配(本批新加)

- [ ] 顶部汉堡按钮 ☰ → 点开 → 左侧抽屉滑出全屏(项目 tabs + 会话列表)
- [ ] 抽屉里点会话 → 抽屉自动关 → 回到对话
- [ ] 点遮罩 / 选 session → 抽屉关闭
- [ ] 聚焦输入框 → 软键盘弹起 → **输入框不被挡**(visualViewport 机制)
- [ ] 输入中文(IME)正常
- [ ] 打开任一设置弹窗 → 全屏,无横向溢出
- [ ] 底部输入框不被 Home Indicator 压住(safe-area)
- [ ] 桌面(≥768px)回归:PC 浏览器看本地,三栏布局零变化

### 5.4 第二台 PC(家里,验证多节点)

家里电脑重复 Step 2(起 daemon + 配同一个 remote + 同一个 shared_secret)+ 生成配对码 → 手机第二次配对(输新码)→ `/nodes` 应显示两张 PC 卡片,各自在线/离线状态独立。

---

## 6. 添加到主屏幕(PWA 安装)

让手机像原生 App 一样:

- **iOS Safari**:分享按钮 → "添加到主屏幕"
- **Android Chrome**:菜单 → "添加到主屏幕" / "安装应用"

之后从主屏幕图标启动 = 全屏 PWA(无浏览器地址栏,体验更接近原生)。manifest + service worker 已在 S4 配好。

---

## 7. 故障排查

| 症状 | 可能原因 | 修法 |
|---|---|---|
| 手机打开 remote 域名**空白** | dist 没部署 / `EVERLASTING_REMOTE_DIST_DIR` 没指对 | 服务器 `curl http://localhost:7457/` 看是否返回 index.html;检查 env |
| 手机打开 remote 域名**直接进 /chat 不跳 /pairing** | health 探测没拿到 remoteId(isRemoteContext 判 false) | 手机 `curl https://remote.dom/health` 看响应有没有 `remoteId` 字段 |
| 手机**一直转圈**进不去 | health 探测超时 / nginx 没放行 /health | 浏览器 devtools Network 看 /health 请求;服务器看 remote 日志 |
| PC 连接状态**认证失败** 🔴 | shared_secret 两端不一致 | 核对 PC RemoteTab 填的 secret vs 服务器 `EVERLASTING_REMOTE_SECRET` |
| PC 连接状态**重连中** 🟡 不绿 | WSS 握手失败 / nginx /ws location 配错 | 服务器 `curl -i https://remote.dom/ws`（期望 426 Upgrade Required 或 400，不是 502/404）；检查 nginx /ws 有没有 Upgrade 头透传 |
| **生成配对码**按钮灰的 | 连接状态未配置/未连 | 先把状态弄绿（§3.3） |
| 配对码**已过期** | 60s 一次性,超了 | PC 重新生成（别拖） |
| 手机配对**"无效或过期"** | 码输错 / 过期 / 已用过 | 重新生成新码（一次性,不能复用） |
| 手机配对**"请求过于频繁"** | 同 IP redeem > 10/min | 等 60s（防暴力扫,正常用碰不到） |
| /nodes 显示离线但 PC 在跑 | tunnel 断了 / 心跳超时 | PC 看连接状态;remote 日志看 `tunnel disconnected` |
| 手机发消息**没响应** | tunnel 断 / PC daemon loopback 挂 / agent 卡 | remote 日志看 proxy 转发;PC daemon 日志看请求 |
| iOS 输入框被键盘挡 | visualViewport 机制没生效（S5） | 确认 `useMobileKeyboard` 加载（/chat 页面）;真机 Safari 才能复现 |
| Dialog 手机上横向溢出 | Step 5 全屏化 CSS 没生效 | 确认 style.css 的 `@media (max-width:767px)` 块在 |

**日志位置**:
- remote:`/tmp/everlasting-remote.log`（`scripts/remote.sh logs`）或 systemd journal
- PC daemon:`scripts/daemon.sh logs` 或前台输出

---

## 8. 运营

### 重新配对(换手机 / token 失效)

手机 localStorage 清掉 device_token(浏览器清站点数据)→ 重新打开 remote 域名 → 回到 /pairing → PC 生成新码 → 配对。

### 撤销设备(手机丢了)

remote.db 的 devices 表删对应 token 行:

```bash
sqlite3 ~/.local/share/dev.everlasting.remote/remote.db
> DELETE FROM devices WHERE display_name LIKE '%丢了的那台%';
```

删后该 token 立即失效,手机请求被 remote 401 拒。

### 多 PC

每台 PC 独立起 daemon + 配同一个 remote + 同一个 shared_secret。各自配对(每个 PC 生成自己的码,手机各配一次)。手机 /nodes 看到多张卡片,数据隔离(各 PC 独立 SQLite)。

### remote 升级

```bash
# 服务器
./scripts/remote.sh stop          # 或 systemctl stop
# 替换二进制(+ 如前端变了,替换 dist/)
./scripts/remote.sh bg --secret <S>   # 重启
```

remote.db 保留(token/devices 不丢)。手机无需重新配对。

### 换 shared_secret

两端同时改:服务器改 `EVERLASTING_REMOTE_SECRET` 重启 remote + 各 PC RemoteTab 改 secret 重存。改期间所有 tunnel 断,手机暂时连不上。已配对 device_token 不受影响(secret 只管 PC→remote 的 WSS 握手,与手机 token 无关)。

---

## 附:快速冒烟(无 PC / 无手机,只验 remote 本身)

无 S2(PC tunnel)时,用 node 脚本模拟 PC daemon 验证 remote 全链路:

```bash
# 服务器或本地
./target/release/everlasting-remote --port 7457 --shared-secret test123 &
node scripts/remote-e2e-smoke.mjs
# 期望:E2E OK —— WSS 握手 → 配对码生成 → redeem → 节点状态 → 非流式反代 全过
```

(Node >= 22,内置 WebSocket。)这只验 remote 单端,不含 PC tunnel / 手机 PWA 真实链路。

---

## 附:数据流速查

| 动作 | 路径 |
|---|---|
| 手机打开 remote 域名 | 手机 → nginx(443)→ remote(7457)ServeDir → 返回 PWA |
| 手机 bootstrap 探测 | PWA → remote `/health` → 返回 `{remoteId}` → 前端判 remote context |
| 手机配对 | PWA → remote `POST /api/v1/pairing/redeem` → 返回 `device_token` |
| 手机读节点列表 | PWA → remote `GET /api/v1/nodes`(带 token)→ 返回在线 PC |
| 手机发消息 | PWA → remote `/api/v1/proxy/...`(带 token)→ remote 查 token→node_id → WSS Request 帧发 PC → PC daemon 打 loopback:7456 → agent → SSE → remote Stream 帧桥接 → 手机 EventSource |
| PC 连 remote | PC daemon → remote `/ws`(WSS,带 secret query)→ 握手 → 注册 node_id |
| PC 心跳 | PC → remote ping/pong 30s;90s 无响应判 PC 离线 → nodes API 更新 |
