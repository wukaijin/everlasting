# handoff 跨 session 接力 — 设计评审(2026-08-19)

评审对象:prd.md / design.md / implement.md(task 状态 planning,未进 implement)。
方法:逐条核对 design 引用的代码锚点与既有契约(compaction / sessions / session_crud / resource_loader / 前端 slashCommand+ChatInput+MessageItem / daemon routes)。

## 结论

**整体质量高,可开工。** 勘察真实、锚点准确、关键坑(水位移植、auto-title 抢注、全量覆盖语义)识别到位,与 /compact 的差异表述清楚。但有 **2 个实现层面问题建议先补进 design.md 再动工**(其中 1 个 bug 级,照字面实现会丢信息);另 4 处轻微表述/机制问题顺手写清即可。

---

## 已核实属实(抽样)

- **生成侧**:`run_manual_compaction`(agent/compaction.rs:889)、prompt 六段含 Work State / Optional Next Step(:596-609)、`send_summary_completion`(:775)、`latest_summary_anchor`(:748)、4k clamp、熔断记账(record_success/failure :720-727)——全部存在且签名吻合。
- **水位语义**:`apply_compaction_watermark`(:199)只认 `kind == "compaction_summary"` 的行——**handoff_summary 不参与水位成立**,design §4 绕开移植坑的方案成立。`latest_summary_anchor` 的 content 来自 text 列(纯摘要,prefix 不落库)——与 /compact 的差异(handoff 需 prefix 落库)确认真实存在。
- **gate 链**:`compact_session_inner`(commands/sessions.rs:1063)群聊拒绝(:1078)/ `llm_compaction_enabled`(:1083)/ in-flight(:1103)/ `lookup_provider_for_session`(:1108)/ 错误映射(:1126-1134)——逐条存在,行号引用准确。
- **落库层**:`insert_compaction_summary`(session_crud.rs:767)content=JSON blocks / text=纯文本双列契约;auto-title 抢注(messages.rs:86-93,title='新对话' 才替换);rename 80 字符截断(session_crud.rs:553);`subagent_runs.parent_session_id` 真列+FK(subagent_runs.rs:31)。
- **前端**:slashCommand 直输拦截(matchBuiltinCommandInput)、ChatInput `executeBuiltin` compact case(:528)与 palette focus 提取(:572)、MessageItem 已有 compaction_summary 卡片外壳(stores/chat.ts:249/335)、chatStore `switchSession` / `reloadSessionMessages`(:530/534)——全部在位。
- **差异点**:`compute_preservation_region` 保留区 / `NothingToCompress` 语义确认;design "全量覆盖 vs 保留区" 的核心差异成立。

---

## 问题清单

### ① [bug 级] `build_compaction_prompt` 的 anchor-skip 契约,design §3.2 未覆盖

compaction.rs:537-543:当 `prior.is_some()` 时,prompt builder **跳过 `compressible[0]`**(视为 prior 占位,内容已由 `<prior-summary>` 块进入 prompt)。manual 路径为此构造 `compressible = [anchor_msg(prior)] + candidates`(compaction.rs:916-926),跳的是占位,无损。

design §3.2 只写"把全部 regular 行作为 compressible 传给 prompt builder"——照字面实现,prior 存在时**第一条 regular 行会被当 anchor 静默跳过**,恰好丢"全量覆盖"最想保住的最新状态。

**修法**:run_handoff 复刻 manual 的 anchor_msg 占位构造(prior 存在时 compressible[0] = anchor_msg(prior),跳过它无损)。建议写进 design §3.2。

### ② [机制缺口] `create_session` 不继承任何东西,"直接调用"不可行

`db::create_session`(session_crud.rs:29-130)**硬编码**:title='新对话'、worktree_path=NULL / worktree_state='none' / last_worktree_path=NULL、workflow_enabled=0、plugin_name='dev'、mode='edit'。design §3.6 要继承 mode/plugin_name/workflow_enabled/worktree 三列,但该函数不接受这些参数。

实现只能二选一:
- 扩展共享签名(违背 implement.md "纯新增不改旧函数" 承诺,且动 create_session 波及面大);
- **create 后 UPDATE**:复用 `set_worktree_state`(session_crud.rs:520)、`set_session_workflow_enabled` / `set_session_plugin_name`(同文件,rename_session 旁)、`rename_session`(:553)设标题——纯新增调用,不破坏既有契约。

**建议**:design §3.6 明确走"create 后 UPDATE"路线;implement.md ② 补上对应步骤(标题可并入 rename_session 调用)。

### ③ [轻微] daemon 路由风格与先例不一致

现有路由扁平 `POST /api/v1/sessions/<command>`(id 在 body),唯一 path-param 是先例 GET `/:id/snapshot`(daemon/routes/sessions.rs:371)。`POST /sessions/:id/handoff`(PRD R7 已批)要么跟 compact 走 body(`/handoff`,session_id 在请求体),要么 handler 从 path 取 id。两者皆可;path 形式与 snapshot 一致,若要最贴近 compact 先例则 body 形式。与 compact 保持同构可减少一处 handler 差异。

### ④ [表述] "content 与 text 两列同值"不精确

insert_compaction_summary 的 content 是 `[{type:"text",text:...}]` JSON block 数组、text 是纯文本,并非字面同值。design §3.7 应写明 content 沿用 JSON block 形态(否则 rehydrate 解析踩坑),语义是"**两列都含完整 prefix+摘要**"(而非字节级相同)。

### ⑤ [细节] validate 段名匹配

prompt 第六段实际标题为 **"Optional Next Step"**(compaction.rs:608),PRD/design 写 "Next Step"。validator 需子串容忍(design §3.5 已写"容忍编号/井号前缀",建议同口径放宽为子串匹配,如 `contains("Next Step")`),design 里明确即可。

### ⑥ [低] parent.metadata.handoff_children 读-改-写非原子

双并发 handoff 可能 clobber 列表(同 softcap 的 metadata 合并问题)。用户驱动低频,可接受;若顺手,用 SQLite `json_set` 单条 UPDATE 原子合并(仅动 sessions.metadata 列,不改 messages)。

---

## 引用勘误(非阻塞)

- PRD 写的 `app/src/components/ChatInput.vue`、`chat.types.ts` 为旧路径;实际在 `app/src/components/chat/ChatInput.vue` 与 `app/src/stores/chat.types.ts`。implement.md 未写死路径,无碍。
- check.jsonl / implement.jsonl 仍为 `_example` 占位(task 停 planning、未进 implement 阶段,与 git 状态一致)。

## 建议行动

开工前补 design.md:
1. §3.2 加 anchor_msg 占位构造(问题①);
2. §3.6 定 "create 后 UPDATE" 继承机制,implement.md ② 同步(问题②);
3. §3.7 改双列表述、§3.5 明确 Next Step 子串匹配(问题④⑤);
4. 路由形式与 compact 对齐与否明确结论(问题③),⑥ 可选。
