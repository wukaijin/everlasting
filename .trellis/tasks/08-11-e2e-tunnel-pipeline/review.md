# Review — S3 e2e 隧道管线(SSE 桥接 + 取消传播 + 双端联通)

> 评审日期:2026-08-12。评审对象:`prd.md`(修订后)+ `design.md` + `implement.md`(status=planning,实施前评审)。
> 方法:对 design 引用的代码库事实逐条核验(S1 的 `pending.rs`/`proxy.rs`/`ws.rs`/`tunnel_registry.rs`/协议 crate;S2 的 `sse_bridge`/`dispatcher`/`client`/`manager`/`tests.rs`;daemon 的 `sse.rs`/`stream.rs`/`cancel.rs`;workspace 与 dev-dep 可行性);对取消传播链路(手机断 → remote → PC → loopback)逐环推演。
> 关联评审:[S1 review](../08-11-remote-daemon-core/review.md) / [S2 review](../08-11-tunnel-client/review.md)(D1 取消语义跨任务项,本 review 复述便于独立阅读)。

## 总体评价

design 质量高——**代码库事实全部核验属实**(协议 crate 帧定义、`PendingReply::Stream` 未实例化、`ws.rs:250` 占位、`sse_bridge` 出站已通、`client.rs:285` 只 log、`SseRegistry` broadcast 单订阅者断开不停 agent、`stream.rs:64` KeepAlive 30s 全部确认),S3 真实剩余工作与 design §1.2 分界表一致,**D1/D2/D3 三个裁决正确且与 S1/S2 现状吻合**:断 SSE 只停转发(Q-T3)、纯后端 + Rust harness、不加新帧 —— 尤其 D1 避免了"抖动误杀长任务",与 `SseRegistry` broadcast 语义天然一致。

**结论:可批准进入实施,但需先修 2 个 P1(都在实施前改 design 即可,不动范围)**:
- **P1-1:stream 转发在 WSS 接收循环内 await mpsc send → 慢手机阻塞整个 node 的接收循环**(Pong 续期饿死 → 心跳误判离线;同 node 其他请求/配对 RPC 全部 head-of-line 阻塞)。设计把"阻塞"当背压特性,但阻塞点选错了位置。
- **P1-2:`cancel_streams_for_node` 按 node_id 清理会误杀同 node 新连接的在途流**(踢旧/重连窗口期)。应改按 conn_id 清理,与 `remove_if_current` 同款思路,代价为零。

另有 1 个**事实性偏差(P2-1)**:design §2.1/§5 声称"端到端背压,合理",但 `SseRegistry::broadcast` 是 `try_send` + 慢订阅者**剔除**(`sse.rs:139-164`),agent 永不阻塞 —— 真实行为是慢手机被断开重连,不是被背压。机制写错但不影响链路正确性,修订措辞即可。

## ✅ 核验通过(证据确凿)

