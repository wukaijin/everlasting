//! request 在途表(design §3.2.1 P2-2 / implement.md Step 6)。
//!
//! remote → PC 发 `Request` 帧时登记 `request_id → PendingReply`,PC
//! 回 `Response`/`Stream` 帧时按 id 路由回等待方。
//!
//! ```text
//! PendingReply::Oneshot(oneshot::Sender<Frame>)  非流式:Response 一次(Step 6)
//! PendingReply::Stream(mpsc::Sender<StreamEvent>) SSE:持续 chunk / End / Error(S3 用)
//! ```
//!
//! **超时清理**:每条 pending 只对应一个等待方(proxy handler 的
//! `timeout(PENDING_TIMEOUT, rx)`),等待方无论成功/超时/错误都
//! `remove(id)` —— 不存在泄漏条目,无需全局清扫 task。
//!
//! **连接断开时**:pending 条目由 60s 超时兜底清理(S1 简化 ——
//! design P2-2 "in-flight 请求不做迁移,MVP 返 502 让前端重试")。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use everlasting_remote_protocol::{Frame, StreamEvent};
use tokio::sync::{mpsc, oneshot};

/// 单条在途请求的回复通道(P2-2 修订:S1 直接按最终形态落地)。
pub enum PendingReply {
    /// 非流式:PC 回 `Response` 帧时 send 进来。
    Oneshot(oneshot::Sender<Frame>),
    /// SSE 流式:PC 的 `Stream` 帧(Chunk/End/Error)持续 send
    /// (S3 实现,当前不实例化)。
    Stream(mpsc::Sender<StreamEvent>),
}

/// `request_id → PendingReply` 表 + id 生成器。
///
/// 手写 `Debug`(Sender 无 Debug;RemoteState derive(Debug) 需要),
/// 只打印在途条数。
pub struct PendingTable {
    inner: DashMap<u64, PendingReply>,
    next_id: AtomicU64,
    /// 单条等待超时(proxy handler 用)。放表内而非模块 const:
    /// 测试用小值(秒级参数无法在测试里等 60s)。
    pub timeout: Duration,
}

impl PendingTable {
    pub fn new(timeout: Duration) -> Self {
        Self {
            inner: DashMap::new(),
            next_id: AtomicU64::new(0),
            timeout,
        }
    }

    /// 原子递增 request_id(进程内唯一)。
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn insert(&self, id: u64, reply: PendingReply) {
        self.inner.insert(id, reply);
    }

    /// 移除并取回复通道(proxy 清理 / ws 接收循环路由)。
    pub fn remove(&self, id: u64) -> Option<PendingReply> {
        self.inner.remove(&id).map(|(_, r)| r)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl std::fmt::Debug for PendingTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingTable")
            .field("in_flight", &self.inner.len())
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_id_monotonic() {
        let t = PendingTable::new(Duration::from_secs(60));
        let a = t.next_id();
        let b = t.next_id();
        assert!(b > a);
    }

    #[test]
    fn insert_remove_roundtrip() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (tx, _rx) = oneshot::channel();
        t.insert(7, PendingReply::Oneshot(tx));
        assert_eq!(t.len(), 1);
        assert!(matches!(t.remove(7), Some(PendingReply::Oneshot(_))));
        assert!(t.remove(7).is_none());
        assert!(t.is_empty());
    }

    #[test]
    fn default_timeout_is_60s() {
        let t = PendingTable::new(Duration::from_secs(60));
        assert_eq!(t.timeout, Duration::from_secs(60));
    }
}
