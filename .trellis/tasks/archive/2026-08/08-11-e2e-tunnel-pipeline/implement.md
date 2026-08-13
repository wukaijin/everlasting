# S3 Implement — e2e 隧道管线实施顺序

> **What/Why**:[prd.md](./prd.md) · **How**:[design.md](./design.md)
> **已确认决策**(2026-08-12):取消=只停转发(D1)、范围=纯后端+harness(D2)、不加新帧(D3)。详见 design §0。
> **执行原则**:每个 step 独立 commit + 独立验证命令。Step 2-4 按 remote → PC 双端分界,各自可单测;Step 5 端到端 harness 串联。**硬约束**(design §1.3):不破 S1 Oneshot / 不破 S2 sse_bridge happy path / agent core 零改 / 断 SSE 不停 agent。

---

## Step 0:落任务文档(本步已完成于评审前)

design.md + implement.md + prd.md 修订(Review 修订记录 + 第 3 条)。**不写代码**,等 design review。

**commit**(评审通过后):`docs(remote): S3 design + implement + prd 修订(取消只停转发/纯后端范围)`

---

## Step 1:前置 spike(已压缩,review P2)

reviewer 核证原 4 项 spike 中 3 项已被 S2 代码预验证:`Body::from_stream`/`bytes_stream` 往返在 `tunnel/tests.rs:47-57` 跑通;mpsc rx drop→send Err 是 tokio 契约;reqwest drop resp 断 loopback 是 S2 工作基础。**S3 只剩 1 项需验证**(跨 crate dev-dep 编译),并入 Step 2 的 commit,不单列。

---

## Step 2:remote `pending.rs` 升级(P1-2 conn_id)+ dev-dep 验证

**目的**:为流式分支 + node 离线清理铺底,不动现有 Oneshot 行为。

**动作**:
1. **dev-dep 验证**(原 Step 1 剩余):`app/src-tauri/Cargo.toml` 加 `[dev-dependencies] everlasting-remote = { path = "../../crates/everlasting-remote" }`;`cargo check` + release lib build 0 警告(确认不污染 release 依赖图)
2. `PendingTable` 条目加 `conn_id: u64` 字段(P1-2,**非 node_id** —— 按 conn_id 精确清理,避免踢旧/重连窗口期误杀新连接的在途流,与 `remove_if_current` 同款思路);`insert(id, conn_id, reply)` 签名
3. `get(id) -> Option<Ref<PendingEntry>>`(只读查,Stream 多 chunk 不能 remove-once;**try_send 同步不跨 await,持 DashMap guard 安全** —— P1-1 修订消除了原"持 guard 跨 await 死锁"风险)
4. `cancel_streams_for_conn(conn_id: u64)`(P1-2):扫 Stream 分支,`try_send(StreamEvent::Error{node_offline})`(失败则直接 remove)+ `remove(id)`;Oneshot 不动(靠 60s 超时)
5. 更新所有现有 `insert` 调用点(proxy.rs Oneshot 路径加 conn_id 参数)
6. **更新 `pending.rs` 模块文档**(P3-2):Stream 条目无 60s 超时(手机可长期挂流)、离线按 conn_id 即时清理;Oneshot 60s 兜底不变

**验证**:
```bash
cargo test -p everlasting-remote --lib pending::
# 期望:conn_id 存取 + get 不 remove + cancel_streams_for_conn 只动 Stream 分支、不碰 Oneshot
```

**commit**:`feat(remote): pending 加 conn_id + get + cancel_streams_for_conn + dev-dep(S3 流式铺底)`

---

## Step 3:remote proxy 流式分支 + ws dispatch_frame Stream 路由(P1-1 try_send)+ 离线清理

**目的**:打通手机 SSE → remote → PC 的完整流式链路(design §2.1)。

**动作**:
1. `routes/proxy.rs::proxy_handler`:加 SSE 判定(`Accept` 含 `text/event-stream`)→ 流式分支:
   - `mpsc::channel(128)` + `pending.insert(id, conn.conn_id, Stream(tx))`(P1-2 conn_id)
   - 发 `Frame::Request`(同非流式,token 剥离复用 `frame_path`/`forward_headers`)
   - 返 `Response::builder().status(200).header(content-type, text/event-stream).body(Body::from_stream(...))`
   - Body stream:`ReceiverStream::new(rx)` 把 `StreamEvent::Chunk → Ok(Bytes)`、`End → 结束`、`Error → 关闭`
   - **首帧超时**(P2-4):挂 30s deadline,首帧未到 → `remove(id)` + 关 body;`note_first_frame(id)` 在首帧到时清 deadline
