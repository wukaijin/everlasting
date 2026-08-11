//! axum router 装配 + remote serve loop(design §1.2 / 对齐 daemon
//! `daemon/server.rs`)。
//!
//! [`serve_remote`] 是 `everlasting-remote` bin 的唯一入口:
//! 1. Bind `0.0.0.0:PORT`(云服务器 nginx 反代到本进程,WSL 无关;
//!    与 daemon 相同显式 `0.0.0.0` 绑定)。
//! 2. `axum::serve(...).with_graceful_shutdown(...)` —— Ctrl+C / SIGTERM
//!    走优雅退出。
//!
//! # Graceful shutdown(与 daemon 的差异)
//!
//! daemon 的 `shutdown_signal` 要关 SSE 注册表 + drain agent loop;
//! remote S1 没有这些 —— 只有 WSS 长连接(Step 5 才落地)。Step 5 时
//! 在 shutdown 前加 `tunnel_registry` 清理(关所有 PC 连接),此处
//! Step 2 先只做 axum 的默认 drain。**不要**给整个 serve future 套
//! `tokio::time::timeout` —— daemon 2026-07-27 的自杀 bug 就出在这
//! (见 `daemon/server.rs::serve_daemon` 注释,教训复用)。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::services::{ServeDir, ServeFile};

use crate::config::RemoteState;
use crate::routes;

/// Build the un-mounted remote router。`health` 在顶层(design §3.1
/// 双路径),其余 `/api/v1/*` 由 [`routes::router`] 装配;未匹配路径
/// 落到 ServeDir 的 SPA fallback(P1-3 —— remote 伺服 PWA 静态文件,
/// PC daemon 的 ServeDir 在 NAT 后手机够不到)。
///
/// 共享 state 的注入方式与 daemon 一致:**`routes::router(state)` 参数
/// 传入**,domain 模块内部 `.with_state(state.clone())`(顶层 `Router<()>`
/// 不能 `with_state`,health / ServeDir fallback 都是无状态服务)。
/// Step 5/6 给 `RemoteState` 加 node_connections / pending 后,handler
/// 直接 `State<Arc<RemoteState>>` 提取,router 装配不再改动。
///
/// 无 `CorsLayer`(对比 daemon):手机 PWA 由 remote 自己伺服,同源,
/// 无跨域需求;nginx 反代也不引入跨域。若未来前端独立部署再补。
pub fn build_router(state: Arc<RemoteState>) -> Router {
    let mut router = Router::new()
        .route("/health", get(routes::health::health))
        .route("/api/v1/health", get(routes::health::health))
        .merge(routes::router(state));

    // P1-3:SPA history-mode fallback(`ServeDir` + `not_found_service` 指向
    // index.html),与 daemon P2.4 D4 同款。dist 缺失时退化为纯 API 服务
    // (dev 模式 / daemon-only 部署)。
    match resolve_dist_dir() {
        Some(dist) => {
            tracing::info!(dist = %dist.display(), "serving static frontend from dist dir");
            let spa =
                ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html")));
            router = router.fallback_service(spa);
        }
        None => {
            tracing::info!(
                "no dist dir found (EVERLASTING_REMOTE_DIST_DIR unset + default \
                 absent); remote runs as API-only"
            );
        }
    }
    router
}

/// Resolve the frontend static-asset directory for `ServeDir`(P1-3)。
///
/// 解析顺序:
/// 1. `EVERLASTING_REMOTE_DIST_DIR` env(运维/测试覆盖,绝对或相对路径)。
/// 2. 默认:`./dist`(相对**进程 CWD**)。remote 是独立部署的服务器二进制,
///    部署形态 = scp 二进制 + `app/dist/` 到同一目录(design §6.1),
///    不像 daemon 有 sidecar staging 的多深度问题 —— 不需要 walk-up
///    查找逻辑。
///
/// 返回 `None` 表示纯 API 模式(前端未构建 / remote-only 部署)。
pub fn resolve_dist_dir() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("EVERLASTING_REMOTE_DIST_DIR") {
        let p = PathBuf::from(raw);
        if p.is_dir() {
            return Some(p);
        }
        tracing::debug!(
            dist = %p.display(),
            "EVERLASTING_REMOTE_DIST_DIR set but not a directory; ignoring"
        );
    }
    let default = PathBuf::from("dist");
    default.is_dir().then_some(default)
}

