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

**background shell sweeper**(RULE-SHELL-001 闭合 2026-08-27,任务
`08-27-rule-shell-001-sweeper`):同款装配 —— bin 在 backup task 后调
`server::spawn_shell_sweeper(&state)`,5min interval 调
`background_shells.sweep_completed_shells(SHELL_RETENTION_MS)`,清除
「Done 且完成超 1h」条目(内存大头是 Done 条目的完整 stdout/stderr 缓冲)。
约束:只扫 Done(Running 携带 kill_tx,移除即孤儿化 kill 通道);通知队列
与 spill 文件不在清扫面(通知自包含 outcome/exit_code,是 1h retention
正当性来源);`AppState::load` / `default_registry()`(~30 处测试构造点)
绝不 spawn —— "no timer tasks in the GUI main process" + 测试运行时零污染。
清扫后 `shell_status` 返 NotFound 是 `BackgroundShellRegistry::status`
文档既有语义("or was already cleaned up"),非行为破坏。

**task scheduler**(F2 定时任务,2026-08-28,任务
`08-28-f2-scheduled-tasks`):第 4 个 spawn,bin 在 sweeper 旁调
`server::spawn_task_scheduler(&state)`,30s tick 扫 `scheduled_tasks`
到期任务经 `chat_inner` 注入(契约细节见
[scheduled-tasks](scheduled-tasks.md))。**停机变体**:backup/sweeper
是纯 detached(进程退出即亡),scheduler 因 fire 有副作用、需要即时停
—— `AppState.scheduler_cancel: CancellationToken` 字段(load_inner 纯
分配,不违 RULE-DAEMON-001;`CancellationToken::new()` 无 spawn 语义),
循环体 `tokio::select!` biased 监听 cancel(沿 tunnel 心跳样板
`tunnel/client.rs`),`shutdown_signal` 在 tunnel stop 之后插
`state.scheduler_cancel.cancel()`。新教训:**需要协作停机的周期任务,
token 挂 AppState 字段**(无 OnceLock 必要——纯分配直接构造);不需要
的才用纯 detached。

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

## Pattern: SSE 契约测试唯一 home 在 sse.rs 内联;e2e.rs 只留路由级用例(RULE-TEST-003,2026-08-30)

**事故形态**:`tests/e2e.rs` 的 e1b 模块曾放 4 个纯 `SseRegistry` pub-API 单元
副本;WP4 改 `compute_replay` 语义时内联套件同步更新、e2e 副本未跟上(空 buffer
+ `Some(last)` 从静默空回放改为发哨兵,副本还断言旧行为)→ 永久红。叠加 e2e
从未进 CI,B1 给 `AddModelRequest` 加必填 `supports_images` 令 fixture 422 后,
干净 HEAD 长红近两周无人知。

**规则**:

1. SseRegistry / `compute_replay` 契约测试只写 `sse.rs` 内联套件(15 例,
   含决策表全臂锁定);**禁止在 `tests/e2e.rs` 复制同名/同义单元副本** ——
   镜像必漂,一处更新另一处必烂。
2. `tests/e2e.rs` 的定位是路由级集成:经 `build_router` 走公开 HTTP 面
   (httpmock 假 LLM + 零特权 seeding),e1a chat / e1c snapshot / e1d health /
   e1e 路由清单。
3. e2e 已进 CI(2026-08-30,`cargo test --test e2e` 紧随 `--lib`);wire 必填
   字段演进(如 `AddModelRequest` 加字段)会即时打红 e2e fixture,不再静默烂。
   排查 e2e 422 第一嫌疑 = req struct 反序列化拒收,diff 对应 `routes/*.rs`
   的 Request struct 必填字段。

---

## Pattern: SessionSummary 运行时态 enrich(busy 字段,F6 2026-08-27)

DB 层恒 `busy:false`,**单点 enrich 在 `list_sessions_inner`**(`commands/sessions.rs`)——Tauri IPC 与 daemon REST 双入口共用,读 `session_active_request` map 置真。要点:

