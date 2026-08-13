# Review — S1 remote daemon core(WSS 服务端 + devices 表 + 反向代理骨架)

> 评审日期:2026-08-11。评审对象:`prd.md` + `design.md`(status=planning,实施前评审)。
> 方法:对 design 引用的代码库事实逐条核验(`Cargo.toml` 结构 / axum feature / `SseRegistry` / `app_config` / `ErrorCategory` / `crypto.rs` / 前端 `http.ts` / CI workflow / `sidecar.rs`);对跨任务契约(S1↔S2)做依赖核对。
> 关联评审:[S2 review](../08-11-tunnel-client/review.md)(P1-1~P1-3 跨任务项两份 review 都写,便于各自目录独立阅读)。

## 总体评价

design 质量高——**事实基础扎实(workspace 翻转、axum ws feature、SSE 单全局、app_config KV、crypto 机制全部核验属实),架构决策与 epic 已确认决策一致且可落地**。loopback 反向代理骨架、"remote 落库配对码(PC 触发)"、"`/api/v1/proxy/*` 前缀"、WS ping/pong 离线判定、panic 强制 secret 等取舍都正确。

**结论:可批准进入实施**,但需先处理 3 个 P1(2 个跨任务契约 + 1 个托管归属)—— 其中**协议 crate 时序矛盾(S1 与 S2 并行断链)**必须在 S1 开工前定;P2/P3 顺手改清。

## ✅ 核验通过(证据确凿)

| 声明(design) | 核验结果 |
|---|---|
| 单 crate + 两 bin,非 workspace,无根 `Cargo.toml` | **属实**:`app/src-tauri/Cargo.toml` `[[bin]] everlasting-daemon`;仓库根无 Cargo.toml |
| axum 0.7 无 `ws` feature(现状 `features = ["macros"]`) | **属实**(Cargo.toml:143);0.7 加 `ws` feature 即内置 WS 支持,前提成立 |
| reqwest 已是 daemon dep | **属实**:`reqwest 0.13`(stream/json/rustls/gzip/brotli/deflate);remote 侧需自己加 |
| `ErrorCategory` 5 变体 + PascalCase | **属实**(`error.rs:33` + `serde(rename_all = "PascalCase")`);§3.5 漏 RateLimit → P3-1 |
| RULE-D-001 `crypto.rs` encrypt/decrypt(master_key, aad) | **属实**(`crypto.rs:56/82`);S2 Q-T5 复用路径存在 |
| `app_config` KV 表 + `get/set_config_value` | **属实**(`db/config.rs`,schema.rs `CREATE TABLE app_config`) |
| SSE 单全局 fan-out + `subscribe(last_event_id)` → (replay, live) | **属实**(`sse.rs:166`,`daemon/routes/stream.rs:60`);**Last-Event-ID 端到端回放链路成立**(手机重连/隧道断恢复无需隧道层状态——本设计最大亮点) |
| 前端单全局 EventSource + 自动回带 Last-Event-ID | **属实**(`app/src/transport/http.ts:229-245`);同时**确认 EventSource 无法带自定义 header** → P1-2 |
| daemon `ServeDir` + SPA fallback 伺服前端 | **属实**(`daemon/server.rs:85`);remote 可复用 → P1-3 |
| daemon 端口 `--port` > env > 7456 | **属实**(daemon main 文档);Q-T6 `local_port` 传入方案可行 |
| `scripts/daemon.sh` 存在、CI `working-directory: app/src-tauri` 不变 | **属实**;但 CI rust-cache `workspaces` 键要改 → P3-3 |
| 心跳 30s/90s 判离线、`node_offline` → 502 | 与 daemon 现有 `ErrorCategory::Network` 语义一致,无冲突 |
| 配对码 remote 落库(PC 触发)与 PRD "PC 端生成"表述微调 | 语义一致(PC 发起、remote 记账),正确 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — [跨任务] `everlasting-remote-protocol` crate 时序矛盾,S1/S2 并行断链

**位置**:S1 design §1.2(`everlasting-remote-protocol/ [新,S3 抽出;S1 先在 frame.rs 本地定义]`)vs S2 design §1.3 / Cargo.toml(`everlasting-remote-protocol = { path = ... }`,`复用 S1 的`)。

**问题**:epic 决策 S1 + S2 **可并行**,但 S2 依赖的共享 crate 被排在 S3 才抽。S2 开工时 crate 不存在 → S2 无帧类型可用,只能临时本地复制(又回到双写)。时序自相矛盾。

**建议**:S1 实施时**直接创建 `crates/everlasting-remote-protocol`**(纯类型 enum + serde,几十行,零依赖),作为 S1 的第一个 commit;frame.rs 不落地或落地后立即被 crate 替代。S3 的"抽出"改为"双端确认改用 crate、删除任何残留本地定义"。这修的是依赖序,不是范围膨胀。

