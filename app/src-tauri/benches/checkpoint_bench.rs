//! B6 build_state_tree bench(N2 checkpoint,任务 09-20-n2-checkpoint-revert,
//! design §9/§10)。轮末快照的成本 = 全仓 stat 扫描 + 内存 index 重建 +
//! tree 对象写,量级应贴近 `git status`;本 bench 出数字落
//! `.trellis/spec/backend/perf-baseline.md` B6 段(数字不进 CI 门禁,
//! 既有裁定;CI 只跑编译门 `cargo check --features bench --benches`)。
//!
//! 合成仓库三档(100 / 1k / 10k 个 tracked 文件,均匀散布 10 个目录,
//! 内容逐文件唯一——blob 去重不参与测量)。文件全是 tracked+clean,
//! 对应生产主形态:轮末快照扫一遍干净仓库 + 少量脏文件。
//!
//! tempdir 只落 ext4(/tmp),严禁 /mnt/*(9p 跨文件系统失真,
//! 同 db_bench 的 fail-loud 约定)。

use std::path::Path;
use std::process::Command as StdCommand;

use criterion::{criterion_group, criterion_main, Criterion};
use everlasting_lib::bench_api::build_state_tree;
use git2::Repository;

/// Init a git repo at `path` + configure identity(与生产无关,只为
/// CLI commit 能跑)。
fn init_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    let init = StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed: {:?}", init);
    for cfg in [
        ["config", "user.email", "bench@example.com"],
        ["config", "user.name", "Bench"],
    ] {
        let c = StdCommand::new("git")
            .args(cfg)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(c.status.success());
    }
}

/// 写 `n` 个 tracked 文件:`dir{i:02}/file{j:05}.txt`,内容逐文件唯一。
fn seed_files(root: &Path, n: usize) {
    const DIRS: usize = 10;
    for i in 0..n {
        let dir = root.join(format!("dir{:02}", i % DIRS));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("file{i:05}.txt")),
            format!("payload {i}\nline two of file {i}\n"),
        )
        .unwrap();
    }
}

fn commit_all(path: &Path) {
    let add = StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = StdCommand::new("git")
        .args(["commit", "-m", "seed", "--no-gpg-sign"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit failed: {:?}", commit);
}

fn bench_checkpoint(c: &mut Criterion) {
    // WSL2:/tmp = ext4;TMPDIR 指向 /mnt/*(9p)时 fail-loud 拒跑。
    let base = std::env::temp_dir();
    assert!(
        !base.starts_with("/mnt/"),
        "TMPDIR under /mnt/* (9p) distorts disk benchmarks: {}",
        base.display()
    );

    for &n in &[100usize, 1_000, 10_000] {
        let dir = tempfile::tempdir_in(&base).expect("tempdir in /tmp");
        let repo_path = dir.path().join("repo");
        init_repo(&repo_path);
        seed_files(&repo_path, n);
        commit_all(&repo_path);
        std::mem::forget(dir); // bench 进程生命周期内保活;OS 退出清理

        let repo = Repository::open(&repo_path).expect("open bench repo");
        // 首跑预热(建首轮 tree 对象),测量段内对象已存在、写树为
        // no-op——数字反映扫描 + index 重建的真实轮末成本。
        let warm = build_state_tree(&repo).expect("warm-up snapshot");

        let mut group = c.benchmark_group(format!("b6_build_state_tree_{n}"));
        // 10k 档单 iter 成本高,缩样本量控制墙钟(同 db_bench b5 惯例)。
        if n >= 10_000 {
            group.sample_size(30);
        }
        group.bench_function("snapshot_full_scan", |b| {
            b.iter(|| {
                let t = build_state_tree(&repo).expect("build_state_tree");
                assert_eq!(t, warm, "clean repo must dedupe to the same tree");
            })
        });
        group.finish();
    }
}

criterion_group!(benches, bench_checkpoint);
criterion_main!(benches);
