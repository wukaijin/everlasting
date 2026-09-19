<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->


## Pattern: daemon 运维伴生物(备份 task + 日志文件,RULE-DAEMON-001 闭合 2026-08-24)

**DB 备份 task**:`bin/everlasting-daemon.rs` 在 `load_daemon_state` 成功
后调用 `server::spawn_backup_task(&state, &data_dir)`(detached,不 join,
不阻塞 serve)。启动即快照一次 + 每 24h 一次,`VACUUM INTO` 到
`<data_dir>/backups/`;失败仅 `warn!` 继续服务。保留策略(2026-09-03 F3
磁盘治理起,`db/backup.rs::prune_backups` 原地改):**大小预算自适应** ——
从新到旧保留,累计超 200 MiB(env `EVERLASTING_BACKUP_BUDGET_MB` 覆盖)
即停,**至少 2 份**(超预算也保留)、最多仍 7 份。契约细节见
[database-guidelines "DB 快照备份"](../database-guidelines.md) +
[disk-governance](disk-governance.md)。
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

**disk governor**(F3 磁盘治理,2026-09-03,任务
`09-03-f3-disk-governance`):第 5 个 spawn,bin 在 scheduler 旁调
`server::spawn_disk_governor(&state)`——24h interval 首拍延迟 5 分钟,
跑回收函数族(worker sweep + 孤儿 worktree + outputs + 备份 prune);
**协作停机变体**(`AppState.disk_governor_cancel`,照 scheduler 先例,
`shutdown_signal` 同段 cancel)。GUI Full 逃生模式只跑一次性启动 pass
(`run_governor_pass_once`,**有意的** Full 分支装配,Thin 场景由本节拍
兜底);WebKitCache 清理是唯一必须装在 setup 公共区(mode resolve 后、
Thin 早退 return 前)的,见 [disk-governance](disk-governance.md) 陷阱段。

**日志文件**(2026-09-03 F3 P0-b 起改为**进程内 appender**,
`daemon.sh` 脚本重定向与轮转退役):

- 落盘方 = daemon bin 自带 tracing 文件 layer
  (`disk/log_rotation.rs::RotatingFileWriter`,零依赖手写 MakeWriter;
  终端 layer 显式 stdout 保留)。路径
  `${XDG_STATE_HOME:-$HOME/.local/state}/dev.everlasting.app/daemon.log`
  —— 排障先看这里,**不再是 `/tmp`**(重启即覆盖的老坑已闭合)。
- **运行期轮转**(修复旧「启动前检查、运行期无限涨」缺口,实测单份曾
  涨 29 MiB):write 前 >10 MiB 滚动保 3 代(`daemon.log` → `.1` → `.2`
  → `.3`,最旧删),稳态上限 ≈ 4×10 MiB。文件 layer `.with_ansi(false)`
  (旧脚本重定向曾把 ANSI 转义码写进日志)。
- 旧脚本行为对照:`daemon.sh` bg 分支输出改 `>/dev/null 2>&1`(双写者
  行交错 + 轮转打架);`rotate_log()` 退役(旧代 `.1/.2/.3` 由 Rust 滚动
  自然覆盖消化);`logs` 子命令(`tail -f` 同路径)不变;`STATE_DIR` 的
  `${HOME:-/tmp}` 降级仍保留(cron/systemd 裸环境 `$HOME` 未设,
  `set -u` 杀脚本)。
- **sidecar / 前台模式新收益**:daemon bin 自带文件 layer,打包 GUI 的
  daemon 日志首次落盘;前台模式终端输出照旧(终端 layer 并行)。
- 打开/滚动失败降级为终端-only no-op,绝不 panic(daemon 必须能起来)。

---
