//! request 在途表(design §3.2.1 P2-2 / implement.md Step 6 / S3 Step 2)。
//!
//! remote → PC 发 `Request` 帧时登记 `request_id → PendingEntry`,PC
//! 回 `Response`/`Stream` 帧时按 id 路由回等待方。
//!
//! ```text
//! PendingReply::Oneshot(oneshot::Sender<Frame>)  非流式:Response 一次(Step 6)
//! PendingReply::Stream(mpsc::Sender<StreamEvent>) SSE:持续 chunk / End / Error(S3)
//! ```
//!
//! **超时清理(两条不同的路径,P3-2 修订)**:
//! - **Oneshot**:proxy handler 用 `timeout(PENDING_TIMEOUT, rx)` 等
//!   Response —— 等待方无论成功/超时/错误都 `remove(id)`,不存在泄漏,
//!   无需全局清扫 task。连接断开时由该 60s 超时兜底清理(S1 简化,
//!   in-flight 请求不做迁移,MVP 返 502 让前端重试)。
//! - **Stream**:**无 60s 超时**(手机可长期挂流,长任务输出不能 60s 截断)。
//!   清理靠两个事件:
//!   1. 流正常结束 / 手机断开 —— ws.rs `dispatch_frame` 的 Stream 分支
//!      (End/Error → 移除;Chunk try_send 失败 = 慢/断 → 剔除)。
//!   2. node 离线(心跳超时 / 连接断开)——
//!      [`PendingTable::cancel_streams_for_conn`] **按 conn_id 即时清理**
//!      (P1-2:连接级粒度,踢旧/重连窗口期不误杀新连接的在途流)。
//!   另外 proxy 流式分支对首帧挂 30s 超时(design P2-4,实现在 proxy.rs,
//!   条目本身不记 deadline)—— PC 永不回首帧时手机不悬挂、条目不泄漏。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::mapref::one::Ref;
use dashmap::DashMap;
use everlasting_remote_protocol::{Frame, StreamEvent};
use tokio::sync::{mpsc, oneshot};

/// 单条在途请求的回复通道(P2-2 修订:S1 直接按最终形态落地)。
pub enum PendingReply {
    /// 非流式:PC 回 `Response` 帧时 send 进来。
    Oneshot(oneshot::Sender<Frame>),
    /// SSE 流式:PC 的 `Stream` 帧(Chunk/End/Error)持续 send
    /// (S3 实例化)。
    Stream(mpsc::Sender<StreamEvent>),
}

/// 一条在途请求的完整条目:回复通道 + 发起它的隧道连接。
///
/// `conn_id`(P1-2)供离线清理按**连接级**精确匹配 —— 与
/// `TunnelRegistry::remove_if_current` 同款思路:重复 node_id 踢旧 /
/// PC 重连的窗口期里,旧连接退出清理不得误杀新连接已在服务的流。
pub struct PendingEntry {
    /// 发起本请求的 WSS 连接的 `ConnHandle::conn_id`。
    pub conn_id: u64,
    pub reply: PendingReply,
}

/// `request_id → PendingEntry` 表 + id 生成器。
///
/// 手写 `Debug`(Sender 无 Debug;RemoteState derive(Debug) 需要),
/// 只打印在途条数。
pub struct PendingTable {
    inner: DashMap<u64, PendingEntry>,
    next_id: AtomicU64,
    /// 单条 Oneshot 等待超时(proxy handler 用)。放表内而非模块 const:
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

    pub fn insert(&self, id: u64, conn_id: u64, reply: PendingReply) {
        self.inner.insert(id, PendingEntry { conn_id, reply });
    }

