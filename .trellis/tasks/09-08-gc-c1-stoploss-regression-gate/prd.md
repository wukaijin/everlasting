# 群聊止损包+回归闸:C1.1 ask-free / C1.2 token 预算 / C3.1 CI 覆盖 / RULE 假注释

## Goal

落地第二场群聊共识(session `eb14d2df`,转录见 BACKLOG 附录 B 增补注)的「止损包 + 回归闸」:让群聊讨论在无人值守下**损失有界**(无权限等待 + token 预算硬停)、机制层回归有闸(MockProvider 单测覆盖),并把「假注释是毒数据」RULE 写进 spec。这是群聊内部改进线(GROUP-CHAT-API-ROADMAP §6 依赖矩阵)的 C1.1 / C1.2 / C3.1 + RULE 四项;C2 证据链(结构化 summary)不在本任务,另立。

上游解锁:本任务交付的 C1.2(`stop_reason=budget`)是 GCE M4「成本治理」的声明上游——预算声明机制就位后,M4 只做消费面(UI/MCP/默认档)。

## Background(共识 → 现状核对,2026-09-08 逐项验码)

共识原文四件事按「亏得有界 → 信得有据 → 改得起」排序;本任务取第 1、3 件。代码现状核对结论:

| 共识项 | 共识设计 | 2026-09-08 码上现状 |
|---|---|---|
| C1.1 ask-free | `group_chat_state.current_speaker` 信号现成;出界 ask 自动拒并回 tool_result 错误;复用 GC3 deny 无害验证 | ✅ 信号确实现成(`ChatLoopRequest.group_chat_state` / `current_speaker` 已贯通群聊两处 turn 构造,group_chat_loop.rs:753-803);❌ ask 路径完全没消费它——`ask_timeout_for_attendance`(permissions/ask.rs:122)只按「有无 live observer」分 120s/8s 两档,**观察者在线的群聊 ask 照样卡 120s**(D1 实证:5×120s×5 round,隧道浏览器在线)。注:全讨论范围(含参与者)是相对共识原文「moderator 段」的**已裁定扩大**(Decisions Q1) |
| C1.2 token 预算 | 外层循环头逐块累计 usage,超限落 `stop_reason=budget`,复用 GC2 Done 语义;MockProvider 三个破坏剧本 | ❌ 全仓库无 `budget` stop_reason;累计也无——编排器把 sink 原样 clone 给内层 loop(group_chat_loop.rs:791),不拦截 `Done{usage}`。数据面现成:`ChatEvent::Done { stop_reason, usage: Option<TokenUsage> }`(llm/types/event.rs:114-117)每内层 LLM 调用发一次,TokenUsage 五字段(llm/types/usage.rs:59) |
| C3.1 机制层 CI | 覆盖:project_root_block 位置 / 熔断 / 轮次帽 / stop_reason 三态 / ask-free 拒绝路径 | 部分已有:project_root_block → `speaker_prompts_carry_working_directory_when_known`(tests_group_chat_prompts.rs:245)✅;熔断 → `group_chat_error_breaker_halts_after_consecutive_error_turns` ✅;轮次帽 → `group_chat_max_rounds_persists_stop_reason` ✅;stop_reason 各值分散已测但无 `budget`;ask-free 路径 = 随 C1.1 新增。tests_group_chat.rs 共 19 用例全在 `--lib`(CI 已跑) |
| RULE 假注释 | 「复述行为的注释一律删,行为断言一律进测试」写进 `.trellis/spec/`,chat_loop / group_chat / subagent 三层生效 | ⚠️ 未落 spec;`.trellis/spec/backend/quality-guidelines.md` 已有 4 条既填约定(clippy gate / fmt / test_pool / parse_md_resource)——在既有 spec **追加**一节,非首填空模板。共识现场指认的第四处病灶(group_chat_loop.rs:60-66 round-robin 注释)**已在当日由 commit `66ef6fa4` 修复**;余下真实病灶 = permissions/types.rs:148-155 `is_worker` 注释(本任务勘察新发现:仍写「Tier 4 ask 必须 collapse 到 Deny」——RULE-FrontSubagent-003 修复**前**旧语义,ask.rs:227-262 现状为完整 ask 往返) |