1. **运行时态不入库、不加列**:busy 是进程内存态(`session_active_request` 含即忙),DB schema 零改动;重启后自然 false(recover 链路另行标 interrupted)。
2. **claim 即注册 = busy 即亮**:F1-A 路由临界区 claim 后、F3 等闸期间也算在途(「已接受在途」语义),红点/关闭确认据此计数。
3. **enrich 只做单点**:严禁在 transport 各自的 handler 里分头 enrich(F1-A「路由口径统一」教训——双处 enrich 必漂移)。
4. **wire 是 additive 可选字段**:`SessionSummary.busy` 序列化恒出(新 daemon)、前端类型标 `busy?: boolean`(旧 daemon 无此字段不炸)。
5. 测试:daemon route 测试用 `serde_json::Value` + `is_boolean()` 断言(SessionSummary 无 Deserialize,不能走强类型往返);idle→busy→idle 三段断言。

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
- **前端 transport 映射**: **已有结构化守卫**(2026-08-25 落地):
  `app/src/transport/http.routes-sync.test.ts` 解析 daemon Rust 路由源码
  (`routes/mod.rs` 的 nest 映射 + 各域文件的 `.route(..., post(...))`),
  断言每条 POST 路由都在 `CMD_TO_DOMAIN` 中且 domain 一致 —— 新增命令漏
  加映射会在 `pnpm test` 直接红,不再依赖手动 e2e。历史同类遗漏四次:
  `update_session_metadata`(group-chat)、`save_attachment`(B1)、
  `get_web_search_config`(F4)、F1 三条 queue 命令。GET 路由(附件下载、
  session 快照、health、SSE)走 `<img>`/EventSource 直连,不进该表,守卫
  天然排除。仍建议 sidecar/浏览器模式各跑一次新命令做 e2e(守卫不校验
  handler 行为)。

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

---

## Scenario: tunnel node_id 派生与自定义(同 hostname 撞车,2026-08-26)

### 1. Scope / Trigger

改 node_id 派生、`set_tunnel_node_id` IPC 或 remote 节点注册逻辑时读本节。
背景 gotcha:两台机器 hostname 相同(如都叫 `carlos`)→ node_id 相同 →
remote `tunnel_registry` 同 key 互踢(新连接踢旧是设计行为,但两台活机器
+ 退避连接成功即重置回 1s)→ 永久互踢循环 + 手机流量随机路由到其中一台,
配对表现"时好时坏"。症状:remote 日志刷 `duplicate node_id, kicking
previous tunnel`。

### 2. Signatures

- `daemon/tunnel/node_id.rs::derive_node_id(pool: &SqlitePool) -> String`
- IPC `set_tunnel_node_id(node_id: Option<String>)`(四处齐全规则见上节;
  请求体 snake_case `node_id` + `#[serde(default)]`)

### 3. Contracts

- 派生三级优先:① `app_config["tunnel_node_id"]` trim 非空 → 直接用
  (统一覆盖用户自定义与历史 fallback UUID);② hostname 净化
  (`sanitize`:只留 `[a-z0-9-]`,非法字符折叠单连字符);③ 随机
  `node-<uuid>` 持久化写同一 key。
- 三态(照 `set_web_search_config` 的 `tavily_api_key` 先例):
  `Some(非空)` = 校验 + 落库 + 从 DB 重建 `TunnelConfig` 经
  `tunnel_manager.set_config` 触发重连(supervisor 既有 `cfg != current`
  路径);`Some("")` = 删 key 回自动派生;`None` = 不动。
- `get_remote_config` payload 含 `nodeId: Option<String>`(自定义值原文,
  null = 自动);payload 本身"未配置 remote 时为 null"语义不变。

### 4. Validation & Error Matrix

- `sanitize(trim(x)) != trim(x)` 或净化后为空(大写/下划线/连续或首尾
  连字符/纯中文/纯空格)→ `InvalidRequest` 中文消息,**不写库**
- remote_url 未配置 → 仅落 key,`set_config(None)`,无 panic

### 5. Good/Base/Bad Cases

- Good: `carlos-home` → 落库,隧道以新 id 注册,remote upsert 新行
- Base: key 无值 → hostname 派生(存量默认行为)
- Bad: 两台机器不设自定义且 hostname 相同 → 互踢(见 Scope 症状)

