//! 配对码生命周期(design §2.3 / implement.md Step 7)。
//!
//! 两条通路:
//!
//! ```text
//! 1. 生成(PC 经 WSS 触发):Frame::Request{path: /internal/pairing/generate}
//!      → handle_internal_rpc(ws.rs 接收循环调用)→ 6 位码落库(60s
//!        过期,绑定请求方 node_id)→ Frame::Response 回 PC
//! 2. 兑换(手机 HTTP):POST /api/v1/pairing/redeem{code, device_name}
//!      → per-IP 限速(P2-3,10/min)→ db::redeem_pairing_code(事务:
//!        校验未过期未用 → 签发 64 hex token → 落 devices → 标 used)
//!      → {device_token, node_id, node_display_name}
//! ```
//!
//! 配对码来源语义(design §2.3 注):生成**动作**由 PC 触发,但**落库**
//! 在 remote —— remote 是 single source of truth,避免双端状态不一致。
//!
//! **redeem 不挂 device_token 中间件**(配对时还没有 token);错误映射
//! (design §3.4):过期/已用/不存在 → 400 `invalid_or_expired_code`;
//! 限速超限 → 429 `RateLimit`。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::routing::post;
use axum::{Json, Router};
use everlasting_remote_protocol::Frame;
use serde::{Deserialize, Serialize};

use crate::config::RemoteState;
use crate::db::crud::{self, RedeemError};
use crate::db::now_ms;
use crate::error::AppError;
use crate::ratelimit::RateLimiter;

/// redeem 限速(design §2.3 P2-3:10 次/分钟)。
pub const REDEEM_RATE_LIMIT_MAX: u32 = 10;
pub const REDEEM_RATE_LIMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// 配对码有效期(design §2.3 / PRD:60s,一次性)。
pub const PAIRING_CODE_TTL_MS: i64 = 60_000;

/// 挂载 `POST /api/v1/pairing/redeem`(无 token 中间件 —— 配对时
/// 还没有 token)。
pub fn router(state: Arc<RemoteState>) -> Router {
    Router::new()
        .route("/api/v1/pairing/redeem", post(redeem))
        .with_state(state)
}

/// 手机 redeem 请求体(design §2.3)。
#[derive(Debug, Deserialize)]
pub struct RedeemRequest {
    /// 6 位配对码(PC 屏幕上念给用户)。
    pub code: String,
    /// 设备名("Carlos 的 iPhone",可空 —— 缺省落空串)。
    #[serde(default)]
    pub device_name: String,
}

/// redeem 成功响应(design §3.4 字段原样)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemedResponse {
    pub device_token: String,
    pub node_id: String,
    pub node_display_name: String,
}

/// 手机 redeem(HTTP)。限速优先于码校验(防暴力扫)。
pub async fn redeem(
    State(state): State<Arc<RemoteState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<RedeemRequest>,
) -> Result<Json<RedeemedResponse>, AppError> {
    if !state.pairing_ratelimit.allow(addr.ip()) {
        tracing::warn!(ip = %addr.ip(), "pairing redeem rate limit exceeded");
        return Err(AppError::rate_limit("redeem attempts too frequent"));
    }

    let redeemed = crud::redeem_pairing_code(&state.db, &body.code, &body.device_name)
        .await
        .map_err(|e| match e {
            // design §3.4:过期/已用/不存在统一 400
            RedeemError::InvalidOrExpiredCode => {
                AppError::invalid_request("invalid_or_expired_code")
            }
            RedeemError::Db(e) => AppError::server(format!("redeem db error: {e}")),
        })?;

    tracing::info!(
        node_id = %redeemed.node_id,
        "pairing code redeemed, device registered"
    );
    Ok(Json(RedeemedResponse {
        device_token: redeemed.device_token,
        node_id: redeemed.node_id,
        node_display_name: redeemed.node_display_name,
    }))
}

