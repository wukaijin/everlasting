//! 磁盘治理 governor:回收函数族(`_inner` 化)+ 每日节拍。
//!
//! F3 PR1(2026-09-03, task `09-03-f3-disk-governance`,design §1/§2)。
//! 四个回收项与双入口:
//!
//! | 回收项 | 实现 | 节拍 | 手动(PR3) |
//! |---|---|---|---|
//! | worker worktree(7 天 mtime) | [`sweep_worker_worktrees_inner`](现有 [`crate::git::worktree::sweep_stale_worker_worktrees`] 的装配包装,P0-a 断链修复) | ✓ | ✓ |
//! | 孤儿 session worktree(P1-a) | [`sweep_orphan_session_worktrees_inner`] | ✓ | ✓ |
//! | outputs 孤儿桶 + 按龄(P1-b) | [`sweep_outputs_inner`] | ✓ | ✓ |
//! | 备份预算自适应 prune(P2-a) | [`db::backup::prune_backups`](crate::db::backup) 原地改 | ✓ | ✓ |
//!
//! - 节拍 = daemon bin 唯一装配的 [`spawn_disk_governor`](24h,首拍延迟
//!   5 分钟避开启动 IO);GUI Full 逃生模式在 lib.rs 跑一次性
//!   [`run_governor_pass_once`](不挂周期 timer,「GUI 主进程零 timer
//!   task」硬约束保持);Thin 模式由 daemon bin 节拍兜底。
//! - kill-switch `disk_governor_enabled`(fail-open:仅字面 `"false"`
//!   关,读法照 `scheduler_tick_with_fire` 先例)只门控**自动**节拍 /
//!   启动 pass;PR3 的手动「立即清理」直接调 [`run_governor_pass_inner`],
//!   不受开关限制(AC9 手动语义)。
//! - 摘要结构(`CleanupResult` / `DiskGovernorOutcome`)是 Serialize +
//!   camelCase wire——节拍日志与 PR3 IPC `run_disk_cleanup` 双消费。
//! - WebKitCache 不在本族(design §4:GUI 启动时机,PR4)。
//!
//! 容错基调(best-effort,照 `git/worktree/sweep.rs` 先例):单条失败
//! `warn!` 跳过继续,**绝不**因一项失败中断整轮;DB 读失败一律保守
//! 「当作存在」(fail-keep)——只有「目录在且 DB 行**确实**不存在」
//! 才判孤儿(design §10 风险缓解)。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::SqlitePool;

use crate::state::AppState;

/// 有主 outputs 桶按龄回收的默认天数(design §2.3;桶龄取桶内最新
/// 文件 mtime,spill 追加会刷新)。
pub const OUTPUTS_AGE_DAYS: u64 = 30;

/// [`OUTPUTS_AGE_DAYS`] 的 env 覆盖(0 / 垃圾值视为未设,沿
/// `resolve_cleanup_period_days` 先例)。
pub const OUTPUTS_AGE_DAYS_ENV: &str = "EVERLASTING_OUTPUTS_AGE_DAYS";

/// 节拍 kill-switch 的 app_config 键(fail-open:仅字面 `"false"` 关)。
/// 与 `commands::config.rs` 的 `AppConfigPayload.diskGovernorEnabled`
/// 读出口、`SETTABLE_APP_FLAGS` 白名单共用本常量,防两处字面漂移
/// (沿 `SCHEDULED_TASKS_ENABLED_KEY` 先例)。
pub const DISK_GOVERNOR_ENABLED_KEY: &str = "disk_governor_enabled";

/// 有主 outputs 按龄回收开关的 app_config 键(fail-open 同上)。孤儿桶
/// 与 `_no_session` 不受此开关管辖(无主数据恒回收,AC3)。
pub const OUTPUTS_AGE_CLEANUP_ENABLED_KEY: &str = "outputs_age_cleanup_enabled";

/// 节拍首拍延迟:避开启动 IO(migrate / backup / orphan-guard 都在
/// 启动窗口)。
const FIRST_TICK_DELAY: Duration = Duration::from_secs(5 * 60);

/// 节拍周期:24h(与 backup task 对称;磁盘回收是日级语义,design §9)。
const TICK_PERIOD: Duration = Duration::from_secs(24 * 3600);

// ---------------------------------------------------------------------------
// 摘要结构(camelCase wire,PR3 `run_disk_cleanup` 直接复用)
// ---------------------------------------------------------------------------

/// 单个回收项的摘要:回收条数 + 回收字节数。
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    /// 回收条数(worktree / outputs 桶 / 备份文件的份数;整项目目录按
    /// 1 计)。
    pub items: u64,
    /// 回收的字节数(表观大小口径,递归累计)。
    pub reclaimed_bytes: u64,
}

