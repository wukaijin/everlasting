//! [`TunnelManager`] —— tunnel 生命周期的统一入口(design §2.4 Q-T4)。
//!
//! 挂在 `AppState`(新字段),由 daemon bin `main` 启动 supervisor;Tauri
//! GUI(Thin/Full)只持有空壳,从不 `start()` → 不 spawn 任何 tunnel task
//! (P1-1 修订:tunnel 只活在 daemon 进程)。
//!
//! ## 职责
//!
//! - **config 驱动**:`watch::Receiver<Option<TunnelConfig>>`(`None` = 停)。
//!   配置变 → supervisor 停旧 task(graceful,等其退出)→ 按新配置拉新
//!   task。daemon 启动也走同一条路径(初始 config 从 DB 读,`set_config`
//!   seed 到 watch channel)。
//! - **shutdown**:`stop()` cancel 内部 `CancellationToken` → supervisor
//!   退出 → 当前 task 关 WSS。`server::shutdown_signal` 调它(先停 tunnel
//!   再 drain)。
//! - **状态查询**:`get_tunnel_status` IPC 读 [`TunnelStatus`](connected /
//!   remote_url / node_id / last_error)。
//! - **RPC 通道**:连接存活期间注册 `mpsc::UnboundedSender<Frame>`(配对码
//!   生成等 PC → remote 内部 RPC 的出口),`pending` 表把 `Frame::Response`
//!   按 id 路由回等待方(design §2.5)。
//!
//! ## 失败隔离
//!
//! supervisor / client task 全独立 tokio task:连接失败只 log + 退避重连,
//! 不 panic、不 crash daemon(硬约束 3)。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use everlasting_remote_protocol::Frame;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::client;
use super::{TunnelConfig, RPC_TIMEOUT, TUNNEL_TARGET};

/// 停旧 task 的优雅退出窗口;超时后 abort 兜底(避免 supervisor 被一个
/// 卡死的连接永久阻塞)。
const STOP_GRACE: Duration = Duration::from_secs(3);

/// 当前 tunnel 连接的状态快照(design §3.1 `get_tunnel_status`)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelStatus {
    /// 是否已连上 remote。
    pub connected: bool,
    /// 配置的 remote 基地址(最后一次成功连接的配置)。
    pub remote_url: Option<String>,
    /// 本机 node_id(remote 侧注册名)。
    pub node_id: Option<String>,
    pub display_name: Option<String>,
    /// 最后一次失败原因:`"auth_failed"` = shared_secret 校验失败(已停止
    /// 重连);`None` = 无失败或已恢复(design §6.2)。
    pub last_error: Option<String>,
}

/// 正在运行的 tunnel task(handle + 它的取消令牌)。
struct RunningTunnel {
    handle: JoinHandle<()>,
    token: CancellationToken,
}

/// 进程级 tunnel 管理器(design §2.4)。`Arc` 持有,`AppState` 共享。
pub struct TunnelManager {
    /// 期望配置(`None` = 停);supervisor 消费,IPC `set_config` 生产。
    config_tx: watch::Sender<Option<TunnelConfig>>,
    config_rx: watch::Receiver<Option<TunnelConfig>>,
    /// supervisor task 句柄(`start()` 幂等)。
    supervisor: Mutex<Option<JoinHandle<()>>>,
    /// 进程 shutdown 信号(daemon shutdown 时由 `server::shutdown_signal`
    /// 经 [`TunnelManager::stop`] 触发)。
    shutdown: CancellationToken,
    /// dispatcher 打 loopback 用的本地端口(main 里
    /// `parse_port_from_args` 的结果传入,不硬编码 7456 —— Q-T6)。
    local_port: AtomicU16,
    /// 当前连接的出站帧通道(`None` = 未连接)。配对 RPC 等经它发帧。
    conn: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    /// 等待 `Frame::Response` 的 RPC 等待方(id → oneshot)。
    pending: Mutex<HashMap<u64, oneshot::Sender<Frame>>>,
    /// RPC 帧 id 分配器(进程内单调)。
    next_rpc_id: AtomicU64,
    /// 状态快照(`get_tunnel_status` 读)。
    status: Mutex<TunnelStatus>,
}

