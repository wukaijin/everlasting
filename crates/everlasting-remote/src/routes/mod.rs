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

use std::sync::Arc;

use axum::Router;

use crate::config::RemoteState;

pub mod health;
pub mod ws;

/// 装配 remote 的领域路由(design §1.2 / §3.1)。签名带
/// `Arc<RemoteState>`(与 daemon `daemon/routes/mod.rs::router` 同款)
/// —— **state 注入方式 = daemon 模式**:每个带 state 的 domain 模块
/// 在各自 `router()` 内 `.with_state(state)` 产出 `Router<()>` 再
/// merge 进来;顶层不做 `with_state`(axum 0.7 里 `Router<()>.with_state`
/// 参数类型是当前 state,不匹配)。
///
/// 命名空间:`/api/v1/proxy/*` 前缀隔离 remote 自身 API 与透传的 PC
/// daemon API(决策 Q-P1),故这里只挂 remote 自己的端点 + `/ws`。
///
/// 注意:health 不在此注册 —— 与 daemon 约定一致(health 双路径
/// `/health` + `/api/v1/health` 都挂在 `server::build_router`,这里
/// 再挂会撞 axum 的 overlapping route panic)。
pub fn router(state: Arc<RemoteState>) -> Router {
    Router::new().merge(ws::router(state))
}
