# 远程手机控制 epic:remote daemon + WSS 隧道 + PWA

## Goal

让用户在离开 PC 后,通过手机 PWA(或任意远程浏览器,包括家里电脑浏览器)访问运行在 PC 上的 `everlasting-daemon`,查看 agent 进度/结果并继续操作。

**架构核心:PC daemon 是一等公民独立可用,remote daemon 是 opt-in 附加层。** PC daemon 不配 remote 时产品跟现状完全一致(本地 httpTransport);配上 remote_url + shared_secret 后,后台维持一条到 remote daemon 的 WSS 长连接,为远程浏览器开通道。remote daemon 是 PC daemon 现有 HTTP API 的"远程反向代理",手机 PWA 用的前端代码与 PC 完全同一套。

落地 ROADMAP [Phase 3](../../../docs/REMOTE-ACCESS-ROADMAP.md) + B11(个人远程遥控通道),把原定 Cloudflare Workers + D1 中继替换为**国内 2C2G 服务器 + 自研 Rust remote daemon**。

## 拓扑

```
手机 PWA / 家里电脑浏览器
        │ HTTPS(nginx 反代 + 证书,用户自理)
        ▼
┌─────────────────────────────────────┐
│ 云服务器(国内 2C2G)                 │
│  remote daemon(独立 crate,Rust)    │
│   - WSS 服务端(收 PC outbound 连接) │
│   - 配对码生命周期 + devices 表      │
│   - 反向代理(token → node WSS 转发) │
│   - SSE 桥接(Stream 帧 ↔ SSE chunk) │
│   - 节点状态 API(基于心跳判在线)     │
│   - shared_secret 校验(防伪 daemon) │
│  remote 自己的 SQLite(只存 token/   │
│  devices/配对码,不存 agent 数据)    │
└─────────────────────────────────────┘
        ▲ WSS 长连接(PC 主动 outbound 穿 NAT)
        │
   ┌────┴─────────────────┐
   │                      │
公司 PC daemon          家里 PC daemon
(常开,在线)            (不常开,开机拉起)
  - agent core           - 同左
  - 独立 SQLite          - 独立 SQLite(数据隔离)
  - tunnel client 模块   - 同左
  - 本地功能零依赖 remote │
```

**关键不变量**:PC daemon 本地功能**完全不依赖** remote daemon。remote 挂了/隧道断了/没配,PC 本地照常工作,只是手机暂时连不上。

## 背景

- 07-20~23 已落地 daemon 化 epic(`everlasting-daemon` 独立进程 + axum HTTP server + ServeDir + httpTransport + SSE stream),为本次 epic 提供基础
- [REMOTE-ACCESS-ROADMAP Phase 3](../../../docs/REMOTE-ACCESS-ROADMAP.md) 远期规划了"认证 + 跨设备远程",本 epic 是它的实际落地版,中继节点从 Cloudflare 换成国内服务器
- 用户场景:daemon 运行,人在 PC 端可操作,人离开后手机看进度/结果;后期家里电脑浏览器也能间接操作公司 PC daemon

## 已确认决策(grill-me session 2026-08-11)

