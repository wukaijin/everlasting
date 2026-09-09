# C2 证据链:群聊结构化 summary(锚点+推测标注+锚点后校验)

## Goal

让群聊审议的收官结论**可信度可分层、证据可核验**:moderator 的 `end_discussion` 从纯自由文本升级为结构化结论(每条主张挂 file:line 锚点 + 实证/推测/争议标注,开放问题单列),收束落库前对锚点做后校验(文件存在、行号在界内),外部消费方(MCP `discussion_result`、GUI 收官卡、双转录导出器)机器可读地区分「实证结论」与「推测/待核」。

**用户价值**:外部 agent(GCE M2)消费讨论结论时不再把模型推测当事实;人读收官卡/转录时证据锚点附核验状态。本任务落地后,GROUP-CHAT-API-ROADMAP §6 依赖矩阵最后一项(C2.1 → M2 `discussion_result` 结论可信度)清零。

## 背景

- 共识来源:第二场 live 群聊(session `eb14d2df`,2026-09-06)。原 session 已清理,以 [ROADMAP §6](../../../../docs/GROUP-CHAT-API-ROADMAP.md) 记账行 + [09-08 止损包 PRD](../archive/2026-09/09-08-gc-c1-stoploss-regression-gate/prd.md) 三件套(结构化 summary / 锚点后校验 / 存量断言迁移)为准。
- 前置已就绪:C1.1/C1.2/C3.1 止损包 + MockProvider 剧本回归闸在 `cargo test --lib`;`GroupChatCtx.project_root`(D1 修复)提供锚点校验路径基准。

## 已定决策(用户裁定 2026-09-09)

| # | 决策 | 结论 |
|---|------|------|
| Q1 | 结构形态 | **schema 结构化参数**:`end_discussion` 增 `conclusions[{claim, anchors[{path, line?}], stance}]` + `open_questions[]`,`summary` 文本保留(叙事+兜底);结构化 JSON 落库新列;锚点校验结果并入结构化数据 |
| Q2 | 无效锚点处置 | **只标注不修改**:锚点附 check 结果(ok / not_found / line_out_of_range / outside_root / unvalidated),不改写 moderator 的主张与 stance,断证信号留给消费方 |
| Q3 | 消费面范围 | **全消费面**:GUI 收官卡结构化渲染 + MCP `discussion_result` 加 detail 字段 + Rust/JS 双转录器结构化节;讨论库面板维持文本摘要(不做结构化 chips) |

## Requirements

### R1 结构化 summary 生产(后端)

- `end_discussion` 工具 schema(`tools/end_discussion.rs:21-39`)增可选 `conclusions`(数组,元素 = `claim` 必填 + `anchors` 可选数组{`path` 必填, `line` 可选} + `stance` 枚举 verified/inferred/disputed)与 `open_questions`(字符串数组);`summary` 参数保留。全部可选 = 缺省零行为变更(既有 MockProvider 剧本发空 `{}` 不破坏)。
- intercept(`execute_intercept`)宽容解析:结构非法时丢弃结构化部分、保留 summary,不报错(收官路径不因格式挂)。
- moderator prompt 三处教学(基础 system prompt 收束条款 / preempt wrap-up 指令 / resume 指令):实证(读过代码)= verified + 锚点;推理未亲证 = inferred;未达共识 = disputed;未决 = open_questions(`agent/group_chat_prompts.rs:40-119`)。

### R2 锚点后校验(后端)

- 编排器退出、`finalize_group_chat_lifecycle` 之前校验(`agent/group_chat_loop.rs:1086-1114` 区间,project_root 取自 ctx):
  - root 缺失 → 全部 unvalidated(detail 仍落库);
  - 路径 resolve 后在 project_root 外 → `outside_root`,**不触 fs**(编排器直接读文件绕过沙盒模型,root 外一律不读);
  - root 内:不存在/是目录 → `not_found`;存在且带 line → 流式数行(16 MiB 字节帽,超帽 unvalidated),line 越界 → `line_out_of_range`,否则 `ok`。
- 校验永不 fail 整场:任何 IO 错误 → 该锚点 unvalidated,finalize 照常。

### R3 落库与行模型(后端)

- `sessions.discussion_detail TEXT`(JSON)additive 列(先例 `add_session_column_if_missing`,`db/migrations/schema.rs:493-499`)。
- `finalize_group_chat_lifecycle`(`db/sessions/session_crud.rs:953-971`)增 `discussion_detail = COALESCE(?, discussion_detail)`;`clear_group_chat_lifecycle`(:919-933,复用场重置)同步清 `discussion_detail = NULL`——**漏清会让续跑场带上一场的断证结论**。
- `SessionRow` 增 `discussion_detail: Option<String>`(SELECT :253 + try_get);JSON 键 snake_case(与 sessions 域 TS 字段 `discussion_summary` 同惯例,区别于 providers 域 camelCase)。

