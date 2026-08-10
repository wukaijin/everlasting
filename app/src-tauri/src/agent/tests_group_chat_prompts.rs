//! 群聊 prompt/role_history 纯函数单元测试(拆分自 group_chat_loop.rs,
//! 08-07-large-file-splitting)。

#![cfg(test)]

use crate::agent::group_chat::GroupChatCtx;
use crate::agent::group_chat_prompts::*;
use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};
use crate::tools::end_discussion::END_DISCUSSION_TOOL_NAME;
use crate::tools::nominate_speaker::NOMINATE_SPEAKER_TOOL_NAME;

mod tests {
    use super::*;

    /// R1 (08-07-group-chat-toolset-and-identity): the moderator's tool set
    /// is the research whitelist PLUS the two arbitration tools — and NOTHING
    /// else. This replaces the pre-R1 blacklist; DB session 8be4687f showed
    /// the full `builtin_tools()` set leaking `update_checklist` / `use_skill`
    /// into group chat, which weak models abused to self-build a speaker
    /// rotation and hallucinate a skill. The whitelist is exhaustive.
    #[test]
    fn group_chat_tool_defs_moderator_has_research_plus_arbitration() {
        let full = crate::tools::builtin_tools();
        // Sanity: the research + arbitration tools actually exist in
        // builtin_tools (otherwise the whitelist is a silent no-op and the
        // test would pass vacuously).
        let src_names: Vec<&str> = full.iter().map(|t| t.name.as_str()).collect();
        for needed in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_fetch",
            NOMINATE_SPEAKER_TOOL_NAME,
            END_DISCUSSION_TOOL_NAME,
        ] {
            assert!(
                src_names.contains(&needed),
                "builtin_tools must include {needed}"
            );
        }

        let mod_tools = group_chat_tool_defs(&full, true);
        let m_names: Vec<&str> = mod_tools.iter().map(|t| t.name.as_str()).collect();