| # | 决策 | 结论 |
|---|---|---|
| Q1 | 部署拓扑 | daemon 跑 PC(公司常开 + 家里不常开)+ 云服务器反向代理;不搬代码到云 |
| Q2 | 手机端形态 | PWA,前端代码与 PC 完全同一套 |
| Q3 | 推送通道 | **不做主动推送**,靠主动拉取/SSE;SSE 后台会断,打开重连 resync(永久不做推送,非延后) |
| Q4 | 边缘服务职责 | B 档(认证 + 节点编排);Rust 同栈;ubuntu 原生二进制;独立 crate(零系统库依赖) |
| Q5 | 隧道方案 | **自研 WSS + 轻量多路复用**(不用 frp/rathole/yamux),满足"全 Rust + 项目自带一切" |
| Q6 | 认证模型 | 配对码(PC daemon 端生成 6 位码 60s 过期)+ devices 表 token + shared_secret 防伪 daemon;单用户 |
| Q7 | 隧道传输形态 | **传 HTTP 原文**,PC daemon 收到 Request 帧后用 reqwest 打自己 loopback(`localhost:7456`);agent core 零改动;PWA 用同一套 API |
| Q8 | PC daemon 定位 | **一等公民独立可用**;remote 是 opt-in 附加层;remote_url/secret 存 DB,前端 settings 配置,实时生效(IPC 通知重连) |
| Q9 | HTTPS | 用户自理(nginx 转发 + 证书);remote daemon 只跑端口 |
| Q10 | DB 隔离 | 每台 PC daemon 独立 SQLite;remote 不存 agent 数据,只存 token/devices/配对码;不做跨节点联邦 |
| Q11 | 遥控/移动适配 | PWA 全权访问(复用前端自带能力,不做权限分层);移动端适配走**中限**(单栏 tab 切换 + 输入框/弹窗移动端友好),不追求原生体验;权限白名单/触屏手势推后 |
| Q12(D) | crate 组织 | **翻 Cargo workspace** + 独立 crate `everlasting-remote`(零依赖 daemon 重库,二进制最小);详见 [S1 design §1.2](../08-11-remote-daemon-core/design.md#12-模块边界新增) |
| Q13(D) | SSE 隔离 | **全局流透传,remote/client 按 request_id 过滤**(现状 SseRegistry 是单全局 broadcast,不按 session_id);零改 agent core;详见 [S2 design §2.3](../08-11-tunnel-client/design.md#23-sse-流式response--stream-帧sse-全局流透传策略) |

## 配对流程(bootstrap)

```
1. 部署 remote daemon 到云服务器(手动跑二进制 + nginx 反代 + 证书)
2. PC daemon 启动(本地,无 remote 配置 → 纯本地模式)
3. PC 前端 settings → Remote tab → 填 remote_url(wss://yourdomain.com/ws)+ shared_secret → 存 DB
   → IPC 通知 daemon → daemon 起 tunnel client → 连 remote(带 secret 认证)
   → remote 校验 secret 通过 → 接受连接,注册"这台 PC daemon"(node_id)
4. PC 前端点"生成配对码" → daemon 经 WSS 请求 remote 生成 → remote 返回 6 位码 → PC GUI 展示(60s 过期,一次性)
5. 手机 PWA 打开 → 输入配对码 → remote 验证 → 签发 device_token + 记录"该手机 ↔ 该 PC daemon"绑定
6. 手机用 device_token 访问 → remote 查表 → 通过对应 PC 的 WSS 转发到 PC daemon loopback → 响应回传
```

接第二台 PC(如家里):家里 PC 走步骤 2-4,手机走步骤 5(第二次配对)。device_token 隐含绑定到那台 PC,remote 按 token 路由,不需要"选节点"抽象 —— 手机首页 = 已配对 PC 卡片列表,点一张进详情。

## 隧道协议(自研轻量多路复用)

单条 WSS 长连接承载所有手机请求 + SSE 流,三类帧:

| 帧类型 | 方向 | 内容 |
|---|---|---|
| `Request { id, method, path, headers, body }` | remote → PC | 一次 HTTP 请求 |
| `Response { id, status, headers, body }` | PC → remote | 非流式响应 |
| `Stream { id, chunk \| end \| error }` | 双向 | SSE/流式响应(SSE chunked 转 Stream 帧,按 id 关联请求) |

- request_id 关联:tokio `mpsc` + `oneshot` + `HashMap<request_id, Sender>`
- 心跳:ping/pong 30s,90s 无响应判 PC 离线 → 更新节点状态 API
- 断线重连:指数退避;in-flight 请求不做迁移,重连后返 502 让前端重试(MVP 简化)
- 取消传播:手机断 SSE → remote drop 映射 → 发 `Stream { id, end }` → PC sender drop → agent loop CancellationToken 触发(复用现有机制)

序列化:JSON(MVP)或 bincode(优化期),几百行能跑。

## Requirements

### 功能需求

1. **remote daemon**(独立 crate `everlasting-remote`,Rust,ubuntu 原生二进制)
   - WSS 服务端:收 PC daemon outbound 连接,维持 `node_id → conn` 路由表
   - 配对码生命周期:PC daemon 经 WSS 请求生成 6 位码,60s 过期,一次性
   - devices 表(独立 SQLite):`device_token → node_id` + `last_seen_at`
   - 反向代理:手机 HTTP 请求 → 查 token → 通过对应 WSS 传 HTTP 原文 → PC loopback → 响应回传
   - SSE 桥接:PC 的 SSE chunked → WSS Stream 帧 → 手机的 SSE 连接
   - 节点状态 API:`/api/v1/nodes`(基于 WSS 心跳判在线/离线),给 PWA 首页
   - shared_secret 校验:PC daemon 连 WSS 时验证(配置一个 secret,所有 PC 共享)
   - 监听端口(用户 nginx 反代 + HTTPS)

2. **PC daemon tunnel client**(在现有 `everlasting-daemon` 加 `tunnel` 模块)
   - 启动读 settings(remote_url + shared_secret),有则起后台 task;无则纯本地模式
   - 维持 WSS 长连接(心跳 ping/pong 30s,断线指数退避重连)
   - 接收 Request 帧 → reqwest 打 `localhost:7456` → 响应塞回 WSS
   - 本地 SSE 响应 → 转 Stream 帧持续推送
   - **agent core 離改动**
   - 配对码生成 API(本地 IPC,PC 前端调用展示)
   - settings 加 remote 配置项(remote_url / shared_secret),存 DB,实时生效(IPC 通知重连)

3. **前端**
   - PC 前端 settings 加 "Remote" tab(remote_url + shared_secret + 生成配对码按钮)
   - PWA manifest + service worker(加到主屏幕,离线壳)
   - PWA 首页 = 已配对节点卡片列表(在线/离线状态)
   - 选节点后 = 现有完整前端(project/session/chat/... 全套复用)
   - 未配对时 = 配对码输入界面
   - transport 层:识别"我是 PWA 环境"→ baseURL 指向 remote daemon 域名
   - 移动端中限适配:三栏 → 单栏 tab 切换 + chat 输入框移动端友好 + permission/ask 弹窗移动端居中

### 非功能需求

- PC daemon 本地功能零依赖 remote(remote 挂了本地照常)
- ubuntu 原生二进制可执行(无 docker / 无 runtime)
- 单用户模型(无多租户)
- 不做主动推送(永久,非延后)
- 安全:P0 解决裸暴露公网(扫描器每天扫);token + HTTPS + nginx + shared_secret 多层

### 不做(明确推后/排除)

- ❌ 主动推送(Web Push / IM webhook)—— 永久不做
- ❌ 多用户 —— 永久不做
- ❌ 远程读写不对称权限层(PWA 等权)—— V2 视实际使用再评估
- ❌ 跨节点状态聚合(各 PC 数据隔离)—— 不做
- ❌ per-PC 凭证(共享 secret)—— V2
- ❌ 隧道断线 in-flight 请求迁移 —— MVP 返 502
- ❌ 触屏手势 / 紧凑视图重设计 / 移动端原生体验 —— V2

## 子任务拆分(5 个,按依赖顺序)

```
S1 remote-daemon-core          S2 tunnel-client          S4 pairing-and-pwa
(独立 crate + WSS 服务端 +     (PC daemon tunnel 模块)    (配对 UI + PWA 壳 + 节点列表)
 devices 表 + 反向代理骨架)          │                          │
        │                            │                          │
        └────────────┬───────────────┘                          │
                     ▼                                          │
              S3 e2e-tunnel-pipeline                            │
              (Request/Response/Stream 帧协议 + SSE 桥接 +      │
               心跳/重连 + shared_secret 校验 双端联通)           │
                     │                                          │
                     └──────────────┬───────────────────────────┘
                                    ▼
                          S5 mobile-adaptation
                          (移动端中限:单栏 tab + 输入框/弹窗适配)
```

- **S1 + S2 可并行**(两端独立开发,S3 联通)
- **S3 依赖 S1 + S2**(双端就绪才能联调帧协议)
- **S4 依赖 S3**(PWA 要能真正访问才有意义,但配对 UI / PWA 壳可提前做)
- **S5 依赖 S4**(在能用的基础上做移动端适配)

详见各子任务 PRD。

## 与 ROADMAP 的关系

本 epic 推进 [REMOTE-ACCESS-ROADMAP Phase 3](../../../docs/REMOTE-ACCESS-ROADMAP.md) 从"远期规划"进入"实施",并替换原定中继方案:

| 原规划 | 本 epic |
|---|---|
| Phase 3 P3.1 配对码 + devices 表 + token 中间件 | ✅ 落地(配对码 + devices 表 + token) |
| Phase 3 P3.2 HTTPS(Let's Encrypt / CF Tunnel) | 用户自理 nginx + 证书 |
| Phase 3 P3.3 读写不对称 | ❌ 推后(PWA 等权,V2 视使用再评估) |
| Phase 3 P3.4 token 存储 XSS 防护 | MVP 接受(localStorage,V2 评估 httpOnly cookie) |
| Phase 3 P3.5 Cloudflare Tunnel / Tailscale Funnel | ❌ 替换为国内服务器 + 自研 WSS 隧道 |
| B11 云端同步(Cloudflare Workers + D1) | ❌ 替换为国内服务器;不做跨节点同步(数据隔离) |

ROADMAP §2 第四档 B11 行需更新:从"Cloudflare Workers + D1"改为"国内服务器 + remote daemon(本 epic 落地)"。

## 验收标准(epic 级)

- [ ] PC daemon 配置 remote 后,后台维持 WSS 长连接,本地功能零影响
- [ ] PC daemon 不配 remote 时,行为与现状完全一致(回归)
- [ ] 手机 PWA 输入配对码 → 拿到 device_token → 能访问已配对 PC 的完整前端
- [ ] 手机发消息 → 经 remote WSS → PC daemon loopback → agent 响应 → SSE 流回手机(实时可见)
- [ ] 手机触发 permission:ask → 弹窗 → 手机点允许 → agent 继续
- [ ] 第二台 PC(家里)单独配对,手机首页看到两个节点卡片,在线/离线状态正确
- [ ] 家里 PC 关机 → 节点状态变离线 → 手机点进去显示离线提示(不白屏)
- [ ] shared_secret 错误的 fake daemon 连不上 remote
- [ ] remote daemon 是 ubuntu 原生二进制,无 docker 无 runtime
- [ ] 移动端中限适配:三栏变单栏 tab,输入框/弹窗移动端可点可用

## Notes

- 本 PRD 是 epic 的 single source of truth,所有架构决策汇总在此。各子任务 PRD 不重复决策,只细化执行。
- grill-me session 原始问答见会话记录,关键认知修正:**PC daemon 是一等公民**(Q8),这根本性改变了 remote 的定位 —— 它是 opt-in 附加层,不是基础设施重构。

## Review 修订记录(2026-08-11 deepseek-v4-flash 评审)

S1 + S2 design 经评审,3 个跨任务 P1 + 各自 P2/P3 已全部吸纳修订。**关键修订(影响多任务的契约)**:

| 修订 | 影响范围 | 落地位置 |
|---|---|---|
| **协议 crate 时序**:S1 首个 commit 创建 `everlasting-remote-protocol`(非 S3 抽出) | S1 §1.2 + S2 §1.3 | [S1 design §1.2 P1-1](../08-11-remote-daemon-core/design.md) |
| **SSE query token 认证**:EventSource 无法带 header,auth 支持 `?access_token=` | S1 §3.1 auth 契约 + S2 dispatcher 透传 | [S1 design §3.1 P1-2](../08-11-remote-daemon-core/design.md) |
| **remote ServeDir 托管 PWA**:PC daemon ServeDir NAT 后够不到,remote 必须自伺服 | S1 scope + S4 验收 | [S1 design §1.3 不变量 5 / §3.1 P1-3](../08-11-remote-daemon-core/design.md) |
| **tunnel 只活 daemon 进程**:Thin 模式 GUI 无 DB/server,lib.rs 不改 | S2 §4.2 重写 | [S2 design §4.2 P1-1](../08-11-tunnel-client/design.md) |

P2/P3(S1:pending 超时 + 配对码限速 + ErrorCategory 5 变体 + nginx timeout 等;S2:hostname dep + URL 编码 + remote_url 校验 + Full 模式说明等)均在各自 design 内就地修订,见 [S1 review](../08-11-remote-daemon-core/review.md) / [S2 review](../08-11-tunnel-client/review.md)。

**复评结论**:S1 + S2 design 可批准进入实施,3 个跨任务 P1 契约已写死各 design。S3/S4/S5 实施前需对照本表确认契约一致。