impl TunnelManager {
    /// 新建管理器(初始配置 `None` = 停,supervisor 未启动)。
    pub fn new() -> Arc<Self> {
        let (config_tx, config_rx) = watch::channel(None);
        Arc::new(Self {
            config_tx,
            config_rx,
            supervisor: Mutex::new(None),
            shutdown: CancellationToken::new(),
            local_port: AtomicU16::new(crate::daemon::server::DEFAULT_DAEMON_PORT),
            conn: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_rpc_id: AtomicU64::new(1),
            status: Mutex::new(TunnelStatus::default()),
        })
    }

    /// 启动 supervisor(幂等)。**只有 daemon bin main 调用**(P1-1)。
    pub fn start(self: &Arc<Self>) {
        let mut guard = self.supervisor.lock().unwrap();
        if guard.is_some() {
            return;
        }
        let me = self.clone();
        *guard = Some(tokio::spawn(supervisor_loop(me)));
    }

    /// 进程 shutdown:取消内部令牌 → supervisor 停当前 task、退出。
    pub fn stop(&self) {
        self.shutdown.cancel();
    }

    /// 设置 dispatcher 的 loopback 端口(main 的 `parse_port_from_args` 结果)。
    pub fn set_local_port(&self, port: u16) {
        self.local_port.store(port, Ordering::Relaxed);
    }

    pub fn local_port(&self) -> u16 {
        self.local_port.load(Ordering::Relaxed)
    }

    /// 更新期望配置(`None` = 停)。supervisor 收到后重启 tunnel task。
    pub fn set_config(&self, cfg: Option<TunnelConfig>) {
        let _ = self.config_tx.send(cfg);
    }

    /// 当前期望配置(`None` = 未配置/已停)。
    pub fn current_config(&self) -> Option<TunnelConfig> {
        self.config_rx.borrow().clone()
    }

    pub fn status(&self) -> TunnelStatus {
        self.status.lock().unwrap().clone()
    }

