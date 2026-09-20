#![cfg(test)]

//! N2 checkpoint loop 接线集成测试(2026-09-20, task
//! `09-20-n2-checkpoint-revert` PR1):`run_chat_loop` 真链路 +
//! MockProvider 脚本 + 真实 git 仓库(project tempdir),钉住:
//!
//! - **基线轮首可达(AC10)**:首轮 write_file 的 session,基线树 =
//!   会话开始前状态,revert 到基线把首轮写入整树退掉;
//! - **写触发门(AC9)**:纯只读 session 仅基线 1 行,且除基线悬空
//!   commit 外**零新增 git 对象**(content addressing 下基线树与
//!   HEAD 树共享既有对象);
//! - **A2+ 判写 shell 信号(AC8 的门机制)**:写重定向 shell 的写入
//!   在**当轮**快照收编(真实 merge_worker 同门,信号分类单测见
//!   `agent/checkpoint.rs::tests`);
//! - **fail-open + EUNMERGED**:conflicted repo 连续写轮零新行,轮
//!   收尾照常(Done 正常发出、无 Error);
//! - **config gate**:`checkpoints_enabled=false` 整体旁路;
//! - **群聊 session 跳过**(PRD R5;门读 session_type);
//! - **删会话清理(AC6)**:`delete_session_inner` 清伞 ref + FK
//!   CASCADE 清行;worktree 路径失效时**恒回退主仓库再试**(评审
//!   修正的泄漏堵口)。
//!
//! harness:`make_harness_with_git_repo`(真 git repo + seed commit)
//! 之上把 `projects.is_git_repo` 翻成 1 —— 与 loop 自身的 git 门
//! (system prompt HEAD 块同源旗标)保持一致。

use std::process::Command as StdCommand;
use std::sync::Arc;

use super::tests_common::{
    chat_loop_deps, chat_loop_request, commit_all_for_test, make_harness_with_git_repo,
    parent_role, test_messages, MockEmitter, TestHarness,
};
use crate::agent::chat_loop::run_chat_loop;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

/// harness 变体:git 仓库 + `projects.is_git_repo = 1`(checkpoint
/// 总门的 git 条件)。
async fn make_git_harness() -> TestHarness {
    let h = make_harness_with_git_repo().await;
    sqlx::query("UPDATE projects SET is_git_repo = 1 WHERE id = ?")
        .bind(&h.project_id)
        .execute(&h.db)
        .await
        .expect("set is_git_repo");
    h
}

