//! C3 摘要式上下文压缩 — PR1 水位替换地基(纯机械,零 LLM)。
//!
//! 08-18-llm-context-compaction PR1。摘要生成(LLM 调用、保留区计算、
//! 降级链、熔断)属 PR2;本模块只实现**请求路径的水位替换**:
//! 前端每请求傻发全量 wire 历史(wire 层 `ChatMessage` 没有 metadata
//! 字段,前端无法告知哪行是摘要),后端以 DB 行
//! (`loaded_session.messages`,SoT)倒序找最新
//! `metadata.kind == "compaction_summary"` 行 S,把 wire 历史中
//! **`seq <= S.metadata.cutoff_seq` 的行**(被压区)折叠为单条摘要
//! 消息,`seq > cutoff` 的常规行逐字保留(修订 2026-08-18:折叠点
//! 按 `cutoff_seq` 精确值,非摘要行自身位置 —— PR2 check P1 发现
//! 按位置折叠会丢保留区与本请求提问)—— 同一 session 第二次请求
//! 不再重付摘要(AC2),被压缩区 DB 原始行不删(D2 搜索/审计无损,
//! AC5)。
//!
//! **对齐前提(load-bearing,评审 P1-3)**:wire 与 db_rows 的 1:1
//! 行序对齐依赖前端 `reloadAfterFinalize`(streamEvents.ts,每请求
//! done 后 load_session 重灌 store)保证下一次 wire 含摘要行。前端
//! rehydrate 管线对 text-only user 行回发的是 **`text` 列原文**
//! (`streamRehydrate.ts` 的 `content: m.text` → `toPayloadContent`
//! 返回纯字符串),所以内容比对锚定 `text` 列而非 content JSON ——
//! DB 侧摘要行 content 通常是 `Blocks([Text])`(insert_system_event
//! 先例),wire 侧则是 `MessageContent::Text`,严格 `PartialEq` 会
//! 假阴性,`to_text()` 归一化两边才相等。由此推出 PR2
//! `insert_compaction_summary` 的列契约:**纯摘要必须同值写进
//! `content` 与 `text` 两列** —— wire 从 `text` 列回发(对齐锚点),
//! 折叠消息从 `content` 列重建,两列分叉会让 in-context 摘要与
//! 对齐/前端展示所用文本漂移。注意 insert_system_event 先例本身两列
//! 并不同值(content 带 "[worktree event] " 前缀、text 列不带)——
//! 摘要行别照抄那个分叉。
//!
//! **陈旧 wire 容忍**:历史 orphan tool_use 的前端 splice 修复
//! (`streamRehydrate.ts` 2013 修复)会向 store 插入合成 user 行,
//! 单条位移由 idx±1 内容重对齐吸收;多处位移/内容彻底对不上 →
//! 返回 `Miss` fail-open(回到 main 行为),由调用方 warn
//! (`watermark_miss`,可观测的非哑失败)。
//!
//! **D3 自愈(design §2.2)**:`edit_user_message` cascade 删掉摘要行
//! 后,倒序"找现存最新"自然回退次新水位或全量历史,零专门代码
//! (`clear_session_messages` 同理,全删即回全量)。
//!
//! # PR2 增量(08-18-llm-context-compaction,摘要生成 + 降级链 + 熔断)
//!
//! - [`compute_preservation_region`]:保留区计算(design §4.1)。复用
//!   `context::group_droppable_turns` 的组语义反方向(从最后一组向前
//!   累积到预算),组边界保证 RULE-A-001 配对原子性在摘要路径同样
//!   成立;最后一条 typed user 所在组强制并入(Cline `findCutIndex`
//!   同款护栏)。
//! - [`build_compaction_prompt`]:摘要 prompt 组装(design §6 模板 +
//!   transcript 渲染截断 + prior-summary 注入)。anchor 消息不进
//!   transcript(不重复喂,评审 P1-2)。
//! - [`CompactionRegistry`]:熔断计数(session → 连续失败次数)。
//!   进程级 `OnceLock` 单例——同 `memory::digest::registry()` 先例
//!   (08-15):`run_chat_loop` 的 24+ 参签名是硬约束不许动,AppState
//!   句柄穿不进去,故走全局单例 + `delete_session_inner` 清理(同
//!   digest 的接线点)。
//!
//! # PR2.5 增量(修订 2026-08-18,保留区跨请求存活修复)
//!
//! - [`compressible_cutoff_seq`]:cutoff_seq 精确计算(design §4.3
//!   修订)。PR2 的"摘要行 seq-1"上界实际是当前输入行的 seq,按它
//!   折叠会在下一请求吞掉保留区与本请求提问(check P1);本函数给出
//!   待压区末行的真实 seq,`SummaryAnchor.cutoff` 随锚点穿参,同
//!   loop 二次压缩(增量合并)场景的行序对齐同样精确。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::db::MessageRow;
use crate::llm::{ChatMessage, ContentBlock, MessageContent, Role};

/// `messages.metadata` 的 `kind` 值:摘要行(design §2.1)。行本身是
/// 普通 `role='user'` 消息行,`content` = 纯摘要正文(前缀话术由 PR2
/// 在 in-context 构建时拼接、不落库 —— 评审 P1-2)。
pub const COMPACTION_SUMMARY_KIND: &str = "compaction_summary";

/// PR2 预留:当前水位摘要锚点。init 时水位替换命中则种子为水位摘要
/// (`LoopInit.summary_anchor`),drive_turn 每次成功压缩后更新为新摘要
/// (`DriveTurnOutcome` 循环内穿参,同 `loop_hit_count` 线程模式 ——
/// 评审 P1-1 修正:不用"摘要落在位置 2"的位置猜测,三种 init 布局
/// 下位置会漂移)。PR3 的增量合并以 `<prior-summary>` 块注入
/// `content`(纯摘要正文,无前缀话术),`seq` 记入
/// `prior_summary_seq`。
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryAnchor {
    /// 水位摘要行的 `messages.seq`(审计 + prior_summary_seq 落库)。
    pub seq: i64,
    /// 纯摘要正文(无回填前缀话术;前缀在 in-context 构建时拼接)。
    pub content: String,
    /// 该摘要行的 `cutoff_seq`(被压缩区最后一行 seq,**修订
    /// 2026-08-18 新增**)。双重身份:水位折叠点(design §2.2)+
    /// [`compressible_cutoff_seq`] 的行序对齐基准(design §4.3 ——
    /// 见该函数文档的对齐论证)。
    pub cutoff: i64,
}

/// 水位未命中/未生效的原因。区分二者让调用方只在对齐失败时 warn
/// (无水位是正常路径,不值得日志噪音)。
#[derive(Debug, Clone, PartialEq)]
pub enum MissReason {
    /// db_rows 里没有 `kind=compaction_summary` 行 —— 正常路径
    /// (从未压缩过 / D3 全删自愈后),原样返回。
    NoWatermark,
    /// 水位行存在但折叠无法执行:cutoff_seq 缺失/非法(旧格式或异常
    /// 行)、`seq == cutoff` 的行不在 db_rows、或 wire 历史在折叠边界
    /// 对不上(陈旧 store 缺多行 / reload 被绕过,±1 重对齐也救不
    /// 回)。fail-open 回全量历史;调用方记 `watermark_miss`。
    AlignmentFailed { summary_seq: i64 },
}

/// [`apply_compaction_watermark`] 的结果。
#[derive(Debug, PartialEq)]
pub enum WatermarkResult {
    /// 水位命中,`messages` = `[摘要 ChatMessage] + [wire 中 seq >
    /// cutoff 的常规行]`(修订 2026-08-18:折叠点 = `cutoff_seq`,
    /// 非摘要行位置 —— 保留区与本请求提问在 wire 尾部,seq > cutoff,
    /// 逐字存活)。注意 PR1 直接用 DB 行 content,**不加**回填前缀
    /// 话术(PR2 才处理,评审 P1-2)。
    Applied {
        messages: Vec<ChatMessage>,
        anchor: SummaryAnchor,
    },
    /// 未生效,`messages` 原样返回;`reason` 告知调用方是否值得 warn。
    Miss {
        messages: Vec<ChatMessage>,
        reason: MissReason,
    },
}

/// 读 `MessageRow.metadata` 的 `kind` 字段。容错口径:
/// - metadata 为 `None`(普通聊天行;TEXT 列非法 JSON 在
///   `db::load_session` 解析时已被 `.ok()` 吸收为 `None`,
///   见 session_crud.rs 的 metadata 解析)→ 无 kind;
/// - metadata 不是 JSON object(标量/数组)→ 无 kind;
/// - `kind` 不是 string → 无 kind。
pub fn message_metadata_kind(metadata: Option<&serde_json::Value>) -> Option<&str> {
    metadata?.as_object()?.get("kind")?.as_str()
}

/// db 行序对齐下 wire[idx] 是否就是 `row` 这一行。比对 = role +
/// `to_text()`(见模块文档:rehydrate 管线对 text-only 行回发 `text`
/// 列原文,`to_text()` 归一化 `Text` 与 `Blocks([Text])` 两种形态)。
fn wire_matches_row(wire: &ChatMessage, row: &MessageRow) -> bool {
    wire.role == row_role(row) && wire.content.to_text() == row.text
}

