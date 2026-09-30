//! WebKitCache 启动清理(F3 PR4 / P2-b,2026-09-03, task
//! `09-03-f3-disk-governance` design §4)。
//!
//! **为什么是启动时机而非节拍**:`<app_data_dir>/WebKitCache` 归 GUI
//! webview 进程活跃使用,daemon 跨进程删除的时机不可控;GUI 启动早期
//! webview 尚未大量使用,是最安全窗口。Linux webkitgtk 持 fd 场景下
//! unlink 语义安全(缓存可再生,webview 会重建)。
//!
//! **装配位置**:daemon bin(`bin/everlasting-daemon.rs`)的
//! `load_daemon_state` 之后一行(de-Tauri 2026-09-30 迁移;原装配点
//! GUI lib.rs setup 公共区随 GUI bin 移除)。装配级回归由
//! [`tests::startup_clean_is_wired_after_state_load_in_daemon_bin`]
//! 源码静态断言守护(函数级测试抓不到装配缺失)。
//!
//! **边界**:浏览器 remote 模式的缓存归浏览器管,与本模块无关
//! (PRD Out of Scope)。

use std::path::Path;

/// 清理阈值:递归大小**超过**该值才 `remove_dir_all`(AC6:>50MiB 清,
/// <50MiB 不动;恰好等于不动)。
pub const WEBKIT_CACHE_THRESHOLD_BYTES: u64 = 50 * 1024 * 1024;

/// 阈值 env 覆盖(单位 MB;0 / 垃圾值视为未设,沿
/// `resolve_cleanup_period_days` 先例)。
pub const WEBKIT_CACHE_THRESHOLD_ENV: &str = "EVERLASTING_WEBKIT_CACHE_MB";

/// GUI webview 缓存目录名(Tauri/WebKitGTK 固定名,`app_data_dir` 下)。
pub const WEBKIT_CACHE_DIR: &str = "WebKitCache";

/// 清理结果(供启动日志;`cleaned=true` 表示目录已删除)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebKitCacheCleanResult {
    pub cleaned: bool,
    /// 清理前的递归字节数(未超阈值时也返回,供 debug 日志)。
    pub bytes: u64,
}

/// 阈值解析纯函数核心(单测锚点,避免并行测试下 set_var 竞态)。
fn threshold_from_env_str(v: Option<&str>) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|mb| *mb > 0)
        .map(|mb| mb * 1024 * 1024)
        .unwrap_or(WEBKIT_CACHE_THRESHOLD_BYTES)
}

/// 运行期阈值解析(env 覆盖 → 缺省常量)。
pub fn resolve_webkit_cache_threshold() -> u64 {
    threshold_from_env_str(std::env::var(WEBKIT_CACHE_THRESHOLD_ENV).ok().as_deref())
}

/// 清理判定与执行:递归大小(复用 [`crate::disk::governor::inspect_dir`],
/// 与回收摘要同口径)> `threshold_bytes` → `remove_dir_all`。
///
/// 容错:目录缺失 no-op(从未产生过缓存);remove 失败 `warn!` 不
/// panic(缓存可再生,清理是附属保障,绝不能挡 GUI 启动)。
pub fn maybe_clean_webkit_cache(cache_dir: &Path, threshold_bytes: u64) -> WebKitCacheCleanResult {
    if !cache_dir.is_dir() {
        return WebKitCacheCleanResult {
            cleaned: false,
            bytes: 0,
        };
    }
    let (bytes, _) = crate::disk::governor::inspect_dir(cache_dir);
    if bytes <= threshold_bytes {
        return WebKitCacheCleanResult {
            cleaned: false,
            bytes,
        };
    }
    tracing::info!(
        cache_dir = %cache_dir.display(),
        bytes,
        threshold_bytes,
        "removing oversized WebKitCache (webview will rebuild)"
    );
    match std::fs::remove_dir_all(cache_dir) {
        Ok(()) => WebKitCacheCleanResult {
            cleaned: true,
            bytes,
        },
        // 目录恰好在我们检查后被(外部)清掉 → 语义上已达成清理。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => WebKitCacheCleanResult {
            cleaned: true,
            bytes,
        },
        Err(e) => {
            tracing::warn!(
                cache_dir = %cache_dir.display(),
                error = %e,
                "WebKitCache removal failed (non-fatal; cache is regenerable)"
            );
            WebKitCacheCleanResult {
                cleaned: false,
                bytes,
            }
        }
    }
}

