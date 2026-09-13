<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: tools[] Token Measurement + Static Pruning (C7, 2026-08-14)

> 配套 task `08-14-c7-tools-token-governance`。把 `tools[]` 数组当作与
> messages 并列的**上下文治理对象**:R1 量(MVP)、R3 静态裁剪(MVP)、
> R2 Anthropic cache 断点(Phase 2)、D Stub 注册(Phase 2)。本 scenario
> 锁 MVP 两路径的可执行契约 + cache 率口径红线。

### 1. Scope / Trigger

- Trigger:多 turn 任务里 `tools[]` 每 turn 全量拼装下发(~7-8k tok/轮,
  单对话首回合 context 的大头 —— 实测 session 50b91178 一句"早上好"
  input=12838),挤占有效历史、提前触发 C3 压缩。C7 把它从"0 分的请求
  前缀段"变成可量化 + 可裁剪的治理对象。
- 为什么 code-spec depth:migration(新列)+ 跨层契约(tools_token 从
  Rust 估算 → SQLite → Pinia → TracePanel,且与 cache 率口径交织)→
  一处口径错(double-count)就污染所有占比展示。

### 2. DB Schema(`turn_trace.tools_token`)

```sql
-- CREATE TABLE 段(greenfield 已含)+ 幂等 ALTER(existing)。nullable:
-- NULL = pre-column 行 / worker(skip_persist)轮 / 估算被跳过。
ALTER TABLE turn_trace ADD COLUMN tools_token INTEGER;
```

migration helper:`add_turn_trace_column_if_missing(pool, "tools_token",
"INTEGER")`(`db/migrations/columns.rs`,镜像 `add_session_audit_events
_column_if_missing` 的 PRAGMA-table_info probe 模式)。

### 3. Signatures

```rust
// db/trace.rs —— tools_token 与 token_usage_json 同 upsert 写入
//   (都源自 Done-event 写点;UNIQUE(session_id,seq) 冲突时两列一起重写)
pub async fn upsert_turn_trace_token(
    pool: &SqlitePool,
    session_id: &str,
    seq: i64,
    usage: &TokenUsage,
    tools_token: Option<u32>,   // C7: cl100k of serialized tools[] JSON
) -> Result<(), sqlx::Error>

#[serde(rename_all = "camelCase")]  // → wire "toolsToken"
pub struct TurnTraceRow { /* ... */ pub tools_token: Option<i64> }

// tools/mod.rs —— R3 静态裁剪(provider 之前的 schema 层)
pub fn filter_tools_for_session_type(
    tools: Vec<ToolDef>,
    is_group_chat: bool,
) -> Vec<ToolDef>   // 非 group_chat 砍 nominate_speaker + end_discussion
```

估算点:`agent/chat_loop/drive.rs` 在 `turn_tool_defs` freeze 后(完整
过滤链 mode→workflow→session_type + dispatch_subagent append 之后)、move
进 `retry_open` 之前,`serde_json::to_string(&turn_tool_defs)` →
`memory::tokens::count_tokens`(cl100k,`tokio::sync::Mutex` 守护,µs–low-ms,
inline 安全)。best-effort:序列化失败 → 空串 → 0,不阻塞 turn。

### 4. Contracts — cache 率口径(关键,勿 double-count)

`context_input_tokens` **已含 tools**(provider 侧进入 context window 的
全部 prompt token;见 §2 Anthropic=input+cc+cr / OpenAI=prompt_tokens)。
`tools_token` 是对其中 tools[] 这一**切片**的单独估算 —— 它是
`context_input` 的**子集**,不是额外项。

- TracePanel tools 占比公式 = `tools_token / context_input_tokens`。
- ⚠️ **禁止** `tools_token / (context_input_tokens + tools_token)`
  —— context_input 已含 tools,加回是 double-count,系统性压低占比。