/// `MessageRow.role`("user"/"assistant" 字符串)→ `Role`。非
/// "assistant" 一律按 user 处理(与 `group_chat_loop.rs
/// reload_messages` 的宽容口径一致)。
fn row_role(row: &MessageRow) -> Role {
    match row.role.as_str() {
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}

/// `MessageRow` → `ChatMessage`(与 `group_chat_loop.rs
/// reload_messages` 同款转换):content JSON 反序列化失败时回退
/// `text` 列。摘要行不带 attachments manifest,`attachments`
/// 固定 `None`。
fn row_to_chat_message(row: &MessageRow) -> ChatMessage {
    let content = serde_json::from_value::<MessageContent>(row.content.clone())
        .unwrap_or_else(|_| MessageContent::Text(row.text.clone()));
    ChatMessage {
        role: row_role(row),
        content,
        speaker: row.speaker.clone(),
        attachments: None,
    }
}

/// 水位替换(design §3 算法,**修订 2026-08-18:折叠点按
/// `cutoff_seq`,不再按摘要行位置**,纯函数、零 LLM):
///
/// 1. db_rows(seq ASC)倒序找首个 `kind=compaction_summary` 行 → S
///    (最新水位;无 → `Miss::NoWatermark` 原样返回);
/// 2. `cutoff = S.metadata.cutoff_seq`(缺字段/非整数 —— 旧格式或
///    异常行 → `Miss::AlignmentFailed` fail-open);
/// 3. `idx` = db_rows 中 `seq == cutoff` 的行下标(被压区末行;
///    找不到 → 同上 fail-open);
/// 4. 对齐防御:`wire[idx]` 内容与该行不符 → 尝试 idx±1 内容匹配
///    (容忍前端 store 单行位移:陈旧缺行 / orphan-repair splice);
///    仍不符 → `Miss::AlignmentFailed`(调用方 warn 后 fail-open);
/// 5. 命中 → `[S 转成的 ChatMessage] + wire[cut+1..]`,其中映射到
///    `kind=compaction_summary` DB 行的 wire 行被剔除(kind 过滤)。
///
/// **为什么按 cutoff 而不是摘要行位置折叠(design §2.2,PR2 check
/// P1)**:摘要行按 seq 游标插在全量行(含保留区 + 当前输入)之后,
/// 按位置折叠会把保留区与本请求用户提问一并丢弃 —— 恰是设计最想
/// 逐字保留的东西,而摘要 transcript 从未覆盖它们。按 cutoff 折叠
/// 后天然正确:保留区/当前输入/后续 assistant 轮的 seq 都 > cutoff
/// → 跨请求存活;旧摘要行 seq < 新 cutoff(被增量合并吸收)→ 出局;
/// 旧摘要行 seq 落在 cutoff 之上时(插入游标在保留区之后的必然
/// 副产物)→ 由第 5 步的 kind 过滤兜住,多次压缩的中间产物不会
/// 重复出席。
///
/// 前缀话术不在此加(design §6:DB 行 content = 纯摘要,前缀只在
/// in-context 构建时拼接,PR2 接线)。
pub fn apply_compaction_watermark(
    wire_messages: Vec<ChatMessage>,
    db_rows: &[MessageRow],
) -> WatermarkResult {
    // 1. 倒序(最新优先)找现存最新水位行。D3 cascade 删掉最新摘要
    //    行后这里自然回退到次新水位 —— 自愈零专门代码。
    let Some((_, summary_row)) =
        db_rows.iter().enumerate().rev().find(|(_, r)| {
            message_metadata_kind(r.metadata.as_ref()) == Some(COMPACTION_SUMMARY_KIND)
        })
    else {
        return WatermarkResult::Miss {
            messages: wire_messages,
            reason: MissReason::NoWatermark,
        };
    };

    // 2. cutoff_seq 是 load-bearing 折叠点(design §2.1/§2.2)。缺
    //    字段(旧格式/异常行)没有安全默认值 —— seq-1 上界正是
    //    PR2 check P1 的错误语义(会吞掉保留区),宁可 fail-open。
    let miss_aligned = |wire: Vec<ChatMessage>| WatermarkResult::Miss {
        messages: wire,
        reason: MissReason::AlignmentFailed {
            summary_seq: summary_row.seq,
        },
    };
    let Some(cutoff) = summary_row
        .metadata
        .as_ref()
        .and_then(|m| m.get("cutoff_seq"))
        .and_then(|v| v.as_i64())
    else {
        return miss_aligned(wire_messages);
    };

    // 3. 被压区末行(折叠边界锚定的 DB 行)。
    let Some(row_idx) = db_rows.iter().position(|r| r.seq == cutoff) else {
        return miss_aligned(wire_messages);
    };

    // 4. 边界对齐 + ±1 防御(比对对象 = cutoff 行本身,语义对齐新
    //    折叠边界)。matches_at 越界(get 返回 None)按不匹配处理,
    //    覆盖 wire 比 db_rows 短的陈旧场景。
    let matches_at = |i: usize| {
        wire_messages
            .get(i)
            .is_some_and(|w| wire_matches_row(w, &db_rows[row_idx]))
    };
    let cut = if matches_at(row_idx) {
        row_idx
    } else if row_idx > 0 && matches_at(row_idx - 1) {
        row_idx - 1
    } else if matches_at(row_idx + 1) {
        row_idx + 1
    } else {
        return miss_aligned(wire_messages);
    };

    // 5. 折叠:wire[0..=cut] → 单条摘要消息(DB 行 content,SoT);
    //    保留 wire[cut+1..],但映射到 summary-kind DB 行的 wire 行
    //    剔除(wire 层无 metadata,经 ±1 对齐建立的位移映射回查
    //    DB 行的 kind;映射越出 db_rows 的尾部 = 本轮新输入,保留)。
    //    保留区 + 本轮新输入的 seq 都 > cutoff → 逐字存活(design §2.2)。
    let summary_msg = row_to_chat_message(summary_row);
    let anchor = SummaryAnchor {
        seq: summary_row.seq,
        content: summary_msg.content.to_text(),
        cutoff,
    };
    let shift = cut as isize - row_idx as isize; // wire[i] ↔ db_rows[i - shift]
    let mut out = Vec::with_capacity(1 + wire_messages.len() - (cut + 1));
    out.push(summary_msg);
    for (i, w) in wire_messages.into_iter().enumerate().skip(cut + 1) {
        let db_idx = (i as isize - shift) as usize;
        let is_summary_row = db_rows.get(db_idx).is_some_and(|r| {
            message_metadata_kind(r.metadata.as_ref()) == Some(COMPACTION_SUMMARY_KIND)
        });
        if !is_summary_row {
            out.push(w);
        }
    }
    WatermarkResult::Applied {
        messages: out,
        anchor,
    }
}

// ---------------------------------------------------------------------------
// PR2.5(修订 2026-08-18):cutoff_seq 精确计算(design §4.3)
// ---------------------------------------------------------------------------

/// 待压区末行的 DB seq —— 摘要行 metadata `cutoff_seq` 的唯一取值源
/// (design §4.3 修订:**精确值,不是"摘要行 seq-1"上界**。seq-1 是
/// 当前输入行的 seq,会让下一请求的折叠点吞掉保留区与本请求提问 ——
/// PR2 check P1 正是此错)。
///
/// **对齐论证(design §4.3)**:压缩时刻的内存列表自合成头之后为
/// `[S?] + 常规行 R`(S? = 一次折叠留下的摘要消息,至多一条且必在
/// 首位 —— 水位替换与循环内压缩的回填都是 `[合成头] + [摘要] +
/// [保留区] + [当前输入]`,后续 turn 只在尾部追加;摘要后接机械
/// 兜底丢组的路径会把 anchor 置 None,首位即常规行)。常规行 R 与
/// DB 行的对应:
///
/// - 无折叠(`prior = None`,本次请求内从未折叠过):init 已持久化
///   当前输入,wire 尾与 DB 行 1:1(评审 P1-3 前提),即
///   `R = db_rows 全部(kind 无关)`,待压区末行 =
///   `db_rows[cut - synthetic_prefix_len - 1]`(design §4.3 原式);
/// - 有折叠(`prior = Some`,水位命中或同 loop 上一轮压缩):
///   折叠产物只保留 `seq > prior.cutoff` 的常规行,故
///   `R = [db_rows 中 seq > prior.cutoff 且 kind ≠ summary 的行] ++
///   [本请求新 persist 的行(当前输入 + 后续 turn)]`。待压区必在
///   当前输入之前结束(保留区护栏保证),故待压区常规行全部落在
///   db_rows 的上述过滤后缀里 —— 取过滤后缀的第 `regular` 个
///   (1-based)即待压区末行。kind 过滤 load-bearing:被吸收的旧
///   摘要行 seq 可以 > 新 cutoff(摘要行插在插入游标 = 保留区之后),
///   不过滤会数错行。
///
/// 防御(fail-open 到机械兜底,调用方 warn,绝不 panic):
/// - 推算下标越出 `db_rows`(wire 与 DB 行序失配 —— 陈旧 store /
///   水位对齐 Miss 后的退化路径 / 摘要后机械兜底打乱行序)。
///
/// **退化边界(regular == 0,常见)**:待压区只剩上一份摘要消息本身
/// —— 同 loop 二次压缩时,上一轮的保留区(≈15k)+ 新 turn 很容易
/// 仍然吃满预算,折叠边界自然贴到摘要消息之后。此时新摘要的覆盖面
/// = 上一份摘要的传递覆盖面,cutoff = `prior.cutoff`(精确,非 Err:
/// 下一请求折叠 [seq > cutoff 且 kind≠summary] 与"没有新压缩"时
/// 完全一致,旧摘要行由 kind 过滤出局)。
///
/// 返回 `Err(reason)` 时调用方按摘要失败处理(`Failed` → 机械兜底)。
pub fn compressible_cutoff_seq(
    synthetic_prefix_len: usize,
    cut: usize,
    prior: Option<&SummaryAnchor>,
    db_rows: &[MessageRow],
) -> Result<i64, &'static str> {
    // 有摘要 ⟺ prior.is_some()(见函数文档的推导;
    // 机械兜底丢组路径已把 anchor 置 None,首位回到常规行)。
    let has_summary = usize::from(prior.is_some());
    let Some(regular) = cut
        .checked_sub(synthetic_prefix_len)
        .and_then(|v| v.checked_sub(has_summary))
    else {
        return Err("compressible region is empty (cut <= synthetic prefix)");
    };
    if regular == 0 {
        // 退化边界:见函数文档 —— 传递覆盖面,cutoff 沿用 prior 的。
        let anchor = prior.expect("regular == 0 only reachable with a prior summary");
        return Ok(anchor.cutoff);
    }

    // 待压区末行 = 常规行 run 的第 `regular` 个(1-based)。
    let candidate = match prior {
        // 无折叠:wire 尾与 DB 行 1:1 → design §4.3 原式
        // db_rows[cut - P - 1](regular == cut - P)。
        None => db_rows.get(regular - 1),
        // 有折叠:只数 seq > prior.cutoff 的常规行(kind 过滤)。
        Some(anchor) => db_rows
            .iter()
            .filter(|r| {
                r.seq > anchor.cutoff
                    && message_metadata_kind(r.metadata.as_ref()) != Some(COMPACTION_SUMMARY_KIND)
            })
            .nth(regular - 1),
    };
    match candidate {
        Some(row) => Ok(row.seq),
        None => Err("cutoff row index out of loaded rows (wire/db row misalignment)"),
    }
}

// ---------------------------------------------------------------------------
// PR2:保留区计算(design §4.1)
// ---------------------------------------------------------------------------

/// 保留区预算下限(design §4.1 / prd Q4:`clamp(15_000, 窗口×10%, 25_000)`)。
/// 任何窗口下至少逐字保留 15k token 的近期消息 —— 过小的保留区会让
/// "近期上下文"语义失效(模型丢失刚发生的事)。
const PRESERVATION_MIN_TOKENS: u32 = 15_000;
/// 保留区预算上限。超大窗口(> 250k)下保留区也不超过 25k —— 保留区
/// 越大,摘要摊销收益越低。
const PRESERVATION_MAX_TOKENS: u32 = 25_000;
/// 保留区预算 = 窗口 × 10%(再 clamp 到上下限)。
const PRESERVATION_WINDOW_RATIO: f64 = 0.10;

/// 保留区 token 预算:`clamp(15_000, window × 0.10, 25_000)`。
pub fn preservation_budget(context_window: u32) -> u32 {
    (((context_window as f64) * PRESERVATION_WINDOW_RATIO) as u32)
        .clamp(PRESERVATION_MIN_TOKENS, PRESERVATION_MAX_TOKENS)
}