| 声明(design) | 核验结果 |
|---|---|
| 协议 crate 已建,`Frame`/`StreamEvent{Chunk,End,Error}` 单源 | **属实**(`everlasting-remote-protocol/src/lib.rs:39/76`);headers 是 `Vec<(String,String)>`(保序)、body 是 `Vec<u8>` —— 与 PRD 历史伪代码(HashMap/Bytes)不一致但 PRD 已标注历史背景,无碍 |
| `PendingReply::Stream(mpsc::Sender<StreamEvent>)` 已定义未实例化 | **属实**(`pending.rs:26-32`,`pending.rs:9` 注释 "S3 用") |
| proxy 非流式 Oneshot 链路已通 + 60s 超时 | **属实**(`proxy.rs:65-126`,`PENDING_TIMEOUT` 60s);`frame_path` 剥 access_token / `forward_headers` 剥 Authorization 已落地且有测试 |
| `ws.rs:250` Stream 分支只 log(占位) | **属实**(`ws.rs:250-253`);`dispatch_frame` 在接收循环内 `.await`(L194)—— P1-1 的改造点 |
| 心跳 30s ping / 90s 离线 + 两处离线路径(L153/L214) | **属实**(`ws.rs:130-163` 心跳超时路径、`ws.rs:213-218` 接收循环退出路径,均持 `handle` → conn_id 可得) |
| `ConnHandle::send_frame` 可用(proxy 发 Request / 发 End 帧) | **属实**(`tunnel_registry.rs:49-52`) |
| S2 `sse_bridge::forward_stream` 出站已通(dispatcher.rs:96 await 调用) | **属实**(`sse_bridge.rs:23-56`,`dispatcher.rs:94-96`);纯字节透传、send 失败即停 |
| S2 `client.rs:285` Stream 接收只 log | **属实**(`client.rs:285-289`);`serve_loop` 对 Request 是 spawn `dispatch_one`(L273)→ 取消分支的落点天然在独立 task 里,✅ |
| `SseRegistry` broadcast 与 agent `CancellationToken` 解耦 | **属实**(`sse.rs:139-164` fan-out `try_send`;`commands/cancel.rs:26-44` 显式 cancel);单订阅者断开不停 agent 的 D1 前提成立 |
| loopback `/api/v1/stream` 带 KeepAlive 30s `:ping` | **属实**(`daemon/routes/stream.rs:64-68`)—— §2.2 断点兜底(30s 内感知手机断)前提成立,零新代码 |
| `POST /api/v1/cancel/cancel_chat` 存在 | **属实**(`daemon/routes/cancel.rs:33`) |
| harness 可行性:remote 侧 `build_router`/`RemoteConfig`/`RemoteState`/`init_pool`/`run_migrations` 全 pub,`RemoteState` 字段 pub | **属实**(`server.rs:44`、`config.rs:59/73`、`db/pool.rs:20`、`db/schema.rs:10`);集成测试可直接在进程内构造真 remote + tempdir db |
| workspace 结构支持跨 crate dev-dep | **属实**(根 `Cargo.toml` members 含 `app/src-tauri` + 两个 remote crate);`everlasting` lib 名是 `everlasting_lib`,`daemon::tunnel` 全 pub(integration test 可达) |
| 附录 B spike 1/2/4 大部分已被现有代码预验证 | **属实**:`Body::from_stream` + reqwest `bytes_stream` 往返已在 S2 `tests.rs:47-57` 跑通;mpsc rx drop 后 send 返 Err 是 tokio 契约(remote `sse_bridge.rs:35` 已依赖同款语义)—— spike 可砍到只剩"dev-dep 编译"一项 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — stream 转发在接收循环内 `await mpsc send`:慢手机 head-of-line 阻塞整个 node

**位置**:design §2.1(伪代码 `mpsc_tx.send(Chunk).await`)、§3.3 对应实现(`ws.rs::dispatch_frame` Stream 分支)。`dispatch_frame` 被接收循环 **直接 await**(`ws.rs:194`),非 spawn。

**问题**:手机慢(如弱网)→ mpsc(128) 满 → `send().await` 阻塞 → **该 node 的 WSS 接收循环整体停摆**:
1. **Pong 续期被饿死**(Pong 分支在 `ws.rs:182` 同一循环)→ 心跳 90s 判离线(`ws.rs:146`)→ 在线节点被误标 offline → 该 node 所有流被清理(§2.3)→ 手机重连 —— **慢手机能把自己所在的 node 打成 flapping**。
2. **同 node 其他请求 head-of-line 阻塞**:另一台手机的请求、PC→remote 的配对 RPC(`/internal/pairing/generate` 也走这同一个循环)全排在慢流后面。
3. daemon 侧恰恰**没有**这个设计:`SseRegistry::broadcast` 明确选 `try_send` + 慢订阅者剔除(`sse.rs:161-163`)来保证 agent loop 永不阻塞 —— remote 应沿用同一语义,而不是在接收循环里阻塞。