/// PC 经 WSS 发来的 internal RPC 分派(design §2.3.1 P3-2;ws.rs 接收
/// 循环调用)。已知路径返回 `Frame::Response`(回 PC),未知路径返回
/// None(调用方记 warn 日志)。只依赖 `node_id`(配对码绑定请求方 PC),
/// 不触 conn sink —— 测试无需构造真实连接。
///
/// 返回的 Response 帧 body 是 JSON:`{code, expires_in}`(生成成功)或
/// `{error}`(生成失败)。status:200 成功 / 500 失败(撞码 retry 耗尽
/// 或 DB 错误 —— 1/1M 量级,但 PC 端在等响应,不能挂 60s 超时)。
pub async fn handle_internal_rpc(
    state: &Arc<RemoteState>,
    node_id: &str,
    id: u64,
    path: &str,
    _body: &[u8],
) -> Option<Frame> {
    match path {
        "/internal/pairing/generate" => {
            let expires_at = now_ms() + PAIRING_CODE_TTL_MS;
            match crud::generate_and_store_pairing_code(&state.db, node_id, expires_at).await {
                Ok(code) => {
                    tracing::info!(node_id, "pairing code generated");
                    Some(rpc_response(
                        id,
                        200,
                        serde_json::json!({ "code": code, "expires_in": PAIRING_CODE_TTL_MS / 1000 }),
                    ))
                }
                Err(e) => {
                    tracing::error!(node_id, error = %e, "pairing code generation failed");
                    Some(rpc_response(id, 500, serde_json::json!({ "error": e.to_string() })))
                }
            }
        }
        _ => {
            tracing::warn!(path, "unknown internal RPC");
            None
        }
    }
}