/// 计算保留区边界(design §4.1,纯函数 + cl100k 估算):
///
/// 1. 组边界:`messages[synthetic_prefix_len .. len-1]` 按
///    `context::group_droppable_turns` 切分成原子组(配对组/单例组)。
///    组语义与机械丢组同源 —— 保留区起点落在组边界上,压缩区与保留区
///    之间永远不会拆开 `assistant(tool_use)` / `user(tool_result)` 配对
///    (RULE-A-001)。
/// 2. **从最后一组向前**累积 `estimate_messages_tokens`,直到 ≥
///    `preservation_budget(context_window)`(方向与 `compact_messages`
///    的"从最旧丢起"互补 —— 保留区是"最新的 N token")。最后一组
///    无条件计入(即使单组就超预算)。
/// 3. 护栏:最后一条 typed user(纯文本、无 tool_result)消息所在组
///    若未被覆盖,强制并入保留区(Cline `findCutIndex` 同款)。
/// 4. 返回 cut(保留区首组起点)。**空待压区(cut ==
///    synthetic_prefix_len)是调用方的信号:直走机械路径**(窗口过小 /
///    历史太短,摘要无利可图)。
///
/// 中段被 `group_droppable_turns` 隐式保护的消息(孤儿 tool_use)若
/// 落在 cut 之后会自动逐字保留(它们不属于任何组,不影响累积)。
pub async fn compute_preservation_region(
    messages: &[ChatMessage],
    synthetic_prefix_len: usize,
    context_window: u32,
) -> usize {
    // 尾部当前输入(len-1)是保护区的一部分但不在组切分范围内
    // (group_droppable_turns 的 tail_index 语义,同 compact_messages)。
    if messages.len() <= synthetic_prefix_len + 1 {
        return synthetic_prefix_len;
    }
    let tail_index = messages.len() - 1;
    let groups =
        crate::agent::context::group_droppable_turns(messages, synthetic_prefix_len, tail_index);
    if groups.is_empty() {
        // 全部中段被隐式保护 → 没有可压区,直走机械(NoCandidates)。
        return synthetic_prefix_len;
    }

    let budget = preservation_budget(context_window);
    let mut acc: u32 = 0;
    let mut cut = groups[groups.len() - 1].0;
    for (start, end) in groups.iter().rev() {
        let group_tokens =
            crate::agent::context::estimate_messages_tokens(&messages[*start..*end]).await;
        acc = acc.saturating_add(group_tokens);
        cut = *start;
        if acc >= budget {
            break;
        }
    }

    // 护栏:最后一条 typed user 所在组必入保留区。当前输入(tail)本身
    // 已在保留区;这里覆盖的是"tail 是 tool_result"的多轮工具循环 ——
    // 用户最后一次真实输入可能在保留区预算之外。
    if let Some(tu) = last_typed_user_index(messages, synthetic_prefix_len, tail_index) {
        if let Some((group_start, _)) = groups.iter().find(|(s, e)| *s <= tu && tu < *e) {
            if *group_start < cut {
                cut = *group_start;
            }
        }
    }

    cut
}

/// 最后一条 typed user 消息下标:role=user、无 tool_result/tool_use 块、
/// 带文本内容(纯 Text 或 Blocks 含 Text)。搜索范围 [start, tail_index)
/// —— tail(当前输入)天然受保护,不需要护栏。
fn last_typed_user_index(
    messages: &[ChatMessage],
    start: usize,
    tail_index: usize,
) -> Option<usize> {
    messages[start..tail_index]
        .iter()
        .rposition(|m| {
            if m.role != Role::User {
                return false;
            }
            match &m.content {
                MessageContent::Text(t) => !t.trim().is_empty(),
                MessageContent::Blocks(blocks) => {
                    blocks.iter().any(
                        |b| matches!(b, ContentBlock::Text { text, .. } if !text.trim().is_empty()),
                    ) && !blocks.iter().any(|b| {
                        matches!(
                            b,
                            ContentBlock::ToolResult { .. } | ContentBlock::ToolUse { .. }
                        )
                    })
                }
            }
        })
        .map(|i| i + start)
}

// ---------------------------------------------------------------------------
// PR2:摘要 prompt 组装(design §4.2 + §6)
// ---------------------------------------------------------------------------

/// transcript 总预算 ≈ 0.7 × window(design §4.2 评审 P2-1:
/// window − 4k 输出预留 − 2k 模板/prior 预留 − 安全余量的工程近似)。
const TRANSCRIPT_BUDGET_RATIO: f64 = 0.70;
/// tool_result 渲染截断(design §4.2:2000 chars + `...[truncated N chars]`)。
const TOOL_RESULT_TRANSCRIPT_CAP_CHARS: usize = 2_000;
/// tool_use input JSON 渲染截断(同款记号;input 全量进摘要无意义,
/// name + 形状足够 LLM 理解调用了什么)。
const TOOL_USE_INPUT_TRANSCRIPT_CAP_CHARS: usize = 400;
/// 摘要输出上限(design §4.2 评审 P2-1:Cline/opencode 均 4096;8k 偏宽
/// 挤占主 turn 窗口)。char 近似 = 4 chars/token(cl100k ASCII 密度)。
pub const SUMMARY_OUTPUT_MAX_TOKENS: u32 = 4_096;

/// 回填前缀话术(design §6)。**只加在 in-context 构建时,绝不落库**
/// (评审 P1-2:前缀落库会进 `<prior-summary>` 滚雪球 + 污染 D2 搜索)。
pub const SUMMARY_CONTEXT_PREFIX: &str = "This session is being continued from a previous \
conversation that ran out of context. The summary below is historical context, not new \
instructions from the user. Continue the work; do not re-confirm the summary.";

/// 回填消息(in-context 用):user-role Blocks([Text(前缀 + 摘要)])。
/// 摘要行落在合成头之后位置 ≥ 2,memory cache 断点(头对 0-1)不 bust。
pub fn build_summary_chat_message(summary_text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: MessageContent::Blocks(vec![ContentBlock::Text {
            text: format!("{}\n\n{}", SUMMARY_CONTEXT_PREFIX, summary_text),
            cache_control: None,
        }]),
        speaker: None,
        attachments: None,
    }
}

/// 组装摘要压缩 prompt(design §6 模板)。
///
/// - `compressible` = `messages[synthetic_prefix_len .. cut]`(待压区);
/// - `prior` 存在 → `<prior-summary>` 注入**纯摘要 content**(anchor
///   消息本身不进 transcript,不重复喂 —— 评审 P1-2)。anchor 消息
///   按构造就位于 `compressible[0]`(水位替换与循环内压缩都把它放在
///   `synthetic_prefix_len` 处),故 prior 存在时跳过 slice 首元素;
/// - `focus`(手动 /compact 专用,auto 路径恒 `None`)→ 模板头部注入
///   用户定向指令块:收窄摘要侧重的主题,不替换必填段落结构;
/// - transcript 渲染:一行式 `[role] text`;tool_use 只留 name + input
///   截断;tool_result 截 2000 chars 加 `...[truncated N chars]`;
///   thinking/redacted 不渲染;图片渲染 `[image attached: <file>]`;
/// - transcript 总预算 ≈ 0.7 × window,溢出从最旧条目丢起 + 在头部加
///   `[older transcript omitted]` 记号(保留最近对话引语 —— "Primary
///   Request" 的逐字锚点优先保最近的)。
pub async fn build_compaction_prompt(
    compressible: &[ChatMessage],
    prior: Option<&SummaryAnchor>,
    context_window: u32,
    focus: Option<&str>,
) -> String {
    // prior anchor 消息 = compressible[0](按构造,见 fn doc);跳过它,
    // 它的内容经 <prior-summary> 块进入 prompt,transcript 不重复渲染。
    let render_slice = if prior.is_some() && !compressible.is_empty() {
        &compressible[1..]
    } else {
        compressible
    };
    let initial_len = render_slice.len();
    let mut lines: Vec<String> = render_slice.iter().map(render_transcript_line).collect();

    // transcript 预算溢出:从最旧条目丢起。测量-估算-再测量的有界循环
    // (≤ 3 轮,每轮 1 次 cl100k 编码):按 chars/token 密度比例估算要
    // 丢弃的字符量,丢完再实测,不达标再补丢。
    let budget_tokens = ((context_window as f64) * TRANSCRIPT_BUDGET_RATIO) as u32;
    for _ in 0..3 {
        let total_chars: usize = lines.iter().map(|l| l.len()).sum();
        let total_tokens = crate::memory::tokens::count_tokens(&lines.join("\n")).await;
        if total_tokens <= budget_tokens || lines.len() <= 1 {
            break;
        }
        let chars_per_tok = total_chars as f64 / total_tokens.max(1) as f64;
        let shed_chars = ((total_tokens - budget_tokens) as f64 * chars_per_tok * 1.10) as usize;
        let mut shed = 0usize;
        while lines.len() > 1 && shed < shed_chars {
            shed += lines.remove(0).len();
        }
    }
    if lines.len() < initial_len {
        lines.insert(0, "[older transcript omitted]".to_string());
    }
    let transcript = lines.join("\n");

    let mut prompt = String::from(
        "You are performing a CONTEXT CHECKPOINT COMPACTION for an AI coding agent.\n\
         Another language model (possibly yourself) started to solve this problem and\n\
         will resume from your summary. Produce a handoff summary, not a response to\n\
         the user.\n\n",
    );
    if let Some(anchor) = prior {
        prompt.push_str(&format!(
            "<prior-summary>\n{}\n</prior-summary>\n\
             The prior summary above may be stale. Where it conflicts with the\n\
             conversation transcript below, THE CONVERSATION WINS. Items completed in\n\
             the transcript move to \"Completed\"; items invalidated are dropped.\n\n",
            anchor.content
        ));
    }
    if let Some(focus) = focus {
        prompt.push_str(&format!(
            "FOCUS INSTRUCTIONS FROM THE USER: {focus}\n\
             The user manually requested this compaction with the focus above.\n\
             Prioritize details related to it across the sections below; the focus\n\
             narrows emphasis, it does not replace or drop any required section.\n\n"
        ));
    }
    prompt.push_str(&format!(
        "Summarize the conversation transcript into these sections:\n\
         1. Primary Request and Intent — the user's goals. CRITICAL: list ALL user\n\
         messages verbatim or near-verbatim; user feedback defines success.\n\
         2. Key Technical Concepts and Decisions\n\
         3. Files and Code Sections — every file path touched or read MUST appear\n\
         (paths are load-bearing: the agent re-reads files on demand)\n\
         4. Errors and Fixes — keep failures visible; do not repeat solved mistakes\n\
         5. Work State — Completed / In progress / Blocked (align with any checklist)\n\
         6. Optional Next Step — quote the most recent conversation directly\n\n\
         Output ONLY the summary. Be concise and structured.\n\n\
         <transcript>\n{}\n</transcript>\n",
        transcript
    ));
    prompt
}

