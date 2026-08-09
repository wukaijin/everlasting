#![cfg(test)]

use uuid::Uuid;

use crate::llm::types::{ContentBlock, MessageContent, Role};
use crate::projects::DEFAULT_PROJECT_ID;

use super::make_pool;
use super::sessions::{
    create_session, find_message_id_by_seq, load_session, persist_turn, record_tool_duration,
    update_message_latency, MessageLatency,
};

#[tokio::test]
async fn persist_turn_with_latency_writes_three_columns() {
    // F5 PRD R3: assistant turns persist with the three latency
    // columns. Pre-F5 callers can pass `None` and the columns
    // stay NULL (verified by the `persist_turn_with_no_latency`
    // test below). The columns are nullable so a legacy
    // pre-upgrade session doesn't error out on rehydrate.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "ok".to_string(),
        cache_control: None,
    }]);
    let latency = MessageLatency {
        ttfb_ms: Some(420),
        gen_ms: Some(2100),
        total_ms: Some(3200),
        thinking_ms: Some(850),
    };
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &content,
        0,
        Some(&latency),
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let m = loaded.messages.first().expect("one message");
    assert_eq!(m.ttfb_ms, Some(420));
    assert_eq!(m.gen_ms, Some(2100));
    assert_eq!(m.total_ms, Some(3200));
    // F5 follow-up: thinking_ms round-trips through
    // `persist_turn` (the agent loop's path). The
    // `update_message_thinking` IPC is a separate write
    // that fires AFTER the controller sees `done`, so
    // the `Some(850)` value here proves the column +
    // `INSERT ... VALUES` bind order is correct.
    assert_eq!(m.thinking_ms, Some(850));
}

#[tokio::test]
async fn persist_turn_with_per_turn_latency_writes_4_columns_for_each_turn() {
    // F5 follow-up per-turn: a 3-turn agent response
    // (thinking→shell→tool_result×2→text) persists 3
    // assistant rows, each with its own 4-column
    // MessageLatency populated. This locks the
    // "per-turn rows all have 4 columns" contract that
    // the F5 single-value `req.thinkingDurationMs`
    // path violated (only the LAST turn's row had
    // `thinking_ms` set; the first N-1 were NULL and
    // rendered as "—" on reload).
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let mk_content = |text: &str| -> MessageContent {
        MessageContent::Blocks(vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }])
    };

    // Turn 0 (assistant, seq=0): thinkingMs=200, totalMs=350
    let lat0 = MessageLatency {
        ttfb_ms: Some(180),
        gen_ms: Some(170),
        total_ms: Some(350),
        thinking_ms: Some(200),
    };
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &mk_content("t0 answer"),
        0,
        Some(&lat0),
        None,
    )
    .await
    .unwrap();

    // Turn 1 (assistant, seq=1): thinkingMs=300, totalMs=450
    let lat1 = MessageLatency {
        ttfb_ms: Some(220),
        gen_ms: Some(230),
        total_ms: Some(450),
        thinking_ms: Some(300),
    };
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &mk_content("t1 answer"),
        1,
        Some(&lat1),
        None,
    )
    .await
    .unwrap();

    // Turn 2 (assistant, seq=2): thinkingMs=500, totalMs=900
    let lat2 = MessageLatency {
        ttfb_ms: Some(300),
        gen_ms: Some(600),
        total_ms: Some(900),
        thinking_ms: Some(500),
    };
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &mk_content("t2 final answer"),
        2,
        Some(&lat2),
        None,
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3);

    // Each assistant row has its own per-turn 4-column
    // latency. seq-lookup mirrors the agent loop's
    // per-turn seq assignment.
    let m0 = &loaded.messages[0];
    assert_eq!(m0.ttfb_ms, Some(180));
    assert_eq!(m0.gen_ms, Some(170));
    assert_eq!(m0.total_ms, Some(350));
    assert_eq!(m0.thinking_ms, Some(200));

    let m1 = &loaded.messages[1];
    assert_eq!(m1.ttfb_ms, Some(220));
    assert_eq!(m1.gen_ms, Some(230));
    assert_eq!(m1.total_ms, Some(450));
    assert_eq!(m1.thinking_ms, Some(300));

    let m2 = &loaded.messages[2];
    assert_eq!(m2.ttfb_ms, Some(300));
    assert_eq!(m2.gen_ms, Some(600));
    assert_eq!(m2.total_ms, Some(900));
    // THIS is the F5 follow-up contract: the LAST
    // row's `thinking_ms` is the LAST turn's thinking
    // duration (500ms), NOT the first turn's (200ms,
    // which is what the F5 single-value
    // `req.thinkingDurationMs` produced — the bug the
    // user's "Thought for —" screenshot hit).
    assert_eq!(m2.thinking_ms, Some(500));
}