**建议**(二选一,推荐 a):
- **(a) `try_send` + 满则剔除**:`dispatch_frame` Stream 分支用 `try_send`;满/失败(rx gone)统一走"手机断"路径(remove + 发 `Stream{End}` 给 PC)。语义与 daemon 完全对齐:慢手机被断 → EventSource 重连补(Last-Event-ID 回放)。零阻塞,接收循环永远不被流拖住。**推荐**。
- (b) spawn 独立转发 task 持 tx 发流(接收循环只做路由);阻塞被隔离在 per-stream task,但慢手机仍会无限期占住 pending 条目 + 手机 body,需要额外的流级超时。

### 🔴 P1-2 — `cancel_streams_for_node` 按 node_id 清理:踢旧/重连窗口期误杀新连接的在途流

**位置**:design §2.3 + §3.4(`PendingTable` 存 `(node_id, PendingReply)`,清理按 node_id 扫)。

**问题**:S1 注册语义是"重复 node_id → 新连接踢旧"(`ws.rs:105-108`)。旧连接退出清理(`ws.rs:213-218`)与新连接服务请求之间存在**窗口**:PC 重连(同 node_id)后,旧连接的心跳超时/接收循环退出才姗姗执行 `cancel_streams_for_node(node_id)` —— 此时若新连接已接了新手机请求(多设备场景:手机 A 流在旧连接、手机 B 的流经新连接,或手机 A 重连请求已打到新连接),**全被误杀**。`node_id` 是"节点级"粒度,清理需要的是"连接级"粒度(与 `remove_if_current` 防误删新连接的思路一致)。

**建议**:pending 条目记 **conn_id** 而非 node_id(proxy_handler 里 `conn` 就是 `Arc<ConnHandle>`,`conn.conn_id` 随手可得);两条离线路径(`ws.rs:153`/`ws.rs:214`)都持有 `handle` → `cancel_streams_for_conn(handle.conn_id)`。实现成本与 node_id 方案相同,语义精确。§3.4 伪代码同步改。

### 🟡 P2-1 — "端到端背压"表述与 `SseRegistry` 实现矛盾(慢手机是"被剔除",不是"被背压")

**位置**:design §2.1 关键点第 3 条("手机慢 → … → agent SSE 发送阻塞。端到端背压,合理")+ §5 "mpsc 容量"行("对齐 SseRegistry,端到端背压")。

**问题**:`SseRegistry::broadcast` 是同步 `try_send`(`sse.rs:139-164`),**agent 发送永不阻塞**;隧道订阅者 live channel(128) 满 → 被 `retain` 剔除 → loopback SSE 流结束 → `sse_bridge` 发 `End` → 手机 SSE 关。真实机制是"慢手机被断开、重连靠 Last-Event-ID 回放",不是端到端背压。remote 侧 mpsc(128) 的真实作用只是**限内存**,不是背压。

**建议**:§2.1/§5 改写为"与 daemon 语义对齐:慢订阅者被剔除重连(Last-Event-ID 回放补),mpsc(128) 仅限内存"。机制描述修正,链路结论不变(修 P1-1 的 (a) 后两者完全一致)。另外注意:PC → remote 的 WSS 出站是 `UnboundedSender`(`client.rs:169`),该段也无背压 —— 单流 MVP 可接受,但措辞上别再说"端到端背压"。

### 🟡 P2-2 — implement Step 5 "复用 S2 脚手架"不可行;场景 3 取消链路需"持续流"才能触发

**位置**:implement.md Step 5 动作 2("复用 S2 `tunnel/tests.rs` 脚手架")+ 场景 3("reqwest 读到一半 drop")。

