# handoff 跨 session 接力 — 技术设计

决议依据:prd.md D1-D4(2026-08-19 用户批准,均采纳推荐项)。所有路径锚点以 app/src-tauri/ 与 app/src/ 为根。

> **实现偏差记录(2026-08-19,实现后回写)**:
> 1. **分层**:§1 草图的 `run_handoff` 全编排在实现中拆为两半——生成在
>    `agent/compaction.rs::generate_handoff_summary`(纯函数+熔断,无 DB
>    写),持久化+继承在 `commands/sessions.rs::persist_handoff_child`。
>    原因:mode 继承走 `set_session_mode_internal`(commands 层单一真源,
>    审计 + Yolo guard),agent 模块不得反向 import commands。
> 2. **trigger 取值**:接力行 metadata 用 `"handoff"` 非 `"manual"`
>    (与 /compact 摘要行区分,卡片徽标可辨)。
> 3. **parent metadata 合并基线现读 DB**(§3.8 原为"读-改-写"):两次
>    接力复用同一陈旧 parent 行会 clobber 上一次的 children——测试实证,
>    改为合并时 fresh load。
> 4. **快路径可达性**(§3.3):manual/auto 压缩必留保留区,正常流程到
>    不了"水位后无新常规行"态;快路径保留为 D3 自愈等边界的防御短路,
>    测试以直接构造 anchor 行覆盖。
> 5. **parent_title 补进行 metadata**(卡片徽标需要,原只在 session
>    metadata)。

## 1. 总体架构

```
/handoff [focus](直输拦截 / palette,与 /compact 同构)
  → 前端 invoke handoff_session({sessionId, focus})
  → handoff_session_inner 命令层(gate 链镜像 compact_session_inner)
  → run_handoff 编排(compaction.rs,新函数)
      1) prior = latest_summary_anchor(rows)          ← 复用:增量合并
      2) candidates = 全部 regular 行(prior 之后的;无保留区切分)
      3) 生成摘要(全量覆盖模式)+ D4 校验(Work State/Next Step)
      4) db::create_session(继承 parent 字段)
      5) insert_handoff_summary(kind=handoff_summary, seq=1)
      6) 双向 metadata 关联(child.parent / parent.children)
  → 返回 HandoffResult → 前端刷新列表 + 切到新会话 + 卡片渲染摘要
```

与 /compact 的关系:同一摘要管道的两个落点。生成侧(prompt / send_summary_completion / 熔断 / clamp)直接复用;落库侧从"插回同 session seq=MAX+1"换成"新 session 首行"。

## 2. 后端命令层

`handoff_session(session_id, focus: Option<String>) -> HandoffResult`

- 位置:`src/commands/sessions.rs`,新增 `handoff_session_inner`,紧邻 `compact_session_inner`(sessions.rs:1063)。
- gate 链逐条镜像:load_session → 群聊拒绝(:1075 同判)→ `llm_compaction_enabled`(:1083)→ in-flight 拒绝(:1095)→ `lookup_provider_for_session`(:1107)→ 错误映射(:1125)。in-flight 拒绝同时保证 parent 行集读取一致(空闲期快照)。
- 注册:Tauri 命令 `src/lib.rs`(compact 在 :287 旁);daemon 路由 `POST /handoff_session`(body `{session_id, focus}`,与 compact 完全同构——compact 先例是扁平 body 形式 `CompactSessionRequest`,routes/sessions.rs:352-363/388;不走 path-param),供 remote/live 冒烟。
- `HandoffResult { new_session_id, new_session_title, cutoff_seq, tokens_before, tokens_after, summary_usage, model }`——字段对齐 `ManualCompactionOutcome`(compaction.rs:845)加新 session 标识。

## 3. 编排:run_handoff(compaction.rs 新函数,~:889 run_manual_compaction 旁)

签名:`(db, session_id, provider, context_window, focus: Option<&str>, rows: &[MessageRow]) -> Result<HandoffOutcome, HandoffError>`

