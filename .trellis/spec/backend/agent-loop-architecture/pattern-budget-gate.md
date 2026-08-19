# Pattern: 关卡⑤上下文预算硬卡(unified-context-budget,2026-08-19)

> 来源:任务 `08-19-unified-context-budget`(PRD/design/review 走
> `.trellis/tasks/08-19-unified-context-budget/`)。实现
> `agent/budget.rs` + `drive.rs` send 前闸门 + `init.rs` spans/目录态
> 产出。

把上下文窗口从"各机制自治"变为统一预算表管到底:send 前最后一道检查
统一估算总量(`system + tools + messages` 三部件加法),超 0.95×window
按优先级静默裁剪请求副本,裁尽仍超才 fail-fast。

## 估算的两类口径(评审 F1 教训,永不互相加计)

- **总量口径**(发送部件加法):`estimate_request_tokens(system,
  tools_json, messages)` = 三部件之和。**messages 部件已含 memory
  指令头对、skill listing、@文件注入正文、图片 pad、全部历史** ——
  memory/skill/@文件/图片是合成消息或消息内块,物理在 messages 里,
  在总量公式上再单独加计任何一项就是重复计数。
- **归因口径**(messages 内部归因):`memory_token` / `at_files_token` /
  `images_token` / `system_token`(prompt 本体 + skill listing)只是
  TracePanel 占比条的切片,之和 ≤ 总量(残差 ≥ 0)。
- 压缩三处触发口径(摘要触发 0.85 / postcheck 0.95 / 机械
  `compact_messages`)全部用总量口径;机械路径经 `extra_tokens` 参数
  携带 system+tools(它只见 messages),无 gate —— 群聊/worker 同受益。

## 时序(D7 重排:tools 链在压缩块之前)

`drive_turn` 顺序:head_sha/system 刷新 → **tools 过滤链 + stubify +
元工具 append + tools_token 估算** → C3 压缩块(统一口径触发)→
turn_messages APPEND 组装 → 图片 resolve → **budget gate** → send。
tools 链前置的原因:压缩触发线需要当轮 tools 估算。闸门放在 APPEND
之后的原因:压缩与 send 之间的增长(checklist/后台 shell 通知/recall)
正是闸门要兜的窗口 —— C3 的 StillOver(0.5 target)会抢先中止一切
确定性超线构造,闸门的真实触发面就是这个窄缝。

## 裁剪臂(D3 优先级,非破坏性 D6)

1. **旧轮次 @文件 span → 占位行**:spans 是**同请求临时产物**
   (`inject_at_tokens` 产出,经 LoopInit 穿参,**不落 DB** —— @文件
   每 request 按当前文件重展开,DB spans 必 stale);同消息多 span 按
   start 降序应用保前序偏移;`span_text` 失配(越界/角色漂移/非字符
   边界)fail-open 跳过。当前 turn 槽位(`current_user_msg_idx`,
   注入后 rposition)保护不裁。
2. **旧轮次 Image 块 → B1 占位降级文案**(模型知有图未发,防幻觉)。
3. **memory 头回退目录态快照**(init 预构建
   `build_instructions_blocks_with_digest(_, true, &空集)`;digest 机制
   不在(开关关/无层)则臂不可用;`MemoryDigestRegistry` 不动 —— 窗口
   持续紧则每轮等效回退,松了零成本恢复)。
4. 臂尽仍超 → Error turn(`context_over_budget` + 各切片 breakdown)
   + abort,对齐 RULE-A-002 形态。

## Gate 与观测

- `context_budget_enabled`(config 缺省 on,fail-open)&&
  `!effective_is_worker && !is_group_chat`(与 digest/compaction 同款
  豁免;机械压缩无条件)。
- **trace 一律记实发值**(预裁 − freed 算术差,臂 3 改记目录态值;
  prd D9)—— 与 provider `context_input` 可比,无裁剪时零特判;预裁
  值只进 audit payload。
- 裁剪发生才落观测:`ChatEvent::BudgetTrim`(非持久化,Retrying 先例,
  前端瞬时 chip + trace 徽标)+ `AuditKind::ContextBudgetTrim`
  (payload `{arms, over_by, pre_total, post_total, window}`,enum 变体
  无 migration)。

## When this bites

- 新增"注入进请求"的机制时,先问它落在哪个部件:进 messages 的
  (如新合成消息)总量口径自动覆盖,**不要**在公式上加项;真发送部件
  (如未来独立 system 段)才进 overhead。
- 改 `inject_at_tokens` 的展开形态(如换 wrapper)必须同步
  `AtFileSpan` 偏移语义(Text→Blocks 形态 = 首 Text 块内偏移)。
- `PROTECTED_HEAD`/保留区语义变化会改变闸门的可触达窗口 —— C3 的
  StillOver 中止线(0.5 target)在闸门(0.95)之前,任何"想看闸门
  触发"的测试构造都要先过 C3 这一关(实证:全 loop 集成只能锁
  no-misfire,臂级行为靠 `enforce_budget` 单测)。
