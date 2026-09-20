#![cfg(test)]

//! N2 PR2(2026-09-20, task `09-20-n2-checkpoint-revert`)—
//! checkpoint 读面命令层测试(design §5 + implement PR2):
//!
//! - **prev_seq 稀疏链**:行 0/1/4(seq 2-3 为只读轮无行)下 list
//!   回传 prev_seq = None/0/1(「前一个存在行」,非字面 seq-1)+
//!   files_changed 计数;
//! - **Unavailable 矩阵**:非 git 项目 / 群聊 session / 无行 session /
//!   基线 seq 请求 diff / 不存在的 seq → kind = `CheckpointsUnavailable`;
//! - **Broken**:DB 行在而 git 对象不在(伪造 commit_sha / 上一行
//!   对象缺失)→ kind = `CheckpointBroken`,不 panic;
//! - **diff 内容**:seq 0→1 的 diff 恰含被改文件,复用 DiffResult。
//!
//! 状态构造沿 `tests_group_chat_presets.rs` 先例:
//! `AppState::load_from_dir(tempdir)` 建真实池;快照链用 PR0 纯函数
//! (`git::checkpoint` 三原语)+ `db::checkpoint::upsert_checkpoint`
//! 手工播种(与 loop 的 capture_and_record 同核,不经 agent loop)。

use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use tempfile::TempDir;

use crate::commands::checkpoint::{get_turn_checkpoint_diff_inner, list_turn_checkpoints_inner};
use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::git::checkpoint as git_ckpt;
use crate::state::AppState;

struct TestEnv {
    state: Arc<AppState>,
    /// git 仓库根(项目路径)。
    repo: TempDir,
    project_id: String,
    session_id: String,
    /// 守卫 app-data tempdir(池开着文件)。
    _dir: TempDir,
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {:?}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// git 仓库 + is_git_repo=1 的项目 + 其上的经典 chat session。
async fn make_env() -> TestEnv {
    let dir = tempfile::tempdir().expect("app data tempdir");
    let state = Arc::new(AppState::load_from_dir(dir.path().to_path_buf()).await);
    let repo = tempfile::tempdir().expect("repo tempdir");

    git_out(repo.path(), &["init", "--initial-branch=main"]);
    git_out(repo.path(), &["config", "user.email", "test@example.com"]);
    git_out(repo.path(), &["config", "user.name", "Test"]);
    std::fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-m", "init", "--no-gpg-sign"]);

    let project = db::create_project(
        &state.db,
        "测试项目",
        repo.path().to_str().unwrap(),
        true,
        Some("main".to_string()),
    )
    .await
    .expect("create project");
    let session_id = uuid::Uuid::new_v4().to_string();
    db::create_session(
        &state.db,
        &session_id,
        &project.id,
        repo.path().to_str().unwrap(),
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .expect("create session");
    TestEnv {
        state,
        repo,
        project_id: project.id,
        session_id,
        _dir: dir,
    }
}

/// 播种一个快照行(与 loop 的 capture_and_record 同核:建树 → parent
/// 接最新行 → 悬空 commit → 伞 ref → upsert)。调用方先改工作区。
async fn seed_snapshot(env: &TestEnv, sid: &str, seq: i64) -> (String, String) {
    let repo = git2::Repository::open(env.repo.path()).expect("open repo");
    let tree = git_ckpt::build_state_tree(&repo).expect("build_state_tree");
    let latest = db::checkpoint::latest_checkpoint(&env.state.db, sid)
        .await
        .expect("latest");
    let parent = latest
        .as_ref()
        .map(|r| git2::Oid::from_str(&r.commit_sha).unwrap());
    let commit =
        git_ckpt::append_snapshot(&repo, parent, tree, sid, seq as u64).expect("append_snapshot");
    git_ckpt::set_umbrella_ref(&repo, sid, commit).expect("umbrella ref");
    let tree_str = tree.to_string();
    let commit_str = commit.to_string();
    db::checkpoint::upsert_checkpoint(&env.state.db, sid, seq, &tree_str, &commit_str)
        .await
        .expect("upsert");
    (tree_str, commit_str)
}

/// 只插 DB 行、不建 git 对象(破链 / 非 git 场景的播种)。
async fn seed_row_only(env: &TestEnv, sid: &str, seq: i64, commit_sha: &str) {
    db::checkpoint::upsert_checkpoint(
        &env.state.db,
        sid,
        seq,
        &format!("tree-{commit_sha}"),
        commit_sha,
    )
    .await
    .expect("upsert row-only");
}

fn kind_of(err: &AppCommandError) -> &str {
    err.kind.as_str()
}

// ---------------------------------------------------------------------------
// list:prev_seq 稀疏链 + files_changed(AC2 的读面呈现)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_reports_previous_existing_row_across_sparse_chain() {
    let env = make_env().await;
    let sid = env.session_id.clone();

    // 基线(首轮 user 行 seq=0)→ 写轮 seq=1 → 只读轮 seq=2/3(无行,
    // 写触发门的稀疏缺口)→ 写轮 seq=4。
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;
    std::fs::write(env.repo.path().join("new.txt"), "born at seq 4\n").unwrap();
    std::fs::write(env.repo.path().join("base.txt"), "v3\n").unwrap();
    seed_snapshot(&env, &sid, 4).await;

    let list = list_turn_checkpoints_inner(&env.state, sid)
        .await
        .expect("list ok");
    let seqs: Vec<i64> = list.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, vec![0, 1, 4], "list 按 seq ASC 全量回传");

