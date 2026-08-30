#![cfg(test)]

// web_search 集成测试(AC5 后端行为契约)。与 tests_web_fetch.rs 平级
// 的 httpmock 模式;「先失败 N 次再成功」的有状态序列 httpmock 0.7
// 不支持(无 up_to_n_times),用 axum 迷你服务 + AtomicUsize 计数器
// 实现(axum 已是 lib 依赖,daemon route 测试同款先例)。超时/退避
// 经 `SearchOpts` 注入短值——生产 30s/300ms 没法单测。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use httpmock::prelude::*;
use serde_json::json;

use crate::tools::web_search::{
    ddg::DdgClient, execute_on, resolve_backend_for_test, tavily::TavilyClient, SearchBackend,
    SearchOpts,
};

/// 快速档:超时 100ms / 退避 1ms(重试三连总耗时 < 10ms)。
fn fast_opts() -> SearchOpts {
    SearchOpts {
        timeout: Duration::from_millis(2_000),
        grace: Duration::from_millis(100),
        retry_base_delay: Duration::from_millis(1),
    }
}

fn tavily_backend(base: &str) -> SearchBackend {
    SearchBackend::Tavily(TavilyClient::new(
        "tvly-test-key",
        base,
        Duration::from_secs(2),
    ))
}

fn ddg_backend(base: &str) -> SearchBackend {
    SearchBackend::Ddg(DdgClient::new(base, Duration::from_secs(2)))
}

fn tavily_body(results: serde_json::Value) -> String {
    json!({ "query": "q", "results": results }).to_string()
}

const DDG_FIXTURE: &str = r#"
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2Fch16-00-concurrency.html&amp;rut=8f3a">Rust Book - Fearless Concurrency</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x&amp;rut=y">Threads — Rust's approach to concurrency.</a>
</div>
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="https://tokio.rs/tokio/tutorial">Tokio tutorial</a>
  </h2>
  <a class="result__snippet" href="//d/l/?uddg=x&amp;rut=y">Async runtime docs.</a>
</div>
"#;

// ---------------------------------------------------------------------------
// Tavily
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tavily_200_returns_hits_attribution_and_request_shape() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/search")
            .header("Authorization", "Bearer tvly-test-key")
            .body_contains("\"search_depth\":\"basic\"")
            .body_contains("\"max_results\":7");
        then.status(200)
            .header("content-type", "application/json")
            .body(tavily_body(json!([
                {"title": "T1", "url": "https://a.example/x", "content": "snippet one"},
                {"title": "T2", "url": "https://b.example/y", "content": "snippet two"}
            ])));
    });

    let (out, is_err) = execute_on(
        &tavily_backend(&server.base_url()),
        "rust async",
        7,
        fast_opts(),
    )
    .await;
    assert!(!is_err, "got error: {out}");
    mock.assert_hits(1);
    assert!(out.contains("1. T1"), "{out}");
    assert!(out.contains("https://a.example/x"), "{out}");
    assert!(out.contains("snippet one"), "{out}");
    assert!(out.contains("via tavily at "), "attribution: {out}");
    assert!(out.ends_with("· 2 results -->"), "{out}");
}

#[tokio::test]
async fn tavily_401_is_terminal_no_retry() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST).path("/search");
        then.status(401).body(r#"{"detail":"Invalid key"}"#);
    });

    let (out, is_err) = execute_on(&tavily_backend(&server.base_url()), "q", 5, fast_opts()).await;
    assert!(is_err, "{out}");
    mock.assert_hits(1); // 401 终态:不重试
    assert!(out.contains("API key"), "{out}");
}

#[tokio::test]
async fn tavily_429_retries_then_exhausts() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST).path("/search");
        then.status(429).body(r#"{"detail":"rate limited"}"#);
    });

    let (out, is_err) = execute_on(&tavily_backend(&server.base_url()), "q", 5, fast_opts()).await;
    assert!(is_err, "{out}");
    mock.assert_hits(3); // 1 次原试 + 2 次重试(R5:≤2 次重试)
    assert!(out.contains("429"), "{out}");
}

#[tokio::test]
async fn tavily_432_quota_copy() {
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(POST).path("/search");
        then.status(432).body("{}");
    });
    let (out, is_err) = execute_on(&tavily_backend(&server.base_url()), "q", 5, fast_opts()).await;
    assert!(is_err, "{out}");
    assert!(out.contains("quota"), "{out}");
}

