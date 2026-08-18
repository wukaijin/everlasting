# Implement — C3 摘要式上下文压缩

> 前置:prd.md(决议已批)+ design.md。3 PR,每 PR 独立可验证、可回滚(开关 `llm_compaction_enabled`)。

## PR1 — 水位替换地基(纯机械,零 LLM)

- [ ] `agent/compaction.rs` 新模块:`apply_compaction_watermark(wire, db_rows) -> Vec<ChatMessage>` 纯函数(design §3 算法:idx±1 重对齐 + watermark_miss trace + fail-open)。
- [ ] `db/sessions/`:metadata.kind 读取 helper(MessageRow 已有 metadata 列,只需 parse;注意消息插入代码在 `db/sessions/messages.rs`,**不是** `db/messages.rs` —— 评审 P2-3)。
- [ ] `chat_loop/init.rs` `prepare_loop_state`:B5 头对插入前接入;gate = `llm_compaction_enabled`(config,缺省 on fail-open)&& !worker && !群聊(`session_type` 判定);替换发生时产出 `SummaryAnchor` 种子 + `synthetic_prefix_len` 记录(供 PR2)。
- [ ] 单测:命中/未命中/对齐失败(含 idx±1 重对齐)/自愈(4+ 用例)。
- [ ] 验证:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md)+ 手工:预置一条 kind=compaction_summary 行,turn-smoke 确认请求 messages 被折叠(日志 or mock)。

**回滚点**:开关关 = 完全回到 main 行为。

## PR2 — 摘要生成 + 降级链 + 熔断

- [ ] `agent/compaction.rs`:`compute_preservation_region`(复用 `group_droppable_turns`,`synthetic_prefix_len` 起算,design §4.1)+ `build_compaction_prompt`(design §6 模板 + transcript 渲染截断 2k + **transcript 总预算 0.7×window 溢出丢最旧**)。
- [ ] `db/sessions/session_crud.rs`:新 `insert_compaction_summary`(仿 `insert_system_event`:689;**seq 吃 loop 游标插入并返回推进值,不走独立 MAX+1** —— 复核新增);**content 存纯摘要,前缀话术构建时加不落库**(评审 P1-2)。
- [ ] `agent/chat_loop/drive.rs` C3 块改造:触发线 0.85(含 helper 同步)→ gate(未熔断)→ 摘要调用(provider 现成 + `retry_open`,无 tools / 禁 thinking / **4k 输出上限**)→ 持久化 → 回填;**`SummaryAnchor` 进 `DriveTurnOutcome`** 循环内穿参(init 种子 + 压缩后更新,同 loop_hit_count 模式;同 loop 二次压缩也覆盖)。
- [ ] 降级链:失败 → `compact_messages` 机械丢组(保留原样,PROTECTED_HEAD=2 预存偏差不在本任务修);空待压区 → 直走机械;摘要后 > 0.95 窗 → 机械兜底 → StillOver 不变。
- [ ] 熔断:`CompactionRegistry`(AppState + LoopInit 穿参,同 stub_loaded 模式;`delete_session` 清理;3 次连续失败粘性跳过)。
- [ ] `CompactResult` + `compaction_json`(**trace.rs:57 手工 json! 三处联动**:Rust + record_compaction + TS event/streamController,评审 P3)+ `ChatEvent::ContextCompacted` 扩 `method` / `summary_usage`(serde default 向后兼容)。
- [ ] 集成测试(design §9 全部 MockProvider 用例,AC1/2/4/5/6/8;群聊 gate 测试建 GroupChat session 行)。

**回滚点**:PR2 出问题 → 开关关(PR1 水位替换同时停,见 design §10)。

## PR3 — 增量合并 + 前端最低渲染 + 观测收尾

- [ ] prior-summary 注入(`<SummaryAnchor>` → `<prior-summary>` 纯摘要 content + "conversation wins";anchor 不进 transcript 不重复喂)+ `prior_summary_seq` 落 metadata(AC3 测,跨请求与同 loop 两种二级压缩各一例)。
- [ ] 前端:`MessageItem` 对 `kind=compaction_summary` 最低渲染(低调系统样式行);streamController `ContextCompacted` 新字段透传;TracePanel TurnCard method 徽标。
- [ ] spec 更新:`agent-loop-architecture`(C3 关卡改写)+ `database-guidelines`(摘要行 metadata 契约)+ `token-usage-tracking`(summary_usage 口径)。
- [ ] 全量回归:cargo 全量 + `pnpm test` + `vue-tsc --noEmit`;turn-smoke 复验。
- [ ] ROADMAP §1.2 补记 + 决策日志条目。

**收尾**:`trellis-check` 全范围(final pass)→ commit → finish-work。

## 风险与注意

- **不要动**:`run_chat_loop` 24+ 参签名(摘要所需 provider/db/drive_turn 均已在作用域)、`compact_messages` 原语义(降级为 fallback 但行为不变)、RULE-A-001/002 不变量、memory 头对注入位置。
- **每 PR 跑**:`cargo fmt` + clippy(lefthook pre-commit 会拦)。
- **时序坑**:摘要行 insert 必须在回填内存**之前**成功(AC2 依赖);persist 走 turn 持久化同款 blocking 模式,避免与 loop 写竞争。