/// internal RPC 的 Response 帧(JSON body + Content-Type)。
fn rpc_response(id: u64, status: u16, value: serde_json::Value) -> Frame {
    Frame::Response {
        id,
        status,
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: value.to_string().into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RemoteConfig;
    use crate::db::pool;
    use crate::db::schema;
    use crate::pending::PendingTable;
    use crate::tunnel_registry::{HeartbeatConfig, TunnelRegistry};
    use axum::http::StatusCode;
    use sqlx::Row;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;
    use tower::ServiceExt;

    /// 构造测试 state(限速器可注入小值;心跳/pending 用宽松默认)。
    async fn test_state(ratelimit: RateLimiter) -> Arc<RemoteState> {
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
            pairing_ratelimit: Arc::new(ratelimit),
        })
    }

    fn test_addr() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345)
    }

    /// 预置 node + 配对码,返回码。
    async fn seed_code(state: &RemoteState) -> String {
        crud::upsert_node(&state.db, "pc-1", "公司 PC")
            .await
            .expect("upsert node");
        crud::generate_and_store_pairing_code(&state.db, "pc-1", now_ms() + PAIRING_CODE_TTL_MS)
            .await
            .expect("generate code")
    }

    /// oneshot 打 redeem 路由(手动塞 ConnectInfo —— oneshot 不走
    /// into_make_service_with_connect_info)。
    async fn redeem_request(
        state: &Arc<RemoteState>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let app = router(state.clone());
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/pairing/redeem")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("build request");
        let mut req = req;
        req.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(test_addr()));
        let resp = app.oneshot(req).await.expect("oneshot");
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        (status, json)
    }

    // ---- redeem HTTP ----

    #[tokio::test]
    async fn redeem_success_returns_token() {
        let state = test_state(RateLimiter::new(100, Duration::from_secs(60))).await;
        let code = seed_code(&state).await;

        let (status, body) = redeem_request(
            &state,
            serde_json::json!({ "code": code, "device_name": "Carlos 的 iPhone" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["nodeId"], "pc-1");
        assert_eq!(body["nodeDisplayName"], "公司 PC");
        let token = body["deviceToken"].as_str().expect("token").to_string();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

        // devices 表有行
        let device = crud::get_device_by_token(&state.db, &token)
            .await
            .expect("query")
            .expect("device row");
        assert_eq!(device.node_id, "pc-1");
        assert_eq!(device.display_name.as_deref(), Some("Carlos 的 iPhone"));
    }

    /// 过期/已用/不存在 → 400 invalid_or_expired_code(design §3.4)。
    #[tokio::test]
    async fn redeem_invalid_code_returns_400() {
        let state = test_state(RateLimiter::new(100, Duration::from_secs(60))).await;

        // 不存在的码
        let (status, body) =
            redeem_request(&state, serde_json::json!({ "code": "999999", "device_name": "d" }))
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["message"], "invalid_or_expired_code");

        // 重复 redeem(先成功一次)
        let code = seed_code(&state).await;
        redeem_request(&state, serde_json::json!({ "code": code, "device_name": "d" }))
            .await;
        let (status, _) =
            redeem_request(&state, serde_json::json!({ "code": code, "device_name": "d" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // 过期码
        let expired_code = crud::generate_and_store_pairing_code(
            &state.db,
            "pc-1",
            now_ms() - 1, // 已过期
        )
        .await
        .expect("expired code");
        let (status, _) = redeem_request(
            &state,
            serde_json::json!({ "code": expired_code, "device_name": "d" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// 限速:max=2 的限速器 → 第 3 次 429 RateLimit。
    #[tokio::test]
    async fn redeem_rate_limited_after_max_attempts() {
        let state = test_state(RateLimiter::new(2, Duration::from_secs(60))).await;
        // 前 2 次:码无效(400)也消耗配额 —— 限速在码校验之前
        let (s1, _) =
            redeem_request(&state, serde_json::json!({ "code": "000000", "device_name": "d" }))
                .await;
        let (s2, _) =
            redeem_request(&state, serde_json::json!({ "code": "000000", "device_name": "d" }))
                .await;
        assert_eq!(s1, StatusCode::BAD_REQUEST);
        assert_eq!(s2, StatusCode::BAD_REQUEST);

        let (s3, body) =
            redeem_request(&state, serde_json::json!({ "code": "000000", "device_name": "d" }))
                .await;
        assert_eq!(s3, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["category"], "RateLimit");
    }

    /// 不同 IP 不共享配额(限速按 IP 独立)。
    #[tokio::test]
    async fn redeem_ratelimit_is_per_ip() {
        let state = test_state(RateLimiter::new(1, Duration::from_secs(60))).await;
        let app = router(state.clone());
        let send = |ip: SocketAddr| {
            let app = app.clone();
            async move {
                let req = axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/pairing/redeem")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(r#"{"code":"000000","device_name":"d"}"#))
                    .expect("build");
                let mut req = req;
                req.extensions_mut().insert(ConnectInfo::<SocketAddr>(ip));
                app.oneshot(req).await.expect("oneshot").status()
            }
        };
        let ip_a = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 1);
        let ip_b = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 1);
        assert_eq!(send(ip_a).await, StatusCode::BAD_REQUEST);
        assert_eq!(send(ip_a).await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(send(ip_b).await, StatusCode::BAD_REQUEST, "其他 IP 独立配额");
    }

    // ---- internal RPC ----

    /// 生成:返回 Response 帧(200 + code + expires_in),库中绑定请求方 node。
    #[tokio::test]
    async fn internal_generate_returns_code_bound_to_node() {
        let state = test_state(RateLimiter::new(100, Duration::from_secs(60))).await;
        crud::upsert_node(&state.db, "pc-1", "公司 PC")
            .await
            .expect("upsert");

        let frame = handle_internal_rpc(&state, "pc-1", 7, "/internal/pairing/generate", b"")
            .await
            .expect("known rpc returns response");
        let Frame::Response { id, status, body, .. } = frame else {
            panic!("expected Response frame");
        };
        assert_eq!(id, 7);
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let code = json["code"].as_str().expect("code").to_string();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(json["expires_in"], 60);

        // 库中该码绑定 pc-1
        let row = sqlx::query("SELECT node_id, used FROM pairing_codes WHERE code = ?")
            .bind(&code)
            .fetch_one(&state.db)
            .await
            .expect("code row");
        assert_eq!(row.get::<String, _>("node_id"), "pc-1");
        assert_eq!(row.get::<i64, _>("used"), 0);
    }

    /// 未知 internal 路径 → None(调用方记 warn)。
    #[tokio::test]
    async fn unknown_internal_rpc_returns_none() {
        let state = test_state(RateLimiter::new(100, Duration::from_secs(60))).await;
        let frame = handle_internal_rpc(&state, "pc-1", 1, "/internal/nope", b"").await;
        assert!(frame.is_none());
    }
}