#[tokio::test]
async fn persist_turn_with_no_latency_leaves_columns_null() {
    // Tool-result rows (the user-role turn the agent loop persists
    // after tool execution) do not have a latency triple — the
    // per-tool duration lives in the content JSON, not on the row.
    // `persist_turn` accepts `None` and the three columns stay NULL.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "ok".to_string(),
        cache_control: None,
    }]);
    persist_turn(&pool, &s.id, Role::User, &content, 0, None, None)
        .await
        .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let m = loaded.messages.first().expect("one message");
    assert!(m.ttfb_ms.is_none());
    assert!(m.gen_ms.is_none());
    assert!(m.total_ms.is_none());
}

#[tokio::test]
async fn update_message_latency_patches_columns_by_id() {
    // The frontend's `update_message_latency` IPC calls this
    // function on `done`. Verify a single UPDATE writes the three
    // columns. The seq → id lookup is in `find_message_id_by_seq`
    // (next test).
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "ok".to_string(),
        cache_control: None,
    }]);
    persist_turn(&pool, &s.id, Role::Assistant, &content, 0, None, None)
        .await
        .unwrap();

    let id = find_message_id_by_seq(&pool, &s.id, 0)
        .await
        .unwrap()
        .expect("id present");

    update_message_latency(
        &pool,
        id,
        &MessageLatency {
            ttfb_ms: Some(100),
            gen_ms: Some(200),
            total_ms: Some(300),
            thinking_ms: Some(75),
        },
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let m = loaded.messages.first().expect("one message");
    assert_eq!(m.ttfb_ms, Some(100));
    assert_eq!(m.gen_ms, Some(200));
    assert_eq!(m.total_ms, Some(300));
    // F5 follow-up: thinking_ms is patched in the same
    // UPDATE statement as the three latency columns. A
    // non-None value here proves the bind order in
    // `update_message_latency`'s SQL is correct (the
    // frontend passes `thinking_ms` as the 4th payload
    // field; the `WHERE id = ?` is the 5th bind).
    assert_eq!(m.thinking_ms, Some(75));
}

#[tokio::test]
async fn update_message_latency_accepts_partial_payload() {
    // Cancel / error paths may only know the total — `ttfb_ms` and
    // `gen_ms` are NULL when the user hits Stop before the first
    // delta. The function must accept a partial MessageLatency
    // without panicking and write NULL for the missing fields.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "ok".to_string(),
        cache_control: None,
    }]);
    persist_turn(&pool, &s.id, Role::Assistant, &content, 0, None, None)
        .await
        .unwrap();
    let id = find_message_id_by_seq(&pool, &s.id, 0)
        .await
        .unwrap()
        .unwrap();

    update_message_latency(
        &pool,
        id,
        &MessageLatency {
            ttfb_ms: None,
            gen_ms: None,
            total_ms: Some(500),
            thinking_ms: None,
        },
    )
    .await
    .unwrap();

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let m = loaded.messages.first().expect("one message");
    assert!(m.ttfb_ms.is_none());
    assert!(m.gen_ms.is_none());
    assert_eq!(m.total_ms, Some(500));
    // F5 follow-up: thinking_ms is also nullable in the
    // partial-payload case (the model never entered the
    // thinking phase, or the cancel cleanup path fired
    // before the thinking close). The column round-trips
    // `None` cleanly.
    assert!(m.thinking_ms.is_none());
}

#[tokio::test]
async fn update_message_latency_patches_thinking_ms_independently() {
    // F5 follow-up: a turn that produced zero thinking (the
    // model answered without a `thinking_delta` event) but
    // had a real latency triple. The patch should land
    // `thinking_ms = None` (column stays NULL because
    // `persist_turn` wrote NULL) and `total_ms = Some(800)`,
    // AND the IPC's UPDATE must not accidentally clear the
    // `ttfb_ms` / `gen_ms` columns when the payload omits
    // the sub-components. Locks the bind order in the
    // single SQL statement.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "ok".to_string(),
        cache_control: None,
    }]);
    // Persist with a non-None latency triple but
    // `thinking_ms = None` — the agent loop's path doesn't
    // know thinking-time at persist time; the controller
    // fires the IPC to patch it after `done`.
    let latency = MessageLatency {
        ttfb_ms: Some(50),
        gen_ms: Some(750),
        total_ms: Some(800),
        thinking_ms: None,
    };
    persist_turn(
        &pool,
        &s.id,
        Role::Assistant,
        &content,
        0,
        Some(&latency),
        None,
    )
    .await
    .unwrap();

    // No follow-up IPC this time — thinking_ms stays NULL.
    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let m = loaded.messages.first().expect("one message");
    assert_eq!(m.ttfb_ms, Some(50));
    assert_eq!(m.gen_ms, Some(750));
    assert_eq!(m.total_ms, Some(800));
    assert!(m.thinking_ms.is_none());
}

