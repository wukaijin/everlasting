# Daemon HTTP Server Contract

> axum `everlasting-daemon` 进程的运维契约:serve loop、graceful
> shutdown(SSE 长连接 + agent loop drain)、shutdown 顺序。对应代码
> `app/src-tauri/src/daemon/` + `app/src-tauri/src/agent/helpers.rs`
> (`cancel_and_drain_all_agent_loops`)。
>
> daemon 化落地于 2026-07-20~23(remote-access epic),架构见
> [docs/ARCHITECTURE §4](../../../docs/ARCHITECTURE.md),编排放
> [docs/REMOTE-ACCESS-ROADMAP.md](../../../docs/REMOTE-ACCESS-ROADMAP.md)。

---

## Invariant: daemon 生命周期绑定 GUI 进程(orphan-guard, 2026-07-27)

**问题**:`tauri dev` 的 Rust live reload 是**强制 kill GUI 进程**(不走
`RunEvent::Exit`),所以 `SidecarHandle::kill()` 永远跑不到。daemon(因
`serve_daemon` 修复后能稳定存活)会成孤儿继续占 7456 → 下次 sidecar 探测
端口冲突 exit 1 → 前端 "daemon 不可用"。GUI crash / 被强杀同理。

**契约**:daemon 进程的生死**必须**绑死其父进程(GUI sidecar 持有者)。
无论 GUI 怎么死(正常 `RunEvent::Exit` / live reload 强杀 / crash / kill -9),
daemon 都必须自动退出,绝不留孤儿占端口。

