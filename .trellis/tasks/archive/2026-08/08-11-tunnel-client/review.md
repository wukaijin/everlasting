# Review — S2 PC daemon tunnel client(tunnel 模块 + WSS 长连接 + loopback 转发)

> 评审日期:2026-08-11。评审对象:`prd.md` + `design.md`(status=planning,实施前评审)。
> 方法:对 design 引用的代码库事实逐条核验(`AppState` 字段数 / daemon main / `sidecar.rs` 双模式 / SSE 单全局 / `app_config` / IPC 三层模式 / 前端 `http.ts`);对跨任务契约(S1↔S2)做依赖核对。
> 关联评审:[S1 review](../08-11-remote-daemon-core/review.md)(P1-1~P1-3 跨任务项两份 review 都写,便于各自目录独立阅读)。

## 总体评价

design 质量高——**对代码库事实的掌握准确(17 字段 AppState、daemon main 结构、三层 IPC 模式、SSE 单全局、app_config 零 migration、reqwest 复用全部属实)**,opt-in 触发、loopback 转发(Q7)、`TunnelManager` watch 生命周期、SSE 取消只断订阅等决策与现有架构吻合,agent core 零改动的硬约束现实可行。

**结论:可批准进入实施,但 §4.2 "Tauri GUI 也 spawn tunnel" 是设计缺陷(P1-1,必须修)**——它既与 Thin 模式的代码事实矛盾,又会造成双连接互踢;另有 2 个跨任务契约(P1-2/P1-3)需要与 S1 同步落地。

## ✅ 核验通过(证据确凿)

| 声明(design) | 核验结果 |
|---|---|
| `AppState` 17 字段 | **精确**(`state.rs:74` 起,字段计数 = 17) |
| daemon main = `server::load_daemon_state` + `server::serve_daemon` | **属实**(`bin/everlasting-daemon.rs`;main 内 clap 解析 `--port`/`--data-dir`) |
| axum 0.7 无 `ws` feature(S2 是 WSS **客户端**,用 tokio-tungstenite,不依赖 axum ws) | **属实**(Cargo.toml:143);客户端选型独立,正确 |
| reqwest 已是 dep(provider 调用用) | **属实**:`reqwest 0.13`(含 stream feature → `bytes_stream()` 可用) |
| SSE 单全局 broadcast + `subscribe(last_event_id)` → (replay, live) | **属实**(`sse.rs:166`,`daemon/routes/stream.rs:60`);Last-Event-ID 透传端到端成立 |
| `app_config` KV + `get/set_config_value`,零 migration | **属实**(`db/config.rs`,schema.rs `CREATE TABLE app_config`);加新 key 不需 migration 的结论正确 |
| IPC 三层模式(`_inner` + `#[tauri::command]` + axum route)+ 前端 `CMD_TO_DOMAIN` | **属实**(`commands/config.rs` / `daemon/routes/config.rs` / `app/src/transport/http.ts:50`);"改 5 处"模式成立(数量勘误见 P3-2) |
| 端口解析 `--port` > env > 7456 | **属实**(daemon main 文档 + `server::resolve_port`);Q-T6 `local_port` 传入方案可行 |
| `sidecar.rs` Thin 模式:GUI 不 load AppState / 不开 pool / 不跑 HTTP server;Full 模式:无 sidecar、无 HTTP server | **属实**(`sidecar.rs` 模块文档)→ §4.2 的前提错误,见 P1-1 |
| 前端 SSE = 单全局 `new EventSource(url)`(http.ts:239) | **属实**;EventSource 无 header 能力 → P1-3 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — §4.2 "Tauri GUI 也 spawn tunnel" 是设计缺陷,双进程双连接互踢

**位置**:S2 design §4.2("Tauri GUI:`lib.rs::setup` 里 spawn(跟 sidecar daemon 启动同处)。否则 Tauri GUI 模式下手机连不上")。

**问题**,与代码事实三重矛盾:

