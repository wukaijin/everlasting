//! Phase 2.2 axum router assembly + daemon serve loop (B1 + B2).
//!
//! `serve_daemon` is the single entry point used by the
//! `everlasting-daemon` bin target. It:
//! 1. Pre-flight health check (Q1 fail-loud decision): GET the
//!    `0.0.0.0:PORT/api/v1/health` endpoint; if a daemon is already
//!    running with the same `daemon_version`, we exit cleanly (the
//!    caller can reuse the existing daemon). If the port is taken
//!    by something else, we fail loud with a Chinese user-facing
//!    message (no silent port hop — Q1 explicitly rejects that).
//! 2. Build the axum router via [`build_router`], wiring the shared
//!    `Arc<AppState>` as `State`.
//! 3. Bind a `TcpListener` on `0.0.0.0:PORT` (WSL-first — Windows
//!    host browsers reach the daemon via WSL 2 localhost forwarding;
//!    see `docs/HACKING-wsl.md`).
//! 4. `axum::serve(...).with_graceful_shutdown(...)` — Ctrl+C /
//!    SIGTERM first calls [`sse::SseRegistry::shutdown`] to end all
//!    live SSE streams (so the drain isn't blocked by never-finishing
//!    `GET /api/v1/stream` connections), then drains remaining
//!    in-flight requests. (历史:`SHUTDOWN_GRACE_SECS` timeout 曾兜底未知
//!    长连接,2026-07-27 起该 timeout 已移除 —— 它误套在整个 serve future
//!    外,导致 daemon 无信号时 3s 自杀。见 [`serve_daemon`] 注释。)
//!
//! The `Arc<AppState>` is shared across every handler (axum clones
//! the `Arc` per request). This matches the Tauri `State<'_,
//! Arc<AppState>>` pattern — the underlying `SqlitePool`, catalog,
//! `PermissionStore`, etc. are all `Arc`-internal and safe to share.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

use crate::background_shell::in_memory::{SHELL_RETENTION_MS, SWEEP_INTERVAL_MS};
use crate::daemon::routes;
use crate::db::backup;
use crate::state::AppState;

/// Default daemon port (Q1 decision — `7456` is the project's
/// canonical port; override via `--port` / `EVERLASTING_DAEMON_PORT`).
pub const DEFAULT_DAEMON_PORT: u16 = 7456;

/// Convenience wrapper exposed for the daemon bin target so it
/// doesn't need to reach the private `state` module directly. P2.1
/// landed `AppState::load_from_dir(PathBuf)`; the bin calls this
/// helper instead of `AppState::load_from_dir` so the daemon bin
/// never touches private modules.
pub async fn load_daemon_state(data_dir: std::path::PathBuf) -> Arc<AppState> {
    Arc::new(AppState::load_from_dir(data_dir).await)
}

/// Spawn the RULE-DB-001 backup loop for the daemon bin (2026-08-24,
/// task `08-24-p1-db-backup-log-rotation`): snapshot the store via
/// `VACUUM INTO` at startup and every 24h afterwards, into
/// `<data_dir>/backups/`, pruning retention down to
/// [`backup::KEEP_BACKUPS`] files after every attempt. Exposed here
/// (same rationale as [`load_daemon_state`]: the bin never touches the
/// private `db` module directly).
///
/// Detached spawn (never joined) — the axum server runs in parallel,
/// and a backup failure only `warn!`s and waits for the next cycle:
/// the backup is an insurance layer and must never block daemon
/// startup/availability (R1). The GUI Full mode (in-process Tauri
/// escape hatch) deliberately stays backup-free — no timer tasks in
/// the GUI main process.
pub fn spawn_backup_task(state: &AppState, data_dir: &Path) {
    // SqlitePool is a cheap Arc clone; the task holds it for the
    // process lifetime alongside the server's own handle.
    let db = state.db.clone();
    let backups = backup::backup_dir(data_dir);
    tokio::spawn(async move {
        // A fresh interval's FIRST tick completes immediately → the loop
        // body runs once right away (startup snapshot); later ticks fire
        // 24h apart, so a long-lived daemon keeps fresh backups without
        // a restart.
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            interval.tick().await;
            let started = std::time::Instant::now();
            match backup::backup_database(&db, &backups).await {
                Ok(path) => {
                    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    tracing::info!(
                        path = %path.display(),
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        size_bytes,
                        "database backup snapshot written"
                    );
                }
                Err(e) => {
                    // D4: no in-loop retry — the 24h cycle IS the retry.
                    tracing::warn!(
                        error = %e,
                        "database backup failed (non-fatal); retrying next cycle"
                    );
                }
            }
            // Retention runs after every attempt so the dir stays bounded
            // even when an earlier backup half-failed. F3 磁盘治理
            // (2026-09-03)起 prune 为预算自适应(200MiB 预算 / 至少 2
            // 份 / 至多 KEEP_BACKUPS 份),返回携带回收字节数。
            match backup::prune_backups(&backups, backup::KEEP_BACKUPS) {
                Ok(outcome) if !outcome.removed.is_empty() => tracing::info!(
                    count = outcome.removed.len(),
                    reclaimed_bytes = outcome.reclaimed_bytes,
                    "pruned old database backups"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "prune database backups failed (non-fatal)")
                }
            }
        }
    });
}