/// Bind + serve remote on `0.0.0.0:PORT`。
///
/// # Graceful shutdown
///
/// 收到 Ctrl+C (SIGINT) 或 SIGTERM (POSIX) 后,`shutdown_signal` 返回,
/// axum drain 所有 in-flight 请求。Step 2 无长连接(SSE/WSS 都未落地),
/// drain 亚秒完成。Step 5 在此处加 `tunnel_registry` 主动断开 PC 连接。
pub async fn serve_remote(state: Arc<RemoteState>, port: u16) -> std::io::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "everlasting-remote listening");

    let router = build_router(state);
    let serve = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal());

    serve.await?;
    tracing::info!("everlasting-remote shutdown complete");
    Ok(())
}

/// Graceful shutdown signal handler。Ctrl+C(portable)+ SIGTERM(unix)。
/// Windows 只 Ctrl+C(与 daemon 约定一致)。
///
/// Step 2 无 shutdown 前置动作;Step 5 在这里关 WSS 连接(踢所有 PC
/// daemon,避免 drain 被永不完成的长连接卡住 —— daemon SSE 的教训)。
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// P1-3:`EVERLASTING_REMOTE_DIST_DIR` 指向存在的目录时生效。
    /// 用 tempfile 避免耦合构建机 `app/dist/` 的存在性。
    /// env 只在单测内 set + remove,`resolve_dist_dir` 的其他路径分支
    /// (默认 `./dist`)依赖 CWD,不读这个 env —— 并行安全。
    #[test]
    fn dist_dir_env_override_when_dir_exists() {
        let tmp = tempdir().expect("create tempdir");
        std::env::set_var("EVERLASTING_REMOTE_DIST_DIR", tmp.path());
        let resolved = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_REMOTE_DIST_DIR");
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    /// 指向不存在路径的 env 被忽略(落到默认分支;默认是否存在取决于
    /// 运行 CWD,只断言不 panic —— 与 daemon 同款测试)。
    #[test]
    fn dist_dir_env_ignored_when_not_a_dir() {
        std::env::set_var(
            "EVERLASTING_REMOTE_DIST_DIR",
            "/nonexistent/path/that/does/not/exist",
        );
        let _ = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_REMOTE_DIST_DIR");
    }

    /// `serve_remote` 在无信号时必须持续服务(bind + health 200),且
    /// 不会自发退出 —— daemon 2026-07-27 自杀 bug 的同款守卫(remote
    /// 从一开始就不犯)。不发信号,用 abort 收尾。
    #[tokio::test]
    async fn serve_remote_serves_health_without_signal() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        // 预占 ephemeral port,取出后立即 drop,交给 serve_remote 重绑。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        // Step 3:serve_remote 需要 `Arc<RemoteState>`(tempdir db + migration)。
        let dir = tempfile::tempdir().expect("create tempdir");
        let config = crate::config::RemoteConfig {
            port: 0,
            db_path: dir.path().join("remote.db"),
            shared_secret: "test".into(),
        };
        let state = crate::config::RemoteState::load(&config)
            .await
            .expect("state loads");

        let mut serve_handle = tokio::spawn(serve_remote(state, port));

        // 等 remote 起来(轮询 health)。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("remote did not become healthy in 5s");
            }
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                let req =
                    b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(req).await;
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await;
                if buf.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 核心断言:serve_handle 未自发 resolve(没有自杀 bug)。
        match tokio::time::timeout(std::time::Duration::from_millis(100), &mut serve_handle).await {
            Ok(res) => panic!(
                "serve_remote returned on its own without a signal: {res:?} — \
                 it must serve indefinitely until SIGINT/SIGTERM"
            ),
            Err(_) => { /* still pending — 正确 */ }
        }

        serve_handle.abort();
    }
}
