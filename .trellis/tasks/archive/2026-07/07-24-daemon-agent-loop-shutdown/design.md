# Design: daemon shutdown agent loop drain

> 配套 `prd.md`。技术设计:签名、契约、数据流、不变量、测试矩阵。
> 复杂任务,Phase 1.1 产出。

---

## 1. 关键复用点(为什么改动小)

destructive-command 路径(`delete_session`/`detach_worktree`/`delete_worktree`)
早就需要解决**同一个问题**(等正在跑的 loop 干净退出),所以脚手架已就位:

| 需要 | 已有设施 | 位置 |
|---|---|---|
| 按 request_id cancel | `cancellations: HashMap<rid, CancellationToken>` | `state.rs:105` |
| 按 session 找活跃 rid | `session_active_request: HashMap<sid, rid>` | `state.rs:112` |
| loop 退出信号 | `inflight_exits: HashMap<rid, oneshot::Receiver<()>>` | `state.rs:128` |
| 单 loop cancel+await | `helpers::cancel_inflight_for_session` + `await_inflight_exit` | `helpers.rs:184,244` |
| loop 响应 cancel | `select!` cancel 臂 `biased;` | `chat_loop.rs:1716,2401` |
| loop 退出时发 done | `done_tx.send(())`(所有 exit path) | `chat.rs:478` |

**本任务 = 把「单 session 的 cancel+drain」模式搬到 shutdown 路径,粒度从
单 session 改成「所有 session」**。`run_chat_loop` 一行不用动。

---

## 2. 签名变更

### 2.1 `daemon/server.rs` —— `shutdown_signal` 接 `AppState`

现状(只接 `SseRegistry`):
```rust
async fn shutdown_signal(registry: Arc<crate::daemon::sse::SseRegistry>);
```

改为接 `Arc<AppState>`,以便访问 `cancellations` + `inflight_exits`:
```rust
async fn shutdown_signal(state: Arc<AppState>);
// 内部:
//   state.sse.shutdown();
//   drain_active_agent_loops(&state).await;
```

`serve_daemon` 的 `with_graceful_shutdown(shutdown_signal(...))` 调用点
同步改传 `Arc::clone(&state)`。

### 2.2 `agent/helpers.rs` —— 新增批量 drain

```rust
/// daemon shutdown 用:并发 cancel 所有活跃 agent loop 的 token,
/// 再并发 await 所有 inflight_exits 的 done 信号,带总 timeout 兜底。
///
/// 与单 loop 的 `cancel_inflight_for_session` + `await_inflight_exit`
/// 语义一致(同源的 `done_tx`/`cancellations`/`inflight_exits` 三件套),
/// 只是粒度从「单 session」变成「所有活跃 request」。
///
/// `total_timeout` 取 `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`(8s)。
pub async fn cancel_and_drain_all_agent_loops(
    cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
    inflight_exits: &Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>,
    total_timeout: Duration,
);
```

放在 `helpers.rs`(紧挨 `cancel_inflight_for_session`/`await_inflight_exit`),
因为它操作的就是这两个函数同源的三件套,逻辑就近、漂移可被一眼发现。

> 命名说明:不叫 `await_all_inflight_exits`(PRD 草稿曾用名),因为它**同时**
> 做 cancel 和 drain 两步,名字要反映这点。`cancel_and_drain_all_agent_loops`
> 更准。

### 2.3 `daemon/server.rs` —— 新增常量

```rust
/// agent loop drain 的总 timeout 上限(秒)。cancel 所有活跃 loop 后,
/// 并发 await 它们的 `done` 信号最久等这么久。实测路径下 loop 多在亚秒
/// 退出,8s 是纯兜底 —— 对照 `await_inflight_exit` 单 loop 的 10s,并发
/// drain 理论上比串行快。
///
/// 与 `SHUTDOWN_GRACE_SECS`(3s,SSE/axum drain)正交:先 drain loop(可能
/// 跑 persist_turn),再让 axum drain 短请求。两者串行最坏 11s,需
/// `scripts/daemon.sh` 的 SIGKILL 窗口 ≥ 11s(本任务拉到 15s)。
const DAEMON_SHUTDOWN_LOOP_DRAIN_SECS: u64 = 8;
```

---

## 3. Contracts —— shutdown 顺序

收到 SIGINT/SIGTERM 后(`shutdown_signal` 内):

1. `tokio::select!` 命中(ctrl_c / SIGTERM)
2. **`state.sse.shutdown()`** —— drop 所有 SSE sender,结束所有 live stream
   (前序任务交付,不动)
