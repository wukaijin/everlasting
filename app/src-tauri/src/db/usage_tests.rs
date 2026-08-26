//! 08-20-turn-usage-event-quota-view WP2 — `db::usage` 聚合测试(AC4)。

#![cfg(test)]

use super::test_support::test_pool;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::usage::usage_window;
use crate::db::trace::upsert_turn_trace_token;
use crate::llm::types::TokenUsage;

async fn seed_provider(pool: &SqlitePool, name: &str) -> String {
    let row =
        crate::db::providers::create_provider(pool, name, name, "https://example.com", "sk-x")
            .await
            .unwrap();
    row.id
}

async fn seed_session(pool: &SqlitePool) -> String {
    let row = crate::db::create_session(
        pool,
        &Uuid::new_v4().to_string(),
        crate::projects::DEFAULT_PROJECT_ID,
        "/tmp",
        "test-model",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    row.id
}

fn usage(input: u32, output: u32) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        context_input_tokens: input,
    }
}

async fn age_row(pool: &SqlitePool, session_id: &str, seq: i64, modifier: &str) {
    sqlx::query(
        "UPDATE turn_trace SET created_at = datetime('now', ?) WHERE session_id = ? AND seq = ?",
    )
    .bind(modifier)
    .bind(session_id)
    .bind(seq)
    .execute(pool)
    .await
    .unwrap();
}

/// AC4:跨 5h 边界数据(新鲜主行 / 新鲜 worker run 行 / 窗口外旧行 /
/// NULL 归因行 / 双 provider)→ per-provider 累计 + 主-worker 拆分 +
/// 窗口滚动 + unknown 桶,全部与手工构造值一致。
#[tokio::test]
async fn usage_window_aggregates_splits_and_rolls_off() {
    let pool = test_pool().await;
    let prov_a = seed_provider(&pool, "provider-a").await;
    let prov_b = seed_provider(&pool, "provider-b").await;
    let sid1 = seed_session(&pool).await;

    // 主行(provider A,新鲜):input 1000。
    upsert_turn_trace_token(
        &pool,
        &sid1,
        "",
        1,
        &usage(1000, 100),
        Some(500),
        None,
        None,
        None,
        Some(200),
        Some(200_000),
        Some(&prov_a),
    )
    .await
    .unwrap();
    // worker run 行(provider A,新鲜):input 300 —— 计入 worker 拆分。
    upsert_turn_trace_token(
        &pool,
        &sid1,
        "run-uuid-1",
        2,
        &usage(300, 30),
        Some(400),
        None,
        None,
        None,
        Some(100),
        Some(200_000),
        Some(&prov_a),
    )
    .await
    .unwrap();
    // 主行(provider A,7h 前):input 9000 —— 必须滑出窗口。
    upsert_turn_trace_token(
        &pool,
        &sid1,
        "",
        3,
        &usage(9000, 900),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&prov_a),
    )
    .await
    .unwrap();
    age_row(&pool, &sid1, 3, "-7 hours").await;
    // 主行(provider B,新鲜):input 200。
    upsert_turn_trace_token(
        &pool,
        &sid1,
        "",
        4,
        &usage(200, 20),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&prov_b),
    )
    .await
    .unwrap();
    // NULL 归因行(新鲜):input 50 —— unknown 桶。
    upsert_turn_trace_token(
        &pool,
        &sid1,
        "",
        5,
        &usage(50, 5),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let report = usage_window(&pool, None, 5, None, 10).await.unwrap();
    assert_eq!(report.window_hours, 5);
    assert_eq!(report.providers.len(), 3, "a + b + unknown");

    let a = report
        .providers
        .iter()
        .find(|p| p.provider_id == prov_a)
        .unwrap();
    assert_eq!(a.display_name.as_deref(), Some("provider-a"));
    assert_eq!(
        a.main_totals.input_tokens, 1000,
        "old row rolled off; only fresh main row"
    );
    assert_eq!(a.worker_totals.input_tokens, 300);
    assert_eq!(a.totals.input_tokens, 1300, "totals = main + worker");
    assert_eq!(a.totals.output_tokens, 130);

    let b = report
        .providers
        .iter()
        .find(|p| p.provider_id == prov_b)
        .unwrap();
    assert_eq!(b.main_totals.input_tokens, 200);
    assert_eq!(b.worker_totals.input_tokens, 0);

    let unknown = report
        .providers
        .iter()
        .find(|p| p.provider_id == "unknown")
        .unwrap();
    assert_eq!(unknown.main_totals.input_tokens, 50);
    assert!(unknown.display_name.is_none());

    // top sessions:sid1 窗口内 main 1250 + worker 300(旧行不计)。
    assert_eq!(report.top_sessions.len(), 1);
    let ts = &report.top_sessions[0];
    assert_eq!(ts.session_id, sid1);
    assert_eq!(ts.window_main_input, 1000 + 200 + 50);
    assert_eq!(ts.window_worker_input, 300);

    // provider 过滤:只见 A。
    let only_a = usage_window(&pool, Some(&prov_a), 5, None, 10)
        .await
        .unwrap();
    assert_eq!(only_a.providers.len(), 1);
    assert_eq!(only_a.providers[0].provider_id, prov_a);
}

/// AC4 小时分布 + AC6 session 累计同源。
#[tokio::test]
async fn usage_window_hourly_and_session_totals() {
    let pool = test_pool().await;
    let prov = seed_provider(&pool, "prov-hourly").await;
    let sid = seed_session(&pool).await;

    upsert_turn_trace_token(
        &pool,
        &sid,
        "",
        1,
        &usage(700, 70),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&prov),
    )
    .await
    .unwrap();
    upsert_turn_trace_token(
        &pool,
        &sid,
        "run-1",
        2,
        &usage(80, 8),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&prov),
    )
    .await
    .unwrap();
    // 第二桶:2h 前。
    upsert_turn_trace_token(
        &pool,
        &sid,
        "",
        3,
        &usage(200, 20),
        None,
        None,
        None,
        None,
        None,
        None,
        Some(&prov),
    )
    .await
    .unwrap();
    age_row(&pool, &sid, 3, "-2 hours").await;

    let report = usage_window(&pool, None, 5, Some(5000), 10).await.unwrap();
    let p = &report.providers[0];
    assert!(p.hourly.len() >= 2, "two distinct hour buckets");
    // 升序 + 各桶 input 之和 = 总量(700+80 新鲜 + 200 @2h 前)。
    let sum: i64 = p.hourly.iter().map(|h| h.input_tokens).sum();
    assert_eq!(sum, 980);

    // AC6:session 全周期累计 = turn_trace 全量聚合(980 input 全在
    // 窗口内;旧行已滑出窗口但本例无旧行)。
    let ts = &report.top_sessions[0];
    assert_eq!(
        ts.lifetime_input, 980,
        "lifetime = full-history turn_trace sum"
    );
    assert_eq!(ts.lifetime_output, 98);
    assert_eq!(report.limit_tokens, Some(5000));
}