/// 一次完整 governor pass 的四项摘要(顺序 = 执行顺序)。
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskGovernorOutcome {
    pub worker_worktrees: CleanupResult,
    pub orphan_session_worktrees: CleanupResult,
    pub outputs: CleanupResult,
    pub backups: CleanupResult,
}

impl DiskGovernorOutcome {
    /// 四项回收字节总和(节拍单条 info! 日志的 `total_bytes` 字段)。
    pub fn total_reclaimed_bytes(&self) -> u64 {
        self.worker_worktrees.reclaimed_bytes
            + self.orphan_session_worktrees.reclaimed_bytes
            + self.outputs.reclaimed_bytes
            + self.backups.reclaimed_bytes
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 递归统计目录:总字节数 + **最新文件 mtime**(桶龄判定的数据源)。
/// 手写栈式 walk,不跟随 symlink,不可读条目跳过(照 `tools/glob.rs`
/// 手写 walk_dir 的「不为此拉新 crate」先例)。目录缺失 → `(0, None)`。
/// PR3 起 `commands::disk::get_disk_usage` 复用本函数做各消费点的字节
/// 累计(单源,口径一致)。
pub(crate) fn inspect_dir(path: &Path) -> (u64, Option<SystemTime>) {
    let mut bytes = 0u64;
    let mut newest: Option<SystemTime> = None;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // 不可读(权限等)→ 跳过该子树
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = entry.path();
            if ft.is_dir() {
                stack.push(p);
            } else {
                let Ok(md) = std::fs::metadata(&p) else {
                    continue;
                };
                bytes += md.len();
                if let Ok(m) = md.modified() {
                    newest = Some(match newest {
                        Some(n) if n >= m => n,
                        _ => m,
                    });
                }
            }
        }
    }
    (bytes, newest)
}

/// 目录名是否符合我们生成的 id 形态(UUID v4 字符串)。worktrees /
/// outputs 的目录名全部来自 `Uuid::new_v4().to_string()`;不认识的
/// 形态**保守跳过**(可能是未来布局版本或外部产物,删除逻辑不碰)。
fn is_uuid_name(name: &str) -> bool {
    uuid::Uuid::parse_str(name).is_ok()
}

/// 龄阈值解析纯函数核心(单测锚点,避免并行测试下 set_var 竞态)。
fn age_days_from_env_str(v: Option<&str>) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(OUTPUTS_AGE_DAYS)
}

/// 龄阈值解析:explicit 参数 > env 覆盖 > [`OUTPUTS_AGE_DAYS`] 常量。
pub fn resolve_outputs_age_days(explicit: Option<u64>) -> u64 {
    if let Some(d) = explicit {
        return d;
    }
    age_days_from_env_str(std::env::var(OUTPUTS_AGE_DAYS_ENV).ok().as_deref())
}

/// 桶是否超龄:桶内最新文件 mtime 早于 cutoff。空桶(None)无龄信息
/// → 不判超龄(0 字节,留着无害;孤儿路径不受此限)。
fn older_than(newest: Option<SystemTime>, cutoff: SystemTime) -> bool {
    newest.is_some_and(|m| m < cutoff)
}

// ---------------------------------------------------------------------------
// 回收项 1:worker worktree sweep(P0-a 装配修复)
// ---------------------------------------------------------------------------

