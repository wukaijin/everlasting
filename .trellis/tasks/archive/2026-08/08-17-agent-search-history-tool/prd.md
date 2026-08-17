# D2② Agent 驱动 search_history tool

## Goal

ROADMAP D2(跨 session 全文搜索)双驱动的 ②:给 agent 一个 `search_history` tool,
让它能检索**全部 project 的历史消息全文**(`messages.text` + `sessions.title`)。
① 用户驱动 SearchModal 已于 08-17 落地(`08-17-cross-session-search`);本任务补上
agent 侧入口,复用同一 `db::search::search_messages` 查询层(SQL 层零改动,不走 IPC)。

典型场景:用户问「上次我们怎么解的 X」「之前讨论过 Y 吗」「那个决策是什么」——
agent 现在只有自主记忆(提炼后的经验),没有原始对话全文检索。

## Requirements

- **R1 tool 注册**:新 `tools/search_history.rs`(照 `remember.rs` 模板:definition +
  execute,经 `ToolContext.db` 直查),`builtin_tools()` 注册 + `execute_tool_inner`
  match arm。**不走** chat_loop 拦截模式(非 blocking tool,普通 dispatch 即可)。
- **R2 输入 schema**:
  - `query: string`(必填,trim 后非空;<3 字符自动走 LIKE 兜底,同 ① 语义);
  - `scope: "all" | "current_project"`(可选,默认 `all`;映射为 `search_messages`
    的 `project_id: Option<&str>`,current_project 取 `ToolContext.project_id`);
  - `limit: number`(可选,默认 20,clamp 1..=50 —— agent 侧比 modal 的 200 上限更紧,
    控制 tool_result token)。
- **R3 输出格式**:LLM 消费的紧凑文本,非 `MessageSearchHit` JSON 直吐(design.md ①
  已定:「给 LLM 的精简摘要 vs 给用户的 snippet/高亮偏移」不同)。每 hit 一行:
  序号 + 日期 + project 名 / session 标题 + (seq, role) + snippet;title 命中单独
  标注。当前 session 的命中加 `(this session)` 标记(模型上下文里已有,提示可跳过)。
  零命中返回 `is_error: false` 的明确文案(含收窄/放宽建议)。
- **R4 权限零改动**:`ToolKind::Other` 默认 Tier 5 silent Allow(同 `remember`),
  `Risk::Low` 默认;plan 模式保留(`filter_tools_for_mode` 是写类黑名单,只读工具
  天然留下);不在 kill list。**权限层一行不改**,PRD 记录此决策即可。
- **R5 worker 可用性**:不进 `STRUCTURALLY_DISABLED`(serial general-purpose worker
  自动获得);**加进 `READONLY_TOOL_ALLOWLIST`**(语义就是只读 DB 查询,并发
  readonly worker 剥离后应保留;连带更新 tools_filter.rs 里「researcher allowlist
  恰等于 READONLY_TOOL_ALLOWLIST」的过时注释)。researcher builtin 的 `tools` 硬编码
  5 项**不动**(动它要连带改 system_prompt 枚举文案,超本任务范围;frontmatter
  agent 可自行声明)。
- **R6 群聊不扩**:`GROUP_CHAT_RESEARCH_TOOLS` 白名单**不加**(08-07 群聊工具白名单
  收敛是有意决策,扩员留 follow-up 评估)。
- **R7 C7D stub 不涉及**:schema 3 参数 + 紧凑 description,非 stub 候选;但注意
  注册新 tool 会给每轮 tools[] 增加 ~200-300 tok,description 必须克制。

## Out of Scope

- 群聊参与者/主持人白名单扩员(R6)。
- researcher builtin 定义与 system_prompt 修改(R5)。
- 按时间过滤 / role 过滤 / 分页翻页(MVP 单页 20 条足够;加参 = 加 schema token)。
- 前端任何改动(① 已交付用户侧;本任务纯后端 tool)。

## Acceptance Criteria

- [x] AC1:`search_history` 出现在 `builtin_tools()` 注册序末尾;`execute_tool_inner`
      有对应 arm;普通(非拦截)路径可执行。
- [x] AC2:≥3 字符走 FTS、<3 字符走 LIKE 的分流对 agent 侧透明成立(继承
      `search_messages` 契约,tool 层不做二次分派)。
- [x] AC3:scope=current_project 只命中当前 project;scope=all(默认)跨 project。
- [x] AC4:limit 默认 20、clamp 到 50;空 query 返回 `is_error: true` + 可行动文案;
      零命中返回 `is_error: false` + 建议。
- [x] AC5:输出为紧凑多行文本,含 project 名 / session 标题 / 日期 / seq / role /
      snippet;title 命中带 kind 标注;当前 session 命中带 `(this session)`。
- [x] AC6:权限链零改动(无 check.rs / permission.rs / dangerous.rs diff);
      `filter_tools_readonly` 剥离后保留 `search_history`(READONLY_TOOL_ALLOWLIST
      含它)。
- [x] AC7:`cargo test -p everlasting --lib` 全绿(新增 tool 单测覆盖 AC2-AC5);
      `cargo fmt --check` + `cargo clippy` 无新增告警。
- [x] AC8:spec 更新 — `.trellis/spec/backend/tool-contract.md` 增 `search_history`
      scenario(签名 / Tier 5 silent Allow 依据 / 复用 db::search 共享层 / agent 侧
      limit 50 vs modal 200 差异)。

## Notes

- 来源:`docs/ROADMAP.md` §1.2 D2 行、`08-17-cross-session-search` prd.md L53 /
  design.md L32/L180(「共用 db::search.rs 的 SQL 层即可,IPC 层各自薄封装——
  follow-up 任务再定工具契约」——本 PRD 即定下该契约)。
