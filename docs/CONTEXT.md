# CONTEXT.md

> Everlasting 项目术语表(glossary)。
> 本文件是 **glossary,只定义术语**;实现决策(schema / 写入时机 / 颜色阈值等)走 `docs/IMPLEMENTATION.md §4` 决策日志,本文件不重复。

---

## 术语表

### Turn (LLM turn)
一次 LLM HTTP 请求(Anthropic Messages API / OpenAI Chat Completions 一次 stream)。
一个用户消息可能引发 N 次 turn(主调 + tool_use 回填),受 agent loop `MAX_TURNS`(200)限制。

### TokenUsage
LLM 一次响应的 token 使用四元组(Anthropic schema 视角):

- **`input_tokens`** — 当次请求中送入的 token 数,**已包含** `cache_creation_input_tokens` + `cache_read_input_tokens`(Anthropic 语义)
- **`output_tokens`** — 当次响应生成的 token 数
- **`cache_creation_input_tokens`** — 当次请求中**新创建**的 cache token(下次可命中)
- **`cache_read_input_tokens`** — 当次请求中**命中**的 cache token

OpenAI Chat Completions 的归一化映射(在 Provider 层完成,ChatEvent 出来时已统一):

- `prompt_tokens` → `input_tokens`
- `completion_tokens` → `output_tokens`
- `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
- `cache_creation_input_tokens` → `0`(OpenAI 暂无对应字段)

### Context Pressure (上下文压力)
**当前 context 窗口的占用比例**。定义为:

- 分子 = session 累计 `input_tokens`(sum over turns)
- 分母 = `ModelRow.context_window`(默认 200K)

`input_tokens` 已包含 cache_creation + cache_read,所以 cache 命中**不重复计**——使用 cache 会让压力增长更慢。`output_tokens` **不计入** context 压力(那是响应,不是 context)。

### Cache Hit (cache 命中)
LLM 一次请求中,从 prompt cache 读回的 token(`cache_read_input_tokens`)。计费按 Anthropic / OpenAI 各自规定(Anthropic `cache_read_input_tokens` 按 0.1x input 价;OpenAI `cached_tokens` 按 0.5x input 价)。

### Context Window (上下文窗口)
LLM 模型能处理的最大 input token 数(Anthropic Sonnet / Opus 默认 200K)。数据来源:`ModelRow.context_window` 列,seed 时硬编码。

### Per-session 累积 (Token 统计颗粒度)
Token 统计在 DB 层的存储颗粒度为 session 维度:`sessions` 表的 4 列(input_tokens_total / output_tokens_total / cache_creation_total / cache_read_total)。每次 LLM turn Done 时单条 SQL UPDATE 累加。

### Anthropic SSE Usage
Anthropic Messages API 的 token 用量在 SSE 流的 `message_delta` 事件中携带(`usage: { input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }`),累计语义,本 turn 累计。

### OpenAI Stream Usage
OpenAI Chat Completions 的 token 用量在流末尾携带(`usage: { prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details: { cached_tokens } }`),**仅在请求体发送 `stream_options: { include_usage: true }` 时**返回。

### Checklist (agent 自跟踪清单)
> **实现状态**:**B12 已落地(2026-06-19)**。TodoWrite 式 `update_checklist` tool — 全量替换(单次调用覆盖完整清单,非增量 diff)+ 三态 `pending` / `in_progress` / `done` + 至多一 `in_progress` coerce(LLM 误标多个 in_progress 时收一)+ loop-local Vec(per-request,通过 `ToolContext` 传,不污染持久化 messages)+ 每轮 ephemeral **append** 注入请求副本(不入持久化,保 memory cache breakpoint 不断)。前端 `<ChecklistCard>` ChatPanel 浮层(展开 / 最小化悬浮球 + 焦点动效)+ checklist store(客户端复刻 coerce)。无新 DB 表(replay 从 DB history 还原,reload 按 `is_error` 过滤 cancel 合成 result)。PR1 `994db84` + PR2 `1896470` + PR3 spec;决策见 [IMPLEMENTATION §4 2026-06-18](../IMPLEMENTATION.md#4-决策日志)。

LLM 在跑复杂多步任务时维护的**结构化进度清单**——agent 自己写、改、标记完成,用于不丢失自己的计划与进度。对齐 Claude Code 的 `TaskCreate/TaskList`、opencode 的 `todowrite`、Cline 的 plan-act。

**不是什么**(本项目内这几个词都已占用,需消歧义):
- **不是** Trellis task(`.trellis/tasks/`,dev-workflow 的 PRD / 排期任务)
- **不是** plan mode(`Mode::Plan`,权限模式,拒 tool_use)
- **不是** subagent(B6,main agent 派 worker agent,独立 context + summary 回填)

典型形态:agent 在一个任务的多 turn run 中反复更新它,每轮把当前清单重新注入 context,从而"看到自己还剩什么没做"。

---

### Subagent / dispatch_subagent
父 session 通过 `dispatch_subagent` tool 派 worker agent 跑独立任务。worker 拥有**独立 context + token 预算**,完成 / 取消 / 失败后回填 summary。B6(2026-06-18/20/21)落地,L3a-d 持续扩展(并发只读 / worker worktree 隔离 / worker 联网 / frontmatter loader)。`app/src-tauri/src/agent/subagent/dispatch.rs` 实现 `run_subagent`。

### SubagentRun
`subagent_runs` 表一行(`migrations.rs:1137`),完整 schema: `id` / `parent_session_id` / `parent_request_id` / `subagent_name` / `status` / `started_at` / `finished_at` / `task` / `final_text` / `summary` / `turn_count` / `token_usage_json` / `transcript_json` / `transcript_truncated` / `worktree_path` / `isolation`(L3b PR1 起)。

- **status** ∈ `{running, completed, cancelled, error, incomplete}`(终态 4 个,无 `failed`)
- `transcript_json` 持久化整段 transcript + `transcript_truncated` 哨兵(超限截断)
- app 启动时 `reap_orphaned_runs` 把上一进程崩溃留下的 `running` 标记为 `error`(防止假 running)

### Worker Worktree
L3b PR1-PR4(L3b = subagent isolation 维)落地的 worker 隔离机制:

- branch 前缀 `worker/<run_id>`,base = parent session worktree HEAD
- `git worktree lock` 跑期间,`destroy_worker` 变体(self-heal 复用 session 变体的三态恢复)
- `SubagentDef.isolation: Option<bool>` 字段 — builtin `general-purpose: Some(true)` / `researcher: None`
- worker 完成 → `git::diff::diff_worker_worktree` 判 changes:
  - 有 → 保留 branch + diff summary 回填,前端 `<WorkerMergeControls>` 触发 `merge_worker` / `discard_worker`
  - 无 → destroy
- `merge_worker`:libgit2 fast-forward / 3-way merge,冲突 → 返 conflict 文件列表 + `is_error: true` + 保留两边 branch(用户手动 resolve)
- `discard_worker`:销毁 worker worktree + branch + clear `subagent_runs.worktree_path` 列
- 启动 sweep(`sweep_stale_worker_worktrees`):libgit2 扫 worker dir + `is_locked()` 跳过 active + mtime > N 天 destroy,`EVERLASTING_CLEANUP_PERIOD_DAYS` env 覆盖,默认 7 天对齐 Claude Code

### BackgroundShell (L1a 后台 shell)
`run_background_shell` 启动后台 shell(tokio Child,**不带 PTY**,L1b follow-up 接 `portable-pty`),`shell_status` 拉 exit_code,`shell_kill` 终止。

- `BackgroundShellRegistry` trait(Q1 决策 C,daemon 化换 impl 不动调用点)+ `InMemoryBackgroundShellRegistry` 进程内 impl(tokio 后台 task 拥有 Child,三触发 `select!`:`child.wait` / `kill_rx` / `sleep(max_runtime)`)
- 默认 `max_runtime_ms` 24h(Q6 决策),session-scoped(Q7 决策)
- `run_background_shell` Tier 4 Shell(同 shell,Plan 拦)/ `shell_status` / `shell_kill` Tier 5
- agent loop 每轮 `drain_notifications` + **APPEND** user message(Q3 决策,opencode-pty 风格,APPEND 非 prepend 保 memory cache breakpoint,同 B12 checklist 不变量)
- 通知仅 `exit_code`(Q4 决策),LLM 主动 `shell_status` 拉详情
- 30KB spill + 1KB head/tail preview 复用 `shell.rs`
- 生命周期:`delete_session` → `kill_all_for_session`;`RunEvent::Exit` → `kill_all`

### MAX_TURNS
当前常量 `200`(`app/src-tauri/src/agent/mod.rs:76`)。Agent Loop 单 request 内最大 turn 数,超限终止。这是循环检测的**硬兜底**。变更轨迹:`20 → 50 → 200`(06-24 C2 落地时调到 200)。

### Context Compression Thresholds (C3)
`context_window * 0.80` 触发 context 压缩,降到 `0.50`,**B5 memory 永远保护**(超限降级时优先裁其它层,不裁指令文件)。实现见 `app/src-tauri/src/agent/context.rs`。

### Loop Detection (C2 循环检测)
**分级触发**取代原文单一 0.9 阈值,因单一阈值无法适配短 / 长 input:

- **L1 精确签名硬触发**(N=3):同一 tool_use 签名(含 `edit_file` 的 `old_string` 避免正当多块编辑误判)连续 3 次,直接打断 loop
- **L2 Jaccard 软提示**(N=5,0.85):最近 5 次 tool_use 的 input 集合 Jaccard ≥ 0.85 时,**hint 作为 `ContentBlock::Text` 注入 result message**,LLM 下一轮看到提示,**不跳过执行、不终止 loop**

`MAX_TURNS=200` 仍是硬兜底。无 AuditKind 落表(§2.5.8)。token 切分纯 Rust `split_whitespace`(不复用 tiktoken)。实现见 `app/src-tauri/src/agent/loop_detection.rs`。

### AuditKind
`session_audit_events.kind` 字符串枚举,**10 类**:`tool_executed` / `tool_allowed` / `tool_denied` / `tool_ask` / `mode_changed` / `grant_added` / `grant_revoked` / `resend_message` 等(完整列表见 [ARCHITECTURE §2.5.8](../ARCHITECTURE.md))。每类 payload_json 结构不同;`record_tool_executed_audit` 落 `tool_executed` 的 `{tool_name, tool_input, duration_ms, exit_code}`。查询走 `list_session_audit_events` Tauri command + 前端 `useAuditStore` + `<AuditLogModal>`(reka-ui Dialog,绑当前 session,kind 下拉 + "仅 critical" 复选)。

### L1 / L2 / L3 命名约定
路线图子档命名:

- **L1**:后台 shell + 完成通知(L1a 不带 PTY / L1b 后续接 portable-pty)
- **L2**:单 turn 多 tool 并发(只读 batch,`is_parallel_eligible` + `FuturesUnordered`)
- **L3**:Subagent 三层:
  - **L3a**:并发只读 dispatch(`force_readonly` 剥写 + `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 3)
  - **L3b**:worker worktree 隔离(PR1 serial 核心 / PR2 concurrent 解锁 / PR3 merge_worker + discard_worker + sweep / PR3+ permission+concurrency hardening / PR4 前端合并/丢弃 UI)
  - **L3c**:worker 联网(`SubagentDef.tools` + `READONLY_TOOL_ALLOWLIST` 加 `web_fetch`)
  - **L3d**:frontmatter loader(`~/.config/everlasting/agents/*.md` + `<project>/.everlasting/agents/*.md`)

---

## 相关决策

- 设计决策走 [`docs/IMPLEMENTATION.md §4 决策日志`](../IMPLEMENTATION.md#4-决策日志)(本文件不重复)
- A4 Token 相关术语已落地、作为历史术语定义保留;Checklist(agent 自跟踪清单)为规划中术语,实现决策待定
- 跨层契约走 `.trellis/spec/backend/llm-contract.md` "Scenario: Token Usage Tracking" 段
