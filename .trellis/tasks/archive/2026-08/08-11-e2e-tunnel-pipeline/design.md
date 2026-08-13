# S3 Design — e2e 隧道管线(SSE 桥接 + 取消传播 + 双端联通)

> **What/Why**:见 [prd.md](./prd.md)。本文是 **How**。
> **决策汇总**:[parent PRD](../08-11-remote-control-epic/prd.md)。
> **S1 契约**:[S1 design](../08-11-remote-daemon-core/design.md) 的 `Frame` 协议 + proxy catch-all + `PendingReply` + `ws::dispatch_frame`。
> **S2 契约**:[S2 design](../08-11-tunnel-client/design.md) 的 tunnel client + `sse_bridge`(出站 chunk) + `client::serve_loop` 帧接收。
> **代码库事实** 基于 2026-08-12 三路探查:S1 已落地 `PendingReply::Stream` 变体但**未实例化**(`pending.rs:31` 定义、`proxy.rs:80` 注释 "S3 才用"、`ws.rs:250-253` 占位 log);S2 `sse_bridge::forward_stream` 出站已通(`dispatcher.rs:96` await 调用),`client.rs:285-289` 的 `Frame::Stream` 接收**只 log**;agent cancel 仅显式 `POST /api/v1/cancel/cancel_chat`(`commands/cancel.rs:26-44`),`SseRegistry` broadcast 与 agent `CancellationToken` **解耦**(单订阅者断开不停 agent)。

---

## 0. 已确认决策(2026-08-12,实施前)

| # | 决策 | 结论 | 裁决来源 |
|---|---|---|---|
| D1 | 取消传播语义 | **只停转发(Q-T3)**:断 SSE 只停隧道转发 + PC 本地 SSE 订阅断;**agent 不停**,停 agent 靠显式 `/api/v1/cancel` | [epic PRD 第 95 行](../08-11-remote-control-epic/prd.md)("断 SSE → agent cancel")与 [S2 design Q-T3](../08-11-tunnel-client/design.md)("只断订阅")矛盾 → 裁决从 Q-T3 |
| D2 | S3 范围 | **纯后端管线 + Rust harness**,不碰前端(token 注入 + PWA 留 S4 `pairing-and-pwa`) | S4 边界清晰,harness 用 reqwest 模拟手机带 token |
| D3 | 协议帧 | **不加新帧**,复用 `StreamEvent::End/Error` 作 remote→PC 取消信号 | 减少协议面,双向 `Frame::Stream` 反向语义已够用 |

**PRD 验收第 3 条按 D1 修订**(见 [prd.md Review 修订记录](./prd.md))。

### Review 修订记录(2026-08-12,design 评审后)

[review.md](./review.md) 8 条意见经**独立核证全部成立**(P1×2 + P2×4 + P3×2),design 各节已就地修订。关键核证(非复述 reviewer,自读原码):P1-1 核 `ws.rs:194` `dispatch_frame(...).await` 确在 `receive_loop` 内、Pong 续期在同循环 L182;P2-1 核 `sse.rs:161` `retain(|tx| tx.try_send(..).is_ok())` 确认 agent 发送永不阻塞。

| Review 项 | 修正 | 落地位置 |
|---|---|---|
| **P1-1** 接收循环内 `await mpsc send` → HoL 阻塞 + node flapping | Stream 分支改 `try_send` + 满则剔除(与 `SseRegistry` 语义对齐) | §2.1/§2.2/§3.1/§3.3 |
| **P1-2** cancel 按 node_id → 踢旧/重连窗口期误杀新连接在途流 | pending 记 **conn_id**,`cancel_streams_for_conn` 替代 `cancel_streams_for_node` | §2.3/§3.4 |
| **P2-1** "端到端背压"事实错误(SseRegistry 是 try_send 剔除) | 改"慢订阅者剔除重连(Last-Event-ID 回放),mpsc(128) 仅限内存" | §2.1/§5 |
| **P2-2** integration test 看不到私有 helper + 场景 3 需持续流 | harness 放 **lib 内 `#[cfg(test)]`** 复用现有 helper;场景 3 持续流 + `SseRegistry` 双订阅者证 agent 不停 | implement Step 5 |
| **P2-3** Response 命中 Stream pending 未定义 → 静默失败 + 误导日志 | 转 `StreamEvent::Error` 送 body 关闭 + 正确日志 | §3.1/§3.2 |
| **P2-4** 流式无首帧超时 → pending 永久泄漏 | 加首帧超时(30s 无帧 → remove + 关 body) | §3.1 |
| **P3-1** EventSource 502 不重连 | 补"502 后靠 S4 前端显式重试" | §2.3/§6.2 |
| **P3-2/3** pending.rs 文档过时 / PRD 帧伪代码 | Step 2 顺带更新文档;PRD 补"以 crate 为准" | implement Step 2 / prd.md |

