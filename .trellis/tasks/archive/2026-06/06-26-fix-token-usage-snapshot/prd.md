# 修复 TokenUsage 上下文占用统计（快照语义 + worker 隔离）

> **来源**：session 调查 → 用户 AskUserQuestion 两个决策 → plan `/home/carlos/.claude/plans/happy-riding-wilkinson.md`。本 PRD 自包含全部 file:line，sub-agent 无需回看 plan 文件。
> **相关 spec**：`.trellis/spec/backend/token-usage-tracking.md`（A4，需重写为 snapshot 语义）、`subagent-runs-schema.md` + `agent-loop-architecture.md`（RULE-A-015 reversal 注解）。

## Goal

前端「上下文占用 %」显示 `1.7M · 100% / 1M` 爆表。改为基于「**最后一次请求的真实上下文占用快照**」（跨 provider 归一化），并让子代理 token 不再污染父 session。目标：百分比准确，作为后续基于阈值的上下文压缩功能的可信前提。

## What I already know（根因，行号已核实）

**实测数据**（DB session `631362ab`「用 spec-auditor 审 tool-contract.md」）：仅 4 条消息（主 agent 2 次 LLM 调用），但 `input_tokens_total=1,689,897`、`cache_read_total=1,582,080`。所有「拉起子代理」的 session 全部爆表；无子代理的短 session（`你好`=23K、`看 app.util.ts`=29K）数值正常。

**根因三层**：

1. **累加语义错**：`db/sessions.rs:374-399` `add_token_usage` 用 `COALESCE(col,0) + ?` 逐 turn 累加进 `sessions.{input,output,cache_creation,cache_read}_total`。「上下文占用」应是单次快照，不是历史求和。
2. **worker 泄漏**：`agent/chat_loop.rs:1149-1183`（add_token_usage 调用在 `:1180`）故意把累加从 `skip_persist` gate 拿出来（RULE-A-015/PR2a），worker 复用父 `session_id`（`agent/subagent/dispatch.rs:369-376` 传 `skip_persist=true`），每 turn usage 都灌进父 session。`agent/subagent/sink.rs:403-416` 注释复述此决策。
3. **跨 provider 口径不一**：`llm/provider/anthropic.rs:751-779` 的 `input_tokens` = 未缓存增量；`llm/provider/openai.rs:915-944` 的（=`prompt_tokens`）= 完整输入。塞同一字段。前端 `input+cache_read+cache_creation` 对 OpenAI 重复计；只取 `input_tokens` 对 Anthropic 严重低估。

**C3 压缩不受影响（本次不动）**：`agent/context.rs:210` `compact_messages` 用 `estimate_messages_tokens(messages)`（clocation tokenizer 估算当前 messages）+ `TRIGGER_RATIO=0.80`（context.rs:45-50），**完全不读 sessions 累加列**。前端 % 与 C3 是两套独立口径，本次让前端 % 改用真实快照，C3 保持 estimate。

**spec 初衷其实就是要 snapshot**：`token-usage-tracking.md:14-16` trigger 描述写「current context usage, **not cumulative session totals**」——实现背离了 spec 本意，本次修复即回归。

**dead-code 累加器**：`db/subagent_runs.rs:20` `add_token_usage_streaming` 有测试（`db/subagent_runs_tests.rs:252-302`）但**无 production callsite**（`sink.rs:77-79`、`:413` 注释确认「no production callsite, only db/tests.rs」）。是累加语义的残留 dead-code。

## Decisions（ADR-lite）

| # | 决策 | 理由 |
|---|---|---|
| D1 | **百分比口径 = LLM 真实占用快照**（用户拍板） | 真实计费值；reload 持久化；C3 保持 estimate 独立（它在请求前判断，那时没 usage）。两者在 80% 线天然接近 |
| D2 | **worker token 隔离到 subagent_runs**（用户拍板） | 子代理跑独立 context，不该算父窗口。父 session 只反映父自己 turn。子代理 token 在 `subagent_runs.token_usage_json`（`dispatch.rs:497` 写）+ SubagentDrawer 查看。放弃「父 UI 实时看子代理烧 token」 |
| D3 | **归一化字段 `TokenUsage.context_input_tokens`** | 跨 provider 统一「本次请求总输入」。Anthropic=`input+cache_creation+cache_read`；OpenAI=`prompt_tokens`。比「前端自己加 4 分量」更干净，且消除跨 provider 重复计/漏计 |
| D4 | **5 个 `last_*` 快照列**（非 1 列） | 让展开详情也是纯净快照，避免「%是快照/detail是累加」口径混杂。成本仅 5 个 INTEGER 列 |
| D5 | **删 `add_token_usage` + dead-code `add_token_usage_streaming`** | 两者都是 bug 源头/累加残留。`_total` 列在 DB 保留冻结（不删列，避免 migration 风险），代码不再写入。符合项目去债惯例（当前 task 线就是 debt cleanup） |
| D6 | **`context_input_tokens` 加 `#[serde(default)]`** | 旧 `subagent_runs.token_usage_json` 反序列化时缺失该字段 → 默认 0，不报错 |

## Requirements（6 阶段，含准确 file:line）

