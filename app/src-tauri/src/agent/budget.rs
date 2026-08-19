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
// WP2 — 关卡⑤硬卡(prd R6-R9,design §3)
// ---------------------------------------------------------------------------

use crate::agent::at_file::AtFileSpan;
use crate::llm::types::{ContentBlock, MessageContent, Role};

/// One trim arm's outcome(kind ∈ `at_file` | `image` | `memory_section`)。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrimArm {
    pub kind: &'static str,
    pub count: usize,
    pub tokens_freed: u32,
}

/// enforce_budget 的结果报告。无裁剪的常态请求 `arms` 为空。
#[derive(Debug, Clone)]
pub struct BudgetTrimReport {
    pub arms: Vec<TrimArm>,
    /// 裁剪前统一总量(pre;audit payload 专用,trace 不落 —— prd D9)
    pub pre_total: u32,
    /// 裁剪后统一总量(trace 的实发口径来源)
    pub post_total: u32,
    pub window: u32,
    pub line: u32,
    /// 臂尽仍超线(fail-fast 信号,prd R9)
    pub still_over: bool,
    /// trace 实发值调整量:at_files / images 切片各减 freed;
    /// memory 切片在臂 3 触发时改记目录态值。
    pub at_files_freed: u32,
    pub images_freed: u32,
    pub memory_effective: Option<u32>,
}

/// 预算线(0.95 × window,对齐 `SUMMARY_POSTCHECK_RATIO`)。
pub fn budget_line(window: u32) -> u32 {
    ((window as f64) * BUDGET_LINE_RATIO) as u32
}

/// 关卡⑤硬卡:send 前对请求副本做统一总量检查,超 `budget_line` 时按
/// D3 优先级静默裁剪(旧轮次 @文件 → 旧轮次图片 → memory 已加载节回退
/// 目录态),逐臂重估、达标即停;臂尽仍超线由 caller fail-fast。
///
/// **非破坏性不变量(prd D6)**:只改本请求的 `messages` 副本与请求局部
/// 视图 —— DB / StubRegistry / MemoryDigestRegistry / spans 本体均不动,
/// 下轮重算。当前 turn 的注入(`msg_idx >= current_user_msg_idx`)不裁
/// (是要处理的活);span 失配 fail-open 跳过(prd R7.1)。
#[allow(clippy::too_many_arguments)]
pub async fn enforce_budget(
    system_prompt: &str,
    tools_json: &str,
    messages: &mut [ChatMessage],
    spans: &[AtFileSpan],
    current_user_msg_idx: usize,
    // 目录态指令块(臂 3 的回退目标)。`None` = 无 digest 机制(臂 3
    // 不可用);`Some(空)` 同样视为不可用。
    memory_catalog_blocks: Option<&[ContentBlock]>,
    window: u32,
) -> BudgetTrimReport {
    let line = budget_line(window);
    let overhead = estimate_request_overhead(system_prompt, tools_json).await;
    let mut total = overhead + crate::agent::context::estimate_messages_tokens(messages).await;
    let mut report = BudgetTrimReport {
        arms: Vec::new(),
        pre_total: total,
        post_total: total,
        window,
        line,
        still_over: false,
        at_files_freed: 0,
        images_freed: 0,
        memory_effective: None,
    };
    if total <= line {
        return report;
    }

    // ---- 臂 1:旧轮次 @文件正文 → 占位行(同消息内多 span 按 start
    // 降序应用,保前序偏移有效;评审 F5)。 ----
    let mut trimmable: Vec<&AtFileSpan> = spans
        .iter()
        .filter(|s| s.msg_idx < current_user_msg_idx)
        .collect();
    trimmable.sort_by_key(|s| (s.msg_idx, s.start));
    let mut at_count = 0usize;
    let mut at_freed = 0u32;
    for span in trimmable.iter().rev() {
        if apply_span_placeholder(messages, span) {
            at_count += 1;
            at_freed += span.tokens;
        }
    }
    if at_count > 0 {
        report.arms.push(TrimArm {
            kind: "at_file",
            count: at_count,
            tokens_freed: at_freed,
        });
        report.at_files_freed = at_freed;
        total = overhead + crate::agent::context::estimate_messages_tokens(messages).await;
        if total <= line {
            report.post_total = total;
            return report;
        }
    }

    // ---- 臂 2:旧轮次图片 → B1 占位降级文案先例(模型知有图未发,
    // 防幻觉;`attachments.rs` resolve 失败降级同款话术形态)。 ----
    let images_before = crate::attachments::estimate_images_token(messages);
    if images_before > 0 {
        let mut img_count = 0usize;
        for (idx, msg) in messages.iter_mut().enumerate() {
            if idx >= current_user_msg_idx || msg.role != Role::User {
                continue;
            }
            if let MessageContent::Blocks(blocks) = &mut msg.content {
                for b in blocks.iter_mut() {
                    if matches!(b, ContentBlock::Image { .. }) {
                        *b = ContentBlock::Text {
                            text: "[image: 历史图片 — 预算裁剪，未发送]".to_string(),
                            cache_control: None,
                        };
                        img_count += 1;
                    }
                }
            }
        }
        let images_after = crate::attachments::estimate_images_token(messages);
        let img_freed = images_before.saturating_sub(images_after);
        if img_count > 0 && img_freed > 0 {
            report.arms.push(TrimArm {
                kind: "image",
                count: img_count,
                tokens_freed: img_freed,
            });
            report.images_freed = img_freed;
            total = overhead + crate::agent::context::estimate_messages_tokens(messages).await;
            if total <= line {
                report.post_total = total;
                return report;
            }
        }
    }

    // ---- 臂 3:memory 已加载节回退目录态(请求副本;registry 不动,
    // 窗口持续紧则每轮等效回退 —— prd R7.3/D6)。 ----
    if let Some(catalog) = memory_catalog_blocks {
        if !catalog.is_empty() {
            if let Some(head) = messages.first_mut() {
                if head.role == Role::User {
                    let before_tok = blocks_text_tokens(&head.content).await;
                    head.content = MessageContent::Blocks(catalog.to_vec());
                    let after_tok = blocks_text_tokens(&head.content).await;
                    let mem_freed = before_tok.saturating_sub(after_tok);
                    if mem_freed > 0 {
                        report.arms.push(TrimArm {
                            kind: "memory_section",
                            count: 1,
                            tokens_freed: mem_freed,
                        });
                        report.memory_effective = Some(after_tok);
                        total = overhead
                            + crate::agent::context::estimate_messages_tokens(messages).await;
                    }
                }
            }
        }
    }

    report.post_total = total;
    report.still_over = total > line;
    report
}

