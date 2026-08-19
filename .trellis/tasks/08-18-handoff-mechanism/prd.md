# handoff 跨 session 接力

## Goal

长任务跨 session 接续:把当前会话的压缩产物(摘要 + 状态)接力到新 session 继续跑——"压缩的落点变成新会话起点"而非原地收窗。与 /compact 共享 90% 管道(摘要生成 / 增量合并),是同一管道的第二个落点。

## Background / 前置

- 父任务(已归档):`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`——摘要 prompt(handoff 话术已在模板)、`build_compaction_prompt`、`SummaryAnchor` 增量合并现成。
- 姊妹任务(均已归档):`08-18-manual-compact-command`(手动 /compact 空闲期编排先例)、`08-18-max-turns-softcap`(软卡 force 穿参先例)。PRD Notes 的排序前置已满足。
- R4 用户驱动为主:MVP 不做自动接力;原始会话保留(落库无损原则沿用)。

## 确认事实(代码勘察,2026-08-19)

**生成侧全复用**:`run_manual_compaction`(app/src-tauri/src/agent/compaction.rs:889)——prompt 六段含 Work State / Next Step(:596-609)、`send_summary_completion`(:775,无 tools + retry_open + 熔断)、`latest_summary_anchor`(:748)、4k clamp;命令层 gate 链先例 `compact_session_inner`(src/commands/sessions.rs:1063:群聊拒绝/llm_compaction_enabled/in-flight/provider 查找)。摘要内容校验现状为无,R2 校验是新增。

**新 session 侧**:`create_session_inner`(src/commands/sessions.rs:41)必须带 project_id,metadata 可创建时写;worktree 后置 attach(`set_worktree_state`,src/db/sessions/session_crud.rs:520)。首条 context 机制 = `role='user'` + `messages.metadata.kind` 标记(先例 worktree_event / compaction_summary)。**水位移植坑**:`apply_compaction_watermark`(compaction.rs:199)要求 cutoff_seq 指向的行在同 session db_rows,旧摘要行直接移植会 AlignmentFailed fail-open。**auto-title 坑**:首条用户消息抢注标题(src/db/sessions/messages.rs:86-103),新 session 需显式设标题。

**parent 关联现状**:sessions 无 parent 列;唯一 session 间关联先例 `subagent_runs.parent_session_id`(真列+FK);`sessions.metadata` JSON 列类型无关(GroupChatConfig 唯一生产消费者),可整读整写。

**前端**:命令入口管道现成(matchBuiltinCommandInput app/src/utils/slashCommand.ts:45 + executeBuiltin ChatInput.vue:528-553 compact 先例 + reloadSessionMessages);resource_loader BUILTIN_COMMANDS(src/resource_loader.rs:105+)。

## Requirements(含已批决议 D1-D4,2026-08-19)

- **R1 接力动作**:`/handoff [focus]` 内置命令 + palette 分发(D1,与 /compact 同构,支持 focus);生成摘要复用自动路径含 prior 增量合并,**全量覆盖语义**(不切保留区,全部 regular 行进摘要——新 session 从摘要独立起步);新建 session 继承 project_id / current_cwd / model_id / mode / plugin_name / workflow_enabled / worktree 三列;成功后前端切到新会话,**等用户输入续跑,不自动起轮**(D2)。
- **R2 摘要质量**:Work State / Next Step 段校验非空;缺失 → 带纠正块静默重试一次 → 仍缺失明确报错,**零副作用不建 session**(D4);prior 快路径(无新 regular 行直接复用 prior 摘要)同样过校验,缺段退化 LLM 补段。
- **R3 来源可溯**:双向 metadata JSON(D3)——child.metadata.handoff.parent_session_id(+parent_title/focus),parent.metadata.handoff_children 列表(容多次接力,读-改-写不 clobber);SQLite json_extract 可查,审计样例进 spec。无 migration。
- **R4 用户驱动**:MVP 无自动触发、无自动首轮;原 session 不清空不归档。
- **R5 落库无损**:原 session messages 行数不变;仅 sessions.metadata 列追加 child 关联。
- **R6 摘要行契约(设计决议)**:新 kind `handoff_summary`,role='user',content(单 Text 块 JSON)与 text(纯文本)两列同载 SUMMARY_CONTEXT_PREFIX + 摘要(prefix 落库自包含),seq=1;**不参与水位**(apply_compaction_watermark 只认 compaction_summary),新 session 后续自动压缩把 handoff 行作 regular 行再摘要,链路自然延续;绕开移植坑。前端复用摘要卡片外壳 + "接力自 {parent_title}" 徽标 + 点击跳 parent。
- **R7 命令层与路由**:gate 链镜像 compact(群聊/llm_compaction_enabled/in-flight/provider);Tauri 命令 + daemon POST /handoff_session(body {session_id, focus},与 compact 同构)双注册(live 冒烟走 daemon)。

## Non-Goals

- 自动接力 / 自动首轮 / 资源自动回收;
- 软卡第四臂(入口贴合长任务场景,留 follow-up);
- 跨 project / 跨机器接力(远期 BACKLOG §4);
- 群聊 / worker 接力(MVP 主 loop 单聊);
- worktree 双 session 互斥(MVP 接受共享,spec 记边界)。

## Acceptance Criteria

- [ ] AC1:接力产出新 session,首条 context = SUMMARY_CONTEXT_PREFIX + 摘要,续跑一轮 wire 正确(mock 端到端);
- [ ] AC2:摘要含 Work State / Next Step——缺段重试成功路径 + 恒缺报错零副作用(无新 session 行)两路都测;
- [ ] AC3:parent 关联可查——child.metadata.handoff.parent_session_id 断言 + parent.metadata.handoff_children 含 child + json_extract 样例查询;
- [ ] AC4:原 session messages 行数接力前后不变;
- [ ] AC5:live 验证一次真实接力(turn-smoke.sh --handoff 模式,重编 daemon 后 PASS)。

## Notes

- 关联 spec:agent-loop-architecture(pattern-llm-compaction §handoff 回写)+ frontend chat spec(handoff 卡片)。
- 设计与实施细节见同目录 design.md / implement.md;验证命令速查在 implement.md。