> **方案差异(作者独立意见)**:P2-2 harness 放置 —— reviewer 建议"复制脚手架或抽 pub",本设计选**放 lib 内 `#[cfg(test)] mod`**(remote crate 作 dev-dep 在 lib test 编译时可用,直接复用 `tunnel/tests.rs` 私有 helper,零复制零 pub 污染)。

---

## 1. 架构总览

### 1.1 S3 完成态拓扑(双端 SSE 联通)

```
手机 PWA EventSource                云服务器 remote daemon              PC daemon
 GET /api/v1/proxy/api/v1/stream    (everlasting-remote)               (tunnel client)
   ?access_token=<T>                                                       
        │  HTTP 200                  ┌──────────────────────┐            │
        │  text/event-stream         │ proxy_handler        │            │
        │ ◄─────────────────────── │  (SSE 流式分支,S3 新)│            │
        │  Body::from_stream(mpsc_rx)│   pending.insert(id, │            │
        │                            │     Stream(mpsc_tx)) │            │
        │                            │                      │ Frame::    │
        │                            │                      │ Request    │
        │                            │ ws::dispatch_frame   │ ◄──────────│ (S2 已实现,
        │                            │  (Stream 路由,S3 新)│            │  dispatcher
        │                            │   PC 的 Stream::Chunk│            │  打 loopback)
        │                            │   → mpsc_tx.try_send     │            │
        │                            │   → 写进手机 body    │ Frame::    │
        │                            │                      │ Stream    │
        │                            │                      │ {Chunk}   │
        │                            │                      │ ──────────►│ sse_bridge
        │                            │                      │            │ (S2 已实现)
        │                            │                      │            │ ▼ loopback
        │                            │                      │            │   /api/v1/stream
        │                            │                      │            │   (SseRegistry)
        │                            │                      │            │
   手机断开(body drop)              │                      │            │
        │                            │ mpsc_tx.try_send 失败    │            │
        │                            │  → remove(id)        │ Frame::    │
        │                            │  → 发 Stream{End}    │ Stream     │
        │                            │   ─────────────────►│ {End}      │
        │                            │                      │            │ client.rs 收到
        │                            │                      │            │ → cancel_stream(id)
        │                            │                      │            │ → sse_bridge select!
        │                            │                      │            │   退出 → drop resp
        │                            │                      │            │ → 本地 SSE 订阅断
        │                            │                      │            │ ★ agent 不停(broadcast)
```

S3 补三块:**(A) remote SSE 桥接**(proxy 流式分支 + ws dispatch_frame Stream 路由)、**(B) PC 取消接收**(client.rs 收 End → cancel sse_bridge)、**(C) node 离线清理 stream pending**。

### 1.2 S1/S2 已实现 vs S3 新写(分界)

| 能力 | 状态 | 位置 |
|---|---|---|
| `Frame` / `StreamEvent{Chunk,End,Error}` 协议 | ✅ S1 已定义单源 | `everlasting-remote-protocol/src/lib.rs` |
| `PendingReply::Stream(mpsc::Sender<StreamEvent>)` 变体 | ✅ S1 已定义,未实例化 | `remote/pending.rs:31` |
| 非流式 Oneshot 链路(手机→PC→Response) | ✅ S1 已通 | `remote/routes/proxy.rs:65` |
| PC `sse_bridge` 出站(SSE chunk → Stream::Chunk) | ✅ S2 已通 | `daemon/tunnel/sse_bridge.rs:23` |
| PC dispatcher SSE 检测 + 转发 | ✅ S2 已通 | `daemon/tunnel/dispatcher.rs:94` |
| shared_secret 握手校验 | ✅ S1+S2 已通 | `remote/routes/ws.rs:70` |
| 心跳(remote ping / 90s 超时判离线) | ✅ S1 已通 | `remote/routes/ws.rs:130` |
| **remote proxy SSE 流式分支** | 🔴 S3 新写 | `remote/routes/proxy.rs` |
| **remote ws dispatch_frame Stream 路由** | 🔴 S3 新写(替换 `ws.rs:250` 占位) | `remote/routes/ws.rs` |
| **remote node 离线清理 stream pending** | 🔴 S3 新写 | `remote/routes/ws.rs` 离线路径 |
| **PC client.rs Stream 接收 → cancel** | 🔴 S3 新写(替换 `client.rs:285` log) | `daemon/tunnel/client.rs` |
| **PC sse_bridge 取消分支(select! on token)** | 🔴 S3 新写 | `daemon/tunnel/sse_bridge.rs` |
| **端到端 harness(5 场景)** | 🔴 S3 新写 | `app/src-tauri/tests/tunnel_e2e.rs` + remote crate 集测 |

