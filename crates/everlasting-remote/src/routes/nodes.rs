//! 节点状态 API(implement.md Step 8)。
//!
//! `GET /api/v1/nodes`(带 `require_device_token` middleware):手机首页
//! 的已配对 PC 卡片列表。当前单 token → 单 node(devices.token 绑定
//! 一个 node_id),返数组形态为将来多设备留形(PRD:手机首页 = 卡片
//! 列表)。
//!
//! status 来源:nodes 表(`online`/`offline`),由 WSS 注册(upsert)/
//! 心跳/断开/心跳超时维护(Step 5);`last_seen_at` = 最后 Pong 时刻。

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::AuthenticatedDevice;
use crate::config::RemoteState;
use crate::db;
use crate::error::AppError;

/// 单节点卡片字段(design §3.3 表 + 在线状态)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub node_id: String,
    pub display_name: String,
    /// `NODE_STATUS_ONLINE` / `NODE_STATUS_OFFLINE`。
    pub status: String,
    /// 最后在线时刻(unix epoch ms,心跳 Pong 刷新)。
    pub last_seen_at: i64,
}

/// 挂载 `GET /api/v1/nodes` + device_token 中间件(与 proxy 同款)。
pub fn router(state: Arc<RemoteState>) -> Router {
    Router::new()
        .route("/api/v1/nodes", get(list_nodes))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_device_token,
        ))
        .with_state(state)
}

/// 该 token 绑定的 node 列表(design-step3.md §10:复用
/// `get_device_by_token`(auth 中间件已查)+ `get_node`,不新增 SQL)。
pub async fn list_nodes(
    State(state): State<Arc<RemoteState>>,
    Extension(device): Extension<AuthenticatedDevice>,
) -> Result<Json<Vec<NodeInfo>>, AppError> {
    let Some(node) = db::crud::get_node(&state.db, &device.node_id)
        .await
        .map_err(|e| AppError::server(format!("node lookup failed: {e}")))?
    else {
        // device → node 引用断了(理论不可达:devices.node_id 有外键
        // 且 node 行无删除路径)。防御性返空数组,不 500。
        tracing::warn!(node_id = %device.node_id, "device bound to missing node");
        return Ok(Json(vec![]));
    };
    Ok(Json(vec![NodeInfo {
        node_id: node.id,
        display_name: node.display_name,
        status: node.status,
        last_seen_at: node.last_seen_at,
    }]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;
    use crate::db::pool;
    use crate::db::schema;
    use crate::db::NODE_STATUS_ONLINE;
    use crate::pending::PendingTable;
    use crate::ratelimit::RateLimiter;
    use crate::tunnel_registry::{HeartbeatConfig, TunnelRegistry};
    use axum::http::StatusCode;
    use std::time::Duration;
    use tower::ServiceExt;

    async fn test_state() -> Arc<RemoteState> {
        let dir = Box::leak(Box::new(tempfile::tempdir().expect("tempdir")));
        let config = RemoteConfig {
            port: 0,
            db_path: dir.path().join("remote.db"),
            shared_secret: "test".into(),
        };
        let pool = pool::init_pool(&config.db_path).await.expect("init pool");
        schema::run_migrations(&pool).await.expect("migrations");
        Arc::new(RemoteState {
            db: pool,
            shared_secret: config.shared_secret,
            node_connections: Arc::new(TunnelRegistry::new()),
            heartbeat: HeartbeatConfig::default(),
            pending: Arc::new(PendingTable::new(Duration::from_secs(60))),
            pairing_ratelimit: Arc::new(RateLimiter::new(1000, Duration::from_secs(60))),
        })
    }

    /// 预置 node + device,返回 token。
    async fn seed(state: &RemoteState, status: &str) -> String {
        db::crud::upsert_node(&state.db, "pc-1", "公司 PC")
            .await
            .expect("upsert node");
        if status != NODE_STATUS_ONLINE {
            db::crud::update_node_status(&state.db, "pc-1", status, db::now_ms())
                .await
                .expect("set status");
        }
        let token = "b".repeat(64);
        db::crud::insert_device(&state.db, &token, "pc-1", Some("test phone"))
            .await
            .expect("insert device");
        token
    }

    async fn list(
        state: &Arc<RemoteState>,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state.clone());
        let mut req = axum::http::Request::builder()
            .method("GET")
            .uri("/api/v1/nodes")
            .body(axum::body::Body::empty())
            .expect("build");
        if let Some(t) = token {
            req.headers_mut().insert(
                "authorization",
                format!("Bearer {t}").parse().expect("header"),
            );
        }
        let resp = app.oneshot(req).await.expect("oneshot");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        (status, json)
    }

    #[tokio::test]
    async fn requires_device_token() {
        let state = test_state().await;
        let (status, body) = list(&state, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["category"], "Auth");
    }

    /// 在线节点:返回绑定 node 的卡片,status online。
    #[tokio::test]
    async fn returns_bound_node_with_status() {
        let state = test_state().await;
        let token = seed(&state, NODE_STATUS_ONLINE).await;

        let (status, body) = list(&state, Some(&token)).await;
        assert_eq!(status, StatusCode::OK);
        let nodes = body.as_array().expect("array");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["nodeId"], "pc-1");
        assert_eq!(nodes[0]["displayName"], "公司 PC");
        assert_eq!(nodes[0]["status"], "online");
        assert!(nodes[0]["lastSeenAt"].as_i64().unwrap() > 0);
    }

    /// 离线节点:status offline(心跳/断开维护的状态如实反映)。
    #[tokio::test]
    async fn returns_offline_status() {
        let state = test_state().await;
        let token = seed(&state, "offline").await;
        let (status, body) = list(&state, Some(&token)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["status"], "offline");
    }

    /// 未知 token → 401(middleware 层;此处验证 wire)。
    #[tokio::test]
    async fn unknown_token_rejected() {
        let state = test_state().await;
        let (status, _) = list(&state, Some(&"c".repeat(64))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
