# subagent 联网能力（worker web_fetch）

> **状态**：种子 prd（从 L3a 验证拆出的独立 task，待 brainstorm 展开）。占位，未 start。

## Goal

让 subagent（researcher + 并发 worker）具备联网能力，支持"并行联网调研"场景（如并行搜索多个外部项目的更新/changelog）。

## 触发（2026-06-25）

L3a 验证时：用户要求"并发 3 个 researcher 搜索 opencode/hermes/openhands 最近更新"，3 个 worker 都报告无 web_fetch / 无联网能力（sub-agent 正常 completed，只是都汇报不能联网）。

## 根因诊断（三层叠加，非单点）

worker 三层都不能联网：

1. **researcher 定义**：`SubagentDef.tools = [read_file, grep, glob, list_dir]`（`subagent/mod.rs:205`）——设计为本地只读探索，无 web_fetch。
2. **并发 force_readonly**：`READONLY_TOOL_ALLOWLIST = [read_file, grep, glob, list_dir]`（`mod.rs:404`）——L3a 把"只读"定义为 4 个本地工具，剥掉 web_fetch；general-purpose 一进并发也被剥成同样（用户观察到的"general-purpose 和 researcher 工具集一样"）。
3. **worker 权限**：即使 web_fetch 在 toolset，worker `is_worker=true` 调它（无 `session_tool_permissions` 预授权）→ `ask_path`（`permissions/check.rs:261` WebFetch 分支）→ is_worker 塌缩 `Deny`（worker 无 UI sink，ask 会挂起，故塌缩）。

→ 并发 worker（researcher 或 general-purpose）**必然无联网**。这是三个独立范围决策的叠加，不是 bug。

## 范围（待 brainstorm 收敛）

让 worker 联网的方案候选：

- `researcher` 的 `SubagentDef.tools` 加 `web_fetch`
- `READONLY_TOOL_ALLOWLIST` 加 `web_fetch`（并发 `force_readonly` 保留 web_fetch）——web_fetch 是只读网络操作、`Risk::Low`（`permissions/types.rs:71`）、有 SSRF 防护（`tools/web_fetch.rs`），符合"只读"语义
- worker web_fetch 权限：解决 is_worker `Deny`——worker web_fetch silent allow（risk=Low + SSRF 已防护，worker-safe）/ 预置 grant / 走父 session grant

## 安全考量（待 brainstorm）

worker 自主联网 = LLM 决定抓什么 URL，用户不可见（worker 无 UI sink）：

- SSRF 已防护（RFC 1918/loopback/link-local/CGNAT 等，`web_fetch.rs`）
- 审计可记录（C4 AuditKind）
- 待评估：worker 抓敏感 URL / 数据外泄 / 联网配额

## Out of Scope

- 并发机制本身（L3a 已完成，`06-24-l3a-readonly-concurrent`）
- worktree 隔离（L3b）

## 关联

- L3a task：`.trellis/tasks/06-24-l3a-readonly-concurrent/`（并发机制，PR1 已完成 + 前端天然 work）
- web_fetch spec：`.trellis/spec/backend/tool-contract.md` §web_fetch tool
- 权限层：`.trellis/spec/backend/permission-layer.md`（is_worker Tier4 塌缩）
