//! B2 DB 读写 bench(任务 09-19-n9-perf-benchmark,design §3)。
//!
//! 双档分表(评审结论 #5):内存档(test_pool,sqlite::memory 无 WAL)=
//! 查询/映射 CPU 成本;disk 档(init_pool + /tmp 下 tempdir 的 .db)=
//! 含 WAL/fsync 真实写成本。**N2 checkpoint 推算只认 disk 档**(内存档
//! 抹掉 auto-commit fsync)。
//!
//! 组:b1 load_session(1k/10k × 双档)/ b2 persist_turn 单条写 /
//! b3 finalize_turn_persist UPSERT 覆盖(N2 auto-commit 落点,用户裁定
//! D4)/ b5 按 seq 删除后缀曲线(N2 revert=reset 的删除成本半边)。
//!
//! tempdir 只落 ext4(/tmp),严禁 /mnt/*(评审结论 #11,WSL2 跨文件
//! 系统失真)。

mod support;

use std::sync::atomic::{AtomicI64, Ordering};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use everlasting_lib::bench_api::{
    finalize_turn_persist, init_pool, load_session, persist_turn, test_pool, MessageContent, Role,
};
use sqlx::SqlitePool;
use support::{load_profile, seed_pool_session};

/// WSL2:/tmp = ext4;TMPDIR 指向 /mnt/*(9p)时 fail-loud 拒跑,不让
/// disk 档数字静默失真。
async fn disk_pool() -> SqlitePool {
    let base = std::env::temp_dir();
    assert!(
        !base.starts_with("/mnt/"),
        "TMPDIR under /mnt/* (9p) distorts disk benchmarks: {}",
        base.display()
    );
    let dir = tempfile::tempdir_in(&base).expect("tempdir in /tmp");
    let pool = init_pool(&dir.path().join("bench-disk.db"))
        .await
        .expect("init_pool");
    // init_pool 只连库设 PRAGMA,不建 schema(test_pool 亦显式迁移)。
    everlasting_lib::bench_api::run_migrations(&pool)
        .await
        .expect("run_migrations");
    std::mem::forget(dir); // bench 进程生命周期内保活;OS 退出清理
    pool
}

/// 走真函数建 project + session(外键与默认值由 DB 层负责,与生产
/// 同路径;裸 INSERT 猜列名/默认值是毒——首跑即翻车的实证)。
async fn seed_project_and_session(pool: &SqlitePool, sid: &str) {
    use everlasting_lib::bench_api::{create_project, create_session, list_projects};
    let path = "/tmp/n9-bench-project";
    create_project(pool, "n9-bench", path, false, None)
        .await
        .expect("create_project");
    let projects = list_projects(pool, false).await.expect("list_projects");
    let pid = projects
        .iter()
        .find(|p| p.path == path)
        .map(|p| p.id.clone())
        .expect("project present");
    create_session(pool, sid, &pid, path, "bench-model", None, None, None)
        .await
        .expect("create_session");
}

fn write_payload(i: i64) -> MessageContent {
    MessageContent::Text(format!("bench write payload #{i} ").repeat(8))
}

