# DB 备份(VACUUM INTO)+ daemon 日志轮转

## Goal

收口两条 P1/P2 耐久性债([DEBT.md](../../reviews/DEBT.md)):

- **RULE-DB-001(P1)**: 全部数据(会话/消息/自主记忆/审计/token 用量)仅单一 `everlasting.db`,零备份机制;一次坏盘/误删/migration 写坏即数据归零。
- **RULE-DAEMON-001(P2)**: daemon 日志写死 `/tmp/everlasting-daemon.log`,重启即覆盖(排障永远缺"出事前"的日志)且单文件无限增长无轮转。

## Requirements

### R1 DB 快照备份(RULE-DB-001)

- daemon 启动时(`load_daemon_state` 成功后)做一次全库快照,之后每 24h 定时一次(长驻 daemon 不重启也有新鲜备份)。
- 用 SQLite `VACUUM INTO`(在线备份,WAL 模式下与并发读写安全;产出紧凑副本而非逐页拷贝)。
- 备份落 `<data_dir>/backups/everlasting-YYYYMMDD-HHMMSS.db`(**跟随 `--data-dir`**,不自作主张另立 state 目录——sidecar 传 GUI `app_data_dir` 时备份与 DB 同根,P2.1 路径一致性不变式自然成立)。
- 保留最近 **7** 份,超出按文件名(时间戳)最旧先删。
- 备份失败**不阻塞 daemon 启动/运行**:tracing `warn!` 后继续(备份是保险层,不能反过来变成可用性风险)。
- GUI Full 模式(in-process Tauri 逃生舱)不备份——备份挂在 daemon bin 入口,GUI 主进程不引入定时任务。

### R2 daemon 日志轮转(RULE-DAEMON-001)

- 日志文件从 `/tmp` 移到 `~/.local/state/dev.everlasting.app/daemon.log`(XDG state 目录;`XDG_STATE_HOME` 存在时优先)。
- **追加写(`>>`)**,不再重启即覆盖。
- 启动时大小轮转:`> 10 MiB` 则滚动保留 3 代(`daemon.log` → `.1` → `.2` → `.3`,最旧删除)。
- Rust 侧 stdout/stderr tracing **不动**(零新依赖,前台模式/`daemon.sh start` 直接看终端的行为不变;sidecar 日志归 GUI 进程管,不在本任务范围)。
- `daemon.sh logs` 跟随新路径;`bg` 启动失败时的 `tail -20` 排障提示同路径。

## Acceptance Criteria

- [x] AC1: 临时 data_dir 建池写数据 → 跑备份 → 产物存在、能用独立 SqlitePool 打开、行数与源库一致。
- [x] AC2: 同一秒内两次备份不冲突(文件名碰撞有后缀/重试,不 panic 不覆盖)。
- [x] AC3: 预置 9 个备份文件跑 prune → 只剩最新 7 个,删的是最旧 2 个。
- [x] AC4: daemon 启动日志含一次 backup 成功记录(path + 耗时);备份失败路径(目录只读等)只 warn 不影响启动。
- [x] AC5: `bash -n scripts/daemon.sh` 通过;`bg` 两次连续启动日志内容追加不丢失;>10MiB 触发滚动且最多 3 代。
- [x] AC6: `cargo test --lib`(everlasting)全绿 + `cargo fmt --check` 干净;`turn-smoke.sh` 不涉及(不动 agent loop)。
- [x] AC7: DEBT.md 删除 RULE-DB-001 / RULE-DAEMON-001 两条(闭合),优先级分布表同步。

## Notes

- 源评估:2026-08-24 harness 缺口评估会话(非 formal review),债项登记见 DEBT.md 对应条目。
- 规模预期 ~80 行(Rust ~50 + shell ~30),零新依赖。
