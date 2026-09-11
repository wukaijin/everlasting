#![cfg(test)]

//! GCE-P1(2026-09-12, task `09-12-gc-preset-settings`)—
//! `create/update_group_chat_preset` 校验矩阵测试(design §6,逐条)。
//!
//! 覆盖:撞内置 key(CI)/ 用户重名(CI)/ update 排除自身 / update
//! 行不存在 / 模型不存在 + disabled 允许 / 参与者 1 与 4 条(2、3 条
//! 通过)/ 参与者名字空·超长·重名 / persona 非法 + 五 kind 全通过 /
//! 名称与描述长度边界。成功路径(CRUD round-trip)在 db 层冒烟
//! (`db/group_chat_presets_tests.rs`),这里只补一条 create→list→delete
//! 闭环锁命令编排。
//!
//! 状态构造沿 `daemon/routes/projects.rs` 测试先例:
//! `AppState::load_from_dir(tempdir)` 建真实池,再经
//! `db::create_provider` + `db::create_model` 显式种模型行
//! (校验经 `db::get_model` 查存在性,必须有行)。

use std::sync::Arc;

use crate::commands::group_chat_presets::{
    create_group_chat_preset_inner, delete_group_chat_preset_inner, list_group_chat_presets_inner,
    update_group_chat_preset_inner, PERSONA_KINDS,
};
use crate::db;
use crate::db::group_chat_presets::GcPresetParticipant;
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

/// 测试态:真实 AppState(tempdir 池)+ 两个种子模型 id。
/// `TempDir` 守卫随结构体持有(池开着文件)。
struct TestEnv {
    state: Arc<AppState>,
    _dir: tempfile::TempDir,
    model_a: String,
    model_b: String,
}

async fn make_env() -> TestEnv {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = Arc::new(AppState::load_from_dir(dir.path().to_path_buf()).await);
    let provider =
        db::create_provider(&state.db, "anthropic", "测试供应商", "https://api.test", "")
            .await
            .expect("seed provider");
    let m1 = db::create_model(
        &state.db,
        &provider.id,
        "test-model-a",
        "测试模型 A",
        None,
        None,
        true,
        false,
        128_000,
    )
    .await
    .expect("seed model a");
    let m2 = db::create_model(
        &state.db,
        &provider.id,
        "test-model-b",
        "测试模型 B",
        None,
        None,
        true,
        false,
        128_000,
    )
    .await
    .expect("seed model b");
    TestEnv {
        state,
        _dir: dir,
        model_a: m1.id,
        model_b: m2.id,
    }
}

/// 两人最小阵容(调用方按需覆写 persona / 名字 / 模型)。
fn roster(a: &str, b: &str) -> Vec<GcPresetParticipant> {
    vec![
        GcPresetParticipant {
            name: "架构".into(),
            model_id: a.into(),
            persona: "arch".into(),
        },
        GcPresetParticipant {
            name: "产品".into(),
            model_id: b.into(),
            persona: "product".into(),
        },
    ]
}

/// 断言结果是 `InvalidRequest`(→ HTTP 400;message 一并打出便于
/// 失败时定位是哪条臂错了)。
fn expect_invalid<T: std::fmt::Debug>(res: Result<T, AppCommandError>, why: &str) {
    let err = res.expect_err(why);
    assert_eq!(
        err.category,
        ErrorCategory::InvalidRequest,
        "{why}: got {:?} ({})",
        err.category,
        err.message
    );
}

// ---------------------------------------------------------------------------
// 成功路径 + 命令编排闭环
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_list_delete_round_trip() {
    let env = make_env().await;
    let row = create_group_chat_preset_inner(
        &env.state,
        "我的评审团".into(),
        "描述".into(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("create ok");
    assert_eq!(row.name, "我的评审团");
    assert_eq!(row.participants.len(), 2);

    let list = list_group_chat_presets_inner(&env.state)
        .await
        .expect("list ok");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, row.id);

    let deleted = delete_group_chat_preset_inner(&env.state, row.id)
        .await
        .expect("delete ok");
    assert!(deleted.ok);
    let list = list_group_chat_presets_inner(&env.state)
        .await
        .expect("list ok");
    assert!(list.is_empty(), "删除后列表为空");
}

// ---------------------------------------------------------------------------
// 校验矩阵(design §3 逐条)
// ---------------------------------------------------------------------------

/// 撞内置 key,大小写不敏感(review / fe_review / arch / retro)。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_builtin_key_case_insensitive() {
    let env = make_env().await;
    for name in [
        "review",
        "REVIEW",
        "Fe_Review",
        "arch",
        "ARCH",
        "retro",
        "Retro",
    ] {
        let res = create_group_chat_preset_inner(
            &env.state,
            name.into(),
            String::new(),
            env.model_a.clone(),
            roster(&env.model_a, &env.model_b),
        )
        .await;
        expect_invalid(res, &format!("撞内置 key:{name}"));
    }
}