2. `routes/ws.rs::dispatch_frame`:`Frame::Stream` 分支替换 L250 占位(**P1-1 try_send,不 await send**):
   - `Chunk{bytes}` → `pending.get(id)` → 持 guard `tx.try_send(Chunk{bytes})`(同步,不跨 await);**`try_send` 失败(`Full` 慢手机 / `Disconnected` 手机断)统一剔除**:`remove(id)` + 发 `Frame::Stream{id,End}` 给 PC(取消信号)+ drop tx。首帧到时 `note_first_frame(id)`(P2-4)
   - `End/Error` → `remove(id)` + 把 event 送进 mpsc(rx 端 ReceiverStream 结束流)
   - **Response 命中 Stream 条目**(P2-3):`ws.rs:241-249` Response 分支当前只匹配 Oneshot;命中 Stream 条目 → 转 `StreamEvent::Error{message: format!("status={status}")}` 送 body 关闭 + 正确日志(非误导的 "unknown id")
3. `routes/ws.rs` 心跳超时(L153) + 接收循环退出(L214)离线路径(均持 `handle` → `conn_id`):加 `pending.cancel_streams_for_conn(handle.conn_id)`(P1-2,**按 conn_id 非 node_id**)

**验证**:
```bash
cargo test -p everlasting-remote --lib proxy:: ws::
# 期望:SSE 流式 round-trip(fake PC WS client 发 Chunk×N+End,reqwest 手机读 + 断言拼接还原)
#       手机断(reqwest drop)→ remote 发 End(fake PC 断言收到)
#       **慢手机剔除**(P1-1):reqwest 故意不读 body → mpsc(128) 满 → try_send Full → 剔除 + 发 End(不阻塞接收循环,fake PC 并发发配对 RPC 仍通)
#       node 离线 → 手机 stream 收 Error 关闭
#       Response 命中 Stream(P2-3):fake PC 对 SSE 请求回非流式 Response → 手机 body 收 Error 关闭
cargo clippy -p everlasting-remote --all-targets  # 0 新增警告
```

**commit**:`feat(remote): SSE 桥接(proxy 流式 + ws try_send Stream 路由 + conn_id 离线清理)`

---

## Step 4:PC 取消接收(TunnelManager stream_cancels + sse_bridge select! + client.rs)

**目的**:PC 收 remote 的取消信号 → 停 sse_bridge 转发(§2.2)。agent 不停。

**动作**:
1. `daemon/tunnel/manager.rs`:加 `stream_cancels: Mutex<HashMap<u64, CancellationToken>>` + `register_stream_cancel(id) -> CancellationToken` + `cancel_stream(id)`
2. `daemon/tunnel/dispatcher.rs` SSE 分支:`let token = manager.register_stream_cancel(id);` → `sse_bridge::forward_stream(id, resp, tx, token).await`(改签名)→ 结束后 `stream_cancels.remove(&id)`
3. `daemon/tunnel/sse_bridge.rs::forward_stream`:外层 `loop { select! { chunk=bytes_stream.next() => .., _=token.cancelled() => break } }`;取消时 `drop(resp)` 断 loopback SSE
4. `daemon/tunnel/client.rs` L285 `Frame::Stream` 接收分支:替换 log → `manager.cancel_stream(id)`(不存在则 debug log,不报错)

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib "daemon::tunnel::"
# 期望:新增 fake remote 发 Stream{End} → PC sse_bridge 退出 → loopback SSE 订阅断
#       断言 agent 未被 cancel(显式 cancel 入口未调)
cargo clippy --all-targets  # 0 新增警告(daemon crate)
```

**commit**:`feat(tunnel): PC 取消接收(sse_bridge select! + client Stream→cancel_stream)`

---

## Step 5:端到端 harness(lib 内 `#[cfg(test)]` + 5 场景,P2-2)

**目的**:design 附录 A 验收主力 —— 真 remote + 真 TunnelManager + fake loopback + reqwest 手机。

