//! B1 harness 开销 bench(任务 09-19-n9-perf-benchmark,design §2)。
//!
//! `run_chat_loop` 整轮(MockProvider 零网络 / MockEmitter 收事件 / 真实
//! pool 落库),覆盖 load_session 回放 → context 组装 → tools schema →
//! provider 调度 → persist → 事件 emit 全链。iter 主体 `block_on` 等整轮
//! 完成(spawn 即返测不到东西)。
//!
//! 隔离(评审结论 #4,硬约束):h2/h3 每次真落库,iteration 间会累积
//! 行并撞 `UNIQUE(session_id, seq)` —— 一律 `iter_batched` +
//! `BatchSize::PerIteration`,setup(测量外)重建 harness 与种子。
//!
//! 归因口径:h3(整轮)− b1(load_session,db_bench)≈ 组装+schema+调度
//! 净开销;单函数拆组以此差值替代(任务 design §2 口径注)。

mod support;

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use everlasting_lib::bench_api::{
    builtin_tools, chat_loop_deps, chat_loop_request, make_harness, parent_role, run_chat_loop,
    set_config_value, test_messages, ChatEvent, MockEmitter, MockProvider, MockResponse,
    TestHarness,
};
use support::{arc_provider, load_profile, seed_session, text_script, tool_call_script};

async fn one_text_turn(h: TestHarness) {
    let emitter = Arc::new(MockEmitter::new());
    let provider = arc_provider(text_script());
    run_chat_loop(
        chat_loop_request(
            builtin_tools(),
            provider,
            200_000,
            "bench-rid".into(),
            h.session_id.clone(),
            test_messages(),
            emitter,
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;
}

/// h4:两轮脚本 = ToolCall(list_dir,真执行)+ 文本收尾。
async fn one_tool_turn(h: TestHarness) {
    let emitter = Arc::new(MockEmitter::new());
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::Events(tool_call_script(&h).into_iter().map(Ok).collect()),
        MockResponse::Events(text_script().into_iter().map(Ok).collect()),
    ]));
    run_chat_loop(
        chat_loop_request(
            builtin_tools(),
            provider,
            200_000,
            "bench-rid-tool".into(),
            h.session_id.clone(),
            test_messages(),
            emitter,
        ),
        chat_loop_deps(&h),
        parent_role(&h),
    )
    .await;
}

fn events(evs: Vec<ChatEvent>) -> MockResponse {
    MockResponse::Events(evs.into_iter().map(Ok).collect())
}

fn bench_harness(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let profile = load_profile();

    // h1:空 session 纯文本一轮(冷启动)。
    c.bench_function("h1_cold_text_turn", |b| {
        b.iter_batched(
            || rt.block_on(make_harness()),
            |h| rt.block_on(one_text_turn(h)),
            BatchSize::PerIteration,
        )
    });

    // h2:预种 1k 消息后一轮(历史回放成本)。
    c.bench_function("h2_1k_history_text_turn", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let h = make_harness().await;
                    seed_session(&h, &profile, 1_000).await;
                    h
                })
            },
            |h| rt.block_on(one_text_turn(h)),
            BatchSize::PerIteration,
        )
    });

    // h3:预种 10k 消息后一轮(长会话启动;秒级 iteration,显式采样参数)。
    let mut group = c.benchmark_group("h3_10k");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(30));
    group.bench_function("history_text_turn", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let h = make_harness().await;
                    seed_session(&h, &profile, 10_000).await;
                    h
                })
            },
            |h| rt.block_on(one_text_turn(h)),
            BatchSize::PerIteration,
        )
    });

    // h3-on:10k + llm_compaction_enabled=true(make_harness 继承测试档
    // off 而生产为 on,评审结论 #6)。压缩先消费一条脚本(摘要轮),
    // 脚本配 [摘要, 主响应]。
    group.bench_function("history_text_turn_compaction_on", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let h = make_harness().await;
                    set_config_value(&h.db, "llm_compaction_enabled", "true")
                        .await
                        .expect("enable compaction");
                    seed_session(&h, &profile, 10_000).await;
                    h
                })
            },
            |h| {
                rt.block_on(async move {
                    let emitter = Arc::new(MockEmitter::new());
                    let provider = Arc::new(MockProvider::new(vec![
                        events(text_script()),
                        events(text_script()),
                    ]));
                    run_chat_loop(
                        chat_loop_request(
                            builtin_tools(),
                            provider,
                            200_000,
                            "bench-rid-on".into(),
                            h.session_id.clone(),
                            test_messages(),
                            emitter,
                        ),
                        chat_loop_deps(&h),
                        parent_role(&h),
                    )
                    .await;
                })
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();

    // h4:工具回路一轮(ToolCall 真执行 + 第二轮文本)。
    c.bench_function("h4_tool_round_trip", |b| {
        b.iter_batched(
            || rt.block_on(make_harness()),
            |h| rt.block_on(one_tool_turn(h)),
            BatchSize::PerIteration,
        )
    });
}

criterion_group!(benches, bench_harness);
criterion_main!(benches);