/// worker worktree sweep 的 governor 包装:**本体零改动**——逐项目调
/// 现有 [`crate::git::worktree::sweep_stale_worker_worktrees`](7 天
/// mtime + locked 跳过 + env 覆盖,契约见 spec worker-sweep)。P0-a
/// 的「宿主断链」是装配问题:此前只有 GUI Full 启动跑,daemon bin
/// (默认模式真正宿主)不调——governor 节拍与 GUI 启动 pass 都经本
/// 包装,断链闭合。
///
/// 摘要:条数 = destroy 计数(本体返回值);字节 = 各项目 `worker/`
/// 根目录 before/after 差值(常见场景 worker 目录不存在,零 walk
/// 成本)。项目清单沿既有 GUI 启动 pass 先例取可见项目
/// (`list_projects(false)`)。
pub async fn sweep_worker_worktrees_inner(db: &SqlitePool, data_dir: &Path) -> CleanupResult {
    let mut out = CleanupResult::default();
    let projects = match crate::db::list_projects(db, false).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "disk governor: list projects failed; worker sweep skipped (non-fatal)"
            );
            return out;
        }
    };
    let cleanup_days = crate::git::worktree::resolve_cleanup_period_days(None);
    for project in &projects {
        let worker_root = data_dir.join("worktrees").join(&project.id).join("worker");
        let before = if worker_root.exists() {
            inspect_dir(&worker_root).0
        } else {
            0
        };
        match crate::git::worktree::sweep_stale_worker_worktrees(
            data_dir,
            &project.id,
            Path::new(&project.path),
            cleanup_days,
        ) {
            Ok(n) => {
                out.items += n as u64;
                if n > 0 {
                    let after = inspect_dir(&worker_root).0;
                    out.reclaimed_bytes += before.saturating_sub(after);
                }
            }
            Err(e) => {
                tracing::warn!(
                    project_id = %project.id,
                    error = %e,
                    "disk governor: worker sweep failed for project (non-fatal; continuing)"
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 回收项 2:孤儿 session worktree(P1-a)
// ---------------------------------------------------------------------------

/// 孤儿 session worktree 回收。遍历 `<data_dir>/worktrees/<project_uuid>/`
/// 下的 session 子目录(布局见 `git/worktree/naming.rs:27-59`),判定:
///
/// - **session 行存在**(Active 或 Detached,任何 worktree_state)→ 不动
///   (Detached 是有意保留的,`commands/worktree.rs` 先例)。
/// - **session 行不存在** → 孤儿 → 复用 [`crate::git::destroy_worktree`]
///   链(`lifecycle.rs::destroy`:删目录 + prune 元数据 + 删
///   `session/<id>` 分支)。DB 行已没了,分支无人引用,随 destroy 删除
///   是数据一致性收尾(design §2.2)。
/// - `<project_uuid>` 目录在但 **projects 行不存在** → 整目录销毁
///   (其下 session 必为孤儿;project 路径已不可知,分支清理无从谈起,
///   只能删本地 checkout 目录)。
///
/// 安全边界:
/// - `worker/` 子目录是 worker 命名空间,明确排除(归回收项 1 管)。
/// - 目录名非 UUID 形态 → 保守跳过(见 [`is_uuid_name`])。
/// - **DB 读失败 → 当作存在跳过**(fail-keep):只有「目录在且 DB 行
///   确实不存在」双条件才动手(design §10)。
/// - 单条 destroy 失败 `warn!` 继续(best-effort,照 worker sweep 先例)。
pub async fn sweep_orphan_session_worktrees_inner(
    db: &SqlitePool,
    data_dir: &Path,
) -> CleanupResult {
    let mut out = CleanupResult::default();
    let worktrees_root = data_dir.join("worktrees");
    let entries = match std::fs::read_dir(&worktrees_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out, // 从未建过 worktree
        Err(e) => {
            tracing::warn!(
                error = %e,
                "disk governor: read worktrees root failed; orphan sweep skipped (non-fatal)"
            );
            return out;
        }
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let project_dir = entry.path();
        let Some(project_uuid) = entry
            .file_name()
            .to_str()
            .map(String::from)
            .filter(|n| is_uuid_name(n))
        else {
            // 非 UUID 形态:不是我们生成的 project 目录,保守跳过。
            continue;
        };

        match crate::db::get_project(db, &project_uuid).await {
            Ok(Some(project)) => {
                sweep_one_project_session_worktrees(
                    db,
                    &project_dir,
                    Path::new(&project.path),
                    &mut out,
                )
                .await;
            }
            Ok(None) => {
                // projects 行没了 → 整目录销毁(其下全是孤儿)。
                let (bytes, _) = inspect_dir(&project_dir);
                tracing::info!(
                    project_id = %project_uuid,
                    dir = %project_dir.display(),
                    "disk governor: destroying worktrees of orphan project"
                );
                if let Err(e) = std::fs::remove_dir_all(&project_dir) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            project_id = %project_uuid,
                            error = %e,
                            "disk governor: remove orphan project dir failed (non-fatal)"
                        );
                        continue;
                    }
                }
                out.items += 1;
                out.reclaimed_bytes += bytes;
            }
            Err(e) => {
                // DB 读失败 → fail-keep,不判孤儿。
                tracing::warn!(
                    project_id = %project_uuid,
                    error = %e,
                    "disk governor: get_project failed; project dir kept (fail-keep)"
                );
            }
        }
    }
    out
}

/// 单个项目目录下的 session worktree 孤儿判定(`sweep_orphan_session_
/// worktrees_inner` 的内层)。独立成函数只是把两层循环的缩进摊平,
/// 无独立调用方。
async fn sweep_one_project_session_worktrees(
    db: &SqlitePool,
    project_dir: &Path,
    project_path: &Path,
    out: &mut CleanupResult,
) {
    let entries = match std::fs::read_dir(project_dir) {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(
                dir = %project_dir.display(),
                error = %e,
                "disk governor: read project worktrees dir failed (non-fatal)"
            );
            return;
        }
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(session_id) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        // worker/ 子目录是 worker 命名空间(naming.rs:53-59),归回收项
        // 1(7 天 mtime + locked 判定)管,这里明确排除。
        if session_id == "worker" {
            continue;
        }
        if !is_uuid_name(&session_id) {
            continue;
        }
        // DB 行存在性:读失败 → fail-keep(防把活 session 误判孤儿)。
        let exists = match crate::db::session_exists(db, &session_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "disk governor: session_exists failed; worktree kept (fail-keep)"
                );
                continue;
            }
        };
        if exists {
            continue; // Active / Detached 都保留
        }
        let wt_path = entry.path();
        let (bytes, _) = inspect_dir(&wt_path);
        tracing::info!(
            session_id = %session_id,
            worktree = %wt_path.display(),
            "disk governor: destroying orphan session worktree"
        );
        if let Err(e) = crate::git::destroy_worktree(project_path, &wt_path, &session_id) {
            tracing::warn!(
                session_id = %session_id,
                worktree = %wt_path.display(),
                error = %e,
                "disk governor: destroy orphan session worktree failed (non-fatal; continuing)"
            );
            continue;
        }
        out.items += 1;
        out.reclaimed_bytes += bytes;
    }
}