fn tool_use_turn(id: &str, name: &str, input: serde_json::Value) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }),
        Ok(ChatEvent::Done {
            stop_reason: Some("tool_use".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

fn end_turn(text: &str) -> MockResponse {
    MockResponse::Events(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Delta { text: text.into() }),
        Ok(ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: Some(TokenUsage::default()),
        }),
    ])
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
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

/// `git cat-file --batch-all-objects --batch-check` 的对象总数 ——
/// AC9「零新增 git 对象」的度量。
fn count_git_objects(dir: &std::path::Path) -> usize {
    let out = git_out(dir, &["cat-file", "--batch-all-objects", "--batch-check"]);
    out.lines().filter(|l| !l.trim().is_empty()).count()
}

fn umbrella_ref_out(dir: &std::path::Path, sid: &str) -> String {
    git_out(dir, &["for-each-ref", &format!("refs/everlasting/{sid}")])
}

async fn run_script(h: &TestHarness, emitter: Arc<MockEmitter>, script: Vec<MockResponse>) {
    let mock = MockProvider::new(script);
    run_chat_loop(
        chat_loop_request(
            vec![],
            Arc::new(mock),
            200_000,
            format!("rid-ckpt-{}", h.session_id),
            h.session_id.clone(),
            test_messages(),
            emitter,
        ),
        chat_loop_deps(h),
        parent_role(h),
    )
    .await;
}

// ---------------------------------------------------------------------------
// AC10:基线在轮首,首轮写入不被吃进基线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn baseline_at_loop_entry_keeps_pre_session_state_reachable() {
    let h = make_git_harness().await;
    // 会话开始前的磁盘状态:tracked.txt = v1(已提交,workdir clean)。
    std::fs::write(h.project_path.join("tracked.txt"), "v1\n").unwrap();
    commit_all_for_test(&h.project_path, "add tracked");

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            // 首轮即写入(评审 P0 场景):若基线建在轮末,v1 状态无路可达。
            tool_use_turn(
                "t1",
                "write_file",
                serde_json::json!({"path": "tracked.txt", "content": "v2\n"}),
            ),
            end_turn("done"),
        ],
    )
    .await;

    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "基线(seq0)+ 写轮(seq1);纯文本收尾轮零行");
    assert_eq!(rows[0].seq, 0, "基线挂首轮 user 行 seq(fresh session = 0)");
    assert_eq!(rows[1].seq, 1, "轮末快照挂本轮 assistant 行 seq");
    assert_ne!(rows[0].tree_sha, rows[1].tree_sha, "写入必须推进树");
    // 内容寻址去重零行外收益:链头 commit 各不相同。
    assert_ne!(rows[0].commit_sha, rows[1].commit_sha);

    // AC10:revert 到基线 = 回到会话开始前(tracked.txt 回 v1)。
    let repo = git2::Repository::open(&h.project_path).unwrap();
    let base_tree = git2::Oid::from_str(&rows[0].tree_sha).unwrap();
    let set = crate::git::checkpoint::compute_restore_set(&repo, base_tree).unwrap();
    let outcome = crate::git::checkpoint::restore_paths(&repo, base_tree, &set).unwrap();
    assert_eq!(outcome.restored, 1, "还原集 = tracked.txt 一项");
    assert_eq!(
        std::fs::read_to_string(h.project_path.join("tracked.txt")).unwrap(),
        "v1\n",
        "首轮写入必须被基线 revert 退掉"
    );

    // 伞 ref 存在且指向链尾(单伞簿记,AC6 的可达前提)。
    let ref_out = umbrella_ref_out(&h.project_path, &h.session_id);
    assert!(
        ref_out.contains(&rows[1].commit_sha),
        "refs/everlasting/<sid> 必须指向链头, got: {ref_out}"
    );
}

// ---------------------------------------------------------------------------
// AC9:纯只读 session 仅基线 1 行、零新增对象
// ---------------------------------------------------------------------------

#[tokio::test]
async fn readonly_session_leaves_only_baseline_and_no_new_objects() {
    let h = make_git_harness().await;
    let objects_before = count_git_objects(&h.project_path);

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            tool_use_turn("r1", "read_file", serde_json::json!({"path": "seed.txt"})),
            end_turn("here is the file"),
        ],
    )
    .await;

    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "纯只读 session 仅基线 1 行(AC9)");
    assert_eq!(rows[0].seq, 0);

    // 零新增 git 对象(基线 tree 除外):基线树与 HEAD 树同内容,
    // content addressing 下共享既有对象 —— 全 session 唯一新对象是
    // 基线的悬空 commit 本身。
    assert_eq!(
        count_git_objects(&h.project_path) - objects_before,
        1,
        "只读轮不得产生任何 git 对象(写触发门的零扫描语义)"
    );

    // 轮收尾照常:end_turn 正常 Done。
    assert_eq!(emitter.error_event_count(), 0);
}

// ---------------------------------------------------------------------------
// A2+ 判写 shell 信号:写重定向 shell 当轮收编(AC8 的门机制)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_shell_signal_snapshots_the_same_turn() {
    let h = make_git_harness().await;

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            // 前台 shell + 写重定向 → classify_prefix ≥ SideEffect →
            // 写信号 → 当轮快照。真实 merge_worker 走同一写触发门。
            tool_use_turn(
                "s1",
                "shell",
                serde_json::json!({"command": "echo worker-product > merged.txt"}),
            ),
            end_turn("merged"),
        ],
    )
    .await;

    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "基线 + 写轮(当轮收编,不晚一轮)");
    assert_eq!(rows[1].seq, 1);

    // 轮间 diff(基线 → 轮1)恰含 shell 产物 —— 该轮「本轮 diff」可见。
    let repo = git2::Repository::open(&h.project_path).unwrap();
    let a = repo
        .find_tree(git2::Oid::from_str(&rows[0].tree_sha).unwrap())
        .unwrap();
    let b = repo
        .find_tree(git2::Oid::from_str(&rows[1].tree_sha).unwrap())
        .unwrap();
    let diff = crate::git::diff::diff_tree_to_tree(&repo, &a, &b).unwrap();
    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["merged.txt"]);
}