        // Has the research whitelist + arbitration.
        for required in [
            "read_file",
            "grep",
            "glob",
            "list_dir",
            "web_fetch",
            NOMINATE_SPEAKER_TOOL_NAME,
            END_DISCUSSION_TOOL_NAME,
        ] {
            assert!(
                m_names.contains(&required),
                "moderator must see {required}, got: {m_names:?}"
            );
        }
        // Does NOT contain the abuse-prone / irrelevant tools (the leak that
        // caused 8be4687f seq 9). Enumerate the full deny-set so a future
        // builtin_tools addition is caught here if someone wrongly whitelists it.
        for forbidden in [
            "use_skill",
            "update_checklist",
            "shell",
            "write_file",
            "edit_file",
            "run_background_shell",
            "shell_status",
            "shell_kill",
            "merge_worker",
            "discard_worker",
            "remember",
            "ask_user_question",
            "use_ui",
            "request_mode_change",
            "request_task_state_transition",
            "create_task",
        ] {
            assert!(
                !m_names.contains(&forbidden),
                "moderator must NOT see {forbidden}, got: {m_names:?}"
            );
        }
    }

    /// R1: the participant's tool set is the research whitelist ONLY — no
    /// arbitration tools (only the moderator arbitrates) AND none of the
    /// abuse-prone / write / execute / interaction tools. This is stricter
    /// than the old `participant_tool_defs` blacklist, which only stripped
    /// the two arbitration tools.
    #[test]
    fn group_chat_tool_defs_participant_has_research_only() {
        let full = crate::tools::builtin_tools();
        let p_tools = group_chat_tool_defs(&full, false);
        let p_names: Vec<&str> = p_tools.iter().map(|t| t.name.as_str()).collect();

        // Has exactly the research whitelist.
        for required in ["read_file", "grep", "glob", "list_dir", "web_fetch"] {
            assert!(
                p_names.contains(&required),
                "participant must see {required}, got: {p_names:?}"
            );
        }
        // No arbitration tools (the original participant_tool_defs guarantee,
        // now achieved via whitelist instead of blacklist).
        for arb in [NOMINATE_SPEAKER_TOOL_NAME, END_DISCUSSION_TOOL_NAME] {
            assert!(
                !p_names.contains(&arb),
                "participant must NOT see {arb}, got: {p_names:?}"
            );
        }
        // No abuse-prone / write / execute / interaction tools.
        for forbidden in [
            "use_skill",
            "update_checklist",
            "shell",
            "write_file",
            "edit_file",
            "run_background_shell",
            "shell_status",
            "shell_kill",
            "remember",
            "ask_user_question",
        ] {
            assert!(
                !p_names.contains(&forbidden),
                "participant must NOT see {forbidden}, got: {p_names:?}"
            );
        }
    }

    /// Regression (08-04 follow-up, user decision "例子移除@，在和别人交流时允许@别人"):
    /// the participant identity-guard block must (a) NOT showcase an `@`-prefix
    /// example (naming `@moderator:` self-primed the models into writing
    /// `@moderator:` / `@M3:` self-labels — DB sessions 2bbc0d55 / 7bb0c351 show
    /// `@M3:  @M3:  @D4F`-style noise accumulating), (b) forbid starting a reply
    /// with your OWN name/role, and (c) explicitly ALLOW @-mentioning another
    /// participant in the body.
    #[test]
    fn participant_prompt_forbids_self_label_but_allows_mentions() {
        let p = participant_system_prompt("M3", None);
        // (a) no `@`-prefixed example in the guard block (it self-primes).
        assert!(
            !p.contains("@moderator:"),
            "guard block must not showcase an @-prefix example: {p:?}"
        );
        // (b) never start with your OWN name/role. (Line-continuation
        // `\n\` breaks "with your OWN" across a newline in the string —
        // match the two fragments separately.)
        assert!(
            p.contains("never start your reply with") && p.contains("your OWN name or role"),
            "must forbid self-label prefixes: {p:?}"
        );
        // (c) @-mentioning another participant in the body is allowed.
        assert!(
            p.contains("without an @ (e.g. \"@D4F，你说得对…\"") || p.contains("@D4F，你说得对"),
            "must explicitly allow @-mentioning others in the body: {p:?}"
        );
    }

    /// The moderator prompt must carry the same "no self-label prefix" rule.
    #[test]
    fn moderator_prompt_forbids_self_label() {
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "M3".to_string(),
                model: "m3".to_string(),
                persona_md: None,
            }],
            moderator_model_id: "mod".to_string(),
        };
        let p = moderator_system_prompt(&ctx);
        assert!(
            p.contains("never start your reply with") && p.contains("your OWN name or role"),
            "moderator prompt must forbid self-label prefixes: {p:?}"
        );
        assert!(
            !p.contains("@moderator:"),
            "moderator prompt must not showcase an @-prefix example: {p:?}"
        );
    }

    // -------------------------------------------------------------------
    // R3 (08-07-group-chat-toolset-and-identity): prompt pacing/takeover
    // hardening. DB session 8be4687f showed the moderator researching for
    // many rounds without nominating (seq 1/3/5) and a participant (M3,
    // seq 9) hijacking the host role via update_checklist + a hallucinated
    // skill. R3 adds explicit pacing guidance to the moderator prompt and
    // an explicit anti-takeover block to the participant prompt.
    // -------------------------------------------------------------------

    /// R3: the moderator prompt must guide "research → nominate" pacing and
    /// forbid building a self-rolled speaker-rotation flow (the seq 1/3/5
    /// over-research + the seq-12-pre "want to use update_checklist" failure
    /// modes).
    #[test]
    fn moderator_prompt_guides_research_to_nominate() {
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "M3".to_string(),
                model: "m3".to_string(),
                persona_md: None,
            }],
            moderator_model_id: "mod".to_string(),
        };
        let p = moderator_system_prompt(&ctx);
        // Pacing: research is allowed but bounded — must hand the floor.
        assert!(
            p.contains("research is a MEANS") && p.contains("hand the floor"),
            "moderator prompt must frame research as means + require nominating: {p:?}"
        );
        // Boundary: nominate_speaker is the ONLY scheduling mechanism; no
        // self-built rotation flow.
        assert!(
            p.contains("ONLY scheduling mechanism") && p.contains("Do NOT use other"),
            "moderator prompt must forbid self-built speaker-rotation: {p:?}"
        );
    }

    /// R3: the participant prompt must explicitly forbid taking over the
    /// moderator's job — the three failure modes from seq 9 (self-built
    /// checklist, addressing the room as host, inventing system tools/skills
    /// to legitimize hosting).
    #[test]
    fn participant_prompt_forbids_takeover() {
        let p = participant_system_prompt("M3", None);
        assert!(
            p.contains("must NOT step in as the host"),
            "participant prompt must forbid taking over the host role: {p:?}"
        );
        // No self-built speaker rotation / checklist (seq 9's update_checklist).
        assert!(
            p.contains("speaker-rotation or progress checklist"),
            "participant prompt must forbid self-built rotation/checklist: {p:?}"
        );
        // No inventing system tools/skills to legitimize hosting (seq 9's
        // hallucinated use_skill("group-chat-director")).
        assert!(
            p.contains("Do NOT invent or invoke system tools"),
            "participant prompt must forbid inventing tools/skills to host: {p:?}"
        );
    }

    /// R1 follow-up (08-07-group-chat-role-history-isolation): the
    /// participant prompt must EXPLICITLY allow + encourage codebase
    /// research (read_file / grep / glob / list_dir / web_fetch are in
    /// the participant's tool whitelist, but the pre-follow-up prompt
    /// never mentioned them — combined with role_history stripping all
    /// other-speaker tool pairs, participants stopped using tools
    /// entirely; DB session 6c00f286: zero participant tool calls vs
    /// 4/5 earlier sessions with participant tool use).
    #[test]
    fn participant_prompt_encourages_research() {
        let p = participant_system_prompt("M3", None);
        assert!(
            p.contains("You MAY research the codebase"),
            "participant prompt must allow research: {p:?}"
        );
        assert!(
            p.contains("read_file / grep / glob / list_dir / web_fetch"),
            "participant prompt must name the research whitelist: {p:?}"
        );
        assert!(
            p.contains("research is a MEANS, not the goal"),
            "participant prompt must bound research (brief look then speak): {p:?}"
        );
        // The allowance must NOT leak the arbitration tools.
        assert!(
            !p.contains("nominate_speaker / end_discussion are available"),
            "arbitration tools must stay moderator-only: {p:?}"
        );
        assert!(
            p.contains("moderator-only"),
            "prompt must state arbitration tools are moderator-only: {p:?}"
        );
    }

    // -------------------------------------------------------------------
    // role_history unit tests (08-07-group-chat-role-history-isolation
    // design §R1 + llm-contract.md §Pair Atomicity) — replaces the 08-04
    // participant_view suite. Each test locks one rewrite rule of the
    // per-role isolation assembler.
    // -------------------------------------------------------------------

    fn user_text(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            speaker: None,
        }
    }

    fn assistant_blocks_speaker(blocks: Vec<ContentBlock>, speaker: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
            speaker: Some(speaker.to_string()),
        }
    }

    fn tool_use(id: &str, name: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
        }
    }

    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "Floor handed to M1.".to_string(),
            is_error: false,
        }
    }

    /// R1: the current role's own assistant rows are preserved VERBATIM
    /// — `role:assistant` + ALL blocks, including Thinking + signature
    /// (Anthropic round-trip contract, AC1/AC5).
    #[test]
    fn role_history_current_role_assistant_verbatim() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "my reasoning".to_string(),
                        signature: "sig-own".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M1 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人发言".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3);
        assert_eq!(history[0], full[0], "human prompt unchanged");
        let own = &history[1];
        assert_eq!(own.role, Role::Assistant, "own row stays assistant");
        assert_eq!(own.speaker.as_deref(), Some("M1"));
        assert_eq!(own.content, full[1].content, "own blocks verbatim");
        assert!(
            matches!(
                &own.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::Thinking { signature, .. } if signature == "sig-own"
                    ))
            ),
            "own Thinking block + signature must survive: {:?}",
            own.content
        );
        // The other speaker (moderator) is rewritten to user.
        assert_eq!(history[2].role, Role::User);
        assert_eq!(history[2].speaker.as_deref(), Some("moderator"));
    }

    /// R1 + P0-2 (评审修订): another speaker's assistant row is rewritten
    /// to `role:user` with the content stripped to a single Text block —
    /// NO `@` prefix (the wire layer adds it) — and the `speaker` field
    /// preserved for wire attribution. Thinking blocks never survive.
    #[test]
    fn role_history_other_speaker_rewritten_as_user() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 2);
        let rewritten = &history[1];
        assert_eq!(
            rewritten.role,
            Role::User,
            "other speaker never stays assistant"
        );
        assert_eq!(
            rewritten.content,
            MessageContent::Text("主持人发言".to_string()),
            "single Text block, verbatim text — no @ prefix, no Thinking"
        );
        assert_eq!(
            rewritten.speaker.as_deref(),
            Some("moderator"),
            "speaker field preserved for the wire layer's attribution"
        );
        assert!(
            !rewritten.content.to_text().contains('@'),
            "content must NOT carry the @ prefix (apply_speaker_prefix would double it)"
        );
    }

    /// R1: other speakers' Thinking / RedactedThinking blocks never enter
    /// the current role's context — they carry signatures bound to THEIR
    /// generation context (re-injecting would break the signature
    /// round-trip or be echoed as if this role produced them).
    #[test]
    fn role_history_other_thinking_dropped() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::RedactedThinking {
                        data: "opaque".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "other participant reasoning".to_string(),
                        signature: "sig-other".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M2 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M2",
            ),
        ];

        let history = role_history(&full, "M1");

        assert!(
            !history.iter().any(|m| matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
                    ))
            )),
            "no other speaker's thinking may survive: {:?}",
            history
        );
        assert_eq!(history[1].content.to_text(), "主持人发言");
        assert_eq!(history[2].content.to_text(), "M2 发言");
    }

    /// R3: another speaker's tool pair (assistant ToolUse + its user
    /// tool_result row) is stripped WHOLE — tool results are NOT shared,
    /// only the text remark is relayed. No orphan survives.
    #[test]
    fn role_history_other_tool_pair_dropped() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "看下文件".to_string(),
                        cache_control: None,
                    },
                    tool_use("r1", "read_file"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("r1")]),
                speaker: None,
            },
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 发言".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3, "tool pair dropped, text remarks kept");
        let text: Vec<String> = history.iter().map(|m| m.content.to_text()).collect();
        assert_eq!(text, vec!["topic", "看下文件", "M1 发言"]);
        assert!(
            !history.iter().any(|m| matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                    ))
            )),
            "other speaker's tool pair must be fully stripped: {:?}",
            history
        );
        assert!(no_orphan_pairs(&history));
    }

    /// R3: the CURRENT role's own tool pair is preserved verbatim
    /// (role pairing stays legal — OpenAI rejects a tool_result without
    /// its tool_use with HTTP 400).
    #[test]
    fn role_history_own_tool_pair_preserved() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "看下文件".to_string(),
                        cache_control: None,
                    },
                    tool_use("r1", "read_file"),
                ],
                "M1",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("r1")]),
                speaker: None,
            },
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3);
        assert_eq!(history[1], full[1], "own tool_use row verbatim");
        assert_eq!(history[2], full[2], "own tool_result row kept");
        assert!(no_orphan_pairs(&history));
    }

    /// R1: the human's original prompt (`speaker == None`) is preserved
    /// as `role:user` — the discussion topic is never lost.
    #[test]
    fn role_history_human_prompt_preserved() {
        let full = vec![user_text("聊聊这个项目")];
        let history = role_history(&full, "M1");
        assert_eq!(history, full);
        assert_eq!(history[0].role, Role::User);
        assert_eq!(history[0].speaker, None);
    }

    /// R1 (08-04 invariant carried over): the moderator's arbitration
    /// pair (nominate/end) never reaches a participant — showing it made
    /// participants conclude they WERE the moderator (DB session
    /// evidence, research/db-evidence.md §2). Here the pair is stripped
    /// via the generic "other speaker's tool pair" path; the moderator's
    /// text remark is still relayed.
    #[test]
    fn role_history_moderator_arbitration_dropped_for_participant() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M1".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 发言".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        assert_eq!(history.len(), 3, "arbitration pair dropped");
        assert_eq!(history[1].content.to_text(), "先请 M1");
        assert_eq!(history[1].role, Role::User);
        assert!(
            !has_arbitration_blocks(&history),
            "no arbitration may survive in the participant history"
        );
        assert!(no_orphan_pairs(&history));
    }

    /// R1: the moderator keeps its OWN arbitration history (it must know
    /// who it has already nominated — 跨轮连贯, AC5). The pair is its
    /// own speaker, so it passes through verbatim.
    #[test]
    fn role_history_moderator_keeps_own_arbitration() {
        let full = vec![
            user_text("hello"),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M1".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
        ];

        let history = role_history(&full, "moderator");

        assert_eq!(history.len(), 3, "own pair preserved verbatim");
        assert_eq!(history[1], full[1]);
        assert_eq!(history[2], full[2]);
        assert!(no_orphan_pairs(&history));
    }

    /// R1: a participant speaking multiple times keeps ALL its own
    /// assistant rows (every turn where speaker == current_role) while
    /// other speakers' rows rewrite to user — the isolation must not
    /// collapse into "keep only the latest turn".
    #[test]
    fn role_history_multiturn_same_role_preserved() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 第一轮".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人插话".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "M1 第二轮".to_string(),
                    cache_control: None,
                }],
                "M1",
            ),
        ];

        let history = role_history(&full, "M1");

        let assistant_texts: Vec<String> = history
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .map(|m| m.content.to_text())
            .collect();
        assert_eq!(
            assistant_texts,
            vec!["M1 第一轮".to_string(), "M1 第二轮".to_string()],
            "all own turns preserved as assistant"
        );
        let user_speakers: Vec<&str> = history
            .iter()
            .filter(|m| m.role == Role::User)
            .filter_map(|m| m.speaker.as_deref())
            .collect();
        assert_eq!(
            user_speakers,
            vec!["moderator"],
            "moderator remark rewritten to user"
        );
    }

    /// AC5 contract-level lock: the current role's Thinking block
    /// signature must round-trip COMPLETE through the assembler (the
    /// Anthropic API 400s on a dropped/mutated signature), and other
    /// speakers' signatures must NOT leak into this role's context.
    #[test]
    fn role_history_signature_roundtrip_contract() {
        let full = vec![
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "reasoning".to_string(),
                        signature: "sig-abc-123".to_string(),
                    },
                    ContentBlock::Text {
                        text: "M1 发言".to_string(),
                        cache_control: None,
                    },
                ],
                "M1",
            ),
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Thinking {
                        thinking: "moderator reasoning".to_string(),
                        signature: "sig-mod".to_string(),
                    },
                    ContentBlock::Text {
                        text: "主持人发言".to_string(),
                        cache_control: None,
                    },
                ],
                "moderator",
            ),
        ];

        let history = role_history(&full, "M1");

        let signatures: Vec<String> = history
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::Blocks(blocks) => blocks.iter().find_map(|b| match b {
                    ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(
            signatures,
            vec!["sig-abc-123".to_string()],
            "own signature round-trips verbatim; the moderator's must not leak"
        );
    }

    /// P0-2 (评审修订): 归属策略方案 (a) —— 改写行 content 不带 `@` 前缀、
    /// 保留 speaker。经 wire 层序列化后,Anthropic `apply_speaker_prefix`
    /// 恰好插入一次 `@name: `(无双重前缀);OpenAI 侧 speaker 经 wire 转换
    /// 保留(适配器填 `name` 字段)。
    #[test]
    fn role_history_wire_no_double_prefix() {
        let full = vec![
            user_text("topic"),
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "主持人发言".to_string(),
                    cache_control: None,
                }],
                "moderator",
            ),
        ];
        let history = role_history(&full, "M1");
        let rewritten = &history[1];
        assert_eq!(rewritten.role, Role::User);
        assert_eq!(rewritten.speaker.as_deref(), Some("moderator"));
        assert_eq!(
            rewritten.content.to_text(),
            "主持人发言",
            "no @ prefix in the rewrite row"
        );

        // Anthropic wire: serialize the history the way the provider
        // body is built (ChatMessage JSON carries `speaker`), then run
        // the attribution pass.
        let mut body = serde_json::json!({
            "messages": history
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>()
        });
        crate::llm::provider::anthropic::apply_speaker_prefix(&mut body);
        let wire_text = body["messages"][1]["content"].as_str().unwrap();
        assert!(
            wire_text.starts_with("@moderator: "),
            "Anthropic wire must attribute the rewrite row once: {wire_text:?}"
        );
        assert_eq!(
            wire_text.matches("@moderator:").count(),
            1,
            "no double @ prefix: {wire_text:?}"
        );
        assert!(
            body["messages"][1].get("speaker").is_none(),
            "apply_speaker_prefix must strip the speaker field from the wire body"
        );

        // OpenAI wire: the rewritten row converts to a user message that
        // still carries `speaker` (the adapter emits the native `name`
        // field from it) with the bare text (no @ prefix).
        let wire = crate::llm::provider::wire::chat_request_to_wire(
            crate::llm::types::ChatRequest {
                model: "m".to_string(),
                max_tokens: 1000,
                messages: history.clone(),
                system: None,
                stream: false,
                tools: vec![],
                thinking: None,
            },
            None,
        );
        match &wire.messages[1] {
            crate::llm::provider::wire::WireMessage::User { content, speaker } => {
                assert_eq!(content, "主持人发言");
                assert_eq!(speaker.as_deref(), Some("moderator"));
            }
            other => panic!("rewritten row must map to WireMessage::User, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // R1 (08-07-group-chat-review-fixes): identity-correctness contract
    // baseline. These tests are the AUTOMATED regression floor for the
    // self-awareness dimension — they do NOT run a real model, so they
    // cannot prove a weak model won't role-collapse at runtime. What
    // they DO prove: the structural invariants that any prompt-level
    // defense relies on (per-role isolated transcript history + an
    // unambiguous role-boundary system prompt) hold even under the
    // worst input we've actually seen in production (DB sessions
    // a6c87247 / b144cc2a: same-model combination + a weak model that
    // prefixes its reply with another participant's name). If someone
    // later weakens `role_history` or strips the identity-guard
    // block from the prompt, these fail immediately.
    // -----------------------------------------------------------------

    /// P1-1 (08-07-group-chat-role-history-isolation 评审,语义重写):
    /// the 08-04 `participant_view` structural contract ("the view never
    /// rewrites content — mislabeled rows pass through verbatim") is
    /// REPLACED by `role_history`'s per-speaker attribution contract: a
    /// mislabeled assistant row (`speaker="M3"` but content reads as
    /// D4F) is rewritten to `role:user + speaker=M3` with the TEXT
    /// unchanged (role_history reassigns role/speaker and drops
    /// thinking — it does not sanitize text). Arbitration stripping +
    /// pair atomicity invariants carry over unchanged.
    #[test]
    fn identity_contract_view_holds_under_same_model_and_mislabel() {
        // Worst-case `full`: the moderator's arbitration pair + an
        // assistant row whose speaker is "M3" but whose CONTENT reads
        // as if D4F is speaking (the role-collapse signature from DB
        // session a6c87247 seq 16).
        let full = vec![
            user_text("聊聊这个项目"),
            // Moderator arbitration pair — must strip atomically (now
            // via the generic "other speaker's tool pair" path).
            assistant_blocks_speaker(
                vec![
                    ContentBlock::Text {
                        text: "先请 M3".to_string(),
                        cache_control: None,
                    },
                    tool_use("c1", "nominate_speaker"),
                ],
                "moderator",
            ),
            ChatMessage {
                role: Role::User,
                content: MessageContent::Blocks(vec![tool_result("c1")]),
                speaker: None,
            },
            // The weak-model mislabel row: speaker claims M3 but content
            // reads as D4F speaking.
            assistant_blocks_speaker(
                vec![ContentBlock::Text {
                    text: "@D4F: 接过 M3 留的钩子…".to_string(),
                    cache_control: None,
                }],
                "M3",
            ),
        ];

        let view = role_history(&full, "M1");

        // Structural invariant 1: zero arbitration blocks survive.
        let c1_survives = view.iter().any(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks.iter().any(
                |b| matches!(b, ContentBlock::ToolUse { id, name, .. }
                    if (name == NOMINATE_SPEAKER_TOOL_NAME || name == END_DISCUSSION_TOOL_NAME)
                        && id == "c1")
                    || matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "c1"),
            ),
            _ => false,
        });
        assert!(
            !c1_survives,
            "arbitration pair c1 must be fully stripped: {view:?}"
        );
        // Structural invariant 2: pair atomicity (no orphan).
        assert!(
            no_orphan_pairs(&view),
            "no orphan tool_use/result: {view:?}"
        );
        // Structural invariant 3 (P1-1 语义重写): the mislabeled row is
        // RE-ATTRIBUTED, not passed through as assistant — it becomes
        // role:user + speaker=M3, text byte-identical (no sanitization:
        // role_history only reassigns role/speaker and drops thinking).
        let mislabeled = view
            .iter()
            .find(|m| m.speaker.as_deref() == Some("M3"))
            .expect("mislabeled row must survive re-attribution");
        assert_eq!(
            mislabeled.role,
            Role::User,
            "mislabeled row must NOT survive as assistant (per-role isolation)"
        );
        assert_eq!(
            mislabeled.content.to_text(),
            "@D4F: 接过 M3 留的钩子…",
            "text is NOT sanitized — only role/speaker are reassigned"
        );
    }

    /// R1: under a same-model combination (moderator and a participant
    /// share the same model_id — the hardest identity case, mandated
    /// supported by PRD D2), the system prompts still draw an
    /// unambiguous role boundary. The participant prompt must assert
    /// it is the participant + forbid adopting the moderator's voice;
    /// the moderator prompt must list the participant in the roster +
    /// not contain the participant's identity-guard wording. This is
    /// the prompt-level structural floor that the view test above
    /// depends on.
    #[test]
    fn identity_contract_prompts_separate_roles_under_same_model() {
        // Same-model combination: moderator_model_id == participant.model.
        // The hardest identity case (PRD D2 mandates it supported).
        let same_model = "deepseek-v4-flash";
        let ctx = GroupChatCtx {
            participants: vec![crate::agent::group_chat::ParticipantConfig {
                name: "D4F".to_string(),
                model: same_model.to_string(),
                persona_md: None,
            }],
            moderator_model_id: same_model.to_string(),
        };

        let participant_prompt = participant_system_prompt("D4F", None);
        let moderator_prompt = moderator_system_prompt(&ctx);

        // Participant prompt: asserts it is D4F + the identity guard.
        assert!(
            participant_prompt.contains("You are D4F"),
            "participant prompt must name the participant: {participant_prompt:?}"
        );
        assert!(
            participant_prompt.contains("The moderator's messages are NOT yours"),
            "participant prompt must carry the role-boundary guard: {participant_prompt:?}"
        );
        assert!(
            participant_prompt.contains("never start your reply with")
                && participant_prompt.contains("your OWN name or role"),
            "participant prompt must forbid self-label prefixes: {participant_prompt:?}"
        );

        // Moderator prompt: lists D4F in the roster + the moderator's
        // own no-self-label rule. Must NOT carry the participant's
        // "you are D4F" wording (would be a role leak).
        assert!(
            moderator_prompt.contains("D4F"),
            "moderator prompt must list the participant: {moderator_prompt:?}"
        );
        assert!(
            moderator_prompt.contains("You are the MODERATOR"),
            "moderator prompt must assert the moderator role: {moderator_prompt:?}"
        );
        assert!(
            !moderator_prompt.contains("You are D4F"),
            "moderator prompt must NOT carry the participant's identity wording (role leak): {moderator_prompt:?}"
        );
    }

    fn has_arbitration_blocks(msgs: &[ChatMessage]) -> bool {
        msgs.iter().any(|m| {
            matches!(
                &m.content,
                MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(
                        b,
                        ContentBlock::ToolUse { name, .. }
                            if name == NOMINATE_SPEAKER_TOOL_NAME || name == END_DISCUSSION_TOOL_NAME
                    ) || matches!(b, ContentBlock::ToolResult { .. }))
            )
        })
    }

    /// llm-contract.md §Pair Atomicity check: every ToolUse has a
    /// matching ToolResult and vice versa (within the message list).
    fn no_orphan_pairs(msgs: &[ChatMessage]) -> bool {
        let mut use_ids = Vec::new();
        let mut result_ids = Vec::new();
        for m in msgs {
            if let MessageContent::Blocks(blocks) = &m.content {
                for b in blocks {
                    match b {
                        ContentBlock::ToolUse { id, .. } => use_ids.push(id.clone()),
                        ContentBlock::ToolResult { tool_use_id, .. } => {
                            result_ids.push(tool_use_id.clone())
                        }
                        _ => {}
                    }
                }
            }
        }
        use_ids.iter().all(|id| result_ids.contains(id))
            && result_ids.iter().all(|id| use_ids.contains(id))
    }
}