/// 一行式 transcript 渲染(一行 = 一条消息)。thinking 块不渲染
/// (签名/正文对摘要无意义);图片退出 context(design §7 B1 行)。
fn render_transcript_line(m: &ChatMessage) -> String {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let mut parts: Vec<String> = Vec::new();
    match &m.content {
        MessageContent::Text(t) => {
            if !t.trim().is_empty() {
                parts.push(t.clone());
            }
        }
        MessageContent::Blocks(blocks) => {
            for b in blocks {
                match b {
                    ContentBlock::Text { text, .. } => {
                        if !text.trim().is_empty() {
                            parts.push(text.clone());
                        }
                    }
                    ContentBlock::ToolUse { name, input, .. } => parts.push(format!(
                        "[tool_use {} {}]",
                        name,
                        truncate_chars(&input.to_string(), TOOL_USE_INPUT_TRANSCRIPT_CAP_CHARS)
                    )),
                    ContentBlock::ToolResult { content, .. } => parts.push(format!(
                        "[tool_result {}]",
                        truncate_chars(content, TOOL_RESULT_TRANSCRIPT_CAP_CHARS)
                    )),
                    ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
                    ContentBlock::ImageRef { file, .. } => {
                        parts.push(format!("[image attached: {}]", file))
                    }
                    ContentBlock::Image { .. } => parts.push("[image attached]".to_string()),
                }
            }
        }
    }
    format!("[{}] {}", role, parts.join(" ").replace('\n', " "))
}

/// char 边界截断 + `...[truncated N chars]` 记号(N = 被丢弃的 char 数)。
fn truncate_chars(s: &str, cap: usize) -> String {
    let total = s.chars().count();
    if total <= cap {
        return s.to_string();
    }
    let kept: String = s.chars().take(cap).collect();
    format!("{}...[truncated {} chars]", kept, total - cap)
}

/// 摘要输出剥壳后的长度兜底:超过 4k token 估算时按 char 近似截断
/// (cl100k ≈ 4 chars/token;截在 char 边界避免 UTF-8 panic)。正常
/// 摘要远低于此,这只是防御模型超长输出挤占主 turn 窗口。
pub fn clamp_summary_output(text: String) -> String {
    let max_chars = (SUMMARY_OUTPUT_MAX_TOKENS as usize) * 4;
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// PR2:熔断 registry(design §4.4)
// ---------------------------------------------------------------------------

/// 连续失败熔断阈值:同 session 连续 3 次摘要失败 → 后续请求跳过摘要
/// 直达机械丢组(prd R5;粘性 —— 直到一次成功或 session 删除)。
pub const COMPACTION_BREAKER_THRESHOLD: u8 = 3;

/// Session → 连续摘要失败次数。进程级 `OnceLock` 单例 —— 选型理由:
/// `run_chat_loop` 的 24+ 参签名是本任务硬约束不许动,AppState 句柄
/// 无法穿入 loop(与 `memory::digest::registry()` 08-15 先例同款处境,
/// 同款解法);`delete_session_inner` 挂清理(commands/sessions.rs,
/// 同 digest/stub 的接线点),daemon 重启自然清空。
#[derive(Default)]
pub struct CompactionRegistry {
    inner: tokio::sync::RwLock<HashMap<String, u8>>,
}

static COMPACTION_REGISTRY: OnceLock<CompactionRegistry> = OnceLock::new();

/// 进程级单例访问点。
pub fn compaction_registry() -> &'static CompactionRegistry {
    COMPACTION_REGISTRY.get_or_init(CompactionRegistry::default)
}

impl CompactionRegistry {
    /// session 当前连续失败次数。
    pub async fn failures(&self, session_id: &str) -> u8 {
        self.inner
            .read()
            .await
            .get(session_id)
            .copied()
            .unwrap_or(0)
    }

    /// 熔断是否触发(连续失败 ≥ 3)。gate 信号 —— 触发后本 session
    /// 后续请求跳过摘要直达机械。
    pub async fn is_tripped(&self, session_id: &str) -> bool {
        self.failures(session_id).await >= COMPACTION_BREAKER_THRESHOLD
    }

