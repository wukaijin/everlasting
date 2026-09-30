//! everlasting-acp —— ACP(Agent Client Protocol)shim。
//!
//! 被编辑器(Zed 等 ACP 客户端)spawn 的子进程:stdin/stdout 按行 JSON-RPC
//! (ACP v1 stable,crate `agent-client-protocol` 锚 1.x),stderr 留日志
//! —— **stdout 只准协议帧,这是 ACP 硬约束**,tracing 输出必须钉死在 stderr。
//!
//! 架构 = 第五客户端形态的瘦翻译层:本进程不跑 agent core,只把 ACP 面
//! 翻译到已运行 daemon 的 HTTP/SSE 接口(daemon 侧零改动,PRD R2)。
//! PR1 覆盖生命周期(initialize / session/new / load / list);
//! SSE 消费 + prompt 驱动(PR2)、权限环 + cancel(PR3)随后。

mod daemon;
mod handlers;
mod permission;
mod sse;
mod translate;

use std::process::ExitCode;

use crate::daemon::{DaemonClient, DEFAULT_DAEMON_URL};

/// 解析 daemon 地址:`EVERLASTING_ACP_DAEMON_URL` → `DAEMON_URL` → 默认本机 7456。
/// Zed `agent_servers` 的 env 数组里配前者;后者兼容 evl 的既有约定。
fn daemon_url_from_env() -> String {
    std::env::var("EVERLASTING_ACP_DAEMON_URL")
        .or_else(|_| std::env::var("DAEMON_URL"))
        .unwrap_or_else(|_| DEFAULT_DAEMON_URL.to_string())
}

/// tracing → **stderr only**(ACP 规定 stdout 只准 JSON-RPC 帧)。
/// 级别走 `RUST_LOG`(默认 info);ANSI 关掉,编辑器日志面板是纯文本。
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let url = daemon_url_from_env();
    let client = match DaemonClient::new(&url) {
        Ok(client) => client,
        Err(err) => {
            // 客户端构造失败(TLS 后端初始化类)连 serve 都进不去,只能退出;
            // 指引照打,与 initialize 的结构化错误同一文案。
            eprintln!("everlasting-acp: {err}");
            eprintln!("start the daemon with `./scripts/daemon.sh start`, then reconnect");
            return ExitCode::FAILURE;
        }
    };

    // 启动期健康检查:只管 stderr 提示,**不退进程** —— 继续进入 serve,
    // 让 initialize 给客户端结构化错误(implement.md PR1;daemon 可能稍后才起)。
    match client.health().await {
        Ok(health) => tracing::info!(
            daemon_url = %url,
            daemon_id = %health.daemon_id,
            daemon_version = %health.daemon_version,
            "daemon reachable"
        ),
        Err(err) => tracing::warn!(
            "daemon health check failed ({err}); initialize will report a structured \
             error — start the daemon with `./scripts/daemon.sh start`"
        ),
    }

    // SSE 任务「启动即挂、全程保持」(PRD R2 / design §4):规避 daemon 对
    // 零订阅者 permission ask 的快拒(缺口 1 MVP 规避)。连接失败不退进程,
    // 循环内指数退避重连;prompt 前置检查连接状态,断线不静默丢帧。
    let sse = sse::SseHandle::spawn(client.clone());

    if let Err(err) = handlers::serve(handlers::App::new(client, sse)).await {
        // connect_to 返回 = stdin EOF(正常关停)或传输/处理错误。
        tracing::error!("ACP connection terminated: {err}");
        return ExitCode::FAILURE;
    }
    tracing::info!("stdin closed, shutting down");
    ExitCode::SUCCESS
}
