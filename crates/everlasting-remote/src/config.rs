//! CLI + env 解析(design §3.6)。
//!
//! 优先级:**CLI flag > env > default**。default 只有 port / db-path 有;
//! `shared_secret` **无默认,必传**(Q-S1 决策 —— 缺 secret 启动即失败,
//! 强制安全,防裸跑)。
//!
//! 解析逻辑拆成纯函数(`resolve_port` / `resolve_db_path` / `resolve_secret`,
//! env 值作为参数传入),方便单测校验优先级链,不碰进程 env。

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use sqlx::SqlitePool;

use crate::pending::PendingTable;
use crate::tunnel_registry::{HeartbeatConfig, TunnelRegistry};

/// 默认监听端口(design §3.6;`7457` 与 daemon 的 7456 错开)。
pub const DEFAULT_REMOTE_PORT: u16 = 7457;

/// 默认 SQLite 路径:`~/.local/share/dev.everlasting.remote/remote.db`
/// (固定 `dev.everlasting.remote` 子目录,与 daemon 的 app identifier
/// 解耦 —— design §4.1:remote 不用 `EVERLASTING_APP_IDENTIFIER`)。
const DEFAULT_DB_SUBDIR: &str = ".local/share/dev.everlasting.remote/remote.db";

/// 环境变量名(design §3.6)。与 CLI flag 同名语义,prefix `EVERLASTING_REMOTE_`。
pub const ENV_PORT: &str = "EVERLASTING_REMOTE_PORT";
pub const ENV_DB_PATH: &str = "EVERLASTING_REMOTE_DB_PATH";
pub const ENV_SECRET: &str = "EVERLASTING_REMOTE_SECRET";

/// `everlasting-remote` CLI(design §3.6)。
///
/// 三个 flag 全部可缺省:`--port` / `--db-path` 有 default,
/// `--shared-secret` 的缺省由 [`RemoteConfig::from_cli`] 在 env 里找,
/// 仍无则报 [`ConfigError::MissingSecret`]。
#[derive(Parser, Debug)]
#[command(
    name = "everlasting-remote",
    version,
    about = "Edge daemon for the remote-control epic: WSS server + device table + reverse proxy to PC daemons over WSS tunnels."
)]
pub struct Cli {
    /// 监听端口(默认 7457;env: EVERLASTING_REMOTE_PORT)
    #[arg(long)]
    pub port: Option<u16>,
    /// SQLite 数据库路径(默认 ~/.local/share/dev.everlasting.remote/remote.db;env: EVERLASTING_REMOTE_DB_PATH)
    #[arg(long)]
    pub db_path: Option<PathBuf>,
    /// WSS 握手共享密钥,必传(env: EVERLASTING_REMOTE_SECRET)
    #[arg(long)]
    pub shared_secret: Option<String>,
}

/// 解析完成的运行配置。`main.rs` 用它启动服务。
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    pub port: u16,
    pub db_path: PathBuf,
    pub shared_secret: String,
}

/// axum 共享运行状态(对应 daemon `AppState` 角色;design §7 对齐)。
///
/// 字段按 step 演进添加(design-step3.md §6,与 implement.md 字面的
/// `{ db, shared_secret, node_connections, pending }` 最终形态一致):
/// - Step 3 落地:`db` + `shared_secret`
/// - Step 5 加:`node_connections`(WSS 注册表)+ `heartbeat`(心跳参数)
/// - Step 6 加:`pending`(request_id → PendingReply,在途请求表)
#[derive(Debug)]
pub struct RemoteState {
    pub db: SqlitePool,
    pub shared_secret: String,
    /// `node_id → 隧道连接` 注册表(design §2.1;Step 5 加)。
    pub node_connections: Arc<TunnelRegistry>,
    /// 心跳参数(design §2.4:30s ping / 90s 判离线)。放 state 而非
    /// 模块 const:测试用小间隔构造 state,生产走 `Default`。
    pub heartbeat: HeartbeatConfig,
    /// 在途请求表(design §3.2.1;Step 6 加)—— proxy 登记,
    /// ws 接收循环按 Response 帧 id 路由回来。
    pub pending: Arc<PendingTable>,
}

