# F3 磁盘治理摸底:现状审计 + 实测占用

> 调研时间 2026-09-03。代码侧由 Explore 子代理完成(全量 file:line 见下),实测为 `du` 直测本机(`~/.local/share/dev.everlasting.app`,root 环境)。

## 1. 实测占用(本机,2026-09-03)

| 消费点 | 实测 | 说明 |
|---|---|---|
| `backups/` | **175M** | DB 备份 7 份保留;单份已涨到 ~26M(DB 本体大小),超出 backup.rs:38 注释的"~15MB×7≈105MB"估算 |
| `WebKitCache/` | **136M** | Tauri webview 缓存,项目代码零治理 |
| `$XDG_STATE_HOME/.../daemon.log{,.1,.2,.3}` | **59M** | 295K + 19M + 29M + 11M。轮转只在 daemon.sh bg 启动前检查(>10MiB 滚动),**运行期不轮转**——`.2` 单份 29M 实证连续运行期可无限涨 |
| `everlasting.db` | 26M(+shm 32K, wal 0) | 主库,持续追加 |
| `outputs/` | 36K | spill,8 个 session 目录 |
| `worktrees/` | 20K | 本机几乎没用 worker;重仓库场景每 worker 一份全量 checkout |
| `attachments/` | — | 本机未见(无粘贴图),≤5MiB/张不去重 |

**结论:实测大头是"辅助数据"(备份+webview缓存+日志 ≈ 370M),比 DB 本体大一个数量级;worktree 在本机不是大头但机制缺口最严重。**

## 2. 现有治理机制(代码侧)

### 2.1 worktree —— 部分,且 sweep 宿主断链

- 布局:`<data_dir>/worktrees/<project_uuid>/<session_id>`(session,`naming.rs:27-29`);`.../worker/<run_id>`(worker,`naming.rs:53-59`)
- **worker sweep 已有**:`sweep_stale_worker_worktrees`(`app/src-tauri/src/git/worktree/sweep.rs:125`)——mtime 超 `cleanup_period_days`(默认 7 天,`sweep.rs:64`;env `EVERLASTING_CLEANUP_PERIOD_DAYS` 可覆盖)销毁;locked(活跃)跳过;孤儿 best-effort destroy
- **断链**:`sweep_stale_workers` 只在 GUI Full 模式 setup spawn(`lib.rs:94-138, 231-235`)。**daemon bin(默认 Thin 模式真正宿主)不跑 sweep**(`bin/everlasting-daemon.rs` / `daemon/server.rs` 无调用)
- session 删除:仅 `worktree_state == Active` 时 destroy(`commands/sessions.rs:442-468` → `lifecycle.rs:183`);Detached 有意保留
- **session 级孤儿(目录在、DB 行没了)无任何回收**:sweep 只遍历 `worker/` 子目录(`sweep.rs:131-134`)
- worker 无变更即毁(`dispatch/finalize.rs:350-372`);有变更留分支待 merge/discard
- 隐性:`.git` 对象库中 `worker/<run_id>`、`session/<id>` 分支累积,仅 destroy 时顺带删

### 2.2 spill/outputs —— 仅 session 删除单一路径

- 位置:`<data_dir>/outputs/<session_id>/<uuid>.txt`(`tools/tool_output.rs:62-64, 161-171`),阈值 30KB(`tool_output.rs:47`)
- sweep = `remove_dir_all`(`tool_output.rs:175-191`),只在 `delete_session_inner`(`commands/sessions.rs:370-378`)+ legacy pre-C6 双路径(`tools/shell.rs:780`)
- **无年龄/总量回收;`outputs/_no_session`(`tool_output.rs:59`)永远没人扫;session 永不删则 outputs 永不删**

### 2.3 attachments —— 随 session 删除,无年龄回收

- `<data_dir>/attachments/<session_id>/<32-hex>.<ext>`(`attachments.rs:47-55`);≤5MiB/张(`:23`)png/jpeg/webp 白名单(`:27`)
- `delete_session_attachments`(`attachments.rs:200-211`)接在 `delete_session_inner`(`commands/sessions.rs:440`)
- 内容哈希去重明确砍出 MVP(`attachments.rs:124-126`);@文件注入不落副本(每轮原位置读盘)

### 2.4 日志 —— Rust 侧零文件 sink;唯一轮转是 daemon.sh 且只在启动时

- GUI `init_tracing`(`main.rs:41-44`)与 daemon bin(`bin/everlasting-daemon.rs:53-58`)均未配置 `.with_writer()`——走 tracing-subscriber fmt **默认 writer = stdout**(0.3.23 源码 `fmt/mod.rs:8` "logging them to stdout";早期调研误记 stderr,外部评审 2026-09-03 指正;`daemon.sh` 的 `>> log 2>&1` 与 sidecar 双管道均两流全抓,stdout/stderr 对行为等价);sidecar 抽回 GUI tracing(`sidecar.rs:191-223`)——打包 GUI 日志完全不落盘
- 唯一轮转:`scripts/daemon.sh` RULE-DAEMON-001(2026-08-24):bg 启动前 >10MiB 滚动保留 3 代(合计理论 ~40MiB 封顶);**运行期不轮转;start(前台)/sidecar 不经此脚本**

### 2.5 SQLite —— WAL 默认参数;无 VACUUM;备份有保留策略

- `init_pool`(`db/migrations/pool.rs:48-56`):WAL + busy_timeout 5000 + foreign_keys ON;无 wal_checkpoint 配置(默认 1000 页 auto)
- 主库无 VACUUM/大小治理;`VACUUM INTO` 仅备份用(`db/backup.rs:83`)
- 备份:startup + 每 24h(`daemon/server.rs:72-118`, `bin/everlasting-daemon.rs:153-158`),`KEEP_BACKUPS=7`(`backup.rs:38`)+ `prune_backups`(`:94`);GUI Full 刻意不备份(`backup.rs:8-10`)

## 3. 磁盘消费点全景表

| 消费点 | 增长特征 | 现有治理 | 缺口定级 |
|---|---|---|---|
| worker worktree | 每次派发全量 checkout | 7 天 mtime sweep(**仅 GUI Full**) | **P0:sweep 宿主断链** |
| session worktree | 每会话全量 checkout | session 删除(仅 Active 态) | P1:孤儿无回收 |
| outputs spill | 每 >30KB 工具输出一份 | 仅随 session 删除 | P1:无年龄/总量回收;`_no_session` 无人扫 |
| daemon 日志 | 连续运行期无限涨 | 启动前检查(单份>10MiB 滚动,保留 3 旧代,合计 4 份≈40MiB 封顶) | **P0:运行期不轮转**(29M 实证) |
| DB 备份 | 每日 +26M 级 | 7 份保留 + prune | P2:保留策略可按大小自适应 |
| WebKitCache | webview 自管 | 无 | P2:Tauri 层,治理手段不同 |
| attachments | ≤5MiB/张 | 随 session 删除 | P2:基本生命周期已有 |
| everlasting.db + WAL | 持续追加 | WAL 默认 checkpoint | P2:26M 暂不紧张 |

## 4. F3 ROADMAP 原始范围对照

ROADMAP 第三档 F3 行:"context/token 治理已落地;agent loop 并发上限已落地;**余下:进程 / 内存、磁盘(worktree / attachments / 日志),与 F1 反压联动**。边界:不含 Provider API 限流。"

→ "进程 / 内存"与磁盘正交;"F1 反压联动"(磁盘满→agent loop 反压)是复杂机制。两者是否进本任务 MVP 待用户裁定。
