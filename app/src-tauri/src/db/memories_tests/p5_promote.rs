#![cfg(test)]

use sqlx::SqlitePool;

use super::make_pool;
use super::memories::{
    bump_hit_count, get_memory_by_id, promote_if_eligible, test_helpers::insert_raw, MemoryKind,
    MemoryScope, MemoryStatus, ACTIVE_TO_VERIFIED_AGE_DAYS, ACTIVE_TO_VERIFIED_AT,
    CANDIDATE_TO_ACTIVE_AT,
};

// ---------------------------------------------------------------------------
// P5 (2026-06-29, 06-29-am-p5-quality): promote_if_eligible
// state-machine auto-promotion (design D2 + §5)
// ---------------------------------------------------------------------------

/// Insert a row with an explicit `created_at` (so the age gate for
/// active→verified can be tested without waiting 3 real days).
/// `insert_raw` always stamps `now`; this helper re-stamps afterwards.
async fn reseat_created_at(pool: &SqlitePool, memory_id: &str, days_ago: i64) {
    let ts = (chrono::Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339();
    sqlx::query("UPDATE autonomous_memories SET created_at = ? WHERE memory_id = ?")
        .bind(&ts)
        .bind(memory_id)
        .execute(pool)
        .await
        .unwrap();
}

/// `promote_if_eligible` promotes a candidate to active when
/// `hit_count` crosses `CANDIDATE_TO_ACTIVE_AT` (D2: 2).
#[tokio::test]
async fn p5_promote_candidate_to_active_at_threshold() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "title",
        "content",
    )
    .await
    .unwrap();
    // hit_count=1 → below threshold, stays candidate.
    bump_hit_count(&pool, "p5-1").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-1")
            .await
            .unwrap()
            .unwrap()
            .status,
        "candidate"
    );
    // hit_count=2 → crosses CANDIDATE_TO_ACTIVE_AT → promoted.
    bump_hit_count(&pool, "p5-1").await.unwrap();
    let row = get_memory_by_id(&pool, "p5-1").await.unwrap().unwrap();
    assert_eq!(row.status, "active");
    assert_eq!(row.hit_count, CANDIDATE_TO_ACTIVE_AT);
}

/// Active → verified requires BOTH the hit threshold AND the age
/// gate. Below either, stays active.
#[tokio::test]
async fn p5_promote_active_to_verified_needs_hits_and_age() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-2",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();

    // Freshly created (age=0). Bump to ACTIVE_TO_VERIFIED_AT — but
    // age gate not met → stays active.
    for _ in 0..ACTIVE_TO_VERIFIED_AT {
        bump_hit_count(&pool, "p5-2").await.unwrap();
    }
    let row = get_memory_by_id(&pool, "p5-2").await.unwrap().unwrap();
    assert_eq!(row.status, "active", "age gate not met → stays active");
    assert_eq!(row.hit_count, ACTIVE_TO_VERIFIED_AT);

    // Now pretend it was created 3+ days ago. Next bump (hit=6)
    // re-checks + promotes.
    reseat_created_at(&pool, "p5-2", ACTIVE_TO_VERIFIED_AGE_DAYS).await;
    bump_hit_count(&pool, "p5-2").await.unwrap();
    let row = get_memory_by_id(&pool, "p5-2").await.unwrap().unwrap();
    assert_eq!(row.status, "verified");
    assert!(row.hit_count > ACTIVE_TO_VERIFIED_AT);

    // Symmetric control: an active memory at the hit threshold but
    // 1 day short of the age gate stays active.
    insert_raw(
        &pool,
        "p5-2b",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "title",
        "content",
    )
    .await
    .unwrap();
    for _ in 0..ACTIVE_TO_VERIFIED_AT {
        bump_hit_count(&pool, "p5-2b").await.unwrap();
    }
    reseat_created_at(&pool, "p5-2b", ACTIVE_TO_VERIFIED_AGE_DAYS - 1).await;
    bump_hit_count(&pool, "p5-2b").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-2b")
            .await
            .unwrap()
            .unwrap()
            .status,
        "active",
        "1 day short of age gate → stays active"
    );
}

/// A `demoted` row is never re-promoted by `promote_if_eligible` —
/// re-promotion is the hygiene job's job (design §5).
#[tokio::test]
async fn p5_promote_skips_demoted_rows() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-3",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Demoted,
        "title",
        "content",
    )
    .await
    .unwrap();
    // Bump many times — should NOT promote (matrix + the D2 match
    // arm only fires on Candidate/Active).
    for _ in 0..(ACTIVE_TO_VERIFIED_AT + 2) {
        bump_hit_count(&pool, "p5-3").await.unwrap();
    }
    assert_eq!(
        get_memory_by_id(&pool, "p5-3")
            .await
            .unwrap()
            .unwrap()
            .status,
        "demoted",
        "demoted rows stay demoted through bump"
    );
}

/// `promote_if_eligible` is a no-op on an unknown memory_id
/// (NotFound is benign — row deleted between bump + promote).
#[tokio::test]
async fn p5_promote_unknown_id_is_noop() {
    let pool = make_pool().await;
    promote_if_eligible(&pool, "does-not-exist")
        .await
        .expect("unknown id → Ok(()) benign no-op");
}

/// A verified row stays verified through further bumps (no
/// demotion via bump — demotion is the hygiene job's job).
#[tokio::test]
async fn p5_promote_verified_stays_verified() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-5",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Verified,
        "title",
        "content",
    )
    .await
    .unwrap();
    bump_hit_count(&pool, "p5-5").await.unwrap();
    bump_hit_count(&pool, "p5-5").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-5")
            .await
            .unwrap()
            .unwrap()
            .status,
        "verified"
    );
}

/// A candidate below the active threshold stays candidate through
/// bumps (hit=1 < CANDIDATE_TO_ACTIVE_AT).
#[tokio::test]
async fn p5_promote_candidate_below_threshold_stays_candidate() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "p5-6",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Candidate,
        "title",
        "content",
    )
    .await
    .unwrap();
    // Single bump → hit=1 < CANDIDATE_TO_ACTIVE_AT(=2) → no promote.
    bump_hit_count(&pool, "p5-6").await.unwrap();
    assert_eq!(
        get_memory_by_id(&pool, "p5-6")
            .await
            .unwrap()
            .unwrap()
            .status,
        "candidate"
    );
}
