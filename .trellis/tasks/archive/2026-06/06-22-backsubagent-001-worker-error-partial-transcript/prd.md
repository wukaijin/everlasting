# RULE-BackSubagent-001: worker error partial transcript

## Goal

worker agent 异常终止（Error / Cancelled / Incomplete-max_turns）时，parent LLM 当前只看到 `[status: error]\n<error text>`，**不知道 worker 已经执行了哪些 tool_call、哪些已成功落地**。parent 因此盲目重做或放弃，无法基于 worker 已落地的 edits 做补偿性修复。本任务让 parent 在非 completed 终态下拿到 worker 已执行 tool 的摘要，支撑补偿性修复决策（跳过已落地的 write/edit、重试失败的 tool）。

## What I already know (代码探查)

- **数据源就绪**：`worker_sink: Arc<SubagentBufferSink>` 累积 worker 全量 transcript，`worker_sink.transcript_snapshot()`（`chat_loop.rs:2447`）已能拿到。`format_dispatch_result` 调用点（`chat_loop.rs:2539`）处 `worker_sink` Arc 仍在 scope → error 分支有能力读这些数据。
- **transcript 4 kind**：`chat_event` / `tool_call` / `tool_result` / `permission_ask`（`subagent.rs:468-473`）。
- **tool_call payload**：`{name, input, tool_use_id}`（`subagent.rs:1002`）。
- **tool_result payload**：`{content, is_error, tool_use_id, duration_ms}`（`subagent.rs:1037-1046`）→ 可用 `is_error` 标每条 tool 落地状态，`tool_use_id` 做配对。
- **status 来源**：`had_error()→Error` / `was_cancelled()→Cancelled` / `was_incomplete()→Incomplete` / else `Completed`（`chat_loop.rs:2403-2419`）。
- **两条出口函数**：`format_dispatch_result(status, worker_text) -> (content, is_error)`（wire，`subagent.rs:1338`）内部调 `format_final_text`（DB `final_text` + drawer Reply，`subagent.rs:1269`）。
- 已有 `truncate_transcript_for_persistence`（4 MiB head+tail cap）模式可参考 cap 策略。

## Open Questions (resolved)

- ~~**Q1 作用范围**~~ → **统一三态**（Error + Cancelled + Incomplete）。
- ~~**Q2 摘要形状**~~ → **name + 关键参数 + ✓/✗**。
- ~~**Q3 wire vs DB**~~ → **只进 wire**；`final_text` 不动（drawer 已有 Tools 段，避免冗余）。解耦 `format_final_text`（DB）与 `format_dispatch_result`（wire，append 摘要段）。
- ~~**Q4 cap 截断**~~ → **head + tail**（最早 N + 最近 N + 中间省略号计数），对齐现有 transcript 持久化模式。

## Requirements

- **R1（作用范围）**：Error + Cancelled + Incomplete 三个非 completed 终态，parent tool_result 都附 worker 已执行 tool 的摘要段；Completed 不附。
- **R2（摘要形状）**：每条 `- {tool_name}({key_param}): ok|failed|?`：
  - per-tool 代表参数：write/edit/read_file→`file_path`、grep/glob→`pattern`、shell→`command`（截断）、web_fetch→`url`、use_skill→`skill`、list_dir→`path`、update_checklist→省略参数。
  - `ok`/`failed` 来自配对 `tool_result.is_error`；orphan tool_call（无配对 result，执行中/未完成）标 `?`。
  - `permission_ask` / `chat_event` kind 不进摘要（与补偿修复无关）。
- **R3（出口分家）**：`format_dispatch_result`（wire）在 `format_final_text` body 之后 append 摘要段（标题 `Worker partial actions:`）；`format_final_text`（DB）不动。
- **R4（cap）**：摘要段总长 ≤ 2 KiB；超限 head+tail 截断，中间用 `... (N tools omitted) ...` 标省略计数。

## Acceptance Criteria

- [ ] worker Error 终态 + 多 tool → parent tool_result wire 含摘要段，每条 `name(key_param): ok|failed`。
- [ ] worker Cancelled 终态 → 同上。
- [ ] worker Incomplete（max_turns）终态 → 同上。
- [ ] worker Completed 终态 → tool_result wire **不含**摘要段（与现状一致）。
- [ ] orphan tool_call（worker 中途 error、result 未 emit）→ 标 `?`。
- [ ] 0 tool 的非 completed 终态 → 摘要段不显示（或显式 `(no tools executed)`），不产生空标题。
- [ ] 超 2 KiB → head+tail 截断 + `(N tools omitted)` 计数正确。
- [ ] `format_final_text`（DB / drawer）输出**不变**（回归保护，现有 `format_final_text_*` / `format_dispatch_result_*` 测试断言不破）。
- [ ] 集成测试：一个真实 worker error 场景，断言 parent 收到的 dispatch_subagent tool_result content 含摘要。