// ---------------------------------------------------------------------------
// fail-open:EUNMERGED 冲突态连续写轮无新行,轮收尾照常
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conflicted_repo_fails_open_with_zero_new_rows() {
    let h = make_git_harness().await;

    // 制造未解合并冲突(两分支同文件发散 → merge 必撞)。
    git_out(&h.project_path, &["checkout", "-b", "side"]);
    std::fs::write(h.project_path.join("seed.txt"), "side\n").unwrap();
    commit_all_for_test(&h.project_path, "side edit");
    git_out(&h.project_path, &["checkout", "main"]);
    std::fs::write(h.project_path.join("seed.txt"), "main\n").unwrap();
    commit_all_for_test(&h.project_path, "main edit");
    let merge = StdCommand::new("git")
        .args(["merge", "side", "--no-edit"])
        .current_dir(&h.project_path)
        .output()
        .unwrap();
    assert!(!merge.status.success(), "setup: merge 应当冲突");

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            // 连续两个写轮:build_state_tree 每轮都撞 EUNMERGED。
            tool_use_turn(
                "w1",
                "write_file",
                serde_json::json!({"path": "new.txt", "content": "x"}),
            ),
            tool_use_turn(
                "w2",
                "write_file",
                serde_json::json!({"path": "new.txt", "content": "y"}),
            ),
            end_turn("done despite conflict"),
        ],
    )
    .await;

    // fail-open:无 Error 事件、end_turn Done 正常发出 —— 轮收尾不受影响。
    assert_eq!(emitter.error_event_count(), 0, "快照失败不得干扰轮收尾");
    let done = emitter.chat_events().iter().any(|p| {
        matches!(&p.event, ChatEvent::Done { stop_reason, .. } if stop_reason.as_deref() == Some("end_turn"))
    });
    assert!(done, "end_turn 收尾必须完整走完");

    // 基线与两个写轮全部失败 → 零行(EUNMERGED 连续冲突轮无新行)。
    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert!(rows.is_empty(), "冲突态下基线与写轮快照都应 fail-open 掉");
    assert!(umbrella_ref_out(&h.project_path, &h.session_id)
        .trim()
        .is_empty());
}

// ---------------------------------------------------------------------------
// config gate:checkpoints_enabled=false 整体旁路
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_gate_off_disables_snapshots() {
    let h = make_git_harness().await;
    crate::db::config::set_config_value(&h.db, "checkpoints_enabled", "false")
        .await
        .unwrap();

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            tool_use_turn(
                "w1",
                "write_file",
                serde_json::json!({"path": "gate.txt", "content": "x"}),
            ),
            end_turn("done"),
        ],
    )
    .await;

    // 写真实发生了(门只关快照,不关工具),但零行零 ref。
    assert!(h.project_path.join("gate.txt").exists());
    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert!(rows.is_empty());
    assert!(umbrella_ref_out(&h.project_path, &h.session_id)
        .trim()
        .is_empty());
}

// ---------------------------------------------------------------------------
// 群聊 session 跳过(PRD R5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_chat_session_skips_checkpoints() {
    let h = make_git_harness().await;
    sqlx::query("UPDATE sessions SET session_type = 'group_chat' WHERE id = ?")
        .bind(&h.session_id)
        .execute(&h.db)
        .await
        .unwrap();

    let emitter = Arc::new(MockEmitter::new());
    run_script(
        &h,
        emitter.clone(),
        vec![
            tool_use_turn(
                "w1",
                "write_file",
                serde_json::json!({"path": "gc.txt", "content": "x"}),
            ),
            end_turn("done"),
        ],
    )
    .await;

    let rows = crate::db::checkpoint::list_checkpoints(&h.db, &h.session_id)
        .await
        .unwrap();
    assert!(rows.is_empty(), "群聊 session 不建链");
    assert!(umbrella_ref_out(&h.project_path, &h.session_id)
        .trim()
        .is_empty());
}