/// daemon 启动入口(daemon bin `load_daemon_state` 后一行装配):
/// 异步 spawn 一次阈值清理,不等结果(不阻塞启动)。每次启动至多
/// 一次,无周期 timer。de-Tauri(2026-09-30)后 GUI webview 不再产生
/// 新缓存,本清理只负责历史存量的回收 —— 模块与 `commands/disk.rs`
/// 的 webkit_cache key 的最终删除见 BACKLOG(de-Tauri 后续项)。
pub fn spawn_startup_clean(app_data_dir: std::path::PathBuf) {
    tokio::spawn(async move {
        let cache_dir = app_data_dir.join(WEBKIT_CACHE_DIR);
        let result = maybe_clean_webkit_cache(&cache_dir, resolve_webkit_cache_threshold());
        if result.cleaned {
            tracing::info!(
                bytes = result.bytes,
                "WebKitCache startup cleanup done (over threshold; webview rebuilds)"
            );
        } else {
            tracing::debug!(
                bytes = result.bytes,
                "WebKitCache startup check: within threshold or absent, no-op"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 超阈值 → 整目录删除,结果 cleaned + 清理前字节数。
    #[test]
    fn oversized_cache_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join(WEBKIT_CACHE_DIR);
        std::fs::create_dir_all(cache.join("blob")).unwrap();
        std::fs::write(cache.join("blob/f1"), vec![0u8; 60]).unwrap();
        // 101 字节 > 100 阈值(恰好等于的边界归不动侧,由
        // `within_threshold_cache_is_untouched` 锁定)。
        std::fs::write(cache.join("f0"), vec![0u8; 41]).unwrap();

        let res = maybe_clean_webkit_cache(&cache, 100);
        assert!(res.cleaned);
        assert_eq!(res.bytes, 101, "recursive size counted before removal");
        assert!(!cache.exists(), "cache dir removed");
    }

    /// 未超阈值(含恰好等于)→ 不动。
    #[test]
    fn within_threshold_cache_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join(WEBKIT_CACHE_DIR);
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("f"), vec![0u8; 50]).unwrap();

        let under = maybe_clean_webkit_cache(&cache, 100);
        assert!(!under.cleaned && under.bytes == 50);
        assert!(cache.exists());

        // 恰好等于阈值:严格 > 才清(AC6「>50MiB 清空,<50MiB 不动」,
        // 边界归属不动侧)。
        let at_threshold = maybe_clean_webkit_cache(&cache, 50);
        assert!(!at_threshold.cleaned, "exactly at threshold is NOT cleaned");
        assert!(cache.exists());
    }

    /// 目录缺失 → no-op(从未产生过缓存的机器)。
    #[test]
    fn missing_cache_dir_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let res = maybe_clean_webkit_cache(&dir.path().join(WEBKIT_CACHE_DIR), 100);
        assert!(!res.cleaned);
        assert_eq!(res.bytes, 0);
    }

    /// 阈值 env 解析(纯函数核心):正整数 MB → 字节;缺失 / 0 / 垃圾值
    /// → 缺省 50MiB。
    #[test]
    fn threshold_env_resolution() {
        assert_eq!(threshold_from_env_str(Some("10")), 10 * 1024 * 1024);
        assert_eq!(threshold_from_env_str(Some(" 200 ")), 200 * 1024 * 1024);
        assert_eq!(threshold_from_env_str(None), WEBKIT_CACHE_THRESHOLD_BYTES);
        assert_eq!(
            threshold_from_env_str(Some("0")),
            WEBKIT_CACHE_THRESHOLD_BYTES
        );
        assert_eq!(
            threshold_from_env_str(Some("abc")),
            WEBKIT_CACHE_THRESHOLD_BYTES
        );
        assert_eq!(
            threshold_from_env_str(Some("-1")),
            WEBKIT_CACHE_THRESHOLD_BYTES
        );
    }

    /// **装配级守护**:静态断言 `spawn_startup_clean` 的调用点在
    /// daemon bin(`bin/everlasting-daemon.rs`)的
    /// `load_daemon_state` 之后。函数级测试抓不到装配缺失;沿
    /// `transport/http.routes-sync.test.ts`「解析源码守卫装配」先例。
    /// de-Tauri(2026-09-30):装配点从 GUI lib.rs setup 迁到 daemon bin。
    #[test]
    fn startup_clean_is_wired_after_state_load_in_daemon_bin() {
        let src = include_str!("../../src/bin/everlasting-daemon.rs");
        let state_load = src
            .find("load_daemon_state")
            .expect("daemon bin must load AppState");
        let call_site = src
            .find("webkit_cache::spawn_startup_clean")
            .expect("daemon bin must call webkit_cache::spawn_startup_clean at startup");
        assert!(
            state_load < call_site,
            "WebKitCache cleanup must be wired AFTER load_daemon_state \
             (needs the resolved data dir)"
        );
    }
}