fn bench_db(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let profile = load_profile();

    // --- b1:load_session 读(纯读无状态污染,iter 即可) ---
    for &n in &[1_000i64, 10_000] {
        let (pool, sid) = rt.block_on(async {
            let pool = test_pool().await;
            let sid = format!("bench-mem-{n}");
            seed_project_and_session(&pool, &sid).await;
            seed_pool_session(&pool, &sid, &profile, n).await;
            (pool, sid)
        });
        c.bench_function(&format!("b1_load_session_mem_{n}"), |b| {
            b.iter(|| rt.block_on(async { load_session(&pool, &sid).await.expect("load_session") }))
        });
        rt.block_on(pool.close());

        let (pool, sid) = rt.block_on(async {
            let pool = disk_pool().await;
            let sid = format!("bench-disk-{n}");
            seed_project_and_session(&pool, &sid).await;
            seed_pool_session(&pool, &sid, &profile, n).await;
            (pool, sid)
        });
        c.bench_function(&format!("b1_load_session_disk_{n}"), |b| {
            b.iter(|| rt.block_on(async { load_session(&pool, &sid).await.expect("load_session") }))
        });
        rt.block_on(pool.close());
    }

    // --- b2:persist_turn 单条写(递增 seq;表从 0 渐增,几百 iteration
    // 的增长对单条 INSERT 影响微小,口径注)。 ---
    let (mem_pool, mem_sid) = rt.block_on(async {
        let pool = test_pool().await;
        let sid = "bench-mem-write".to_string();
        seed_project_and_session(&pool, &sid).await;
        (pool, sid)
    });
    let seq = AtomicI64::new(0);
    c.bench_function("b2_persist_turn_mem", |b| {
        b.iter(|| {
            let i = seq.fetch_add(1, Ordering::SeqCst);
            rt.block_on(async {
                persist_turn(
                    &mem_pool,
                    &mem_sid,
                    Role::Assistant,
                    &write_payload(i),
                    i,
                    None,
                    None,
                )
                .await
                .expect("persist")
            })
        })
    });

    let (disk_pool_w, disk_sid_w) = rt.block_on(async {
        let pool = disk_pool().await;
        let sid = "bench-disk-write".to_string();
        seed_project_and_session(&pool, &sid).await;
        (pool, sid)
    });
    let seq_d = AtomicI64::new(0);
    c.bench_function("b2_persist_turn_disk", |b| {
        b.iter(|| {
            let i = seq_d.fetch_add(1, Ordering::SeqCst);
            rt.block_on(async {
                persist_turn(
                    &disk_pool_w,
                    &disk_sid_w,
                    Role::Assistant,
                    &write_payload(i),
                    i,
                    None,
                    None,
                )
                .await
                .expect("persist")
            })
        })
    });

    // --- b3:finalize_turn_persist UPSERT 覆盖(固定 seq,零累积)。 ---
    c.bench_function("b3_finalize_turn_mem", |b| {
        b.iter(|| {
            rt.block_on(async {
                finalize_turn_persist(
                    &mem_pool,
                    &mem_sid,
                    Role::Assistant,
                    &write_payload(0),
                    0,
                    None,
                    None,
                )
                .await
                .expect("finalize")
            })
        })
    });
    c.bench_function("b3_finalize_turn_disk", |b| {
        b.iter(|| {
            rt.block_on(async {
                finalize_turn_persist(
                    &disk_pool_w,
                    &disk_sid_w,
                    Role::Assistant,
                    &write_payload(0),
                    0,
                    None,
                    None,
                )
                .await
                .expect("finalize")
            })
        })
    });

    // --- b5:按 seq 删除后缀(N2 revert 语义;单语句 DELETE 后缀 50 行,
    // 测量段只包 DELETE,setup 段恢复尾部)。1k 与 10k 两表大小曲线点,
    // disk 档(N2 推算口径)。 ---
    const TAIL: i64 = 50;
    for &n in &[1_000i64, 10_000] {
        let (pool, sid) = rt.block_on(async {
            let pool = disk_pool().await;
            let sid = format!("bench-disk-del-{n}");
            seed_project_and_session(&pool, &sid).await;
            seed_pool_session(&pool, &sid, &profile, n).await;
            (pool, sid)
        });
        let mut group = c.benchmark_group(format!("b5_delete_suffix_disk_{n}"));
        group.sample_size(30);
        group.bench_function("delete_tail_50", |b| {
            b.iter_batched(
                || {
                    // setup(测量外):恢复尾部 50 行(直接 INSERT,persist
                    // 语义在此无关——只补行数与行形)
                    rt.block_on(async {
                        for i in (n - TAIL)..n {
                            sqlx::query(
                                "INSERT OR REPLACE INTO messages \
                                 (session_id, role, content, text, has_tool_calls, has_tool_results, created_at, seq) \
                                 VALUES (?, 'assistant', '{}', 'bench restore', 0, 0, ?, ?)",
                            )
                            .bind(&sid)
                            .bind(format!("2026-09-19T00:00:{:02}Z", i % 60))
                            .bind(i)
                            .execute(&pool)
                            .await
                            .expect("restore tail row");
                        }
                    })
                },
                |_| {
                    rt.block_on(async {
                        sqlx::query("DELETE FROM messages WHERE session_id = ? AND seq >= ?")
                            .bind(&sid)
                            .bind(n - TAIL)
                            .execute(&pool)
                            .await
                            .expect("delete suffix");
                    })
                },
                BatchSize::PerIteration,
            )
        });
        group.finish();
        rt.block_on(pool.close());
    }

    rt.block_on(mem_pool.close());
    rt.block_on(disk_pool_w.close());
}

criterion_group!(benches, bench_db);
criterion_main!(benches);