### 1.3 关键不变量(硬约束)

1. **不破 S1 非流式 Oneshot 路径** —— proxy 流式是新分支(Accept 判定后分流),Oneshot 路径原样保留。
2. **不破 S2 `sse_bridge` happy path** —— 出站 chunk 逻辑不动,只加 `select!` 取消分支。
3. **agent core 零改动**(延续 S1/S2 硬约束) —— 取消信号只到 `sse_bridge` 层,不进 `agent::`。
4. **断 SSE 不停 agent**(D1) —— `SseRegistry` 是 broadcast,单订阅者(隧道)断开只从 `senders` 移除,agent `CancellationToken` 不触发。
5. **不引入新协议帧**(D3) —— 双向 `Frame::Stream` 的反向(remote→PC)语义兼任取消信号。

---

## 2. 数据流

### 2.1 SSE 流式上行(手机看实时 agent 输出)

```
手机                    remote proxy                remote ws dispatch         PC tunnel(sse_bridge)      PC loopback
 │  GET /api/v1/proxy/api/v1/stream                                            │                          │
 │  Accept: text/event-stream                                                  │                          │
 │  ?access_token=<T>                                                          │                          │
 │ ───────────────────► │                                                       │                          │
 │                      │ auth 中间件解 token → node_id                         │                          │
 │                      │ tunnel_registry.get(node_id)                          │                          │
 │                      │ 判定 SSE:Accept 含 text/event-stream                  │                          │
 │                      │ id = pending.next_id()                                │                          │
 │                      │ (mpsc_tx, mpsc_rx) = channel(128)                      │                          │
 │                      │ pending.insert(id, conn_id, Stream(mpsc_tx))          │                          │
 │                      │ Frame::Request{id, GET, /api/v1/stream, hdrs, []}     │                          │
 │                      │  (path 剥 /api/v1/proxy 前缀 + access_token query)     │                          │
 │                      │ conn.send_frame ──────────────────────────────────► │                          │
 │                      │ 返 HTTP 200 + Body::from_stream(mpsc_rx)              │                          │
 │                      │  (content-type: text/event-stream)                    │                          │
 │ ◄────────────────── │                                                       │ dispatcher → reqwest GET │
 │                      │                                                       │   localhost:7456/stream  │
 │                      │                                                       │ ◄──────────────────────►│ axum SseRegistry
 │                      │                                                       │   .subscribe(last_event_id)
 │                      │                                                       │ ◄── SSE chunk ──────────│
 │                      │                                                       │ sse_bridge 包 Stream::   │
 │                      │                       Frame::Stream{id, Chunk{bytes}} │  Chunk 发回 remote       │
 │                      │ ◄────────────────────────────────────────────────── │                          │
 │                      │ dispatch_frame:get(id) → Stream(mpsc_tx)              │                          │
 │                      │  mpsc_tx.try_send(Chunk) 满则剔除(P1-1)              │                          │
 │ ◄── chunk bytes ──── │  (Body::from_stream 写手机 body)                      │                          │
 │                      │                                                       │                          │
 │                      │                  … 重复直到 PC SSE 结束 …             │                          │
 │                      │                       Frame::Stream{id, End}          │                          │
 │                      │ ◄────────────────────────────────────────────────── │ (loopback SSE 关闭)      │
 │                      │ dispatch_frame:End → remove(id) + body stream 结束     │                          │
 │ ◄── body 关闭 ────── │                                                       │                          │
```

**关键点**:
- **SSE 识别**:proxy_handler 检查 `Accept` header 含 `text/event-stream` → 流式分支。EventSource 原生带此 header,无需前端额外配合(D2 下前端 token 注入留 S4,但 Accept header 是浏览器自动的,不依赖 token 注入)。
- **裸字节透传**:remote 不解析 SSE 语义(`id:/event:/data:`),把 `StreamEvent::Chunk{bytes}` 原样写进 HTTP body,浏览器 EventSource 自己按 `\n\n` 解析(沿用 S2 `sse_bridge` 的纯字节透传契约)。
- **慢手机处理 = try_send + 剔除**(P1-1 修订,与 daemon `SseRegistry` 语义对齐):`dispatch_frame` 的 Stream 分支用 **`mpsc_tx.try_send(Chunk)`**,不 `await`。满(`Full`)或断(`Disconnected`)统一走"剔除"路径 —— `remove(id)` + 发 `Frame::Stream{id,End}` 给 PC + drop tx → 手机 body 的 `ReceiverStream` 收到 channel 关闭 → body 结束 → EventSource 重连(Last-Event-ID 回放补)。**mpsc(128) 仅限内存**,不提供端到端背压(P2-1:daemon `SseRegistry` 是同步 `try_send` + retain 剔除慢订阅者,agent 发送永不阻塞,故不存在"手机慢→agent 阻塞"的背压链;真实行为是慢手机被剔除重连)。
- **为什么不能 `await send`**(P1-1):`dispatch_frame` 被 `receive_loop` **直接 await**(`ws.rs:194`),若 Stream 分支 `await send` 阻塞 → 同循环的 Pong 续期(L182)饿死 → 心跳判 stale → 在线 node 误标 offline → **node flapping**;同 node 的配对 RPC(internal 分支)、其他 Response 路由全部 HoL 阻塞。`try_send` 保证接收循环永不被单条流拖住。