    // prev_seq = 前一个存在行(非字面 seq-1:seq 4 的 prev 是 1)。
    assert_eq!(list[0].prev_seq, None, "基线行无 prev");
    assert_eq!(list[1].prev_seq, Some(0));
    assert_eq!(list[2].prev_seq, Some(1), "稀疏链下 prev 跳过无行轮");

    // files_changed:基线 0;seq1 改 1 文件;seq4 改 1 + 新建 1 = 2。
    assert_eq!(list[0].files_changed, 0, "基线行 files_changed = 0");
    assert_eq!(list[1].files_changed, 1);
    assert_eq!(list[2].files_changed, 2);
    assert!(list[0].created_at > 1_600_000_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_baseline_only_chain_has_single_zero_entry() {
    // 纯只读 session(AC9 形状):仅基线 1 行。
    let env = make_env().await;
    seed_snapshot(&env, &env.session_id, 0).await;

    let list = list_turn_checkpoints_inner(&env.state, env.session_id.clone())
        .await
        .expect("list ok");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].seq, 0);
    assert_eq!(list[0].prev_seq, None);
    assert_eq!(list[0].files_changed, 0);
}

// ---------------------------------------------------------------------------
// Unavailable 矩阵(非 git / 群聊 / 无行 / 基线 / 缺 seq)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_unavailable_when_no_rows() {
    let env = make_env().await;
    let err = list_turn_checkpoints_inner(&env.state, env.session_id.clone())
        .await
        .expect_err("no rows must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
    assert_eq!(err.category, ErrorCategory::InvalidRequest);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_unavailable_for_non_git_project() {
    let env = make_env().await;
    // 非 git 项目 + 其上的 session(行是否在都一样:门在行查询之前
    // 也之后都行 —— 这里连行一起给,验证门优先)。
    let project = db::create_project(
        &env.state.db,
        "非 git 项目",
        "/tmp/non-git-checkpoint-proj",
        false,
        None,
    )
    .await
    .expect("create non-git project");
    let sid = uuid::Uuid::new_v4().to_string();
    db::create_session(
        &env.state.db,
        &sid,
        &project.id,
        "/tmp/non-git-checkpoint-proj",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .expect("create session");
    seed_row_only(&env, &sid, 0, &"a".repeat(40)).await;

    let err = list_turn_checkpoints_inner(&env.state, sid)
        .await
        .expect_err("non-git must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_unavailable_for_group_chat_session() {
    let env = make_env().await;
    let sid = uuid::Uuid::new_v4().to_string();
    db::create_session(
        &env.state.db,
        &sid,
        &env.project_id,
        env.repo.path().to_str().unwrap(),
        "GLM-4.7",
        None,
        Some("group_chat"),
        None,
    )
    .await
    .expect("create group-chat session");
    seed_row_only(&env, &sid, 0, &"b".repeat(40)).await;

    let err = list_turn_checkpoints_inner(&env.state, sid)
        .await
        .expect_err("group chat must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_not_found_is_invalid_request() {
    let env = make_env().await;
    let err = list_turn_checkpoints_inner(&env.state, "no-such-session".into())
        .await
        .expect_err("unknown session");
    assert_eq!(err.category, ErrorCategory::InvalidRequest);
    assert_eq!(kind_of(&err), "Manual", "普通 InvalidRequest 不带专用 kind");
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_of_baseline_seq_is_unavailable() {
    // 基线行无 diff 入口(vs 空树会把整仓列成新增)。
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;

    let err = get_turn_checkpoint_diff_inner(&env.state, sid, 0)
        .await
        .expect_err("baseline diff must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");

    // 不存在的 seq 同样 Unavailable。
    let err = get_turn_checkpoint_diff_inner(&env.state, env.session_id.clone(), 99)
        .await
        .expect_err("missing seq must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
}

// ---------------------------------------------------------------------------
// Broken:DB 行在、git 对象不在
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_broken_when_commit_object_missing() {
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    // 伪造一行:sha 合法 40-hex 但对象不存在(如手动 gc 后的残留行)。
    seed_row_only(&env, &sid, 1, &"f".repeat(40)).await;

    let err = list_turn_checkpoints_inner(&env.state, sid)
        .await
        .expect_err("missing object must degrade as broken");
    assert_eq!(kind_of(&err), "CheckpointBroken");
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_broken_when_prev_object_missing() {
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;
    // 把基线的 git 对象砍掉是模拟「行在对象不在」的最小面:直接把
    // 基线行的 commit_sha 改成悬空值(对象树缺失走同一判定臂)。
    seed_row_only(&env, &sid, 0, &"e".repeat(40)).await;

    let err = get_turn_checkpoint_diff_inner(&env.state, sid, 1)
        .await
        .expect_err("prev row missing object must degrade as broken");
    assert_eq!(kind_of(&err), "CheckpointBroken");
}

// ---------------------------------------------------------------------------
// diff 内容(DiffResult 同构,前端 DiffView 直渲)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn diff_between_snapshots_reuses_diff_result_shape() {
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;

    let result = get_turn_checkpoint_diff_inner(&env.state, sid, 1)
        .await
        .expect("diff ok");
    assert_eq!(result.files.len(), 1);
    let f = &result.files[0];
    assert_eq!(f.path, "base.txt");
    assert_eq!(f.status, "modified");
    assert_eq!(f.added, 1, "numstat 口径:v1→v2 是 1+/1-");
    assert_eq!(f.removed, 1);
    assert!(f.diff_text.contains("+v2"), "unified diff 正文就绪直渲");
}
