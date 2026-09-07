# design — 群聊止损包 + 回归闸(C1.1 / C1.2 / C3.1 / RULE)

> 需求与决策见 [prd.md](./prd.md)。本文只讲「怎么做」。

## 0. 总览

四个工作流,两个 work commit(共识契约:止损包同 PR 两 commit 顺序敏感,C1.1 先合),R3 随 R1/R2 走,R4 独立先落(零代码依赖,且为后续清扫提供判定依据)。

| Work | 触碰面 | Commit |
|---|---|---|
| R4 RULE 进 spec + 病灶清扫 | `.trellis/spec/backend/quality-guidelines.md` + 三处过时注释 | commit 0(spec/docs,先行) |
| R1 C1.1 ask-free | permissions + chat_loop 接线 + 单测 | commit 1 |
| R2 C1.2 budget | group_chat_loop 累计/检查 + GroupChatConfig + 前端白名单/notice + GUI 输入 + 单测 | commit 2 |

---

## 1. R1 — C1.1 ask-free

### 1.1 信号通路

```
ChatLoopRequest.group_chat_state: Option<GroupChatState>   ← 已存在
  → run_chat_loop 内 PermissionContext 构造(chat_loop/init.rs:398)
      新增字段 group_chat_ask_free: bool(= req.group_chat_state.is_some())
  → ask_path 入口短路判定(见 §1.2 位置择优)
```

- worker 分支(ask.rs:278)**不动**:群聊三处 run_chat_loop 调用(moderator:545 / participant:782 / preempt 收束轮:1161)的 `CallerRole.is_worker` 全为 `Some(false)`(530-531 / 767-768 / 1145-1146),不会进 worker 分支。
- 后台 shell 升级路径(评审修正):`background_escalation.rs:370` 在 `#[cfg(test)] mod tests` 内;生产侧后台升级经 drive.rs:832 `permission_ctx.clone()` **自动继承 init 构建的 ctx**——群聊场的后台升级重跑会继承 `group_chat_ask_free=true`,语义上恰是想要的(讨论中的升级重跑同样不该等审批),零改动;仅需为编译在测试 harness(370 区)补字段默认值。

### 1.2 短路语义(替换而非超时)

在 `ask_path` parent 分支 emit 之前(评审注:短路判定可上提到函数头 `ask_no_timeout_enabled` 读之前——`group_chat_ask_free` 为 true 时必然非 worker,提前返回省一次 config DB 读;实现时择优,语义等价):

```rust
if ctx.group_chat_ask_free {
    // 不 register_ask、不 emit_permission_ask、不进 select——
    // 零往返、零等待、前端零 modal。
    let _ = record_audit(db, ctx, AuditKind::ToolDenied, tool_name, tool_input,
                         Some(ASK_FREE_DENY_REASON)).await;
    return Decision::Deny { reason: ASK_FREE_DENY_REASON.into(), critical: false };
}
```

- 常量 `ASK_FREE_DENY_REASON`(permissions/ask.rs,与 `ASK_TIMEOUT` 同区):
  `"group chat runs ask-free: permission ask auto-denied; continue without this action"`
  —— 英文、面向 LLM 读(GC3 先例:deny reason 进 tool_result(is_error),模型自适应改道)。文案是**测试断言契约**(AC1),定死不再改。
- 审计沿用 `AuditKind::ToolDenied`,reason 带 ask-free 前缀可事后甄别,**不新增 variant**(R1 约束;评审核验通过:ToolDenied 本就是多源 variant,reason 区分是既有实践;实现时在该 variant 的 doc 注释补 ask-free 来源一行)。
- GC3 的 attendance 窗口逻辑(120s/8s)对群聊变为不可达——`ask_timeout_for_attendance` 不改,经典聊天行为逐字节不变。

### 1.3 与 Yolo 的关系

Yolo 在 Tier 4 之前的 check 层 bypass(ask.rs 注释:262-266),ask-free 短路在 Tier 4 入口,两层互不可见。群聊 + Yolo = 全工具自由(现行为);群聊 + 非 Yolo = 研究工具照常、越界即时拒。语义正交。

