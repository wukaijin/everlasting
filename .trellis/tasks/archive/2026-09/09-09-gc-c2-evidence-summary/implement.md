# Implement — C2 证据链:群聊结构化 summary

> 执行顺序 = 依赖序:类型/解析 → 工具与状态 → 校验 → 落库 → 编排器接线 → prompt → 转录(Rust)→ 前端 → JS(M1/MCP)→ 文档 → 门禁 → live。每步末尾的可跑验证就地执行,最后 AC5 全门一遍。

## Phase 1 后端生产与落库

- [ ] 1.1 `agent/discussion_detail.rs`:serde 类型(DiscussionDetail/Conclusion/Anchor/Stance/AnchorCheck,snake_case wire)+ 宽容 `parse_from_input(&serde_json::Value) -> Option<DiscussionDetail>` + 校验纯核 `validate_anchor`(注入读)+ 薄壳 `validate_anchors(detail, root: Option<&Path>)`。单测:解析四形态(完整/部分缺省/垃圾 None/空 conclusions)+ 校验五臂(tempdir fixture:ok / not_found / line_out_of_range / outside_root 含目录穿越与绝对路径 / 无 root 全 unvalidated)+ IO 错误臂 + 16MiB 帽(小帽参数化测,不真造 16MiB 文件)。
- [ ] 1.2 `tools/end_discussion.rs`:`definition()` schema 增 conclusions/open_questions;`execute_intercept` 解析 → `st.end_detail`。`tools/nominate_speaker.rs` `GroupChatTurnState` 增 `end_detail: Option<DiscussionDetail>`;`group_chat_loop.rs:430` 初始化。
- [ ] 1.3 DB:`db/migrations/schema.rs` 加列;`finalize_group_chat_lifecycle` 增参 + COALESCE;`clear_group_chat_lifecycle` 增 `discussion_detail = NULL`;`SessionRow` 增字段 + SELECT + try_get。迁移/落库/复用清空三组单测(db 侧既有测试文件内)。
- [ ] 1.4 编排器接线(`group_chat_loop.rs:1086-1114`):take end_detail → validate(ctx.project_root)→ serialize → finalize;serialize/校验异常 warn 不 fail。
- [ ] 1.5 prompt 三处教学(`group_chat_prompts.rs` moderator_system_prompt / wrapup / resume)+ `tests_group_chat_prompts` 断言。
- [ ] 1.6 orchestrator E2E:升级既有剧本或新增——MockProvider moderator 发结构化 end_discussion(带 root 内真实临时文件锚点 + 越界锚点 + root 外锚点),断言行 detail 落库且 check 各臂正确;另一剧本只发 summary(零结构),断言 detail NULL(缺省零变更)。
- [ ] 1.7 Rust 转录:`group_chat_transcript.rs` args + render `## conclusions`/`## open_questions` 节(None/空省略);render 单测扩展;`group_chat_loop.rs:1131` 调用点传 typed detail。

验证:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md;新用例 + 基线 2343 全绿)+ `cargo clippy -p everlasting -- -D warnings` + `cargo fmt --check`。

## Phase 2 前端

- [ ] 2.1 `chat.types.ts`:DiscussionDetail 镜像类型 + 两处行类型加 `discussion_detail?`;`streamEvents.ts` 受控合并 + `streamRehydrate.ts` 同步。
- [ ] 2.2 `DiscussionSummaryCard.vue`:input 解析(live)+ store detail 叠加(收官后 path+line 匹配)+ stance 徽章/锚点行/check 记号/open_questions 渲染 + 无结构兜底;样式沿用卡片既有 token。
- [ ] 2.3 vitest:结构化四形态渲染 / check 叠加 / 坏 detail JSON 容错 / 无结构兜底;store 合并用例。

验证:`cd app && pnpm test` + `pnpm exec vue-tsc --noEmit`(以仓库实际命令为准)。

## Phase 3 JS 消费端(scripts/)

- [ ] 3.1 `group-chat-run.mjs` `renderTranscript`:detail JSON → conclusions/open_questions 节 + 坏 JSON 警告行;`group-chat-run.test.mjs` 用例。
- [ ] 3.2 `group-chat-mcp.mjs` `discussion_result`:输出加 `detail`(坏 JSON → null + detail_warning);`group-chat-mcp.test.mjs` 用例;确认 wire 预算锁断言仍绿(输入未动)。

验证:`node --test scripts/group-chat-run.test.mjs` + `node --test scripts/group-chat-mcp.test.mjs` + 冒烟 `node scripts/group-chat-mcp-smoke.mjs`(非 live)。

## Phase 4 文档与记账

- [ ] 4.1 DAEMON-API.md:§4 SessionRow 字段表 + §6 discussion_result 输出 detail;转录节说明。
- [ ] 4.2 ROADMAP §6 依赖矩阵 C2.1 划掉 + §0/§5 状态行补一句;GROUP-CHAT-API-ROADMAP 状态头更新。
- [ ] 4.3 spec 沉淀(trellis-update-spec,收尾):结构化 summary 契约(类型/wire/校验语义/复用清空红线)进 backend spec(群聊相关 pattern 文件)。

## Phase 5 门禁与 live

- [ ] 5.1 AC5 全门:两端全量测试 + clippy/fmt/vue-tsc + e2e(`cd app && pnpm test:e2e`,收官卡若已有 e2e 覆盖则确认未破)。
- [ ] 5.2 AC6 live:`node scripts/group-chat-run.mjs run --preset review --topic <小议题>` 一场,验收收官行 detail / 转录 conclusions 节 / 抽查 ≥2 锚点与仓库实际一致 / MCP `discussion_result` 读同场 detail。
- [ ] 5.3 收官:`python3 ./.trellis/scripts/task.py` 归档流程 + journal。

## 风险点与回滚

- 高风险文件:`group_chat_loop.rs`(退出块)、`session_crud.rs`(迁移/清空)——detail 清空漏改是复用场数据污染源,AC3 专项断言锁。
- 前端卡是纯增量组件逻辑,回滚独立。
- JS 两文件纯消费端,坏 JSON 全降级,无破坏面。
- 整体回滚 = additive 列无消费即休眠(见 design §8)。

## 边界确认(实现前自查)

- [ ] moderator prompt 教学不超预算(system prompt 增量 ≤15 行)。
- [ ] `end_detail` 不进 ChatEvent/wire 事件面(design §7 决策)。
- [ ] 讨论库面板零改动(R6/Q3 边界)。
