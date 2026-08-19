# Design — MAX_TURNS 软卡化

> 依据 prd.md 决议表(阈值 200 / +200 粒度 / 10min 超时停止 / gate 关隐藏压缩选项)。
> 所有 file:line 锚点基于 2026-08-19 main(712259b)。

## 1. 总体结构

把 `chat_loop.rs:905` 的 `for turn in 1..=turn_limit` 重构为 `loop` + 预算变量;
撞线时主 loop 进入软卡询问(worker 直接 break 走今日硬终态)。询问体复用 C2+
(drive.rs:1597-1960)的 register → `emit_tool_question` → select 三臂结构,
**新增超时臂**。压缩联动不走手动 `/compact`(其 seq=MAX+1 契约要求空闲期),
而是给 `drive_turn` 加 force flag,让下一轮自动路径强制触发。

```
loop {
    turn += 1;
    if turn > turns_budget {
        if effective_is_worker || group_chat_state.is_some() { break; }  // → 撞线后硬终态(原样)
        match ask_turn_limit_softcap(...).await {      // 新 helper(chat_loop.rs)
            SoftcapOutcome::Continue        => { turns_budget += TURN_LIMIT_GRANT; continue; }
            SoftcapOutcome::CompactContinue => { turns_budget += TURN_LIMIT_GRANT;
                                                 force_compaction = true; continue; }
            SoftcapOutcome::Terminal        => break,   // 终态已在 helper 内 emit
        }
    }
    // ……原 per-turn 体原封不动(drive_turn / dispatch_tool_calls / finalize_turn)
    force_compaction = false;   // 一次性消费
}
```

**as-built 修正(2026-08-19)**:break 门除 worker 外还必须含
`group_chat_state.is_some()`——群聊以 `max_turns=1` 复用 `run_chat_loop`
跑每 speaker 段(group_chat_loop.rs §54),tool_use 结尾的 speaker 轮同样会
耗尽预算;不过此门则软卡误入群聊路径(R4 违规),且无人看 store 的
speaker 段会挂满 10 分钟超时(实现期间实测:全量套件被拖到 30 分钟)。
回归用例 `softcap_group_chat_breaks_without_ask`。

撞线边界测试钩子(QA/live 用,AC5):`turn > softcap_boundary()` 优先于
`turn > turns_budget` 判定,`softcap_boundary()` = env
`EVERLASTING_SOFTCAP_TURN_BOUNDARY`(parse 失败/未设 → 用 `turns_budget`)。
生产不设 env 时行为与 `turn > turns_budget` 完全一致。

## 2. 模块与契约

### 2.1 chat_loop.rs — 循环重构 + 询问 helper

- `for turn in 1..=turn_limit` → 上述 `loop`;`turn` 仍从 1 起、逐轮 +1
  (drive_turn 的 turn 参数 / loop_intervention id / DBG 语义不变)。
- 撞线后硬终态块(现 1055-1098)拆分:
  - persist + `Done{max_turns, usage: last_usage_terminal}` 提为模块级
    `async fn emit_max_turns_terminal(...)`(worker break 落点与软卡停止/
    超时落点共用,保证 AC2"与今日等价"字面成立);
  - `if effective_is_worker { sink.record_worker_messages(&messages) }` 留在
    循环后(worker 永不经软卡,flag 无需考虑)。
- 新增 `SoftcapOutcome { Continue, CompactContinue, Terminal }` +
  `#[allow(clippy::too_many_arguments)] async fn ask_turn_limit_softcap(...)`。
  入参:question_store / sink / rid / session_id / db / token / skip_persist /
  turn / turns_budget / compaction_on / last_usage_terminal / last_cwd / seq
  (drive.rs 同款 too_many_arguments 先例)。

**payload(条件构建,决议 4)**:

- `tool_use_id = format!("turn_limit_softcap_{turn}")`(turn = budget+1);
- `PendingInteraction::TurnLimitSoftcap(ToolQuestionPayload)`(**新 variant**,
  前端按 kind 渲染浮动卡,同 LoopIntervention 的 2026-07-28 事故教训:绝不能
  tag 成 `Question`——无 tool_use 锚点会永不渲染);