### 2.2 取消传播(只停转发版,D1)

```
手机断开 EventSource        remote proxy/ws           remote → PC WSS        PC client/sse_bridge       PC loopback
 │ body future drop            │                         │                      │                          │
 │                             │ mpsc_rx drop            │                      │                          │
 │                             │ (Body::from_stream 消费端断)│                   │                          │
 │                             │                         │                      │                          │
 │                             │ ws dispatch_frame 收到 PC 下一个 Chunk:        │                          │
        │                             │   mpsc_tx.try_send(Chunk) → Err(Full|Disconnected) │                       │
        │                             │   → 手机慢/断,剔除(remove+发End)│              │                          │
 │                             │   → pending.remove(id)  │                      │                          │
 │                             │   → conn.send Frame::Stream{id, End}           │                          │
 │                             │   ───────────────────► │                      │                          │
 │                             │                         │ client.rs serve_loop │                          │
 │                             │                         │  收到 Frame::Stream   │                          │
 │                             │                         │  {id, End}            │                          │
 │                             │                         │  (remote→PC = 取消)   │                          │
 │                             │                         │  → tunnel_manager     │                          │
 │                             │                         │    .cancel_stream(id) │                          │
 │                             │                         │  → CancellationToken  │                          │
 │                             │                         │    .cancel()          │                          │
 │                             │                         │                      │ sse_bridge select! 退出 │
 │                             │                         │                      │  → drop reqwest resp    │
 │                             │                         │                      │  → loopback SSE 连接断  │
 │                             │                         │                      │ ◄── axum 检测客户端断 ──│
 │                             │                         │                      │    SseRegistry.senders  │
 │                             │                         │                      │    移除该订阅            │
 │                             │                         │                      │ ★ agent loop 不停        │
 │                             │                         │                      │   (broadcast;显式        │
 │                             │                         │                      │    /cancel 才停)         │
```

**与原 PRD 的差异**(D1):原 PRD 第 95 行 / 验收第 3 条期望"断 SSE → agent CancellationToken 触发 → agent 停"。**修订为**:断 SSE 只到 `sse_bridge` 层(drop resp),agent 继续。理由见 §5 权衡。

**断点兜底**:若 PC 在手机断开后**不再发任何 Chunk**(loopback SSE 静默),remote 的 `ws dispatch_frame` 不会被触发去发 End。两条兜底:
1. **PC 端 KeepAlive**:loopback `/api/v1/stream` 的 axum `Sse` 带 `KeepAlive(30s : ping)`(`stream.rs:64`),每 30s 产一个 ping 帧 → `sse_bridge` 转 Chunk → remote dispatch_frame 触发 → 感知手机断。**最坏 30s 延迟感知**。
2. **remote 端 SSE body 探活**(可选,见 §5):remote 给手机 SSE body 加自己的 idle 探测;但 MVP 依赖方案 1(PC KeepAlive 已有)。

### 2.3 node 离线清理(心跳超时 / clean disconnect)— 按 **conn_id**(P1-2 修订)

S1 离线路径(`ws.rs:153-161` 心跳超时、`ws.rs:214-218` 接收循环退出)目前只 `remove_if_current` + `update_node_status(offline)`。S3 加一步,**按 conn_id 精确清理**(非 node_id):

```
ws 离线路径(心跳超时 / Close / 读错误),持 handle → conn_id:
  → remove_if_current(node_id, conn_id)          [S1 已有]
  → update_node_status(offline)                  [S1 已有]
  → pending.cancel_streams_for_conn(conn_id)     [S3 新增,P1-2]
      扫所有 (conn_id, PendingReply::Stream) 匹配本连接的流:
        → mpsc_tx.try_send(StreamEvent::Error{message: "node_offline"})  // 失败直接 remove
        → remove(id)
      → 手机 SSE body 收到 Error → body 结束 → EventSource 重连(见末尾)
```

