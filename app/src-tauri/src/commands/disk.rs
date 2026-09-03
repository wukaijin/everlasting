//! F3 磁盘治理 IPC(P3,2026-09-03, task `09-03-f3-disk-governance`
//! design §5):设置面「存储」分类的两个命令,照 `commands::
//! background_shells` 先例(`_inner` 业务函数 + `#[tauri::command]`
//! 薄包装)。
//!
//! - [`get_disk_usage`] — 各磁盘消费点的字节占用(db / backups /
//!   outputs / worktrees / attachments / 日志 / WebKitCache)+ 总量。
//!   实现 `spawn_blocking` + 手写递归 walk(复用 `disk::governor::
//!   inspect_dir`,照 `tools/glob.rs` 先例:不跟随 symlink、不可读
//!   条目跳过计 0,不拉 walkdir)。
//! - [`run_disk_cleanup`] — 手动「立即清理」:直调 PR1 的
//!   [`crate::disk::governor::run_governor_pass_inner`]。**不查
//!   kill-switch**(AC9:`diskGovernorEnabled=false` 时机停但手动
//!   清理仍可用);不含 WebKitCache(那是 PR4 的 GUI 启动时机)。
//!   每日节拍与手动按钮共享同一 `_inner` 族,零逻辑漂移。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::disk::governor::{self, DiskGovernorOutcome};
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// 单个磁盘消费点条目(wire: camelCase)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsageEntry {
    /// 稳定 id(测试锚点 + 未来特性消费;不随文案漂移)。
    pub key: String,
    /// 用户可读标签(设置面直接渲染)。
    pub label: String,
    pub bytes: u64,
}

/// `get_disk_usage` 响应:固定条目列表 + 总量(顶层扁平,camelCase)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsageReport {
    pub entries: Vec<DiskUsageEntry>,
    pub total_bytes: u64,
}

/// 条目小构造(避免 collect 里的样板)。
fn entry(key: &str, label: &str, bytes: u64) -> DiskUsageEntry {
    DiskUsageEntry {
        key: key.to_string(),
        label: label.to_string(),
        bytes,
    }
}

/// 一个目录的递归字节数(缺失 / 不可读 → 0;复用 governor 的
/// `inspect_dir` 保持与回收摘要同口径)。
fn dir_usage(path: &Path) -> u64 {
    governor::inspect_dir(path).0
}