/// 有状态序列(429 → 200):重试后成功。httpmock 0.7 无按次数行为,
/// axum 迷你服务 + 计数器实现(deterministic,无 sleep)。
#[tokio::test(flavor = "multi_thread")]
async fn tavily_429_then_200_retries_to_success() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/search",
            axum::routing::post(move || {
                let hits = hits_clone.clone();
                async move {
                    let n = hits.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            axum::Json(json!({"detail": "slow down"})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            axum::Json(json!({"query": "q", "results": [
                                {"title": "OK", "url": "https://ok.example", "content": "after retry"}
                            ]})),
                        )
                    }
                }
            }),
        );
        let _ = axum::serve(listener, app).await;
    });

    let base = format!("http://{addr}");
    let (out, is_err) = execute_on(&tavily_backend(&base), "q", 5, fast_opts()).await;
    assert!(!is_err, "{out}");
    assert!(out.contains("OK"), "{out}");
    assert!(out.contains("after retry"), "{out}");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

/// 5xx 也在可重试集合(R5):500 → 503 → 200。
#[tokio::test(flavor = "multi_thread")]
async fn tavily_5xx_sequence_retries_to_success() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/search",
            axum::routing::post(move || {
                let hits = hits_clone.clone();
                async move {
                    let n = hits.fetch_add(1, Ordering::SeqCst);
                    let code = match n {
                        0 => 500,
                        1 => 503,
                        _ => 200,
                    };
                    let body = if code == 200 {
                        json!({"query": "q", "results": [
                            {"title": "Recovered", "url": "https://r.example", "content": "c"}
                        ]})
                    } else {
                        json!({"error": "boom"})
                    };
                    let status =
                        StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    (status, axum::Json(body))
                }
            }),
        );
        let _ = axum::serve(listener, app).await;
    });

    let (out, is_err) = execute_on(
        &tavily_backend(&format!("http://{addr}")),
        "q",
        5,
        fast_opts(),
    )
    .await;
    assert!(!is_err, "{out}");
    assert!(out.contains("Recovered"), "{out}");
    assert_eq!(hits.load(Ordering::SeqCst), 3); // 两次重试额度用满后成功
}

// ---------------------------------------------------------------------------
// DDG
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ddg_200_parses_decodes_and_respects_count() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/").query_param("q", "rust 并发");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(DDG_FIXTURE);
    });

    let (out, is_err) = execute_on(
        &ddg_backend(&server.base_url()),
        "rust 并发",
        5,
        fast_opts(),
    )
    .await;
    assert!(!is_err, "{out}");
    mock.assert_hits(1);
    // uddg percent-decode
    assert!(
        out.contains("https://doc.rust-lang.org/book/ch16-00-concurrency.html"),
        "{out}"
    );
    assert!(out.contains("Rust Book - Fearless Concurrency"), "{out}");
    // count=1 只取第一条(同一 query 复用同一 mock)
    let (one, _) = execute_on(
        &ddg_backend(&server.base_url()),
        "rust 并发",
        1,
        fast_opts(),
    )
    .await;
    assert!(one.contains("1. Rust Book"), "{one}");
    assert!(!one.contains("Tokio"), "{one}");
}

#[tokio::test]
async fn ddg_202_rate_limited_not_retried_and_mentions_web_fetch() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/");
        then.status(202)
            .header("content-type", "text/html; charset=utf-8")
            .body("<html>anomaly</html>");
    });

    let (out, is_err) = execute_on(&ddg_backend(&server.base_url()), "q", 5, fast_opts()).await;
    assert!(is_err, "{out}");
    mock.assert_hits(1); // 202 软封锁:不重试(R5)
    assert!(out.contains("web_fetch"), "引导文案: {out}");
    assert!(out.contains("202"), "{out}");
}

#[tokio::test]
async fn ddg_200_zero_results_is_parse_error() {
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body("<html><body>shape changed</body></html>");
    });

    let (out, is_err) = execute_on(&ddg_backend(&server.base_url()), "q", 5, fast_opts()).await;
    assert!(is_err, "{out}");
    assert!(out.contains("0 results"), "{out}");
}