**为什么按 conn_id 不按 node_id**(P1-2):S1 注册语义是"重复 node_id → 新连接踢旧"(`ws.rs:105-108`)。PC 重连场景下,旧连接 C1 的退出清理(`cancel_streams_*`)与新连接 C2 服务新请求之间存在**窗口**:C2(conn_id=1)已接管 node 并服务新手机流,旧 C1(conn_id=0)的退出清理若按 `node_id` 扫,会**误杀 C2 的在途流**。按 `conn_id` 清理与 `remove_if_current` 防误删新连接同款思路,**精确到连接级,零误杀,实现成本相同**(proxy_handler 拿到的 `conn: Arc<ConnHandle>` 自带 `conn_id`,两条离线路径都持 `handle` → `conn_id` 可得)。

**需要 `PendingTable` 记 conn_id**(当前只记 `id → PendingReply`)。给条目加 `conn_id: u64` 字段(`insert(id, conn_id, reply)`),见 §3.4。

**非流式 Oneshot 不受影响**:Oneshot 的清理仍由 proxy_handler 的 60s 超时兜底;node 离线时在途 Oneshot 请求,下次 `conn` 已不可达(新请求直接 502),在途的等 Response 超时(60s)返 502。`cancel_streams_for_conn` 只扫 Stream 分支,不碰 Oneshot —— **MVP 不做 Oneshot 即时清理,靠 60s 超时**。

> **EventSource 重连边界(P3-1)**:离线清理触发的是**连接级** body 关闭,浏览器 EventSource 会自动重连;但重连请求打到 proxy 时 node 仍离线 → 502 → **EventSource 遇 HTTP 非 200/204 直接 fail 且不重试**(规范行为)。故 502 后的恢复**靠 S4 前端的显式 backoff 重试**,非 EventSource 自动。S3 侧无代码改动,此处仅澄清语义。

---

## 3. 契约 / 接口

### 3.1 remote proxy SSE 识别 + 流式分支

| 判定 | 行为 |
|---|---|
| `Accept` 含 `text/event-stream` | 流式分支(mpsc + Body::from_stream) |
| 其他 | 现有 Oneshot 分支(不动) |

**为什么按 Accept 而非 path**:① EventSource 原生带 `Accept: text/event-stream`,无需前端配合;② path 判定(`/api/v1/stream`)耦合 PC 路由,proxy 应通用;③ 普通请求不会误带此 Accept。

**流式分支返回**:
```rust
// 伪代码(实现时定具体 Error 类型)
let conn_id = conn.conn_id;                              // P1-2:按连接清理
let (tx, rx) = mpsc::channel::<StreamEvent>(128);
state.pending.insert(id, conn_id, PendingReply::Stream(tx));
conn.send_frame(&Frame::Request{ id, method, path, headers, body }).await?;
// 不等 Response,立即返流式 body。首帧超时见下方 P2-4。
let body_stream = ReceiverStream::new(rx).map(|ev| -> Result<Bytes, _> {
    match ev {
        StreamEvent::Chunk{bytes} => Ok(bytes.into()),
        StreamEvent::End => { /* 结束:yield None,ReceiverStream 自然 close */ }
        StreamEvent::Error{..} => { /* 异常关闭:yield Err 或结束流 */ }
    }
});
Response::builder()
    .status(200)
    .header("content-type", "text/event-stream")
    .header("cache-control", "no-cache")
    .body(Body::from_stream(body_stream))
```

**不用 axum `Sse<Event>`**:remote 是裸字节透传(`StreamEvent::Chunk` 已是 SSE body 原文),axum `Sse` 会要求 `Event` 帧化(重新构造 `id:/event:/data:`),破坏透传契约。`Body::from_stream` + 手动 content-type 才对(参考 daemon `stream.rs` 的 `Sse` 是给本机 `SseFrame` 序列化用的,场景不同)。

**首帧超时(P2-4)**:流式 Request 帧发出后,若 PC 永不回(loopback 挂死 / reqwest 无超时)且手机也不断 → 手机 body 永久悬挂 + pending 永久泄漏(§2.3 清理只在 node 离线触发)。**对策**:流式分支挂一个 30s 首帧 deadline —— 30s 内 `dispatch_frame` 未为该 id 推过任何 Chunk/End/Error → `remove(id)` + 关 body(写一个 `StreamEvent::Error{timeout}` 或直接 drop tx 让 ReceiverStream 结束)。首帧之后由 PC KeepAlive(30s `:ping` → Chunk)持续触发,无需再加流级超时。实现:pending 条目记 `first_frame_deadline`,dispatch_frame 首次命中时清除;或 body_stream 套 `take_until(sleep(30s))`。

