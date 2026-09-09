//! 定时审议转录自动导出(GCE-M4a Step 5,任务
//! 09-07-gce-m4a-scheduled-deliberation design §5)。
//!
//! daemon 在**定时场**(`sessions.metadata.created_via == "scheduled"`)
//! 收官时自动导出 markdown 转录到 `{app_data_dir}/discussions/` —— 不依赖
//! 源码检出(M1 脚本的 out/ 落仓库根是 CLI 消费语境,daemon 原生导出
//! 落用户数据根)。调用点 = 编排器终态块
//! ([`crate::agent::group_chat_loop`],`finalize_group_chat_lifecycle`
//! 之后),**失败仅 warn 不影响终态落库**(沿 M2「导出失败降级」先例);
//! 守卫 `gc_ctx.created_via == Some("scheduled")` 挂在 ctx 字段上
//! (评审 P1-4:终态块全通道共享,无守卫一挂就会把 GUI/MCP 场也导出)。
//!
//! 内容边界(评审未决 1/4 定案):头部最低集 = 任务名 / session_id /
//! 起止时间 / 参与者清单 / stop_reason;正文 = 轮次 + per-speaker 发言
//! (**轮次 = seq + speaker 连续归组近似** —— messages 表无 round 列,
//! 字节级一致性不做保证);tool 调用含名字 + 参数摘要、**不含 blobs**;
//! 尾部 = discussion_summary。与 M1 脚本 `renderTranscript` 是两套实现
//! (人查阅 vs headless 消费),核心结构对齐即可(design §10 已接受)。

use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

/// task_name → 文件名白名单片段(评审未决 3 升必做):CJK 保留、剥
/// 路径分隔符(`/` `\`)、Windows 保留字符与控制字符、压缩连续空白、
/// 长度截断(40 chars)、首尾连字符修剪;空结果回退 `discussion`
/// (文件名永不为空、永不携带路径分量 —— 用户输入不可信)。
pub(crate) fn sanitize_task_name_for_path(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        // is_whitespace 判定必须先于 is_control:\t 等同时属于两类,
        // 语义上是分隔符(折成空格),走 control 分支丢弃会把相邻词
        // 粘连成「空白压缩」。
        if ch.is_whitespace() {
            out.push(' ');
        } else if ch.is_control()
            || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        {
            continue;
        } else if ch == '.' && out.ends_with('.') {
            // 连续点号折叠成单个:剥离路径分隔符后残留的「..」不得进
            // 文件名(防目录逃逸)。
            continue;
        } else {
            out.push(ch);
        }
    }
    let collapsed: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(40).collect();
    let trimmed = truncated.trim().trim_matches('-').to_string();
    if trimmed.is_empty() {
        "discussion".to_string()
    } else {
        trimmed
    }
}

