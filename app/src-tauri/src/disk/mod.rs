//! F3 磁盘治理(2026-09-03, task `09-03-f3-disk-governance`):agent
//! 后台作业磁盘数据的自动限损层。
//!
//! 摸底结论(research/disk-usage-audit.md):长期增长大头是辅助数据
//! (备份 / webview 缓存 / 日志),而机制缺口最严重的是 worktree 与
//! outputs spill——「孤儿」(DB 行没了、目录还在)此前无人回收。
//!
//! 结构(design §1):
//! - [`governor`] 回收函数族(`_inner` 化,返回回收摘要):worker
//!   worktree sweep(P0-a 装配修复,本体零改动)、孤儿 session
//!   worktree(P1-a)、outputs 孤儿桶 + 按龄(P1-b)、备份预算自适应
//!   prune(实现在 `db/backup.rs`)。
//! - [`log_rotation`] daemon 日志的进程内文件 sink + 大小轮转
//!   (P0-b,PR2;daemon bin 的 tracing 文件 layer,`daemon.sh` 的
//!   脚本轮转由此退役)。
//! - [`webkit_cache`] GUI 启动期的 WebKitCache 阈值清理(P2-b,PR4;
//!   **装配在 lib.rs setup 公共区**——Thin/Full 都过,勿仿 Full 分支
//!   装配点,见该模块头注的陷阱警示)。
//! - 双入口共享同一函数族:daemon bin 每日节拍
//!   ([`governor::spawn_disk_governor`],首拍延迟 5 分钟)与设置面
//!   「立即清理」IPC(PR3)。GUI Full 逃生模式只在启动期跑一次性
//!   pass(lib.rs)——「GUI 主进程零 timer task」硬约束保持;Thin
//!   场景由 daemon bin 节拍兜底。
//! - kill-switch `disk_governor_enabled`(app_config KV,fail-open:
//!   仅字面 `"false"` 关)只管自动节拍;手动清理不受限(AC9)。
//!
//! 日志轮转(P0-b)与 WebKitCache 清理(P2-b)分别在本模块的
//! [`log_rotation`] 与 [`webkit_cache`](后者 GUI 启动时机,公共区装配,
//! 陷阱警示见其头注)。

pub mod governor;
pub mod log_rotation;
pub mod webkit_cache;