### 🔴 P1-2 — [跨任务] SSE 认证缺口:EventSource 无法带 `Authorization` header

**位置**:S1 design §3.1(`* /api/v1/proxy/*` 认证 = `Authorization: Bearer <device_token>`)+ 前端 `http.ts:239`(`new EventSource(url)`,原生 API 无 header 能力)。

**问题**:手机 PWA 的 SSE 走 `${remoteDomain}/api/v1/proxy/api/v1/stream`,必须带 token 才能过 auth 中间件;但浏览器 EventSource **不能设置请求头**。按现状契约,手机 SSE 必然 401。S4 会在这里卡死,且 S1 的 auth 设计没留口子。

**建议**:S1 `auth.rs` 中间件定为 **"Bearer header 或 `?access_token=` query 二选一"**(SSE 路径必然走 query);认证后从 path/query 剥掉 token,转发给 PC 的 Request 帧**不带 token**(与 P2-1 同一步)。S4 构造 EventSource URL 时 append `?access_token=...`。契约在 S1 写死,写进 §3.1 表格。

### 🔴 P1-3 — [跨任务] PWA 静态文件托管无主

**位置**:epic PRD 部署段(只写"部署 remote daemon + nginx 反代 + 证书")+ S1 routes(无 ServeDir)+ S4 验收("手机浏览器打开 remote 域名 → 加载 PWA")。

**问题**:手机从 remote 域名加载前端代码,但**没有任何组件在 remote 域名伺服 dist**。PC daemon 的 ServeDir(server.rs:85)在 NAT 后,手机够不到;S1-S5 没有任务负责这件事。

**建议**:二选一,必须在 S1/S4 开工前定:
- **(a) remote daemon 加 `tower-http::services::ServeDir` + SPA fallback**(与 PC daemon 同款,纯 Rust,不破坏"零系统库依赖";部署 = 服务器上 scp 一份 dist)→ **推荐**,符合 Q5 "全 Rust + 项目自带一切"
- (b) nginx 直接伺服静态目录(用户手动上传 dist,部署文档多一步)

选 (a) 则 S1 scope 加一条 ServeDir + `not_found_service(index.html)`(几行),S1 是部署地基,顺带做掉最省事。

### 🟡 P2-1 — Authorization 剥离只写在 S2,没写在 S1 自己的流程里

**位置**:S1 design §2.2 转发流程 vs S2 design §2.2 决策 Q-T2("remote 验证 token 后转 Request 帧时移除 Authorization")。

**问题**:该决定是 **remote 侧(本任务)的行为**,却只声明在 S2 的文档里。S1 与 S2 并行,S1 实施者按 §2.2 原样转发会带上 `Authorization: Bearer <device_token>` → PC daemon 无认证但 header 泄露 device token 到 PC 本地日志/代码路径(PC 侧不该知道手机 token)。

**建议**:S1 §2.2 数据流加显式一步:"构造 Request 帧时剔除 `Authorization` header(已消费;PC 本地无认证,且 token 不应出现在 PC 侧)"。与 P1-2 的剥 token 合并为同一处逻辑。

### 🟡 P2-2 — pending 无超时 + 结构未为 SSE 预留

**位置**:S1 design §2.2(`pending: DashMap<id, oneshot::Sender<Frame>>`,`预留 pending 槽位`)。

**问题**:
1. **无超时**:PC 静默/慢响应 → 手机 HTTP 请求挂死,`pending` 无界增长(内存泄漏)。PC 掉线而 registry 未及时清时尤其容易触发。
2. **结构只装得下非流式**:SSE 是持续流,`oneshot` 收一次就没了;S3 上 SSE 桥接时必然重构 `pending` 结构。

**建议**:S1 直接把 pending 设计成
```rust
enum PendingReply {
    Oneshot(oneshot::Sender<Frame>),        // 非流式:Response 一次
    Stream(mpsc::Sender<StreamEvent>),      // SSE:持续 chunk,End/Error 收尾
}
// DashMap<u64, PendingReply> + 每条挂 60s 超时 → 502/504 + 清理
```
S3 只加 Stream 分支的使用路径,不改结构。超时值 MVP 取 60s(与配对码同量级),`ErrorCategory::Network` + 502 返手机。

### 🟡 P2-3 — 配对码:无暴力破解限速 + 撞码无 retry

**位置**:S1 design §2.3 / §3.3(`pairing_codes.code TEXT PRIMARY KEY`,6 位数字)。

**问题**:
1. 6 位码 = 1M 空间 + 60s 窗口 + 公网可达(epic NFR 明确威胁模型是"扫描器每天扫")→ redeem 无速率限制时暴力空间可扫。
2. 两个 node 同时生成可能撞码 → `INSERT` PRIMARY KEY conflict → 500。

