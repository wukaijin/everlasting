//! WSS 隧道注册表(design §2.1 / implement.md Step 5)。
//!
//! `node_id → Arc<ConnHandle>` 映射:PC daemon 连上后注册,手机反向代理
//! (Step 6)按 token 解析出的 node_id 找连接。`Arc` 句柄廉价克隆,
//! 多任务(心跳 ping / 代理转发 / 踢旧 / shutdown)各自持句柄并发发帧,
//! sink 内部 `Mutex` 串行化写。
//!
//! **只存 sink(remote → PC 发送方向)**:接收方向由 ws.rs 的接收循环
//! 单独持有 stream,不进注册表。`TunnelConn` 的"持 SplitSink +
//! SplitStream"在注册表侧 = 只 sink + 元数据(implement.md 表述的
//! 落地形态)。
//!
//! **踢旧语义(design §2.1 "重复 node_id → 新连接踢旧")**:注册时若同
//! node_id 已有连接,`register` 返回旧句柄,调用方(ws.rs)给旧连接发
//! `Close`;旧连接的接收循环收到 Close(或兜底超时检查发现已被替换)
//! 自然退出。`remove_if_current` 保证清理路径只动自己的连接 ——
//! 被替换的旧连接退出时**不会**误删新连接或误标 offline。

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use dashmap::DashMap;
use futures_util::{stream::SplitSink, SinkExt};
use tokio::sync::Mutex;

use crate::db::now_ms;

/// 单个 PC 隧道连接的共享句柄。
pub struct ConnHandle {
    /// 进程内唯一连接序号。清理路径用它区分"我是不是当前注册的连接"
    /// (`remove_if_current`)—— 防止旧连接的退出逻辑误伤新连接。
    pub conn_id: u64,
    /// PC daemon 自报的稳定 node_id(design §2.1:remote 不派生,接受自报)。
    pub node_id: String,
    /// "公司 PC" / "家里 PC"(PC 自报,可空)。
    pub display_name: String,
    /// 发送方向(remote → PC)。多发送方(心跳 / 代理 / 踢旧)经 Mutex 串行。
    pub sink: Mutex<SplitSink<WebSocket, Message>>,
    /// 最后收到 Pong 的时刻(unix epoch ms)。注册时初始化为 now;
    /// 接收循环收到 Pong 时更新;心跳 task 读它判超时(90s 无 pong → 离线)。
    pub last_pong_ms: AtomicI64,
}

impl ConnHandle {
    /// 发一帧(JSON Text)。写失败 = 连接已死,调用方自行决定处理
    /// (心跳退出 / 代理返 502)。
    pub async fn send_frame(
        &self,
        frame: &everlasting_remote_protocol::Frame,
    ) -> Result<(), axum::Error> {
        let text = serde_json::to_string(frame).expect("Frame 序列化不可能失败");
        self.sink.lock().await.send(Message::Text(text)).await
    }

    /// 心跳 ping(design §2.4:30s 一次)。payload 任意 <125B,空即可。
    pub async fn ping(&self) -> Result<(), axum::Error> {
        self.sink.lock().await.send(Message::Ping(Vec::new())).await
    }

    /// 主动关闭(踢旧 / shutdown / 超时清理)。PC 侧收到 Close 后回
    /// Close,接收循环自然退出。
    pub async fn close(&self) -> Result<(), axum::Error> {
        self.sink.lock().await.send(Message::Close(None)).await
    }

    /// 距上次 Pong 已超过 `timeout` 毫秒?
    pub fn stale_by(&self, timeout_ms: i64) -> bool {
        now_ms() - self.last_pong_ms.load(Ordering::Relaxed) > timeout_ms
    }
}

/// `node_id → 隧道连接` 注册表。
///
/// 手写 `Debug`(`ConnHandle` 里的 `SplitSink` 无 Debug;只打印长度,
/// RemoteState 的 derive(Debug) 需要它)。
pub struct TunnelRegistry {
    conns: DashMap<String, Arc<ConnHandle>>,
    next_conn_id: AtomicU64,
}

impl std::fmt::Debug for TunnelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelRegistry")
            .field("tunnels", &self.conns.len())
            .finish()
    }
}

