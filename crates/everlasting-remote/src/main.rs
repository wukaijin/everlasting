// 见 lib.rs 同名 allow:clippy 1.96 对自然语言 doc 注释的误报。
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]

//! `everlasting-remote` bin entry(task `08-11-remote-daemon-core`,S1)。
//!
//! 云服务器侧的独立二进制(PC daemon 的 tunnel client 是 S2)。与 daemon
//! bin(`everlasting-daemon`)的差别:无 Tauri / sidecar / agent core,
//! CLI 三参数(`--port` / `--db-path` / `--shared-secret`)。
//!
//! ## CLI
//!
//! ```sh
//! everlasting-remote [--port <N>] [--db-path <PATH>] --shared-secret <SECRET>
//! # 或环境变量:
//! #   EVERLASTING_REMOTE_PORT=<N>
//! #   EVERLASTING_REMOTE_DB_PATH=<PATH>
//! #   EVERLASTING_REMOTE_SECRET=<SECRET>
//! ```
//!
//! 优先级:`--flag` > env > default(design §3.6)。`shared_secret` **无
//! 默认必传**(Q-S1 决策)—— flag 和 env 都缺时启动即 panic + 明确报错,
//! 强制安全防裸跑。

use clap::Parser;

use everlasting_remote::config::{Cli, ConfigError, RemoteConfig};
use everlasting_remote::server;

#[tokio::main]
async fn main() {
    // 先初始化 tracing,再解析配置 —— 解析过程中的 warn(如非法端口 env)
    // 能进日志。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,everlasting_remote=debug")
            }),
        )
        .init();

    let cli = Cli::parse();
    let config = match RemoteConfig::from_cli(cli) {
        Ok(config) => config,
        Err(ConfigError::MissingSecret) => {
            // Q-S1:secret 必传,双通道都缺直接 panic(带明确报错)。
            panic!("everlasting-remote 启动失败:{}", ConfigError::MissingSecret);
        }
    };

    tracing::info!(
        port = config.port,
        db_path = %config.db_path.display(),
        "everlasting-remote starting"
    );

    if let Err(e) = server::serve_remote(config.port).await {
        tracing::error!(error = %e, "everlasting-remote failed");
        std::process::exit(1);
    }
}
