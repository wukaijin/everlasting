//! chat_loop 单元测试(迁移自 chat_loop.rs 内联 mod tests,
//! 08-08-a-class-chat-loop-split)。
//!
//! 仅覆盖 `user_message_matches` + `dd_guard_hit`(均留在 hub)。原内联
//! `mod tests` 的 `use super::*;` 改写为 `use super::chat_loop::*;`(sibling
//! module),需要 hub 把两个被测函数暴露为 `pub(crate)`。

#![cfg(test)]

#[allow(unused_imports)]
use std::sync::Arc;

#[allow(unused_imports)]
use tokio::sync::Mutex;

#[allow(unused_imports)]
use super::chat_loop::*;
#[allow(unused_imports)]
use crate::db::MessageRow;
#[allow(unused_imports)]
use crate::llm::{ChatMessage, MessageContent, Role};
#[allow(unused_imports)]
use serde_json::json;

fn db_row(role: &str, content: serde_json::Value, text: &str, seq: i64) -> MessageRow {
    MessageRow {
        id: seq,
        session_id: "s".to_string(),
        role: role.to_string(),
        content,
        text: text.to_string(),
        has_tool_calls: false,
        has_tool_results: false,
        created_at: "t".to_string(),
        seq,
        metadata: None,
        ttfb_ms: None,
        gen_ms: None,
        total_ms: None,
        thinking_ms: None,
        speaker: None,
    }
}

fn user_msg(content: MessageContent) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content,
        speaker: None,
        attachments: None,
    }
}

/// tool_result: same tool_use_id → match (the group_chat reload
/// case this whole fix exists for).
#[test]
fn user_message_matches_tool_result_same_id() {
    let blocks = json!([{
        "type": "tool_result",
        "tool_use_id": "call_abc",
        "content": "Floor handed to M3."
    }]);
    let row = db_row("user", blocks.clone(), "", 4);
    let mem = user_msg(serde_json::from_value(blocks).unwrap());
    assert!(user_message_matches(&row, &mem));
}

/// tool_result: different tool_use_id → no match (different tool
/// interaction, must not be treated as the same row).
#[test]
fn user_message_matches_tool_result_different_id() {
    let row_blocks = json!([{"type":"tool_result","tool_use_id":"call_abc","content":"x"}]);
    let mem_blocks = json!([{"type":"tool_result","tool_use_id":"call_xyz","content":"x"}]);
    let row = db_row("user", row_blocks, "", 4);
    let mem = user_msg(serde_json::from_value(mem_blocks).unwrap());
    assert!(!user_message_matches(&row, &mem));
}

/// plain text: identical text → match.
#[test]
fn user_message_matches_plain_text_equal() {
    let row = db_row("user", json!("hello"), "hello", 2);
    let mem = user_msg(MessageContent::Text("hello".to_string()));
    assert!(user_message_matches(&row, &mem));
}

/// plain text: different text → no match (a fresh send whose text
/// differs from the prior persisted row — the normal-chat case).
#[test]
fn user_message_matches_plain_text_different() {
    let row = db_row("user", json!("old message"), "old message", 2);
    let mem = user_msg(MessageContent::Text("new message".to_string()));
    assert!(!user_message_matches(&row, &mem));
}

/// role mismatch (db row is assistant) → no match.
#[test]
fn user_message_matches_wrong_role() {
    let row = db_row("assistant", json!("hello"), "hello", 1);
    let mem = user_msg(MessageContent::Text("hello".to_string()));
    assert!(!user_message_matches(&row, &mem));
}

/// malformed DB content JSON → no match (safe default: persist).
#[test]
fn user_message_matches_malformed_db_content() {
    let row = db_row("user", json!(42), "", 4); // not a valid MessageContent
    let mem = user_msg(MessageContent::Text("hi".to_string()));
    assert!(!user_message_matches(&row, &mem));
}

// -----------------------------------------------------------------
// D-D entry-guard extension (08-07-group-chat-role-history-isolation
// P0-1/P0-3): `dd_guard_hit` unit tests. The two match paths +
// snapshot rule are pure functions over the loaded rows.
// -----------------------------------------------------------------