// ---------------------------------------------------------------------------
// AC6:删会话清伞 ref(恒回退主仓库再试)+ FK CASCADE 清行
// ---------------------------------------------------------------------------

/// 直接经 git 原语给 `h.project_path` 建一条单快照链 + 伞 ref
/// (AC6 钉的是清理链路,不必经 loop)。返回链头 commit。
fn seed_chain_git_side(h: &TestHarness) -> String {
    let repo = git2::Repository::open(&h.project_path).unwrap();
    let tree = crate::git::checkpoint::build_state_tree(&repo).unwrap();
    let commit =
        crate::git::checkpoint::append_snapshot(&repo, None, tree, &h.session_id, 0).unwrap();
    crate::git::checkpoint::set_umbrella_ref(&repo, &h.session_id, commit).unwrap();
    commit.to_string()
}

/// 在 `state.db`(delete_session_inner 消费的库)里补齐 project +
/// session + checkpoint 行。
async fn seed_state_db(h: &TestHarness, state: &crate::state::AppState, commit: &str) {
    crate::db::create_project(
        &state.db,
        "del-project",
        h.project_path.to_str().unwrap(),
        true,
        None,
    )
    .await
    .unwrap();
    let projects = crate::db::list_projects(&state.db, false).await.unwrap();
    let project = projects
        .iter()
        .find(|p| p.path == h.project_path.to_string_lossy())
        .unwrap()
        .clone();
    crate::db::create_session(
        &state.db,
        &h.session_id,
        &project.id,
        h.project_path.to_str().unwrap(),
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .unwrap();
    crate::db::checkpoint::upsert_checkpoint(&state.db, &h.session_id, 0, "tree", commit)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_session_clears_umbrella_ref_and_rows_ac6() {
    let h = make_git_harness().await;
    let commit = seed_chain_git_side(&h);
    assert!(!umbrella_ref_out(&h.project_path, &h.session_id)
        .trim()
        .is_empty());

    let state = Arc::new(crate::state::AppState::load_from_dir(h.app_data_dir.clone()).await);
    seed_state_db(&h, &state, &commit).await;

    crate::commands::sessions::delete_session_inner(&state, h.session_id.clone())
        .await
        .unwrap();

    // 伞 ref 消失(AC6 前半)。
    assert!(
        umbrella_ref_out(&h.project_path, &h.session_id)
            .trim()
            .is_empty(),
        "删会话必须清 refs/everlasting/<sid>"
    );
    // DB 行消失(AC6 后半;FK CASCADE)。
    let rows = crate::db::checkpoint::list_checkpoints(&state.db, &h.session_id)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn dead_worktree_path_falls_back_to_main_repo_for_ref_cleanup() {
    // 评审修正的泄漏场景:session 绑定的 worktree 路径已不存在
    // (detached / 手删),ref 物理落主仓库共享 refs —— 清理必须
    // 回退 project.path 再试,不得静默跳过。
    let h = make_git_harness().await;
    let commit = seed_chain_git_side(&h);

    let state = Arc::new(crate::state::AppState::load_from_dir(h.app_data_dir.clone()).await);
    seed_state_db(&h, &state, &commit).await;
    // 伪造 active 绑定指向已删除的 worktree 路径。
    sqlx::query("UPDATE sessions SET worktree_state = 'active', worktree_path = ? WHERE id = ?")
        .bind(h.project_path.join("does-not-exist").to_str().unwrap())
        .bind(&h.session_id)
        .execute(&state.db)
        .await
        .unwrap();

    crate::commands::sessions::delete_session_inner(&state, h.session_id.clone())
        .await
        .unwrap();

    assert!(
        umbrella_ref_out(&h.project_path, &h.session_id)
            .trim()
            .is_empty(),
        "worktree 路径失效时必须回退主仓库清 ref(否则共享 refs 永久泄漏)"
    );
}