1. **prior 增量合并**:`latest_summary_anchor(rows)`(compaction.rs:748),与 manual 同源。
2. **全量覆盖语义(与 compact 的关键差异)**:compact 用 `compute_preservation_region` 只压头部、保留尾部;handoff 把全部 regular 行(prior.cutoff 之后)作为 compressible 传给 `build_compaction_prompt`——不切保留区,因为新 session 从摘要独立起步,尾部信息必须进摘要。**anchor 占位契约(评审①,bug 级)**:prior 存在时必须复刻 manual 的占位构造 `compressible = [anchor_msg(prior)] + candidates`(compaction.rs:922-930)——builder 对 prior.is_some() 跳过 `compressible[0]` 视为占位(:543-547),不构造则 prior.cutoff 之后最旧一条 regular 行被静默丢出 transcript;`tokens_before` 视图构造同步镜像(:938)。transcript 预算(0.7×window oldest-drop)仍由 prompt builder 内部兜底。
3. **快路径**:prior 存在且 `seq > prior.cutoff` 的 regular 行为 0 → 直接以 prior.content 为接力摘要(跳过 LLM,内容就是上一份全量摘要),但仍走 D4 校验(模板产物应已含两段;缺失则退化到 LLM 路径补段)。
4. **空会话拒绝**:无 prior 且无 regular 行 → `NothingToHandoff` 错误,零副作用。
5. **D4 校验 + 重试**:新增 `validate_handoff_summary(text) -> Vec<HandoffMissingSection>`——段标题匹配用**子串**(`Work State` / `Next Step`;模板第六段实际标题为 "Optional Next Step",compaction.rs:605),标题在且对应正文非空。缺失 → 第二次 `send_summary_completion`,prompt 追加纠正块("上一份摘要缺 X 段,必须包含全部六段");仍缺失 → `SummaryMissingSections(sections)` 错误,**不建 session**。熔断记账:最终成功 `record_success`,最终失败 `record_failure`(与 manual 一致;中间重试不计)。
6. **建新 session(评审②:create 后 UPDATE 路线)**:`db::create_session`(session_crud.rs:29)只收 `(id, project_id, cwd, model, model_id, session_type, metadata)` 七参,title/worktree 三列/workflow_enabled/plugin_name/mode 全部硬编码——继承分两步:
   - **create 时传参**:project_id、current_cwd、model、model_id、metadata(= child 的 `{"handoff": {parent_session_id, parent_title, focus?}}` 创建即写入,child 侧零读-改-写);
   - **create 后 UPDATE(全部纯新增调用,不动既有函数)**:`rename_session`(:553)设 title = `接力: {parent.title 去掉一层已有 "接力: " 前缀}`(80 截断,防首条"继续"抢注 auto-title,messages.rs:86);`set_worktree_state`(:520)一条语句写 worktree 三列(签名即 state+path+last 三参,按 parent 原样复制,见 §7 风险);`set_session_workflow_enabled`(:617)/`set_session_plugin_name`(:662);mode 经 `set_session_mode_internal`(commands/question.rs:373,pub(crate),复用 mode_changed 审计 + Yolo root guard;仅 parent.mode != edit 时调用,避免常见路径多一条审计)。
   - **失败清理**:任一后置步骤失败 → best-effort 走既有 delete_session 路径清掉空壳 child 再报错(空壳无 messages,删除无损),保住"失败零副作用"语义。
7. **摘要落库**:新 db fn `insert_handoff_summary`(session_crud.rs:767 insert_compaction_summary 旁):`role='user'`,**content 列 = 单 Text 块 JSON `[{"type":"text","text": prefix+摘要}]`(insert_compaction_summary 同款形态,:777),text 列 = 同内容纯文本**——"两列同值"指语义同载完整 `SUMMARY_CONTEXT_PREFIX + "\n\n" + 摘要`(自包含落库——prefix 在 /compact 路径是仅 wire 拼接不落库,handoff 反之,因为新 session 没有水位机制替它拼),非字节级同形态;`messages.metadata = {kind: "handoff_summary", parent_session_id, trigger: "manual", focus, cutoff_seq, tokens_before/after, model}`,seq=1(新 session 空表,调用方给游标)。
8. **双向 metadata**:child 侧已在 §3.6 create 参数写入;parent 读-改-写合并 `{"handoff_children": [child_id, …]}`(列表容多次接力;不得 clobber 已有键——GroupChatConfig 先例)。并发 clobber 风险(评审⑥)MVP 接受(用户驱动低频),可选硬化 SQLite `json_set` 原子合并不进 MVP。仅动 sessions.metadata 列,不动 messages。

## 4. 新 session 的水位语义(关键坑的解法)

