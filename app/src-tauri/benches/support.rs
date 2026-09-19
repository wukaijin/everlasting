//! benches 共享支撑(seed_session + profile 解析)。非 bench target
//! (autobenches=false;harness.rs / db_bench.rs `mod support;` 引入)。
//!
//! 下述 allow(dead_code):本文件被 harness / db_bench 两个 bench 目标
//! 各自独立编译,单目标视角必然只消费一半函数/字段(text_share 供
//! F1 gen-fixtures.mjs 同读 profile),非真死码。

#![allow(dead_code)]

use std::sync::Arc;

use serde::Deserialize;
use sqlx::SqlitePool;

use everlasting_lib::bench_api::{
    persist_turn, ChatMessage, ContentBlock, MessageContent, Role, TestHarness,
};

/// 种子形态 profile(benches/profile.json,单一出处;F1 的
/// gen-fixtures.mjs 同读)。PR1 手写草稿,PR2 画像后只改数字。
#[derive(Debug, Clone, Deserialize)]
pub struct SeedProfile {
    pub text_share: f64,
    pub tool_pair_share: f64,
    pub thinking_share: f64,
    pub text_len_short: usize,
    pub text_len_long: usize,
    pub long_text_share: f64,
    pub tool_result_len: usize,
}

pub fn load_profile() -> SeedProfile {
    serde_json::from_str(include_str!("profile.json")).expect("parse benches/profile.json")
}

fn filler(len: usize, seed: u64) -> String {
    // 确定性伪语料:长度可控、内容不参与测量的助记填充。
    let mut s = String::with_capacity(len + 16);
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    while s.len() < len {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        s.push_str(&format!("{:016x} ", (x >> 33) % 1_000_000_007));
    }
    s.truncate(len);
    s
}

/// 按 profile 形态生成第 `i` 条种子消息(user/assistant 对话流:
/// text / tool_use+tool_result 对 / thinking 混合,长短参数化)。
/// seq 分配:0 = user 开场,之后 assistant/user(tool_result) 成对。
fn seed_message(i: i64, profile: &SeedProfile) -> ChatMessage {
    let (role, content) = if i == 0 {
        (Role::User, MessageContent::Text(filler(60, 1)))
    } else if i % 2 == 1 {
        // assistant 行:按 share 混合纯文本 / thinking / tool_use
        let r = ((i as u64 * 2654435761) % 10_000) as f64 / 10_000.0;
        let long = ((i as u64 * 40503) % 10_000) as f64 / 10_000.0 < profile.long_text_share;
        let len = if long {
            profile.text_len_long
        } else {
            profile.text_len_short
        };
        let content = if r < profile.thinking_share {
            MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: filler(400, i as u64 * 7 + 3),
                    signature: filler(80, i as u64 * 11 + 5),
                },
                ContentBlock::Text {
                    text: filler(len, i as u64 * 13 + 7),
                    cache_control: None,
                },
            ])
        } else if r < profile.thinking_share + profile.tool_pair_share {
            MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: filler(80, i as u64 * 17 + 9),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: format!("toolu_seed_{i}"),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "src/main.rs", "offset": 1, "limit": 200 }),
                },
            ])
        } else {
            MessageContent::Text(filler(len, i as u64 * 19 + 11))
        };
        (Role::Assistant, content)
    } else {
        // user 行:若前一 assistant 行是 tool_use 则本行是 tool_result
        let prev_r = (((i - 1) as u64 * 2654435761) % 10_000) as f64 / 10_000.0;
        let content = if prev_r >= profile.thinking_share
            && prev_r < profile.thinking_share + profile.tool_pair_share
        {
            MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: format!("toolu_seed_{}", i - 1),
                content: filler(profile.tool_result_len, i as u64 * 23 + 13),
                is_error: false,
                images: None,
                resolved: None,
            }])
        } else {
            MessageContent::Text(filler(profile.text_len_short, i as u64 * 29 + 15))
        };
        (Role::User, content)
    };
    ChatMessage {
        role,
        content,
        speaker: None,
        attachments: None,
    }
}

/// 往 harness 的 session 里预种 `n` 条消息(persist_turn 逐条,
/// 与生产写入同路径)。10k 档在 setup 段执行(测量外)。
pub async fn seed_session(harness: &TestHarness, profile: &SeedProfile, n: i64) {
    for i in 0..n {
        let m = seed_message(i, profile);
        persist_turn(
            &harness.db,
            &harness.session_id,
            m.role,
            &m.content,
            i,
            None,
            None,
        )
        .await
        .expect("seed persist_turn");
    }
}

/// db_bench 用:在给定 pool 的 session 里种 `n` 条(同 seed_message)。
pub async fn seed_pool_session(pool: &SqlitePool, session_id: &str, profile: &SeedProfile, n: i64) {
    for i in 0..n {
        let m = seed_message(i, profile);
        persist_turn(pool, session_id, m.role, &m.content, i, None, None)
            .await
            .expect("seed persist_turn");
    }
}

/// h4 工具回路脚本第一轮:list_dir ToolCall(只读 Tier,project 内放行)。
pub fn tool_call_script(harness: &TestHarness) -> Vec<everlasting_lib::bench_api::ChatEvent> {
    use everlasting_lib::bench_api::ChatEvent;
    let path = harness.project_path.to_string_lossy().into_owned();
    vec![
        ChatEvent::Start,
        ChatEvent::ToolCall {
            id: "toolu_bench_h4".into(),
            name: "list_dir".into(),
            input: serde_json::json!({ "path": path, "pattern": null }),
        },
        ChatEvent::Done {
            stop_reason: Some("tool_use".into()),
            usage: None,
        },
    ]
}

/// 纯文本一轮脚本(h1/h2/h3 主响应)。
pub fn text_script() -> Vec<everlasting_lib::bench_api::ChatEvent> {
    use everlasting_lib::bench_api::ChatEvent;
    vec![
        ChatEvent::Start,
        ChatEvent::Delta {
            text: "benchmark response body".into(),
        },
        ChatEvent::Done {
            stop_reason: Some("end_turn".into()),
            usage: None,
        },
    ]
}

/// Arc 包装便利(MockProvider 要 Arc<dyn Provider>)。
pub fn arc_provider(
    events: Vec<everlasting_lib::bench_api::ChatEvent>,
) -> Arc<everlasting_lib::bench_api::MockProvider> {
    Arc::new(everlasting_lib::bench_api::MockProvider::new(vec![
        everlasting_lib::bench_api::MockResponse::Events(events.into_iter().map(Ok).collect()),
    ]))
}