/// 与已有用户预设重名,大小写不敏感。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_duplicate_user_name_case_insensitive() {
    let env = make_env().await;
    create_group_chat_preset_inner(
        &env.state,
        "My Team".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("first create ok");
    let res = create_group_chat_preset_inner(
        &env.state,
        "my team".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "用户重名(CI)");
}

/// update 查重排除自身:保留原名合法;改成另一行名(CI)拒绝。
#[tokio::test(flavor = "multi_thread")]
async fn update_uniqueness_excludes_self() {
    let env = make_env().await;
    let row_a = create_group_chat_preset_inner(
        &env.state,
        "Team A".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("create A");
    create_group_chat_preset_inner(
        &env.state,
        "Team B".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("create B");

    // 保留原名(update 排除自身)→ ok。
    let kept = update_group_chat_preset_inner(
        &env.state,
        row_a.id.clone(),
        "Team A".into(),
        "改描述".into(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("update with unchanged name ok");
    assert_eq!(kept.description, "改描述");

    // 改成 Team B(不同大小写)→ 拒。
    let res = update_group_chat_preset_inner(
        &env.state,
        row_a.id,
        "team b".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "update 撞他行名(CI)");
}

/// update 行不存在 → InvalidRequest(防编辑竞态静默)。
#[tokio::test(flavor = "multi_thread")]
async fn update_missing_row_is_invalid_request() {
    let env = make_env().await;
    let res = update_group_chat_preset_inner(
        &env.state,
        "no-such-id".into(),
        "随便".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "update 不存在");
}

/// 模型不存在(moderator 与 participants 分别缺)→ 400 且 message
/// 指明「模型不存在」。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_missing_models() {
    let env = make_env().await;
    // moderator 缺。
    let res = create_group_chat_preset_inner(
        &env.state,
        "缺主持".into(),
        String::new(),
        "ghost-model".into(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    let err = res.expect_err("moderator 模型缺必须被拒绝");
    assert_eq!(err.category, ErrorCategory::InvalidRequest);
    assert!(err.message.contains("模型不存在"), "got: {}", err.message);
    // participant 缺。
    let mut bad = roster(&env.model_a, &env.model_b);
    bad[1].model_id = "ghost-model".into();
    let res = create_group_chat_preset_inner(
        &env.state,
        "缺参与".into(),
        String::new(),
        env.model_a.clone(),
        bad,
    )
    .await;
    expect_invalid(res, "participants 模型不存在");
}

/// disabled 模型允许保存(禁用是使用处问题,保存不拦)。
#[tokio::test(flavor = "multi_thread")]
async fn create_allows_disabled_models() {
    let env = make_env().await;
    db::set_model_disabled(&env.state.db, &env.model_b, true)
        .await
        .expect("disable model b");
    let row = create_group_chat_preset_inner(
        &env.state,
        "含禁用模型".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("disabled model must be allowed at save time");
    assert_eq!(row.participants[1].model_id, env.model_b);
}

/// 参与者 1 条(低于下限)与 4 条(高于上限)均拒绝;3 条(上限)通过。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_participant_count_out_of_bounds() {
    let env = make_env().await;
    let one = vec![roster(&env.model_a, &env.model_b)[0].clone()];
    let res = create_group_chat_preset_inner(
        &env.state,
        "独角戏".into(),
        String::new(),
        env.model_a.clone(),
        one,
    )
    .await;
    expect_invalid(res, "participants 1 条");

    let four = vec![
        GcPresetParticipant {
            name: "甲".into(),
            model_id: env.model_a.clone(),
            persona: "arch".into(),
        },
        GcPresetParticipant {
            name: "乙".into(),
            model_id: env.model_a.clone(),
            persona: "product".into(),
        },
        GcPresetParticipant {
            name: "丙".into(),
            model_id: env.model_a.clone(),
            persona: "backend".into(),
        },
        GcPresetParticipant {
            name: "丁".into(),
            model_id: env.model_a.clone(),
            persona: "frontend".into(),
        },
    ];
    let res = create_group_chat_preset_inner(
        &env.state,
        "大团".into(),
        String::new(),
        env.model_a.clone(),
        four.clone(),
    )
    .await;
    expect_invalid(res, "participants 4 条");

    // 3 条(上限)通过。
    let three: Vec<GcPresetParticipant> = four.into_iter().take(3).collect();
    let row = create_group_chat_preset_inner(
        &env.state,
        "三人团".into(),
        String::new(),
        env.model_a.clone(),
        three,
    )
    .await
    .expect("3 人(上限)必须通过");
    assert_eq!(row.participants.len(), 3);
}

/// 参与者名字:空白 / 超 20 字符 / 预设内重名(trim 形态)均拒绝。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_bad_participant_names() {
    let env = make_env().await;
    let cases: Vec<(&str, Vec<GcPresetParticipant>)> = vec![
        (
            "名字空白",
            vec![
                GcPresetParticipant {
                    name: "   ".into(),
                    model_id: env.model_a.clone(),
                    persona: "arch".into(),
                },
                GcPresetParticipant {
                    name: "产品".into(),
                    model_id: env.model_b.clone(),
                    persona: "product".into(),
                },
            ],
        ),
        (
            "名字超长",
            vec![
                GcPresetParticipant {
                    name: "长".repeat(21),
                    model_id: env.model_a.clone(),
                    persona: "arch".into(),
                },
                GcPresetParticipant {
                    name: "产品".into(),
                    model_id: env.model_b.clone(),
                    persona: "product".into(),
                },
            ],
        ),
        (
            "重名(trim 后)",
            vec![
                GcPresetParticipant {
                    name: "架构".into(),
                    model_id: env.model_a.clone(),
                    persona: "arch".into(),
                },
                GcPresetParticipant {
                    name: "架构 ".into(),
                    model_id: env.model_b.clone(),
                    persona: "backend".into(),
                },
            ],
        ),
    ];
    for (why, participants) in cases {
        let res = create_group_chat_preset_inner(
            &env.state,
            "坏名单".into(),
            String::new(),
            env.model_a.clone(),
            participants,
        )
        .await;
        expect_invalid(res, why);
    }
}

/// persona 白名单:非法 kind 拒绝;五种内置 kind 各建一场均通过。
#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_invalid_persona_and_accepts_all_kinds() {
    let env = make_env().await;
    let mut bad = roster(&env.model_a, &env.model_b);
    bad[0].persona = "wizard".into();
    let res = create_group_chat_preset_inner(
        &env.state,
        "非法 persona".into(),
        String::new(),
        env.model_a.clone(),
        bad,
    )
    .await;
    expect_invalid(res, "persona 非法");

    // 五 kind 全通过(每场两人同 kind,名字不同避重名)。
    for (i, kind) in PERSONA_KINDS.iter().enumerate() {
        let participants = vec![
            GcPresetParticipant {
                name: "甲".into(),
                model_id: env.model_a.clone(),
                persona: kind.to_string(),
            },
            GcPresetParticipant {
                name: "乙".into(),
                model_id: env.model_b.clone(),
                persona: kind.to_string(),
            },
        ];
        let row = create_group_chat_preset_inner(
            &env.state,
            format!("五团{i}"),
            String::new(),
            env.model_a.clone(),
            participants,
        )
        .await
        .unwrap_or_else(|e| panic!("内置 kind「{kind}」必须通过: {e:?}"));
        assert_eq!(row.participants[0].persona, *kind);
    }
}

/// 名称与描述长度边界(按字符数):40/200 通过,41/201 拒绝;
/// 名称空白(trim 后空)拒绝。
#[tokio::test(flavor = "multi_thread")]
async fn create_enforces_name_and_description_length_bounds() {
    let env = make_env().await;
    // 名称 40 字符(上沿)通过。
    let row = create_group_chat_preset_inner(
        &env.state,
        "a".repeat(40),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("40 字符名称必须通过");
    assert_eq!(row.name.chars().count(), 40);

    // 描述 200 字符(上沿)通过。
    update_group_chat_preset_inner(
        &env.state,
        row.id.clone(),
        "a".repeat(40),
        "描".repeat(200),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await
    .expect("200 字符描述必须通过");

    // 名称 41 字符 → 拒。
    let res = create_group_chat_preset_inner(
        &env.state,
        "a".repeat(41),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "名称超长");

    // 描述 201 字符 → 拒。
    let res = update_group_chat_preset_inner(
        &env.state,
        row.id,
        "a".repeat(40),
        "描".repeat(201),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "描述超长");

    // 名称空白(trim 后空)→ 拒。
    let res = create_group_chat_preset_inner(
        &env.state,
        "   ".into(),
        String::new(),
        env.model_a.clone(),
        roster(&env.model_a, &env.model_b),
    )
    .await;
    expect_invalid(res, "名称空白");
}