3. **`cancel_and_drain_all_agent_loops(&state.cancellations, &state.inflight_exits,
   8s)`** —— 本任务新增:
   - a. 锁 `cancellations`,clone 出所有 token(锁内只 clone,不阻塞),释放锁
   - b. 对每个 token `.cancel()`(agent loop 的 cancel 臂 `biased;` 优先命中)
   - c. 锁 `inflight_exits`,take 出所有 `oneshot::Receiver`(single-consumer,
        take 后 map 清空),释放锁
   - d. 并发 await 所有 receiver(`futures::join_all` 或手写 `select!`),
        总 timeout `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`
   - e. timeout 到则 log warn 后 return(不阻塞后续)
4. `shutdown_signal` 返回 → axum `with_graceful_shutdown` 开始 drain
   (SSE 流已在步骤 2 结束,只剩短请求,亚秒完成)
5. `tokio::time::timeout(SHUTDOWN_GRACE_SECS=3, serve)` 兜底
6. `serve_daemon` 返回 → `main` return ExitCode → 进程退出

**与单 loop drain 的语义一致性**:步骤 3 等同于「对所有活跃 session 各跑一遍
`cancel_inflight_for_session` + `await_inflight_exit`」,只是并发 + 总 timeout
聚合,而非每 session 独立 10s。

---

## 4. 关键不变量

1. **必须先 `sse.shutdown()` 再 cancel loop**:顺序不能反。SSE 仍连着时 cancel
   loop,前端会在 loop 退出后还挂着一条死 SSE 连接,直到 sse.shutdown。虽然最终
   都会清,但语义上「先断流、再停处理」更干净,且与 daemon.sh 的「关进程」方向一致。

2. **cancel 和 drain 必须在同一调用内**:不能「先 cancel,后另起一个 await」。
   两步分离的话,中间窗口里新进来的 request(理论上 shutdown 时不会再有新
   request,但防御性写法)会漏 cancel。`cancel_and_drain_all_agent_loops`
   封装成原子两步。

3. **drain 用 `done_tx` 而非 spawn handle**:`done_tx.send(())` 在 `run_chat_loop`
   **返回后**才触发(`chat.rs:478`),即「loop 完全退出,含 in-flight tool
   完成 + persist_turn 落库」。spawn handle 只能告诉我们「spawn task 结束」
   (含被 drop),语义不如 `done_tx` 精确。

4. **锁内只 clone/take,不 await**:`cancellations`/`inflight_exits` 是
   `tokio::Mutex`,锁内若 await 会阻塞该 map 的所有其他使用者(虽然 shutdown
   时基本无竞争,但守纪律)。step 3a/3c 锁内只做 clone/take + Vec 收集,
   await 全在锁外。

5. **idempotent**:重复 cancel 同一 token 是 no-op(tokioutil 语义);重复
   take 同一 receiver 不可能(single-consumer,take 后 key 已移除)。

---

## 5. Validation & Error Matrix

| 条件 | 行为 |
|------|------|
| 无活跃 agent loop 时 SIGTERM | `cancellations` 空,cancel 步 no-op;`inflight_exits` 空,drain 步立即返回。整体 = 现状(只 sse.shutdown + axum drain) |
| 1 个活跃 loop(正常跑) | cancel 命中 select 臂 → loop 走 cancel 路径 persist → `done_tx.send` → drain 在亚秒收到 |
| N 个活跃 loop | 并发 cancel + 并发 drain,总耗时 ≈ max(各 loop 退出时间),非 sum |
| 某 loop 的 in-flight tool 卡住(超 8s) | 总 timeout 8s 到,log warn,return;该 loop 被进程退出斩断(已尽力) |
| 某 loop panic | `done_tx` drop(未 send),receiver 收 `Err` → drain 视为「已退出」(等价语义),不阻塞 |
| cancel 后 loop 又起新 turn | 不会 —— cancel token 一旦 cancel,loop 的 select 臂立即命中,不再进入下一轮 send |
| drain 期间有新 request 进来 | shutdown 时 axum 还在 serve,理论上可能。但新 request 的 token 也会被... 不会,因为 step 3a 已 clone 完毕。**防御**:step 3 在 sse.shutdown 之后,此时前端已收不到 SSE(连接断了),实际不会有新 chat request 到达。可接受。 |

---

## 6. 测试矩阵

### 6.1 单元测试(`agent/helpers.rs` 的 `#[cfg(test)]`)

- `cancel_and_drain_all_agent_loops_cancels_all_tokens` —— 3 个 token,调用后
  全部 `is_cancelled() == true`
