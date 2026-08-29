// 见 lib.rs 同名 allow:clippy 1.96 对自然语言 doc 注释的误报。
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]

//! `everlasting-daemon` bin entry (Phase 2.2 B1, 2026-07-21).
//!
//! Hosts the agent core outside the Tauri GUI process. Shares the
//! same `everlasting_lib` crate as the Tauri app so `AppState`,
//! `agent::chat_loop`, `db`, `tools`, etc. are single-sourced —
//! only the entry point differs (`#[tokio::main]` here vs Tauri's
//! `Builder::setup` in `lib.rs`).
//!
//! ## CLI
//!
//! ```sh
//! everlasting-daemon [--port <N>] [--data-dir <PATH>]
//! # or
//! EVERLASTING_DAEMON_PORT=<N> everlasting-daemon
//! ```
//!
//! `--data-dir` (P2.4 D2.3): the GUI sidecar passes the Tauri-resolved
//! `app_data_dir` so the daemon opens the SAME SQLite file the GUI
//! would have (P2.1 path consistency). Absent → platform default.
//!
//! Port resolution (Q1 decision, daemon/server.rs::resolve_port):
//! `--port` flag > `EVERLASTING_DAEMON_PORT` env > default 7456.
//! Port conflicts fail loud with a Chinese user-facing message
//! (never auto-hop ports — Q1 explicitly rejects that to avoid
//! multi-daemon data splits).
//!
//! ## Scope (P2.2)
//!
//! - 79 HTTP routes via `_inner` delegation (Q0 decision).
//! - `GET /api/v1/health` returns `{daemonId, daemonVersion,
//!   apiVersions, uptimeSeconds}`.
//! - Graceful shutdown on SIGINT / SIGTERM.
//!
//! Out of scope for P2.2:
//! - SSE event stream (P2.3 — `HttpSseSink` + `/api/v1/stream`).
//! - Static-file serving for production single-binary deploy
//!   (P2.4 — `tower-http::services::ServeDir`).
//! - E2E harness (P2.5).

use std::process::ExitCode;

use clap::{Arg, ArgAction, Command};

use everlasting_lib::daemon::server;