/// Spawn the RULE-SHELL-001 completed-shell sweeper for the daemon
/// bin (2026-08-27, task `08-27-rule-shell-001-sweeper`): every
/// [`SWEEP_INTERVAL_MS`] (5min), prune Done shell entries older
/// than [`SHELL_RETENTION_MS`] (1h) from `state.background_shells`,
/// releasing their stdout/stderr buffers. Without this, a
/// weeks-long daemon process grows the `shells` map without bound
/// (Done entries keep their full output buffers even after the
/// disk spill). Exposed here with the same rationale as
/// [`load_daemon_state`] / [`spawn_backup_task`]: the bin never
/// touches the private `background_shell` module directly.
///
/// Detached spawn (never joined) following the backup-task
/// pattern. Each run is a pure in-memory map pass; a removal
/// count of 0 stays silent, > 0 logs at `info!` (no per-shell
/// details — session ids are not log-relevant here). The interval's
/// first tick fires immediately: a fresh daemon starts with an
/// empty map, so the startup sweep is a harmless no-op. The GUI
/// paths (Tauri Full mode, tests) deliberately do NOT assemble
/// this — "no timer tasks in the GUI main process"; GUI processes
/// are short-lived and `kill_all` on exit.
pub fn spawn_shell_sweeper(state: &AppState) {
    // Cheap Arc clone; the task holds it for the process lifetime
    // alongside the server's own handle.
    let shells = state.background_shells.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(SWEEP_INTERVAL_MS));
        loop {
            interval.tick().await;
            let removed = shells.sweep_completed_shells(SHELL_RETENTION_MS).await;
            if removed > 0 {
                tracing::info!(count = removed, "swept completed background shell entries");
            }
        }
    });
}

/// Wire the background-shell UI event emitter for the daemon process
/// (2026-09-02, task `09-02-chat-task-panel`): shell lifecycle events
/// (`background_shell:update`) broadcast to every SSE subscriber via
/// [`crate::daemon::sse::SseRegistry::broadcast`] — the same
/// dual-transport pattern as the subagent events. Exposed as a
/// function (not inlined in the bin) with the same rationale as
/// `spawn_shell_sweeper`: the bin never touches the private
/// `background_shell` module directly. Called ONCE from the daemon
/// bin at assembly, before `serve_daemon`; the Tauri Full mode wires
/// its own emitter in `lib.rs` setup, and Thin-mode GUIs never wire
/// one (events stay a registry-level no-op).
pub fn wire_background_shell_events(state: &AppState) {
    let sse = state.sse.clone();
    state
        .background_shells
        .set_event_emitter(Arc::new(move |name, payload| {
            sse.broadcast(name, payload);
        }));
}

/// F2 定时任务调度循环(2026-08-28, task `08-28-f2-scheduled-tasks`,
/// design §5):每 [`crate::scheduler::SCHEDULER_TICK_SECS`](30s)跑一次
/// 单一扫描算法(`scheduler::scheduler_tick`),到点任务经 `chat_inner`
/// 注入一轮带 origin 标记的 agent 运行。**唯一**装配点是 daemon bin
/// (GUI Full 模式零 timer 硬约束不变 ——「no timer tasks in the GUI
/// main process」)。
///
/// Detached spawn(不 join,不阻塞 serve),仿 `spawn_backup_task` /
/// `spawn_shell_sweeper` 形;停机沿 tunnel 心跳的
/// `CancellationToken + select!` 样板:tick 循环监听
/// `state.scheduler_cancel`(字段挂 `AppState`,`load_inner` 只构造
/// token 绝不 spawn —— RULE-DAEMON-001),`shutdown_signal` 在 tunnel
/// stop 之后 cancel。cancel 后循环当 tick 退出;正在 fire 的单个注入由
/// 既有 `cancel_and_drain_all_agent_loops` 兜底。
///
/// interval 的首 tick 立即完成 = 启动即做一次补偿评估(停机跨过 fire
/// 点的 D4 catch-up 语义)。`pending_by_task`(任务 → 队列条目 uuid
/// 去重表)由本循环体跨 tick 持有 —— 纯内存,daemon 重启即空,与
/// 消息队列的风险姿态一致。
pub fn spawn_task_scheduler(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        tracing::info!(
            tick_secs = crate::scheduler::SCHEDULER_TICK_SECS,
            "task scheduler started"
        );
        let mut pending_by_task: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut interval =
            tokio::time::interval(Duration::from_secs(crate::scheduler::SCHEDULER_TICK_SECS));
        loop {
            tokio::select! {
                biased;
                _ = state.scheduler_cancel.cancelled() => {
                    tracing::info!("task scheduler stopped (shutdown)");
                    break;
                }
                _ = interval.tick() => {
                    crate::scheduler::scheduler_tick(&state, &mut pending_by_task).await;
                }
            }
        }
    });
}

/// F3 磁盘治理每日节拍(2026-09-03, task `09-03-f3-disk-governance`
/// PR1):24h 一轮跑回收函数族(worker sweep / 孤儿 session worktree /
/// outputs / 备份 prune),首拍延迟 5 分钟避开启动 IO(migrate /
/// backup / orphan-guard)。**唯一装配点 = daemon bin**(GUI Full 模式
/// 只在启动期跑一次性 pass,零 timer 硬约束保持;Thin 场景由本节拍
/// 兜底——P0-a worker sweep 宿主断链由此闭合)。停机沿
/// `spawn_task_scheduler` 样板:token 挂 `AppState.disk_governor_cancel`,
/// `shutdown_signal` 在 scheduler cancel 同段 cancel。kill-switch
/// `disk_governor_enabled`(fail-open)每轮重读。本函数是 bin-facing
/// wrapper(同 `spawn_task_scheduler` 理由:bin 不触 crate 私有模块),
/// 本体在 `crate::disk::governor`。
pub fn spawn_disk_governor(state: &Arc<AppState>) {
    crate::disk::governor::spawn_disk_governor(state);
}