**建议**:redeem 加 per-IP 限速(如 10 次/分钟,内存计数即可,MVP 不需要 Redis);生成码时冲突 retry 2-3 次(重新随机)。两处都是几行。

### 🟢 P3-1 — ErrorCategory 对齐漏 RateLimit,且未提 PascalCase

**位置**:S1 design §3.5 vs `error.rs:33-39`。

**问题**:实际 5 变体:`Auth / RateLimit / InvalidRequest / Server / Network` + `#[serde(rename_all = "PascalCase")]`。§3.5 只列 4 个、没写大小写规则。前端按 category 路由(Auth→Settings / RateLimit→toast / ...),remote 若只实现 4 个,前端遇到 remote 的 429 会路由错乱。

**建议**:复制全部 5 变体 + PascalCase 序列化(§3.5 表格补一行说明)。

### 🟢 P3-2 — `/internal/` RPC 接收循环未定位到模块

**位置**:S1 design §2.3(配对码流图画了"PC → Frame::Request(/internal/...) → remote")。

**问题**:design 没说明 **ws.rs 的帧接收循环**在哪、如何分派:path 以 `INTERNAL_PREFIX` 开头 → remote 内部处理;否则 → S1 阶段无转发目标(PC 不发请求),log + 忽略(S2 后才有转发)。

**建议**:`routes/ws.rs` 加一小节"接收循环 + 分派规则"。另:S1 PRD 写 `POST /api/v1/internal/pairing/generate`,design 写 `/internal/pairing/generate`(INTERNAL_PREFIX)—— 以 design 为准,PRD 措辞顺手统一(这是 WS 内部 RPC,不是 HTTP 路由)。

### 🟢 P3-3 — [跨任务] workspace 翻转的 CI 尾巴

**位置**:S1 design §4.1("CI workflow 路径不变")vs `.github/workflows/ci.yml:57`。

**问题**:`working-directory: app/src-tauri` 确实不变,但 **`Swatinem/rust-cache` 的 `workspaces: 'app/src-tauri'` 在 Cargo.lock 迁到根后会缓存 miss**(不是失败,是每次全量编译,CI 时间从 ~60s 级涨到分钟级)。

**建议**:`workspaces: '.'`。另建议根 `Cargo.toml` 设 `[workspace] default-members = ["crates/everlasting-remote", "crates/everlasting-remote-protocol"]`——否则根目录裸 `cargo build` 会连 Tauri 重依赖一起编;daemon 侧构建仍显式 `-p everlasting`(行为不变)。

### 🟢 P3-4 — secret 常时比较 + nginx access log 记录 query

**位置**:S1 design §2.1(`auth::verify_shared_secret(query.secret)`)+ 部署 §6.1。

**问题**:
1. secret 比较建议 `subtle::ConstantTimeEq`(一行,防 timing side-channel;个人部署威胁低,但零成本)。
2. **query 传 secret 会被 nginx access log 记进 `$request`**(Q-T1 决策没提这点)—— 服务器日志里躺着 shared_secret。

**建议**:部署文档加一句:nginx `log_format` 去掉 `$request` 或在 `location /ws` 下 `access_log off`(或接受,单用户知情即可)。常时比较直接做。

### 🟢 P3-5 — nginx `/ws` location 建议补 `proxy_read_timeout`

**位置**:S1 design §6.1 nginx 示例。

**问题**:心跳 30s vs nginx 默认 `proxy_read_timeout 60s` 边界太紧(WS ping/pong 帧是字节流会刷新读超时计数器,但空闲期长时可能被掐)。

**建议**:示例加 `proxy_read_timeout 300s;`(与心跳间隔的倍数匹配),注释说明。

## 🟦 其他备注(可不动)

- **设计亮点**:Last-Event-ID → 隧道透传 → `SseRegistry` replay 端到端成立,手机 SSE 重连/隧道断恢复**无需隧道层任何状态**——与 Q13"全局流透传"决策完全吻合,是这套设计里最优雅的一环。
- `"/api/v1/proxy/*"` 前缀决策正确:隔离 remote 自身 API 命名空间,且与前端 `CMD_TO_DOMAIN`(http.ts:50)的 domain 分派模式兼容,transport 改造面小。
- 配对码"remote 落库、PC 触发"避免双写竞态,正确;`code PRIMARY KEY` + retry(P2-3)后无剩余竞态。
- 回滚形状干净:S1 全新增文件 + 根 Cargo.toml,`git revert` 单 PR 即回滚,daemon 零改动。
- 心跳由 remote 单端主导(客户端只回 pong),比双端互 ping 少一层状态机,正确。

## 复评建议

1. 定 P1-1(协议 crate 移进 S1 第一个 commit)+ P1-3(a/b 托管方案)→ 可 `task.py start`
2. P1-2 契约写进 §3.1 后与 S2 同步生效
3. P2/P3 实施前合并修订(单 commit doc 修,不必单独立项)