## Definition of Done

- 新增 `summarize_worker_tool_actions` + per-tool key_param 单测；`format_dispatch_result` 签名扩展 + 回归测试。
- `cargo check` 0 warning；`cargo test --lib` 全 pass（含 `PKG_CONFIG_PATH`）。
- DEBT.md RULE-BackSubagent-001 标 closed + commit hash；spec 同步（`agent-loop-architecture.md` / `tool-contract.md` 的 `format_dispatch_result` 描述 + Incomplete/Cancelled/Error 三态 wire shape）。
- 四段式 commit（fix → docs(debt) → archive → journal）。

## Technical Approach

新增纯函数 `summarize_worker_tool_actions(transcript: &[TranscriptEntry]) -> String`：
1. 单次遍历：`HashMap<tool_use_id, (name, key_param)>` 收 tool_call；二次遍历 tool_result 按 `tool_use_id` 配对标 `ok`/`failed`，未配对的 tool_call 标 `?`（保留 transcript 原序）。
2. 每条 `- {name}({key_param}): {status}`；`permission_ask`/`chat_event` 跳过。
3. head+tail cap 2 KiB：累计字节数超限时，保留首 N + 尾 M，中间插 `... (K tools omitted) ...`（K = 总数 - N - M）。

`format_dispatch_result` 签名加 `partial_actions: Option<&str>`：`Some(non-empty)` → body 后 append `\n\nWorker partial actions:\n{actions}`；`None`/empty → 维持现状。

调用点 `chat_loop.rs:2539`：Completed 传 `None`，三态传 `Some(summarize_worker_tool_actions(&worker_sink.transcript_snapshot()))`。`transcript_snapshot()` 已在 2447 调用（DB 持久化），此处二次调用返回 Vec clone，零副作用。

## Decision (ADR-lite)

**Context**: worker 非正常终态时 parent 拿不到 partial transcript，无法补偿修复（B6 review defect B）。DEBT 字面只点 Error，但写于 Incomplete（max_turns）状态引入之前。

**Decision**: 统一三态（Error + Cancelled + Incomplete）都填摘要；摘要只进 wire 不进 DB（drawer 已有 Tools 段）；形状 = name + 关键参数 + ok/failed/?；cap = 2 KiB head+tail。

**Consequences**:
- `format_dispatch_result` 与 `format_final_text` 解耦（wire 内容 > DB 内容），两者消费者不同属合理。
- parent LLM 获得补偿修复所需的"哪些文件已落地"信息。
- 不做：result content 摘要（Option C，error_reason 顶层已有）、failed 优先/类型加权 cap（head+tail 够 MVP）、final_text 塞摘要（冗余）。

## Out of Scope

- failed 优先 / 类型加权 cap 策略（MVP head+tail；follow-up 视真实 worker 重型场景需求）。
- result content 输出摘要（每条加失败原因/截断输出）。
- `final_text` / DB / drawer 也显示摘要段（drawer 已有 Tools 段）。
- drawer 视觉差异化（依赖 Tools 段，非本任务）。

## Implementation Plan (small PRs)

- **PR1**: 新增 `summarize_worker_tool_actions` + per-tool `key_param_for_tool` helper + head+tail cap + 单测（纯函数，不动调用点与 `format_dispatch_result`）。
- **PR2**: `format_dispatch_result` 加 `partial_actions` 参数 + `chat_loop.rs:2539` 接线 + 回归测试 + 1 个 worker error 集成测试断言 parent tool_result 含摘要。
- **PR3**: spec 同步（`agent-loop-architecture.md` / `tool-contract.md`）+ DEBT.md RULE-BackSubagent-001 close（commit hash 回填）+ 四段式 commit。

## Technical Notes

- `app/src-tauri/src/agent/subagent.rs:1338` `format_dispatch_result`（Error arm `:1347`，Incomplete arm `:1348`）。
- `app/src-tauri/src/agent/subagent.rs:1269` `format_final_text`（不动）。
- `app/src-tauri/src/agent/chat_loop.rs:2539` 调用点；`:2447` `transcript_snapshot()` 已有调用可复用。
- 来源：B6 review `docs/review/b6-subagent-assessment.md` §4 defect B。