/// Build the un-mounted daemon router. Exposed so integration tests
/// (`daemon::routes::tests_*`) can construct a router against a
/// test-only `AppState` without going through the TCP serve loop.
///
/// `health` lives at the top level so it's reachable without the
/// `/api/v1` prefix too — the Q1 port-conflict probe hits
/// `/api/v1/health` specifically, but exposing the bare `/health`
/// path as well costs nothing and is friendlier for ad-hoc curl.
pub fn build_router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        .route("/health", get(routes::health::health))
        .route("/api/v1/health", get(routes::health::health))
        .merge(routes::router(state));

    // P2.4 D4: serve the built SPA from `app/dist/` so a single daemon
    // binary delivers both the API (`/api/v1/*`) and the frontend (same
    // origin → no CORS preflight in sidecar mode). Mounted as a
    // *fallback service* so every `/api/v1/*` route wins over static
    // files; only unmatched paths fall through to the SPA.
    //
    // SPA history-mode fallback: `ServeDir::not_found_service(ServeFile`
    // of `index.html`) so client-side routes (e.g. `/settings`) resolve
    // to the app shell instead of a 404. When the dist dir is absent
    // (dev mode — the frontend runs on vite :1420, or a daemon-only
    // deployment) the fallback is skipped, leaving a pure API server.
    match resolve_dist_dir() {
        Some(dist) => {
            tracing::info!(dist = %dist.display(), "serving static frontend from dist dir");
            let spa =
                ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html")));
            router = router.fallback_service(spa);
        }
        None => {
            tracing::info!(
                "no dist dir found (EVERLASTING_DIST_DIR unset + default \
                 absent); daemon runs as API-only (dev mode expects vite on :1420)"
            );
        }
    }

    // P2.3 dev-only CORS (C6):vite dev server (1420) 与 daemon
    // (7456) 跨域,`fetch` + `EventSource` 需 daemon 放行 preflight。
    // `very_permissive` 允许任意 origin / method / header(不带
    // credentials —— 此处 SSE/fetch 均无 cookie)。
    //
    // P2.4 决议:**保留**此 CORS 层。sidecar 同源场景下不触发
    // preflight(同源请求不经 CORS),故保留对 sidecar 零成本;而
    // dev 模式(vite 1420 ↔ daemon 7456 跨域)仍需要它。移除留 P2.5
    // 复盘定夺(若确认 dev 也可同源化则删)。
    router.layer(CorsLayer::very_permissive())
}

/// Resolve the frontend static-asset directory for `ServeDir` (P2.4 D4).
///
/// Resolution order:
/// 1. `EVERLASTING_DIST_DIR` env var (operator / test override; absolute).
/// 2. Default: walk up from the *daemon executable* to locate `app/dist`
///    ([`find_dist_dir`]). Walking up (rather than hard-coding a relative
///    path) is required because the daemon binary lives at different
///    depths across build modes — see [`find_dist_dir`] for the layout
///    table. Using `current_exe()` (not `env!("CARGO_MANIFEST_DIR")`)
///    keeps production single-binary deploys working regardless of
///    install layout.
///
/// Returns `None` when nothing resolves — callers treat that as
/// "API-only mode" (no frontend built, or daemon-only deployment).
pub fn resolve_dist_dir() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("EVERLASTING_DIST_DIR") {
        let p = PathBuf::from(raw);
        if p.is_dir() {
            return Some(p);
        }
        tracing::debug!(
            dist = %p.display(),
            "EVERLASTING_DIST_DIR set but not a directory; ignoring"
        );
    }
    find_dist_dir(&std::env::current_exe().ok()?)
}

/// Locate `app/dist` by walking up from a daemon executable path.
/// `current_exe()` is the canonical cross-platform way to locate
/// co-bundled assets; CARGO_MANIFEST_DIR only exists at build time.
/// Walk at most 10 levels to bound the scan.
///
/// Two layouts are recognized:
/// 1. **Tauri crate-root layout** (sidecar staging + pre-workspace
///    cargo): an ancestor named `src-tauri` → sibling `dist`.
///    - sidecar (P2.4 staging): `app/src-tauri/binaries/everlasting-daemon-<triple>`
///      → one `..` to `src-tauri`, then `../dist`
///    - pre-workspace `cargo build --release`:
///      `app/src-tauri/target/release/everlasting-daemon`
///      → three `..` to `src-tauri`, then `../dist`
/// 2. **Workspace layout** (2026-08-11 flip): the daemon binary now
///    lands at `<workspace>/target/<profile>/everlasting-daemon`, with
///    no `src-tauri` ancestor on the way up. Detect the workspace root
///    via its `app/src-tauri` child and take `app/dist`.
///
/// When layout 1's `src-tauri` is found but its sibling `dist` is
/// missing, the search stops (never falls through to an unrelated
/// `dist` higher up — the original P2.4 D4 guard).
fn find_dist_dir(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent()?;
    for _ in 0..10 {
        if dir.file_name().and_then(|n| n.to_str()) == Some("src-tauri") {
            // Layout 1: `dist` is a sibling of the crate root.
            let dist = dir.parent()?.join("dist");
            return dist.canonicalize().ok().filter(|p| p.is_dir());
        }
        if dir.join("app/src-tauri").is_dir() {
            // Layout 2: workspace root — `dist` lives under `app/`.
            let dist = dir.join("app/dist");
            return dist.canonicalize().ok().filter(|p| p.is_dir());
        }
        dir = dir.parent()?;
    }
    None
}

/// Resolve the daemon port per Q1 decision:
/// `--port <N>` flag > `EVERLASTING_DAEMON_PORT` env > default 7456.
///
/// The bin target handles the CLI / env parsing and passes the
/// resolved value here. This helper exists as a free function so
/// tests can verify the precedence chain without spawning the CLI.
pub fn resolve_port(cli_port: Option<u16>, env_port: Option<u16>) -> u16 {
    cli_port.or(env_port).unwrap_or(DEFAULT_DAEMON_PORT)
}