**问题**(两条独立问题):
1. **`#[cfg(test)] mod tests` 不可见**:`daemon/tunnel/tests.rs` 是 lib 内私有测试模块,`tests/tunnel_e2e.rs`(integration test,只用 `everlasting_lib` 公共 API)看不到 `spawn_fake_loopback`/`test_cfg`/`wait_for_connected`。只能复制(约 60 行)或抽 `pub` 测试辅助模块。
2. **S2 的 fake loopback SSE 只发 2 个 chunk 就结束**(`tests.rs:47-57`)—— 手机读完自然收到 End,**取消链路根本不会被触发**(取消只在"流仍持续时手机断开"发生)。场景 3 需要 loopback 发**无限/长流**(带间隔持续发 chunk),手机中途 drop,才能断言 remote 发 `Stream{End}` → PC `cancel_stream` → loopback SSE 订阅断。

**建议**:Step 5 改"复制并改造脚手架(fake loopback 增加持续流版本 + 可选 `SseRegistry` 真身)";场景 3 用持续流 + 手机 drop。另:**"agent 未 cancel"断言**在 harness 里没有真 agent 可查 —— 建议让 fake loopback 的 `/api/v1/stream` 用**真实的 `SseRegistry`**(daemon 公共 API),并挂两个订阅者(隧道 + 另一个"本地浏览器"),断言取消后第二个订阅者**仍持续收到事件** —— 这直接证明 broadcast 未被破坏(= agent 不会停),且顺带回归了"多订阅者互不影响"。

### 🟡 P2-3 — 流式分支对 PC 回 `Frame::Response` 未定义:静默失败 + 误导日志

**位置**:design §3.1 流式分支(只处理 Stream 帧)+ 现有 `ws.rs:241-249` Response 分支。

**问题**:手机 SSE 请求发出后,若 PC loopback 回的是**非流式响应**(404 未知路径 / 401 等,content-type 非 event-stream),PC dispatcher 走非流式路径发 `Frame::Response` → remote `ws.rs:244` `remove(id)` 后只匹配 `Oneshot`,Stream 条目被移除时打 `warn "Response for unknown/unmatched request id"`(误导性日志),**手机拿到 200 SSE + 立即关闭,错误体丢失**,表现为静默失败。design 未定义此路径。

**建议**:Response 帧命中 Stream pending → 转 `StreamEvent::Error`(带 status/message)送手机 body 关闭,或至少正确日志(`stream got non-stream response id=.. status=..`)。MVP 低概率(`/api/v1/stream` 恒 SSE),但 harness 若加"404 路径"场景就会踩;一行分支的事,建议顺手做。

### 🟡 P2-4 — 流式分支无首帧超时:pending 条目可永久泄漏

**位置**:design §3.1 流式分支(无任何超时)+ §2.1。

**问题**:非流式有 60s `PENDING_TIMEOUT` 兜底(`proxy.rs:45`);流式分支 `Request` 帧发出后若 PC 永不回(loopback hang / reqwest 无超时)且手机也不断,则:手机 body 永久悬挂 + pending 条目永久存活(§2.3 的清理只在"node 离线"触发)。每次此类失败泄漏一条内存。

**建议**:流式分支加**首帧超时**(如 30s 内无任何 Stream 帧 → remove + 关 body);首帧之后由 KeepAlive 兜底(已覆盖)。与 P1-1 的 (b) 方案互为替代,若选 P1-1 (a) 则此为独立补充。

### 🟢 P3-1 — "EventSource 自动重连"表述不精确:HTTP 错误码不重连

**位置**:design §2.3("手机 SSE body 收到 Error → body 异常关闭 → EventSource 自动重连")+ §6.2 失败模式表。

**问题**:浏览器 `EventSource` 只在**连接级断开**时自动重连;**HTTP 非 200/204 响应(如 node 离线后的 502)直接 fail 且不重试**。node 离线清理 → body 关闭(连接级,会重连)→ 重连请求打到 proxy → 502 → **永久停**。设计把"重连"当自动能力,但 502 后的恢复需要前端显式重试(S4 的活)。

