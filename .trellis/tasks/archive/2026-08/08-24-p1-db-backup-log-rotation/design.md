# Design — DB 备份 + daemon 日志轮转

## WP1 DB 快照备份(Rust)

### 落点与结构

新文件 `app/src-tauri/src/db/backup.rs`:

```
pub fn backup_dir(data_dir: &Path) -> PathBuf          // data_dir.join("backups")
pub async fn backup_database(db: &SqlitePool, dir: &Path) -> std::io::Result<PathBuf>
    // 1. create_dir_all(dir)
    // 2. 文件名 everlasting-{local now: %Y%m%d-%H%M%S}.db;已存在则 -2/-3… 后缀(AC2)
    // 3. sqlx::query(&format!("VACUUM INTO '{}'", escaped_path)).execute(db)
    //    — 路径来自我们自己拼的 timestamp,不含单引号;仍走 escape 防御
pub fn prune_backups(dir: &Path, keep: usize) -> io::Result<Vec<PathBuf>>
    // 列 dir 下 everlasting-*.db,文件名字典序 = 时间序,删最旧,返回被删列表
```

- **挂点**:`bin/everlasting-daemon.rs` main,`load_daemon_state` 成功后 spawn tokio task:

  ```
  启动即备份一次 → 循环 tokio::time::interval(24h) 再备份
  每次成功 tracing::info!(path, elapsed_ms, size_bytes)
  失败 tracing::warn!(error) 继续 —— 备份 task 与 axum server 并行,不 join
  ```

- 归属 `db/` 模块(操作 SqlitePool,与 migrations/pool.rs 同层),daemon 只是调用方;将来 GUI/CLI 想备份直接复用。

### 关键决策

| # | 决策 | 理由 |
|---|------|------|
| D1 | `VACUUM INTO` 而非逐页 copy / `sqlite3 .backup` | sqlx 原生可用;WAL 下在线安全(读事务);产出 VACUUM 紧凑库(顺带治碎片);不需要 shellout sqlite3 CLI |
| D2 | 备份目录跟 `data_dir` 而非 DEBT 草案的 `~/.local/state` | sidecar 传 GUI app_data_dir 时备份与 DB 同根,`--data-dir` 语义一致;跨平台不依赖 Linux-only 的 `dirs::state_dir()` |
| D3 | 时间戳文件名 + 后缀重试,不做锁 | 24h 间隔 + 启动一次,同秒碰撞只有测试会触发;文件名排序天然可 prune |
| D4 | 失败不重试(等下一个周期) | 简单;周期本身就是重试 |
| D5 | 常量 `KEEP_BACKUPS = 7`,不暴露配置 | 15MB×7≈105MB 无需调参;YAGNI |

### 测试(db/backup.rs `#[cfg(test)]`)

- `backup_creates_valid_copy`:tempdir 建池(复用现有 test pool 模式)+ 插 session → backup → 用新 SqlitePool 打开副本数行数(AC1)
- `backup_same_second_collision`:同 dir 跑两次 → 两文件都在、都非空(AC2)
- `prune_keeps_newest_n`:写 9 个假文件 → prune(7) → 剩 7 个、最旧 2 个被删(AC3)
- `prune_ignores_foreign_files`:`everlasting-*.db` glob 不误删其他文件

## WP2 daemon.sh 日志轮转(shell)

```bash
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/dev.everlasting.app"
LOG_FILE="$STATE_DIR/daemon.log"          # 替换原 /tmp 行

rotate_log() {                             # do_start(bg) 前调用
    [[ -f "$LOG_FILE" ]] || return 0
    local size; size=$(stat -c%s "$LOG_FILE" 2>/dev/null || echo 0)
    (( size <= 10 * 1024 * 1024 )) && return 0
    mkdir -p "$STATE_DIR"
    rm -f "$LOG_FILE.3"
    mv -f "$LOG_FILE.2" "$LOG_FILE.3"
    mv -f "$LOG_FILE.1" "$LOG_FILE.2"
    mv -f "$LOG_FILE"   "$LOG_FILE.1"
}
```

- `bg` 启动行 `> "$LOG_FILE"` 改 `>> "$LOG_FILE"`(覆盖 → 追加),前置 `mkdir -p "$STATE_DIR"` + `rotate_log`。
- `do_logs` / `do_start` 失败提示路径随 `LOG_FILE` 常量自动更新,零额外改动。
- **不动 Rust tracing**:前台模式照旧打终端;轮转职责单点在启动侧,运行中增长上限 ≈ 10MB + 3 代滚动(4×10MB 封顶)。

## 明确不做

- GUI main.rs 加 RollingFile(DEBT File 字段提到,但 GUI 是前台桌面进程,stdout 可见;保持零依赖)
- 备份压缩(gzip)/ 上传远端 / 恢复 IPC(先有快照,恢复走手工 `cp`,量级不值得产品化)
- 备份触发暴露成 REST 路由或配置项
