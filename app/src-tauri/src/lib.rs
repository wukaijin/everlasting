// clippy 1.96 的 `doc_lazy_continuation` / `doc_overindented_list_items`
// 对本 crate 里大量中/英文混排的 `///` 自然语言段落会误判成 markdown
// 列表续行(建议在纯文本续写处加缩进,会破坏语义)。这些 lint 不区分
// "列表项续行" 与 "普通段落换行",对本项目的文档注释基本是噪音 —— CI
// 也只跑 `cargo test --lib` + `cargo fmt`,不跑 clippy。crate 级 allow
// 抑制这两条,其余 clippy lint 仍保持默认 deny-on-check(见本地手动跑法)。
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]

//! Everlasting library crate — module declarations only.
//!
//! de-Tauri（2026-09-30, task `09-30-de-tauri`）：GUI bin（原
//! `src/main.rs` + `lib.rs::run()` 的 Tauri Builder）与 sidecar
//! 生命周期（`src/sidecar.rs`）已整链移除，daemon + web 是唯一
//! 形态。本 crate 现在只有一个 bin：`everlasting-daemon`（axum
//! HTTP + SSE + MCP），与 lib 共享同一 agent core。
//!
//! The actual logic lives in:
//!
//! - [`state`] — `AppState`, `CancellationGuard`, event payloads,
//!   `ProviderCatalog`.
//! - [`commands`] — the `*_inner` function family (the daemon
//!   routes' implementation surface), grouped by concern.
//! - [`agent`] — the chat handler + spawned agent loop,
//!   `resolve_chat_provider` + `PreFlightError`, the system prompt
//!   builder, the thinking-block accumulator, and the helper
//!   utilities.
//!
//! `init_tracing` lived in the removed GUI `main.rs`; the daemon
//! bin owns its own tracing setup.

mod agent;
mod attachments;
mod background_shell;
mod commands;
mod crypto;
mod db;
// B9+ D4 (2026-07-13): hand-written unified-diff parser + hunk
// applier. Zero new dependency (TECH §1.4). Lives at top level (not
// under `tools/`) because it's the structural backbone of the
// `apply_ui_diff` IPC, not a standalone tool — the IPC handler does
// the I/O, this module owns only the textual transformation.
mod diff_apply;
// Phase 2.2 (2026-07-21, task `07-20-remote-access-daemon-split`):
// HTTP daemon stack — `pub` so the `everlasting-daemon` bin target
// (a separate crate that depends on this library) can reach
// `everlasting_lib::daemon::serve_daemon`. Inside the daemon module,
// references to other top-level modules use `crate::xxx` paths, which
// work because the daemon module is itself part of the lib crate
// (the private `mod commands` etc. are visible to sibling modules
// inside the same crate, just not to external consumers).
pub mod daemon;
// F3 磁盘治理 (2026-09-03, task `09-03-f3-disk-governance`): 回收函数族
// (worker sweep / 孤儿 session worktree / outputs / 备份 prune)+ daemon
// 每日节拍 + 日志进程内轮转 writer。`pub`:daemon bin 需要直接装配
// `log_rotation::RotatingFileWriter` 类型(tracing layer 的类型参数,
// 无法像函数那样经 server.rs 包装)。
pub mod disk;
// N9 性能基准 (2026-09-19, task `09-19-n9-perf-benchmark`): benches(独立
// crate)的唯一入口面,再导出 cfg(test) 树内的构造件与 DB 基建。默认构建
// (无 bench feature)本模块零编译;CI 编译门命令
// `cargo check -p everlasting --lib --features bench --benches`。
#[cfg(feature = "bench")]
pub mod bench_api;
mod error;
mod files;
mod git;
mod llm;
mod memory;
mod projects;
// /proc TCP listener 归因 (2026-09-21, task `09-21-sandbox-net-bindonly`
// R5): 就绪探测的 PGID 归因机器;后续 AllowAll capability token 的血统
// 拒授复用同一实现,故独立成模块。
mod procnet;
mod resource_loader;
// P3b 执行期沙盒 (2026-08-31, task `08-31-a2-p3b-sandbox-executor`):
// Landlock + seccomp 执行器,ReadOnly 档 shell 命令的限损层。crate 私有:
// 消费者是 tools/shell.rs、tools/run_background_shell.rs (PR2) 与
// commands/config.rs (设置面读出口)。
mod sandbox;
// F2 定时任务 (2026-08-28, task `08-28-f2-scheduled-tasks`): 调度内核
// (preset 档位纯函数 + 30s tick 单一扫描算法 + TaskOrigin 来源标记)。
// crate 私有:唯一消费者是 daemon/server.rs 的 spawn_task_scheduler
// wrapper、agent 的 origin 载体链与本模块测试。
mod scheduler;
mod skill;
mod state;
mod tools;