### 阶段 1 — TokenUsage 归一化

- `llm/types.rs:317-323` 加第 5 字段（`#[serde(default)]` 必需）：
  ```rust
  #[serde(default)]
  pub context_input_tokens: u32,
  ```
  注释说明「跨 provider 归一化的本次请求总输入，前端 % 的分子；Anthropic=input+cc+cr，OpenAI=prompt_tokens」。
- `llm/provider/anthropic.rs:773-778` `Some(TokenUsage{..})` 内加（`input`/`cache_creation`/`cache_read` 是未截断 u64，先求和再 `min(u32::MAX)`）：
  ```rust
  context_input_tokens: (input + cache_creation + cache_read).min(u32::MAX as u64) as u32,
  ```
- `llm/provider/openai.rs:938-943` 加（`input`=prompt_tokens 已含 cached）：
  ```rust
  context_input_tokens: input.min(u32::MAX as u64) as u32,
  ```
- `llm/provider/mock.rs`：核查是否直接构造 `TokenUsage`（grep 显示 mock 仅在 doc comment 提及 `TokenUsage::default()`，走 parse 路径则自动生效；若直接构造则补字段）。
- **所有 `TokenUsage { ... }` 字面量补字段**（不用 helper，保留口径可见性）：
  - `db/sessions_tests.rs:495,519,525,546,565`
  - `db/subagent_runs_tests.rs:111,270,276,296,332,839`
  - `llm/types.rs:820,870`
  - `agent/subagent/sink.rs:738,881,961`
  - `agent/tests_subagent.rs:1297,1311,1325,2606,2639,2678`

### 阶段 2 — DB migration + 快照函数

- `db/migrations.rs:325` 之后（复用 `add_session_column_if_missing`，模式见 `:669-685`）加 5 列：
  ```rust
  add_session_column_if_missing(pool, "last_context_input_tokens", "INTEGER").await?;
  add_session_column_if_missing(pool, "last_input_tokens", "INTEGER").await?;
  add_session_column_if_missing(pool, "last_output_tokens", "INTEGER").await?;
  add_session_column_if_missing(pool, "last_cache_creation", "INTEGER").await?;
  add_session_column_if_missing(pool, "last_cache_read", "INTEGER").await?;
  ```
- `db/sessions.rs`：**删** `add_token_usage`（`:374-399`），**新增**覆盖写 `update_last_turn_usage`：
  ```rust
  pub async fn update_last_turn_usage(pool, session_id, usage: &TokenUsage) -> Result<(), sqlx::Error> {
      // UPDATE sessions SET last_context_input_tokens=?, last_input_tokens=?,
      //   last_output_tokens=?, last_cache_creation=?, last_cache_read=?, updated_at=? WHERE id=?
  }
  ```
- `db/subagent_runs.rs:20` + `db/subagent_runs_tests.rs:252-302`：**删** dead-code `add_token_usage_streaming` 及其测试（无 production callsite）。
- `db/types.rs`：`SessionRow` + `SessionSummary` 各加 5 个 `last_*: Option<i64>`（`*_total` 字段保留冻结）。
- `db/sessions.rs`：`list_sessions`（`:85-139`，SELECT 在 `:87-98`、map 在 `:120-137`）和 `load_session`（`:148-180`，SELECT `:150-156`）的 SELECT + map 补 5 列；`create_session`（`:58-76`）SessionRow 构造补 5 个 `None`。
- `agent/tests_prompts.rs`：SessionRow fixture 补 5 个 `last_*: None`。

### 阶段 3 — worker 隔离（`agent/chat_loop.rs:1149-1183`）

- Done 事件里 `add_token_usage` → `update_last_turn_usage`，**关回 `!skip_persist` gate**（参照 `:578`/`:1395`/`:1636` 的 `if !skip_persist { ... }` 模式）。
- 注释 reversal 说明（2026-06-26 reversal of RULE-A-015/PR2a：worker 复用父 session_id 会污染父 %，故重新关回 gate；worker token 隔离到 subagent_runs）。
- **配套代码注释更新**（非 spec 文档）：
  - `agent/subagent/sink.rs:77-79`、`:403-416`（「decoupled from skip_persist」→「inside skip_persist gate; worker usage via cumulative_usage only」）
  - `agent-loop-architecture.md:59-72` 的 `skip_persist` 参数注释（这是 spec，归 Phase 3.3，但代码内 inline 注释若有同步改）
- `agent/subagent/dispatch.rs:497` `cumulative_usage → subagent_runs.token_usage_json` **不变**（worker token 仍完整保留）。

### 阶段 4 — 前端类型 + store

- `stores/chat.types.ts:75-80` `SessionTokenUsage` 加 `context_input_tokens: number`，注释改为「最后一次请求快照（非累加）」。
- `stores/chat.types.ts:238` `SessionSummary` 加 5 个 `last_*: number | null`（`*_total` 字段保留）。
- `stores/chat.ts`：
  - `accumulateTokenUsage`（`:153-167`）→ 重命名 `setLastTurnUsage`，改**覆盖写**：`tokenUsageBySession.set(sessionId, { ...usage })`（删 `+=` 分支）。
  - `loadSessions` seed（`:393-403`）改读 `last_*`，判定用 `s.last_context_input_tokens !== null`（归一化字段作「有快照」标志）。
  - store 导出（`:1304-1384`）同步改名。
