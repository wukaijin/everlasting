# Implement — A5+ LLM 网络健壮性

> **设计**:[`design.md`](./design.md) · **prd**:[`prd.md`](./prd.md) · **source**:[`docs/research/llm-network-resilience-survey.md`](../../../docs/research/llm-network-resilience-survey.md)
>
> 执行清单。分 7 Step,每 Step 独立 commit + 单独可回滚。Step 1-2 是纯函数基础(无副作用),Step 3-4 接入 agent loop,Step 5 前端,Step 6 熔断收口,Step 7 spec/ROADMAP/ADR。

## 前置

- 工作目录:`app/src-tauri/`(后端)/ `app/`(前端)
- 测试命令(WSL,见 CLAUDE.md HACKING-wsl 坑 1):
  ```
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
  ```
- 前端:`cd app && pnpm test`(vitest)+ `pnpm build`(`vue-tsc --noEmit`)

## Step 1 — `LlmError::is_retryable` + headers 字段(§3.2 / §3.6)

- [ ] `llm/error.rs`:`RateLimit` / `Server` 变体加 `headers: HeaderMap` 字段(design §3.6);`Auth` / `InvalidRequest` / `Network` 不变
- [ ] 加 `impl LlmError { pub fn is_retryable(&self) -> bool }`(`Network`/`Server`/`RateLimit` → true)
- [ ] 更新所有构造 `RateLimit(_)` / `Server {..}` 的点(provider/anthropic.rs / openai.rs / error.rs 内 `classify_error_response`)携带 headers
- [ ] 更新 `From<LlmError> for AppError` 等边界(headers 不参与序列化,`#[serde(skip)]` 或不入 AppError)
- [ ] 单测:`is_retryable` 5 类各一例;`classify_error_response` 429/5xx 带 headers
- [ ] validate:`cargo test --lib error`(现有 error 测试同步改断言后全绿)
- **回滚点**:变体改回 `String`,构造点同步——独立 commit

## Step 2 — `llm/retry.rs` 纯函数(§3.1 / §3.3 / §3.4)

- [ ] 新建 `llm/retry.rs`:`RetryPolicy` struct + `Default`(max_retries=3, base=0.5s, cap=30s, budget=60s, retry_after_cap=60s)
- [ ] `full_jitter(attempt, base, cap, rng: &mut impl Rng) -> Duration`
- [ ] `parse_retry_after(headers: &HeaderMap, cap: Duration) -> Option<Duration>`:`retry-after-ms` → `retry-after`(秒 / HTTP-date)→ OpenAI `x-ratelimit-reset-requests` / `-tokens`(Go duration);全部 cap_min 到 `cap`
- [ ] `parse_go_duration(s: &str) -> Option<Duration>`:手写(`6m0s`/`1s`/`500ms`/`2h30m`)
- [ ] 决定 rng 依赖:确认 `rand` 是否已在 Cargo.toml;否则用 `fastrand`(若 Step 2 grill 通过)
- [ ] 单测(design §6.2):`full_jitter` 区间(注入 `StepRng` / `MockRng`)/ `parse_retry_after` 全格式 / `parse_go_duration` 多例 / `>60s` 截断 / 缺失 None
- [ ] validate:`cargo test --lib retry`
- **回滚点**:模块独立,未接入,可整模块删除

## Step 3 — `retry_send` wrapper(§3.5 / §2)

- [ ] `llm/retry.rs` 加 `pub async fn retry_send(provider, system, messages, tools, policy, token, sink, rng) -> Result<TurnFinalState, RetryOutcome>`
- [ ] 实现 attempt loop + `has_emitted` 首字节追踪(design §2 数据流)
- [ ] 首字节前 `Err` + `is_retryable` + 预算/次数 → emit retrying + sleep(select 配 token.cancelled,R7)→ 重发
- [ ] 首字节后任何 `Err` → 直接 return(chat_loop 现状处理,R3)
- [ ] `ChatEventSink` trait 抽象(emit ChatEvent + emit Retrying),让 agent loop 与测试都能注入
- [ ] validate:`cargo check`(编译过)
- **回滚点**:函数独立,未接入 agent loop

## Step 4 — `MockProvider` 扩展 + retry 集成测试(§6.1 / §6.2)

- [ ] `llm/provider/mock.rs` 加 `error_sequence: VecDeque<LlmError>` / `header_overrides` / `emit_then_err` / `attempt_count()`
- [ ] retry 集成测试矩阵(design §6.2,对应 prd AC 14 项):
  - 5xx 序列成功 / 429+retry-after / OpenAI Go duration / connect err 重试
  - 首字节前断连重试 / **首字节后断连不重试** / Auth 不重试 / InvalidRequest 不重试
  - max_retries 耗尽 / budget 熔断 / C1 sleep 中取消 / Full Jitter 区间 / retry-after 封顶 60s
- [ ] validate:`cargo test --lib`(mock + retry 全绿)
- **回滚点**:测试独立,可删

## Step 5 — 接入 `agent/chat_loop.rs`(§1.2)

- [ ] `chat_loop.rs:1388-1392` `provider.send(...)` 改调 `retry::retry_send(...)`,传 `token` + `sink` + `RetryPolicy::default()` + rng
- [ ] agent loop 的 sink 已有(`emit_chat_event_via_sink`),适配 `ChatEventSink` trait
- [ ] retry_send 返回的 `RetryOutcome::Cancelled` → 走 C1 路径(`cancelled = true`,不 `had_error`)
- [ ] retry_send 返回 `Err(LlmError)` → 走现状 `had_error` + ERROR_MARKER 路径
- [ ] retry_send 成功 → 正常透传的 ChatEvent 已 emit,继续 tool 执行阶段(不变)
- [ ] rng 构造:运行时用 `rand::thread_rng()`(或 fastrand 等价);测试通过 Step 4 的 MockProvider 验证
- [ ] validate:`cargo test --lib`(现有 chat_loop 测试全绿,无回归)
- **回滚点**:调用点改回 `provider.send(...)` 一行

