//! Library crate for the `everlasting-remote` edge daemon(task
//! `08-11-remote-daemon-core`)。跑在云服务器,承担:WSS 服务端(收 PC
//! daemon outbound 连接)+ devices 表 + 配对码生命周期 + 反向代理骨架。
//!
//! **不变量(design §1.3)**:零依赖 daemon 的 `everlasting_lib` —— 不拉
//! libgit2 / agent core / Tauri,ubuntu `cargo build --release` 产出最小
//! 二进制;remote.db 独立于 daemon 的 everlasting.db;帧类型来自
//! `everlasting-remote-protocol`(单源,不复制)。
//!
//! 模块布局(design §1.2):
//! - [`config`]:CLI + env 解析(CLI > env > default;secret 必传)
//! - [`error`]:HTTP 错误契约(5 变体 `ErrorCategory` + `AppError`)
//! - [`server`]:router 装配 + serve loop + ServeDir 静态托管
//! - [`routes`]:`/api/v1/*` 领域路由(Step 2 只 health)

// 见 daemon lib.rs 同名 allow:clippy 1.96 对自然语言 doc 注释的误报。
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]

pub mod config;
pub mod error;
pub mod routes;
pub mod server;