// ---------------------------------------------------------------------------
// 回收项 3:outputs(P1-b)
// ---------------------------------------------------------------------------

/// `<data_dir>/outputs/` 按 session 桶回收。三类(design §2.3):
///
/// - **孤儿桶**(目录名 = session_id 且 DB 行不存在)→ 直接删(无龄
///   判定;复用 [`crate::tools::tool_output::sweep_session_outputs`],
///   与 session 删除同一条删除路径)。
/// - **有主桶** → 按 [`OUTPUTS_AGE_DAYS`] 龄回收:桶龄 = 桶内最新文件
///   mtime(spill 追加会刷新),超龄 → 清空桶(spill 按需重建目录)。
///   回收前查开关 `outputs_age_cleanup_enabled`(fail-open:仅字面
///   `"false"` 关;AC3「开关关时有主桶不删」)。C6 恢复链路影响:被
///   回收 session 的旧 tool_result 恢复模式降级为已删(spill miss 消费
///   方已有容错),R2 裁定的可接受代价。
/// - **`_no_session`**(无 session fallback 桶)→ 同龄回收,不经开关
///   (无主数据恒回收)。
///
/// 安全边界同回收项 2:DB 读失败 fail-keep;非 UUID 形态(且非
/// `_no_session`)保守跳过;单桶失败 `warn!` 继续。目录缺失 no-op。
pub async fn sweep_outputs_inner(db: &SqlitePool, data_dir: &Path) -> CleanupResult {
    let mut out = CleanupResult::default();
    let outputs_root = data_dir.join("outputs");
    let entries = match std::fs::read_dir(&outputs_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out, // 从未 spill 过
        Err(e) => {
            tracing::warn!(
                error = %e,
                "disk governor: read outputs root failed; outputs sweep skipped (non-fatal)"
            );
            return out;
        }
    };

    // 有主桶按龄回收的总开关(fail-open;一次 pass 读一次,不逐桶读)。
    let age_cleanup_enabled =
        match crate::db::config::get_config_value(db, OUTPUTS_AGE_CLEANUP_ENABLED_KEY).await {
            Ok(Some(v)) => v != "false",
            _ => true,
        };
    let cutoff = SystemTime::now() - Duration::from_secs(resolve_outputs_age_days(None) * 86400);

    for entry in entries.flatten() {
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let bucket = entry.path();
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let (bytes, newest) = inspect_dir(&bucket);

        if name == crate::tools::tool_output::NO_SESSION_DIR {
            // 无 session fallback 桶:同龄回收,不经开关(无主数据)。
            if older_than(newest, cutoff) && remove_bucket(&bucket) {
                out.items += 1;
                out.reclaimed_bytes += bytes;
            }
            continue;
        }
        if !is_uuid_name(&name) {
            continue; // 不认识的目录形态,保守跳过
        }

        // 孤儿判定:读失败 → fail-keep。
        let exists = match crate::db::session_exists(db, &name).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    bucket = %name,
                    error = %e,
                    "disk governor: session_exists failed; outputs bucket kept (fail-keep)"
                );
                continue;
            }
        };
        if !exists {
            // 孤儿桶:与 session 删除同一条删除路径。
            crate::tools::tool_output::sweep_session_outputs(data_dir, &name).await;
            if !bucket.exists() {
                out.items += 1;
                out.reclaimed_bytes += bytes;
            }
            continue;
        }
        // 有主桶:开关 + 龄。
        if age_cleanup_enabled && older_than(newest, cutoff) {
            tracing::info!(
                bucket = %bucket.display(),
                "disk governor: removing stale owned outputs bucket (> age threshold)"
            );
            if remove_bucket(&bucket) {
                out.items += 1;
                out.reclaimed_bytes += bytes;
            }
        }
    }
    out
}