/// 设计参考:graceful shutdown drain 阶段的预期硬上限(秒)。正常路径下
/// [`SseRegistry::shutdown`] 主动结束后 SSE 流会亚秒完成;agent loop drain
/// 由 [`DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`] 独立兜底。这个常量现已**不再**
/// 用于 `serve_daemon`(历史 bug:`tokio::time::timeout(3s, serve)` 会给
/// 整个服务 future 套超时 → 3s 后无信号自杀),仅保留供 shutdown 回归测试
/// (`serve_daemon_*` 系列)计算「daemon 应在多久内响应/不应在多久内退出」
/// 的预算基准,以及作为 `scripts/daemon.sh`(15s SIGKILL)与文档的参考锚点。
/// 故加 `#[cfg(test)]` —— prod 构建里它是 dead code,不该出现在二进制里。
#[cfg(test)]
const SHUTDOWN_GRACE_SECS: u64 = 3;

/// Agent loop drain 的总 timeout 上限(秒)。收到信号、关完 SSE 后,
/// 遍历 `state.cancellations` cancel 所有活跃 agent loop,再并发 await
/// 它们的退出信号(`state.inflight_exits`),最多等这么久。实测路径下
/// loop 多在亚秒退出(用户点 Stop 走的是同一条 cancel 路径),8s 是纯
/// 兜底 —— 对照 [`crate::agent::helpers::await_inflight_exit`] 单 loop
/// 的 10s,并发 drain 理论上比串行快。
///
/// 与历史的 axum-drain grace(3s,仅作回归测试预算基准 `SHUTDOWN_GRACE_SECS`)
/// 正交:先 drain loop(让 in-flight tool 跑完 `persist_turn` 落库),再让
/// axum drain 短请求。
/// 两者串行最坏 11s,故 `scripts/daemon.sh` 的 SIGKILL 窗口拉到 15s
/// 留 4s 余量,保证 SIGKILL 永远是「等不过来」的最后手段而非抢先于
/// drain。详见 `agent::helpers::cancel_and_drain_all_agent_loops` 与
/// `.trellis/spec/backend/daemon-server.md`。
const DAEMON_SHUTDOWN_LOOP_DRAIN_SECS: u64 = 8;

/// Bind + serve the daemon on `0.0.0.0:PORT`. The `everlasting-daemon`
/// bin target is a thin shell that resolves the port, loads
/// `AppState`, and calls this.
///
/// # Graceful shutdown
///
/// 收到 Ctrl+C (SIGINT) 或 SIGTERM (POSIX) 后:
/// 1. [`shutdown_signal`] 先调 [`SseRegistry::shutdown`] 主动 drop 所有
///    SSE 订阅者 → `stream.rs` 的 SSE body 自然 `end()` → axum 感知
///    这些连接「完成」。这一步根治了"SSE 长连接永不自然完成 →
///    graceful shutdown 无限挂起"的问题(原本靠 `daemon.sh` SIGKILL 兜底)。
/// 2. **接着 drain 活跃 agent loop**:[`agent::helpers::cancel_and_drain_all_agent_loops`]
///    遍历 `state.cancellations` cancel 所有正在跑的 loop(agent loop 的
///    `select!` cancel 臂 `biased;` 优先命中,走用户点 Stop 的同一条路径),
///    再并发 await `state.inflight_exits` 的退出信号(总 timeout
///    [`DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`])。让 in-flight tool 跑完
///    `persist_turn` 落库后进程才退出 —— 否则 runtime 销毁会硬斩 spawn
///    task,丢「tool 已执行、DB 未落」那一轮(原 spec 的 follow-up 缺口)。
/// 3. `shutdown_signal` 返回后,axum 的 `with_graceful_shutdown` 开始
///    drain 所有 in-flight 连接 —— 此时 SSE 流已结束,只剩可快速 drain
///    的短请求,整体亚秒完成。
/// 4. (历史)曾有 `SHUTDOWN_GRACE_SECS` timeout 兜底未知长连接;2026-07-27
///    移除 —— 它误套整个 serve future 致 daemon 无信号 3s 自杀。现由
///    `daemon.sh`(standalone)/ GUI sidecar 的 SIGKILL 作最后防线。
///
/// 关键:必须用 `with_graceful_shutdown`(而非 `select!` + drop serve
/// future)。drop `axum::serve` 的 future 会 abort 所有连接 task(粗暴
/// 断开),而非 drain —— 那会丢失正在处理中的请求。
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "everlasting-daemon listening");

    let router = build_router(Arc::clone(&state));
    let serve =
        axum::serve(listener, router).with_graceful_shutdown(shutdown_signal(Arc::clone(&state)));

    // 正常服务直到收到 SIGINT/SIGTERM 并完成 drain。**不能**在这里给整个
    // `serve` future 套 `tokio::time::timeout` —— 历史 bug 就出在这:
    //
    //   tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve)
    //
    // `serve`(= `axum::serve(...).with_graceful_shutdown(sig)`)在没有
    // shutdown 信号时会**永久**跑下去(正常服务请求),于是 3s 超时必然
    // 触发 `Err` 臂 → `serve_daemon` 返回 `Ok(())` → bin 打印
    // "exited cleanly" → 进程 exit 0。表现:daemon 每次 listen 后 ~3s
    // 自杀(sidecar terminated `code:Some(0), signal:None`),前端 15s
    // health probe 永远连不上 → "daemon 不可用"。原本意图(注释里写的
    // "timeout 兜底")只想兜住 drain 阶段,但 `with_graceful_shutdown`
    // 把「等信号」和「drain」捆在同一个 future 里,无法只对 drain 加超时。
    //
    // drain 的硬上限由 `shutdown_signal` 内部保证:
    //   1. `sse.shutdown()` 主动断所有 SSE 长连接(根治挂起的源头);
    //   2. `cancel_and_drain_all_agent_loops` 在
    //      `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`(8s)内 cancel + drain 所有
    //      活跃 agent loop。
    // 之后 axum 只剩短请求,drain 亚秒完成。万一仍有未知长连接卡住:
    //   - standalone 脚本路径:`scripts/daemon.sh` 的 SIGKILL(15s)兜底;
    //   - sidecar 路径:GUI `RunEvent::Exit` → `child.kill()` 经
    //     tauri-plugin-shell 升级到 SIGKILL。
    // 故此处不再额外加 timeout —— 既修掉自杀 bug,又不丢兜底语义。
    serve.await?;
    tracing::info!("everlasting-daemon shutdown complete");
    Ok(())
}