/// 在请求副本上把一个 span 的注入正文替换为占位行。fail-open:span
/// 解析不到(消息没了 / 角色漂移 / 文本变短 / 非字符边界)→ 不动,
/// 返回 false(prd R7.1)。
fn apply_span_placeholder(messages: &mut [ChatMessage], span: &AtFileSpan) -> bool {
    let placeholder = format!(
        "[at-file {}: 约 {} tokens，预算裁剪省略]",
        span.path, span.tokens
    );
    let Some(msg) = messages.get_mut(span.msg_idx) else {
        return false;
    };
    if msg.role != Role::User {
        return false;
    }
    let mut replaced = false;
    match &mut msg.content {
        MessageContent::Text(t) => {
            replaced = replace_range(t, span, &placeholder);
        }
        // @图注入致 Text→Blocks 形态:偏移定在首个 Text 块内
        //(span_text 同款寻址)。
        MessageContent::Blocks(blocks) => {
            for b in blocks.iter_mut() {
                if let ContentBlock::Text { text, .. } = b {
                    replaced = replace_range(text, span, &placeholder);
                    break;
                }
            }
        }
    }
    replaced
}

fn replace_range(text: &mut String, span: &AtFileSpan, placeholder: &str) -> bool {
    // `str::get` 拒绝越界 / 非边界区间 → fail-open。
    if text.get(span.start..span.end).is_none() {
        return false;
    }
    text.replace_range(span.start..span.end, placeholder);
    true
}

async fn blocks_text_tokens(content: &MessageContent) -> u32 {
    let texts: Vec<&str> = match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        MessageContent::Text(t) => vec![t.as_str()],
    };
    count_tokens(&texts.join("\n")).await
}

