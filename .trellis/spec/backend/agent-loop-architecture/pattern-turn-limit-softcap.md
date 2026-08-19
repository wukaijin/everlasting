# Pattern: MAX_TURNS 软卡(撞线询问替代硬终断,2026-08-19)

> 来源:任务 `08-18-max-turns-softcap`(PRD/design/research 走
> `.trellis/tasks/archive/2026-08/08-18-max-turns-softcap/`,现实现
> `chat_loop.rs` 的 `ask_turn_limit_softcap` + `emit_max_turns_terminal` +
> `drive.rs` 的 force 穿参)。

单聊主 loop 撞线(缺省 `MAX_TURNS = 200`)不再无条件
`stop_reason="max_turns"` 硬停,改为 QuestionStore 软卡询问:
继续(+200)/ 压缩后续跑(gate 开时)/ 停止。C3+ 摘要压缩保证
context 可无限续,轮数上限只剩"无人值守烧钱"一个守门理由——
守门方式从硬断升级为询问 + 10 分钟超时兜底。

## 结构契约(load-bearing)

- **循环骨架**:`for turn in 1..=turn_limit` → `loop` + `turns_budget`
  (可变)。`turn` 仍从 1 起逐轮 +1;软卡询问消耗一个 turn 号
  (budget+1)不执行 turn 体。「继续」= `turns_budget +=
  TURN_LIMIT_GRANT`(`= MAX_TURNS`),400/600… 每次撞线再问。
- **break 门 = `effective_is_worker || group_chat_state.is_some()`**:
  worker(有 C1 resume)与群聊 speaker 段
  (`group_chat_loop.rs` 以 `max_turns=1` 复用 `run_chat_loop`)保持
  硬卡直接 break。**漏掉群聊门会让 tool_use 结尾的 speaker 轮挂满
  10 分钟超时**(实现期间实测:全量套件 30 分钟)。回归用例
  `softcap_group_chat_breaks_without_ask`。
- **撞线点在循环边界**:上一轮 tool results 已由 `finalize_turn`
  落库,DB 尾部干净——停止/超时分支**不需要**
  `finalize_pending_tool_results`,直接复用提取出的
  `emit_max_turns_terminal`(worker break 与软卡停止共用,保证
  "与今日终态等价"字面成立;`stop_reason` 字符串仍为
  `"max_turns"`,消费方零感知)。
- **四臂 biased select**(结构照抄 C2+ 主动干预 + 新增超时臂):
  `token.cancelled()` → `Done{cancelled}`;
  `sleep(softcap_ask_timeout())` → 超时停止(缺省 10min);
  `rx` Answered 三分支 / Cancelled(跳过→停止)/ Err(dropped→
  cancelled)。rx 臂**不显式 remove**——`QuestionStore::resolve`
  在同一临界区原子移除槽位(见 question_store.rs resolve 契约)。
  register `AlreadyPending` → warn + 降级为今日硬停(防御,理论上
  不可达)。
- **payload 条件构建**:`compaction_on` 三选项,否则两选项(卡片
  不展示选了也无效的选项);label 精确匹配解析,未匹配/畸形 →
  停止(防御默认,C2+ 同款)。

## 「压缩后续跑」force 穿参

不走手动 `/compact`(其 seq=MAX+1 契约要求空闲期,loop 活跃时
调用会与 persist 撞号)。软卡置 `force_compaction = true`,下一轮
`drive_turn` 按值收参——**只绕过 C3 的 token 触发线**,gate
(开关/worker/熔断/skip_persist)与空待压区(cut ==
synthetic_prefix_len → 机械路径)全部照旧;drive_turn 返回后
立即置 false(一次性,不泄漏到后续 turn)。观测区分:
`attempt_summary_compaction` 的 metadata `trigger` 字段由
`trigger_label` 参数注入(auto 路径 `"auto"`,软卡 force
`"softcap"`)。

## 新 variant / audit kind(均无 migration)

- `PendingInteraction::TurnLimitSoftcap(ToolQuestionPayload)` +
  `InteractionKind` → `"turn_limit_softcap"`;`tool_use_id` 前缀
  `turn_limit_softcap_{turn}`。**绝不能 tag 成 `Question`**——无
  tool_use 锚点的合成询问会永不渲染(2026-07-28 事故,同
  `LoopIntervention` 教训)。前端 `streamEvents.ts` 按前缀打 kind
  标签、`ChatPanel.vue` 渲染浮动卡(复用 `AskUserQuestionCard`)。
- `AuditKind::TurnLimitSoftcap` → `"turn_limit_softcap"`(纯 TEXT
  kind 列);action 集:`asked`(register 成功即落)/ `continued` /
  `compacted_continued` / `stopped`(含跳过与 AlreadyPending 降级)/
  `timeout_stopped` / `cancelled`。worker 与群聊不落本表。

## env 钩子(QA/测试专用,先例 `P1_DBG`)

- `EVERLASTING_SOFTCAP_TIMEOUT_MS`:询问超时覆盖(缺省 600_000)。
  测试用它调短超时臂;设值的测试必须串行(`SOFTCAP_ENV_LOCK`)+
  `EnvVarGuard`(Drop 清理)防并行泄漏。
- `EVERLASTING_SOFTCAP_TURN_BOUNDARY`:撞线边界,**只在 loop 初始化
  时应用一次**(替换初始 budget)。若每轮重读,「继续」加成后边界
  仍固定在 env 值 → 每轮重问(QA 陷阱,实现期间修掉)。

## 测试模板

resolver spawn 轮询 `get_payload()`,按 `entry.kind` 分流作答
(`tests_agent_loop/softcap.rs` 的 `spawn_resolver`);无 resolver 的
用例靠超时臂收尾。mock 编排注意:force 压缩会消耗一响脚本
(摘要旁路 completion 在主 turn send 之前,`is_summary_send`
断言区分)。
