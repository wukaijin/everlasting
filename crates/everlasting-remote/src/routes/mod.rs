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

/// 装配 remote 的 `/api/v1/*` 领域路由。签名带 `Arc<RemoteState>`(与
/// daemon `daemon/routes/mod.rs::router` 同款)——
/// **state 注入方式 = daemon 模式**:每个 domain 模块(Step 4+ 的
/// pairing / nodes / proxy)在各自 `router()` 内建好路由后
/// `.with_state(state.clone())` 产出 `Router<()>` 再 merge 进来;
/// 顶层不做 `with_state`(axum 0.7 里 `Router<()>.with_state(state)`
/// 类型不匹配,health / ServeDir fallback 都是无状态服务)。
///
/// Step 3 尚无带 state 的 handler,`_state` 参数仅为锁定签名 ——
/// 后续 step 直接用,不改调用点。
pub fn router(_state: Arc<RemoteState>) -> Router {
    Router::new()
}
