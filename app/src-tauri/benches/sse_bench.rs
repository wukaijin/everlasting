//! B3 SSE 双路径 bench(任务 09-19-n9-perf-benchmark,design §4;降级形态)。
//!
//! 探针结论(2026-09-19 坐实):路径 B(live 首事件)不可行——
//! `build_provider` 的 protocol dispatch 是字符串匹配("anthropic"/
//! "openai"/"openai_responses"),无 mock 臂,且 scripted MockProvider
//! 无法经 providers 表/catalog 携带脚本;给生产开 mock 注入机制超出
//! 测量任务边界。降级线落地:
//!
//! - b3a **HTTP/SSE 握手开销**:oneshot GET /api/v1/stream,await 完成
//!   即响应头就绪(路由 dispatch + handler + 响应建立)。**不读 body**
//!   ——空 replay 的下一帧是 30s KeepAlive,读首块会量到心跳(评审
//!   结论 #8 的坑)。
//! - b3b **replay TTFB**:预填 1000 帧后带 `Last-Event-ID: 0` 订阅,
//!   量响应 body 首块字节到达(replay 切片 + 序列化 + flush)。
//!
//! 归因式(落档):b3a ≈ daemon HTTP 面净开销下界;b3b − b3a ≈ replay
//! 路径开销。live 首事件面留待后续任务需要时补 mock 注入机制(边界
//! 如实标注,不虚报覆盖)。

use axum::body::Body;
use axum::http::Request;
use criterion::{criterion_group, criterion_main, Criterion};
use everlasting_lib::bench_api::{build_router, load_daemon_state};
use futures_util::StreamExt;
use tower::ServiceExt;

fn bench_sse(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");

    // 全套 AppState(data_dir 建库 + catalog);backup/sweeper 是显式
    // spawn 函数,load 不启动它们,进程内无后台任务污染。
    let state = rt.block_on(async {
        let base = std::env::temp_dir();
        assert!(
            !base.starts_with("/mnt/"),
            "TMPDIR under /mnt/* distorts benchmarks: {}",
            base.display()
        );
        let dir = tempfile::tempdir_in(&base).expect("tempdir in /tmp");
        let s = load_daemon_state(dir.path().to_path_buf()).await;
        std::mem::forget(dir); // bench 进程生命周期内保活
        s
    });
    let router = build_router(state.clone());

    // b3a:握手(oneshot 完成 = 200 + text/event-stream 头就绪)。
    c.bench_function("b3a_sse_handshake", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = Request::builder()
                    .uri("/api/v1/stream")
                    .body(Body::empty())
                    .expect("request");
                let resp = router.clone().oneshot(req).await.expect("oneshot");
                assert_eq!(resp.status(), axum::http::StatusCode::OK);
                // 不读 body:空 replay 首帧是 30s KeepAlive。
            })
        })
    });

    // b3b:replay TTFB(预填 1000 帧在测量外一次性完成)。
    rt.block_on(async {
        for i in 0..1000 {
            state.sse.broadcast(
                "chat-event",
                &serde_json::json!({ "i": i, "kind": "delta", "text": "bench replay frame" }),
            );
        }
    });
    c.bench_function("b3b_sse_replay_ttfb_1000", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = Request::builder()
                    .uri("/api/v1/stream")
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .expect("request");
                let resp = router.clone().oneshot(req).await.expect("oneshot");
                let mut stream = resp.into_body().into_data_stream();
                let first = stream.next().await.expect("replay first chunk");
                assert!(first.is_ok());
            })
        })
    });
}

criterion_group!(benches, bench_sse);
criterion_main!(benches);