/// 从 wire content blocks(JSON)提取 tool_use 证据链(名字 + 1 个关键
/// 参数,60 字符截断)—— M1 `summarizeToolUses` 的 Rust 对齐;纯
/// tool_result 轮(content 无 tool_use 块)返回 None。**不含 blobs**:
/// 只取名字与短参数摘要,tool_result 正文不进转录。
fn summarize_tool_uses(content: &serde_json::Value) -> Option<String> {
    let arr = content.as_array()?;
    let uses: Vec<String> = arr
        .iter()
        .filter_map(|b| {
            if b.get("type")?.as_str()? != "tool_use" {
                return None;
            }
            let name = b.get("name")?.as_str()?;
            let key = ["command", "file_path", "path", "pattern", "query", "topic"]
                .iter()
                .find_map(|k| {
                    b.get("input")
                        .and_then(|i| i.get(*k))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("");
            let key: String = key.chars().take(60).collect();
            Some(format!("{name}{{{}}}", key.replace('\n', " ")))
        })
        .collect();
    if uses.is_empty() {
        None
    } else {
        Some(uses.join(" "))
    }
}

/// 渲染入参(纯函数,单测直调;字段语义见
/// [`render_scheduled_transcript`] 头部注释)。
pub(crate) struct TranscriptRenderArgs<'a> {
    pub task_name: &'a str,
    pub session_id: &'a str,
    /// RFC3339(或任何可读时间串)。
    pub started_at: &'a str,
    pub ended_at: &'a str,
    /// moderator 的展示标签(session.model)。
    pub moderator_label: &'a str,
    /// `(name, model_id)` 清单。
    pub participants: &'a [(String, String)],
    pub stop_reason: &'a str,
    pub messages: &'a [crate::db::types::MessageRow],
    pub discussion_summary: Option<&'a str>,
    /// C2 证据链:结构化结论(锚点带校验结果)。None/空 = 省略节。
    pub discussion_detail: Option<&'a crate::agent::discussion_detail::DiscussionDetail>,
}

/// 渲染转录 markdown(纯函数)。结构:
/// `# 定时审议「任务名」转录` + 头部最低集(5 行)→ 正文(seq+speaker
/// 连续归组:换 speaker 落 `### {speaker}` 小节;发言 blockquote 隔离
/// 碎格式,工具轮一行证据链)→ 尾部 discussion_summary。
pub(crate) fn render_scheduled_transcript(args: &TranscriptRenderArgs<'_>) -> String {
    let TranscriptRenderArgs {
        task_name,
        session_id,
        started_at,
        ended_at,
        moderator_label,
        participants,
        stop_reason,
        messages,
        discussion_summary,
        discussion_detail,
    } = args;
    let roster = if participants.is_empty() {
        moderator_label.to_string()
    } else {
        format!(
            "{moderator_label} 主持 + {}",
            participants
                .iter()
                .map(|(n, m)| format!("{n}/{m}"))
                .collect::<Vec<_>>()
                .join(" + ")
        )
    };
    let mut out = String::new();
    out.push_str(&format!("# 定时审议「{task_name}」转录\n\n"));
    out.push_str(&format!("- session: `{session_id}`({roster})\n"));
    out.push_str(&format!("- 任务: {task_name}\n"));
    out.push_str(&format!("- 起止: {started_at} → {ended_at}\n"));
    out.push_str(&format!("- stop_reason: {stop_reason}\n"));
    out.push_str(&format!("- 消息数: {}\n", messages.len()));
    out.push_str("\n---\n");
    // 轮次归组:seq+speaker 连续归组近似(同 speaker 连续行落同节)。
    let mut current_speaker: Option<String> = None;
    for m in messages.iter() {
        let is_tool_turn = m.speaker.is_none() && (m.has_tool_calls || m.has_tool_results);
        let key: String = match (&m.speaker, is_tool_turn) {
            (Some(s), _) => s.clone(),
            (None, true) => "工具轮".to_string(),
            (None, false) => "用户".to_string(),
        };
        if current_speaker.as_deref() != Some(key.as_str()) {
            out.push_str(&format!("\n### {key}\n\n"));
            current_speaker = Some(key);
        }
        if is_tool_turn {
            let tools = summarize_tool_uses(&m.content)
                .unwrap_or_else(|| "(工具调用轮/tool_result)".to_string());
            out.push_str(&format!("- seq{} (工具调用 {})\n", m.seq, tools));
            continue;
        }
        let body = m.text.trim();
        if body.is_empty() {
            out.push_str(&format!("- seq{} (空)\n", m.seq));
        } else {
            // blockquote 隔离:LLM 输出里的列表/标题碎片不打断归组结构。
            for line in body.split('\n') {
                out.push_str(&format!("  > {line}\n"));
            }
            out.push('\n');
        }
    }
    out.push_str("\n---\n");
    match discussion_summary {
        Some(summary) if !summary.trim().is_empty() => {
            out.push_str(&format!("\n## discussion_summary\n\n{summary}\n"));
        }
        _ => {
            out.push_str("\n## discussion_summary\n\n(缺失:本场未走 end_discussion 收束,读转录尾段人工收束)\n");
        }
    }
    out.push_str(&render_conclusions_section(*discussion_detail));
    out
}

/// C2 证据链:结构化结论节(纯函数)。detail None 或 conclusions 空 →
/// 空串(旧场零回归);锚点后缀校验记号:✓ = ok,⚠(check) = 断证,
/// 无记号 = 未校验(live 期/root 缺失)。
fn render_conclusions_section(
    detail: Option<&crate::agent::discussion_detail::DiscussionDetail>,
) -> String {
    use crate::agent::discussion_detail::{AnchorCheck, DiscussionDetail, Stance};

    fn anchor_suffix(a: &crate::agent::discussion_detail::Anchor) -> String {
        let loc = match a.line {
            Some(l) => format!("{}:{}", a.path, l),
            None => a.path.clone(),
        };
        match a.check {
            Some(AnchorCheck::Ok) => format!("`{loc}` ✓"),
            Some(
                c
                @ (AnchorCheck::NotFound | AnchorCheck::LineOutOfRange | AnchorCheck::OutsideRoot),
            ) => {
                let word = match c {
                    AnchorCheck::NotFound => "not_found",
                    AnchorCheck::LineOutOfRange => "line_out_of_range",
                    AnchorCheck::OutsideRoot => "outside_root",
                    _ => unreachable!(),
                };
                format!("`{loc}` ⚠({word})")
            }
            _ => format!("`{loc}`"),
        }
    }

    let Some(DiscussionDetail {
        conclusions,
        open_questions,
    }) = detail
    else {
        return String::new();
    };
    if conclusions.is_empty() && open_questions.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if !conclusions.is_empty() {
        out.push_str("\n## conclusions\n\n");
        for c in conclusions {
            let stance = match c.stance {
                Stance::Verified => "verified",
                Stance::Inferred => "inferred",
                Stance::Disputed => "disputed",
            };
            if c.anchors.is_empty() {
                out.push_str(&format!("- [{stance}] {}\n", c.claim));
            } else {
                let anchors = c
                    .anchors
                    .iter()
                    .map(anchor_suffix)
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&format!("- [{stance}] {} — {anchors}\n", c.claim));
            }
        }
    }
    if !open_questions.is_empty() {
        out.push_str("\n## open_questions\n\n");
        for q in open_questions {
            out.push_str(&format!("- {q}\n"));
        }
    }
    out
}