- `stores/streamController.ts:976-978`：`accumulateTokenUsage` → `setLastTurnUsage`。

### 阶段 5 — 前端显示

- `components/chat/ChatInputHintRow.vue:104-113`：百分比分子 `tokenUsage.input_tokens` → `tokenUsage.context_input_tokens`；主数字同。展开详情 4 行（`:120-136`）结构不变（语义已是「最后一次请求分量」）。
- `components/chat/ChatInput.vue:153-158`：`usageLevel` 分子 `u.input_tokens` → `u.context_input_tokens`。

### 阶段 6 — 测试（必改清单）

**Rust（删/改）**：
- `agent/tests_subagent.rs:1281` `agent_loop_dispatch_subagent_token_usage_folds_into_parent` —— **改写**为断言 worker token **不**进父 session（父 `last_context_input_tokens` 只反映父 turn），worker usage 在 `subagent_runs.token_usage_json`。
- `agent/tests_subagent.rs:2603` `l3a_concurrent_token_usage_folds_into_parent` —— 同改写。
- `db/sessions_tests.rs:480-578` `add_token_usage_*` 3 个测试 —— **删**（函数已删），新增 `update_last_turn_usage_overwrites_not_accumulates`（调两次→值=第二次）+ `list_sessions_includes_last_turn_columns`。
- `db/subagent_runs_tests.rs:252-302` `add_token_usage_streaming_*` —— **删**。
- `llm/provider/{anthropic,openai}.rs` parse 测试 —— 加 `context_input_tokens` 断言（Anthropic=input+cc+cr；OpenAI=prompt_tokens，有 cached 也不加）。

**前端**：`utils/tokenUsage.test.ts`（`tokenUsageLevel` 接受 pct 数字，大概率不受影响，核查）；新测 `setLastTurnUsage` 覆盖语义。

## Acceptance Criteria

- [ ] session `631362ab` 冷启动后 ChatInput 显「—」（旧 session `last_*`=NULL fallback），爆表 1.7M 消失
- [ ] 新发 1 turn（不拉子代理）→ % 用 `context_input_tokens / context_window`，数值合理（如 200K 窗口 ~25%）
- [ ] 拉子代理后父 % 只反映父 turn（<100%）；SubagentDrawer 仍显示 worker `token_usage_json`
- [ ] OpenAI/GLM 模型 % 用 prompt_tokens（含 cached），数值合理
- [ ] `cargo test --lib`（带 PKG_CONFIG_PATH）全绿，含改写后的 worker 隔离测试
- [ ] `npx vitest run` + `npx vue-tsc --noEmit` 全绿

## Out of Scope

- C3 压缩逻辑（`context.rs` estimate + 0.80 阈值）不动
- `_total` 4 列在 DB 保留冻结（不删列），SessionSummary 的 `_total` 字段保留不读
- 不做「父端实时聚合子代理 token」（D2 明确放弃）
- 不做 `_total` 列/字段的清理（留后续 debt PR）

## Technical Notes

- **Rust 测试命令**（WSL，见 CLAUDE.md）：
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib db::sessions_tests agent::tests_subagent llm::provider
  ```
- **前端**：`cd app && npx vitest run && npx vue-tsc --noEmit`
- **跨 provider 边界**：快照语义下，session 中途切 provider，下一 turn 自然覆盖为正确口径，无累加错乱。
- **旧数据 NULL**：前端 seed 判定必须用 `last_context_input_tokens !== null`，fallback 显「—」而非 0%。
- **`max_turns` synthetic terminal**（`chat_loop.rs:1797-1820`）：父 loop 仍走 `ChatEvent::Done` → `update_last_turn_usage` 正常；worker 的 max_turns 走 sink 不写父 session（符合隔离）。

## Spec to update（Phase 3.3 trellis-update-spec 处理，非 implement）

- `.trellis/spec/backend/token-usage-tracking.md`：重写为 snapshot 语义（`add_token_usage`→`update_last_turn_usage`；`context_input_tokens` 新字段；§3 累加描述→覆盖写；§4 Validation Matrix 的「cumulative」表述；§5/§7 累加样例）。line 14-16 的「not cumulative」初衷终于落地。
- `.trellis/spec/backend/subagent-runs-schema.md:210-216`：RULE-A-015 段补 reversal 注解（2026-06-26：worker token 重新隔离，不再 fold 进 parent）。
- `.trellis/spec/backend/agent-loop-architecture.md`：RULE-A-015 段（`:61-72`、`:644-780`、`:940` 测试表）补 reversal；测试名 `folds_into_parent` 已改。

## Research References

- DB 实测脚本：本会话 `/tmp/inspect631.py`、`/tmp/inspect2.py`（session 631362ab 消息 + subagent transcript）
- plan 全文：`/home/carlos/.claude/plans/happy-riding-wilkinson.md`（main session 可读，sub-agent 看不到，内容已并入本 PRD）