/// Graceful shutdown signal handler. Fires on Ctrl+C (portable) or
/// SIGTERM (Unix only). Windows builds fall back to Ctrl+C only,
/// which matches the Tauri GUI's signal handling convention.
///
/// 信号触发后,**返回前**按顺序做两件事(都在 axum 的 graceful drain
/// 开始之前):
///
/// 1. **[`SseRegistry::shutdown`]** —— 主动结束所有 SSE 长连接。这是
///    让 axum `with_graceful_shutdown` 不被永不完成的 SSE 连接卡住的
///    关键(不主动关 SSE 的话,graceful drain 会无限等待每个活跃的
///    `GET /api/v1/stream`)。
/// 2. **[`crate::agent::helpers::cancel_and_drain_all_agent_loops`]** ——
///    cancel 所有活跃 agent loop 并并发 await 它们的退出信号(总 timeout
///    [`DAEMON_SHUTDOWN_LOOP_DRAIN_SECS`])。让正在跑的 loop 走 cancel
///    路径(同用户点 Stop),把 in-flight tool 的 `persist_turn` 跑完落库,
///    避免进程退出时 runtime 销毁硬斩 spawn task 丢一轮结果。
///
/// 接收完整 `Arc<AppState>` 而非只接 `SseRegistry`,是因为第 2 步要访问
/// `state.cancellations` + `state.inflight_exits`(参见
/// `agent::helpers::cancel_and_drain_all_agent_loops` 的三件套)。
async fn shutdown_signal(state: Arc<AppState>) {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }

    // 步骤 1:主动结束所有 SSE 长连接 —— 在返回给 axum 的
    // graceful_shutdown 之前,让 drain 不被永不完成的 SSE 连接卡住。
    state.sse.shutdown();

    // 步骤 1.5(S2, 2026-08-11, task `08-11-tunnel-client`):停 tunnel
    // 客户端。先停 tunnel 再 drain —— WSS 连接主动关闭(发 Close 帧),
    // remote 立刻感知节点离线;tunnel 是独立 task,失败/断线从不影响
    // daemon,这里只是给它一个明确的 shutdown 信号(design §7 对齐:
    // 与现有 serve 行为正交,只加通知,不改 drain 语义)。
    state.tunnel_manager.stop();

    // 步骤 1.6(F2 定时任务, 2026-08-28, design §5):停调度循环。
    // cancel 后 tick 循环当拍退出,不再发起新的 fire;正在 fire 的单个
    // 注入由下方 `cancel_and_drain_all_agent_loops` 兜底(该注入已注册
    // 进 cancellations/inflight_exits)。
    state.scheduler_cancel.cancel();

    // 步骤 1.7(F3 磁盘治理, 2026-09-03):停每日磁盘回收节拍(同款
    // token 样板)。回收是 best-effort 文件操作,cancel 后 tick 循环
    // 当拍退出即可——正在进行的单项删除不中断、不回滚(被删数据本就
    // 是裁定可删的),下一轮不再发起。
    state.disk_governor_cancel.cancel();

    // 步骤 2:cancel + drain 所有活跃 agent loop。必须排在 sse.shutdown
    // 之后(先断流、再停处理,语义更干净;且 SSE 关后前端不再收到新事件,
    // 也不会有新 chat request 到达)。这一步让 in-flight tool 跑完
    // persist_turn 落库,闭合原 spec 的 follow-up 缺口(agent loop 硬终止)。
    crate::agent::helpers::cancel_and_drain_all_agent_loops(
        &state.cancellations,
        &state.inflight_exits,
        Duration::from_secs(DAEMON_SHUTDOWN_LOOP_DRAIN_SECS),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 进程级互斥锁:两个发真实 SIGTERM 给 `getpid()` 的集成测试
    /// (`serve_daemon_shutdown_completes_with_active_sse` 与
    /// `serve_daemon_shutdown_drains_active_agent_loop`)**必须串行**。
    /// 否则 cargo test 默认多线程下,A 的 SIGTERM 会被 B 的
    /// `shutdown_signal` select 臂捕获(同一进程,同一信号),造成
    /// 「daemon 起不来 / shutdown 被对端抢先触发」的假失败。二者
    /// 共享本锁,确保任一时刻只有一个 SIGTERM 测试在跑。
    static SIGNAL_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Q1 port resolution precedence: CLI > env > default. The bin
    /// target's `--port` flag wins, then the env var, then the
    /// 7456 default. The single source of truth for the
    /// "never auto-hop ports" decision (Q1) is `serve_daemon`'s
    /// `TcpListener::bind` — failing there fails loud.
    #[test]
    fn port_resolution_precedence() {
        // No CLI, no env → default.
        assert_eq!(resolve_port(None, None), DEFAULT_DAEMON_PORT);
        // Env wins over default.
        assert_eq!(resolve_port(None, Some(9999)), 9999);
        // CLI wins over env.
        assert_eq!(resolve_port(Some(7456), Some(9999)), 7456);
    }

    /// P2.4 D4: `EVERLASTING_DIST_DIR` pointing at a real dir wins.
    /// Uses `tempfile` to avoid coupling to the build's `app/dist/`
    /// presence. This test MUST run with the env var set ONLY for this
    /// process; since `cargo test` serializes env mutations poorly,
    /// we use a unique var name pattern via the existing function —
    /// but the function reads the canonical `EVERLASTING_DIST_DIR`,
    /// so we accept that this test is order-sensitive and run it
    /// without the env set elsewhere (the default-path test below).
    #[test]
    fn dist_dir_env_override_when_dir_exists() {
        // SAFETY on parallelism: we set+unset the env around a single
        // read. Other tests that call `resolve_dist_dir()` without
        // setting the env will simply fall through to the default-path
        // branch, which is env-independent. The only collision would
        // be two tests both setting EVERLASTING_DIST_DIR — there is
        // exactly one such test (this one).
        let tmp = tempfile::tempdir().expect("create tempdir");
        std::env::set_var("EVERLASTING_DIST_DIR", tmp.path());
        let resolved = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_DIST_DIR");
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    /// P2.4 D4: a non-existent `EVERLASTING_DIST_DIR` is ignored
    /// (falls through to default, which may or may not exist depending
    /// on the build host — we only assert it doesn't panic).
    #[test]
    fn dist_dir_env_ignored_when_not_a_dir() {
        std::env::set_var(
            "EVERLASTING_DIST_DIR",
            "/nonexistent/path/that/does/not/exist",
        );
        // Must not panic; result is host-dependent (default path).
        let _ = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_DIST_DIR");
    }

    // ---- find_dist_dir:纯路径单测(不碰 env / current_exe,并行安全)----

    /// Layout 1(pre-workspace / sidecar):exe 在 `src-tauri/target/...`
    /// 之下 → 找到 src-tauri 的兄弟 `dist`。
    #[test]
    fn find_dist_dir_pre_workspace_layout() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let src_tauri = tmp.path().join("src-tauri");
        let dist = tmp.path().join("dist");
        std::fs::create_dir_all(src_tauri.join("target/release")).unwrap();
        std::fs::create_dir_all(&dist).unwrap();
        let exe = src_tauri.join("target/release/everlasting-daemon");

        assert_eq!(find_dist_dir(&exe), Some(dist.canonicalize().unwrap()));
    }

    /// Layout 2(2026-08-11 workspace 翻转):exe 在 `<workspace>/target/...`
    /// 之下,祖先里没有 `src-tauri` → 靠 `app/src-tauri` 标记定位
    /// workspace 根,取 `app/dist`。这是 daemon.sh 指向根 target 后
    /// 浏览器模式必须命中的路径。
    #[test]
    fn find_dist_dir_workspace_layout() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let app = tmp.path().join("app");
        std::fs::create_dir_all(app.join("src-tauri")).unwrap();
        std::fs::create_dir_all(app.join("dist")).unwrap();
        let exe = tmp.path().join("target/release/everlasting-daemon");

        assert_eq!(
            find_dist_dir(&exe),
            Some(app.join("dist").canonicalize().unwrap()),
            "workspace layout must resolve to app/dist"
        );
    }

    /// Layout 1 命中 `src-tauri` 但兄弟 `dist` 缺失 → 停(不上溯),
    /// 即使 src-tauri 之上还有无关的 `dist`(P2.4 D4 的防串味守卫)。
    #[test]
    fn find_dist_dir_stops_at_src_tauri_without_dist() {
        let outer = tempfile::tempdir().expect("create outer tempdir");
        // 无关 dist 放在 src-tauri 之上(外层);inner 里 src-tauri 的
        // 兄弟没有 dist —— 必须返回 None,不能上溯命中 outer/dist。
        std::fs::create_dir_all(outer.path().join("dist")).unwrap();
        let inner = outer.path().join("inner");
        std::fs::create_dir_all(inner.join("src-tauri/target/release")).unwrap();
        let exe = inner.join("src-tauri/target/release/everlasting-daemon");

        assert_eq!(find_dist_dir(&exe), None);
    }

    /// 什么布局都不命中 → None(纯 API 模式)。
    #[test]
    fn find_dist_dir_none_when_no_layout_matches() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        std::fs::create_dir_all(tmp.path().join("target/release")).unwrap();
        let exe = tmp.path().join("target/release/everlasting-daemon");
        assert_eq!(find_dist_dir(&exe), None);
    }

    /// Graceful shutdown 端到端:有活跃 SSE 长连接时,daemon 收到
    /// SIGTERM 后必须在 grace window 内退出,而不是无限挂起(原本
    /// 靠 daemon.sh SIGKILL 兜底)。
    ///
    /// 这是本任务的核心回归测试:若 `serve_daemon` 回归到「等所有
    /// in-flight 连接完成」的旧行为(没有 `sse.shutdown()` 主动关
    /// SSE),本测试会因为 `serve_daemon` 超过 grace window 超时
    /// 而失败。
    ///
    /// 测试通过的标准:`serve_daemon` 在 `SHUTDOWN_GRACE_SECS` 内
    /// 返回(而非挂起/超时),证明 `sse.shutdown()` 主动结束后 axum
    /// 的 graceful drain 能快速完成。
    #[cfg(unix)]
    #[tokio::test]
    async fn serve_daemon_shutdown_completes_with_active_sse() {
        use std::sync::Arc;
        use tempfile::TempDir;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        // 本测试向 getpid() 发真实 SIGTERM,与 drain 测试共享进程信号,
        // 必须串行(见 SIGNAL_TEST_MUTEX)。
        let _guard = SIGNAL_TEST_MUTEX.lock().await;

        // 预占一个 ephemeral port,取出端口号后立即 drop,交给
        // serve_daemon 重绑(有微小竞态,测试可接受)。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let dir = TempDir::new().expect("tempdir");
        let state = load_daemon_state(dir.path().to_path_buf()).await;

        // spawn serve_daemon。
        let serve_handle = tokio::spawn(serve_daemon(Arc::clone(&state), port));

        // 等 daemon 起来(轮询 health)。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("daemon did not become healthy in 5s");
            }
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                let req =
                    b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(req).await;
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await;
                if buf.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 建立一个 SSE 长连接 —— 这是触发旧 bug「graceful shutdown
        // 挂起」的必要条件。连接保持打开,不发任何会让它结束的内容。
        let mut sse = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect for SSE");
        let sse_req =
            b"GET /api/v1/stream HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n";
        sse.write_all(sse_req).await.expect("send SSE GET");
        // 读一点响应头确认连接建立(200 + headers),但不读完 body
        // (SSE body 无限流)。
        let mut hdr = [0u8; 64];
        let _ = sse.read(&mut hdr).await;
        assert!(
            hdr.starts_with(b"HTTP/1.1 200"),
            "SSE endpoint must respond 200, got: {}",
            String::from_utf8_lossy(&hdr)
        );

        // 发真实 SIGTERM 给当前进程 —— 触发 serve_daemon 的
        // shutdown_signal。libc 已是项目依赖(process group kill 等)。
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }

        // 核心断言:serve_daemon 必须在 grace window 内完成。
        // SHUTDOWN_GRACE_SECS 是 serve_daemon 内部的 timeout 上限;
        // 正常路径下 sse.shutdown() 后 drain 亚秒完成。留 2x 余量
        // 防 CI 慢机器。
        let completion = tokio::time::timeout(
            std::time::Duration::from_secs(SHUTDOWN_GRACE_SECS * 2 + 2),
            serve_handle,
        )
        .await;

        match completion {
            Ok(Ok(_)) => {
                // serve_daemon 正常返回 —— 通过。
            }
            Ok(Err(e)) => panic!("serve_daemon returned error: {e}"),
            Err(_) => {
                panic!(
                    "serve_daemon did NOT complete within {}s of SIGTERM — \
                     graceful shutdown is still hanging on the active SSE \
                     connection (the bug this test guards against)",
                    SHUTDOWN_GRACE_SECS * 2 + 2
                );
            }
        }
    }

    /// Graceful shutdown 在「有活跃 agent loop」时也必须在窗口内完成 ——
    /// 这是 task `07-24-daemon-agent-loop-shutdown` 的核心回归守卫。
    ///
    /// 本测试验证的是 **shutdown drain 机制本身**(cancel 遍历 + 并发 await
    /// + 总 timeout 兜底),而非「persist_turn 真的落库」(后者由
    /// `tests_agent_loop.rs::agent_loop_cancel_in_turn_2_kills_loop` 在
    /// agent loop 层单测覆盖,daemon 路径注入 provider 成本过高,见
    /// design.md §6.2 方案选型 (b))。
    ///
    /// 构造方式:起 `serve_daemon`,直接往 `state.cancellations` 塞一个
    /// CancellationToken + 对应 `state.inflight_exits` 塞一个**永不 resolve**
    /// 的 oneshot receiver(把 sender 移进一个永不完成的 task 持有)。这
    /// 模拟「一个真正卡死的 agent loop」—— 是 drain timeout 兜底的
    /// 最坏情况。发 SIGTERM 后,`shutdown_signal` 应:
    ///   1. `sse.shutdown()`(无 SSE 连接,亚秒)
    ///   2. cancel 那个 token(断言 `is_cancelled()`)
    ///   3. 并发 await receiver —— 卡住,但总 timeout
    ///      `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS` 到后 return(不挂起)
    ///   4. axum drain + serve 返回
    ///
    /// 通过标准:`serve_daemon` 在 `DAEMON_SHUTDOWN_LOOP_DRAIN_SECS +
    /// SHUTDOWN_GRACE_SECS + 余量` 内返回(回归守卫:若 drain 机制被破坏
    /// 成「无限等 receiver」,本测试超时失败)。同时断言 token 已被 cancel
    /// (证明 cancel 步骤真的跑了,不是没跑直接退)。
    #[cfg(unix)]
    #[tokio::test]
    async fn serve_daemon_shutdown_drains_active_agent_loop() {
        use std::sync::Arc;
        use tempfile::TempDir;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;
        use tokio_util::sync::CancellationToken;

        // 本测试向 getpid() 发真实 SIGTERM,与 SSE 测试共享进程信号,
        // 必须串行(见 SIGNAL_TEST_MUTEX)。
        let _guard = SIGNAL_TEST_MUTEX.lock().await;

        // 预占 ephemeral port。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let dir = TempDir::new().expect("tempdir");
        let state = load_daemon_state(dir.path().to_path_buf()).await;

        // 植入一个「活跃但卡死」的 agent loop:token + 永不 resolve 的
        // exit receiver(sender 被一个永不完成的 task 持有,receiver 永远
        // pending)。这正是 drain timeout 兜底要应对的最坏情况 —— 若 drain
        // 机制正确,会在 DAEMON_SHUTDOWN_LOOP_DRAIN_SECS 后 return;若被
        // 破坏成无限等,本测试超时失败。
        let token = CancellationToken::new();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut map = state.cancellations.lock().await;
            map.insert("rid-hung".to_string(), token.clone());
        }
        {
            let mut map = state.inflight_exits.lock().await;
            map.insert("rid-hung".to_string(), done_rx);
        }
        // 把 sender 移进永不完成的 task 持有(模拟卡死的 loop 不会 send)。
        // `done_tx` 一旦 drop 会令 receiver 立刻 resolve(Err),这就测不出
        // timeout 了;所以必须让它在 task 里活着。
        let _keeper = tokio::spawn(async move {
            let _keep_alive = done_tx;
            std::future::pending::<()>().await;
        });

        let serve_handle = tokio::spawn(serve_daemon(Arc::clone(&state), port));

        // 等 daemon 起来(轮询 health)。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("daemon did not become healthy in 5s");
            }
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                let req =
                    b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(req).await;
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await;
                if buf.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 发真实 SIGTERM。
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }

        // 核心断言 1:serve_daemon 必须在 drain timeout + grace + 余量内
        // 返回。DAEMON_SHUTDOWN_LOOP_DRAIN_SECS 是 drain 的硬上限,加上
        // SHUTDOWN_GRACE_SECS(axum drain)和 3s 余量防 CI 慢机器。若 drain
        // 机制退化成「无限等 receiver」,这里会超时失败。
        let worst = DAEMON_SHUTDOWN_LOOP_DRAIN_SECS + SHUTDOWN_GRACE_SECS + 3;
        let completion =
            tokio::time::timeout(std::time::Duration::from_secs(worst), serve_handle).await;

        match completion {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => panic!("serve_daemon returned error: {e}"),
            Err(_) => {
                panic!(
                    "serve_daemon did NOT complete within {}s of SIGTERM — \
                     the agent-loop drain is hanging on the never-resolving \
                     receiver instead of hitting DAEMON_SHUTDOWN_LOOP_DRAIN_SECS",
                    worst
                );
            }
        }

        // 核心断言 2:token 已被 cancel。证明 shutdown 路径确实跑了 cancel
        // 步骤(不是没 cancel 就直接退)。
        assert!(
            token.is_cancelled(),
            "the hung agent loop's token must be cancelled by the shutdown drain"
        );
    }

    /// `serve_daemon` 在**不发任何信号**时必须持续服务 —— 这是
    /// 2026-07-27 修复的回归的**直接**守卫。
    ///
    /// 历史 bug:`tokio::time::timeout(SHUTDOWN_GRACE_SECS, serve)` 套在
    /// 整个 `serve` future 外。`serve`(`axum::serve(...).with_graceful_shutdown(sig)`)
    /// 无信号时永久运行(正常),于是 3s 后 timeout 必然 `Err` → 进程 exit 0
    /// → daemon 每次 listen 后 ~3s 自杀,前端 15s health probe 永远连不上。
    ///
    /// 本测试不发 SIGTERM/SIGINT,只起 `serve_daemon` 然后跨过
    /// `SHUTDOWN_GRACE_SECS`(3s)后再探一次 `/api/v1/health`。旧代码下 daemon
    /// 会在 3s 时退出,第二次探活连不上(或 `serve_handle` 已 resolve)→
    /// 断言失败。修复后 daemon 仍在,health 仍 200,`serve_handle` 仍 pending。
    ///
    /// 与上面两个 SIGTERM 测试互补:它们只验证「信号来了能 drain」,无法
    /// 捕获「没信号也自杀」。本测试填这个空。
    #[cfg(unix)]
    #[tokio::test]
    async fn serve_daemon_keeps_serving_without_signal_past_grace_window() {
        use std::sync::Arc;
        use tempfile::TempDir;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        // 本测试自身不发信号,但 **必须** 与两个 SIGTERM 测试
        // (`serve_daemon_shutdown_completes_with_active_sse` /
        // `serve_daemon_shutdown_drains_active_agent_loop`)串行。原因:
        // `shutdown_signal` 安装的是**进程级** SIGTERM handler,cargo test
        // 默认多线程下,若本测试与某个 `libc::kill(getpid(), SIGTERM)`
        // 测试并发,那个信号会命中本 daemon 的 handler → 本 daemon 被错误
        // 触发 graceful shutdown → 睡过 grace window 后 health 探活失败
        // → 假性回归失败。共享同一把 `SIGNAL_TEST_MUTEX` 即可规避。
        // (早先注释判断「不发信号所以无需共享」是错的,正是此处漏隔离。)
        let _guard = SIGNAL_TEST_MUTEX.lock().await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let dir = TempDir::new().expect("tempdir");
        let state = load_daemon_state(dir.path().to_path_buf()).await;

        let mut serve_handle = tokio::spawn(serve_daemon(Arc::clone(&state), port));

        // 辅助:裸 TCP GET /api/v1/health,返回是否 200。
        async fn health_ok(port: u16) -> bool {
            let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await else {
                return false;
            };
            let req =
                b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(req).await;
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf).await;
            buf.starts_with(b"HTTP/1.1 200")
        }

        // 等 daemon 起来。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("daemon did not become healthy in 5s");
            }
            if health_ok(port).await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 睡过 SHUTDOWN_GRACE_SECS + 余量。旧代码(timeout 包整个 serve)
        // 在此刻 daemon 已自杀。
        tokio::time::sleep(std::time::Duration::from_secs(SHUTDOWN_GRACE_SECS + 2)).await;

        // 核心断言 1:daemon 仍在服务(health 仍 200)。
        assert!(
            health_ok(port).await,
            "daemon stopped serving within {}s without any signal — it is \
             auto-terminating (regression: tokio::time::timeout wrapping the \
             whole serve future)",
            SHUTDOWN_GRACE_SECS + 2,
        );

        // 核心断言 2:serve_handle 仍未 resolve(进程没有自发退出)。
        // 用极短超时轮询:立即返回 Ready 说明 serve 已退出(回归)。
        match tokio::time::timeout(std::time::Duration::from_millis(100), &mut serve_handle).await {
            Ok(res) => panic!(
                "serve_daemon returned on its own without a signal: {res:?} — \
                 it must serve indefinitely until SIGINT/SIGTERM"
            ),
            Err(_) => { /* still pending — 正确 */ }
        }

        // 清理:cancel 整个测试的 serve(避免 leak 到其他测试)。
        serve_handle.abort();
    }
}