/// 裁尽仍超线时 Error turn 的 breakdown 文案(prd R9:错误信息含各
/// 切片)。占位值由 caller 从 drive 侧各计数点填入。
#[allow(clippy::too_many_arguments)]
pub fn format_over_budget_message(
    post_total: u32,
    window: u32,
    tools_token: u32,
    memory_token: Option<u32>,
    system_token: u32,
    at_files_token: u32,
    images_token: u32,
) -> String {
    let mem = memory_token
        .map(|t| t.to_string())
        .unwrap_or_else(|| "n/a".into());
    let history = post_total.saturating_sub(
        tools_token + system_token + at_files_token + images_token + memory_token.unwrap_or(0),
    );
    format!(
        "context_over_budget: 统一估算 {post_total} tokens 超预算线(0.95 × {window})。\
         切片 breakdown:tools ≈{tools_token} · memory ≈{mem} · system ≈{system_token} · \
         @文件 ≈{at_files_token} · 图片 ≈{images_token} · 历史+杂项 ≈{history}。\
         裁剪臂已用尽 —— 请缩小本轮输入(减少 @文件/图片)或手动 /compact 后重试。"
    )
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
        assert_eq!(budget_line(20_000), 19_000);
    }

    // -----------------------------------------------------------------
    // WP2 enforce_budget(关卡⑤硬卡,prd AC3/AC5 单测半边)
    // -----------------------------------------------------------------

    fn span(msg_idx: usize, start: usize, end: usize, path: &str, tokens: u32) -> AtFileSpan {
        AtFileSpan {
            msg_idx,
            start,
            end,
            path: path.to_string(),
            tokens,
        }
    }

    fn user_blocks_text_image(prefix: &str, img_label: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: prefix.to_string(),
                    cache_control: None,
                },
                ContentBlock::Image {
                    source: crate::llm::types::ImageSource {
                        source_type: "base64".to_string(),
                        media_type: "image/png".to_string(),
                        data: img_label.to_string(),
                    },
                },
            ]),
            speaker: None,
            attachments: None,
        }
    }

    /// 常态:未超线 → 零裁剪、消息原样(AC6 半边:gate 关/未超线
    /// 行为与现状一致)。
    #[tokio::test]
    async fn enforce_budget_noop_below_line() {
        crate::memory::tokens::ensure_initialized().await;
        let mut messages = vec![user("hello"), assistant_text("hi"), user("question")];
        let before = messages.clone();
        let total = crate::agent::context::estimate_messages_tokens(&messages).await;
        let report = enforce_budget("", "", &mut messages, &[], 2, None, total * 10).await;
        assert!(report.arms.is_empty());
        assert!(!report.still_over);
        assert_eq!(messages, before, "未超线不得动请求副本");
    }

    /// AC3 臂 1:旧轮次 @文件 span → 占位行;当前 turn 的 span 保护
    /// 不裁;臂 1 修足后早停(无 image/memory 臂)。
    #[tokio::test]
    async fn arm1_trims_old_at_files_and_protects_current() {
        crate::memory::tokens::ensure_initialized().await;
        let old_body = "OLD_FILE_BODY_CONTENT ".repeat(80);
        let cur_body = "CUR_FILE_BODY_CONTENT ".repeat(80);
        let old_text = format!("see {} end", old_body);
        let cur_text = format!("cur {} end", cur_body);
        let old_start = 4;
        let cur_start = 4;
        let mut messages = vec![
            assistant_text("ack"),
            user(old_text.clone()),
            user(cur_text.clone()),
        ];
        let total = crate::agent::context::estimate_messages_tokens(&messages).await;
        let spans = vec![
            span(1, old_start, old_start + old_body.len(), "old.rs", 400),
            span(2, cur_start, cur_start + cur_body.len(), "cur.rs", 400),
        ];
        // window = 总量 → line = 0.95×total < total → 超线。
        let report = enforce_budget("", "", &mut messages, &spans, 2, None, total).await;

        assert_eq!(report.arms.len(), 1, "只应有臂 1(早停)");
        assert_eq!(report.arms[0].kind, "at_file");
        assert_eq!(report.arms[0].count, 1);
        assert_eq!(report.at_files_freed, 400);
        assert!(!report.still_over, "臂 1 修足后必须落线内");
        // 旧 span 被占位替换,当前 span 逐字保留。
        let old_out = messages[1].content.to_text();
        assert!(old_out.contains("预算裁剪省略"), "old: {}", old_out);
        assert!(!old_out.contains("OLD_FILE_BODY"));
        assert!(messages[2].content.to_text().contains("CUR_FILE_BODY"));
    }

    /// AC3:span 失配(end 超出文本)fail-open 跳过,不 panic、不动
    /// 该消息(臂继续往后走,本例直接 still_over)。
    #[tokio::test]
    async fn arm1_stale_span_fails_open() {
        crate::memory::tokens::ensure_initialized().await;
        let body = "stale span fixture body padding ".repeat(10);
        let mut messages = vec![assistant_text("ack"), user(body.clone())];
        let spans = vec![span(1, 0, 9_999, "gone.txt", 500)];
        // 窗口远小于总量(线 = 0.95×20 = 19 << ~60 tok)→ 超线且无臂可裁。
        let report = enforce_budget("", "", &mut messages, &spans, 1, None, 20).await;
        assert!(report.arms.is_empty(), "失配 span 不得记臂");
        assert!(report.still_over, "无臂可裁 → 仍超线");
        assert_eq!(messages[1].content.to_text(), body);
    }

    /// AC3 臂 2:旧轮次 Image 块降级为占位文案;当前 turn 的 Image
    /// 保护。Image pad ≈1600 tok/张,一张旧图就够修一个小超线。
    #[tokio::test]
    async fn arm2_degrades_old_images_keeps_current() {
        crate::memory::tokens::ensure_initialized().await;
        let mut messages = vec![
            assistant_text("ack"),
            user_blocks_text_image("old shot:", "b2x"),
            user_blocks_text_image("cur shot:", "b3x"),
        ];
        let total = crate::agent::context::estimate_messages_tokens(&messages).await;
        // 超线一点点(线 = 0.95×total),一张旧图 ≈1600 tok 足以修足。
        let report = enforce_budget("", "", &mut messages, &[], 2, None, total).await;

        assert_eq!(report.arms.len(), 1);
        assert_eq!(report.arms[0].kind, "image");
        assert_eq!(report.arms[0].count, 1);
        assert!(
            report.images_freed > 1_000,
            "Image pad ≈1600,freed={}",
            report.images_freed
        );
        // 旧图 → 占位文案;当前图逐字保留。
        let old_blocks = match &messages[1].content {
            MessageContent::Blocks(b) => b.clone(),
            _ => panic!("old must stay Blocks"),
        };
        let placeholder_text = match &old_blocks[1] {
            ContentBlock::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        assert!(placeholder_text.contains("预算裁剪"));
        assert!(matches!(
            &messages[2].content,
            MessageContent::Blocks(b) if matches!(b[1], ContentBlock::Image { .. })
        ));
    }

    /// AC3 臂 3:memory 头(指令 User 消息)回退目录态;registry 语义
    /// 不在本层(请求副本)。memory_effective = 目录态 token 值。
    #[tokio::test]
    async fn arm3_retracts_memory_head_to_catalog() {
        crate::memory::tokens::ensure_initialized().await;
        let big_head = "M".repeat(4_000);
        let mut messages = vec![
            user(big_head.clone()),
            assistant_text("Understood."),
            user("question"),
        ];
        let total = crate::agent::context::estimate_messages_tokens(&messages).await;
        let catalog = vec![ContentBlock::Text {
            text: "<memory-catalog>AGENTS.md: 1 section</memory-catalog>".to_string(),
            cache_control: None,
        }];
        let report = enforce_budget(
            "",
            "",
            &mut messages,
            &[],
            2,
            Some(&catalog),
            total, // line = 0.95×total < total → 超线;臂 3 一把修足
        )
        .await;

        assert_eq!(report.arms.len(), 1);
        assert_eq!(report.arms[0].kind, "memory_section");
        assert!(report.memory_effective.is_some());
        assert!(!report.still_over);
        assert!(messages[0].content.to_text().contains("memory-catalog"));
    }

    /// AC5:臂尽仍超线 → still_over + breakdown 文案含各切片。
    #[tokio::test]
    async fn arms_exhausted_reports_still_over_with_breakdown() {
        crate::memory::tokens::ensure_initialized().await;
        let mut messages = vec![user(
            "hello world with some padding to pass the tiny line. ".repeat(5),
        )];
        let report = enforce_budget("", "", &mut messages, &[], 0, None, 20).await;
        assert!(report.arms.is_empty());
        assert!(report.still_over);
        assert_eq!(report.post_total, report.pre_total);

        let msg = format_over_budget_message(9_000, 8_000, 3_900, Some(2_000), 1_500, 400, 600);
        assert!(msg.contains("context_over_budget"));
        assert!(msg.contains("tools ≈3900"));
        assert!(msg.contains("memory ≈2000"));
        assert!(msg.contains("@文件 ≈400"));
        assert!(msg.contains("图片 ≈600"));
        let history = 9_000 - 3_900 - 2_000 - 1_500 - 400 - 600;
        assert!(msg.contains(&format!("历史+杂项 ≈{history}")));
    }
}
