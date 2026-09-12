# 实施记录(2026-09-13)

## 改动面

- `scripts/group-chat-mcp.mjs`:
  - `coreStatus(deps, ledger, sessionId, { waitSeconds, detail })` 重写——
    基线快照 →(wait 时)循环:sleep(waitTickMs=2s)→ loadSession(消息面)+
    pollSession(busy 面)→ `statusSignal`(busy/stop_reason/消息数/末 seq)变化即返;
    到点前必做一次实查再返 `wait_timed_out:true`(边界变化不空报超时)。
  - `progressFields`(B2):messages / last_speaker(末条带 speaker 消息)/
    tokens(computeTokens 同口径,失败整键省略);`token_budget` 出记账
    (coreStart 落,仅声明时写键)。
  - `findSession` 带回 projectId(等待循环免重走兜底链);`realDeps` 增
    `sleep`/`waitTickMs` 可注入时钟。
  - wire:status shape 增 `wait_seconds`(int 1-30)/`detail`(bool)可选参;
    TOOLS_BUDGET_CHARS 3800→4200(实测 4093,用户预批)。
- `scripts/group-chat-mcp.test.mjs`:26→31 用例(detail 富快照+降级 / 消息面
  唤醒 / 真时钟到点超时 / 等待中翻终态+转录 / wait 校验+终态即返不等待 /
  记账 token_budget;AC4 描述+wire schema 断言同步)。
- `scripts/group-chat-mcp-smoke.mjs`:BUDGET 4200;2a 探针 = wait/detail 接线
  (不存在 id 基线即报)+ `wait_seconds:0` zod 越界拒。
- 文档:AGENTS.md GCE-M2 bullet、spec scripts/group-chat-mcp-deploy.md §6、
  SKILL.md 宿主指引(教 wait/detail 用法,「别裸轮询空转」)、roadmap §3 工具表
  +关键语义注 + §3 现状注(八工具/4200/31 用例)。

## 验证

1. `node --test scripts/group-chat-mcp.test.mjs` → 31/31 pass;
   `node --test scripts/group-chat-run.test.mjs` → 20/20 pass(引擎零改动回归)。
2. 非 live 冒烟 node 直连 PASS(wire 4093 < 4200;daemon 在跑态)。
3. 真 daemon 实链零成本探针:已完成场(session caa5020a)coreStatus
   `{detail:true}` → messages=61 / last_speaker=moderator / tokens total=458887
   (4 speakers)/ transcript_path 落位——真实消息形状与 progressFields 兼容。
4. standalone bin 重编译(挂载本就指向 bin,config 未触盘)+ `--bin` 冒烟
   PASS(新参数进产物;宿主新会话生效)。

## 已知边界

- wait 唤醒粒度 = 消息落地(发言轮完),非 delta 流——SSE 实时流仍在
  daemon(§6.2),MCP 面刻意不消费(GC3);到点前实查一次,时间戳精度 ±2s。
- 等待中的唤醒信号不含 token 增量维度(turn_trace 计数)——tokens 是快照
  附带字段,非独立信号(消息面变化与 turn 完成同步,足够)。
- 宿主工具超时未知方(ZCode/Claude Code)以 30s 上限保守规避;若某宿主
  超时 <30s,该宿主应用小值 wait 或退回裸轮询。