/// 导出定时场转录(编排器终态钩子的实现)。读 session + messages +
/// checkpoint(起止时间的最近见证)→ 渲染 → 落
/// `{app_data_dir}/discussions/{YYYY-MM-DD 本地}-{task_name 白名单清洗}
/// -{sid 前 8}.md`。失败返回 Err(调用方仅 warn —— 不影响终态落库)。
pub(crate) async fn export_scheduled_transcript(
    db: &SqlitePool,
    app_data_dir: &Path,
    session_id: &str,
    stop_reason: &str,
    discussion_summary: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let loaded = crate::db::load_session(db, session_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;
    let session = &loaded.session;
    // metadata:participants / scheduled_task_name(created_via 守卫由
    // 调用点持有 —— 这里读归因键;坏 metadata 降级空清单,不阻断导出)。
    let (participants, task_name) = match session.metadata.as_ref() {
        Some(v) => {
            let parts: Vec<(String, String)> = v
                .get("participants")
                .and_then(|p| p.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            Some((
                                p.get("name")?.as_str()?.to_string(),
                                p.get("model")?.as_str()?.to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let task = v
                .get("scheduled_task_name")
                .and_then(|t| t.as_str())
                .unwrap_or("未命名任务")
                .to_string();
            (parts, task)
        }
        None => (Vec::new(), "未命名任务".to_string()),
    };
    // 起止时间:checkpoint.started_at(本讨论真实起点;钩子挂在终态
    // checkpoint 删除之前)→ 首消息 created_at → session.created_at 兜底。
    let started_at = crate::db::get_group_chat_checkpoint(db, session_id)
        .await
        .ok()
        .flatten()
        .map(|cp| cp.started_at)
        .or_else(|| loaded.messages.first().map(|m| m.created_at.clone()))
        .unwrap_or_else(|| session.created_at.clone());
    let ended_at = loaded
        .messages
        .last()
        .map(|m| m.created_at.clone())
        .unwrap_or_else(|| session.updated_at.clone());

    let content = render_scheduled_transcript(&TranscriptRenderArgs {
        task_name: &task_name,
        session_id,
        started_at: &started_at,
        ended_at: &ended_at,
        moderator_label: &session.model,
        participants: &participants,
        stop_reason,
        messages: &loaded.messages,
        discussion_summary,
        // C2:行里读(单一事实源 = DB,导出前 finalize 已落列);
        // 坏 JSON 降级 None,不阻断导出。
        discussion_detail: session
            .discussion_detail
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .as_ref(),
    });
    let date = chrono::Local::now().format("%Y-%m-%d");
    let sid8: String = session_id.chars().take(8).collect();
    let dir = app_data_dir.join("discussions");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!(
        "{date}-{}-{sid8}.md",
        sanitize_task_name_for_path(&task_name)
    ));
    tokio::fs::write(&path, content).await?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_row(
        seq: i64,
        speaker: Option<&str>,
        text: &str,
        content: serde_json::Value,
        tool: bool,
    ) -> crate::db::types::MessageRow {
        crate::db::types::MessageRow {
            id: seq,
            session_id: "sid".into(),
            role: if speaker.is_some() {
                "assistant"
            } else {
                "user"
            }
            .into(),
            content,
            text: text.into(),
            has_tool_calls: tool,
            has_tool_results: tool,
            created_at: format!("2026-09-07T10:0{seq}:00Z"),
            seq,
            metadata: None,
            ttfb_ms: None,
            gen_ms: None,
            total_ms: None,
            thinking_ms: None,
            speaker: speaker.map(str::to_string),
            status: None,
        }
    }

    /// 白名单清洗:CJK 保留、路径分隔符/控制字符剥除、长度截断、空白
    /// 压缩、空回退(评审未决 3 必做项)。
    #[test]
    fn sanitize_keeps_cjk_strips_path_and_control_chars() {
        assert_eq!(sanitize_task_name_for_path("每周架构复盘"), "每周架构复盘");
        assert_eq!(
            sanitize_task_name_for_path("a/b\\c:d*e?f\"g<h>i|j"),
            "abcdefghij",
            "path separators + windows-reserved stripped"
        );
        assert_eq!(
            sanitize_task_name_for_path("含\u{0000}控制\u{0007}符"),
            "含控制符"
        );
        assert_eq!(
            sanitize_task_name_for_path("  多   空白\t压缩  "),
            "多 空白 压缩"
        );
        let long = "字".repeat(80);
        assert_eq!(sanitize_task_name_for_path(&long).chars().count(), 40);
        assert_eq!(
            sanitize_task_name_for_path("///"),
            "discussion",
            "empty fallback"
        );
        assert_eq!(sanitize_task_name_for_path(""), "discussion");
        // 不含路径分量(防目录逃逸)。
        let s = sanitize_task_name_for_path("../../etc/passwd");
        assert!(!s.contains('/') && !s.contains('\\') && !s.contains(".."));
    }

    /// 渲染:头部最低集 / seq+speaker 连续归组 / 工具证据链(名+参数,
    /// 无 blobs)/ summary 尾部 / blockquote 隔离。
    #[test]
    fn render_groups_by_speaker_and_carries_evidence_chain() {
        let participants = vec![
            ("架构".to_string(), "uuid-a".to_string()),
            ("产品".to_string(), "uuid-b".to_string()),
        ];
        let messages = vec![
            msg_row(0, None, "复盘上周架构", serde_json::json!(""), false),
            msg_row(
                1,
                None,
                "",
                serde_json::json!([
                    {"type": "tool_use", "id": "t1", "name": "grep", "input": {"pattern": "group_chat"}},
                    {"type": "tool_result", "tool_use_id": "t1", "content": "huge blob..." }
                ]),
                true,
            ),
            msg_row(
                2,
                Some("架构"),
                "第一行\n- 列表碎片",
                serde_json::json!(""),
                false,
            ),
            msg_row(3, Some("架构"), "架构追加", serde_json::json!(""), false),
            msg_row(4, Some("产品"), "产品观点", serde_json::json!(""), false),
        ];
        let out = render_scheduled_transcript(&TranscriptRenderArgs {
            task_name: "每周复盘",
            session_id: "sid-1234567890",
            started_at: "2026-09-07T10:00:00Z",
            ended_at: "2026-09-07T10:12:00Z",
            moderator_label: "MiniMax-M3",
            participants: &participants,
            stop_reason: "group_chat_end",
            messages: &messages,
            discussion_summary: Some("共识A"),
            discussion_detail: None,
        });
        // 头部最低集。
        assert!(out.contains("# 定时审议「每周复盘」转录"));
        assert!(out
            .contains("- session: `sid-1234567890`(MiniMax-M3 主持 + 架构/uuid-a + 产品/uuid-b)"));
        assert!(out.contains("- 起止: 2026-09-07T10:00:00Z → 2026-09-07T10:12:00Z"));
        assert!(out.contains("- stop_reason: group_chat_end"));
        // 归组:架构的 seq2/seq3 同节(一个 ### 架构),换 speaker 落新节。
        assert_eq!(out.matches("### 架构").count(), 1);
        assert_eq!(out.matches("### 产品").count(), 1);
        assert!(out.matches("### 用户").count() >= 1);
        // 工具证据链:名 + 关键参数;无 blobs。
        assert!(out.contains("grep{group_chat}"));
        assert!(!out.contains("huge blob"));
        // blockquote 隔离碎格式。
        assert!(out.contains("  > - 列表碎片"));
        // 尾部 summary。
        assert!(out.contains("## discussion_summary\n\n共识A"));
    }

    /// summary 缺失:如实落警告行(不伪造)。
    #[test]
    fn render_warns_when_summary_missing() {
        let out = render_scheduled_transcript(&TranscriptRenderArgs {
            task_name: "t",
            session_id: "s",
            started_at: "a",
            ended_at: "b",
            moderator_label: "m",
            participants: &[],
            stop_reason: "max_rounds",
            messages: &[],
            discussion_summary: None,
            discussion_detail: None,
        });
        assert!(out.contains("缺失:本场未走 end_discussion 收束"));
    }

    /// C2 证据链:conclusions / open_questions 节渲染(stance 标注 +
    /// 锚点校验记号);detail None 或空 = 省略节(旧场零回归)。
    #[test]
    fn render_conclusions_section_with_anchor_checks() {
        let detail: crate::agent::discussion_detail::DiscussionDetail =
            serde_json::from_str(
                r#"{"conclusions":[
                    {"claim":"实锚","anchors":[{"path":"a.rs","line":2,"check":"ok"}],"stance":"verified"},
                    {"claim":"断证","anchors":[{"path":"b.rs","line":9,"check":"not_found"}],"stance":"verified"},
                    {"claim":"推测","stance":"inferred"},
                    {"claim":"争议","anchors":[{"path":"c.rs"}],"stance":"disputed"}
                ],"open_questions":["何时复核"]}"#,
            )
            .unwrap();
        let out = render_conclusions_section(Some(&detail));
        assert!(out.contains("## conclusions"));
        assert!(out.contains("- [verified] 实锚 — `a.rs:2` ✓"));
        assert!(out.contains("- [verified] 断证 — `b.rs:9` ⚠(not_found)"));
        assert!(out.contains("- [inferred] 推测\n"));
        assert!(out.contains("- [disputed] 争议 — `c.rs`\n"));
        assert!(out.contains("## open_questions\n\n- 何时复核\n"));

        // None / 空 detail = 空串,主渲染不受影响。
        assert_eq!(render_conclusions_section(None), "");
        let empty = crate::agent::discussion_detail::DiscussionDetail::default();
        assert_eq!(render_conclusions_section(Some(&empty)), "");
    }

    /// 导出集成:seed 定时场 session(metadata 归因 + checkpoint 起点)
    /// + 消息行 → 落 `{app_data_dir}/discussions/{date}-{清洗后的
    /// 任务名}-{sid8}.md`,头部含任务名/参与者/summary。
    #[tokio::test]
    async fn export_writes_markdown_under_discussions_dir() {
        let pool = crate::db::test_support::test_pool().await;
        let name = format!("gc-tr-{}", uuid::Uuid::new_v4().simple());
        let path = format!("/tmp/{name}");
        crate::db::create_project(&pool, &name, &path, false, None)
            .await
            .unwrap();
        let project = crate::db::list_projects(&pool, false)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        let metadata = serde_json::json!({
            "participants": [{"name": "架构", "model": "uuid-a"}],
            "created_via": "scheduled",
            "scheduled_task_id": "task-1",
            "scheduled_task_name": "每周/架构:复盘",
        });
        crate::db::create_session(
            &pool,
            &sid,
            &project.id,
            &path,
            "MiniMax-M3",
            None,
            Some("group_chat"),
            Some(&metadata.to_string()),
        )
        .await
        .unwrap();
        crate::db::upsert_group_chat_checkpoint(&pool, &sid, 2, 0)
            .await
            .unwrap();
        // started_at 是 INSERT 时写死的 now(ON CONFLICT 不动它),转录
        // 头部「起止」取它 —— fixture 覆盖成确定性值以断言。
        sqlx::query(
            "UPDATE group_chat_checkpoints SET started_at = '2026-09-07T10:00:00Z' \
             WHERE session_id = ?",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (session_id, role, content, text, created_at, seq) \
             VALUES (?, 'user', '[]', '复盘议题', '2026-09-07T10:00:00Z', 0)",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages (session_id, role, content, text, speaker, created_at, seq) \
             VALUES (?, 'assistant', '[]', '架构发言', '架构', '2026-09-07T10:05:00Z', 1)",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let out = export_scheduled_transcript(
            &pool,
            tmp.path(),
            &sid,
            "group_chat_end",
            Some("共识:保持现状"),
        )
        .await
        .expect("export");
        assert!(out.starts_with(tmp.path().join("discussions")), "{out:?}");
        let fname = out.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            fname.starts_with("2026-") && fname.contains("每周架构复盘"),
            "filename = date + sanitized task name: {fname}"
        );
        assert!(fname.ends_with(&format!("-{}.md", &sid[..8])));
        assert!(!fname.contains('/'), "task name must not carry path parts");
        let content = tokio::fs::read_to_string(&out).await.unwrap();
        assert!(
            content.contains("定时审议「每周/架构:复盘」转录"),
            "{content}"
        );
        assert!(content.contains("架构/uuid-a"));
        assert!(content.contains("2026-09-07T10:00:00Z → 2026-09-07T10:05:00Z"));
        assert!(content.contains("### 架构"));
        assert!(content.contains("共识:保持现状"));
    }
}