**实现**(`bin/everlasting-daemon.rs::main`,Linux):
`prctl(PR_SET_PDEATHSIG, SIGTERM)` —— 内核在父进程终止时自动给 daemon 发
SIGTERM,走 [`shutdown_signal`](#) 的优雅退出路径。两个 race 防护:
1. `getppid() == 1`(父进程已死被 init 收养)→ 立即 exit 1,不 bind 端口。
2. prctl 只在调用一刻设置;daemon 不会 reparent,故无需重设。

**不变量**:
- **禁止**移除 prctl 调用(或改成 no-op)除非有等价的孤儿清理机制
  (如 PID 文件 + 启动时清理)。移除后,任何 GUI 异常退出都会留孤儿。
- standalone 启动(`cargo run --bin everlasting-daemon` / `daemon.sh`)也安全:
  父进程是 shell,shell 退出 daemon 跟着退,符合"前台跑、关终端即停"。
- 非 Linux 平台(macOS/Windows)无 prctl —— 需另外实现(`proc_exit` /
  Job Object),当前 sidecar 模式仅 Linux 有此问题。

**验证**:手动 `kill -9` daemon 父进程后,daemon 应在 ~1s 内收到 SIGTERM 并
走完整 graceful shutdown(日志:`received SIGTERM` → `shutdown complete`
→ `exited cleanly`),端口自动释放。

---

## Scenario: Graceful Shutdown 与 SSE 长连接

### 1. Scope / Trigger

**触发条件**:任何修改 `serve_daemon` 的 shutdown 路径,或新增
streaming endpoint(SSE / WebSocket / chunked transfer)时。

**问题根源**:axum 的 `with_graceful_shutdown` 在收到信号后,会等
**所有 in-flight 连接自然完成**才退出进程。但 SSE 长连接
(`GET /api/v1/stream`)是**永不自然完成**的 —— 它的 body 是一个
`ReceiverStream`,只要 daemon 不主动 drop sender,就永远 pending。

结果:有活跃 SSE 连接时,`with_graceful_shutdown` 无限挂起,靠
`scripts/daemon.sh` 的 `SIGTERM → 8s → SIGKILL` 兜底清理(2026-07-23
手动测试暴露,2026-07-24 修复,task `07-24-daemon-graceful-shutdown`)。

### 2. Signatures

```rust
// daemon/sse.rs
impl SseRegistry {
    /// 主动结束所有 live SSE 流。清空 senders → 每个 ReceiverStream
    /// 返回 None → stream body 自然 end() → axum 感知连接完成。
    pub fn shutdown(&self);
}

// daemon/server.rs
const SHUTDOWN_GRACE_SECS: u64 = 3;   // 设计参考锚点,不再用于 serve_daemon

/// signal 收到后,先调 sse.shutdown() 再 drain 活跃 agent loop,最后返回。
async fn shutdown_signal(state: Arc<AppState>);

/// serve + with_graceful_shutdown。**不能**给整个 serve future 套
/// `tokio::time::timeout` —— 会变成「无信号也 3s 自杀」(2026-07-27 修过)。
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()>;
```

### 3. Contracts

shutdown 顺序(收到 SIGINT/SIGTERM 后):

1. `shutdown_signal` 的 `tokio::select!` 命中(ctrl_c / SIGTERM)
2. **`registry.shutdown()`** —— drop 所有 SSE sender,结束所有 live stream
3. `shutdown_signal` 返回 → axum `with_graceful_shutdown` 开始 drain
   此时 SSE 流已结束,只剩可快速 drain 的短请求
4. `serve.await` 自然完成(进程随后退出)。**没有外层 timeout** ——
   drain 的硬上限由 `shutdown_signal` 内部的两步保证(见 §关键不变量);
   万一仍有未知长连接卡住,由 `daemon.sh`(standalone)/ GUI `RunEvent::Exit`
   的 SIGKILL 兜底

### 4. Validation & Error Matrix

| 条件 | 行为 |
|------|------|
| 无 SSE 连接时 SIGTERM | drain 亚秒完成 |
| 有活跃 SSE 连接时 SIGTERM | `sse.shutdown()` 结束 stream 后 drain 亚秒完成 |
| 未来加了未知长连接卡住 | `daemon.sh`(standalone)或 GUI sidecar 的 SIGKILL 兜底;**进程内不再用 timeout 提前 return**(历史教训:给整个 `serve` 套 timeout 会让 daemon 在无信号时也 N 秒自杀,见 §7) |
| **没有 SIGTERM/SIGINT** | `serve` 永久服务请求(进程不退出)—— **必须如此**,任何给 `serve` 套 deadline 的写法都是 bug |
| `shutdown()` 后再 `broadcast` | 静默丢弃(daemon 已退出,无消费者),不 panic |
| `shutdown()` 对空 registry | no-op,不 panic |
| `shutdown()` 重复调用 | idempotent |

### 5. 关键不变量

- **必须用 `with_graceful_shutdown`**,不能改回 `select! { serve, signal }`。
  drop `axum::serve` 的 future 会 **abort** 所有连接 task(粗暴断开),
  丢失正在处理中的请求;`with_graceful_shutdown` 才是 drain 语义。
  (2026-07-24 实现中途踩过这个坑:`select!` 未命中分支 drop serve future
  导致连接被 abort 而非 drain。)
- **`serve_daemon` 里禁止给 `serve` 套 `tokio::time::timeout` 或
  `tokio::select!`**。`serve` 在无信号时永久运行是**正确的**(正常服务);
  任何 deadline 都会让 daemon 在 N 秒后无信号自杀(2026-07-27 的回归就是
  `tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve)`,详见 §7「错(二)」)。
  drain 的硬上限交给 `shutdown_signal` 内部(`sse.shutdown()` +
  `cancel_and_drain_all_agent_loops`),进程级 SIGKILL 兜底。
- **`shutdown()` 必须在 `shutdown_signal` 返回前调**,在 axum 开始 drain
  之前结束 SSE,否则 drain 仍会被卡。

### 6. Tests Required

- `daemon::sse::tests::shutdown_clears_all_subscribers` — subscriber_count 归零
- `daemon::sse::tests::shutdown_ends_live_channel` — live channel 返回 None(**核心**)
- `daemon::sse::tests::shutdown_on_empty_registry_is_noop`
- `daemon::sse::tests::shutdown_is_idempotent`
- `daemon::sse::tests::shutdown_silently_drops_post_shutdown_broadcast`
- `daemon::server::tests::serve_daemon_shutdown_completes_with_active_sse` —
  **真实 TCP + 真实 SIGTERM 集成测试**:有活跃 SSE 连接时,`serve_daemon`
  必须在 `SHUTDOWN_GRACE_SECS * 2 + 2` 内返回(回归守卫,若 shutdown
  机制被破坏会超时失败)

### 7. Wrong vs Correct

#### Wrong — `select!` + drop serve future

```rust
// ❌ drop serve future 会 abort 所有连接,丢失 in-flight 请求,
//    且 SSE 连接被硬切(客户端看到连接中断而非正常结束)。
tokio::select! {
    res = axum::serve(listener, router) => return res,
    _ = shutdown_signal() => { state.sse.shutdown(); }
}
// serve future 已被 drop,无法继续 drain。
```

#### Correct — `with_graceful_shutdown` + signal 内调 shutdown,**不**套外层 timeout

```rust
// ✅ axum 的 drain 机制正常工作;signal 内先 sse.shutdown() 让 SSE
//    流自然结束,drain 才不会被永不完成的连接卡住。
let serve = axum::serve(listener, router)
    .with_graceful_shutdown(shutdown_signal(Arc::clone(&state)));
serve.await?;   // 没有 tokio::time::timeout —— 见下方「错(二)」
```

> **❌ 错(二)——给整个 `serve` 套 deadline(2026-07-27 修复的回归)**:
> 历史版本的 §7「Correct」写的是
> `tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve).await`,意图是给
> drain 阶段加兜底。但 `serve = axum::serve(...).with_graceful_shutdown(sig)`
> 在**没有 shutdown 信号时会永久跑下去**(正常服务请求),而 timeout 套的是
> **整个 serve future** —— 于是无论有没有信号,3s 后必然走 `Err` 臂 →
> `serve_daemon` 返回 `Ok(())` → bin 打印 "exited cleanly" → 进程 exit 0。
>
> 表现:daemon 每次 listen 后 ~3s 自杀(sidecar `TerminatedPayload
> {code:Some(0), signal:None}`,日志里**没有** `received SIGTERM`),
> 前端 15s health probe 永远连不上 → "daemon 不可用"。逃生 `?transport=tauri`
> 可绕过(它跳过 health probe)。
>
> 根因:`with_graceful_shutdown` 把「等信号」和「drain」捆在同一个 future,
> 无法只对 drain 加超时。drain 的硬上限改由 `shutdown_signal` 内部两步
> 保证(`sse.shutdown()` + `cancel_and_drain_all_agent_loops` 的 8s),进程
> 级 SIGKILL(`daemon.sh` / GUI sidecar)兜底。**结论:`serve_daemon` 里
> 禁止给 `serve` 套任何 `tokio::time::timeout` / `tokio::select!`。**

---

## Pattern: 新增 streaming endpoint 的 shutdown 检查

**问题**:未来加任何 streaming endpoint(WebSocket、SSE、chunked),
都会重新撞上"长连接永不自然完成 → graceful shutdown 卡住"。

**检查清单**(新增 streaming endpoint 时):

- [ ] 该 endpoint 的 stream body 在 shutdown 时能否被主动结束?
- [ ] 是否在 `shutdown_signal` 里加了对应的结束调用(类比 `sse.shutdown()`)?
- [ ] 是否加了超时兜底测试(活跃连接时 shutdown 在 grace 内完成)?

若 stream body 持有 `mpsc::Receiver`,则在 `shutdown()` 里 `clear()` senders
即可(同 `SseRegistry` 模式）。若是其他载体(如 `WebSocket`),需对应的
close 路径。

---

## ✅ 已覆盖:agent loop drain(闭合硬终止缺口)

> 本契约的 graceful shutdown **同时覆盖** SSE 长连接 **和** 正在跑的
> agent loop。原「⚠️ 已知未覆盖」缺口已于 2026-07-24 闭合(task
> `07-24-daemon-agent-loop-shutdown`)。

**原缺口**:`serve_daemon` 只 `sse.shutdown()` 关 SSE,不 cancel 正在跑的
agent loop → 进程退出时 tokio runtime 销毁直接丢弃 `chat.rs` 的 spawn task,
落在「tool 执行完 → 还没 `persist_turn` 落库」窗口的那一轮结果会丢(已发
SSE 给前端,DB 没落)。

**闭合方式**:`shutdown_signal` 在 `sse.shutdown()` 之后,**复用**
destructive-command 路径(`delete_session` / `detach_worktree` /
`delete_worktree`)早为同一问题造好的 cancel+drain 基础设施,把「单 session
的 cancel+drain」搬到 shutdown 路径,粒度改成「所有 session」:

1. 遍历 `state.cancellations` 对所有 `CancellationToken` 调 `.cancel()` ——
   agent loop 的 `select!` cancel 臂 `biased;` 优先命中,走用户点 Stop 的
   **同一条已验证路径**。
2. 并发 await `state.inflight_exits` 的所有 `oneshot::Receiver`(总 timeout
   `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS = 8s` 兜底)。复用 `chat.rs` 的
   `done_tx.send(())` 信号(在 `run_chat_loop` **返回后**才触发,即 loop
   完全退出含 in-flight tool 完成 + persist_turn 落库)—— **而非新引
   spawn handle**,语义更准且复用现成设施。
3. 编排落在 `agent::helpers::cancel_and_drain_all_agent_loops`(单 loop 版
   `cancel_inflight_for_session` + `await_inflight_exit` 的批量对应),保证
   drain 语义单源、不漂移。

**shutdown 顺序(收到 SIGINT/SIGTERM 后)**:
```
signal → sse.shutdown() → cancel_and_drain_all_agent_loops(8s)
       → axum drain(SHUTDOWN_GRACE_SECS=3s) → 进程退出
```
最坏 8s + 3s = 11s,故 `scripts/daemon.sh` 的 SIGTERM→SIGKILL 窗口从 8s
拉到 **15s**(留 4s 余量),保证 SIGKILL 永远是「等不过来」的最后手段而非
抢先于 drain。

**关键不变量(本场景特有)**:
- **必须先 `sse.shutdown()` 再 cancel loop**:先断流、再停处理,语义更干净;
  且 SSE 关后前端不再收到新事件,也不会有新 chat request 到达。
- **cancel + drain 必须在同一调用内原子两步**:中间窗口里新进来的 request
  会漏 cancel。`cancel_and_drain_all_agent_loops` 封装成原子两步。
- **锁内只 clone/drain,不 await**:`cancellations` / `inflight_exits` 是
  `tokio::Mutex`,锁内若 await 会阻塞该 map 的所有其他使用者。

**测试**:
- `daemon::server::tests::serve_daemon_shutdown_drains_active_agent_loop` —
  真实 TCP + 真实 SIGTERM + 植入「卡死的活跃 loop」(永不 resolve 的
  oneshot),断言 `serve_daemon` 在 drain timeout + grace 内返回(回归守卫)
  且 token 已被 cancel。
- 「persist_turn 真的落库」不变量由 `tests_agent_loop.rs::
  agent_loop_cancel_in_turn_2_kills_loop` 在 agent loop 层单测覆盖
  (daemon 路径注入 provider 成本过高,见该任务 design.md §6.2 方案 (b))。
- 两个 SIGTERM 测试共享 `SIGNAL_TEST_MUTEX` 串行(同进程信号不可并发)。


---

## Pattern: daemon 运维伴生物(备份 task + 日志文件,RULE-DAEMON-001 闭合 2026-08-24)

**DB 备份 task**:`bin/everlasting-daemon.rs` 在 `load_daemon_state` 成功
后调用 `server::spawn_backup_task(&state, &data_dir)`(detached,不 join,
不阻塞 serve)。启动即快照一次 + 每 24h 一次,`VACUUM INTO` 到
`<data_dir>/backups/` 保 7 份;失败仅 `warn!` 继续服务。契约细节见
[database-guidelines "DB 快照备份"](../database-guidelines.md)。
**不要**把 spawn 挪进 bin 内联 —— lib 的 `db` 模块私有,沿用
"wrapper so the bin never touches private modules" 先例。

**日志文件**(仅 `daemon.sh bg/restart` 的 standalone 模式;前台模式照旧
打终端,Rust stdout tracing 不动、零依赖):

- 路径 `${XDG_STATE_HOME:-$HOME/.local/state}/dev.everlasting.app/daemon.log`
  —— 排障先看这里,**不再是 `/tmp`**(重启即覆盖的老坑已闭合)。
- `bg` 启动 **追加写(`>>`)**,不覆盖历史。
- 启动时大小轮转:>10 MiB 滚动保 3 代(`daemon.log` → `.1` → `.2` →
  `.3`,最旧删);运行中增长上限 ≈ 4×10 MiB。
- `rotate_log()` 的存在性守卫 + `|| true` 降级是**必须的**:首次滚动
  `.1`/`.2` 不存在,裸 `mv` 在 `set -e` 下会中断 `do_start` 导致 daemon
  永远起不来(design 伪代码照抄必踩,实测复现过)。
- `STATE_DIR` 用 `${HOME:-/tmp}` 降级:cron/systemd 裸环境下 `$HOME`
  未设,`set -u` 直接杀死脚本(连 `help` 都打不出)。

---

## Pattern: SSE `stream-resync` 哨兵决策表(compute_replay,2026-08-24)

`daemon/sse.rs::compute_replay(buffer, next_id, last_event_id)` 是重连
客户端拿什么回放的唯一裁决点。**前 WP4 语义(2026-08-24 之前)只有
"ring 淘汰"一个哨兵臂,daemon 重启后 reconnect 拿到空回放不发哨兵** ——
前端自愈回路因此永不触发(实证:check pass F1,当年 Explore 研究报告
的"重启必收哨兵"是错的,代码才是真相)。现行决策表:

| `last_event_id` | buffer | 结果 |
|---|---|---|
| `None` | 任意 | 空回放(首连,不变) |
| `Some(last)` | 空 | **哨兵 `reason=restart`**(重启后首连;误报安全:前端以哨兵为触发做 DB 死亡预言机,尾部 `in_progress` → no-op) |
| `Some(last)`,`last+1 < oldest` | 非空 | 哨兵 `reason=buffer_overrun`(淘汰,原语义) |
| `Some(last)`,`last >= next_id` | 非空 | **哨兵 `reason=restart`**(跨进程 id 别名:新进程 id 从 1 重计,`id > last` 过滤会"看似当前"地漏放,但客户端的陈旧高 id 与新进程即将发出的低 id 会错位 —— 不能证明连续性就发哨兵) |
| `Some(last)` ∈ `[oldest-1, next_id-1]` | 非空 | 正常回放 `id > last`(不变) |

**约束**:

- 哨兵帧 `id: 0`、payload `{"reason":"restart"|"buffer_overrun"}` —— reason
  仅排障信号,前端不消费,不构成契约面。
- 两个新哨兵臂**可以 liberal**:前端 `handleStreamResync` 的死亡预言机
  (force `load_session` 看末尾 assistant 行 status)把误报消化为 no-op,
  后端无需精确判定"是否真死"。
- `large_payload_skips_buffer`(>256 KiB 帧不入 ring)造成的 id 空洞**不**
  发哨兵(先在行为,客户端靠 snapshot 自愈)。
- 改 `compute_replay` 必须同步决策表注释与测试(14 例锁全臂)。

---

## Scenario: 新增一个 IPC 命令(2026-08 + Phase 4 group-chat 沉淀)

### 1. Scope / Trigger

> 当需要新增一个前端 → 后端的命令(`transport.invoke("xxx", { ... })`),
> 必须在 **至少 4 处** 同步注册,缺一处即破该路径下的功能。

触发场景: Tauri Full 模式 + sidecar 模式 + 浏览器模式三条路径并行,
每条路径的「命令注册表」是独立的(各自维护自己的 cmd→domain 映射),
但所有路径都调用同一个 `#[tauri::command]` handler 的镜像实现。

### 2. Signatures

| 路径 | 入口 | 文件 |
|---|---|---|
| **后端实现** | `pub async fn xxx_inner(...)` | `app/src-tauri/src/commands/<domain>.rs` |
| **Tauri command 注册** | `#[tauri::command] pub async fn xxx(...)` | 同上文件 |
| **tauri 命令清单** | `commands::domain::xxx` 加进 `tauri::generate_handler!` | `app/src-tauri/src/lib.rs` (invoke_handler 块) |
| **daemon POST 路由** | `pub async fn xxx_handler(...)` + `.route("/xxx", post(xxx_handler))` | `app/src-tauri/src/daemon/routes/<domain>.rs` |
| **daemon router** | `xxx` 加进 `Router::new().route(...)` 链 | `app/src-tauri/src/daemon/routes/<domain>.rs`(`pub fn router(...)`) |
| **前端 transport 映射** | `xxx: "<domain>"` 加进 `CMD_TO_DOMAIN` | `app/src/transport/http.ts` |
| **前端 façade** | `async function xxx()` in store | `app/src/stores/<store>.ts` |

### 3. Contracts

- **请求体**:snake_case(Rust 序列化)→ 经 `transformArgsTopLevel` 转 camelCase
  → 前端 store 用 camelCase 传。**不要在协议层混用两种命名**。
- **响应体**: 同上,反向。`#[serde(rename_all = "...")]` 决定具体命名。
- **错误体**: `AppCommandError` 统一 → REST 端 `IntoResponse` 序列化
  → 前端 `TransportError.body` 透传(`app/src/transport/types.ts`)。

### 4. Validation & Error Matrix

| 模式 | 缺 `CMD_TO_DOMAIN` 注册 | 缺 `lib.rs` `invoke_handler` 注册 | 缺 daemon route |
|---|---|---|---|
| **Tauri Full** | ✅ 走 IPC 通道,不查表 | ❌ 报 "command not found" | ❌ 路由 404(此模式不查 daemon) |
| **Sidecar(spawn daemon)** | ❌ 报 "unknown cmd ..." | ✅ 走 IPC(由 sidecar spawn 的 daemon) | ❌ 路由 404 |
| **浏览器模式(跨域访问 daemon)** | ❌ 报 "unknown cmd ..." | ✅ 直接调 daemon | ❌ 路由 404 |

**关键点**: Sidecar 模式 + 浏览器模式 **都** 走 httpTransport + `CMD_TO_DOMAIN`。
只有 Tauri Full 模式(Tauri 进程内 IPC)走 `invoke_handler` 注册。
**新加 IPC 命令时,最容易漏的就是 `CMD_TO_DOMAIN` 一行** ——
sidecar 模式下侥幸报错还好,纯浏览器模式下用户首次访问才发现。

### 5. Good/Base/Bad Cases

#### Good — 4 处齐全

```typescript
// app/src/transport/http.ts
"update_session_metadata": "sessions",  // (1) ✓
```

```rust
// app/src-tauri/src/lib.rs
.invoke_handler(tauri::generate_handler![
    commands::sessions::create_session,
    commands::sessions::update_session_metadata,  // (2) ✓
    // ...
])
```

```rust
// app/src-tauri/src/daemon/routes/sessions.rs
.route("/update_session_metadata", post(update_session_metadata))  // (3) ✓
```

#### Bad — 漏 `CMD_TO_DOMAIN`(Phase 4 group-chat 真实案例)

```typescript
// app/src/transport/http.ts:148  ← 漏!
export const CMD_TO_DOMAIN = {
  create_session: "sessions",
  load_session: "sessions",
  // ... 没有 update_session_metadata
};
```

**症状**: sidecar + 浏览器模式报 `TransportError(0, "unknown cmd \"update_session_metadata\" — no domain mapping ...")`。Tauri Full 模式侥幸(走 IPC 不查表)。

**Fix**: 在 `http.ts` 加 `"update_session_metadata": "sessions"` 一行。
trellis-check 报告问题;commit `002ac90` 修复。

### 6. Tests Required

- **后端 handler 单测**: 在 `commands/<domain>.rs` 或 `db/<domain>_tests.rs`
  直接调 `xxx_inner(...)`,验证 SQL/状态变更。
- **Tauri command 注册检测**: `cargo build --lib` 0 error(若漏 `lib.rs`,
  编译失败但错误不直观)。
- **daemon route 调用**: `daemon::server::tests::*` 用 `#[tokio::test]` + axum
  的 `Router` 直接调用,验证路由 + handler 联动。
- **前端 transport 映射**: 该项目缺自动化测试;最容易漏的就是 `CMD_TO_DOMAIN`
  一行。**手动 e2e 验证**:sidecar 模式 / 浏览器模式 各跑一次新命令。
  trellis-check 跨层 review 也覆盖此项。

### 7. Wrong vs Correct

#### Wrong — 只加 handler + lib.rs command,漏 daemon route

```rust
// commands/sessions.rs: 写好 handler (1) ✓
// lib.rs: 加 invoke_handler (2) ✓
// daemon/routes/sessions.rs: 漏 .route("/update_session_metadata", ...) (3) ✗
```

Tauri Full 模式工作,sidecar 直接 404。

#### Correct — 4 处齐全

```rust
// (1) commands/sessions.rs::update_session_metadata_inner
// (2) lib.rs 含 commands::sessions::update_session_metadata
// (3) daemon/routes/sessions.rs::router() 含 .route("/update_session_metadata", post(handler))
// (4) app/src/transport/http.ts::CMD_TO_DOMAIN["update_session_metadata"] = "sessions"
```

任何新 IPC 命令上线前,验证四条全在 — 推荐跑 `grep -rn "<cmd_name>" app/src-tauri/src/{lib.rs,daemon/routes} app/src/transport/`。

### 8. 真实案例

**Phase 4 (07-29-group-chat, 2026-07-31)**: 加 `update_session_metadata` 用来
runtime 编辑群聊参与者配置。代码层 + Tauri command + daemon route 三处齐全,
但 `CMD_TO_DOMAIN` 漏了一行。trellis-check 跨层 review 报告 L3 severity 1,
commit `002ac90` 修复。在此之前,sidecar / 浏览器模式下的群聊配置重编辑
全部失败。