// ---------------------------------------------------------------------------
// 超时路径(注入短预算;design §3:超时提为参数否则没法单测)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timeout_fires_with_injected_short_budget() {
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(POST).path("/search");
        then.status(200)
            .header("content-type", "application/json")
            .delay(Duration::from_secs(2))
            .body(tavily_body(json!([])));
    });

    let opts = SearchOpts {
        timeout: Duration::from_millis(100),
        grace: Duration::from_millis(100),
        retry_base_delay: Duration::from_millis(1),
    };
    let started = std::time::Instant::now();
    let (out, is_err) = execute_on(&tavily_backend(&server.base_url()), "q", 5, opts).await;
    assert!(is_err, "{out}");
    assert!(out.contains("timed out"), "{out}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "整体预算必须兜住慢响应: {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 选路(resolve_backend;AC4 的后端半)
// ---------------------------------------------------------------------------

/// 每测试独立 in-memory 池(app_config 写入不能共享池)。
async fn make_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

async fn store_key(pool: &sqlx::SqlitePool, key: &str) {
    let mk = crate::crypto::derive_master_key().unwrap();
    let enc = crate::crypto::encrypt(&mk, key, crate::tools::web_search::KEY_AAD).unwrap();
    crate::db::set_config_value(pool, crate::tools::web_search::KEY_TAVILY_API_KEY, &enc)
        .await
        .unwrap();
}

async fn make_ctx(pool: &sqlx::SqlitePool) -> crate::tools::ToolContext {
    crate::tools::ToolContext {
        worktree_path: std::path::PathBuf::from("/repo/p"),
        cwd: std::path::PathBuf::from("/repo/p"),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: pool.clone(),
        project_id: String::new(),
        data_dir: std::path::PathBuf::from("/repo"),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
    }
}

#[tokio::test]
async fn routing_auto_without_key_falls_back_to_ddg() {
    let pool = make_pool().await;
    let ctx = make_ctx(&pool).await;
    match resolve_backend_for_test(&ctx).await.unwrap() {
        SearchBackend::Ddg(_) => {}
        other => panic!("auto 无 key 应走 DDG,得到 {:?}", other.name()),
    }
}

#[tokio::test]
async fn routing_auto_with_key_picks_tavily() {
    let pool = make_pool().await;
    store_key(&pool, "tvly-real").await;
    let ctx = make_ctx(&pool).await;
    match resolve_backend_for_test(&ctx).await.unwrap() {
        SearchBackend::Tavily(_) => {}
        other => panic!("auto 有 key 应走 Tavily,得到 {:?}", other.name()),
    }
}

#[tokio::test]
async fn routing_explicit_tavily_without_key_is_actionable_error() {
    let pool = make_pool().await;
    crate::db::set_config_value(&pool, crate::tools::web_search::KEY_PROVIDER, "tavily")
        .await
        .unwrap();
    let ctx = make_ctx(&pool).await;
    let err = resolve_backend_for_test(&ctx).await.unwrap_err();
    assert!(err.contains("Settings"), "{err}");
}

#[tokio::test]
async fn routing_explicit_ddg_wins_over_stored_key() {
    // 显式 ddg 时残留 key 不复活(AC4:切 ddg = 停用 Tavily)。
    let pool = make_pool().await;
    store_key(&pool, "tvly-stale").await;
    crate::db::set_config_value(&pool, crate::tools::web_search::KEY_PROVIDER, "ddg")
        .await
        .unwrap();
    let ctx = make_ctx(&pool).await;
    match resolve_backend_for_test(&ctx).await.unwrap() {
        SearchBackend::Ddg(_) => {}
        other => panic!("显式 ddg 应走 DDG,得到 {:?}", other.name()),
    }
}

#[tokio::test]
async fn routing_bad_ciphertext_degrades_to_no_key() {
    // machine-id 变了/密文损坏 → 视为无 key → auto 回落 DDG,不炸。
    let pool = make_pool().await;
    crate::db::set_config_value(
        &pool,
        crate::tools::web_search::KEY_TAVILY_API_KEY,
        "not-valid-base64-ciphertext",
    )
    .await
    .unwrap();
    let ctx = make_ctx(&pool).await;
    match resolve_backend_for_test(&ctx).await.unwrap() {
        SearchBackend::Ddg(_) => {}
        other => panic!("坏密文应视为无 key 走 DDG,得到 {:?}", other.name()),
    }
}
