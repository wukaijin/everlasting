//! 统一上下文预算估算(unified-context-budget WP1,2026-08-19)。
//!
//! 一把尺子量请求总占用:**按发送部件加法** —— system prompt + tools[]
//! + messages 三部件之和。C3+ 压缩触发线、摘要 postcheck、机械
//! `compact_messages` 的"当前占用"三处全部切换到该口径(修复旧口径
//! 只数 messages、漏计与 messages 并列发送的 tools/system 的洞——
//! 小窗口模型 32k/64k 下两者占比可观,请求可能在 messages 未达触发
//! 线时整体超窗)。
//!
//! # 核心不变量(prd D8,评审 F1)
//!
//! **messages 部件已含 memory 指令头对、skill listing、@文件注入正文、
//! 图片(无精估 pad)与全部历史 —— 绝不在这三部件之和上再单独加计任何
//! 切片。** 归因切片(memory_token / at_files_token / images_token /
//! system_token 中的 skill listing 部分)是从 messages **内部**归因的
//! 展示口径,与总量口径是两类账,永不互相加计:
//!
//! - 总量 = `count_tokens(system)` + `count_tokens(tools_json)` +
//!   `estimate_messages_tokens(messages)`(本模块唯一公式);
//! - 归因之和 ≤ 总量(残差 = 总量 − tools − memory − @files − images
//!   − system ≥ 0),由 AC1 单测锁定。
//!
//! WP2 的 budget gate(关卡⑤硬卡)消费同一把尺:超
//! [`BUDGET_LINE_RATIO`]×window 触发静默裁剪,裁尽仍超才 fail-fast。
//! 本模块 WP1 只提供度量;裁剪引擎是 WP2 范围。

use crate::llm::ChatMessage;
use crate::memory::tokens::count_tokens;

/// 预算线比例(WP2 关卡⑤硬卡触发线;对齐
/// `context::SUMMARY_POSTCHECK_RATIO` 的 0.95)。0.85 压缩触发线是第一道
/// 防线,硬卡只在压缩失败/静态切片挤压时动作,线贴窗留 5% 余量吸收
/// cl100k 本地估算与 provider 计量的系统性偏差(prd D4 / design §5)。
// WP2 budget gate 消费(PR3);PR1 先定义 + 单测锁值。
#[allow(dead_code)]
pub const BUDGET_LINE_RATIO: f64 = 0.95;

/// 与 messages 并列发送、但不在 messages 里的两个部件:system prompt
/// 本体 + 序列化 tools[] JSON。机械 `compact_messages` 只见 messages,
/// 统一口径经 `extra_tokens` 参数传入(调用侧 = 本函数)。
pub async fn estimate_request_overhead(system_prompt: &str, tools_json: &str) -> u32 {
    count_tokens(system_prompt).await + count_tokens(tools_json).await
}

