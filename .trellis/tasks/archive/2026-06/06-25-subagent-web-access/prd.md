# subagent 联网能力（worker web_fetch）

> **状态**：brainstorm 收敛（2026-06-25），待 Final Confirmation → Phase 2。

## Goal

让 subagent（researcher + 并发 worker）具备联网能力，支持"并行联网调研"场景（如并发搜索多个外部项目的更新 / changelog）。

## 触发（2026-06-25）

L3a 验证时：用户要求"并发 3 个 researcher 搜索 opencode / hermes / openhands 最近更新"，3 个 worker 都报告无 web_fetch / 无联网能力（sub-agent 正常 completed，只是都汇报不能联网）。

## 根因诊断（三层叠加）—— 基线验证版（2026-06-25）

worker 三层都不能联网，但**第 3 层与种子 PRD 描述不同**：

### 第 1 层 ✅ 准确：researcher SubagentDef.tools 不含 web_fetch
`subagent/mod.rs:205` —— `tools: &["read_file", "grep", "glob", "list_dir"]`。researcher 设计为本地只读探索。general-purpose 的 allowlist 为空（`mod.rs:230`，继承 builtin_tools() 全集），含 web_fetch。

### 第 2 层 ✅ 准确：force_readonly 并发剥掉 web_fetch
`subagent/mod.rs:404` —— `READONLY_TOOL_ALLOWLIST = &["read_file", "grep", "glob", "list_dir"]`。L3a 把"只读并发"定义为 4 个本地工具，`filter_tools_readonly`（`mod.rs:420`）剥掉 web_fetch。并发路径下 general-purpose 也被剥成同样 4 个（用户观察到的"general-purpose 和 researcher 工具集一样"）。

### 第 3 层 ❌ 种子 PRD 过时：worker ask 已不塌缩 Deny
种子 PRD 写"worker is_worker=true 调 web_fetch → ask_path → 塌缩 Deny（worker 无 UI sink，ask 会挂起，故塌缩）"。**这是 2026-06-20 PR2b 时的行为，已被 2026-06-22 RULE-FrontSubagent-003 fix 推翻**：
- `dispatch.rs:342-343` 的"collapse to Deny"注释是旧描述，同文件 `dispatch.rs:366` 自己标注"pre-2026-06-22"
- 当前 worker ask 走 **WorkerAskBanner**（前端 `components/chat/WorkerAskBanner.vue` 实存 + test）：`ask.rs:124` is_worker 分支 → 注册 oneshot → 3-arm `biased` select（parent cancel / 120s `ASK_TIMEOUT` / oneshot 响应），用户可 AllowOnce / AllowAlways / Deny

**关键决定性发现 —— 方案 D（父 session grant 继承）天然已工作，无需新代码**：
worker 的 `PermissionContext.session_id` = `parent_session_id`（`dispatch.rs:314` 传 parent_session_id 给 worker run_chat_loop → `chat_loop.rs:411-412` `PermissionContext.session_id = session_id` 参数）。而 `check.rs:257` 的 `check_tool_grant(db, &ctx.session_id, "web_fetch")` 查 `session_tool_permissions` 表里该 session 的 web_fetch grant（`db/permissions.rs:128-134` `WHERE session_id = ? AND tool_name = ?`）。**所以 worker web_fetch 已经天然继承父 session 的 web_fetch grant**：
- 父 session 有 web_fetch grant（用户在主对话对 web_fetch 点过"始终允许"）→ worker web_fetch **自动 Allow**（`check.rs:257` 命中）零 banner
- 父 session 无 grant → worker web_fetch 弹 WorkerAskBanner（AllowOnce / AllowAlways / Deny；**AllowAlways 不持久化** —— `ask.rs:267-273` 有意设计，防 worker 跨权限边界写父 session grant）

→ **L3a 验证时 worker 报"无 web_fetch"，纯粹是第 1+2 层把工具从 toolset 剥掉了，worker 根本没机会触发第 3 层 ask。**

## Requirements

1. **第 1 层**：`researcher` 的 `SubagentDef.tools` 加 `web_fetch`（`subagent/mod.rs:205`）
2. **researcher system_prompt 同步**：`mod.rs:192-203` 的"You have access to `read_file`, `grep`, `glob`, and `list_dir`"补 `web_fetch`（否则 LLM 不知自己有联网工具）
3. **第 2 层**：`READONLY_TOOL_ALLOWLIST` 加 `web_fetch`（`subagent/mod.rs:404`）—— web_fetch 是只读网络操作、`Risk::Low`（`permissions/types.rs:71`）、有 SSRF 防护（`tools/web_fetch.rs`），符合"只读并发"语义（无本地副作用，并发 N 个独立 GET 无共享状态竞争；对照 L2 排除 web_fetch 仅因 Tier4 默认 ask 的 UX 考虑，非安全性）
4. **第 3 层零改动**：复用现有逻辑（父 session grant 命中 → 自动 Allow；否则 WorkerAskBanner）