/// 删一个 outputs 桶。缺失视为已删(幂等);其他失败 `warn!` 返回
/// false(摘要不计)。
fn remove_bucket(bucket: &Path) -> bool {
    match std::fs::remove_dir_all(bucket) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            tracing::warn!(
                bucket = %bucket.display(),
                error = %e,
                "disk governor: remove outputs bucket failed (non-fatal)"
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// 回收项 4:备份预算自适应 prune(P2-a,本体在 db/backup.rs)
// ---------------------------------------------------------------------------

/// 备份 prune 的 governor 包装:`db::backup::prune_backups` 已原地改
/// 为预算自适应(200MiB 预算 / 至少 2 份 / 至多 7 份,env
/// `EVERLASTING_BACKUP_BUDGET_MB` 覆盖),这里只做错误容错 + 摘要转换。
fn prune_backups_item(data_dir: &Path) -> CleanupResult {
    let dir = crate::db::backup::backup_dir(data_dir);
    match crate::db::backup::prune_backups(&dir, crate::db::backup::KEEP_BACKUPS) {
        Ok(outcome) => CleanupResult {
            items: outcome.removed.len() as u64,
            reclaimed_bytes: outcome.reclaimed_bytes,
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                "disk governor: prune backups failed (non-fatal)"
            );
            CleanupResult::default()
        }
    }
}

// ---------------------------------------------------------------------------
// pass 编排 + kill-switch
// ---------------------------------------------------------------------------

/// 节拍 kill-switch 判定(`disk_governor_enabled`,fail-open:仅字面
/// `"false"` 关,读法照 `scheduler_tick_with_fire` 先例——缺失 / 读
/// 失败 / 其他值都是开)。只门控自动节拍与启动 pass;PR3 手动
/// 「立即清理」不查它(AC9)。
pub async fn disk_governor_enabled(db: &SqlitePool) -> bool {
    match crate::db::config::get_config_value(db, DISK_GOVERNOR_ENABLED_KEY).await {
        Ok(Some(v)) => v != "false",
        _ => true,
    }
}

/// 完整回收 pass,固定顺序:worker sweep → 孤儿 session worktree →
/// outputs → 备份 prune。**不含 kill-switch 判定**(调用方自查):
/// 每日节拍与 GUI 启动 pass 经门控入口 [`run_governor_pass_once`],
/// PR3 的手动「立即清理」直接调本函数。也不含 WebKitCache(GUI 启动
/// 时机,PR4)。文件系统操作直接在当前 task 执行(照既有 sweep 先例;
/// 每日一次,阻塞成本可接受)。
pub async fn run_governor_pass_inner(db: &SqlitePool, data_dir: &Path) -> DiskGovernorOutcome {
    let worker_worktrees = sweep_worker_worktrees_inner(db, data_dir).await;
    let orphan_session_worktrees = sweep_orphan_session_worktrees_inner(db, data_dir).await;
    let outputs = sweep_outputs_inner(db, data_dir).await;
    let backups = prune_backups_item(data_dir);
    DiskGovernorOutcome {
        worker_worktrees,
        orphan_session_worktrees,
        outputs,
        backups,
    }
}

/// 门控入口:kill-switch 开 → 跑 [`run_governor_pass_inner`] 并出单条
/// info! 汇总(含回收字节总数);关 → 空转返回 `None`(AC9:节拍停,
/// 手动清理仍可用)。每日节拍的每轮 tick 与 GUI Full 启动一次性 pass
/// 共用本入口。
pub async fn run_governor_pass_once(
    db: &SqlitePool,
    data_dir: &Path,
) -> Option<DiskGovernorOutcome> {
    if !disk_governor_enabled(db).await {
        tracing::debug!("disk governor disabled by config; skipping pass");
        return None;
    }
    let outcome = run_governor_pass_inner(db, data_dir).await;
    tracing::info!(
        worker_worktrees = outcome.worker_worktrees.items,
        worker_worktrees_bytes = outcome.worker_worktrees.reclaimed_bytes,
        orphan_session_worktrees = outcome.orphan_session_worktrees.items,
        orphan_session_worktrees_bytes = outcome.orphan_session_worktrees.reclaimed_bytes,
        outputs_buckets = outcome.outputs.items,
        outputs_bytes = outcome.outputs.reclaimed_bytes,
        backups_pruned = outcome.backups.items,
        backups_bytes = outcome.backups.reclaimed_bytes,
        total_bytes = outcome.total_reclaimed_bytes(),
        "disk governor pass complete"
    );
    Some(outcome)
}

/// 每日磁盘治理节拍(F3 PR1)。**唯一装配点 = daemon bin**
/// (`daemon/server.rs::spawn_disk_governor` wrapper);照
/// `spawn_task_scheduler` 形态:detached spawn + `tokio::select! { biased;
/// cancel_token.cancelled() => break; interval.tick() => ... }`。周期
/// 24h,首拍延迟 [`FIRST_TICK_DELAY`](避开启动 IO);kill-switch 每轮
/// 重读(经 [`run_governor_pass_once`])。停机令牌挂
/// `AppState.disk_governor_cancel`(`shutdown_signal` 在 scheduler
/// cancel 同段 cancel)。
pub(crate) fn spawn_disk_governor(state: &std::sync::Arc<AppState>) {
    let state = std::sync::Arc::clone(state);
    tokio::spawn(async move {
        tracing::info!(
            first_tick_delay_secs = FIRST_TICK_DELAY.as_secs(),
            period_secs = TICK_PERIOD.as_secs(),
            "disk governor started"
        );
        let mut interval =
            tokio::time::interval_at(tokio::time::Instant::now() + FIRST_TICK_DELAY, TICK_PERIOD);
        loop {
            tokio::select! {
                biased;
                _ = state.disk_governor_cancel.cancelled() => {
                    tracing::info!("disk governor stopped (shutdown)");
                    break;
                }
                _ = interval.tick() => {
                    let db = state.db.clone();
                    let data_dir = state.app_data_dir.clone();
                    run_governor_pass_once(&db, &data_dir).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use std::sync::Arc;

    /// 建 file-backed AppState(tempdir),与其他模块测试同款
    /// (`commands/config.rs` 先例;multi_thread flavor 供内部 spawn 用)。
    async fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
        let tmp = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::load_from_dir(tmp.path().to_path_buf()).await);
        (tmp, state)
    }

    async fn insert_project(state: &AppState, name: &str, path: &str) -> String {
        crate::db::create_project(&state.db, name, path, false, None)
            .await
            .unwrap()
            .id
    }

    /// 插一行 session(裸 SQL,沿 `db/backup.rs` 测试先例);
    /// `worktree_state` 可指定(默认覆盖 Detached 形态——AC2 锁
    /// 「行在(含 Detached)不删」)。
    async fn insert_session_row(state: &AppState, id: &str, project_id: &str, ws_state: &str) {
        sqlx::query(
            "INSERT INTO sessions (id, title, created_at, updated_at, model, project_id, worktree_state) \
             VALUES (?, 'gov-test', '2026-09-03T00:00:00Z', '2026-09-03T00:00:00Z', 'GLM-4.7', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(ws_state)
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// 把文件 mtime 回拨 N 天(std `File::set_modified`,1.75 起稳定,
    /// 免去 `touch -t` 子进程——tests_worktree.rs 先例的现代替代)。
    fn backdate_file(path: &Path, days_ago: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(days_ago * 86400))
            .unwrap();
    }

    fn new_uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    // ---- 孤儿 session worktree ----

    /// 真孤儿:目录在、session 行无 → 被 destroy(目录消失 + 摘要计数)。
    #[tokio::test(flavor = "multi_thread")]
    async fn orphan_session_worktree_destroyed_when_row_missing() {
        let (tmp, state) = test_state().await;
        // 项目行在(路径指向不存在的目录 → destroy 内 Repository::open
        // 失败仅 warn,物理目录删除仍完成——正是 best-effort 契约)。
        let pid = insert_project(
            &state,
            "p-gov-1",
            &tmp.path().join("no-repo").display().to_string(),
        )
        .await;
        let sid = new_uuid();
        let wt = tmp.path().join("worktrees").join(&pid).join(&sid);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("file.txt"), vec![b'x'; 1024]).unwrap();

        let res = sweep_orphan_session_worktrees_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 1, "one orphan destroyed");
        assert!(
            res.reclaimed_bytes >= 1024,
            "reclaimed bytes cover the payload"
        );
        assert!(!wt.exists(), "orphan worktree dir removed");
    }

    /// 假孤儿:session 行在(worktree_state=Detached)→ 不删。
    #[tokio::test(flavor = "multi_thread")]
    async fn session_worktree_kept_when_row_exists_detached() {
        let (tmp, state) = test_state().await;
        let pid = insert_project(&state, "p-gov-2", "/tmp/nowhere").await;
        let sid = new_uuid();
        insert_session_row(&state, &sid, &pid, "detached").await;
        let wt = tmp.path().join("worktrees").join(&pid).join(&sid);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("file.txt"), b"keep me").unwrap();

        let res = sweep_orphan_session_worktrees_inner(&state.db, tmp.path()).await;
        assert_eq!(
            res.items, 0,
            "Detached session's worktree is intentionally kept"
        );
        assert!(wt.exists());
        assert_eq!(res.reclaimed_bytes, 0);
    }

    /// projects 行没了 → 整目录销毁(其下 session 必为孤儿)。
    #[tokio::test(flavor = "multi_thread")]
    async fn whole_project_dir_removed_when_project_row_missing() {
        let (tmp, state) = test_state().await;
        let ghost_pid = new_uuid();
        let project_dir = tmp.path().join("worktrees").join(&ghost_pid);
        std::fs::create_dir_all(project_dir.join(new_uuid())).unwrap();
        std::fs::create_dir_all(project_dir.join(new_uuid())).unwrap();
        std::fs::write(project_dir.join("stray.txt"), b"x").unwrap();

        let res = sweep_orphan_session_worktrees_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 1, "whole project dir counted as one item");
        assert!(!project_dir.exists(), "orphan project dir removed");
    }

    /// `worker/` 命名空间不受孤儿 sweep 侵扰(归回收项 1 管)。
    #[tokio::test(flavor = "multi_thread")]
    async fn worker_namespace_untouched_by_orphan_sweep() {
        let (tmp, state) = test_state().await;
        let pid = insert_project(&state, "p-gov-3", "/tmp/nowhere").await;
        let worker_dir = tmp
            .path()
            .join("worktrees")
            .join(&pid)
            .join("worker")
            .join("run-1");
        std::fs::create_dir_all(&worker_dir).unwrap();
        std::fs::write(worker_dir.join("in-flight.txt"), b"active worker").unwrap();

        let res = sweep_orphan_session_worktrees_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 0);
        assert!(
            worker_dir.exists(),
            "worker/ namespace is the worker sweep's domain"
        );
    }

    /// worktrees 根目录缺失 → no-op(不报错)。
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_worktrees_dir_is_noop() {
        let (tmp, state) = test_state().await;
        let res = sweep_orphan_session_worktrees_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 0);
        assert_eq!(res.reclaimed_bytes, 0);
    }

    // ---- outputs ----

    /// 孤儿桶:目录在、session 行无 → 删(与 session 删除同路径)。
    #[tokio::test(flavor = "multi_thread")]
    async fn orphan_output_bucket_removed() {
        let (tmp, state) = test_state().await;
        let sid = new_uuid();
        let bucket = tmp.path().join("outputs").join(&sid);
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("a.txt"), vec![b'y'; 512]).unwrap();

        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 1);
        assert!(res.reclaimed_bytes >= 512);
        assert!(!bucket.exists(), "orphan bucket removed");
    }

    /// 有主桶未超龄 → 保留。
    #[tokio::test(flavor = "multi_thread")]
    async fn owned_fresh_output_bucket_kept() {
        let (tmp, state) = test_state().await;
        let pid = insert_project(&state, "p-out-1", "/tmp/nowhere").await;
        let sid = new_uuid();
        insert_session_row(&state, &sid, &pid, "none").await;
        let bucket = tmp.path().join("outputs").join(&sid);
        std::fs::create_dir_all(&bucket).unwrap();
        std::fs::write(bucket.join("a.txt"), b"fresh").unwrap();

        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 0);
        assert!(bucket.exists(), "fresh owned bucket kept");
    }

    /// 有主桶超龄且开关开 → 清空(桶目录删除;spill 按需重建)。
    #[tokio::test(flavor = "multi_thread")]
    async fn owned_stale_output_bucket_removed() {
        let (tmp, state) = test_state().await;
        let pid = insert_project(&state, "p-out-2", "/tmp/nowhere").await;
        let sid = new_uuid();
        insert_session_row(&state, &sid, &pid, "none").await;
        let bucket = tmp.path().join("outputs").join(&sid);
        std::fs::create_dir_all(&bucket).unwrap();
        let file = bucket.join("a.txt");
        std::fs::write(&file, b"stale").unwrap();
        backdate_file(&file, OUTPUTS_AGE_DAYS + 10); // 40 天前 > 30 天阈值

        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 1, "stale owned bucket recycled");
        assert!(!bucket.exists());
    }

    /// 有主桶超龄但开关 `outputs_age_cleanup_enabled=false` → 不删
    /// (AC3 细粒度开关;fail-open 语义:仅字面 "false" 关)。
    #[tokio::test(flavor = "multi_thread")]
    async fn owned_stale_output_bucket_kept_when_disabled() {
        let (tmp, state) = test_state().await;
        crate::db::config::set_config_value(&state.db, OUTPUTS_AGE_CLEANUP_ENABLED_KEY, "false")
            .await
            .unwrap();
        let pid = insert_project(&state, "p-out-3", "/tmp/nowhere").await;
        let sid = new_uuid();
        insert_session_row(&state, &sid, &pid, "none").await;
        let bucket = tmp.path().join("outputs").join(&sid);
        std::fs::create_dir_all(&bucket).unwrap();
        let file = bucket.join("a.txt");
        std::fs::write(&file, b"stale but switch off").unwrap();
        backdate_file(&file, OUTPUTS_AGE_DAYS + 10);

        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 0);
        assert!(bucket.exists(), "switch off → owned bucket survives");
    }

    /// `_no_session` 桶:超龄删(不经开关);未超龄保留。开关关也不拦
    /// 超龄回收(无主数据恒回收,AC3)。注意桶龄 = 桶内**最新**文件
    /// mtime,所以两形态分两轮造(混在一个桶里 fresh 会把桶龄拉新)。
    #[tokio::test(flavor = "multi_thread")]
    async fn no_session_bucket_age_gated_unconditionally() {
        let (tmp, state) = test_state().await;
        // 开关关上仍应回收超龄 `_no_session`(无主,不走开关)。
        crate::db::config::set_config_value(&state.db, OUTPUTS_AGE_CLEANUP_ENABLED_KEY, "false")
            .await
            .unwrap();
        let ns = tmp
            .path()
            .join("outputs")
            .join(crate::tools::tool_output::NO_SESSION_DIR);

        // 轮 1:桶内只有 40 天前的旧文件 → 整桶回收。
        std::fs::create_dir_all(&ns).unwrap();
        let stale = ns.join("stale.txt");
        std::fs::write(&stale, b"old").unwrap();
        backdate_file(&stale, OUTPUTS_AGE_DAYS + 10);
        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(
            res.items, 1,
            "stale _no_session recycled despite switch off"
        );
        assert!(!ns.exists(), "stale _no_session bucket removed");

        // 轮 2:桶内只有新文件 → 保留。
        std::fs::create_dir_all(&ns).unwrap();
        let fresh = ns.join("fresh.txt");
        std::fs::write(&fresh, b"new").unwrap();
        let res2 = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res2.items, 0, "fresh _no_session kept");
        assert!(fresh.exists());
    }

    /// outputs 根目录缺失 → no-op。
    #[tokio::test(flavor = "multi_thread")]
    async fn missing_outputs_dir_is_noop() {
        let (tmp, state) = test_state().await;
        let res = sweep_outputs_inner(&state.db, tmp.path()).await;
        assert_eq!(res.items, 0);
        assert_eq!(res.reclaimed_bytes, 0);
    }

    // ---- kill-switch / 参数解析 ----

    /// kill-switch 判定:仅字面 "false" 关;缺失 / "0" / "no" / 读失败
    /// (不适用)都开(fail-open 铁律)。
    #[tokio::test(flavor = "multi_thread")]
    async fn disk_governor_killswitch_fail_open() {
        let (tmp, state) = test_state().await;
        // 缺省开。
        assert!(disk_governor_enabled(&state.db).await);
        // 仅字面 "false" 关。
        crate::db::config::set_config_value(&state.db, DISK_GOVERNOR_ENABLED_KEY, "false")
            .await
            .unwrap();
        assert!(!disk_governor_enabled(&state.db).await);
        // 其他值一律开(fail-open)。
        for v in ["0", "no", "false ", "FALSE"] {
            crate::db::config::set_config_value(&state.db, DISK_GOVERNOR_ENABLED_KEY, v)
                .await
                .unwrap();
            assert!(
                disk_governor_enabled(&state.db).await,
                "value {v:?} must fail-open (enabled)"
            );
        }
        // 节拍空转:开关关时 run_governor_pass_once 返回 None(判定函数
        // 之上的门控闭环)。
        crate::db::config::set_config_value(&state.db, DISK_GOVERNOR_ENABLED_KEY, "false")
            .await
            .unwrap();
        assert!(
            run_governor_pass_once(&state.db, tmp.path())
                .await
                .is_none(),
            "disabled governor pass must be a no-op returning None"
        );
    }

    /// OUTPUTS_AGE_DAYS env 解析(纯函数核心):正整数生效;缺失 / 0 /
    /// 垃圾值 → 常量缺省。
    #[test]
    fn outputs_age_days_env_resolution() {
        assert_eq!(age_days_from_env_str(Some("7")), 7);
        assert_eq!(age_days_from_env_str(Some(" 90 ")), 90);
        assert_eq!(age_days_from_env_str(None), OUTPUTS_AGE_DAYS);
        assert_eq!(age_days_from_env_str(Some("0")), OUTPUTS_AGE_DAYS);
        assert_eq!(age_days_from_env_str(Some("abc")), OUTPUTS_AGE_DAYS);
        assert_eq!(age_days_from_env_str(Some("-1")), OUTPUTS_AGE_DAYS);
    }

    /// `older_than` 边界:None(空桶)不判超龄;新于 cutoff 不超龄;
    /// 旧于 cutoff 超龄。
    #[test]
    fn older_than_boundaries() {
        let now = SystemTime::now();
        assert!(!older_than(None, now), "empty bucket has no age signal");
        assert!(!older_than(Some(now), now - Duration::from_secs(60)));
        assert!(older_than(
            Some(now - Duration::from_secs(61)),
            now - Duration::from_secs(60)
        ));
    }
}