- ⚠️ **禁止**把 tools_token 加进 cache 率分母(`cache_read /
  context_input`)—— cache 率现状已被 tools 稀释偏低(tools 无 cache 断点;
  R2 Phase 2 后 cache_read 才含 tools);tools_token 只**单列**展示,
  不混入 cache 率分子或分母。
- wire:`tools_token` 经 `TurnTraceRow`(camelCase)→ 前端
  `TurnTraceRow.toolsToken` → `TurnTrace.toolsToken?`(undefined =
  pre-column / live 路径未带)。`list_turn_traces` IPC 自动透传,无需改
  IPC handler。live 路径(无 reload)tools_token 暂为 undefined(design
  决定:不为此加 ChatEvent 字段),reload 后(回看)落盘值出现。

### 5. R3 静态裁剪(跨层影响 + 先例)

过滤链 `drive.rs:504`:`filter_tools_for_session_type(filter_tools_for_
workflow(filter_tools_for_mode(...)))`。三环都是纯 `Vec<ToolDef>` 集合
减法,顺序无关。

- **非 group_chat 砍 `nominate_speaker` + `end_discussion`**(落实
  `tools/mod.rs:224` 的 "Phase 4 may filter" 注释)。省 ~465 tok/轮
  (<7%);大头(`use_ui`/`ask_user_question`/`remember`/`shell`)通用,
  静态裁不动 —— 省 window 的大动作是 D Stub(Phase 2)。R3 的 MVP 价值
  主要是"卫生"(非群聊不暴露无意义工具),不是省 token。
- **group_chat 是 no-op**:群聊走 `group_chat_tool_defs` 白名单(`group_
  chat_prompts.rs`,主持人含仲裁工具、参与者不含),本过滤器不二次干预。
- **cache 稳定性(prd R3.2)**:同一 session 的 `session_type` 固定 →
  裁剪结果跨连续 turn 稳定 → OpenAI 自动前缀缓存 / 未来 R2 断点在一个
  mode 段内命中。
- `session_type` 从 `loaded_session.session.session_type` 读(零成本,
  无 DB round-trip)。chat_loop 对 nominate/end 的运行时 no-op 拦截(按
  tool_name)不依赖 tools[] 注册,裁掉工具注册不影响该拦截。

### 6. Tests Required(断言点)

- `tools_token_defaults_null_for_legacy_or_worker_rows` —— raw SQL 写入
  (无 tools_token 列)读回 `None`;再 upsert 补 `Some(425)` 不丢
  token_usage_json。
- `upsert_overwrites_same_column_on_conflict` —— 两次 upsert 传
  `Some(111)` / `Some(222)`,断言最终 `Some(222)`(冲突时 tools_token 与
  token_usage_json 一起重写,锁定 C7 upsert 契约)。
- `upsert_accumulates_columns_across_writes` —— 传 `Some(7000)`,断言
  `rows[0].tools_token == Some(7000)`。
- R3:`tools::tests_session_type_filter` —— classic_chat 裁两工具、
  group_chat no-op。
- 前端:`TurnCard` 渲染 `tools 7K` cell + title 含 `70%`
  (`toolsToken=7000, context_input=10000`);toolsToken 缺失时不渲染。
- 回归:`filter_tools_for_mode` * + `group_chat_tool_defs` * 不变。

### 7. Wrong vs Correct

#### Wrong:把 tools_token 加进分母(double-count)

```ts
// BAD —— context_input 已含 tools,加回是重复计算
const toolsPct = toolsToken / (contextInput + toolsToken);
// 结果系统性偏低(7000 / (10000+7000) ≈ 41% 而非真实 70%)
```

#### Correct:tools_token 是 context_input 的切片

```ts
// GOOD —— tools_token ÷ context_input(子集 / 全集)
const toolsPct = contextInput > 0 ? toolsToken / contextInput : null;
// 7000 / 10000 = 70%(与"tools[] 占本轮 context 七成"的直觉一致)
```