    /// 失败计数 +1(saturating;不重置 —— 连续性是信号)。
    pub async fn record_failure(&self, session_id: &str) {
        let mut guard = self.inner.write().await;
        let entry = guard.entry(session_id.to_string()).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// 成功清零(prd R5:一次成功即恢复摘要路径)。
    pub async fn record_success(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }

    /// 清理(`delete_session_inner` 接线点 —— session_id 复用不得拿到
    /// 残留的熔断状态)。
    pub async fn clear(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// 手动 /compact 入口(08-18-manual-compact-command)
// ---------------------------------------------------------------------------

/// 从 DB 行集倒序找最新水位摘要行,推导 [`SummaryAnchor`]。
///
/// 与 `apply_compaction_watermark` 的第 1-2 步同源,但服务于空闲期
/// (无 wire 可折叠):手动压缩用它的产出做增量合并的 prior 输入。
/// `cutoff_seq` 缺失/非整数(旧格式/异常行)→ `None` —— 与水位替换
/// 的 fail-open 同语义:退回"无水位",重新全量摘要,不 panic。
pub fn latest_summary_anchor(rows: &[MessageRow]) -> Option<SummaryAnchor> {
    let row = rows
        .iter()
        .rev()
        .find(|r| message_metadata_kind(r.metadata.as_ref()) == Some(COMPACTION_SUMMARY_KIND))?;
    let cutoff = row.metadata.as_ref()?.get("cutoff_seq")?.as_i64()?;
    Some(SummaryAnchor {
        seq: row.seq,
        // text 列 = 纯摘要正文(insert 契约两列同值,前缀不落库)
        content: row.text.clone(),
        cutoff,
    })
}

/// 摘要旁路 completion 的失败形态(共用 helper 的错误面;auto 路径
/// 需要区分"用户取消"以避免计入熔断,手动路径无取消源、统一按失败)。
#[derive(Debug)]
pub(crate) enum SummaryStreamError {
    Cancelled,
    Failed(&'static str),
}

/// 摘要旁路 completion(auto drive 路径与手动 /compact 入口共用):
/// `retry_open` 包裹、无 tools、单条 user prompt;剥壳只收 assistant
/// text(Delta)+ `Done` 的 usage;`Ok(ChatEvent::Error)` 与 `Err` 都算
/// 失败(RULE-A-011 同源 —— 漏接 Ok(Error) 会把半截文本当完整摘要)。
/// 输出**未**做 4k 截断,调用方自行 `clamp_summary_output`。
pub(crate) async fn send_summary_completion(
    provider: &dyn crate::llm::Provider,
    token: &tokio_util::sync::CancellationToken,
    prompt: String,
) -> Result<(String, Option<crate::llm::types::TokenUsage>), SummaryStreamError> {
    use crate::llm::retry::{retry_open, OpenOutcome, RetryPolicy, RetrySink};
    use crate::llm::types::ChatEvent;
    use futures_util::StreamExt;

    /// retry sink:静默(no-op)。摘要调用是旁路 —— retrying 通知挂到
    /// in-flight 的主 assistant 占位气泡会让用户看到"还没开始输出就在
    /// 重试"的困惑;可观测性由失败 warn + 熔断 registry 承担。
    struct SummaryRetrySink;
    impl RetrySink for SummaryRetrySink {
        fn emit_retrying(&self, _event: crate::llm::retry::RetryingEvent) {}
    }

    let request = ChatMessage {
        role: Role::User,
        content: MessageContent::Text(prompt),
        speaker: None,
        attachments: None,
    };
    let mut rng = fastrand::Rng::new();
    let sink = SummaryRetrySink;
    let outcome = retry_open(
        provider,
        None,
        vec![request],
        vec![],
        &RetryPolicy::default(),
        token,
        &sink,
        &mut rng,
    )
    .await;
    let mut stream = match outcome {
        OpenOutcome::Stream(s) => s,
        OpenOutcome::Cancelled => return Err(SummaryStreamError::Cancelled),
    };

    let mut text = String::new();
    let mut usage: Option<crate::llm::types::TokenUsage> = None;
    let mut errored = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(ChatEvent::Delta { text: t }) => text.push_str(&t),
            Ok(ChatEvent::Done { usage: u, .. }) => usage = u,
            Ok(ChatEvent::Error { .. }) => {
                errored = true;
                break;
            }
            Ok(_) => {}
            Err(_) => {
                errored = true;
                break;
            }
        }
    }
    if errored {
        return Err(SummaryStreamError::Failed("summary stream errored"));
    }
    if text.trim().is_empty() {
        return Err(SummaryStreamError::Failed("summary output empty"));
    }
    Ok((text, usage))
}

/// 手动压缩结果载荷(`compact_session` 命令响应;serde snake_case,
/// TS 镜像类型 `ManualCompactionResult`)。
#[derive(Debug, serde::Serialize)]
pub struct ManualCompactionOutcome {
    pub cutoff_seq: i64,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub summary_usage: Option<crate::llm::types::TokenUsage>,
    /// provider 协议族(Debug 名,与 auto 路径 metadata 同口径)。
    pub model: String,
}

/// 手动压缩失败类别(命令层映射为用户可读错误)。
#[derive(Debug)]
pub enum ManualCompactionError {
    /// 待压区为空:水位之后的常规历史全部落在保留区(或会话几乎为
    /// 空)。没付 LLM 调用,不计熔断。
    NothingToCompress,
    /// 摘要 completion 失败(流错误/空输出)。已计熔断。
    SummaryFailed(&'static str),
    /// 摘要行落库失败。已计熔断。
    PersistFailed,
}

/// 手动 /compact:空闲期(turn 边界外)对 DB 现存历史执行一次摘要
/// 压缩(prd R1-R4)。
///
/// 与 auto 路径(drive.rs `attempt_summary_compaction`)共享全部纯函数
/// (prompt 组装 / 保留区 / 落库 / 熔断),差异在编排上下文:
///
/// - 输入 = `load_session` 的 DB 行(无 loop 内存态;空闲期 wire↔DB 1:1
///   由前端 reloadAfterFinalize 保证,无对齐风险);
/// - 水位 prior 从最新摘要行现推([`latest_summary_anchor`]),等价
///   init 水位命中时的 anchor 种子 → 已有水位走增量合并(R3);
/// - 无 0.85 触发线(用户主动收窗,低 context 同样执行,R1),但空
///   待压区拒绝(NothingToCompress);
/// - **seq = MAX(seq)+1:仅在本函数被 in-flight guard 保护的前提下
///   安全**(无活跃 loop 才无并发 persist;active loop 内必须吃 loop
///   游标 —— 见 pattern-llm-compaction 的 seq 契约。命令层
///   `compact_session_inner` 查 `session_active_request` 后才调用);
/// - 熔断(D6):不查 `is_tripped`(手动不受熔断限制),失败照记
///   `record_failure` / 成功 `record_success`(与 auto 共享信号,
///   成功顺带解熔断)。
///
/// caller(`compact_session_inner`)负责 scope/config gate(群聊拒绝、
/// `llm_compaction_enabled` 开关)与 in-flight guard。
pub async fn run_manual_compaction(
    db: &sqlx::SqlitePool,
    session_id: &str,
    provider: std::sync::Arc<dyn crate::llm::Provider>,
    context_window: u32,
    focus: Option<&str>,
    rows: &[MessageRow],
) -> Result<ManualCompactionOutcome, ManualCompactionError> {
    let prior = latest_summary_anchor(rows);

    // candidate = 水位之后的常规行(kind 过滤:被吸收的旧摘要行 seq
    // 可能 > 新水位 cutoff,不过滤会数错行 —— compressible_cutoff_seq
    // 同款口径)。与 auto 路径的等价物:折叠产物 [S?] + 常规行。
    let filtered: Vec<&MessageRow> = rows
        .iter()
        .filter(|r| {
            message_metadata_kind(r.metadata.as_ref()) != Some(COMPACTION_SUMMARY_KIND)
                && prior.as_ref().is_none_or(|a| r.seq > a.cutoff)
        })
        .collect();
    let candidates: Vec<ChatMessage> = filtered.iter().map(|r| row_to_chat_message(r)).collect();

    // 保留区(DB 行不含合成头,synthetic 偏移 = 0;尾部天然受保护)。
    let cut = compute_preservation_region(&candidates, 0, context_window).await;
    if cut == 0 || cut >= candidates.len() {
        return Err(ManualCompactionError::NothingToCompress);
    }
    // 待压区末行精确 seq(契约:禁"摘要行 seq-1"近似)。
    let cutoff_seq = filtered[cut - 1].seq;

    // build_compaction_prompt 的 prior 语义假设 anchor 消息位于
    // compressible[0](auto 路径构造)并跳过之;手动路径没有该内存
    // 消息,补一条占位(内容经 <prior-summary> 块进场,占位被跳过)。
    let anchor_msg = |a: &SummaryAnchor| ChatMessage {
        role: Role::User,
        content: MessageContent::Text(a.content.clone()),
        speaker: None,
        attachments: None,
    };
    let mut compressible: Vec<ChatMessage> = Vec::with_capacity(cut + 1);
    if let Some(anchor) = prior.as_ref() {
        compressible.push(anchor_msg(anchor));
    }
    compressible.extend_from_slice(&candidates[..cut]);

    // tokens_before = 当前水位视图([旧摘要?] + 全部 candidate)——
    // 下一请求不压缩时的 context 量级(观测口径,非精确账)。
    let mut view_before: Vec<ChatMessage> = Vec::with_capacity(candidates.len() + 1);
    if let Some(anchor) = prior.as_ref() {
        view_before.push(anchor_msg(anchor));
    }
    view_before.extend(candidates.iter().cloned());
    let tokens_before = crate::agent::context::estimate_messages_tokens(&view_before).await;

    let prompt =
        build_compaction_prompt(&compressible, prior.as_ref(), context_window, focus).await;

    // 手动入口无取消源(fresh token 永不触发);Cancelled 分支是
    // retry_open 语义完备性的防御位,按失败处理。
    let token = tokio_util::sync::CancellationToken::new();
    let (raw_text, usage) = match send_summary_completion(provider.as_ref(), &token, prompt).await {
        Ok(v) => v,
        Err(SummaryStreamError::Cancelled) => {
            compaction_registry().record_failure(session_id).await;
            return Err(ManualCompactionError::SummaryFailed("summary cancelled"));
        }
        Err(SummaryStreamError::Failed(reason)) => {
            compaction_registry().record_failure(session_id).await;
            tracing::warn!(
                session_id = %session_id,
                reason,
                "manual compaction: summary completion failed"
            );
            return Err(ManualCompactionError::SummaryFailed(reason));
        }
    };
    let summary_text = clamp_summary_output(raw_text);

    // tokens_after = [新摘要] + 保留区。
    let mut view_after: Vec<ChatMessage> = vec![build_summary_chat_message(&summary_text)];
    view_after.extend_from_slice(&candidates[cut..]);
    let tokens_after = crate::agent::context::estimate_messages_tokens(&view_after).await;

    // metadata 契约同 auto(design §2.1)+ trigger/focus 手动增量。
    // serde default 兼容旧回看行(缺 focus 字段)。
    let model = format!("{:?}", provider.protocol());
    let next_seq = rows.iter().map(|r| r.seq).max().unwrap_or(0) + 1;
    let metadata = serde_json::json!({
        "kind": COMPACTION_SUMMARY_KIND,
        "cutoff_seq": cutoff_seq,
        "preserve_from_seq": cutoff_seq + 1,
        "tokens_before": tokens_before,
        "tokens_after": tokens_after,
        "trigger": "manual",
        "focus": focus,
        "model": model,
        "prior_summary_seq": prior.as_ref().map(|a| a.seq),
        "summary_usage": usage,
    });
    if let Err(e) = crate::db::sessions::insert_compaction_summary(
        db,
        session_id,
        &summary_text,
        next_seq,
        &metadata,
    )
    .await
    {
        compaction_registry().record_failure(session_id).await;
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            next_seq,
            "manual compaction: insert_compaction_summary failed"
        );
        return Err(ManualCompactionError::PersistFailed);
    }
    compaction_registry().record_success(session_id).await;
    tracing::info!(
        session_id = %session_id,
        cutoff_seq,
        tokens_before,
        tokens_after,
        "manual compaction applied"
    );
    Ok(ManualCompactionOutcome {
        cutoff_seq,
        tokens_before,
        tokens_after,
        summary_usage: usage,
        model,
    })
}

// ---------------------------------------------------------------------------
// Tests(风格对齐 agent/context.rs:同文件内嵌 tests mod)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a user-text wire message.
    fn user(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.into()),
            speaker: None,
            attachments: None,
        }
    }

    /// Helper: build an assistant-text wire message.
    fn assistant(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(text.into()),
            speaker: None,
            attachments: None,
        }
    }

    /// Helper: build a DB MessageRow. content 刻意存成
    /// `Blocks([Text])` 形态(insert_system_event / 未来
    /// insert_compaction_summary 的落库形态),与 wire 侧的
    /// `MessageContent::Text` 形成归一化对照 —— 这是 rehydrate
    /// 管线的真实往返(见模块文档的对齐前提)。
    fn row(seq: i64, role: &str, text: &str, metadata: Option<serde_json::Value>) -> MessageRow {
        MessageRow {
            id: seq,
            session_id: "s1".to_string(),
            role: role.to_string(),
            content: serde_json::json!([{ "type": "text", "text": text }]),
            text: text.to_string(),
            has_tool_calls: false,
            has_tool_results: false,
            created_at: "2026-08-18T00:00:00Z".to_string(),
            seq,
            metadata,
            ttfb_ms: None,
            gen_ms: None,
            total_ms: None,
            thinking_ms: None,
            speaker: None,
        }
    }

    /// Helper: build a compaction-summary DB row (design §2.1 metadata)。
    /// `cutoff` = 被压缩区最后一行 seq(修订后是 load-bearing 折叠点,
    /// 不再默认 seq-1 —— 那正是 PR2 check P1 的错误语义)。
    fn summary_row(seq: i64, cutoff: i64, text: &str) -> MessageRow {
        row(
            seq,
            "user",
            text,
            Some(serde_json::json!({
                "kind": COMPACTION_SUMMARY_KIND,
                "cutoff_seq": cutoff,
                "tokens_before": 171_000,
                "tokens_after": 52_300,
            })),
        )
    }

    // -----------------------------------------------------------------------
    // 命中:被压区(seq <= cutoff)折叠为单条摘要消息;保留区与本轮
    // 新输入逐字存活(PR2.5 P1 回归的单元半边)。
    //
    // 行布局镜像真实时间线:请求 N 压缩时 cutoff = 2(q2 行),摘要行
    // 按插入游标落在 seq 6(当前输入 5 之后),回答在 7;请求 N+1 的
    // wire = DB 全行 reload + 新输入。按旧语义(摘要行位置折叠)会
    // 把 wire[0..=6] 全折掉 —— 保留区(a3/q4/current question)与本
    // 请求提问全部丢失;按 cutoff 折叠只折 wire[0..=2]。
    //
    // Load-bearing:wire 侧摘要是 `MessageContent::Text`(rehydrate
    // 回发 `text` 列原文),DB 侧是 `Blocks([Text])` —— 严格
    // `PartialEq` 会假阴性,`to_text()` 归一化必须命中。
    // -----------------------------------------------------------------------

    #[test]
    fn hit_folds_prefix_into_summary() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None),
            row(2, "user", "q2", None),
            row(3, "assistant", "a3", None),
            row(4, "user", "q4", None),
            row(5, "user", "current question", None),
            summary_row(6, 2, "SUMMARY_BODY"),
            row(7, "assistant", "final answer", None),
        ];
        // wire = 请求 N+1 reload(DB 全行)+ 本轮新输入。
        let wire = vec![
            user("q1"),
            assistant("a1"),
            user("q2"),
            assistant("a3"),
            user("q4"),
            user("current question"),
            user("SUMMARY_BODY"),
            assistant("final answer"),
            user("follow-up question"),
        ];

        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(
                    anchor,
                    SummaryAnchor {
                        seq: 6,
                        content: "SUMMARY_BODY".to_string(),
                        cutoff: 2,
                    },
                    "anchor carries the DB row's seq + pure summary text + cutoff"
                );
                // [摘要] + [seq > cutoff 的常规行(q4 区/a4 区/提问/
                // 回答)] + 新输入;S 自身行(wire 下标 6,seq 6 > 2)
                // 被 kind 过滤剔除,不与头部摘要重复。
                assert_eq!(messages.len(), 6, "[summary] + kept regular rows + tail");
                assert_eq!(messages[0].role, Role::User);
                assert_eq!(messages[0].content.to_text(), "SUMMARY_BODY");
                // P1 回归核心:保留区 + 本请求提问 + 回答逐字存活。
                assert_eq!(messages[1].content.to_text(), "a3");
                assert_eq!(messages[2].content.to_text(), "q4");
                assert_eq!(messages[3].content.to_text(), "current question");
                assert_eq!(messages[4].content.to_text(), "final answer");
                assert_eq!(messages[5].content.to_text(), "follow-up question");
                // 被压区行(q1/a1/q2)不在。
                for gone in ["q1", "a1", "q2"] {
                    assert!(
                        !messages.iter().any(|m| m.content.to_text() == gone),
                        "被压区行 {gone} 必须出局"
                    );
                }
                // 折叠头来自 DB 行(SoT),Blocks 形态原样保留。
                assert_eq!(messages[0].content, {
                    let r = summary_row(6, 2, "SUMMARY_BODY");
                    let c: MessageContent = serde_json::from_value(r.content).unwrap();
                    c
                });
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    /// 摘要行是 DB 最后一行、wire 尾部无新输入的边界(group 式
    /// reload 尾 / 请求竞态):cut+1 == wire.len(),尾部为空不 panic。
    #[test]
    fn hit_with_summary_as_last_row_and_empty_tail() {
        let db = vec![row(0, "user", "q1", None), summary_row(1, 0, "S")];
        let wire = vec![user("q1"), user("S")];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 1);
                assert_eq!(messages.len(), 1, "[S] + wire[1..] 经 kind 过滤后为空");
                assert_eq!(messages[0].content.to_text(), "S");
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 未命中:无摘要行 → 原样返回(正常路径,无需 warn)。
    // -----------------------------------------------------------------------

    #[test]
    fn no_summary_row_returns_original() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None),
            row(2, "user", "q2", None),
        ];
        let wire = vec![user("q1"), assistant("a1"), user("q2"), user("new")];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(reason, MissReason::NoWatermark);
                assert_eq!(messages, before, "messages must be returned unchanged");
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    /// 全新 session(db 无任何行)。
    #[test]
    fn empty_db_rows_is_no_watermark() {
        let wire = vec![user("first message")];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &[]);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(reason, MissReason::NoWatermark);
                assert_eq!(messages, before);
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 对齐防御:idx±1 重对齐(单行位移容忍,比对对象 = cutoff 行,
    // 语义对齐新折叠边界)+ 彻底失败 Miss。
    // -----------------------------------------------------------------------

    /// 陈旧 store 缺 cutoff 前一行:wire[idx] 对不上、wire[idx-1] 命中。
    #[test]
    fn realigns_at_idx_minus_one_when_wire_missing_a_row() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None), // wire 缺这行(前端 store 陈旧)
            row(2, "user", "q2", None),      // cutoff 行
            summary_row(3, 2, "S"),
            row(4, "user", "q3", None),
        ];
        let wire = vec![
            user("q1"),
            user("q2"), // 对应 db 下标 2 的 cutoff 行,物理落在 wire[1]
            user("S"),
            user("q3"),
            user("new"),
        ];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 3);
                // 重对齐后 wire[cut+t] ↔ db_rows[row_idx + t] 的位移映射
                // 仍正确:S(wire 2)↔ db 3 → kind 剔除;q3(wire 3)↔ db 4。
                assert_eq!(messages.len(), 3, "[S] + q3 + new");
                assert_eq!(messages[0].content.to_text(), "S");
                assert_eq!(messages[1].content.to_text(), "q3");
                assert_eq!(messages[2].content.to_text(), "new");
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    /// wire 多一行(orphan-repair splice 先例:前端向 store 插入合成
    /// user 行):wire[idx] 对不上、wire[idx+1] 命中。
    #[test]
    fn realigns_at_idx_plus_one_when_wire_has_extra_row() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "user", "q2", None), // cutoff 行
            summary_row(2, 1, "S"),
            row(3, "user", "q3", None),
        ];
        let wire = vec![
            user("q1"),
            user("Tool execution was interrupted..."), // 前端 splice 的合成行
            user("q2"),                                // cutoff 行被推到 wire[2]
            user("S"),
            user("q3"),
            user("new"),
        ];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 2);
                assert_eq!(messages.len(), 3, "[S] + q3 + new");
                assert_eq!(messages[0].content.to_text(), "S");
                assert_eq!(messages[1].content.to_text(), "q3");
                assert_eq!(messages[2].content.to_text(), "new");
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    /// ±1 都对不上(缺多行 / 内容被改)→ Miss::AlignmentFailed
    /// (带 summary_seq 供调用方 warn),messages 原样 —— fail-open
    /// 回到 main 行为。
    #[test]
    fn alignment_failure_fails_open_with_reason() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None),
            summary_row(2, 1, "S"),
            row(3, "user", "q2", None),
        ];
        // wire 在 cutoff 行(db 下标 1)±1 完全对不上:缺两行 + 内容不同。
        let wire = vec![
            user("totally different"),
            user("another"),
            user("more drift"),
            user("new"),
        ];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(
                    reason,
                    MissReason::AlignmentFailed { summary_seq: 2 },
                    "miss reason carries the summary row's seq for the warn"
                );
                assert_eq!(messages, before, "fail-open: original wire untouched");
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 修订新增的 fail-open 路径:cutoff_seq 缺失/非法;cutoff 行不在
    // db_rows(D3 边角)。二者都没有安全默认值 —— seq-1 上界正是会吞
    // 掉保留区的错误语义。
    // -----------------------------------------------------------------------

    #[test]
    fn missing_cutoff_seq_metadata_fails_open() {
        let no_field = row(
            2,
            "user",
            "S",
            Some(serde_json::json!({ "kind": COMPACTION_SUMMARY_KIND })),
        );
        let non_integer = row(
            3,
            "user",
            "S2",
            Some(serde_json::json!({ "kind": COMPACTION_SUMMARY_KIND, "cutoff_seq": "12" })),
        );
        for db in [
            vec![row(0, "user", "q1", None), no_field],
            vec![row(0, "user", "q1", None), non_integer],
        ] {
            let summary_seq = db.last().unwrap().seq;
            let wire = vec![user("q1"), user("S-ish"), user("new")];
            let before = wire.clone();
            let result = apply_compaction_watermark(wire, &db);
            match result {
                WatermarkResult::Miss { messages, reason } => {
                    assert_eq!(
                        reason,
                        MissReason::AlignmentFailed { summary_seq },
                        "缺 cutoff_seq 字段/非整数 → AlignmentFailed fail-open"
                    );
                    assert_eq!(messages, before);
                }
                other => panic!("expected Miss, got {:?}", other),
            }
        }
    }

    /// cutoff 行被删(D3 cascade 删掉了被压区末行但摘要行存活的边角;
    /// 正常 cascade 会连摘要行一起删,这里防御异常布局)→ fail-open。
    #[test]
    fn cutoff_row_absent_from_db_rows_fails_open() {
        let db = vec![
            row(0, "user", "q1", None),
            summary_row(2, 5, "S"), // seq == 5 的行不在 db_rows
            row(6, "user", "q2", None),
        ];
        let wire = vec![user("q1"), user("S"), user("q2"), user("new")];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(reason, MissReason::AlignmentFailed { summary_seq: 2 });
                assert_eq!(messages, before);
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 多水位:倒序最新优先 + kind 过滤吸收旧摘要(修订 load-bearing)。
    // -----------------------------------------------------------------------

    #[test]
    fn newest_watermark_wins() {
        let db = vec![
            summary_row(1, 0, "S_OLD"),
            row(2, "user", "q1", None),
            summary_row(3, 2, "S_NEW"),
            row(4, "user", "q2", None),
        ];
        let wire = vec![
            user("S_OLD"),
            user("q1"),
            user("S_NEW"),
            user("q2"),
            user("new"),
        ];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 3, "newest watermark row wins");
                assert_eq!(anchor.content, "S_NEW");
                assert_eq!(anchor.cutoff, 2);
                // 折叠 wire[0..=2](含 S_OLD 行,seq 1 <= cutoff 2);
                // kept 区里 S_NEW 自身行(wire 下标 3)被 kind 过滤。
                assert_eq!(messages.len(), 3, "[S_NEW] + q2 + new");
                assert_eq!(messages[0].content.to_text(), "S_NEW");
                assert_eq!(messages[1].content.to_text(), "q2");
                assert_eq!(messages[2].content.to_text(), "new");
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    /// 多级水位 + kind 过滤的 load-bearing 场景:被吸收的旧摘要行
    /// seq 可以 **>** 新 cutoff(摘要行插在插入游标 = 保留区之后,
    /// PR2.5 修订前的按位置折叠正是栽在这里)。context 只见 S_NEW,
    /// S_OLD 行即便落在 kept 区间也被 kind 过滤出局。
    #[test]
    fn absorbed_old_summary_row_is_kind_filtered_even_above_cutoff() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "user", "q2", None),
            row(2, "user", "q3", None),
            row(3, "assistant", "a3", None),
            row(4, "user", "a4", None),
            summary_row(5, 2, "S_OLD"), // 请求 N 的摘要:折 [0..=2]
            row(6, "assistant", "answer", None),
            summary_row(7, 3, "S_NEW"), // 请求 N+1 增量:折 [S_OLD, a3]
        ];
        let wire = vec![
            user("q1"),
            user("q2"),
            user("q3"),
            assistant("a3"),
            user("a4"),
            user("S_OLD"),
            assistant("answer"),
            user("S_NEW"),
            user("new question"),
        ];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 7);
                assert_eq!(anchor.cutoff, 3);
                // kept = [a4(seq4), answer(seq6), 新输入];S_OLD(seq5 >
                // cutoff3)与 S_NEW 自身(seq7)都被 kind 过滤。
                assert_eq!(messages.len(), 4, "[S_NEW] + a4 + answer + new");
                assert_eq!(messages[0].content.to_text(), "S_NEW");
                assert_eq!(messages[1].content.to_text(), "a4");
                assert_eq!(messages[2].content.to_text(), "answer");
                assert_eq!(messages[3].content.to_text(), "new question");
                for gone in ["S_OLD", "q1", "q2", "q3", "a3"] {
                    assert!(
                        !messages.iter().any(|m| m.content.to_text() == gone),
                        "{gone} 不得出席(S_OLD 防 kind 过滤漏网)"
                    );
                }
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // D3 删除自愈:最新摘要行被 cascade 删除后,倒序"找现存最新"
    // 自然回退次新水位;全删则回全量(design §2.2,零专门代码)。
    // -----------------------------------------------------------------------

    #[test]
    fn d3_cascade_delete_self_heals_to_older_watermark() {
        // D3 编辑 q3(seq 3)→ cascade 删 seq>3(S_NEW@4、q4@5);
        // 前端 store 同步截断。S_OLD@2(cutoff=1)存活 → 成为现存
        // 最新水位,q1/q2 仍被它覆盖。
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "user", "q2", None),
            summary_row(2, 1, "S_OLD"),
            row(3, "user", "q3", None),
        ];
        let wire = vec![user("q1"), user("q2"), user("S_OLD"), user("resend of q3")];
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Applied { messages, anchor } => {
                assert_eq!(anchor.seq, 2, "fell back to the surviving watermark");
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[0].content.to_text(), "S_OLD");
                assert_eq!(messages[1].content.to_text(), "resend of q3");
            }
            other => panic!("expected Applied, got {:?}", other),
        }
    }

    #[test]
    fn d3_delete_all_summaries_returns_full_history() {
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None),
            row(2, "user", "q2", None),
        ];
        let wire = vec![user("q1"), assistant("a1"), user("q2"), user("new")];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(reason, MissReason::NoWatermark);
                assert_eq!(messages, before, "full history, main behavior");
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // metadata 容错:非法/异形 metadata 一律视为无 kind。
    // -----------------------------------------------------------------------

    #[test]
    fn malformed_metadata_is_not_a_summary() {
        // `None` = 普通行 / TEXT 列非法 JSON(load_session 的 .ok() 吸收)。
        let plain = row(0, "user", "q1", None);
        // 非 object 的 metadata(标量)。
        let scalar = row(1, "user", "q2", Some(serde_json::json!("not an object")));
        // kind 非 string。
        let numeric_kind = row(2, "user", "q3", Some(serde_json::json!({ "kind": 123 })));
        // 其它 kind(worktree_event 先例)。
        let other_kind = row(
            3,
            "user",
            "q4",
            Some(serde_json::json!({ "kind": "worktree_event" })),
        );
        let db = vec![plain, scalar, numeric_kind, other_kind];
        let wire = vec![user("q1"), user("q2"), user("q3"), user("q4"), user("new")];
        let before = wire.clone();
        let result = apply_compaction_watermark(wire, &db);
        match result {
            WatermarkResult::Miss { messages, reason } => {
                assert_eq!(reason, MissReason::NoWatermark);
                assert_eq!(messages, before);
            }
            other => panic!("expected Miss, got {:?}", other),
        }
    }

    /// `message_metadata_kind` helper 的直接口径测试。
    #[test]
    fn message_metadata_kind_reading() {
        assert_eq!(
            message_metadata_kind(Some(&serde_json::json!({"kind": "compaction_summary"}))),
            Some("compaction_summary")
        );
        assert_eq!(message_metadata_kind(None), None);
        assert_eq!(
            message_metadata_kind(Some(&serde_json::json!("scalar"))),
            None
        );
        assert_eq!(
            message_metadata_kind(Some(&serde_json::json!({"kind": 7}))),
            None
        );
        assert_eq!(
            message_metadata_kind(Some(&serde_json::json!({"other": 1}))),
            None
        );
    }

    // =====================================================================
    // PR2.5:compressible_cutoff_seq(design §4.3 修订:cutoff 精确计算)
    // =====================================================================

    /// 无折叠(prior = None)场景 = design §4.3 原式:
    /// `db_rows[cut - synthetic_prefix_len - 1].seq`。
    #[test]
    fn cutoff_seq_without_prior_uses_design_index_formula() {
        // wire ↔ db 1:1 + 尾部当前输入(design §4.3 对齐前提)。
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "assistant", "a1", None),
            row(2, "user", "q2", None),
            row(3, "assistant", "a3", None),
            row(4, "user", "q4", None),
            row(5, "user", "current question", None),
        ];
        let messages = vec![
            user("q1"),
            assistant("a1"),
            user("q2"),
            assistant("a3"),
            user("q4"),
            user("current question"),
        ];
        // 待压区 = messages[0..3](q1/a1/q2)→ 末行 = db_rows[2].seq = 2。
        let cut = 3usize;
        assert_eq!(
            compressible_cutoff_seq(0, cut, None, &db).unwrap(),
            2,
            "prior=None:精确 = db_rows[cut - P - 1].seq(非 seq-1 近似)"
        );
        // 同一布局 + 合成头(P=2):待压区下标平移,结果不变。
        let mut with_prefix = vec![user("mem-u"), assistant("mem-a")];
        with_prefix.extend(messages.clone());
        assert_eq!(compressible_cutoff_seq(2, cut + 2, None, &db).unwrap(), 2);
        // 关键反例(旧 bug 语义):摘要行插在游标 6 → "seq-1" 会得到 5
        // (当前输入行的 seq,折叠点吞掉保留区)。精确计算绝不能返回它。
        assert_ne!(compressible_cutoff_seq(0, cut, None, &db).unwrap(), 5);
    }

    /// 有折叠(prior = Some,水位命中或同 loop 上一轮压缩):待压区
    /// 常规行落在 `seq > prior.cutoff 且 kind ≠ summary` 的过滤后缀里
    /// —— 位移吸收 + 被吸收旧摘要行跳过(kind 过滤 load-bearing)。
    #[test]
    fn cutoff_seq_with_prior_offsets_past_fold_and_skips_summaries() {
        // 场景(design §2.2 修订的时间线):请求 N 折了 [0..=2]
        // (S@cutoff=2),保留区 a3@3/a4@4/提问@5,S 行插在 6,回答在 7。
        // 请求 N+1 水位折叠后的内存列表 = [S 消息] + a3/a4/answer + 新尾。
        let db = vec![
            row(0, "user", "q1", None),
            row(1, "user", "q2", None),
            row(2, "user", "q3", None),
            row(3, "assistant", "a3", None),
            row(4, "user", "a4", None),
            row(5, "user", "current question", None),
            summary_row(6, 2, "S"),
            row(7, "assistant", "answer", None),
        ];
        let anchor = SummaryAnchor {
            seq: 6,
            content: "S".to_string(),
            cutoff: 2,
        };
        // 内存列表(合成头 P=0)= 水位折叠产物:
        // [S 消息] + [a3, a4, current, answer] + 新尾。
        // 待压区 = 列表前 3 条 = [S, a3, a4] → 常规行 a3/a4,末行
        // = 过滤后缀(seq>2 且非 summary:a3/a4/current/answer)的
        // 第 2 个 = a4@4。
        let cut = 3usize;
        assert_eq!(
            compressible_cutoff_seq(0, cut, Some(&anchor), &db).unwrap(),
            4,
            "prior=Some:位移 + kind 过滤后取待压区末行真实 seq"
        );
        // 若 kind 过滤漏掉中间的 summary 行(错误实现会把 S@6 数进
        // 常规行),这里就会取到错行 —— 过滤后缀显式不含 seq 6。
        let cut_wider = 5usize; // 待压区 [S, a3, a4, current, answer] → 末行 answer@7
        assert_eq!(
            compressible_cutoff_seq(0, cut_wider, Some(&anchor), &db).unwrap(),
            7
        );
    }

    /// 退化边界(常见):待压区只剩 prior 摘要消息本身(regular == 0)
    /// → 传递覆盖面,cutoff 沿用 prior.cutoff(新摘要的折叠效果与
    /// "没有新压缩"完全一致,旧摘要行由 kind 过滤出局)。
    #[test]
    fn cutoff_seq_degenerate_summary_only_region_is_transitive() {
        let db = vec![row(0, "user", "q1", None), row(1, "user", "tail", None)];
        let anchor = SummaryAnchor {
            seq: 0,
            content: String::new(),
            cutoff: -1,
        };
        assert_eq!(
            compressible_cutoff_seq(0, 1, Some(&anchor), &db).unwrap(),
            -1,
            "regular == 0:cutoff = prior.cutoff(传递)"
        );
    }

    /// 对齐失效防御:推算下标越出 db_rows(wire 与 DB 行序失配 ——
    /// 如测试直灌 wire 而 DB 空 / 陈旧 store 多位移)→ Err,不 panic。
    #[test]
    fn cutoff_seq_out_of_bounds_errors_instead_of_panicking() {
        // DB 空(历史只在 wire 上,生产不会发生 —— reloadAfterFinalize
        // 保证 wire 镜像 DB;防御陈旧/异常)。
        assert!(compressible_cutoff_seq(0, 2, None, &[]).is_err());
        // regular 超出过滤后缀长度(prior 场景)。
        let db = vec![
            row(0, "user", "q1", None),
            summary_row(1, 0, "S"),
            row(2, "user", "q2", None),
        ];
        let anchor = SummaryAnchor {
            seq: 1,
            content: "S".to_string(),
            cutoff: 0,
        };
        // 过滤后缀只剩 q2@2 一个常规行,regular = 2 越界。
        assert!(compressible_cutoff_seq(0, 3, Some(&anchor), &db).is_err());
    }

    // =====================================================================
    // PR2:compute_preservation_region(design §4.1)
    // =====================================================================

    /// Helper: build an assistant turn carrying tool_use + a matching
    /// user tool_result(组原子性测试的素材)。
    fn tool_pair(id: &str) -> (ChatMessage, ChatMessage) {
        (
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/x"}),
                }]),
                speaker: None,
                attachments: None,
            },
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: id.to_string(),
                    content: "ok".to_string(),
                    is_error: false,
                }]),
                speaker: None,
                attachments: None,
            },
        )
    }

    /// Helper: big padding user text(~n_chars)。
    fn pad(n_chars: usize) -> String {
        "the quick brown fox jumps over the lazy dog. "
            .repeat(n_chars / 45 + 1)
            .chars()
            .take(n_chars)
            .collect()
    }

    /// 预算 clamp 三段:小窗 → 下限 15k;中窗 → window×10%;
    /// 大窗 → 上限 25k。
    #[test]
    fn preservation_budget_clamps() {
        assert_eq!(preservation_budget(20_000), 15_000, "小窗吃下限");
        assert_eq!(preservation_budget(150_000), 15_000, "恰在拐点");
        assert_eq!(preservation_budget(200_000), 20_000, "10% 窗口");
        assert_eq!(preservation_budget(300_000), 25_000, "大窗吃上限");
    }

    /// 从最后一组向前累积:窗口 20_000(预算被 clamp 到 15_000,测试
    /// 用不了那么大)→ 改用能真实驱动的口径:构造足够多的组,预算
    /// 15_000 token,断言累积从最后一组开始、覆盖到预算即停。
    #[tokio::test]
    async fn preservation_accumulates_from_tail_to_budget() {
        crate::memory::tokens::ensure_initialized().await;
        // 每条 pad(4000) ≈ 1000 token。头 2 + 40 条单例 + tail。
        let mut messages = vec![user("mem-u"), assistant("mem-a")];
        for i in 0..40 {
            messages.push(user(pad(4000)));
            messages.push(assistant(format!("ack {}", i)));
        }
        messages.push(user("current question"));
        let cut = compute_preservation_region(&messages, 2, 200_000).await;
        // 预算 15_000 → 保留区 ≈ 15-17 条 pad(每组 user+assistant)。
        // 断言:cut 在合成头之后(存在待压区),且保留区从 tail 侧
        // 起算连续 —— 这里验证下界与方向性(精确 token 边界由
        // estimator 决定,不作 brittle 断言)。
        assert!(cut > 2, "存在待压区(cut={})", cut);
        assert!(cut < messages.len() - 1, "保留区非空(cut={})", cut);
        // 保留区估算 ≈ [cut..] 的 token 总量应 ≥ 预算(护栏方向)。
        let preserved = crate::agent::context::estimate_messages_tokens(&messages[cut..]).await;
        assert!(
            preserved >= preservation_budget(200_000),
            "保留区至少覆盖预算:preserved={} budget={}",
            preserved,
            preservation_budget(200_000)
        );
    }

    /// 空待压区:历史太短(只有合成头 + 尾)→ cut == synthetic_prefix_len,
    /// 调用方直走机械。
    #[tokio::test]
    async fn preservation_empty_compressible_returns_prefix() {
        let messages = vec![user("mem-u"), assistant("mem-a"), user("q")];
        let cut = compute_preservation_region(&messages, 2, 200_000).await;
        assert_eq!(cut, 2, "无中段可压");

        let cut0 = compute_preservation_region(&messages[..2], 2, 200_000).await;
        assert_eq!(cut0, 2, "只有合成头");
    }

    /// typed-user 护栏:tail 是 tool_result 的工具循环里,最后一次
    /// typed user 在预算覆盖之外 → 其所在组强制并入保留区。
    #[tokio::test]
    async fn preservation_guardrail_forces_last_typed_user_group() {
        crate::memory::tokens::ensure_initialized().await;
        // 布局:头2 + 大量 pad(把预算吃满)+ typed user("真问题")
        // + assistant(tool_use) + user(tool_result)= tail。
        let mut messages = vec![user("mem-u"), assistant("mem-a")];
        for _ in 0..40 {
            messages.push(user(pad(4000)));
        }
        let typed_idx = messages.len();
        messages.push(user("the real question"));
        let (tu, tr) = tool_pair("tu_1");
        messages.push(tu);
        messages.push(tr);
        // tail_index = len-1(tr)。typed user 在 [2, tail) 内。
        let cut = compute_preservation_region(&messages, 2, 200_000).await;
        assert!(
            cut <= typed_idx,
            "typed user 组必入保留区:cut={} typed_idx={}",
            cut,
            typed_idx
        );
    }

    /// 组边界对齐:cut 落在组起点 —— 配对组不拆(RULE-A-001)。
    #[tokio::test]
    async fn preservation_cut_respects_group_boundaries() {
        crate::memory::tokens::ensure_initialized().await;
        let mut messages = vec![user("mem-u"), assistant("mem-a")];
        for _ in 0..40 {
            let (tu, tr) = tool_pair("tu");
            messages.push(tu);
            messages.push(tr);
        }
        messages.push(user("current question"));
        let cut = compute_preservation_region(&messages, 2, 200_000).await;
        // 组起点全是偶数偏移(2,4,6,...):配对组从偶数下标开始。
        assert_eq!(cut % 2, 0, "cut 必须落在配对组起点(偶数下标)");
        // 保留区里每个 tool_use 的下一条就是它的 tool_result。
        for (i, m) in messages[cut..].iter().enumerate() {
            if let MessageContent::Blocks(blocks) = &m.content {
                if blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
                {
                    let next = messages[cut..].get(i + 1);
                    assert!(
                        matches!(next, Some(n) if n.role == Role::User
                            && matches!(&n.content, MessageContent::Blocks(bs) if bs.iter()
                                .any(|b| matches!(b, ContentBlock::ToolResult { .. })))),
                        "保留区 tool_use 必须紧跟 tool_result(idx={})",
                        i
                    );
                }
            }
        }
    }

    /// synthetic_prefix_len 起算:不同合成头长度下,待压区都不卷进
    /// 合成头(评审 P2-2 —— skill listing 不喂摘要)。
    #[tokio::test]
    async fn preservation_starts_after_synthetic_prefix() {
        crate::memory::tokens::ensure_initialized().await;
        let mut messages = vec![user("skills listing"), user("mem-u"), assistant("mem-a")];
        for _ in 0..40 {
            messages.push(user(pad(4000)));
        }
        messages.push(user("current question"));
        // synthetic_prefix_len = 3(memory 头对 + skills)。
        let cut = compute_preservation_region(&messages, 3, 200_000).await;
        assert!(cut >= 3, "cut 不进合成头(cut={})", cut);
        assert!(cut > 3, "中段有 40 条 pad,待压区非空(cut={})", cut);
    }

    // =====================================================================
    // PR2:build_compaction_prompt(design §6)
    // =====================================================================

    /// 模板骨架:段落标题 + transcript 包裹 + 输出指令;无 prior 时
    /// 不注入 `<prior-summary>`。
    #[tokio::test]
    async fn prompt_template_skeleton_without_prior() {
        crate::memory::tokens::ensure_initialized().await;
        let compressible = vec![user("fix the login bug"), assistant("I'll look at auth.rs")];
        let prompt = build_compaction_prompt(&compressible, None, 200_000, None).await;
        assert!(prompt.contains("CONTEXT CHECKPOINT COMPACTION"));
        assert!(prompt.contains("Primary Request and Intent"));
        assert!(prompt.contains("Output ONLY the summary"));
        assert!(
            !prompt.contains("<prior-summary>"),
            "无 anchor 不注入 prior 块"
        );
        assert!(prompt.contains("<transcript>"));
        assert!(prompt.contains("[user] fix the login bug"));
        assert!(prompt.contains("[assistant] I'll look at auth.rs"));
    }

    /// prior 注入:纯摘要 content 进 `<prior-summary>`;anchor 消息
    /// (compressible[0])不进 transcript(不重复喂,评审 P1-2)。
    #[tokio::test]
    async fn prompt_injects_prior_summary_and_skips_anchor_message() {
        crate::memory::tokens::ensure_initialized().await;
        let anchor = SummaryAnchor {
            seq: 42,
            content: "PRIOR_SUMMARY_BODY".to_string(),
            cutoff: 41,
        };
        let compressible = vec![
            // anchor 消息按构造位于 compressible[0]。
            user("PRIOR_SUMMARY_BODY"),
            user("second question"),
            assistant("second answer"),
        ];
        let prompt = build_compaction_prompt(&compressible, Some(&anchor), 200_000, None).await;
        assert!(
            prompt.contains("<prior-summary>\nPRIOR_SUMMARY_BODY\n</prior-summary>"),
            "prior 块注入纯摘要 content"
        );
        assert!(prompt.contains("THE CONVERSATION WINS"));
        // transcript 只渲染 slice[1..]:anchor 消息不重复出现。
        assert_eq!(
            prompt.matches("PRIOR_SUMMARY_BODY").count(),
            1,
            "anchor 内容只出现一次(prior 块),不进 transcript"
        );
        assert!(prompt.contains("[user] second question"));
    }

    /// tool_result 截 2000 chars + 记号;thinking 不渲染;tool_use 只留
    /// name + input 截断。
    #[tokio::test]
    async fn prompt_transcript_rendering_rules() {
        crate::memory::tokens::ensure_initialized().await;
        let huge_result = "x".repeat(5_000);
        let asst = ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: "secret".to_string(),
                    signature: "sig".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/f", "extra": "y".repeat(1_000)}),
                },
            ]),
            speaker: None,
            attachments: None,
        };
        let result_msg = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: huge_result.clone(),
                is_error: false,
            }]),
            speaker: None,
            attachments: None,
        };
        let prompt = build_compaction_prompt(&[asst, result_msg], None, 200_000, None).await;
        assert!(
            prompt.contains("[tool_use read_file "),
            "tool_use 留 name+input"
        );
        assert!(
            !prompt.contains("secret") && !prompt.contains("sig"),
            "thinking 不渲染"
        );
        assert!(
            prompt.contains(&format!("...[truncated {} chars]", 5_000 - 2_000)),
            "tool_result 截 2000 chars 加记号"
        );
        // tool_use input(1028 chars JSON)截 400:出现第二个截断记号,
        // 且被丢弃量为正(不硬编码 JSON 总长,serde_json Map 序不保证)。
        let tool_use_truncations = prompt.matches("...[truncated ").count();
        assert!(
            tool_use_truncations >= 2,
            "tool_use input 也须截断加记号(共 {} 处)",
            tool_use_truncations
        );
        assert!(
            !prompt.contains(&"y".repeat(500)),
            "tool_use input 超长部分被截掉"
        );
        // 截断后的 tool_result 渲染不超过 cap + 记号长度。
        let line = prompt
            .lines()
            .find(|l| l.starts_with("[user] [tool_result"))
            .expect("tool_result line present");
        assert!(line.len() < "[user] [tool_result ".len() + 2_000 + 64);
    }

    /// 图片占位:ImageRef 渲染 `[image attached: <file>]`。
    #[tokio::test]
    async fn prompt_renders_image_placeholder() {
        crate::memory::tokens::ensure_initialized().await;
        let m = ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ImageRef {
                file: "/tmp/shot.png".to_string(),
                media_type: "image/png".to_string(),
            }]),
            speaker: None,
            attachments: None,
        };
        let prompt = build_compaction_prompt(&[m], None, 200_000, None).await;
        assert!(prompt.contains("[image attached: /tmp/shot.png]"));
    }

    /// transcript 总预算溢出:从最旧条目丢起 + `[older transcript
    /// omitted]` 记号,最近条目保留。
    #[tokio::test]
    async fn prompt_transcript_budget_drops_oldest() {
        crate::memory::tokens::ensure_initialized().await;
        // 窗口 20_000 → transcript 预算 14_000 token ≈ 56k chars。
        // 造 20 条 pad(6_000 chars ≈ 1_500 token)≈ 30k token,溢出。
        let compressible: Vec<ChatMessage> = (0..20)
            .map(|i| {
                let text = format!("MSG_{} {}", i, pad(6_000));
                user(text)
            })
            .collect();
        let prompt = build_compaction_prompt(&compressible, None, 20_000, None).await;
        assert!(
            prompt.contains("[older transcript omitted]"),
            "溢出丢最旧须留记号"
        );
        assert!(!prompt.contains("MSG_0 "), "最旧条目被丢弃");
        assert!(prompt.contains("MSG_19 "), "最近条目保留");
    }

    /// clamp_summary_output:超长摘要按 4k token ≈ 16k chars 截断。
    #[test]
    fn summary_output_clamped() {
        assert_eq!(
            clamp_summary_output("short".to_string()),
            "short",
            "短文本原样"
        );
        let huge = "a".repeat(20_000);
        let clamped = clamp_summary_output(huge);
        assert_eq!(clamped.chars().count(), 16_384, "4k token × 4 chars/token");
    }

    // =====================================================================
    // PR2:熔断 registry(design §4.4)
    // =====================================================================

    /// 3 次连续失败触发熔断;成功清零;清理;session 隔离。
    #[tokio::test]
    async fn breaker_counts_resets_and_clears() {
        let r = CompactionRegistry::default();
        let sid = "s-breaker";
        assert!(!r.is_tripped(sid).await, "初始未熔断");
        r.record_failure(sid).await;
        r.record_failure(sid).await;
        assert!(!r.is_tripped(sid).await, "2 次不熔断");
        assert_eq!(r.failures(sid).await, 2);
        r.record_failure(sid).await;
        assert!(r.is_tripped(sid).await, "3 次熔断");
        // 成功清零。
        r.record_success(sid).await;
        assert!(!r.is_tripped(sid).await);
        assert_eq!(r.failures(sid).await, 0);
        // 再失败 1 次不熔断(计数已清零,连续性被成功打断)。
        r.record_failure(sid).await;
        assert!(!r.is_tripped(sid).await);
        // session 隔离 + clear。
        r.record_failure(sid).await;
        r.record_failure(sid).await;
        r.record_failure(sid).await;
        assert!(r.is_tripped(sid).await);
        assert!(!r.is_tripped("s-other").await, "跨 session 不串");
        r.clear(sid).await;
        assert_eq!(r.failures(sid).await, 0, "clear 后归零");
    }
}
