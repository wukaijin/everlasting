#![cfg(test)]

//! group_chat_presets-domain integration tests (2026-09-12,
//! task `09-12-gc-preset-settings`, GCE-P1).
//!
//! Coverage:
//! - create / get / list / update / delete CRUD round-trip
//! - list order is stable (ORDER BY name)
//! - update on unknown row → None; delete is idempotent (false)
//! - name UNIQUE constraint backstop (精确匹配撞名报错;大小写不敏感
//!   与内置 key 校验在 commands 层,见 commands/group_chat_presets.rs)
//! - GCE-P1b(2026-09-12, task `09-12-gc-preset-override`)覆盖行:
//!   builtin_key 往返 / 两条 NULL 行共存(UNIQUE 索引对 NULL 互不相
//!   撞)/ 同 key 第二条覆盖行被 UNIQUE 索引拒绝

use sqlx::SqlitePool;

use super::{
    group_chat_presets::{
        create_group_chat_preset, delete_group_chat_preset, get_group_chat_preset,
        list_group_chat_presets, update_group_chat_preset, GcPresetParticipant,
    },
    test_support::test_pool,
};

async fn make_pool() -> SqlitePool {
    test_pool().await
}

/// 两人预设的最小参与者名单(2..=3 边界的下沿)。
fn roster() -> Vec<GcPresetParticipant> {
    vec![
        GcPresetParticipant {
            name: "架构".into(),
            model_id: "model-uuid-a".into(),
            persona: "arch".into(),
        },
        GcPresetParticipant {
            name: "产品".into(),
            model_id: "model-uuid-b".into(),
            persona: "product".into(),
        },
    ]
}

#[tokio::test]
async fn group_chat_presets_create_get_round_trips_participants_json() {
    let pool = make_pool().await;
    let row = create_group_chat_preset(
        &pool,
        "我的评审团",
        "本地验证用",
        "model-uuid-mod",
        roster(),
        None,
    )
    .await
    .expect("create preset");
    assert!(!row.id.is_empty(), "UUID v4 id 服务端生成");
    assert_eq!(row.name, "我的评审团");
    assert_eq!(row.moderator_model_id, "model-uuid-mod");
    assert_eq!(row.participants.len(), 2);
    assert_eq!(row.builtin_key, None, "不带 builtin_key = 普通用户行");

    // get 回读:participants JSON 列 ↔ Vec 序列化在 db 层闭环。
    let got = get_group_chat_preset(&pool, &row.id)
        .await
        .expect("get")
        .expect("row exists");
    assert_eq!(got, row, "round-trip keeps the whole row");
}

#[tokio::test]
async fn group_chat_presets_get_missing_returns_none() {
    let pool = make_pool().await;
    let got = get_group_chat_preset(&pool, "no-such-id").await.unwrap();
    assert!(got.is_none(), "unknown id → None (not Err)");
}

#[tokio::test]
async fn group_chat_presets_list_is_sorted_and_complete() {
    let pool = make_pool().await;
    // 乱序插入,验证 ORDER BY name。
    create_group_chat_preset(&pool, "zeta团", "", "m", roster(), None)
        .await
        .unwrap();
    create_group_chat_preset(&pool, "alpha团", "", "m", roster(), None)
        .await
        .unwrap();
    create_group_chat_preset(&pool, "mid团", "", "m", roster(), None)
        .await
        .unwrap();
    let all = list_group_chat_presets(&pool).await.unwrap();
    let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["alpha团", "mid团", "zeta团"]);
}

#[tokio::test]
async fn group_chat_presets_update_patches_fields_and_unknown_returns_none() {
    let pool = make_pool().await;
    let row = create_group_chat_preset(&pool, "旧名", "旧描述", "m-old", roster(), None)
        .await
        .unwrap();

    let new_roster = vec![
        GcPresetParticipant {
            name: "后端".into(),
            model_id: "model-uuid-b".into(),
            persona: "backend".into(),
        },
        GcPresetParticipant {
            name: "前端".into(),
            model_id: "model-uuid-a".into(),
            persona: "frontend".into(),
        },
        GcPresetParticipant {
            name: "外人".into(),
            model_id: "model-uuid-c".into(),
            persona: "outsider".into(),
        },
    ];
    let updated = update_group_chat_preset(
        &pool,
        &row.id,
        "新名",
        "新描述",
        "model-uuid-mod",
        new_roster,
    )
    .await
    .unwrap()
    .expect("row exists");
    assert_eq!(updated.name, "新名");
    assert_eq!(updated.description, "新描述");
    assert_eq!(updated.moderator_model_id, "model-uuid-mod");
    assert_eq!(updated.participants.len(), 3);
    assert_eq!(
        updated.created_at, row.created_at,
        "update 不触碰 created_at(re-read 带回真实值)"
    );

    // 未知 id → None(commands 层转 InvalidRequest)。
    let miss = update_group_chat_preset(&pool, "no-such-id", "x", "", "m", roster())
        .await
        .unwrap();
    assert!(miss.is_none());
}