**流式请求收到非流式响应(P2-3)**:若 PC loopback 回非 SSE(404/401 等),PC dispatcher 走非流式发 `Frame::Response` → remote `ws::dispatch_frame` 的 Response 分支(`ws.rs:241-249`)当前只匹配 `Oneshot`,命中 `Stream` 条目会 `remove` 后走 else 打误导日志 `"Response for unknown/unmatched request id"` 且**错误体丢失**(手机拿 200 SSE + 空 body 关闭,静默失败)。**对策**:Response 分支命中 Stream 条目 → 转 `StreamEvent::Error{message: format!("status={status}")}` 送 body 关闭 + 正确日志 `stream got non-stream response id=.. status=..`。手机已收到 200 无法改状态码(HTTP 限制),错误经 body 关闭传达;MVP `/api/v1/stream` 恒 SSE 低概率,但一行分支且 harness 404 场景会踩,顺手做。

### 3.2 `Frame::Stream` 双向语义(不加新帧,D3)

| 方向 | `StreamEvent` 变体 | 含义 |
|---|---|---|
| PC → remote | `Chunk{bytes}` | SSE 的一段(纯字节透传) |
| PC → remote | `End` | loopback SSE 正常结束(server 关连接) |
| PC → remote | `Error{message}` | loopback SSE 读错 / reqwest 失败 |
| **remote → PC** | `End` | **取消信号**(手机断 SSE,停转发) |
| **remote → PC** | `Error{message}` | remote 侧错误(如 node 离线清理),停转发 |

PC `client.rs` 收到任何 remote→PC 的 `Frame::Stream` → `cancel_stream(id)`(不存在该 id 则忽略,log)。

**为什么不加 `Cancel` 变体**:① `End` 语义上就是"流结束",取消=提前结束,复用自然;② 减少协议面,不破 S1/S2 已部署的帧定义;③ PC 侧处理统一(收到 End/Error 都停转发)。

### 3.3 PC 取消落地(TunnelManager + sse_bridge)

```
TunnelManager 加:
  stream_cancels: Mutex<HashMap<u64, CancellationToken>>
  register_stream_cancel(id) -> CancellationToken   // dispatcher SSE 分支调
  cancel_stream(id)                                  // client.rs 收 remote End 调

dispatcher.rs SSE 分支:
  let token = manager.register_stream_cancel(id);
  sse_bridge::forward_stream(id, resp, tx, token.clone()).await;  // 改签名加 token
  manager.stream_cancels.lock().remove(&id);  // 结束清理

sse_bridge::forward_stream 改为 select!:
  loop {
    select! {
      chunk = bytes_stream.next() => match chunk {
        Some(Ok(b)) => tx.send(Stream::Chunk{bytes:b}).await,  // P2-1:tx 是 UnboundedSender,send 不阻塞(对端 drop 才 Err)
        Some(Err(e)) => { tx.send(Stream::Error{..}); break; }
        None => { tx.send(Stream::End); break; }
      }
      _ = token.cancelled() => {
        // remote 发 End / 手机断 → 取消 → drop resp(本地 SSE 订阅断,agent 不停)
        break;
      }
    }
  }
  drop(resp);  // 显式断 reqwest(断 loopback SSE)
```

### 3.4 `PendingTable` 改动(P1-2 conn_id + P1-1 try_send)

```rust
// 现:DashMap<u64, PendingReply>
// 改:DashMap<u64, PendingEntry{conn_id: u64, reply: PendingReply, first_frame_deadline: Option<Instant>}>

// 新 API:
pub fn insert(&self, id: u64, conn_id: u64, reply: PendingReply);  // P1-2:conn_id 非 node_id
pub fn get(&self, id: u64) -> Option<Ref<PendingEntry>>;            // 只读查(Stream 多 chunk)
pub fn cancel_streams_for_conn(&self, conn_id: u64);                // P1-2:按连接清理(离线路径调)
pub fn note_first_frame(&self, id: u64);                            // P2-4:首帧到达,清 deadline
// remove(id) 保留
```

**`get` 持 guard + try_send 不跨 await**(P1-1 修订后风险消除):`dispatch_frame` 的 Stream 分支用 `try_send`(同步,不 `await`),故可持 DashMap `Ref` guard → `try_send` → 立即释放 guard,**不跨 await,无死锁风险**。原 design 担心的"持 guard 跨 await"问题因改 try_send 自动消除(若用 `send().await` 则需 clone tx 再 drop guard)。