/// 估算一次 LLM 请求的**统一总占用**(token,cl100k 本地估算):
///
/// ```text
/// estimate_request_tokens = count_tokens(system_prompt)     // 发送部件
///                         + count_tokens(tools_json)        // 发送部件
///                         + estimate_messages_tokens(messages)
/// ```
///
/// 第三项已含 memory 头对 + skill listing + @文件正文 + 图片 pad +
/// 历史 —— 见模块级不变量:**不要**在此之上加计任何切片(评审 F1 的
/// 重复计数教训)。
pub async fn estimate_request_tokens(
    system_prompt: &str,
    tools_json: &str,
    messages: &[ChatMessage],
) -> u32 {
    estimate_request_overhead(system_prompt, tools_json).await
        + crate::agent::context::estimate_messages_tokens(messages).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, MessageContent, Role};

    fn user(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.into()),
            speaker: None,
            attachments: None,
        }
    }

    /// AC1:统一总量 = 三部件之和(system + tools + messages),
    /// 逐部件独立计数后相加验证(不是恒等式复算 —— 三部件分别用
    /// 不同输入计数,断言加法结构)。
    #[tokio::test]
    async fn estimate_request_tokens_equals_three_parts_sum() {
        crate::memory::tokens::ensure_initialized().await;
        let system = "You are a coding agent. ".repeat(50);
        let tools_json = r#"[{"name":"read_file","description":"Read a file"}]"#.repeat(20);
        let messages = vec![
            user("hello, please review this patch carefully"),
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text("Sure, looking at it now.".to_string()),
                speaker: None,
                attachments: None,
            },
            user("focus on the error handling paths"),
        ];

        let sys_part = count_tokens(&system).await;
        let tools_part = count_tokens(&tools_json).await;
        let msgs_part = crate::agent::context::estimate_messages_tokens(&messages).await;
        assert!(sys_part > 0 && tools_part > 0 && msgs_part > 0);

        let total = estimate_request_tokens(&system, &tools_json, &messages).await;
        assert_eq!(total, sys_part + tools_part + msgs_part);
        // Empty send-side parts contribute 0 (worker stub path /
        // serialization failure best-effort → empty string).
        assert_eq!(
            estimate_request_tokens("", "", &messages).await,
            msgs_part,
            "empty system/tools must degrade to the messages part"
        );
    }

    /// AC1:归因切片来自 messages 内部,互不重叠、之和 ≤ 总量。构造
    /// 含 memory 合成头对 + skill listing 形态消息 + @文件注入文本 +
    /// 图片 pad 块的 messages,验证各切片 ≤ messages 部件、切片之和
    /// ≤ messages 部件(即 tools+system+切片之和 ≤ 总量,残差 ≥ 0)。
    #[tokio::test]
    async fn attribution_slices_inside_messages_do_not_exceed_total() {
        crate::memory::tokens::ensure_initialized().await;
        let system = "system prompt body";
        let tools_json = r#"[{"name":"shell"}]"#;

        // memory 合成头对形态(Blocks 指令块)+ skill listing 形态 +
        // @文件注入正文 + 历史 + 图片 pad 块 —— 全部物理在 messages 内。
        let memory_block = |t: &str| crate::llm::types::ContentBlock::Text {
            text: t.to_string(),
            cache_control: None,
        };
        let messages = vec![
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![memory_block(
                    "<memory-banner>CLAUDE.md instructions body ...",
                )]),
                speaker: None,
                attachments: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text("Understood.".to_string()),
                speaker: None,
                attachments: None,
            },
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![memory_block(
                    "<skills>available: budget-review, ui-review</skills>",
                )]),
                speaker: None,
                attachments: None,
            },
            user("look at this:\n<file path=\"big.rs\">\nfn main() {}\n</file>"),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    crate::llm::types::ContentBlock::Text {
                        text: "screenshot please".to_string(),
                        cache_control: None,
                    },
                    crate::llm::types::ContentBlock::ImageRef {
                        file: "att-1.png".to_string(),
                        media_type: "image/png".to_string(),
                    },
                ]),
                speaker: None,
                attachments: None,
            },
        ];

        // 归因切片计数(messages 内部归因口径,与 init.rs 各计数点同式)。
        let memory_slice = count_tokens("<memory-banner>CLAUDE.md instructions body ...").await;
        let skill_slice =
            count_tokens("<skills>available: budget-review, ui-review</skills>").await;
        let at_files_slice = count_tokens("<file path=\"big.rs\">\nfn main() {}\n</file>").await;

        let total = estimate_request_tokens(system, tools_json, &messages).await;
        let msgs_part = crate::agent::context::estimate_messages_tokens(&messages).await;

        // 各切片 ≤ messages 部件;归因之和 ≤ messages 部件 → 加上
        // tools+system 两部件后仍 ≤ 总量(残差 = 总量 − 五切片 ≥ 0)。
        assert!(memory_slice <= msgs_part);
        assert!(skill_slice <= msgs_part);
        assert!(at_files_slice <= msgs_part);
        assert!(memory_slice + skill_slice + at_files_slice <= msgs_part);
        let sys_tok = count_tokens(system).await;
        let tools_tok = count_tokens(tools_json).await;
        assert!(
            sys_tok + tools_tok + memory_slice + skill_slice + at_files_slice <= total,
            "five slices + send parts must not exceed the unified total"
        );
    }

    /// AC2(机械路径半边):messages 部件未达 0.85 触发线、但
    /// tools+system 挤窗使统一总量过线 → `compact_messages` 触发;
    /// extra=0(旧口径)同输入不触发。集成半边见
    /// `tests_agent_loop/budget.rs::tools_and_system_squeeze_triggers_mechanical_compaction`。
    #[tokio::test]
    async fn tools_and_system_overhead_crosses_trigger_when_messages_under() {
        crate::memory::tokens::ensure_initialized().await;
        // 窗口 10_000:trigger = 8_500,target = 5_000。messages 自校准
        // 到 [6_200, 8_000)(< 8_500 旧口径不触发;overhead = 2_500 补计
        // 后总 ∈ [8_700, 10_500) ≥ 8_500)。压缩后 target 5k − 2.5k =
        // 2.5k,保护头尾 < 1k,中段可丢 → 落线内,无 StillOver。
        // cl100k 对重复文本的压缩比不稳定(实测 ≈4.3 chars/token 漂移),
        // 固定条数的算术夹具会脆 —— 与集成测试同款自校准循环。
        let mut messages = vec![user("memory head"), assistant_text("ack")];
        let mut pairs = 0usize;
        loop {
            messages.push(user(
                "the quick brown fox jumps over the lazy dog. ".repeat(40),
            ));
            messages.push(assistant_text("sure thing. ".repeat(150)));
            pairs += 1;
            let est = crate::agent::context::estimate_messages_tokens(&messages).await;
            if est >= 6_200 {
                break;
            }
            assert!(pairs < 40, "self-calibration runaway: {} pairs", pairs);
        }
        messages.push(user("current question"));

        let msgs_part = crate::agent::context::estimate_messages_tokens(&messages).await;
        let trigger = crate::agent::context::trigger_threshold(10_000);
        assert!(
            (msgs_part as u64) < trigger as u64 && msgs_part >= 6_200,
            "fixture premise: messages part under the trigger line and over \
             the calibration floor (got {})",
            msgs_part
        );
        let overhead = 2_500u32;
        assert!(
            (msgs_part + overhead) as u64 >= trigger as u64,
            "fixture premise: unified total over the line"
        );

        // 旧口径(messages-only):不触发,原样返回。
        let old = crate::agent::context::compact_messages(messages.clone(), 10_000, 0).await;
        assert_eq!(old.dropped_count, 0, "messages-only 口径不触发");

        // 统一口径:触发,丢组后回落。
        let new = crate::agent::context::compact_messages(messages, 10_000, overhead).await;
        assert!(new.dropped_count > 0, "统一口径必须触发压缩");
        assert_eq!(
            new.degradation,
            crate::agent::context::DegradationKind::None
        );
        // tokens_before/after 均为统一口径(含 overhead)。
        assert!((new.tokens_before as u64) >= trigger as u64);
        assert!((new.tokens_after as u64) < crate::agent::context::target_threshold(10_000) as u64);
    }

    fn assistant_text(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(text.into()),
            speaker: None,
            attachments: None,
        }
    }

    /// WP2 常量先定义本 PR(消费点在 WP2 的 budget gate)。
    #[test]
    fn budget_line_ratio_matches_postcheck_ratio() {
        assert!((BUDGET_LINE_RATIO - 0.95).abs() < f64::EPSILON);
    }
}