- 问题文案:"本轮对话已达到 200 轮上限(agent 仍在工作中)。是否继续?";
- 选项顺序:`继续(+200 轮)` → [`压缩后续跑`(仅 `compaction_on`)] → `停止`;
  解析按 label 精确匹配,未匹配/畸形 → 停止(C2+ 防御性默认同款)。

**select 四臂(biased)**:

| 臂 | 行为 | audit action |
|---|---|---|
| `token.cancelled()` | remove 槽位 → `Done{cancelled}`(无需 finalize_pending_tool_results——撞线点 tool results 已落库) | `cancelled` |
| `sleep(softcap_ask_timeout())` | remove 槽位 → `emit_max_turns_terminal`(AC2 等价收尾) | `timeout_stopped` |
| `rx` Answered「继续」 | 预算 +200 继续 | `continued` |
| `rx` Answered「压缩后续跑」 | 预算 +200 + force_compaction=true | `compacted_continued` |
| `rx` Answered 其他/畸形 | `emit_max_turns_terminal` | `stopped` |
| `rx` Cancelled(跳过) | `emit_max_turns_terminal` | `stopped` |
| `rx` Err(dropped) | warn → `Done{cancelled}`(C2+ 同款) | `cancelled` |

`register` 返回 `AlreadyPending`(理论上不可达:turn 内 ask_user_question 在
finalize 前已 resolve;防御)→ warn + 走 `emit_max_turns_terminal`(降级为
今日行为,audit `stopped`)。

**常量与超时**(chat_loop.rs 顶部或 mod.rs MAX_TURNS 旁):

- `const TURN_LIMIT_GRANT: usize = MAX_TURNS;`(200)
- `fn softcap_ask_timeout() -> Duration`:env `EVERLASTING_SOFTCAP_TIMEOUT_MS`
  覆盖,缺省 600_000ms(10min)。env 调试钩子先例:`P1_DBG`(drive.rs:2064)。
  Rust 2021,`env::set_var` 安全;仅软卡专项测试设置,其余测试不经此路径。

### 2.2 drive.rs — force 压缩穿参

- `drive_turn` 增参 `force_compaction: bool`(签名加参先例:stub_on /
  digest_on / memory_token / summary_anchor 一脉相承)。
- C3 gate(drive.rs:225)改为
  `if summary_gate && (force_compaction || tokens_pre as u64 >= trigger as u64)`
  ——**只绕过 token 触发线,gate 本身(开关/worker/熔断/skip_persist)与
  空待压区(cut == synthetic_prefix_len → 机械路径)全部照旧**;force 是
  一次性消费(chat_loop 在 drive_turn 返回后置 false)。
- `attempt_summary_compaction` 增参 `trigger_label: &'static str`,metadata
  `trigger` 字段由硬编码 `"auto"` 改为该参数(auto 路径传 `"auto"`,软卡
  force 路径传 `"softcap"`)——观测可区分。

### 2.3 question_store.rs — 新 variant

- `InteractionKind::TurnLimitSoftcap` → `as_str() = "turn_limit_softcap"`。
- `PendingInteraction::TurnLimitSoftcap(ToolQuestionPayload)`(复用 payload
  形状,同 LoopIntervention 先例;serde tag snake_case 自动生效)。
- resolve / get_payload / remove 全部 kind 无关,零改动;daemon 远端链路
  (`routes/question.rs` resolve + `sse.rs` `tool:question` broadcast)kind
  无关,零改动。

### 2.4 permissions/audit.rs — 新 AuditKind

- `AuditKind::TurnLimitSoftcap` → `"turn_limit_softcap"`(纯 TEXT kind 列,
  无 migration,LoopIntervention 先例)。
- `record_turn_limit_softcap_audit(db, session_id, turn, budget, action, seq)`
  镜像 `record_loop_intervention_audit`(best-effort,warn+swallow);
  payload JSON:`{ turn, budget, action }`。
- action 集:`asked / continued / compacted_continued / stopped /
  timeout_stopped / cancelled`(register 成功后立即落 `asked`,同 C2+)。

### 2.5 前端(4 处小改)