### 6. Tests Required

- 派生三臂各自单测:key 优先于 hostname;key 空走 hostname;fallback
  UUID 持久化后二次读回稳定
- HTTP route 全链 roundtrip:非法值逐个断言 4xx **且库中无写入**;
  清除后 cfg 回 hostname 派生(真实案例:`08-26-custom-node-id` 的
  `tunnel_node_id_set_validate_clear_roundtrip`)

### 7. Wrong vs Correct

#### Wrong — hostname 优先于持久化 key

2026-08-26 前的实现:注释宣称"DB 文件即身份,不随 hostname 漂移",
但 hostname 派生成功时根本不读 key → 改名漂移 + 同 hostname 撞车。

#### Correct — key 有值即优先

身份以 `app_config["tunnel_node_id"]` 锚定;写入仅经 `set_tunnel_node_id`
(校验后的自定义值)或 fallback 生成;hostname 只影响"key 无值时"的默认。

### 8. 增补(同日):`set_tunnel_display_name` 镜像契约

- 三态同 `set_tunnel_node_id`;校验差异:**无字符集限制**(显示名给人看,
  允许中文,传输已有 percent-encode)+ `trim` 非空 + `chars().count() <= 64`
  (按字符数,非 UTF-8 字节)。
- 读取链不动:`build_tunnel_config` 里 `tunnel_display_name`(滤空)→
  hostname 原文 → node_id。remote 侧 `nodes.display_name` 经每次连接
  upsert 刷新——手机配对看到的 `node_display_name` 即来自该表。
- 展示语义:status 快照不带 displayName;自定义值经 `get_remote_config`
  的 `displayName` 回显(None = 自动)。

## Pattern: 下线一个弃用 IPC 命令(RULE-SHIM-001 闭合 2026-08-30)

「新增 4 处注册」的镜像清单 —— 删一个命令同样缺一不可,且比新增多两处
测试侧清单。全链(以 `get_pending_question` 下线为参照):

1. 命令本体:`commands/<domain>.rs` 的 `#[tauri::command]` 壳 + `_inner`
   (inner 若因此失去最后一个调用方则同删;daemon 路由若只剩它是消费者,
   路由 handler + `Get*Request` 结构体一并删)。
2. Tauri 注册表:`lib.rs` `generate_handler!` 列表 + 相邻 `#[allow(deprecated)]`
   与说明注释。
3. daemon 路由:`daemon/routes/<domain>.rs` 的 `.route("/<cmd>", ...)`。
4. 命令清单:`commands/mod.rs::all_command_names()`(文档型 inventory,
   漏删不报错,只会留漂移)。
5. 前端映射:`transport/http.ts` 的 cmd→domain 表 + `questionCards.types.ts`
   类 `*_CMD` 常量及其消费方。
6. **测试侧**(新增没有的):`tests/e2e.rs` 路由 inventory 列表 +
   专测该命令的用例(改名/删除/移植到现代等价物)。

前置条件:全仓 grep 命令名确认前端/脚本零消费(含 `"<cmd>"` 字符串字面量);
历史叙事文档(docs/HACKING-*、_history)中的提及是事实记录,不算消费方,不追改。

## Contract: `/api/v1/health` 保持 stateless(RULE-HEALTH-001 闭合 2026-08-30)

- handler **不接 `State<Arc<AppState>>`**:Q1 端口冲突探测在新 daemon 加载
  AppState **之前**就要打到这个端点(问的是占端口的旧 daemon),stateless
  顶层 router 挂载是它的前提。需要带状态的体检,另立
  `/api/v1/health/detailed`,不动本端点。
- wire 形状:`{daemonId, daemonVersion, apiVersions, uptimeSeconds}`(camelCase)。
  `sessionCount` 字段已删除 —— 曾是 `-1` 哨兵,核实零消费方(仅 TS 接口声明
  + 测试 fixture)后移除;JSON 删字段对旧消费者向后兼容(absent ≠ 哨兵语义)。
  单测断言 `sessionCount` **缺席**(防止无意识回填)。