**`cancel_streams_for_conn`(P1-2)**:扫所有 `PendingEntry` 且 `conn_id` 匹配 + `reply` 是 `Stream` 变体 → `mpsc_tx.try_send(StreamEvent::Error{node_offline})`(try_send,失败则直接 remove)+ `remove(id)`。只动 Stream,不碰 Oneshot(Oneshot 靠 60s 超时,§2.3)。conn_id 来自 proxy_handler 的 `conn.conn_id`(随手可得),离线路径持 `handle.conn_id`。

---

## 4. 兼容性 / 迁移

### 4.1 不破现状

- **非流式 Oneshot 链路**:`proxy_handler` 流式分支是 `if sse { ... } else { 现有 }`,Oneshot 路径 1:1 保留。S1 的 proxy/ws/pending 测试全绿即证明。
- **S2 `sse_bridge` happy path**:出站 chunk 循环保留,只在外层包 `select!` 加取消分支。token 未 cancel 时行为 = 现状。
- **PC `client.rs` 非 Stream 帧**:Request/Response 处理不动,只 Stream 分支从 log 升级为 cancel。

### 4.2 协议兼容

不加新帧(D3),S1/S2 已部署的 `Frame`/`StreamEvent` 定义**零改动**。双向 Stream 语义是"约定",不涉及序列化变更。老 PC(未升级 S3)收到 remote End 仍 log 忽略(S2 行为),只是取消不了 —— 升级 S3 后才生效,**渐进可部署**。

### 4.3 回滚

删 S3 新代码:remote proxy 流式分支回 Oneshot、ws dispatch_frame Stream 回 log、pending 回旧 API;PC client.rs Stream 回 log、sse_bridge 去 select!、TunnelManager 去 stream_cancels。`app/src-tauri/tests/tunnel_e2e.rs` 删;`dev-dependencies everlasting-remote` 删。回到 S2 完成态(非流式全通,SSE 不可用)。

---

## 5. 关键权衡

| 决策 | 选择 | 理由 | 否决项 |
|---|---|---|---|
| 取消语义 | 只停转发,不停 agent(D1) | ① 与 S2 design Q-T3 + `SseRegistry` broadcast 一致 ② SSE 重连/网络抖动不误杀 agent ③ 不破 broadcast 多订阅者语义 | 断 SSE 即停 agent:破 broadcast,抖动误杀,违背 Q-T3 |
| SSE 识别 | Accept header | EventSource 原生带,通用,不耦合 PC 路由 | path 判定:耦合 PC,非通用 |
| remote SSE body | Body::from_stream(裸字节) | 透传契约,Chunk 是 SSE 原文 | axum Sse<Event>:要求 Event 帧化,破透传 |
| mpsc 容量 | bounded(128) + **try_send 满则剔除**(P1-1) | 与 `SseRegistry` 语义对齐;**仅限内存**,慢手机被剔除重连(Last-Event-ID 回放补),非端到端背压(P2-1) | `send().await`:阻塞接收循环 → HoL + flapping(P1-1 否决) |
| 取消信号帧 | 复用 Stream{End/Error}(D3) | 减协议面,渐进可部署 | 新增 Cancel 变体:破已部署帧,无额外收益 |
| harness 形态 | lib 内 `#[cfg(test)]`(跨 crate dev-dep) | 确定性,复用现有 helper,能测时序敏感的 cancel/断线 | Node 脚本:Node 22 无全局 EventSource,时序 flaky |
| node 离线 stream 清理 | 按 **conn_id** 主动扫 `cancel_streams_for_conn`(P1-2) | 手机 SSE 不挂死,及时关 body;精确到连接级,踢旧/重连窗口期不误杀新流 | 按 node_id:误杀新连接在途流;靠 60s 超时:体验差,占槽 |
| 断开感知延迟 | 依赖 PC KeepAlive(30s ping)触发 Chunk | loopback Sse 已带 KeepAlive,零新代码 | remote 自加 idle 探测:重复造轮子,MVP 不做 |

**取消语义(D1)的代价**:用户在手机上"关掉页面"不会停 agent,agent 继续在 PC 上跑(消耗 token / 执行 tool)。用户必须显式点"停止"(`POST /api/v1/cancel`,经隧道透传)。这是**有意的产品取舍**:网络抖动不该误杀长任务,显式停止 = 明确意图。文档(REMOTE-DEPLOY / 前端提示,S4 落)需说明。

---

## 6. 运营 / 回滚

### 6.1 日志

`tracing` target `everlasting::daemon::tunnel`(PC)+ `everlasting_remote::routes::ws/proxy`(remote)。S3 新增事件:
- `INFO tunnel_stream_start id=.. path=/api/v1/stream`(S2 已有)
- `DEBUG stream_cancelled id=.. reason=client_gone`(PC 收 remote End,S3 新)
- `WARN node_offline_streams_cancelled node_id=.. count=N`(node 离线清理,S3 新)
- `DEBUG sse_client_gone id=..`(remote 感知手机断,S3 新)