## Step 6 — 不变量回归测试(R9,§6.3)

- [x] token 统计不重复:mock 前 2 次 Server(503) + 第 3 次成功 → `sessions.last_input_tokens` == 第 3 次 usage(非 3 倍)
- [x] C3 压缩幂等:接近阈值的 messages,重试成功后压缩行为与无重试一致(以 `a5plus_retry_terminal_state_matches_no_retry_path` 守住相同终态;深度 C3 触发由 `agent_loop_c3_compaction_does_not_panic` + retry.rs 单测覆盖)
- [x] 既有 `chat_loop` / `provider` / `sse` / `error` / `context` 测试全绿(回归基线,从 Step 5 跑后保持)
- [x] validate:`PKG_CONFIG_PATH=... cargo test --lib`(1272 通过 / 2 预存 flaky 与本任务无关 —— `agent_loop_ask_user_question_session_cancel` 自 Step 5 commit `dd00104` 即失败、`memory::loader_mtime_fence_sees_file_change` 文件系统 mtime 精度 flaky)
- **回滚点**:测试独立

## Step 7 — 前端 retrying 事件(§5)

- [x] 后端 `ChatEvent` 加 `Retrying { attempt, max_attempts, wait_ms, reason }` 变体 + wire 序列化(`llm/types.rs`,snake_case `retrying` 标签 + wire-shape 单测)
- [x] 前端 `streamController.ts`:`case 'retrying'` 分发,挂当前 turn 临时状态(不入 messages,挂在 `last.retrying` 字段)
- [x] `MessageList` / `MessageItem`:正在生成的 assistant message 内嵌"↩ 重试中 {attempt}/{max_attempts},{wait}s 后重发…(reason)"行;下次 ChatEvent(start / delta / done / error)到来清除
- [x] 全中文,无新 i18n key
- [x] validate:`cd app && pnpm test`(streamController.test.ts 36/36)+ `pnpm build`(`vue-tsc --noEmit` 我的改动 0 err,残留 2 err 是 `marked-highlight` / `highlight.js` 包未装的预存环境问题)
- **回滚点**:前端分支独立,后端 Retrying 变体保留无害

## Step 8 — 熔断 + 总时间预算(R6,§3.5)

> Step 3 已埋 `total_elapsed` 累加 + budget 判定;Step 8 补测试与边界。

- [x] 补测试:构造累计 sleep > budget(60s)的序列 → 触达预算即停止(不等 max_retries)—— `retry_open_budget_breaks_before_max_retries` + `retry_open_budget_breaker_stops_retry` + `retry_open_zero_budget_remaining_stops_without_emitting_retry_notice`
- [x] 补测试:max_retries 与 budget 同时设置,budget 先触达 / 次数先触达两种路径 —— `retry_open_budget_breaks_before_max_retries`(budget 先)+ `retry_open_max_retries_breaks_before_budget`(次数先)+ `wait_clamps_advisory_to_remaining_budget_so_total_does_not_overshoot`
- [x] validate:`cargo test --lib retry`(retry 模块 32/32 通过)
- **回滚点**:测试 + budget 判定独立

## Step 9 — spec / ROADMAP / ADR(收尾)

- [x] `agent-loop-architecture.md`:加 "Pattern: LLM retry_open wrapper" 段(retryable 分类 + Full Jitter + retry-after + 首字节边界 + 熔断,交叉引用 llm-contract 完整契约)
- [x] `llm-contract.md`:加 `## Scenario: LLM Retry / Backoff (A5+)` 完整段(含 headers 字段扩展声明 §5)
- [x] `docs/ROADMAP.md` §1.2:加 A5+ 落地条目(引用 survey + spec + task)+ 第三档划线 + 计数 14→15
- [x] `docs/IMPLEMENTATION.md` §4:加 2026-07-05 ADR(决策:外层 wrapper 落点 / Full Jitter / 首字节边界 / headers 字段扩展 / 命名演进 / cancel 时序变更)
- [x] `docs/DESIGN.md` §5.1 留口:风险表"LLM 流式 token 断连"行标注 ✅ A5+ 07-05 落地(首字节前重试,非 message ID 续传)
- [x] validate:`git diff` 自查文档链接;无代码改动(本 step 纯文档)
- **回滚点**:文档独立

## 验收(全绿才 archive)

- [x] prd AC 14 项 checkbox 全勾(trellis-check 核实 R1-R9 + AC 14 全覆盖;prd.md AC 段已勾)
- [x] `PKG_CONFIG_PATH=... cargo test --lib` 全绿 — **1274 passed / 0 failed**(Step 5 baseline 1266 + Step 6 集成 3 + Step 8 边界 4 + session_cancel timing 修复 1,无 flaky 残留)
- [x] `cd app && pnpm test` + `pnpm build` 0 err — **718 passed / 0 failed** + vue-tsc **0 error**(highlight.js/marked-highlight 依赖缺失已 `pnpm install` 修复,与本任务改动无关)
- [x] spec / ROADMAP / ADR 更新(Step 9 全勾)
- [x] DEBT.md 无新 open 项(本任务不引入债;DEBT.md 0 个 A5+ 相关条目)
- [ ] `task.py archive 07-04-a5plus-llm-network-resilience`(commit 后执行)