- `cancel_and_drain_all_agent_loops_on_empty_is_noop` —— 空 map,不 panic,
  亚秒返回
- `cancel_and_drain_all_agent_loops_drains_done_signals` —— 3 个 receiver,
  对应 sender 先 send,drain 全部收到
- `cancel_and_drain_all_agent_loops_timeout_on_hung_sender` —— sender 不 send,
  总 timeout 到后返回(用小 timeout 如 100ms 测,不真等 8s)

### 6.2 集成测试(`daemon/server.rs` 的 `tests`)

`serve_daemon_shutdown_drains_active_agent_loop` —— **核心回归守卫**:

复用 `tests_agent_loop.rs::agent_loop_cancel_in_turn_2_kills_loop` 的 mock
脚本结构,但走 `serve_daemon` 的真实 TCP + 真实 SIGTERM 路径(参照现有
`serve_daemon_shutdown_completes_with_active_sse` 的脚手架):

1. 起 `serve_daemon`(ephemeral port + TempDir state)
2. POST `/api/v1/agent/chat` 发起一个 chat(mock provider 脚本:turn 1
   `list_dir` tool_use + turn 2 `HangingThenCancel`)
3. 等 turn 1 的 tool 执行完(轮询 mock `call_count >= 2`,即 turn 2 的 send
   已被调用 = turn 1 已完整执行+persist)
4. 发 SIGTERM(`libc::kill(getpid(), SIGTERM)`)
5. 断言:
   - `serve_daemon` 在 `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS * 2 + SSE grace + 余量`
     内返回(防 CI 慢机器)
   - **`SELECT content FROM messages WHERE session_id=? AND role='tool'`**
     非空 —— turn 1 的 tool_result 已落库(核心不变量)

> 测试难点 & 应对:daemon 路径的 chat 走真实 `AppState`(生产 `chat_inner`),
> 用的是 `state.catalog` 里的真实 provider,不是 test harness 的 MockProvider。
> 需 grill/研究:怎么让 daemon 路径用上 MockProvider?选项:
> (a) `AppState` 加 test-only 的 provider override 入口
> (b) 测试不 POST /chat,而是直接往 `state.cancellations` 塞一个挂起的 token +
>     对应 inflight_exits receiver(模拟「有活跃 loop」),只测 drain 机制本身,
>     不测「persist_turn 真的落库」
> (c) 起一个本地 mock HTTP server 当 provider endpoint,catalog 指向它
>
> 倾向 (b) 作为 daemon 集成测试(测 drain 机制 + shutdown 不挂),「persist_turn
> 落库」不变量留给 `agent_loop_cancel_in_turn_2_kills_loop` 已覆盖的单测层
> (它已能断言 tool_result 落库)。这样 daemon 测试不引入 provider 注入复杂度。
> 实施时若发现 (b) 说服力不足,再升级到 (a)。

### 6.3 回归测试

- 现有 `serve_daemon_shutdown_completes_with_active_sse` 必须仍通过
  (确认 SSE shutdown 未被破坏)
- 现有 `tests_cancellation.rs` 全部仍通过(确认单 loop cancel 语义未动)

---

## 7. 兼容性 / Rollout

- 纯增量,无 API/DB schema 变更。
- `shutdown_signal` 是 private 函数,签名变更是内部的,无外部影响。
- `daemon.sh` 的 15s 是 operator 可感知的变化(关停更慢一点),但语义正确
  (等落库 vs 硬斩)。更新脚本顶部注释说明。
- Rollback:revert 一个 commit 即可(改动集中在 server.rs + helpers.rs +
  daemon.sh + 测试)。

---

## 8. 决策记录(此任务锁定)

| 决策 | 选择 | 理由 |
|---|---|---|
| drain 策略 | 显式并发 await(完整方案) | 彻底闭合数据一致性缺口;cancel+drain 模式已被 destructive 路径验证,风险低 |
| drain 总 timeout | 8s | 对照单 loop 10s,并发更快;留余量给 daemon.sh |
| daemon.sh 窗口 | 8s → 15s | 11s 最坏路径(8 drain + 3 SSE)需 ≥11s SIGKILL 窗口,15s 留 4s 余量 |
| drain 信号源 | `done_tx`/`inflight_exits` | 比 spawn handle 更准(loop 完全退出才 send);复用现成设施,不新引 |
| 批量函数位置 | `agent/helpers.rs` | 紧挨单 loop 版本,漂移可一眼发现 |
| agent loop 本体 | 零改动 | 已为 cancel 设计好(`biased;` cancel 臂) |
