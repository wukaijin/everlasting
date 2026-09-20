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
//! PR3(同日)追加 —— revert 两步命令层测试:
//!
//! - **AC3 语义**:preview 还原集 = diff(当前态, 目标树)路径全集;
//!   execute 后还原集内文件内容回目标树、目标树无者删除、还原集外
//!   不动;分支指针(HEAD)与暂存区(staged 内容)前后一致。
//! - **AC4 门禁**:外来写入 → foreign_delta 非空(preview);确认后
//!   有效 token 照常执行;preview→execute 之间新落盘 → 旧 token 被拒
//!   (`StalePreview`),重新 preview 后可执行。
//! - **busy 拒绝(OQ-C)**:本 session 在 `session_active_request`
//!   注册面挂条目 → execute 拒(`SessionBusy`),清条目后放行。
//! - **AC5 audit**:execute 成功落 `checkpoint_reverted` 行,payload
//!   含 target_seq / restored / deleted / paths / gate_tree_oid / source。
//! - **AC8 后半**:revert 不删 worker 分支(分支仍可解析、指针不动)。
//! - **链不截断回归**:revert → 新快照轮(seq 2)→ 轮间 diff 含
//!   revert 影响,旧行(0/1)仍在。
//! - **归属标记**:write 家族 tool 审计路径 → `tool_written`(优先);
//!   目标 seq 之后有 A2+ 判写 shell 轮 → `shell_write`;无证据 →
//!   `unknown`。
//!
//! 状态构造沿 `tests_group_chat_presets.rs` 先例:
//! `AppState::load_from_dir(tempdir)` 建真实池;快照链用 PR0 纯函数
//! (`git::checkpoint` 三原语)+ `db::checkpoint::upsert_checkpoint`
//! 手工播种(与 loop 的 capture_and_record 同核,不经 agent loop)。

use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;

use tempfile::TempDir;

use crate::commands::checkpoint::{
    get_turn_checkpoint_diff_inner, list_turn_checkpoints_inner,
    revert_to_checkpoint_execute_inner, revert_to_checkpoint_preview_inner,
};
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

// ---------------------------------------------------------------------------
// PR3: revert 两步(design §5 / AC3 / AC4 / AC5 / AC8 后半)
// ---------------------------------------------------------------------------

/// 播一条 `tool_executed` 审计行(归属标记的证据面)。
async fn seed_tool_executed(
    env: &TestEnv,
    tool: &str,
    input: serde_json::Value,
    turn_seq: Option<i64>,
) {
    let payload = serde_json::json!({
        "tool_name": tool,
        "tool_input": input,
        "duration_ms": 1,
        "exit_code": null,
    });
    db::record_audit_event(
        &env.state.db,
        &env.session_id,
        "tool_executed",
        Some(&payload.to_string()),
        turn_seq,
    )
    .await
    .expect("seed audit row");
}