    /// 只读查(Stream 多 chunk 路径用:每次 Chunk 命中同一条目,不能
    /// remove-once)。返回 DashMap `Ref` guard —— **持 guard 期间不得
    /// `.await`**(try_send 是同步的,调用方照此约定,P1-1 修订)。
    pub fn get(&self, id: u64) -> Option<Ref<'_, u64, PendingEntry>> {
        self.inner.get(&id)
    }

    /// 移除并取回复通道(proxy 清理 / ws 接收循环路由)。
    pub fn remove(&self, id: u64) -> Option<PendingReply> {
        self.inner.remove(&id).map(|(_, e)| e.reply)
    }

    /// node 离线清理(P1-2):按 **conn_id** 扫所有 `PendingReply::Stream`
    /// 条目,送 `Error{node_offline}` 进 mpsc 关闭手机 SSE body 后移除。
    ///
    /// **只动 Stream,不碰 Oneshot**(Oneshot 靠 60s 超时兜底,design
    /// §2.3 —— MVP 不做在途非流式请求的即时清理)。返回清理条数
    /// (ws.rs 离线路径打日志用)。try_send 失败(rx 已 drop,手机 body
    /// 已结束)不 panic,直接移除即可。
    pub fn cancel_streams_for_conn(&self, conn_id: u64) -> usize {
        let ids: Vec<u64> = self
            .inner
            .iter()
            .filter(|e| e.value().conn_id == conn_id)
            .filter(|e| matches!(e.value().reply, PendingReply::Stream(_)))
            .map(|e| *e.key())
            .collect();
        let mut count = 0usize;
        for id in ids {
            if let Some((_, entry)) = self.inner.remove(&id) {
                if let PendingReply::Stream(tx) = entry.reply {
                    let _ = tx.try_send(StreamEvent::Error {
                        message: "node_offline".to_string(),
                    });
                    count += 1;
                }
            }
        }
        count
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
        t.insert(7, 0, PendingReply::Oneshot(tx));
        assert_eq!(t.len(), 1);
        assert!(matches!(t.remove(7), Some(PendingReply::Oneshot(_))));
        assert!(t.remove(7).is_none());
        assert!(t.is_empty());
    }

    /// conn_id 存取:insert 记下,get 能读到。
    #[test]
    fn conn_id_stored_and_readable() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (tx, _rx) = mpsc::channel(4);
        t.insert(1, 42, PendingReply::Stream(tx));
        let entry = t.get(1).expect("entry present");
        assert_eq!(entry.conn_id, 42);
        assert!(matches!(entry.reply, PendingReply::Stream(_)));
    }

    /// get 不 remove:连续两次 get 都命中(Stream 多 chunk 的前提)。
    #[test]
    fn get_does_not_remove() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (tx, _rx) = mpsc::channel(4);
        t.insert(1, 0, PendingReply::Stream(tx));
        assert!(t.get(1).is_some());
        assert!(t.get(1).is_some(), "get 必须只读,不能消耗条目");
        assert_eq!(t.len(), 1);
    }

    /// cancel_streams_for_conn 只清指定 conn 的 Stream 条目:
    /// 其他 conn 的 Stream、同 conn 的 Oneshot 都不动。
    #[test]
    fn cancel_streams_for_conn_scoped_to_conn_and_stream() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (_tx1, rx1) = mpsc::channel::<StreamEvent>(4);
        t.insert(1, 7, PendingReply::Stream(_tx1));
        let (_tx2, _rx2) = mpsc::channel::<StreamEvent>(4);
        t.insert(2, 8, PendingReply::Stream(_tx2)); // 别的 conn
        let (tx3, _rx3) = oneshot::channel();
        t.insert(3, 7, PendingReply::Oneshot(tx3)); // 同 conn 但 Oneshot

        assert_eq!(t.cancel_streams_for_conn(7), 1);
        assert!(t.get(1).is_none(), "conn 7 的 Stream 应被清掉");
        assert!(t.get(2).is_some(), "conn 8 的 Stream 不应被误伤");
        assert!(t.get(3).is_some(), "Oneshot 靠 60s 超时,不应被清");
    }

    /// 手机 body 已结束(rx drop)时清理:try_send 失败但条目照常移除,
    /// 不 panic。
    #[test]
    fn cancel_with_dead_receiver_still_removes() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (tx, rx) = mpsc::channel::<StreamEvent>(4);
        t.insert(1, 9, PendingReply::Stream(tx));
        drop(rx); // 手机已断开
        assert_eq!(t.cancel_streams_for_conn(9), 1);
        assert!(t.is_empty());
    }

    /// 离线清理的 Error 能送进还活着的 mpsc(手机 body 未断)。
    #[test]
    fn cancel_delivers_node_offline_error() {
        let t = PendingTable::new(Duration::from_secs(60));
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(4);
        t.insert(1, 3, PendingReply::Stream(tx));
        assert_eq!(t.cancel_streams_for_conn(3), 1);
        let ev = tokio::runtime::Runtime::new()
            .expect("rt")
            .block_on(rx.recv())
            .expect("error event");
        assert!(matches!(ev, StreamEvent::Error { message } if message == "node_offline"));
    }

    #[test]
    fn default_timeout_is_60s() {
        let t = PendingTable::new(Duration::from_secs(60));
        assert_eq!(t.timeout, Duration::from_secs(60));
    }
}