impl RemoteState {
    /// 单点初始化:开 pool(WAL/busy_timeout)→ 跑幂等 migration →
    /// 全量置 offline(boot 不变量:重启后无任何隧道连接,陈旧 online
    /// 会让节点 API 误报)→ 包 Arc。`main.rs` 唯一调用;失败即 panic
    /// (DB 不可用无意义继续)。
    pub async fn load(config: &RemoteConfig) -> Result<Arc<Self>, sqlx::Error> {
        let db = crate::db::pool::init_pool(&config.db_path).await?;
        crate::db::schema::run_migrations(&db).await?;
        crate::db::crud::mark_all_offline(&db).await?;
        Ok(Arc::new(Self {
            db,
            shared_secret: config.shared_secret.clone(),
            node_connections: Arc::new(TunnelRegistry::new()),
            heartbeat: HeartbeatConfig::default(),
            pending: Arc::new(PendingTable::new(crate::routes::proxy::PENDING_TIMEOUT)),
        }))
    }
}

/// 配置解析错误。当前只有一个变体;后续(如 db-path 校验)加变体即可。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// Q-S1:secret 无默认必传。`--shared-secret` flag 和
    /// `EVERLASTING_REMOTE_SECRET` env 都缺时启动失败(panic)。
    MissingSecret,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::MissingSecret => write!(
                f,
                "缺少 shared_secret:请传 --shared-secret <SECRET> 或设置 {} 环境变量",
                ENV_SECRET
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl RemoteConfig {
    /// 完整解析:CLI flag + env 回退 + default。secret 双通道都缺 → Err。
    ///
    /// env 读取在函数内完成(调用方无需准备 env);port env 非法值
    /// (非数字)记 warn 并忽略(落到 default),不 fail-loud —— 与 daemon
    /// `resolve_port` 的宽松行为一致。
    pub fn from_cli(cli: Cli) -> Result<Self, ConfigError> {
        let secret = resolve_secret(cli.shared_secret, env_var_nonempty(ENV_SECRET))
            .ok_or(ConfigError::MissingSecret)?;
        Ok(Self {
            port: resolve_port(cli.port, env_port()),
            db_path: resolve_db_path(
                cli.db_path,
                env_var_nonempty(ENV_DB_PATH).map(PathBuf::from),
            ),
            shared_secret: secret,
        })
    }
}

/// Port 解析:**CLI > env > default 7457**(daemon `resolve_port` 同款
/// 优先级链,见 `daemon/server.rs`)。env 值由调用方解析好传入,
/// 纯函数可单测。
pub fn resolve_port(cli_port: Option<u16>, env_port: Option<u16>) -> u16 {
    cli_port.or(env_port).unwrap_or(DEFAULT_REMOTE_PORT)
}

/// db-path 解析:**CLI > env > 默认 `~/.local/share/dev.everlasting.remote/remote.db`**。
pub fn resolve_db_path(cli_path: Option<PathBuf>, env_path: Option<PathBuf>) -> PathBuf {
    cli_path.or(env_path).unwrap_or_else(default_db_path)
}

/// secret 解析:**CLI > env**,双通道都缺 → None(调用方决定失败)。
/// 空字符串按缺失处理(防 `--shared-secret ""` 裸跑)。
pub fn resolve_secret(cli_secret: Option<String>, env_secret: Option<String>) -> Option<String> {
    cli_secret.or(env_secret).filter(|s| !s.is_empty())
}

/// 默认 db 路径:`$HOME/.local/share/dev.everlasting.remote/remote.db`。
/// `HOME` 未设置(容器等极端环境)时退化为相对路径 `.local/share/...`,
/// 不 panic。
fn default_db_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(DEFAULT_DB_SUBDIR)
}

/// 读 env 变量,空字符串按缺失处理(空 env 值没有意义)。
fn env_var_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.is_empty())
}