impl Default for TunnelRegistry {
    fn default() -> Self {
        Self {
            conns: DashMap::new(),
            next_conn_id: AtomicU64::new(0),
        }
    }
}

impl TunnelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配进程内唯一 conn_id(ws.rs 构造 ConnHandle 用)。
    pub fn next_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// 注册(覆盖旧连接)。返回**被替换的旧句柄** —— 调用方应给它发
    /// Close 踢掉(design §2.1 "新连接踢旧")。
    pub fn register(&self, node_id: String, handle: Arc<ConnHandle>) -> Option<Arc<ConnHandle>> {
        self.conns.insert(node_id, handle)
    }

    /// 取句柄(廉价 Arc clone,不持 guard)。
    pub fn get(&self, node_id: &str) -> Option<Arc<ConnHandle>> {
        self.conns.get(node_id).map(|e| e.value().clone())
    }

    /// 仅当注册表里的连接仍是 `conn_id` 时移除 —— 清理路径专用,
    /// 防止旧连接退出时误删被新连接占用的条目。
    pub fn remove_if_current(&self, node_id: &str, conn_id: u64) -> Option<Arc<ConnHandle>> {
        self.conns
            .remove_if(node_id, |_, h| h.conn_id == conn_id)
            .map(|(_, h)| h)
    }

    /// 无条件移除(当前无并发替换场景需要它;保留给诊断/测试)。
    pub fn remove(&self, node_id: &str) -> Option<Arc<ConnHandle>> {
        self.conns.remove(node_id).map(|(_, h)| h)
    }

    /// 注册表里是否仍是 `conn_id` 的连接(接收循环兜底检查用)。
    pub fn is_current(&self, node_id: &str, conn_id: u64) -> bool {
        self.conns.get(node_id).map(|e| e.value().conn_id) == Some(conn_id)
    }

    pub fn len(&self) -> usize {
        self.conns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.conns.is_empty()
    }

    /// shutdown:给所有连接发 Close(server.rs 优雅退出时调)。
    /// 先收集句柄再逐个发(不持 DashMap guard 跨 await)。
    pub async fn close_all(&self) {
        let handles: Vec<Arc<ConnHandle>> = self.conns.iter().map(|e| e.value().clone()).collect();
        for h in handles {
            if let Err(e) = h.close().await {
                tracing::debug!(node_id = %h.node_id, error = %e, "close on shutdown failed (conn already dead)");
            }
        }
        self.conns.clear();
    }

    /// 在线节点数(诊断用)。
    pub fn online_count(&self) -> usize {
        self.conns.len()
    }
}

/// 心跳参数(design §2.4:30s ping / 90s 无 pong 判离线)。放
/// `RemoteState` 而非模块 const:测试用小间隔构造 state(秒级参数
/// 无法在测试里等 90s),生产走 `Default`。
#[derive(Debug, Clone, Copy)]
pub struct HeartbeatConfig {
    /// ping 间隔。
    pub ping_interval: Duration,
    /// 无 pong 判离线阈值。
    pub timeout: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            timeout: Duration::from_secs(90),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表纯逻辑可测的部分。**有 socket 的路径**(注册/替换/踢旧/
    /// 心跳超时)在 routes/ws.rs 集成测试里用真实 WebSocket 覆盖 ——
    /// `ConnHandle` 的 sink 是 axum `SplitSink`,单测无法凭空构造,
    /// 这里只测不依赖句柄的簿记。
    ///
    /// 覆盖说明:register 替换语义 / remove_if_current 防误删 / is_current
    /// 分别在 ws.rs 的 `duplicate_node_id_kicks_old_conn` /
    /// `heartbeat_timeout_marks_node_offline` / `clean_disconnect_marks_offline`
    /// 集成测试里以真实连接断言。

    #[test]
    fn conn_id_monotonic() {
        let reg = TunnelRegistry::new();
        let a = reg.next_conn_id();
        let b = reg.next_conn_id();
        assert!(b > a);
    }

    #[test]
    fn default_heartbeat_is_30s_90s() {
        let cfg = HeartbeatConfig::default();
        assert_eq!(cfg.ping_interval, Duration::from_secs(30));
        assert_eq!(cfg.timeout, Duration::from_secs(90));
    }
}
