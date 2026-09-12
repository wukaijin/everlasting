# MCP discussion_status 长轮询与进度富化(2026-09-13)

## 背景

GCE-M2 定案「工具绝不阻塞、进度靠轮询」后,宿主 LLM 只能盲轮 `discussion_status`
(busy/stop_reason/elapsed_s 三字段,一次调用常零信息)。daemon 层虽有 SSE 实时流
(§6.2),但 MCP/M1 刻意不挂——挂 SSE 观察者会破坏 GC3 无人值守 8s 快拒(权限 ask
变 120s)。用户要求:在不挂 SSE 的前提下让宿主能高效追踪进度(方案二)。

## 方案(用户 09-13 拍板;wire 预算锁按需上调)

- **B1 长轮询**:`discussion_status` 增可选 `wait_seconds`(int 1–30)。server 内部
  ~2s 拍 HTTP 轮询(pollSession + loadSession,非 SSE,GC3 性质不变),信号 =
  busy/stop_reason 翻转或消息数/末 seq 变化;变化即返,到点返 `wait_timed_out:true`
  (到点前必做一次实查,不空报超时)。终态在基线即返(转录惰性导出照旧)。
- **B2 进度富化**:`detail:true`(或 wait,隐含)时返回 `messages`(消息数)/
  `last_speaker`(末条带 speaker 的消息)/ `tokens`(与 result 同口径
  listTurnTraces→aggregateTokens,失败整键省略)/ `token_budget`(start 时落记账,
  仅声明时)。
- **预算锁**:实测 wire 字符后上调(原 3800/实测 3678;预计 +~350)。两处常量
  (TOOLS_BUDGET_CHARS / smoke BUDGET)+ 注释 + AGENTS.md + spec scripts 同步。
- **定案修订**:「工具调用绝不阻塞」不变式记录唯一有界例外 = wait_seconds ≤30s
  (避开宿主 MCP 工具超时)。

## 验收

1. `node --test scripts/group-chat-mcp.test.mjs` 全绿(新增:detail 字段/变化唤醒/
   到点超时/终态即返/wait 校验/记账 token_budget;AC4 断言同步新锁)。
2. `node scripts/group-chat-mcp-smoke.mjs` 非 live PASS(预算新值 + wait_seconds
   探针进错误链)。
3. wire schema 实测 ≤ 新锁,且 ≥ 旧实测(防 schema 误删)。
4. 文档同步:AGENTS.md GCE-M2 bullet、spec scripts/group-chat-mcp-deploy.md、
   .agents/skills/group-chat/SKILL.md、roadmap(如有 status 廉价轮询描述)。
