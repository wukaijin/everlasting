//! S2 tunnel client(2026-08-11, task `08-11-tunnel-client`):PC daemon
//! 连 remote daemon 的 opt-in WSS 长连接 + loopback 转发模块。
//!
//! 架构见 `.trellis/tasks/08-11-tunnel-client/design.md`。核心不变量:
//!
//! 1. **agent core 零改动** —— 本模块(及其子模块)**不得 import**
//!    `agent::` / `tools::` / `provider::` 任何模块。dispatcher 只拿
//!    `Frame::Request` 用 reqwest 打 `http://localhost:{local_port}`,
//!    不调 handler 函数、不绕过 axum(Q7 决策)。
//! 2. **本地功能零依赖 remote** —— 不配 `remote_url` 时
//!    [`TunnelManager`] 不 spawn 任何 task,daemon 行为与现状完全一致。
//! 3. **tunnel 失败不 crash daemon** —— 连接失败只 log + 指数退避重连
//!    (1s → 2s → 4s → … → cap 60s);shared_secret 校验失败(auth 类
//!    错误)停止重连(配置错误,重连无意义,design §6.2)。
//! 4. **tunnel 只活在 daemon 进程**(P1-1 修订)—— 只由
//!    `bin/everlasting-daemon.rs::main` 经 `state.tunnel_manager` 启动;
//!    `lib.rs` 只加 invoke_handler 注册。Thin 模式 GUI 无 DB/server,
//!    tunnel 无从启动;双进程同 node_id 会互踢 flapping。
//!
//! 模块布局(design §1.2 + implement.md):
//! - [`manager`] — [`TunnelManager`](manager::TunnelManager):watch 驱动
//!   的 supervisor,统一管 config 变更 / shutdown / 状态查询(Q-T4)
//! - [`client`] — WSS 连接 + 心跳(remote 主导 ping,客户端只回 pong)+
//!   断线重连循环
//! - [`dispatcher`] — 收 `Frame::Request` → reqwest 打 loopback → 回
//!   `Frame::Response` / `Stream`
//! - [`sse_bridge`] — SSE 响应检测(`Content-Type: text/event-stream`)
//!   → 逐 chunk 转 `Stream::Chunk` → `End` / `Error`;纯字节透传,
//!   chunk 边界不对齐 SSE event 边界没关系(Q-T3)
//! - [`node_id`] — hostname 派生稳定 node_id(remote 记住,重连不变)
//! - [`config`] — `app_config` KV 的 remote 配置读写(零 migration)+
//!   P2-2 的 remote_url 校验 / 规范化
//!
//! 帧类型来自 `everlasting-remote-protocol` crate(帧定义单源,不得复制)。

pub mod client;
pub mod config;
pub mod dispatcher;
pub mod manager;
pub mod node_id;
pub mod sse_bridge;

/// 进程级 tunnel 管理器(AppState 持有,daemon bin 启动)。
pub use manager::TunnelManager;

#[cfg(test)]
mod tests;

/// tracing target:所有 tunnel 日志走这里,`RUST_LOG` 可单独调级别
/// (design §6.1:`everlasting::daemon::tunnel`)。
pub const TUNNEL_TARGET: &str = "everlasting::daemon::tunnel";

/// 心跳 / 断线判定:90s 内未收到任何帧(remote 主导的 Ping 也算)即视为
/// 连接已死(网络分区时 TCP 可能不报错),走断线重连(design §2.4 / PRD)。
pub const HEARTBEAT_STALE_MS: i64 = 90_000;

/// 一次连接的 WSS 握手超时(connect_async 默认无超时,DNS/半开连接可能
/// 挂死;超时后按瞬态错误走退避重连)。
pub const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 指数退避起始延迟(design §6.2:1s → 2s → 4s → … → cap 60s)。
pub const BACKOFF_START: std::time::Duration = std::time::Duration::from_secs(1);
/// 指数退避上限。
pub const BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(60);

/// 配对码 RPC 等待 remote 响应的超时(design §2.5;remote 内部 RPC 是本地
/// DB 操作,秒级完成;15s 已覆盖公网 WSS 往返余量)。
pub const RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// TunnelManager 的当前配置(design §1.2 / §2.4)。
///
/// `remote_url` 是 remote 的**基地址**(如 `wss://remote.example.com`,不含
/// 尾斜杠,无路径/query/fragment —— P2-2 校验)—— 连接 URL 由
/// [`client::build_ws_url`] 拼成 `{remote_url}/ws?secret=...&node_id=...&display_name=...`。
/// `display_name` 只在 query 里传(percent-encoded,P2-1),remote 侧
/// `upsert_node` 会用它刷新节点显示名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelConfig {
    /// remote 基地址,`wss://`(生产)或 `ws://`(本地调试),已去尾斜杠。
    pub remote_url: String,
    /// 与 remote 的 `--shared-secret` 一致(P2-1:query 值 percent-encode)。
    pub shared_secret: String,
    /// 稳定 node_id(hostname 派生 / 持久化 fallback,见 [`node_id`])。
    pub node_id: String,
    /// 节点显示名(默认 = hostname,可存 `app_config "tunnel_display_name"`)。
    pub display_name: String,
}
