# Error Handling

> How errors are handled in this project.

---

## Overview

<!--
Document your project's error handling conventions here.

Questions to answer:
- What error types do you define?
- How are errors propagated?
- How are errors logged?
- How are errors returned to clients?
-->

(To be filled by the team)

---

## Error Types

<!-- Custom error classes/types -->

(To be filled by the team)

---

## Error Handling Patterns

<!-- Try-catch patterns, error propagation -->

(To be filled by the team)

---

## API Error Responses

<!-- Standard error response format -->

(To be filled by the team)

---

## Common Mistakes

<!-- Error handling mistakes your team has made -->

### RULE-ERR-SURFACE-001（2026-09-01，session 2e438939）：吞错必须对 LLM 可见

**事故**：`resolve_current_task` 按容错策略跳过解析失败的 `task.json`（只 warn 日志），workflow 会话因此每轮都解析为「无 active task」；同时 `create_task` 的 AlreadyExists 报错指向一个不存在的「open the existing one」工具。LLM 被两个互斥的守卫卡死，烧了 ~25 条消息后靠 `rm -rf` 任务目录脱困。同 session 另发：子代理流中网络错误后整 run 报废、进度全丢，parent 从零重派。

**规则**：

1. **面向 LLM 的恢复路径上，任何被吞掉的错误必须透传到模型可见的通道**（breadcrumb / tool_result / system 注入），不能只进 daemon 日志——日志模型看不见，等于让引擎和模型活在两个世界里。
2. **错误提示只允许指向真实存在的工具或动作**；提示里出现的每个「下一步」都必须是模型可执行的调用。
3. **自愈型容错（serde default / lenient parse）与显式报错互补**：读侧对常见手写缺口给 default，仍解析失败的文件把 `(slug, 原始错误)` 透出到 breadcrumb 让模型修文件。
4. **worker 错误退出时若历史仍 pair-safe（合成 tool_result 补对 + marker），保留 messages 供 `resume_from` 续跑**，并在 tool_result 里明确告知 parent 怎么续（见 `drive.rs` 09-01-subagent-network-resume 块）。

---