#[tokio::test]
async fn find_message_id_by_seq_returns_none_for_unknown_pair() {
    // Defensive: a controller racing the agent loop's
    // `persist_turn` (cancel cleanup persists after `done`)
    // could fire `update_message_latency` before the row exists.
    // The lookup must return `None`, not error.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let id = find_message_id_by_seq(&pool, &s.id, 999).await.unwrap();
    assert!(id.is_none());
}

#[tokio::test]
async fn record_tool_duration_patches_matching_tool_result_block() {
    // F5 PRD R2 / ADR-lite decision 1: per-tool duration is
    // embedded in the `tool_result` block of `messages.content`
    // JSON. The function reads the message, walks the content
    // array, finds the matching `tool_use_id`, and writes
    // `{"duration_ms": <n>}` into the block. Other blocks in
    // the array are untouched.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // Persist a user-role turn with TWO tool_result blocks; only
    // the second one should get the patch.
    let content = MessageContent::Blocks(vec![
        ContentBlock::ToolResult {
            tool_use_id: "toolu_abc".to_string(),
            content: "result for tool 1".to_string(),
            is_error: false,
        },
        ContentBlock::ToolResult {
            tool_use_id: "toolu_def".to_string(),
            content: "result for tool 2".to_string(),
            is_error: false,
        },
    ]);
    persist_turn(&pool, &s.id, Role::User, &content, 0, None, None)
        .await
        .unwrap();

    let patched = record_tool_duration(&pool, &s.id, "toolu_def", 250)
        .await
        .unwrap();
    assert!(patched, "patch landed on a block");

    let loaded = load_session(&pool, &s.id).await.unwrap().unwrap();
    let blocks = loaded.messages[0]
        .content
        .as_array()
        .expect("content is array");
    assert_eq!(blocks.len(), 2);
    // First block: untouched.
    assert_eq!(
        blocks[0].get("duration_ms"),
        None,
        "first tool_result must NOT have duration_ms"
    );
    // Second block: duration_ms set.
    assert_eq!(
        blocks[1].get("duration_ms").and_then(|v| v.as_i64()),
        Some(250)
    );
    // tool_use_id preserved verbatim (the patch must not mutate
    // the other fields).
    assert_eq!(
        blocks[1].get("tool_use_id").and_then(|v| v.as_str()),
        Some("toolu_def")
    );
}

#[tokio::test]
async fn record_tool_duration_returns_false_when_no_block_matches() {
    // Defensive: a `tool:result` event for a tool_use the agent
    // loop never persisted (e.g. the cancel cleanup) is a no-op,
    // not an error. The IPC consumer (frontend) treats `Ok(false)`
    // as a benign outcome.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::ToolResult {
        tool_use_id: "toolu_existing".to_string(),
        content: "x".to_string(),
        is_error: false,
    }]);
    persist_turn(&pool, &s.id, Role::User, &content, 0, None, None)
        .await
        .unwrap();

    let patched = record_tool_duration(&pool, &s.id, "toolu_never_persisted", 100)
        .await
        .unwrap();
    assert!(!patched);
}

#[tokio::test]
async fn record_tool_duration_handles_text_only_message_without_error() {
    // A text-only user message has no tool_result blocks; the
    // function must return Ok(false) (no error) and not touch the
    // content JSON.
    let pool = make_pool().await;
    let s = create_session(
        &pool,
        &Uuid::new_v4().to_string(),
        DEFAULT_PROJECT_ID,
        "/tmp",
        "GLM-4.7",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let content = MessageContent::Blocks(vec![ContentBlock::Text {
        text: "hello".to_string(),
        cache_control: None,
    }]);
    persist_turn(&pool, &s.id, Role::User, &content, 0, None, None)
        .await
        .unwrap();

    let patched = record_tool_duration(&pool, &s.id, "toolu_anything", 100)
        .await
        .unwrap();
    assert!(!patched);
}