/// 收集全部消费点条目。`logs_dir` 注入(XDG state 的应用目录,生产
/// 走 [`crate::disk::log_rotation::state_log_path`] 的父目录;单测传
/// tempdir,避免依赖宿主真实 env)。
fn collect_usage_entries(data_dir: &Path, logs_dir: Option<&Path>) -> Vec<DiskUsageEntry> {
    // db = 主库 + WAL + SHM 三文件(缺失计 0;SHM/WAL 常驻 daemon 在跑时)。
    let db_bytes = ["everlasting.db", "everlasting.db-wal", "everlasting.db-shm"]
        .iter()
        .map(|name| {
            std::fs::metadata(data_dir.join(name))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum();
    let logs_bytes = logs_dir.map(dir_usage).unwrap_or(0);
    vec![
        entry("db", "数据库(主库 + WAL)", db_bytes),
        entry(
            "backups",
            "数据库备份",
            dir_usage(&data_dir.join("backups")),
        ),
        entry(
            "outputs",
            "工具输出 spill",
            dir_usage(&data_dir.join("outputs")),
        ),
        entry(
            "worktrees",
            "Git worktrees",
            dir_usage(&data_dir.join("worktrees")),
        ),
        entry(
            "attachments",
            "图片附件",
            dir_usage(&data_dir.join("attachments")),
        ),
        entry("logs", "daemon 日志", logs_bytes),
        entry(
            "webkit_cache",
            "WebKit 缓存",
            dir_usage(&data_dir.join("WebKitCache")),
        ),
    ]
}

/// `get_disk_usage` 业务本体(Tauri / daemon 双入口)。
/// `spawn_blocking`:du 是同步递归 walk,重仓库 worktree 可能上万
/// 条目,别占 executor 线程(`files.rs` @-walker 同款先例)。
pub async fn get_disk_usage_inner(
    state: &Arc<AppState>,
) -> Result<DiskUsageReport, AppCommandError> {
    let data_dir: PathBuf = state.app_data_dir.clone();
    let logs_dir: Option<PathBuf> = crate::disk::log_rotation::state_log_path()
        .parent()
        .map(Path::to_path_buf);
    let entries =
        tokio::task::spawn_blocking(move || collect_usage_entries(&data_dir, logs_dir.as_deref()))
            .await
            .map_err(|e| {
                AppCommandError::new(
                    ErrorCategory::Server,
                    format!("磁盘占用统计任务异常退出: {e}"),
                )
            })?;
    let total_bytes = entries.iter().map(|e| e.bytes).sum();
    Ok(DiskUsageReport {
        entries,
        total_bytes,
    })
}

#[tauri::command]
pub async fn get_disk_usage(
    state: State<'_, Arc<AppState>>,
) -> Result<DiskUsageReport, AppCommandError> {
    get_disk_usage_inner(&state).await
}

/// `run_disk_cleanup` 业务本体:手动「立即清理」。直调
/// `run_governor_pass_inner`(四项:worker sweep / 孤儿 session
/// worktree / outputs / 备份 prune),**不查 kill-switch**(AC9 手动
/// 语义);返回 PR1 的 `DiskGovernorOutcome`(camelCase wire,前端
/// toast 逐项摘要直接消费)。
pub async fn run_disk_cleanup_inner(
    state: &Arc<AppState>,
) -> Result<DiskGovernorOutcome, AppCommandError> {
    Ok(governor::run_governor_pass_inner(&state.db, &state.app_data_dir).await)
}

#[tauri::command]
pub async fn run_disk_cleanup(
    state: State<'_, Arc<AppState>>,
) -> Result<DiskGovernorOutcome, AppCommandError> {
    run_disk_cleanup_inner(&state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    /// 全量条目:mock 目录布局 → 各条目字节数精确 + 总量 = 各项和 +
    /// key 集齐全。logs 目录注入 tempdir(不依赖宿主 XDG env)。
    #[tokio::test(flavor = "multi_thread")]
    async fn usage_reports_every_consumer_with_exact_bytes() {
        let data_dir = tempfile::tempdir().unwrap();
        let logs_dir = tempfile::tempdir().unwrap();

        // db 三件套(主库 + wal;shm 缺失 → 计 0)。
        std::fs::write(data_dir.path().join("everlasting.db"), vec![0u8; 1000]).unwrap();
        std::fs::write(data_dir.path().join("everlasting.db-wal"), vec![0u8; 24]).unwrap();
        // backups:两层嵌套。
        std::fs::create_dir_all(data_dir.path().join("backups/gen")).unwrap();
        std::fs::write(data_dir.path().join("backups/a.db"), vec![0u8; 500]).unwrap();
        std::fs::write(data_dir.path().join("backups/gen/b.db"), vec![0u8; 250]).unwrap();
        // outputs:session 桶。
        std::fs::create_dir_all(data_dir.path().join("outputs/s1")).unwrap();
        std::fs::write(data_dir.path().join("outputs/s1/x.txt"), vec![0u8; 40]).unwrap();
        // worktrees:空目录树(存在但 0 字节)。
        std::fs::create_dir_all(data_dir.path().join("worktrees/p1/s1")).unwrap();
        // attachments / WebKitCache。
        std::fs::create_dir_all(data_dir.path().join("attachments/s2")).unwrap();
        std::fs::write(data_dir.path().join("attachments/s2/img.png"), vec![0u8; 7]).unwrap();
        std::fs::create_dir_all(data_dir.path().join("WebKitCache/blob")).unwrap();
        std::fs::write(data_dir.path().join("WebKitCache/blob/f"), vec![0u8; 13]).unwrap();
        // 日志目录:两代文件。
        std::fs::write(logs_dir.path().join("daemon.log"), vec![0u8; 300]).unwrap();
        std::fs::write(logs_dir.path().join("daemon.log.1"), vec![0u8; 60]).unwrap();

        let entries = collect_usage_entries(data_dir.path(), Some(logs_dir.path()));
        let by_key = |k: &str| entries.iter().find(|e| e.key == k).unwrap().bytes;
        assert_eq!(by_key("db"), 1024, "db = 主库 + wal(shm 缺失计 0)");
        assert_eq!(by_key("backups"), 750, "recursive walk 累计嵌套目录");
        assert_eq!(by_key("outputs"), 40);
        assert_eq!(by_key("worktrees"), 0, "empty tree counts 0");
        assert_eq!(by_key("attachments"), 7);
        assert_eq!(by_key("logs"), 360);
        assert_eq!(by_key("webkit_cache"), 13);
        assert_eq!(entries.len(), 7, "固定七个消费点条目(顺序 = 渲染顺序)");
        let total: u64 = entries.iter().map(|e| e.bytes).sum();
        assert_eq!(total, 1024 + 750 + 40 + 0 + 7 + 360 + 13);
    }

    /// 全缺失(data_dir / logs_dir 尚不存在)→ 全部条目在场、字节 0,
    /// 不 panic(新装机首开的常态)。
    #[test]
    fn usage_tolerates_missing_dirs() {
        let data_dir = tempfile::tempdir().unwrap();
        let entries = collect_usage_entries(data_dir.path(), None);
        assert_eq!(entries.len(), 7);
        assert!(entries.iter().all(|e| e.bytes == 0));
    }

    /// 消费点路径被普通文件占用(read_dir → ENOTDIR,与「不可读目录」
    /// 同一条 Err 跳过分支;root 环境下 chmod 无意义,用形态冲突构造)
    /// → 计 0 不 panic。
    #[test]
    fn usage_counts_zero_when_consumer_path_is_a_regular_file() {
        let data_dir = tempfile::tempdir().unwrap();
        std::fs::write(data_dir.path().join("outputs"), b"not a dir").unwrap();
        let entries = collect_usage_entries(data_dir.path(), None);
        let outputs = entries.iter().find(|e| e.key == "outputs").unwrap();
        assert_eq!(outputs.bytes, 0, "ENOTDIR path counts 0 without panic");
    }

    /// `run_disk_cleanup_inner`:空 data_dir 上的 no-op pass → Ok +
    /// 全零摘要(手动入口不炸;真实回收行为由 governor 套件锁定)。
    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_on_fresh_state_returns_zero_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let outcome = run_disk_cleanup_inner(&state).await.unwrap();
        assert_eq!(outcome.worker_worktrees.items, 0);
        assert_eq!(outcome.orphan_session_worktrees.items, 0);
        assert_eq!(outcome.outputs.items, 0);
        assert_eq!(outcome.backups.items, 0);
        assert_eq!(outcome.total_reclaimed_bytes(), 0);
    }

    /// IPC 层 roundtrip:`get_disk_usage_inner` 走完整 spawn_blocking
    /// 链路(logs 走宿主 env 解析,只断言条目在场与总量自洽,不断言
    /// 绝对值)。
    #[tokio::test(flavor = "multi_thread")]
    async fn usage_inner_reports_seven_entries_and_consistent_total() {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        let report = get_disk_usage_inner(&state).await.unwrap();
        assert_eq!(report.entries.len(), 7);
        let sum: u64 = report.entries.iter().map(|e| e.bytes).sum();
        assert_eq!(report.total_bytes, sum, "total = sum of entries");
    }
}