### 6.2 失败模式

| 失败 | 行为 |
|---|---|
| 手机断 SSE | remote 感知(下次 Chunk send 失败 / PC KeepAlive 触发)→ 发 End → PC 停转发;agent 不停 |
| PC 断 WSS(在 SSE 流期间) | remote ws 离线清理 → 手机 stream 收 Error 关闭;手机 EventSource 重连 → 等隧道重连 |
| node 离线(心跳超时) | §2.3 清理 + nodes API 反映 offline |
| loopback `/api/v1/stream` 500 | PC `sse_bridge` 收 reqwest err → 发 `Stream::Error` → remote 写 body → 手机 EventSource 重连 |
| remote 重启 | WSS 断,PC 退避重连;手机 in-flight stream 全关(EventSource 重连,resync 靠 Last-Event-ID 透传) |

> **EventSource 重连边界(P3-1)**:表中"EventSource 重连"指**连接级关闭**后浏览器自动重连(规范行为)。但重连请求若拿到 **HTTP 502**(node 仍离线)→ EventSource 直接 **fail 且不重试** —— 此时恢复靠 **S4 前端的显式 backoff 重试**,非自动。S3 侧不实现前端重试,仅在文档标明语义。

### 6.3 回滚形状

见 §4.3。单 PR 回滚,S1/S2 完成态保留(非流式全通)。

---

## 7. 与现有代码的对齐

| 现有约定 | S3 遵守 |
|---|---|
| `PendingReply` enum(S1) | 加 Stream 分支实例化,不改变体定义 |
| proxy catch-all `/api/v1/proxy/*path`(S1) | 流式分支复用同 handler,不新增 route |
| `ws::dispatch_frame` 帧路由(S1) | Stream 分支替换占位,Response 分支不动 |
| `sse_bridge` 纯字节透传(S2) | select! 加取消分支,chunk 逻辑不动 |
| `SseRegistry` broadcast + `subscribe(last_event_id)`(daemon) | 不改;tunnel 是它的一个订阅者 |
| `commands/cancel.rs` 显式 cancel | 不改(D1:断 SSE 不调它) |
| `axum::body::Body::from_stream`(axum 0.7) | remote 流式 body 用它(裸字节) |
| reqwest `bytes_stream()`(S2) | 不改,只在外层 select! |

**不改的东西**:`agent/` / `tools/` / `provider::` / `SseRegistry` 实现 / `everlasting-remote-protocol` 帧定义 / 现有 axum route / 非流式 Oneshot 路径。

---

## 附录 A:S3 验收 checklist(对应修订后 PRD)

| PRD 验收项(修订后) | S3 实现位置 |
|---|---|
| 手机(reqwest)经 remote `GET /api/v1/stream` → 实时收 PC SSE chunk 序列 | proxy 流式分支 + ws dispatch_frame Stream 路由 + sse_bridge |
| 手机断 SSE → 隧道停转发 + PC 本地订阅断;**agent 不停** | ws dispatch_frame send 失败 → 发 End + PC client cancel_stream + sse_bridge select! |
| PC 断线 → 手机 in-flight stream 关闭;PC 重连后恢复 | node 离线清理 stream pending + 隧道重连(S2) |
| shared_secret 错 → 拒绝 | S1/S2 已覆盖,harness 复用 |
| 心跳 90s 无 pong → 离线 + stream pending 清理 + nodes API 反映 | ws 离线路径 + `cancel_streams_for_conn`(P1-2) |
| 端到端 harness 5 场景全绿 | `tests/tunnel_e2e.rs` + remote crate 集测 |
| clippy 0 新增警告 / 全量 cargo test 零回归 / fmt clean | 全量验证步骤 |

## 附录 B:前置 spike(压缩,review 备注)

reviewer 核证原 4 项 spike 大部分已被现有代码预验证:
- `Body::from_stream` + reqwest `bytes_stream` 往返 —— S2 `tests.rs:47-57` 已跑通(SSE chunk 透传测试)
- `mpsc::Sender::send` 在 rx drop 后返 `SendError` —— tokio 契约,remote `sse_bridge.rs:35` 已依赖同款语义
- reqwest `bytes_stream()` drop resp 断 loopback —— S2 dispatcher/sse_bridge 已基于此工作

**S3 实际只剩 1 项需验证**(并入 implement Step 2 的 commit,省独立 spike 步骤):
- [ ] 跨 crate dev-dep:`everlasting` 加 `everlasting-remote` 作 `[dev-dependencies]` 能编译(不污染 release 依赖图;release lib build 仍 0 警告)