**建议**:design §2.3 补一句:"离线清理触发的是连接级关闭(EventSource 会重连),但重连后 502 会终止 —— 502 后的恢复靠 S4 前端的显式 backoff 重试";写进 S4 的前置条件。S3 侧无代码改动。

### 🟢 P3-2 — `pending.rs` 模块文档在 S3 后过时

**位置**:`pending.rs:11-16`("每条 pending 只对应一个等待方(proxy handler 的 timeout)","连接断开时由 60s 超时兜底清理")。

**问题**:S3 后 Stream 条目无 60s 超时(手机可长期挂流),node 离线走 §2.3 即时清理 —— 文档与行为不符,后人读代码会被误导。

**建议**:Step 2 改 `PendingTable` 时顺带更新模块文档(Stream:无超时、离线即时清理;Oneshot:60s 兜底不变)。

### 🟢 P3-3 — PRD 帧伪代码与真实 crate 定义不一致(建议加"以 crate 为准")

**位置**:prd.md §帧协议定稿(`HashMap<String,String>` / `Bytes` vs 实际的 `Vec<(String,String)>` / `Vec<u8>`)。

**问题**:PRD 已标注"历史背景,S3 零改动",但伪代码与 `protocol/src/lib.rs` 实际定义(保序 headers、binary body 的取舍)不一致,读者可能按伪代码理解协议。

**建议**:该节补一行"实际定义以 `everlasting-remote-protocol` crate 为准(保序 headers / Vec<u8> body)",消除歧义。零成本。

## 🟦 其他备注(可不动)

- **D1 取消语义的代价已如实写明**(§5):手机关页面不停 agent、需显式 cancel —— 与 `SseRegistry` broadcast 语义天然一致,产品取舍合理;S4 文档前置已列。
- **断点兜底依赖 PC KeepAlive(30s)**:已核验 `stream.rs:64` 属实,零新代码;最坏 30s 感知延迟在 MVP 可接受。harness 若要快测取消,loopback 持续流本身就会触发即时感知,无需等 KeepAlive。
- **PC 断线时 in-flight 清理已存在**(S2 兜底):WSS 断 → `frame_rx` drop → `sse_bridge` send 失败即停 → drop resp → loopback SSE 断(S2 代码天然覆盖),remote 侧靠 S3 的 node 离线清理关手机 body —— 两端职责边界清晰,正确。
- **协议零改动(D3)与渐进部署**:老 PC(未升级 S3)收到 remote End 仍 log 忽略 —— 与 `client.rs:285` 现状一致,升级即生效,无兼容坑。
- **`cancel_stream(id)` 幂等性**:token cancel 幂等 + 未知 id 忽略,与 `client.rs:279-283` 的 Response 未知 id 处理同款,无竞态问题。
- **spike 可压缩**(附录 B):1/2/4 已被现有测试与 tokio 契约预验证(见核验表末行),Step 1 实际只剩"dev-dep 编译"一项需验证 —— 可并入 Step 2 的 commit,省 0.5h。

## 复评建议

1. **修 P1-1**(dispatch_frame Stream 分支改 `try_send` + 满则剔除,与 `SseRegistry` 语义对齐)→ 改 design §2.1/§3.3 伪代码后即可开工
2. **修 P1-2**(pending 记 conn_id,`cancel_streams_for_conn` 替代 `cancel_streams_for_node`)→ 改 design §2.3/§3.4 + implement Step 2 动作 4
3. P2 合并修订:§2.1/§5 背压措辞(P2-1)、Step 5 脚手架与场景 3 描述(P2-2)、Response 命中 Stream 的分支(P2-3)、首帧超时(P2-4)
4. P3 顺手清(pending.rs 文档、PRD 伪代码标注、EventSource 重连措辞)
5. 修订完成后按 implement Step 1→6 顺序实施;P1-1 (a) 落地后,Step 3 的验证里补一条"慢手机(不读 body)被剔除 → remote 发 End → PC 停转发"的用例