1. **Thin 模式(默认)GUI 进程里 tunnel 无从启动**:`sidecar.rs` 明确 GUI 不 load `AppState`、不开 `SqlitePool`、不跑 HTTP server——tunnel 要读 `app_config`(无 DB)、dispatcher 要打 loopback(无 server),design 自己的前提在 GUI 进程不成立。
2. **双 spawn 必然双连接互踢**:若两进程都连 remote 且自报同一 `node_id`(hostname 派生,两台进程必然相同),S1 `tunnel_registry` 的"重复 node_id → 新连接踢旧"会让两个连接**互相踢,永续 flapping**。
3. **Full 模式更不成立**:无 sidecar、无 HTTP server,tunnel 无 loopback 可代理。

**真相**:"GUI 模式手机连不上"的担心不存在——Thin 模式 GUI 经 **sidecar daemon** 跑 agent core,sidecar 内 spawn 的 tunnel 天然覆盖 GUI 场景。**tunnel 只该活在 daemon 进程**。

**建议**:§4.2 改写为:
- tunnel 只在 `bin/everlasting-daemon.rs` 内 spawn(`lib.rs` 零改动,`spawn_tunnel_client` 放 `everlasting_lib::daemon::tunnel` 供 daemon bin 调用即可,不需要第二条 spawn 路径)
- 显式注明:Full 模式(逃生通道,前端走 Tauri IPC)下无 HTTP server,remote 不可用——可接受,文档注明
- 若未来真要 GUI 进程直连,node_id 必须与 daemon 区分(但那会产生"一台 PC 两个节点",不推荐)

### 🔴 P1-2 — [跨任务] 协议 crate 时序:S2 依赖的 crate 被 S1 排在 S3

**位置**:S2 design §1.3 / Cargo.toml(`everlasting-remote-protocol = { path = ... }`,标"复用 S1 的")vs S1 design §1.2(crate"[新,S3 抽出;S1 先在 frame.rs 本地定义]")。

**问题**:S1 + S2 并行,但共享 crate 排在 S3 才创建 → S2 开工时无帧类型可用(临时本地复制 = 双写,违背"帧定义单源")。

**建议**:已写入 [S1 review P1-1](../08-11-remote-daemon-core/review.md):**S1 第一个 commit 直接创建协议 crate**(纯类型,几十行)。S2 的依赖写法保持不变,实施前提 = S1 落地 crate;S1 未落地前 S2 可先做 config/IPC/TunnelManager 等不依赖帧类型的部分。

### 🔴 P1-3 — [跨任务] SSE 认证:EventSource 无法带 `Authorization` header

**位置**:S2 design §2.2 header 透传规则(Q-T2 只处理了"remote 剥 Authorization")+ 前端 `http.ts:239`(原生 EventSource,无 header 能力)。

**问题**:手机 PWA 的 SSE 走 remote proxy,EventSource 不能设 header → token 只能走 query(`?access_token=`)。S2 的 dispatcher 只透传 header 是不够的——**remote 侧认证契约必须支持 query token**,且转发给 PC 的 Request 帧要剥干净 token(不留 query 残渣,PC 的 `/api/v1/stream` 不需要也不该看到 token)。

**建议**:契约(已写入 [S1 review P1-2](../08-11-remote-daemon-core/review.md)):S1 auth 中间件接受 "Bearer header 或 `?access_token=` 二选一",认证后从 path/query 剥 token。S2 侧在 §2.2 加一句:"若 Request 帧 path 带 `access_token` query(SSE 路径),属 remote 未剥净的异常,dispatcher 原样透传即可(不主动剥),以 S1 契约为准"——两边各写各的,契约单源。

### 🟡 P2-1 — `hostname` dep 漏列 + query 参数未 URL 编码

**位置**:S2 design §3.3(`hostname::get()`)+ §2.1 连接 URL(`format!("wss://{}?secret={}&node_id={}&display_name={}")`)+ §1.2 Cargo.toml 新增清单(只列 `everlasting-remote-protocol` + `tokio-tungstenite`)。

**问题**:
1. `hostname` crate **不在** `app/src-tauri/Cargo.toml` 依赖里(grep 无结果)——新增清单漏了。
2. 连接 URL 直接 `format!` 拼接:**中文 display_name("公司 PC")、含特殊字符的 secret** 未 percent-encoding → 服务端 query 解析错乱或握手失败。