**handoff_summary 不参与水位**:`apply_compaction_watermark`(compaction.rs:199)只认 `kind == "compaction_summary"` 的行找锚点;handoff 行在 wire 里就是普通 user 消息(prefix 已落库,自包含)。由此:

- 直接移植旧 session 摘要行会 AlignmentFailed fail-open 的坑(cutoff_seq 跨 session 失配)从根上绕开;
- 新 session 后续增长触发自动压缩时,handoff 行作为 regular 行进 transcript 被再摘要,链路自然延续(首个锚点在新 session 内自洽);
- D2 搜索天然覆盖(text 列有值,role='user')。

不新增 migration(kind 复用 messages.metadata,同 softcap 先例)。

## 5. 前端

- **命令注册**:resource_loader `BUILTIN_COMMANDS`(src/resource_loader.rs:105+)加 `handoff`(描述:"接力到新会话");`app/src/utils/slashCommand.ts:18-23` BUILTIN 列表 + `matchBuiltinCommandInput`(:45)自动覆盖直输拦截。
- **分发**:`ChatInput.vue` `executeBuiltin` 加 case( compact 先例 :528-553):toast "正在生成接力摘要…" → `transport.invoke("handoff_session", {sessionId, focus})` → 成功后刷新 session 列表 + **切到 new_session_id**(复用现有会话切换/加载管线,新 session 消息只有 handoff 行,reload 即出卡片)→ toast 展示 tokens_before→after;失败 toast 报错(零副作用,留在原会话)。palette 路径 focus 提取镜像 compact(:572)。
- **卡片渲染**:`MessageItem` 对 `kind == "handoff_summary"` 复用 compaction_summary 摘要行卡片外壳,徽标区显示"接力自 {parent_title}"+ 点击跳 parent session(chatStore 切会话);metadata 字段走 `chat.types.ts` 加 `HandoffSummaryMeta`。
- **TS 类型**:`HandoffResult` 镜像后端(chat.types.ts:542 ManualCompactionResult 旁)。

## 6. 测试与验收映射

| AC | 验证 |
|----|------|
| AC1 | 集成:stub provider + 有历史的 session A → run_handoff → load session B,B 首行 kind=handoff_summary、text 含 prefix;B 内续跑一轮(wire 首条 = prefix+摘要) |
| AC2 | 单测:provider 首次缺 Next Step → 二次 prompt 含纠正块 → 成功;恒缺 → SummaryMissingSections 且 sessions 表无新行 |
| AC3 | DB 断言:child.metadata.handoff.parent_session_id == A;A.metadata.handoff_children 含 B;json_extract 样例查询进 spec |
| AC4 | DB 断言:A 的 messages 行数接力前后不变 |
| AC5 | live:turn-smoke.sh 加 `--handoff` 模式(建临时 session 跑数轮 → POST /sessions/:id/handoff → 断言新 session + 续跑 + 清理两 session) |

新增测试落点:`tests_agent_loop/handoff.rs`(编排+校验)、`db/sessions_tests/handoff_summary.rs`(落库+水位忽略+metadata 合并)、命令层 route 冒烟、resource_loader builtin 断言、FE slashCommand/executeBuiltin/卡片测。

## 7. 风险与边界

- **worktree 双 session 共享**:复制 worktree_path 后两个 session 指向同一目录。MVP 接受(parent 闲置、用户驱动;等价于两个终端开同一目录)。spec 记录边界,不做互斥锁。
- **compaction.rs 共享文件改动**:run_handoff 是纯新增,不动 run_manual_compaction / 自动路径函数;风险集中在文件内新增 helper(validate_handoff_summary)——全量 compaction 50 测回归兜底。
- **摘要质量快路径退化**:prior 快路径产物若缺段,退化走 LLM 补段,不静默放行(AC2 对快路径同样成立)。
- **超长会话全量摘要**:handoff 无保留区,compressible 可能比 compact 大;transcript 0.7×window oldest-drop 兜底,4k clamp 照常。

## 8. spec 回写计划

- `agent-loop-architecture/pattern-llm-compaction.md` 加 §handoff 入口:两落点关系、全量覆盖语义、handoff_summary 行契约(kind/prefix 落库/水位不参与/seq=1)、双向 metadata 契约 + json_extract 审计查询样例。
- frontend chat spec 加 handoff 卡片段(复用 compaction_summary 外壳 + parent 跳转),对齐 search-history-card 先例。