### R4 消费面(全)

- **GUI 收官卡**(`DiscussionSummaryCard.vue`):live 期从 `ToolCallInfo.input`(已解析的 tool_use 参数,`chat.types.ts:39-43`)渲染 conclusions(stance 徽章:✅实证/💭推测/⚖争议 + 锚点行 + check 记号)+ open_questions;收官后从 store session 合并的 `discussion_detail`(同 `streamEvents.ts:1466-1476` stop_reason/discussion_summary 受控合并模式)叠加核验记号(按 path+line 匹配);无 conclusions 入参 → 现行 markdown 文本渲染兜底(旧场/朴素收官零回归);结构化在场时 summary 叙事全文仍渲染。
- **MCP `discussion_result`**(`scripts/group-chat-mcp.mjs:227-243`):输出加 `detail`(解析后的结构化对象|null)。仅输出扩展,**不动 start_discussion 入参 schema,wire 预算锁 3200 不涉**(锁的是工具输入 wire)。
- **双转录导出器**:Rust `render_scheduled_transcript`(`agent/group_chat_transcript.rs:110`,M4a 定时场)与 JS `renderTranscript`(`scripts/group-chat-run.mjs:344` 一带,M1/MCP 共享)各加 `## conclusions` / `## open_questions` 节(detail 缺失则省略),锚点带 check 记号。

### R5 存量断言迁移(收窄)

记账原文「存量断言迁移」在 additive 设计下收窄为:既有 8 处自由文本断言(`agent/tests_group_chat.rs:1074/:1189/:1351/:1648/:1808/:1950/:2090/:2442`)**零改动全绿即兼容证明**;另升级 ≥1 个 MockProvider 剧本发结构化 end_discussion 覆盖持久化链路 + 新增专项剧本(结构化/校验各臂)。

### R6 兼容

- 旧场(discussion_detail NULL)全消费面走文本兜底;`end_discussion` 只发 summary(无结构化参数)= 现行为不变;DAEMON-API 契约只增不改。

## Acceptance Criteria

- [x] AC1(结构化生产):moderator 收束产出结构化 conclusions(含锚点与 stance)+ open_questions;prompt 教学锚点进 `tests_group_chat_prompts`;MockProvider 剧本覆盖 verified+锚点 / inferred 无锚点 / disputed / 开放问题四形态。
- [x] AC2(锚点校验):五臂单测全绿(ok / not_found / line_out_of_range / outside_root 不触 fs / 无 root 全 unvalidated);IO 错误不 fail 整场;校验结果落 detail。
- [x] AC3(落库与复用):finalize 落 `discussion_detail`;**复用场重置清空 detail** 有专项断言;SessionRow 带回;存量 8 断言 + 19 既有群聊用例零改动全绿(缺省零行为变更)。
- [x] AC4(消费面):GUI 卡结构化渲染(stance 徽章 + 锚点 check 记号 + 无结构兜底)vitest 过;MCP result detail + JS 转录节 node --test 过;Rust 转录节单测过;旧场(mock 无 detail)四消费面零回归。
- [x] AC5(门禁):`cargo test -p everlasting --lib` 全绿(基线 2343+)+ `cd app && pnpm test` 全绿(基线 1652+)+ clippy/fmt/vue-tsc 净 + `node --test scripts/group-chat-run.test.mjs`、`node --test scripts/group-chat-mcp.test.mjs` 全绿。
- [x] AC6(live):一场 `group-chat-run` 实跑,收官行带 detail、转录含 conclusions 节、锚点校验结果与仓库实际状态一致(抽查 ≥2 锚点)。

## Out of Scope

- 讨论库面板(`DiscussionLibraryModal`)结构化 chips——文本摘要与 LIKE 检索维持。
- 锚点点击跳转源码的 GUI 机制(若实现期发现现成 file-link 原语可低成本复用则顺带,否则不做)。
- MCP `start_discussion` 入参变更(wire 锁 3200 不动);远程暴露认证(M4 余项);C1.3 wall-time 预算(共识缓做)。
- discussion_summary 文本从 detail 机械生成(两字段独立,moderator 各自产出)。
- 讨论过程内(非收束时)逐轮证据链;summary 质量的模型侧调优。

## Technical Notes(勘察锚点)

- 生产链路:`end_discussion({summary})` → `SharedTurnState.end_summary`(`tools/nominate_speaker.rs:42`)→ 编排器 `finalize_group_chat_lifecycle` → `sessions.discussion_summary`。
- 收官卡数据源现状:tool_result 信封(`extractToolResultDisplay`),非 session 行——结构化需走 `call.input` + store 合并双通道(见 R4)。
- finalize 后 store 合并先例:`streamEvents.ts:1466-1476`(P1a 终局字段受控合并,detail 照抄该模式)。
- MockProvider 剧本现发空 `{}`(`tests_group_chat.rs:169`)——additive 参数天然兼容。