**建议**:补 `hostname` dep;query 值用 `urlencoding`/`percent-encoding` 编码;或简化:重连时只带 `node_id`(display_name 只在首次注册带,变更走配置)——query 面越小越不容易错。

### 🟡 P2-2 — `remote_url` 无校验/规范化,配置错误会无限重连

**位置**:S2 design §2.4(`set_remote_config` 直接存 DB)+ §2.1(`wss://{remote_url}?secret=...` 拼接)。

**问题**:用户可能填 `https://domain.com`(错 scheme)、`wss://domain.com/ws/`(尾斜杠 → `//?secret=` 断链)、带路径参数。按现状:写库成功 → tunnel 后台无限重连(错误配置,重连无意义,§6.2 只对"secret 错"做了停止处理)。

**建议**:`set_remote_config` 校验:scheme 必须 `wss://`(本地调试允许 `ws://`)、去尾斜杠、失败返 `InvalidRequest`(前端 inline 提示);`tunnel status` 增加 `config_invalid` 态或直接拒绝写库。

### 🟢 P3-1 — S2 PRD 与 design 心跳方向不一致

**位置**:S2 `prd.md`("心跳:30s ping,等 pong;90s 无 pong → 重连"——读作客户端主动 ping)vs design §1.1/§2.1(等 remote ping,响应 pong,与 S1 一致)。

**问题**:PRD 措辞会让实施者做成"双端互 ping"或"客户端 ping、remote 也 ping"的重叠心跳。design(remote 主导 ping)是对的、且与 S1 契约一致。

**建议**:PRD 心跳行改为"响应 remote 的 30s ping(回 pong);连接断开/90s 无帧 → 重连"。以 design 为准。

### 🟢 P3-2 — "5 处改动"列了 6 条

**位置**:S2 design §3.1 标题("新增 IPC(5 处改动全列)")+ 编号 1-6。

**问题**:列表实际 6 条(其中 2 条 `schema.rs` / `db/config.rs` 标注"无改动")。计数误导实施者核对清单。

**建议**:标题改"6 条(含 2 条确认无改动)"或把无改动两条移出编号列表。

### 🟢 P3-3 — Full 模式无 HTTP server 未显式说明

**位置**:S2 design §4.2(修正后补充)。

**问题**:§4.1 双模式只写了"本地 vs tunnel",没写 Full 模式(逃生通道)下 tunnel 不可用。

**建议**:与 P1-1 修正一并注明:"Full 模式(`?transport=tauri`)无 daemon HTTP server,tunnel 不 spawn,remote 仅 Thin/daemon 模式可用"。

## 🟦 其他备注(可不动)

- **loopback reqwest 转发(Q7)与现有架构完美匹配**:daemon 已有 ServeDir + httpTransport(79 路由),agent core 零改动约束现实可行;reqwest 复用(已 dep)不新增重量依赖。
- **SSE 取消只断隧道订阅、不传 agent loop** 与 `SseRegistry` broadcast 语义一致,正确——手机断开不应停 agent,显式 `/api/v1/cancel` 才停。
- **`TunnelManager` + watch 方案**与"config 实时生效"需求吻合;注意旧 task 优雅退出期间 in-flight 由 remote 侧 502 兜底(MVP 已声明,可接受)。
- **失败模式表(§6.2)覆盖到位**:secret 错停止重连(配置错误重连无意义)判断正确,与 P2-2 的"URL 错也要停"是同一原则的延伸。
- SSE chunk 边界不对齐 event 边界无碍(remote 原样写、浏览器 EventSource 自解析)——判断正确,隧道层保持纯字节透传。

## 复评建议

1. 修 P1-1(§4.2 重写为"tunnel 只活在 daemon 进程,lib.rs 不改")→ 可与 S1 并行开工
2. P1-2/P1-3 依赖 S1 契约落地,S2 开工先做不依赖帧类型的部分(config/IPC/TunnelManager),帧类型一到即接
3. P2/P3 实施前合并修订(单 commit doc 修)