/// 标准 revert 场景:基线(base.txt=v1)→ seq1(base.txt=v2 + 新建
/// new.txt,审计含 write_file new.txt 与判写 shell 轮)。
async fn make_revert_env() -> TestEnv {
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    std::fs::write(env.repo.path().join("new.txt"), "born at seq1\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;
    seed_tool_executed(
        &env,
        "write_file",
        serde_json::json!({"path": "new.txt", "content": "…"}),
        Some(1),
    )
    .await;
    seed_tool_executed(
        &env,
        "shell",
        serde_json::json!({"command": "echo hi > out.txt"}),
        Some(1),
    )
    .await;
    env
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_preview_lists_restore_set_with_attribution() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");

    // 还原集 = diff(当前态, 目标树)路径全集,action 逐项正确。
    let mut got: Vec<(String, String, String)> = preview
        .files
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                serde_json::to_string(&f.action).unwrap(),
                serde_json::to_string(&f.attribution).unwrap(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(got.len(), 2, "还原集恰为两个分歧路径: {got:?}");
    assert_eq!(got[0].0, "base.txt");
    assert_eq!(got[0].1, "\"checkout\"");
    assert_eq!(got[1].0, "new.txt");
    assert_eq!(got[1].1, "\"delete\"");

    // 归属:base.txt 无 tool 路径证据,但目标 seq 之后有判写 shell 轮
    // (turn_seq=1 > 0)→ shell_write;new.txt 命中 write_file 审计路径
    // → tool_written(优先于 shell)。
    assert_eq!(got[0].2, "\"shell_write\"");
    assert_eq!(got[1].2, "\"tool_written\"");

    // 无外来写入时 foreign_delta 为 None(警告区仅非空渲染的依据)。
    assert!(preview.foreign_delta.is_none());

    // preview_token = "<target_tree>:<gate_tree>",两个 40-hex。
    let parts: Vec<&str> = preview.preview_token.split(':').collect();
    assert_eq!(parts.len(), 2);
    for p in parts {
        assert_eq!(p.len(), 40, "token 段应为 40-hex oid: {p}");
    }
    assert_eq!(preview.target_seq, 0);
    assert!(preview.target_created_at > 1_600_000_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_preview_attribution_unknown_without_evidence() {
    // 无任何审计行:两路径都落 unknown(共享 cwd 常态,badge 中性色)。
    let env = make_env().await;
    let sid = env.session_id.clone();
    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(env.repo.path().join("base.txt"), "v2\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid, 0)
        .await
        .expect("preview ok");
    assert_eq!(preview.files.len(), 1);
    assert_eq!(
        serde_json::to_string(&preview.files[0].attribution).unwrap(),
        "\"unknown\""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_preview_flags_foreign_delta() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();

    // 轮间隙外来写入(不经快照链,直接改文件):门禁必须看见。
    std::fs::write(env.repo.path().join("base.txt"), "hand-edited\n").unwrap();

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid, 0)
        .await
        .expect("preview ok");
    let foreign = preview.foreign_delta.expect("外来写入必须触发警告区");
    assert_eq!(foreign.len(), 1);
    assert_eq!(foreign[0].path, "base.txt");
    // AC4:外来在场时还原集照常给出(强制确认,非拒绝)。
    assert_eq!(preview.files.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_execute_restores_files_and_records_audit() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();
    let repo_dir = env.repo.path().to_path_buf();

    // 暂存区钉一个改动:AC3 要求 staged 内容前后一致(索引不被触碰)。
    // 注意借既有 tracked 文件 —— 新建文件会被 gate 树(add -A)收进
    // 还原集并被 revert 删掉,那恰恰是还原语义而非索引缺陷。
    git_out(&repo_dir, &["add", "base.txt"]);
    let staged_before = git_out(&repo_dir, &["diff", "--cached"]);
    let head_before = git_out(&repo_dir, &["rev-parse", "HEAD"]);

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");
    let result =
        revert_to_checkpoint_execute_inner(&env.state, sid.clone(), 0, preview.preview_token)
            .await
            .expect("execute ok");
    assert_eq!(result.restored, 1, "base.txt 回写");
    assert_eq!(result.deleted, 1, "new.txt 删除");

    // AC3:还原集内文件回目标树、目标树无者删除、还原集外不动。
    assert_eq!(
        std::fs::read_to_string(repo_dir.join("base.txt")).unwrap(),
        "base\n"
    );
    assert!(!repo_dir.join("new.txt").exists());

    // AC3:分支指针与暂存区一致(还原不碰 git 用户面)。
    assert_eq!(head_before, git_out(&repo_dir, &["rev-parse", "HEAD"]));
    assert_eq!(staged_before, git_out(&repo_dir, &["diff", "--cached"]));

    // 链不截断:旧行全在。
    let list = list_turn_checkpoints_inner(&env.state, sid.clone())
        .await
        .expect("list ok");
    let seqs: Vec<i64> = list.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, vec![0, 1]);

    // AC5:audit 落行,payload 携带契约字段。
    let events = db::list_audit_events(&env.state.db, &sid)
        .await
        .expect("audit read");
    let row = events
        .iter()
        .find(|e| e.kind == "checkpoint_reverted")
        .expect("checkpoint_reverted row must exist");
    let payload: serde_json::Value =
        serde_json::from_str(row.payload_json.as_deref().expect("payload")).unwrap();
    assert_eq!(payload["target_seq"], 0);
    assert_eq!(payload["restored"], 1);
    assert_eq!(payload["deleted"], 1);
    assert_eq!(payload["source"], "ui");
    let paths = payload["paths"].as_array().expect("paths array");
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().any(|p| p == "base.txt"));
    assert!(paths.iter().any(|p| p == "new.txt"));
    assert_eq!(
        payload["gate_tree_oid"].as_str().expect("gate oid").len(),
        40,
        "gate_tree_oid 是 40-hex"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_execute_rejects_stale_token_then_recovers() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();
    let repo_dir = env.repo.path().to_path_buf();

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");
    let old_gate = preview.preview_token.split(':').nth(1).unwrap().to_string();
    let old_token = preview.preview_token;

    // preview→confirm 之间新落盘(不经链):旧 token 必须被拒。
    std::fs::write(repo_dir.join("late.txt"), "landed after preview\n").unwrap();
    let err = revert_to_checkpoint_execute_inner(&env.state, sid.clone(), 0, old_token)
        .await
        .expect_err("stale token must be rejected");
    assert_eq!(kind_of(&err), "StalePreview");
    assert!(err.retryable, "StalePreview 语义 = 重新 preview 可恢复");

    // 重新 preview 拿新 token → 执行成功(AC4「确认后按确认语义执行」;
    // late.txt 在目标树中不存在 → 进还原集被删)。
    let fresh = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("re-preview ok");
    let fresh_gate = fresh.preview_token.split(':').nth(1).unwrap();
    assert_ne!(fresh_gate, old_gate, "gate 树变了 token 必须变");
    let result = revert_to_checkpoint_execute_inner(&env.state, sid, 0, fresh.preview_token)
        .await
        .expect("execute with fresh token");
    assert_eq!(result.restored, 1);
    assert_eq!(result.deleted, 2, "new.txt + late.txt 都不在目标树");
    assert!(!repo_dir.join("late.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_execute_rejects_busy_own_session() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();

    // OQ-C:MVP 只拒本 session busy。真源 = loop 的
    // session_active_request 注册面(chat 路由同一判定)。
    env.state
        .session_active_request
        .lock()
        .await
        .insert(sid.clone(), "rid-in-flight".to_string());
    let err = revert_to_checkpoint_execute_inner(&env.state, sid.clone(), 0, "any".into())
        .await
        .expect_err("busy session must reject execute");
    assert_eq!(kind_of(&err), "SessionBusy");

    // busy 时 preview 仍可用(读操作;execute 才有 TOCTOU 门)。
    revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview is a read, not gated on busy");

    env.state.session_active_request.lock().await.remove(&sid);
    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");
    revert_to_checkpoint_execute_inner(&env.state, sid, 0, preview.preview_token)
        .await
        .expect("busy cleared → execute ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_baseline_is_valid_target_but_missing_seq_unavailable() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();

    // 基线(seq0)是合法 target(AC10「回到会话前」)。
    revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("baseline target ok");

    // 不存在的 seq → Unavailable(与读面同类型化)。
    let err = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 99)
        .await
        .expect_err("missing seq must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
    let err = revert_to_checkpoint_execute_inner(&env.state, sid, 99, "t".into())
        .await
        .expect_err("missing seq must degrade");
    assert_eq!(kind_of(&err), "CheckpointsUnavailable");
}

#[tokio::test(flavor = "multi_thread")]
async fn revert_keeps_worker_branch_remergeable() {
    // AC8 后半:revert 不删 `worker/<run_id>` 分支,且分支**可重
    // merge** —— 三层真断言(指针解析 + 产物内容可解析 + 临时
    // detached worktree 实走一次 merge),不做「ref 还在」的形式断言。
    // 场景复刻 merge_worker 前置态:worker 分支 off 干净 HEAD 建出、
    // 带一个独有 commit(worker 产物文件),host 侧随后走经典 revert
    // (基线 → 写轮 seq1,与 make_revert_env 同核;worker 产物已 commit
    // 进分支、不占工作区,故快照树不含它)。
    let env = make_env().await;
    let sid = env.session_id.clone();
    let repo_dir = env.repo.path().to_path_buf();

    git_out(&repo_dir, &["branch", "worker/run-1"]);
    git_out(&repo_dir, &["checkout", "worker/run-1"]);
    std::fs::write(repo_dir.join("worker_output.txt"), "worker artifact\n").unwrap();
    git_out(&repo_dir, &["add", "-A"]);
    git_out(&repo_dir, &["commit", "-m", "worker work", "--no-gpg-sign"]);
    git_out(&repo_dir, &["checkout", "main"]);
    let branch_before = git_out(&repo_dir, &["rev-parse", "worker/run-1"]);

    seed_snapshot(&env, &sid, 0).await;
    std::fs::write(repo_dir.join("base.txt"), "v2\n").unwrap();
    std::fs::write(repo_dir.join("new.txt"), "born at seq1\n").unwrap();
    seed_snapshot(&env, &sid, 1).await;

    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");
    revert_to_checkpoint_execute_inner(&env.state, sid, 0, preview.preview_token)
        .await
        .expect("execute ok");

    // 断言 1:分支指针原样(revert 不碰任何 ref —— PRD R2「永不触碰
    // 分支指针」的 worker 分支特例)。
    assert_eq!(
        branch_before,
        git_out(&repo_dir, &["rev-parse", "worker/run-1"]),
        "worker 分支指针必须原样保留"
    );
    // 断言 2:worker 产物仍经分支可解析(指针指向的 commit/tree/blob
    // 对象链完好,不是只留一个悬空 ref 名)。
    assert_eq!(
        git_out(&repo_dir, &["show", "worker/run-1:worker_output.txt"]),
        "worker artifact\n",
        "worker 产物必须仍可经分支解析出内容"
    );
    // 断言 3:可重 merge —— 临时 detached worktree(不动被 revert 的
    // 主工作区)里实走一次 merge,产物如约落进合并结果。
    let wt = tempfile::tempdir().expect("merge worktree tempdir");
    git_out(
        &repo_dir,
        &[
            "worktree",
            "add",
            "--detach",
            wt.path().to_str().unwrap(),
            "HEAD",
        ],
    );
    git_out(wt.path(), &["merge", "--no-edit", "worker/run-1"]);
    assert_eq!(
        std::fs::read_to_string(wt.path().join("worker_output.txt")).unwrap(),
        "worker artifact\n",
        "重 merge 后 worker 产物必须出现在合并结果里"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn chain_not_truncated_after_revert_next_diff_shows_it() {
    let env = make_revert_env().await;
    let sid = env.session_id.clone();
    let repo_dir = env.repo.path().to_path_buf();

    // revert 到基线(删 new.txt、base.txt 回 v1)。
    let preview = revert_to_checkpoint_preview_inner(&env.state, sid.clone(), 0)
        .await
        .expect("preview ok");
    revert_to_checkpoint_execute_inner(&env.state, sid.clone(), 0, preview.preview_token)
        .await
        .expect("execute ok");

    // 下一写轮(seq2,与 loop 的 capture_and_record 同核):revert 后
    // 的净效果 = base.txt 回到 v1(v2→v1)+ new.txt 消失。先制造一个
    // 新内容让 seq2 树 ≠ seq0 树,diff 更可断言。
    std::fs::write(repo_dir.join("base.txt"), "v3-after-revert\n").unwrap();
    std::fs::write(repo_dir.join("post.txt"), "post revert\n").unwrap();
    seed_snapshot(&env, &sid, 2).await;

    // 链不截断:0/1/2 全在;seq2 的 prev = 1(revert 不回滚不剪链)。
    let list = list_turn_checkpoints_inner(&env.state, sid.clone())
        .await
        .expect("list ok");
    let seqs: Vec<i64> = list.iter().map(|s| s.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2]);

    // 轮间 diff(seq1 → seq2)含 revert 影响:base.txt 被改回(经 v1
    // 再到 v3)、new.txt 消失、post.txt 新增 —— 历史可审计。
    let diff = get_turn_checkpoint_diff_inner(&env.state, sid, 2)
        .await
        .expect("turn diff ok");
    let mut paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["base.txt", "new.txt", "post.txt"]);
    let new_file = diff.files.iter().find(|f| f.path == "new.txt").unwrap();
    assert_eq!(new_file.status, "deleted");
}