#[tokio::main]
async fn main() -> ExitCode {
    // Initialize tracing FIRST so the orphan-guard below can log.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,everlasting=debug")),
        )
        .init();

    // Orphan-guard (2026-07-27): 让 daemon 在父进程(GUI sidecar 持有者)
    // 死亡时自动退出。Linux 用 `prctl(PR_SET_PDEATHSIG, SIGTERM)` 让内核在
    // 父进程终止时给本进程发 SIGTERM —— daemon 已有的 `shutdown_signal`
    // handler 会走优雅退出。
    //
    // 为什么需要(sidecar 场景):`tauri dev` 的 Rust live reload 是**强制
    // kill GUI 进程**(不走 `RunEvent::Exit`),所以 `SidecarHandle::kill()`
    // 永远跑不到,daemon 会成孤儿继续占端口 → 下次 sidecar 探测端口冲突
    // exit 1 → 前端 "daemon 不可用"。GUI 正常退出 / crash / 被强杀同理。
    // prctl 把"daemon 生命周期绑死 GUI"沉到内核层,无论 GUI 怎么死都生效。
    //
    // 2026-08-24 gate 修复:仅 sidecar 模式(`EVERLASTING_SIDECAR=1`,
    // 由 sidecar.rs 的 spawn 注入)才启用。此前无条件设置,standalone 的
    // `daemon.sh bg` 必死:daemon 的父进程是 daemon.sh 脚本本身,脚本在
    // `sleep 1.5` 验活通过、打印"后台运行中"后正常退出 → 内核 PDEATHSIG
    // 发 SIGTERM → daemon 起来 ~1.5s 即被优雅杀掉(日志只见 graceful
    // shutdown,极易误判为"自己崩了")。standalone(`daemon.sh` /
    // `cargo run` / CI)不绑父进程生命周期:后台服务被 init 收养是正常
    // 形态;防护 1(`getppid()==1` 拒启动)也一并 gate —— systemd unit /
    // daemonize 场景 ppid 天然是 1,不 gate 会直接拒启。
    //
    // 两个 race 防护(POSIX prctl 的已知坑):
    //   1. 若父进程在 prctl 调用前已死,本进程会被 init(PID 1)收养 →
    //      prctl 此时绑的是 init,不会触发。所以先判 `getppid() == 1` 直接退。
    //   2. `PR_SET_PDEATHSIG` 只在调用那一刻设置;父进程之后的变更不再
    //      监听。daemon 不会 reparent(整个生命周期父进程不变),故无需重设。
    #[cfg(target_os = "linux")]
    if is_sidecar_mode() {
        tracing::debug!("sidecar mode: orphan-guard armed (PDEATHSIG=SIGTERM)");
        // 防护 1:父进程已死(被 init 收养)→ 立即退,不 bind 端口。
        // 排除自身 PID==1 的极端情况(容器里 daemon 可能就是 init)。
        if std::process::id() != 1 && unsafe { libc::getppid() } == 1 {
            tracing::error!(
                "everlasting-daemon: sidecar parent already exited (reparented to init); refusing to start as orphan"
            );
            return ExitCode::from(1);
        }
        // 防护 2:设 PDEATHSIG = SIGTERM。失败仅 warn,不阻塞启动
        // (降级到"GUI 必须显式 kill"的旧行为,孤儿仍可能 —— 至少不崩)。
        // SAFETY: PR_SET_PDEATHSIG + 合法 signum 是定义良好的;main 早期单线程,
        // 无并发。参数按 libc 绑定(5 个 c_ulong)传入。
        let rc = unsafe {
            libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGTERM as libc::c_ulong,
                0,
                0,
                0,
            )
        };
        if rc != 0 {
            tracing::warn!(
                error = %std::io::Error::last_os_error(),
                "PR_SET_PDEATHSIG failed (non-fatal); daemon will NOT auto-die with parent — orphan possible on GUI crash/reload"
            );
        }
    }

    let port = parse_port_from_args();
    // P2.4 D2.3: `--data-dir` lets the GUI sidecar pass the exact
    // `app_data_dir` the Tauri app would have resolved (P2.1 path
    // consistency invariant — daemon + GUI must read/write the same
    // SQLite file). Falls back to the platform default when absent
    // (standalone daemon runs, CI, dev `cargo run --bin`).
    let data_dir = parse_data_dir_from_args().unwrap_or_else(resolve_data_dir);

    tracing::info!(
        port,
        sidecar = is_sidecar_mode(),
        data_dir = %data_dir.display(),
        daemon_version = env!("CARGO_PKG_VERSION"),
        "everlasting-daemon starting"
    );

    // Q1 port-conflict probe (fail-loud): GET the
    // `/api/v1/health` endpoint. A 200 with the matching
    // `daemon_version` means "ours, reuse" (caller exits cleanly);
    // a 200 from something else OR a non-200 means the port is
    // squatted. P2.2 ships the simplest variant: any successful
    // response fails the startup with a Chinese message. The
    // richer layered check (same daemon_id reuse, other process
    // loud-fail) lands when P2.4 ships the GUI sidecar handshake.
    if let Err(msg) = probe_port_conflict(port).await {
        eprintln!("{msg}");
        return ExitCode::from(1);
    }

    // AppState::load_from_dir (P2.1) — opens the SQLite pool,
    // runs migrations, builds the provider catalog, spawns the
    // startup backfill. The `projects:refreshed` emit path is
    // skipped (no AppHandle available; P2.3 wires the SSE sink).
    let state = server::load_daemon_state(data_dir.clone()).await;

    // RULE-DB-001 backup loop (2026-08-24, task
    // `08-24-p1-db-backup-log-rotation`): startup snapshot + every 24h,
    // `VACUUM INTO` into `<data_dir>/backups/`. Detached task; backup
    // failures only warn (insurance layer, never an availability risk).
    // See `spawn_backup_task` for the full contract.
    server::spawn_backup_task(&state, &data_dir);

    // RULE-SHELL-001 sweeper (2026-08-27, task
    // `08-27-rule-shell-001-sweeper`): every 5min prune Done
    // background-shell entries past the 1h retention, releasing
    // their stdout/stderr buffers (unbounded growth in a
    // long-lived daemon). Daemon-only; GUI stays timer-free.
    server::spawn_shell_sweeper(&state);

    // F2 定时任务调度循环(2026-08-28, task
    // `08-28-f2-scheduled-tasks`): 30s tick 单一扫描算法,到点任务经
    // chat_inner 注入目标 session。**只有这里装配** —— GUI Full 模式
    // 零 timer 硬约束保持(可建任务但不会触发,Settings 面板注明);
    // 停机链在 shutdown_signal 的 tunnel stop 之后 cancel
    // `state.scheduler_cancel`。首 tick 立即 = 启动补偿评估(D4)。
    server::spawn_task_scheduler(&state);

    // S2 tunnel client (2026-08-11, task `08-11-tunnel-client`,design §2.4
    // / §4.2 P1-1 修订):**只有这里** spawn tunnel —— lib.rs 零改动,
    // Tauri GUI(Thin/Full)持有空壳 manager 但从不 start()。双进程同
    // node_id 会互踢 flapping,故绝不能双 spawn。
    //
    // 不配 remote_url(或 key 缺失)→ `load_remote_config` 返 None →
    // `set_config(None)` 不 spawn 任何 task → daemon 行为与现状完全一致
    // (本地功能零依赖 remote,硬约束 2)。
    state.tunnel_manager.set_local_port(port);
    state.tunnel_manager.start();
    match everlasting_lib::daemon::tunnel::config::load_remote_config(&state.db).await {
        Ok(Some(cfg)) => {
            tracing::info!(
                target: everlasting_lib::daemon::tunnel::TUNNEL_TARGET,
                remote = %cfg.remote_url,
                node_id = %cfg.node_id,
                "remote tunnel configured, starting tunnel client"
            );
            state.tunnel_manager.set_config(Some(cfg));
        }
        Ok(None) => {
            tracing::debug!(
                "no remote_url configured; tunnel client not started (pure local mode)"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "load_remote_config failed; tunnel client not started (pure local mode)"
            );
        }
    }

    match server::serve_daemon(state, port).await {
        Ok(()) => {
            tracing::info!("everlasting-daemon exited cleanly");
            ExitCode::from(0)
        }
        Err(e) => {
            tracing::error!(error = %e, "everlasting-daemon exited with error");
            eprintln!("daemon 启动失败: {}", e);
            ExitCode::from(1)
        }
    }
}