fn group_chat_state() -> Option<crate::tools::nominate_speaker::SharedTurnState> {
    Some(Arc::new(Mutex::new(
        crate::tools::nominate_speaker::GroupChatTurnState {
            next_speaker: None,
            discussion_ended: false,
        },
    )))
}

/// P0-1: 群聊作用域内,tail user 行 `speaker.is_some()`(role_history
/// 改写产物)→ 视同已落库,不触发 persist_turn(无 DB 重复行)。
#[test]
fn dd_guard_skips_persist_for_speaker_user_in_group_chat() {
    let loaded = vec![
        db_row("user", json!("hello"), "hello", 1),
        db_row("assistant", json!([{"type":"text","text":"x"}]), "", 2),
        db_row("user", json!("old"), "old", 3),
    ];
    let msg = ChatMessage {
        role: Role::User,
        content: MessageContent::Text("主持人发言".to_string()),
        speaker: Some("moderator".to_string()),
        attachments: None,
    };
    let hit = dd_guard_hit(false, group_chat_state().as_ref(), &loaded, &msg);
    assert!(
        hit.is_some(),
        "speaker-carrying tail must be judged already-persisted"
    );
    assert_eq!(hit.unwrap().seq, 3, "seq anchors on the tail-most user row");
}

/// 非群聊(group_chat_state None)+ 经典聊天(speaker None)→ 守卫行为
/// 不变(不命中);群聊 human prompt(speaker None)命中时 snapshot 仍是
/// 原内容(不因扩展而丢失 at_file 注入能力)。
#[test]
fn dd_guard_unchanged_for_classic_chat_speaker_none() {
    let loaded = vec![db_row("user", json!("hello"), "hello", 2)];
    let msg = user_msg(MessageContent::Text("hello".to_string()));
    // Classic chat: no group_chat_state → the guard never fires,
    // even on an exact content match.
    assert!(dd_guard_hit(false, None, &loaded, &msg).is_none());
    // Group-chat human prompt (speaker None) with a matching row:
    // the original path still hits (seq = the matched row) and the
    // snapshot keeps the content (at_file injection still works).
    let hit = dd_guard_hit(false, group_chat_state().as_ref(), &loaded, &msg);
    assert_eq!(hit.as_ref().map(|h| h.seq), Some(2));
    assert_eq!(
        hit.map(|h| h.snapshot),
        Some(Some(MessageContent::Text("hello".to_string()))),
        "speaker-None hit keeps the content snapshot (unchanged behavior)"
    );
}

/// P0-3: 改写行(speaker Some)命中守卫后 `last_user_snapshot` 返回
/// None → chat_loop.rs:1116 的 at_file 注入条件 (`injections` 非空 &&
/// snapshot Some)为 false,改写行不触发 `@file` 注入(无注入 manifest
/// 写入、无 FileInjections 事件)。
#[test]
fn dd_guard_rewrite_row_skips_at_file_injection() {
    let loaded = vec![db_row("user", json!("hello"), "hello", 1)];
    let msg = ChatMessage {
        role: Role::User,
        content: MessageContent::Text("M1 发言".to_string()),
        speaker: Some("M1".to_string()),
        attachments: None,
    };
    let hit = dd_guard_hit(false, group_chat_state().as_ref(), &loaded, &msg).unwrap();
    assert!(
        hit.snapshot.is_none(),
        "rewrite row must not trigger at_file injection"
    );
}

/// P0-3: 改写行的 seq 取**尾部最后一个 user 行**,而非 DB 第一条
/// user 行(find 不再因常数短路匹配错行)。
#[test]
fn dd_guard_rewrite_row_seq_not_first_user_row() {
    let loaded = vec![
        db_row("user", json!("hello"), "hello", 1),
        db_row("assistant", json!([{"type":"text","text":"x"}]), "", 2),
        db_row("user", json!("tool_result"), "", 7),
    ];
    let msg = ChatMessage {
        role: Role::User,
        content: MessageContent::Text("主持人发言".to_string()),
        speaker: Some("moderator".to_string()),
        attachments: None,
    };
    let hit = dd_guard_hit(false, group_chat_state().as_ref(), &loaded, &msg).unwrap();
    assert_eq!(
        hit.seq, 7,
        "seq must be the tail-most user row, NOT the first user row (1)"
    );
}