#[tokio::test]
async fn group_chat_presets_delete_removes_row_and_is_idempotent() {
    let pool = make_pool().await;
    let row = create_group_chat_preset(&pool, "待删", "", "m", roster(), None)
        .await
        .unwrap();
    assert!(
        delete_group_chat_preset(&pool, &row.id).await.unwrap(),
        "first delete removes the row"
    );
    assert!(
        !delete_group_chat_preset(&pool, &row.id).await.unwrap(),
        "second delete is a no-op (false,幂等删除)"
    );
    assert!(get_group_chat_preset(&pool, &row.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn group_chat_presets_name_unique_constraint_backstop() {
    let pool = make_pool().await;
    create_group_chat_preset(&pool, "同名", "", "m", roster(), None)
        .await
        .unwrap();
    // 精确匹配重名撞 UNIQUE 列约束(commands 层的大小写不敏感校验在
    // 之前已拦,这里锁 DB 兜底存在且生效)。
    let dup = create_group_chat_preset(&pool, "同名", "", "m", roster(), None).await;
    assert!(dup.is_err(), "exact duplicate name must violate UNIQUE");
}

// ---------------------------------------------------------------------------
// GCE-P1b(2026-09-12, task `09-12-gc-preset-override`)覆盖行
// ---------------------------------------------------------------------------

/// 覆盖行往返:create 带 builtin_key = Some("arch") → list / get 原样
/// 带回(覆盖行 = 普通行的超集,只多链接键一列)。
#[tokio::test]
async fn group_chat_presets_override_row_round_trips_builtin_key() {
    let pool = make_pool().await;
    let row = create_group_chat_preset(
        &pool,
        "架构档重制版",
        "覆盖内置 arch",
        "model-uuid-mod",
        roster(),
        Some("arch"),
    )
    .await
    .expect("create override row");
    assert_eq!(row.builtin_key.as_deref(), Some("arch"));

    let all = list_group_chat_presets(&pool).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, row.id);
    assert_eq!(
        all[0].builtin_key.as_deref(),
        Some("arch"),
        "list 带回链接键"
    );

    let got = get_group_chat_preset(&pool, &row.id)
        .await
        .unwrap()
        .expect("row exists");
    assert_eq!(got, row, "round-trip keeps the override row intact");
}

/// UNIQUE 索引对 NULL 互不相撞(SQLite 语义):两条 builtin_key =
/// None 的普通用户行共库共存,覆盖行唯一性不波及普通行。
#[tokio::test]
async fn group_chat_presets_null_builtin_key_rows_coexist() {
    let pool = make_pool().await;
    create_group_chat_preset(&pool, "团队甲", "", "m", roster(), None)
        .await
        .unwrap();
    create_group_chat_preset(&pool, "团队乙", "", "m", roster(), None)
        .await
        .unwrap();
    let all = list_group_chat_presets(&pool).await.unwrap();
    assert_eq!(all.len(), 2, "两条 builtin_key 全 NULL 的行必须共存");
    assert!(all.iter().all(|r| r.builtin_key.is_none()));
}

/// 同一内置 key 的第二条覆盖行撞 `idx_group_chat_presets_builtin_key`
/// UNIQUE 索引 → insert 报错(commands 层前置查重给可读 400,本索引
/// 是「每个内置 key 至多一条覆盖行」的 DB 层并发兜底)。
#[tokio::test]
async fn group_chat_presets_duplicate_override_key_rejected_by_unique_index() {
    let pool = make_pool().await;
    create_group_chat_preset(&pool, "架构档重制版", "", "m", roster(), Some("arch"))
        .await
        .unwrap();
    let dup = create_group_chat_preset(&pool, "架构档又改", "", "m", roster(), Some("arch")).await;
    assert!(
        dup.is_err(),
        "second override for the same builtin key must violate the UNIQUE index"
    );
}