**动作**:
1. dev-dep 已在 Step 2 加(`everlasting-remote` 作 `[dev-dependencies]`)。**harness 放 lib 内**(P2-2 2a,作者方案差异):新建 `app/src-tauri/src/daemon/tunnel/e2e_tests.rs` + `tunnel/mod.rs` 加 `#[cfg(test)] mod e2e_tests;` —— **不**放 `tests/` integration test(后者只能用 `everlasting_lib` 公共 API,看不到 `tunnel/tests.rs` 的私有 helper `spawn_fake_loopback`/`test_cfg`/`wait_for_connected`)。lib 内 `#[cfg(test)]` 可复用这些 helper,零复制零 pub 污染;remote crate(dev-dep)在 lib test 编译时同样可用。
2. fake loopback 的 `/api/v1/stream` **用真实 `SseRegistry`**(daemon 公共 API)并挂**两个订阅者**(隧道 + 另一个"本地浏览器"模拟)。取消场景里断言:隧道订阅断后,**第二个订阅者仍持续收到事件** —— 直接证明 broadcast 未被破坏(= agent 不会因手机断而停),顺带回归"多订阅者互不影响"(比"查 agent 是否 cancel"更可测,因 harness 无真 agent)。
3. 5 场景(全部端口 0,并行安全):
   - **非流式**:reqwest GET `/api/v1/proxy/api/v1/health` → 200 + body(回归 S1 链路)
   - **SSE 流式**:reqwest GET `/api/v1/proxy/api/v1/stream`(带 token,`bytes_stream()` 读)→ 断言收到 PC 的 chunk 序列拼接还原
   - **取消停转发**(P2-2 2b,需**持续流**):fake loopback 发**带间隔的无限流**(非 S2 的 2-chunk-结束);reqwest 读到中途 drop future → 断言 remote 发 End → 断言 PC loopback SSE 隧道订阅断 + **第二个订阅者仍持续收事件**(agent 不停)
   - **慢手机剔除**(P1-1):reqwest 故意不读 body → remote try_send 满剔除 → 发 End;断言 fake PC 并发发配对 RPC 仍即时响应(接收循环未被阻塞)
   - **PC 断线**:kill WSS(drop tunnel task)→ reqwest in-flight stream 收到关闭/Error;PC 重连后新请求恢复
   - **secret 拒绝**:复用 S1/S2,断言握手 401

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib "daemon::tunnel::e2e_tests::"
# 期望:5 场景全绿(含慢手机剔除 + 双订阅者证 agent 不停)
```

**commit**:`test(remote): S3 端到端 harness(lib 内,5 场景含慢手机剔除 + 双订阅者证 agent 不停)`

---

## Step 6:全量验证 + 回归

**目的**:确认零回归 + 0 警告 + fmt,收尾。

**动作**:
```bash
# 后端全量(AGENTS.md 约定)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 期望:既有 1682 + S3 新增 全绿,零回归
cd app/src-tauri && cargo clippy --all-targets  # 0 新增警告(只剩既有 db/trace.rs:741)
cd app/src-tauri && cargo fmt --check
# remote crate
cargo test -p everlasting-remote  # 全绿
cargo clippy -p everlasting-remote -p everlasting-remote-protocol --all-targets  # 0 警告
cargo fmt --check
# Node 冒烟(真实 binary happy path,非流式已有,SSE 可选追加)
bash scripts/remote.sh start
node scripts/remote-e2e-smoke.mjs
```

**commit**(若有零散修):`chore(remote): S3 收尾(fmt/clippy/冒烟)`

---

## S3 完成标准(对应 PRD 验收,修订后)

- [ ] Step 1-6 全部 commit
- [ ] design §1.2 表里 6 项 🔴 S3 新写全部落地
- [ ] 端到端 harness 5 场景全绿
- [ ] 全量 `cargo test --lib` 零回归(既有 1682 + 新增)
- [ ] clippy 0 新增警告(remote + daemon)
- [ ] **取消语义验证**:断 SSE 后 agent 仍在跑(显式 cancel 才停)—— 写进 harness 断言

## 实施顺序总结

```
Step 0 (文档,已完成) → Step 1 (spike) → Step 2 (pending 升级)
  → Step 3 (remote SSE 桥接,核心) → Step 4 (PC 取消接收)
  → Step 5 (端到端 harness) → Step 6 (全量验证)
```

Step 2 是铺底,Step 3 是核心难点(remote SSE 桥接 + 取消链路起点),Step 4 是取消链路终点,Step 5 串联验证。Step 3-4 可分人并行(remote / PC 各一端),Step 5 必须等 3+4 完成。
