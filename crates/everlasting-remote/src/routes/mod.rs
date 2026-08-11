//! remote 自己的 `/api/v1/*` 路由装配(design §1.2 / §3.1)。
//!
//! 与 daemon `daemon/routes/mod.rs` 的角色相同:把每个领域 route 模块
//! 组装进一个 `Router`。Step 2 还没有 `/api/v1/*` 领域路由(health 由
//! `server::build_router` 单独挂,见下);Step 4-8 依次挂
//! pairing / nodes / proxy。
//!
//! 命名空间:`/api/v1/proxy/*` 前缀隔离 remote 自身 API 与透传的 PC
//! daemon API(决策 Q-P1),故这里永远只挂 remote 自己的端点。
//!
//! 注意:health 不在本函数注册 —— 与 daemon 约定一致(health 双路径
//! `/health` + `/api/v1/health` 都挂在 `server::build_router`,这里
//! 再挂会撞 axum 的 overlapping route panic)。

use axum::Router;

pub mod health;

/// 装配 remote 的 `/api/v1/*` 领域路由。无状态 —— remote 的
/// `Arc<RemoteState>` 在 Step 3 引入后改为 `router(state)` 签名
/// (见实施计划 Step 3 `config.rs::RemoteState`)。
pub fn router() -> Router {
    Router::new()
}