## Requirements

### R1 — C1.1 ask-free:群聊会话权限 ask 不等待

- 群聊会话(`group_chat_state` 为 Some 的内层 run)任一 speaker turn(**主持人段 + 参与者段,全讨论 ask-free——用户 2026-09-08 裁定**,依据不变量 #1「关键路径无权限等待」无段限定)触发 Tier 4 ask 时,**不发 ask 往返**(无 modal、无 120s/8s 窗口),直接 `Decision::Deny`,reason 写明 ask-free 政策(供 LLM 读 tool_result(is_error) 后自适应改道;GC3 已验证 deny 无害)。
- 实现面:`PermissionContext` 增群聊标记(在 `run_chat_loop` 构造 ctx 处从 `ChatLoopRequest.group_chat_state` 推导),`ask_path` 入口短路。Yolo 语义不变(Yolo 在 Tier 4 之前 bypass,不受影响)。
- 审计:deny 落 `session_audit_events`(沿用既有 AuditKind,不新增 variant;worker 分支不适用——群聊 speaker 非 worker)。

### R2 — C1.2 token 预算:`stop_reason=budget` 硬停

- 编排器经 sink 装饰器拦截每个内层 `Done{usage}`,逐块累计声明口径的 token(**口径已定:四计费字段求和** input+output+cache_creation+cache_read,context_input 是 trace 观测口径与 input 重叠不计——用户 2026-09-08 裁定);外层循环头(每 round 的 moderator 仲裁前)检查,超限 → `HaltReason::Budget` → 终态 `stop_reason="budget"`。
- 复用 GC2 语义:落库 `sessions.stop_reason` + 终态 Done 携带,前端 finalize 白名单 + `groupChatNotice` 增 `budget` 档(streamController.ts:604 switch、streamEvents.ts:623 终态集)。
- 超限精度契约:检查点在轮头,participant turn 内层 ≤20 次 LLM 调用(max_turns=Some(20))、moderator turn ≤1(Some(1)),故实际消耗 = 声明值 + **一个 speaker turn** 的量级;终止语义 = 越线后于下一轮头确定性停。这是接受的 overshoot 上界,如实写进 design。
- 预算声明:`GroupChatConfig`(sessions.metadata JSON)增 additive 可选键 `token_budget: Option<u64>`,serde default None = 不限(缺省零行为变更)。通道现状(评审修正):**GUI 通道本任务落地**(Q3);M1 script / MCP start_discussion 建群 body 均为固定键白名单、今天都不传该键,待各自加参归 M4;**M4a 定时场暂不透传**——`GroupChatTaskConfig` 是闭结构(scheduled_tasks.rs:64-68,未知键 serde 静默丢弃)、fire 建群是枚举白名单重建 metadata(scheduler/mod.rs:1032-1048)而非原样展开,透传需 scheduled 侧加字段 + fire 加键,归 M4。默认档(全局缺省上限)归 M4 成本治理,不在本任务。
- **GUI 建群弹窗顺带加 `token_budget` 可选输入**(GroupChatConfigModal,留空 = 不限——用户 2026-09-08 裁定),前端 vitest 覆盖。

### R3 — C3.1 回归闸补齐

- 新增机制层单测(纯 MockProvider,零 LLM):
  1. ask-free:群聊 speaker 出界 ask → 即时 Deny + tool_result 错误,断言无 ask 往返(零等待);
  2. budget 三个破坏剧本(共识指定):永不点名 / 狂调工具 / 每轮报错——预算到线确定性终止,`stop_reason=budget` 可断言;
  3. 前端 `groupChatNotice("budget")` + finalize 白名单 vitest 断言。
- 既有覆盖差集核对(熔断 / 轮次帽 / project_root_block)已在 Background 表确认存在,不重复建。

### R4 — RULE「假注释是毒数据」进 spec + 病灶清扫

- RULE 正文(「复述行为的注释一律删,行为断言一律进测试」+ 产品理由:群聊参与者读注释,假注释是毒数据,D3 实证 deepseek 读错注释进共识链)**在既有** `.trellis/spec/backend/quality-guidelines.md` **追加**一节(该文件已有 4 条既填约定)。
- 病灶清扫(评审修正后仅一处):permissions/types.rs:148-155 `is_worker` 字段注释(仍写「Tier 4 ask 必须 collapse 到 Deny」——RULE-FrontSubagent-003 修复前旧语义,现状 = 完整 ask 往返),按 RULE 改写。共识指认的 group_chat_loop.rs:60-66 round-robin 注释已由 commit `66ef6fa4`(2026-09-06)修复,本任务无操作。

## Acceptance Criteria

- [ ] AC1(C1.1):单测证明群聊 speaker turn 的越界 ask 即时 Deny(无 observer/有 observer 两态都零等待),tool_result 错误文案含 ask-free 说明;经典聊天(非群聊)ask 行为逐字节不变(既有 tests_ask 全绿即证)。
- [ ] AC2(C1.2):三个破坏剧本 MockProvider 单测全绿,均越线后于下一轮头确定性终止且 `sessions.stop_reason='budget'` 落库、终态 Done 携带;未声明 token_budget 的既有群聊测试(19 用例)全绿即证缺省零行为变更。
- [ ] AC3(前端):`budget` 进 finalize 白名单 + notice 文案;GroupChatConfigModal 的 token_budget 输入(留空 = 不限)落 metadata 正确;vitest 断言通过。
- [ ] AC4(RULE):quality-guidelines.md 含 RULE 正文与三层适用范围;permissions/types.rs:148-155 过时注释已按 RULE 处置(group_chat_loop.rs round-robin 项已由 `66ef6fa4` 先期修复,核对即可);本任务触碰文件内注释无新增违规。
- [ ] AC5(门):`cargo test -p everlasting --lib` 全绿(现基线 2343+)+ `cd app && pnpm test` 全绿(现基线 1652+)+ clippy/vue-tsc/fmt 净;不改 DAEMON-API 契约面(R1/R2 均为既有端点行为内变更,metadata 键 additive)。

## Out of Scope

- C2 证据链(结构化 summary / 锚点后校验 / 存量断言迁移)——另立任务。
- C1.3 wall-time 预算——共识明确缓做(ask-free 落地后观察)。
- M4 成本治理消费面(默认档 / MCP 工具参数 / $ 换算)——C1.2 只交 metadata 声明键 + GUI 通道;M1 script / MCP 建群 body 是固定键白名单,加参各自归 M4;M4a 定时场透传(scheduled 侧加字段 + fire 加键)归 M4。
- MCP start_discussion 的 token_budget 参数 schema 扩展(wire 预算重锁)——M4 一并做,本任务 metadata 键对 MCP 侧透传即可(script 直传已可用)。

## Decisions(2026-09-08 用户裁定,原 Open Questions 已收敛)

- **Q1 ask-free 范围 = 全讨论**(主持人段 + 参与者段;按 `group_chat_state` 单条件判定,完整满足不变量 #1;想给参与者全工具自由的用户对该 session 开 Yolo)。
- **Q2 预算口径 = 四计费字段求和**(input+output+cache_creation+cache_read;context_input 不计)。
- **Q3 GUI 面 = 本任务顺带加**(GroupChatConfigModal 可选输入,留空 = 不限)。

## Notes

- 规划方式:本任务设计大半由第二场共识预收敛(止损包 = 同 PR 两 commit 顺序敏感,C1.1 先合);本 PRD 做的是共识 → 码上现状逐项核对(Background 表)+ 三个余下决策收敛(Decisions)。
- 技术设计与执行清单见同目录 design.md / implement.md。