    /// 覆盖状态快照(supervisor spawn 新 task 前用它预置目标地址)。
    pub fn set_status(&self, status: TunnelStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// 连接建立后由 client 调用:登记出站通道 + 状态 connected。
    pub fn register_conn(&self, tx: mpsc::UnboundedSender<Frame>, cfg: &TunnelConfig) {
        *self.conn.lock().unwrap() = Some(tx);
        *self.status.lock().unwrap() = TunnelStatus {
            connected: true,
            remote_url: Some(cfg.remote_url.clone()),
            node_id: Some(cfg.node_id.clone()),
            display_name: Some(cfg.display_name.clone()),
            last_error: None,
        };
    }

    /// 连接退出后由 client 调用:清出站通道 + 状态 disconnected
    /// (保留 remote_url/node_id 供前端展示)。
    pub fn clear_conn(&self) {
        *self.conn.lock().unwrap() = None;
        self.status.lock().unwrap().connected = false;
    }

    /// shared_secret 校验失败(已停止重连)—— 状态置 `auth_failed`
    /// (design §6.2 前端 status 显示)。
    pub fn mark_auth_failed(&self, cfg: &TunnelConfig) {
        *self.status.lock().unwrap() = TunnelStatus {
            connected: false,
            remote_url: Some(cfg.remote_url.clone()),
            node_id: Some(cfg.node_id.clone()),
            display_name: Some(cfg.display_name.clone()),
            last_error: Some("auth_failed".to_string()),
        };
    }

    /// 当前连接的出站帧通道(未连接 → `None`)。
    pub fn conn_tx(&self) -> Option<mpsc::UnboundedSender<Frame>> {
        self.conn.lock().unwrap().clone()
    }

    /// 登记一个等待 `Frame::Response` 的 RPC 等待方(配对码生成用)。
    pub fn pending_insert(&self, id: u64, tx: oneshot::Sender<Frame>) {
        self.pending.lock().unwrap().insert(id, tx);
    }

    /// 取出并消费 id 对应的等待方(client serve loop 收到 Response 时调)。
    pub fn pending_remove(&self, id: u64) -> Option<oneshot::Sender<Frame>> {
        self.pending.lock().unwrap().remove(&id)
    }

    fn next_rpc_id(&self) -> u64 {
        self.next_rpc_id.fetch_add(1, Ordering::Relaxed)
    }

    /// PC → remote 的内部 RPC(design §2.5):经当前连接发 `Frame::Request`,
    /// 等 `Frame::Response`(id 关联,`pending` 表路由)。
    ///
    /// 错误均为人类可读消息(IPC 层转 `AppCommandError`):
    /// - 未连接 → "remote 未连接"
    /// - 发送失败(连接刚断)→ "连接已断开"
    /// - 超时(`RPC_TIMEOUT`)→ "未在 15 秒内响应"
    pub async fn send_rpc_and_wait(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<Frame, String> {
        let tx = self
            .conn_tx()
            .ok_or_else(|| "remote 未连接,请先在设置中配置并等待连接".to_string())?;
        let id = self.next_rpc_id();
        let (done_tx, done_rx) = oneshot::channel();
        self.pending_insert(id, done_tx);
        let frame = Frame::Request {
            id,
            method: method.to_string(),
            path: path.to_string(),
            headers: Vec::new(),
            body,
        };
        if tx.send(frame).is_err() {
            self.pending_remove(id);
            return Err("remote 连接已断开,请重试".to_string());
        }
        match tokio::time::timeout(RPC_TIMEOUT, done_rx).await {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(_)) => {
                self.pending_remove(id);
                Err("remote 连接已断开,请重试".to_string())
            }
            Err(_) => {
                self.pending_remove(id);
                Err(format!(
                    "remote 未在 {} 秒内响应,请重试",
                    RPC_TIMEOUT.as_secs()
                ))
            }
        }
    }
}

/// supervisor:watch config → 停旧 / 拉新 task;shutdown → 全停退出。
async fn supervisor_loop(me: Arc<TunnelManager>) {
    let mut rx = me.config_rx.clone();
    let mut running: Option<RunningTunnel> = None;
    let mut current: Option<TunnelConfig> = None;

    loop {
        let cfg = rx.borrow_and_update().clone();
        if cfg != current {
            if let Some(r) = running.take() {
                stop_and_wait(r).await;
            }
            current = cfg.clone();
            if let Some(cfg) = cfg {
                // spawn 前先更新状态快照:连接中(connected=false)但目标
                // URL 已是新配置 —— 断连/退避期间前端 `get_tunnel_status`
                // 显示准确的目标地址,而不是旧连接的残留。
                me.set_status(TunnelStatus {
                    connected: false,
                    remote_url: Some(cfg.remote_url.clone()),
                    node_id: Some(cfg.node_id.clone()),
                    display_name: Some(cfg.display_name.clone()),
                    last_error: None,
                });
                let token = CancellationToken::new();
                let handle = tokio::spawn(client::run_tunnel(cfg, me.clone(), token.clone()));
                running = Some(RunningTunnel { handle, token });
                tracing::debug!(target: TUNNEL_TARGET, "tunnel task spawned");
            }
        }

        tokio::select! {
            _ = me.shutdown.cancelled() => {
                if let Some(r) = running.take() {
                    stop_and_wait(r).await;
                }
                tracing::info!(target: TUNNEL_TARGET, "tunnel manager stopped");
                return;
            }
            res = rx.changed() => {
                if res.is_err() {
                    // config sender 全 drop(manager 即将销毁)
                    if let Some(r) = running.take() {
                        stop_and_wait(r).await;
                    }
                    return;
                }
            }
        }
    }
}

/// 优雅停一个 tunnel task:先 cancel 令牌,`STOP_GRACE` 内等它退出;
/// 超时 abort 兜底(避免卡死的连接阻塞 supervisor)。
async fn stop_and_wait(r: RunningTunnel) {
    r.token.cancel();
    let mut handle = r.handle;
    tokio::select! {
        _ = &mut handle => {}
        _ = tokio::time::sleep(STOP_GRACE) => {
            handle.abort();
            tracing::warn!(target: TUNNEL_TARGET, "tunnel task did not exit within grace, aborted");
        }
    }
}