/// 是否以 GUI sidecar 身份运行。GUI 的 `sidecar.rs::spawn_and_manage`
/// spawn 时注入 `EVERLASTING_SIDECAR=1`;standalone(`daemon.sh` /
/// `cargo run` / CI)不设 → orphan-guard(PDEATHSIG)关闭,后台 daemon
/// 不再被短命父进程(daemon.sh 脚本)的退出连坐(2026-08-24 回归修复)。
fn is_sidecar_mode() -> bool {
    sidecar_flag_from_env(std::env::var_os("EVERLASTING_SIDECAR"))
}

/// `is_sidecar_mode` 的纯函数核心。拆出来单测:直接测 env 读取需要
/// set_var,并行测试下有竞态。语义与 `EVERLASTING_GUI_FULL_STATE` 对齐:
/// 仅字面量 `"1"` 生效(非 "1" 一律当 standalone,宁可少绑也不误杀)。
fn sidecar_flag_from_env(v: Option<std::ffi::OsString>) -> bool {
    v.as_deref() == Some(std::ffi::OsStr::new("1"))
}

/// Parse `--port <N>` from CLI args. Falls back to
/// `EVERLASTING_DAEMON_PORT` env, then the default 7456.
fn parse_port_from_args() -> u16 {
    let matches = build_cli().get_matches();
    let cli_port = matches
        .get_one::<String>("port")
        .and_then(|s| s.parse::<u16>().ok());
    let env_port = std::env::var("EVERLASTING_DAEMON_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok());
    server::resolve_port(cli_port, env_port)
}

/// P2.4 D2.3: parse `--data-dir <PATH>` from CLI args. Returns `None`
/// when absent (caller falls back to `resolve_data_dir`). The GUI
/// sidecar passes the Tauri-resolved `app_data_dir` so the daemon opens
/// the same SQLite file the GUI would have (P2.1 path consistency).
fn parse_data_dir_from_args() -> Option<std::path::PathBuf> {
    let matches = build_cli().get_matches();
    matches
        .get_one::<String>("data-dir")
        .map(std::path::PathBuf::from)
}

/// Build the shared clap `Command` (so `--port` and `--data-dir`
/// parsing stay in lockstep — both read the same `get_matches` pass).
fn build_cli() -> Command {
    Command::new("everlasting-daemon")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Everlasting agent daemon (Phase 2.2). Hosts the agent core over HTTP/SSE.")
        .arg(
            Arg::new("port")
                .long("port")
                .help("Port to listen on (default: 7456, or $EVERLASTING_DAEMON_PORT)")
                .action(ArgAction::Set),
        )
        .arg(
            Arg::new("data-dir")
                .long("data-dir")
                .help(
                    "Data directory for the SQLite store + worktrees. \
                     Defaults to the platform app-data dir. The GUI \
                     sidecar passes the Tauri-resolved app_data_dir so \
                     daemon + GUI share one SQLite file.",
                )
                .action(ArgAction::Set),
        )
}

/// Resolve the daemon's data directory. Uses the same platform base
/// + identifier convention as Tauri's `app.path().app_data_dir()`
/// (which is `dirs::data_dir().join(config.identifier)`):
/// - `$XDG_DATA_HOME/<identifier>` on Linux (or `~/.local/share/<identifier>`)
/// - `~/Library/Application Support/<identifier>` on macOS
/// - `%APPDATA%\<identifier>` on Windows
///
/// `<identifier>` is injected at compile time by `build.rs`
/// (`EVERLASTING_APP_IDENTIFIER`, read from `tauri.conf.json`) so it
/// stays in lockstep with the GUI's `app_data_dir()` — the P2.1
/// path-consistency invariant (daemon + GUI read/write the same
/// SQLite file). Previously this joined a hardcoded `"everlasting"`,
/// which diverged from the GUI's identifier-based path
/// (`dev.everlasting.app`) and caused a standalone daemon run to
/// open an empty db separate from the GUI's.
///
/// P2.4 sidecar mode is unaffected (the GUI passes the exact
/// `app_data_dir` via `--data-dir`, overriding this fallback).
fn resolve_data_dir() -> std::path::PathBuf {
    // Compile-time constant injected by build.rs from tauri.conf.json.
    // `env!` (not `std::env::var`) so it's baked into the binary and
    // can't drift from the GUI's config.identifier at runtime.
    let identifier = env!("EVERLASTING_APP_IDENTIFIER");
    if let Some(dir) = dirs::data_dir() {
        dir.join(identifier)
    } else {
        // Fallback: cwd (defensive — should never happen on a
        // well-formed platform). Suffix with the identifier so even
        // this degenerate path stays GUI-consistent.
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(identifier)
    }
}

/// Q1 port-conflict probe. GET `http://localhost:{port}/api/v1/health`.
/// Returns `Ok(())` if the port is free (connection refused), `Err`
/// with a Chinese user-facing message if something is already
/// squatting. The layered "same daemon_version → reuse" check is
/// deferred to P2.4 (needs the daemon_id handshake).
async fn probe_port_conflict(port: u16) -> Result<(), String> {
    let url = format!("http://localhost:{}/api/v1/health", port);
    // Short timeout — if the port is free, connect fails instantly;
    // if squatted by an unresponsive process, we don't hang startup.
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "port-conflict probe: client build failed; skipping probe");
            return Ok(());
        }
    };
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            Err(format!(
                "端口 {} 已被其他进程占用(收到 HTTP {} 响应)。\n\
                 everlasting-daemon 不会自动跳端口(Q1 决议:避免多 daemon 数据分裂)。\n\
                 解决方法:\n  1) 确认端口 {} 上跑的是另一个 everlasting-daemon 实例并复用它;\n  \
                 2) 或换端口:`everlasting-daemon --port <其他端口>`。",
                port, status, port,
            ))
        }
        Err(e) => {
            tracing::debug!(error = %e, "port-conflict probe: connection refused (port is free)");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_data_dir, sidecar_flag_from_env};
    use std::ffi::OsStr;

    /// 2026-08-24 orphan-guard gate:仅 `EVERLASTING_SIDECAR=1`(字面量
    /// "1")按 sidecar 对待 → 设 PDEATHSIG;缺失/空/"0"/"true" 都是
    /// standalone → 不绑父进程生命周期,`daemon.sh bg` 的 daemon 不再
    /// 随 daemon.sh 脚本退出被内核 SIGTERM 连坐。
    #[test]
    fn sidecar_flag_from_env_requires_literal_one() {
        assert!(sidecar_flag_from_env(Some("1".into())));
        assert!(!sidecar_flag_from_env(None));
        assert!(!sidecar_flag_from_env(Some("".into())));
        assert!(!sidecar_flag_from_env(Some("0".into())));
        assert!(!sidecar_flag_from_env(Some("true".into())));
    }

    /// P2.1 path-consistency invariant: the daemon's data dir MUST end
    /// with the same identifier the GUI's `app_data_dir()` uses
    /// (`config.identifier` from tauri.conf.json, baked in here via
    /// `EVERLASTING_APP_IDENTIFIER`). A standalone daemon run opening a
    /// different subdirectory than the GUI would silently split the
    /// SQLite store — this test catches that regression at the
    /// identifier-join level (platform data_dir base is host-dependent,
    /// so only the trailing component is asserted).
    #[test]
    fn resolve_data_dir_ends_with_app_identifier() {
        let dir = resolve_data_dir();
        let expected = OsStr::new(env!("EVERLASTING_APP_IDENTIFIER"));
        assert_eq!(
            dir.file_name(),
            Some(expected),
            "resolve_data_dir() should end with the bundle identifier \
             ({}), got {} — GUI/daemon db will split",
            env!("EVERLASTING_APP_IDENTIFIER"),
            dir.display()
        );
    }

    /// The identifier must not be the old hardcoded `"everlasting"`
    /// (the bug). Guards against a build.rs regression that fails to
    /// read tauri.conf.json and falls back to a wrong default.
    #[test]
    fn resolve_data_dir_not_legacy_hardcoded() {
        let dir = resolve_data_dir();
        assert_ne!(
            dir.file_name(),
            Some(OsStr::new("everlasting")),
            "resolve_data_dir() still resolves to legacy 'everlasting' \
             subdir — EVERLASTING_APP_IDENTIFIER injection broken"
        );
    }
}