---

## 2. R2 — C1.2 token 预算

### 2.1 数据流

```
GroupChatConfig.token_budget: Option<u64>        ← metadata additive 键(serde default)
  → build_group_chat_ctx 解析进 GroupChatCtx(新字段 token_budget)
  → run_group_chat_loop 入口建 TokenTally(Arc<BudgetTally>)
  → TallySink 装饰器包住原 sink,替换传给所有内层 run_chat_loop 的 sink
      (moderator / participant / preempt 收束轮三处同替换)
  → 每个内层 LLM turn 的 Done{usage: Some(TokenUsage)} 经装饰器:
      tally.fetch_add(input + output + cache_creation + cache_read)
  → 外层循环头(for round 迭代头部)检查:
      budget 声明 && tally.load() > budget → halt_reason = Budget; break
```

### 2.2 TallySink 装饰器

- `ChatEventSink` trait(state.rs:825 起)**10 个方法**(4 必实现 + 6 带默认实现)全转发;仅 `emit_chat_event` 检查 `ChatEvent::Done { usage: Some(u) }` 累计(`ChatEventPayload.event` 经 serde flatten 可内省),其余透传。**`has_live_observer` 必须显式转发到内层**——daemon HttpSseSink 是唯一生产覆写,漏转发会破坏 GC3 观察者感知(留作未来的暗雷)。~60 行机械代码,放 group_chat_loop.rs(仅群聊用,不导出)。
- 计数器 `AtomicU64`(跨 await 无锁);口径 = **四计费字段求和**(prd Decisions Q2)。`context_input_tokens` 不计(与 input 重叠的 trace 观测口径)。
- **usage = None 贡献 0**:错误/取消 turn 常无 usage 报告,预算对其失明——该面由 GC5 熔断兜住(连续错误 halt),两机制互补不重叠。design 如实记录此边界。

### 2.3 HaltReason::Budget 与终态语义

- `HaltReason` 增 `Budget` 变体;post-loop 映射 `STOP_REASON_BUDGET: &str = "budget"`(常量区与既有 STOP_REASON_* 同排)。
- **不跑 preempt 式收束轮**:preempt 的收束是「体面打断」语义;budget 到线 = 钱已烧完,收束轮本身还要烧一个 moderator turn,与止损目的相悖。直接 break → finalize 落库(stop_reason="budget",summary 缺,与 max_rounds/error 同类)。
- 检查点(轮头)与 checkpoint 表:P1a checkpoint 记 round + error streak;budget halt 是**主动终态**不是崩溃,不写 interrupted checkpoint(与 max_rounds 同:正常退出路径本就不留 checkpoint 行——沿用既有语义,零改动)。
- **overshoot 上界**(契约):检查在轮头,单 speaker turn 内层 ≤ 20 次 LLM 调用(max_turns=Some(20)),实际消耗 ≤ 声明值 + 一个 speaker turn。不在内层循环中途打断(需把预算穿进 run_chat_loop,侵入面大,一期不做)。
- 复用/reuse session 二跑:预算按**本次 run 的 tally** 计(TallySink 随 loop 生灭),不是 session 全周期累计——与共识「逐块累计」字母一致。**边界注记(评审补)**:P1a interrupted-resume 语义是「同一场续跑」,而 tally 随新 run 重置——一场被 SIGKILL 打断后续跑,总消耗理论上可达 2×budget;已声明的取舍,若 M4 要收紧可选 checkpoint 行加 consumed 字段,本任务不做。resume 链路经 chat_inner → build_group_chat_ctx 重读 metadata,budget 声明跨 resume 存活(chat.rs:842-847)。

### 2.4 前端