**影响面确认**：`READONLY_TOOL_ALLOWLIST` 只被 `filter_tools_readonly` 引用，而后者只在 `dispatch.rs:157`（force_readonly 并发路径）调用；L2 单 turn 并发用独立谓词 `is_parallel_eligible`（`chat_loop.rs:1439`），不引用本常量 → 改动不波及 L2。

## Acceptance Criteria

- [ ] `builtin_subagents_researcher_tool_allowlist`（`mod.rs:504`）断言 researcher.tools 含 web_fetch
- [ ] `filter_researcher_keeps_only_read_tools_and_strips_disabled`（`mod.rs:535`）涉及 web_fetch 的断言更新（researcher 现在 keep web_fetch）
- [ ] `l3a_filter_tools_readonly_keeps_only_four_read_tools`（`tests_subagent.rs:1914`）改名 + 断言 `filter_tools_readonly` 保留 web_fetch（5 个只读工具）
- [ ] researcher system_prompt 列出 web_fetch
- [ ] `cargo test`（PKG_CONFIG_PATH）全绿
- [ ] spec 更新：`tool-contract.md` / `permission-layer.md` 补"worker 继承父 session web_fetch grant"
- 手动验证（端到端难单测）：worker web_fetch 在父 session 有 grant 时自动 Allow；无 grant 弹 WorkerAskBanner

## Definition of Done

- 上述单测更新 + 新增 force_readonly 保留 web_fetch 断言
- `cargo check` + `cargo test` 绿
- spec 文档更新（行为变化）
- 无安全语义变化（第 3 层纯复用现有，无 silent allow / 无 worker 写父 grant）

## Technical Approach

最小改动：2 处常量 + 1 处 prompt 文本 + 测试断言 + spec。第 3 层复用现有 WorkerAskBanner + 父 grant 继承（已天然工作，`check.rs:257` 已查父 session grant）。

## Decision (ADR-lite)

- **Context**：worker 三层无联网；基线验证发现第 3 层（worker ask）经 2026-06-22 RULE-FrontSubagent-003 fix 已走 WorkerAskBanner，且 worker `ctx.session_id` = 父 session id 使方案 D（父 grant 继承）天然工作。任务实质从"三层都要改"缩小为第 1+2 层 2 处常量改动。
- **Decision**：最小 MVP —— 仅第 1+2 层常量 + prompt 改动，第 3 层零改动复用现有父 grant 继承 + WorkerAskBanner。
- **Consequences**：
  - 父 session 无 grant 时并发 N worker 弹 N banner（AllowAlways 不持久化）—— 接受现状，workaround 是用户预先在主对话对 web_fetch 点"始终允许"让所有 worker 继承
  - `silent allow` / `AllowAlways 持久化` / 联网配额 作为 follow-up（各自需独立安全 grill）

## Implementation Plan

- **PR1（核心）**：`mod.rs` —— researcher `SubagentDef.tools` + system_prompt 加 web_fetch；`READONLY_TOOL_ALLOWLIST` 加 web_fetch；更新 `mod.rs` 2 个 researcher 测试断言 + 改名/更新 `tests_subagent.rs` 的 `l3a_filter_tools_readonly_*` 测试
- **收尾 commit**：spec 文档（`tool-contract.md` / `permission-layer.md`）+ `IMPLEMENTATION.md §4` 决策日志 + `ROADMAP.md` L3c 收口

## 安全考量

worker 自主联网 = LLM 决定抓什么 URL。已防护：
- SSRF（RFC 1918 / loopback / link-local / CGNAT / multicast / reserved + 169.254.169.254 短路，`web_fetch.rs`）
- 5 MiB body cap + rustls TLS + 30s timeout
- 审计可记录（C4 AuditKind，worker resolve 走 `WorkerAskAllowed` / `WorkerAskDenied` / `WorkerAskTimedOut` / `WorkerAskCancelled`）

## Out of Scope

- 并发机制本身（L3a 已完成，`06-24-l3a-readonly-concurrent`）
- worktree 隔离（L3b）
- worker web_fetch silent allow（需独立安全 grill：数据外泄 / 配额）
- worker AllowAlways 持久化到父 session grant（破坏现有跨权限边界设计）

## 关联

- L3a task：`.trellis/tasks/06-24-l3a-readonly-concurrent/`
- web_fetch spec：`.trellis/spec/backend/tool-contract.md` §web_fetch tool
- 权限层：`.trellis/spec/backend/permission-layer.md`（is_worker Tier4 + WorkerAskBanner）
- 关键代码：`subagent/mod.rs:205/404` / `permissions/check.rs:253-274` / `permissions/ask.rs:124-290` / `dispatch.rs:314/339-379`