| 文件 | 改动 |
|---|---|
| `app/src/stores/streamEvents.ts`(~884-920) | `handleToolQuestion` 前缀识别加 `turn_limit_softcap_` → `kind: "turn_limit_softcap"` |
| `app/src/stores/questionCards.types.ts`(~443) | `PendingInteractionEntry` kind 联合类型 + payload 类型扩展 |
| `app/src/components/chat/ChatPanel.vue`(93-116 / 990-1010) | `turnLimitSoftcap` computed(kind 判等)+ 浮动 `AskUserQuestionCard` 模板块(镜像 loopIntervention;settled → removePending 同款) |
| `app/src/utils/audit.ts`(162/293/363) | kind 联合 + 筛选项 label「轮数软卡」+ 渲染 case(payload: turn/budget/action) |

resolve 链路(`AskUserQuestionCard` → `resolveToolQuestion` IPC/HTTP)形状即
`Vec<QuestionAnswer>`,与 LoopIntervention 完全同构,零改动。

## 3. 关键权衡

- **force 走自动路径而非 run_manual_compaction**:手动入口的 seq=MAX+1 只在
  无活跃 loop 时安全(pattern-llm-compaction seq 契约);loop 内强推会撞
  persist 撞号。force flag 让摘要仍吃 loop 游标(insert_compaction_summary
  吃当前 seq、返回推进值),契约不破。
- **超时默认停止而非续跑**:无人值守时替用户确认烧钱不可接受;停止 = 今日
  行为零回归,且撞线点 DB 尾部干净,用户回来发新消息即续。
- **worker 不软卡**:worker 有 C1 resume + 软终态,父 loop 会经 tool_result
  看到 worker 的 max_turns 并可再派;break 路径保持 1055-1098 原样(AC4)。
- **不设无限继续**:与 R2 兜底精神冲突,一次确认后烧钱无上限(决议已否)。

## 4. 兼容与回滚

- stop_reason 字符串不变:`停止`/超时仍 emit `Done{max_turns}`(前端/worker
  父 loop/trace 消费方零感知);新增事件仅 `tool:question`(既有通道)。
- 唯一行为变化点:主 loop 撞线由立即硬停 → 询问(或 10min 后停)。现有唯一
  依赖该行为的测试 `agent_loop_max_turns_emits_done_marker`(basic.rs:822)
  改造其 resolver:按 kind 分流(loop_intervention → 继续;
  turn_limit_softcap → 停止),断言不变。
- 回滚:单 commit revert;无 schema、无 config、无协议变更。

## 5. 测试设计(mock 端到端,AC5/AC3)

新文件 `app/src-tauri/src/agent/tests_agent_loop/softcap.rs`(mod.rs 注册;
resolver 模板抄 basic.rs:859-893 的 spawn 轮询,按 `get_payload().kind`
分流):

1. `softcap_continue_extends_budget`:max_turns=Some(3) + 撞线 + resolver 答
   「继续」→ 脚本第 4 响 end_turn;断言 send_count=4、audit action=continued、
   Done{end_turn}。
2. `softcap_stop_emits_max_turns_terminal`:撞线 + 答「停止」→ 断言恰好一次
   Done{max_turns}、send_count=budget、cwd/touch 落库(skip_persist=false)。
3. `softcap_timeout_stops`:`EVERLASTING_SOFTCAP_TIMEOUT_MS=50` + 无 resolver
   → Done{max_turns} + audit timeout_stopped。
4. `softcap_cancel_during_ask`:pending 时 cancel token → Done{cancelled} +
   槽位已清。
5. `softcap_compact_continue_forces_compaction`:compaction_on=true + 答
   「压缩后续跑」→ 断言摘要行落库(metadata.trigger="softcap")+ audit
   compacted_continued + loop 续跑(摘要 completion 消耗一响脚本,
   mock 手法参考 tests_agent_loop/manual_compaction.rs)。
6. `softcap_gate_off_hides_compact_option`:compaction_on=false → get_payload
   断言仅 2 选项。
7. worker 回归:现有 subagent max_turns 测试不动、全绿即 AC4 证据。

live(AC5):`EVERLASTING_SOFTCAP_TURN_BOUNDARY=2` + turn-smoke 链路实跑,
验证真实 UI 弹卡 + 三分支(手动点选)。
