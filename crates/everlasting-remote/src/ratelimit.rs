//! per-IP 滑动窗口限速(design §2.3 P2-3,implement.md Step 7)。
//!
//! 配对码 6 位 = 1M 空间 + 60s 窗口 + 公网可达(epic NFR 威胁模型是
//! "扫描器每天扫"),`POST /api/v1/pairing/redeem` 无限制时可暴力扫。
//! 内存计数 `DashMap<IpAddr, Window>`(MVP 不需 Redis),窗口随每次
//! `allow` 惰性滚动(过期即重置,无后台清扫 task)。
//!
//! 窗口参数可注入(max / window):生产 10 次/分钟;测试用小值,无需
//! 等真实窗口。

use std::net::IpAddr;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// 计数窗口:窗口起点 + 窗口内计数。
struct Window {
    start: Instant,
    count: u32,
}

/// per-IP 限速器。
pub struct RateLimiter {
    inner: DashMap<IpAddr, Window>,
    max: u32,
    window: Duration,
}

impl RateLimiter {
    /// `max` 次 / `window` 时长。
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            inner: DashMap::new(),
            max,
            window,
        }
    }

    /// 该 IP 是否允许本次请求。允许则计数 +1;窗口过期自动滚动重置;
    /// 超限返回 false(调用方返 429)。
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut entry = self.inner.entry(ip).or_insert_with(|| Window {
            start: now,
            count: 0,
        });
        if now.duration_since(entry.start) >= self.window {
            // 窗口过期:滚动新窗口
            entry.start = now;
            entry.count = 0;
        }
        if entry.count >= self.max {
            return false;
        }
        entry.count += 1;
        true
    }

    /// 当前记录的 IP 数(诊断 / 测试)。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("tracked_ips", &self.inner.len())
            .field("max", &self.max)
            .field("window", &self.window)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, n))
    }

    /// 窗口内 max 次全过,第 max+1 次拒绝。
    #[test]
    fn allows_up_to_max_then_rejects() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.allow(ip(1)));
        assert!(limiter.allow(ip(1)));
        assert!(!limiter.allow(ip(1)), "第 3 次应被拒");
        assert_eq!(limiter.len(), 1);
    }

    /// 不同 IP 独立计数。
    #[test]
    fn per_ip_independent() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        assert!(limiter.allow(ip(1)));
        assert!(!limiter.allow(ip(1)));
        assert!(limiter.allow(ip(2)), "其他 IP 不受影响");
    }

    /// 窗口滚动:过期后恢复(小窗口注入,不用等真实窗口)。
    #[test]
    fn window_rolls_after_expiry() {
        let limiter = RateLimiter::new(1, Duration::from_millis(50));
        assert!(limiter.allow(ip(1)));
        assert!(!limiter.allow(ip(1)));
        std::thread::sleep(Duration::from_millis(80));
        assert!(limiter.allow(ip(1)), "窗口过期后应恢复");
    }
}