/// 读 port env,非法数字 warn + 忽略(落到 default)。
fn env_port() -> Option<u16> {
    let raw = env_var_nonempty(ENV_PORT)?;
    match raw.parse() {
        Ok(p) => Some(p),
        Err(_) => {
            tracing::warn!(value = %raw, "{ENV_PORT} 不是合法端口号,忽略");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 两个碰进程 env 的测试(`from_cli` 读真实 env)必须串行,否则
    /// 并行测试会互相污染 `EVERLASTING_REMOTE_SECRET`(cargo test 默认
    /// 多线程,同 daemon `SIGNAL_TEST_MUTEX` 的处理思路)。
    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

    // ---- resolve_* 纯函数:优先级链 ----

    #[test]
    fn port_precedence_cli_env_default() {
        assert_eq!(resolve_port(None, None), DEFAULT_REMOTE_PORT);
        assert_eq!(resolve_port(None, Some(9999)), 9999);
        assert_eq!(resolve_port(Some(7458), Some(9999)), 7458); // CLI 赢
    }

    #[test]
    fn db_path_precedence_cli_env_default() {
        assert_eq!(
            resolve_db_path(
                Some(PathBuf::from("/cli.db")),
                Some(PathBuf::from("/env.db"))
            ),
            PathBuf::from("/cli.db")
        );
        assert_eq!(
            resolve_db_path(None, Some(PathBuf::from("/env.db"))),
            PathBuf::from("/env.db")
        );
        let default = resolve_db_path(None, None);
        // 默认落在 `$HOME/.local/share/dev.everlasting.remote/remote.db`。
        // HOME 只在本测试的进程中读取,不修改 —— 并行安全。
        let home = env::var("HOME").unwrap_or_default();
        assert_eq!(
            default,
            PathBuf::from(home).join(".local/share/dev.everlasting.remote/remote.db")
        );
        assert!(default.ends_with("remote.db"));
    }

    #[test]
    fn secret_precedence_cli_env_and_empty_filtered() {
        assert_eq!(
            resolve_secret(Some("cli".into()), Some("env".into())),
            Some("cli".into())
        );
        assert_eq!(resolve_secret(None, Some("env".into())), Some("env".into()));
        assert_eq!(resolve_secret(Some("".into()), None), None);
        assert_eq!(resolve_secret(None, Some("".into())), None);
        assert_eq!(resolve_secret(None, None), None);
    }

    // ---- from_cli(读真实 env,串行)----

    #[test]
    fn from_cli_cli_values_win_over_env() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        // 设 env 兜底,CLI 显式值应赢(port/db-path/secret 全部 CLI 优先)。
        env::set_var(ENV_PORT, "9999");
        env::set_var(ENV_DB_PATH, "/env.db");
        env::set_var(ENV_SECRET, "env-secret");
        let cli = Cli {
            port: Some(7458),
            db_path: Some(PathBuf::from("/cli.db")),
            shared_secret: Some("cli-secret".into()),
        };
        let cfg = RemoteConfig::from_cli(cli).expect("config resolves");
        env::remove_var(ENV_PORT);
        env::remove_var(ENV_DB_PATH);
        env::remove_var(ENV_SECRET);

        assert_eq!(cfg.port, 7458);
        assert_eq!(cfg.db_path, PathBuf::from("/cli.db"));
        assert_eq!(cfg.shared_secret, "cli-secret");
    }

    #[test]
    fn from_cli_env_values_fill_defaults() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        env::set_var(ENV_PORT, "7777");
        env::set_var(ENV_SECRET, "env-secret");
        let cli = Cli {
            port: None,
            db_path: None,
            shared_secret: None,
        };
        let cfg = RemoteConfig::from_cli(cli).expect("config resolves");
        env::remove_var(ENV_PORT);
        env::remove_var(ENV_SECRET);

        assert_eq!(cfg.port, 7777);
        assert_eq!(cfg.shared_secret, "env-secret");
        // db_path 走 default(HOME 解析在 default_db_path 测试里已覆盖)。
    }

    /// Q-S1:双通道都缺 secret → MissingSecret(调用方 panic)。
    /// 先 remove env,确保本测试不受其他测试/CI 环境残留影响。
    #[test]
    fn from_cli_missing_secret_is_error() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        env::remove_var(ENV_SECRET);
        let cli = Cli {
            port: None,
            db_path: None,
            shared_secret: None,
        };
        assert_eq!(
            RemoteConfig::from_cli(cli).unwrap_err(),
            ConfigError::MissingSecret
        );
    }

    /// 错误消息应同时提示 flag 和 env 两条通道(运维可操作性)。
    #[test]
    fn missing_secret_message_mentions_both_channels() {
        let err = ConfigError::MissingSecret;
        let msg = err.to_string();
        assert!(msg.contains("--shared-secret"));
        assert!(msg.contains(ENV_SECRET));
    }
}