- **终态白名单有两处,都要加 `"budget"`**(评审修正):①`streamEvents.ts:151-161`(`handleChatEvent` 头部早判——驱动定时场收官 toast 路由与跨客户端/驱逐场 finalize,漏了则 budget 终态在定时场不弹 toast、跨客户端认领场 activeRequests 悬挂);②`streamEvents.ts:644-650`(done 处理器 finalize 门)。
- `groupChatNotice`(streamController.ts:604)增 case:`"budget"` → `"讨论已达 token 预算上限，自动停止。"`;
- 可选:`scheduledStopReasonLabel`(streamController.ts)加 budget → 「达到 token 预算上限」case(default 只透出裸字符串,不加也能用)。

### 2.5 GUI

- 写入链路(评审修正后完整路径):`GroupChatConfigModal.vue`(可选数字输入「token 预算」,留空 = 不写键)→ `chatSessionActions.ts:94-97` `createNewSession` 的 metadata 打包加键。vitest:留空不写键 / 填值写入。
- **编辑路径保键**:同文件 `updateGroupChatConfig`(chatSessionActions.ts:149-152)现全量替换 metadata 为 `{participants}`——编辑名单会静默抹掉 token_budget。改为在既有 metadata 上合并(保留未知/新增键),vitest 补「编辑名单不丢 token_budget」断言。

### 2.6 被否掉的替代路线(记档)

- **轮头读 sessions A4 累计列**:已核实是孤儿列(2026-06-26 snapshot 重构后无写点,db/usage.rs:88-92 注释明示),不可用。
- **轮头 SUM turn_trace(run_id 归因)**:依赖群聊 speaker turn 的 trace 行写全覆盖 + run_id 分组语义(rid 在群聊跨 speaker 的复用关系未核实),且每次轮头多一把 DB 查询。装饰器自包含、零 DB 依赖、可单测,胜出。

---

## 3. R3 — 回归闸测试清单

后端(`tests_group_chat.rs` / `tests_ask.rs`,MockProvider 零 LLM):

1. `group_chat_ask_free_denies_out_of_bounds_ask_without_roundtrip`:群聊 harness 触发越界 ask → `MockEmitter` 零 `permission_ask` 记录、tool_result(is_error) 含 `ASK_FREE_DENY_REASON`、用时为零(无 timeout 机制参与)。
2. budget 三剧本(共识指定):
   - `group_chat_budget_halts_when_moderator_never_nominates`(永不点名);
   - `group_chat_budget_halts_on_tool_loop_burn`(狂调工具:participant 多内层 turn 各带 usage);
   - `group_chat_budget_halts_on_error_turns_with_usage`(每轮报错但带 usage 的 Done)。
   各断言:halt + `sessions.stop_reason == "budget"` + 终态 Done 携带;另加一例 `token_budget = None` 不触发的对照(既有 19 用例亦复证缺省零变更)。
3. 前端 vitest:`groupChatNotice("budget")` 文案;streamEvents finalize;GUI modal。

---

## 4. R4 — RULE 与清扫

- `quality-guidelines.md` **追加**一节(评审修正:该文件已有 4 条既填约定,非空模板):RULE 正文(「复述行为的注释一律删,行为断言一律进测试」)+ 产品理由(群聊参与者读注释,假注释是毒数据——D3 实证 deepseek 读错注释进共识链)+ 适用三层(chat_loop / group_chat / subagent)+ 判定示例(本任务清扫的一处)。
- 清扫清单(评审修正后仅一处;group_chat_loop.rs:60-66 round-robin 注释已由 `66ef6fa4` 于共识当日修复):
  permissions/types.rs:148-155 `is_worker` 注释(「collapse to Deny」是 RULE-FrontSubagent-003 修复前旧语义;现状 = 完整 ask 往返,ask.rs:227-262 注释为准)——改写为现状描述,行为契约由 tests_ask 锁定。

---

## 5. 兼容与回滚

- metadata `token_budget` additive,serde default None:旧 session / 旧调用方零影响;缺省预算关闭 = 行为开关默认 off,无需 kill-switch。回滚 = revert 两 commit,无 schema/契约残留。
- DAEMON-API:§6 群聊 metadata 键表加一行 `token_budget`(additive 文档,非契约变更)。
