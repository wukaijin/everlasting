# A4 Token 用量统计(per-session 累积 + ChatInput hint 区)

## Goal

让用户在 ChatInput 的 hint 区直接看到当前 session 的 context 压力(`input_tokens_total / context_window`,50% 黄 / 75% 红),在每次 LLM turn Done 立即刷新。第一版目标是"看到自己烧了多少",不做 $ 换算、不做 per-turn 细分、不做独立统计面板。落库口径跟 Anthropic 官方 statusline 一致(`current context usage, not cumulative session totals`,但作用域换成 per-session,反映本 session 的 context 占用)。

## What I already know

### Code 现状(grill 阶段已扫)

- **Anthropic SSE**(`app/src-tauri/src/llm/provider/anthropic.rs:506-516`):`message_delta` **只读** `delta.stop_reason`,**不读** `usage` 字段
- **OpenAI SSE**(`app/src-tauri/src/llm/provider/openai.rs:218-223`):`build_http_body` 没发 `stream_options: { include_usage: true }`,末 chunk 不带 usage
- **`ChatEvent::Done`**(`app/src-tauri/src/llm/types.rs:255-256`):`Done { stop_reason: Option<String> }`,**没** usage 字段
- **DB schema**(`app/src-tauri/src/db/migrations.rs:99-220` + `db/sessions.rs:28-71`):`sessions` 12 列(无 token),`messages` 9 列(无 token)
- **Agent loop**(`app/src-tauri/src/agent/chat.rs:306-650`):`for turn in 1..=MAX_TURNS` 循环,`ChatEvent::Done` 处理在 line 421-423
- **前端**(`app/src/components/chat/ChatInput.vue:138-144`):`chat-input__hint-text` 区域是 textarea 下方 hint 行,显示快捷键提示

### 调研发现(2026-06-10)

- **OpenAI 调研**:Chat Completions 已支持 prompt caching,字段 `usage.prompt_tokens_details.cached_tokens`,需发 `stream_options: { include_usage: true }` 才返回
- **Anthropic 官方 statusline**:`total_input_tokens` 反映"current context usage, not cumulative"
- **sanztheo/claude-code-statusline**(开源参考):从 JSONL transcript 取最新 turn 的 `input_tokens + cache_read + cache_creation` 三列求和
- **claude-devtools**:7 类别细分(CLAUDE.md / Tool / Thinking / Skills),不做百分比环

调研结论沉淀在 [`CONTEXT.md`](../../../../CONTEXT.md)(项目级 glossary)。

## 锁定决策(grill-with-docs 阶段 8 题)

| # | 决策点 | 锁定值 |
|---|--------|--------|
| 1 | 颗粒度 | per-session 累积(单条 UPDATE / turn) |
| 2 | 字段集 | 4 列(input/output/cache_creation/cache_read),OpenAI 归一化 |
| 3 | 写入时机 | 每次 LLM turn Done 立即累加 |
| 4 | 价格换算 | 只 token 数字,不 $ |
| 5 | 展示位置 | ChatInput hint 区,class 重命名 |
| 6 | 总用量口径 | `sum(input_tokens) per turn`,分母 context_window,50/75 颜色 |
| 7 | 旧 session | 4 列 nullable,NULL 显 "—" |
| 8 | usage 通路 | `ChatEvent::Done { stop_reason, usage: TokenUsage }`,归一化在 Provider 层 |

## Requirements

### R1 — LLM 层 usage 归一化

- `ChatEvent::Done` 扩展为 `Done { stop_reason, usage: Option<TokenUsage> }`
- 新增 `TokenUsage` struct(`llm/types.rs`):`{ input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }`
- Anthropic provider:解析 `message_delta.usage` → TokenUsage
- OpenAI provider:`build_http_body` 加 `stream_options: { include_usage: true }`,末 chunk 解析 `usage` 归一化到 TokenSchema(cached_tokens → cache_read,cache_creation 填 0)
- 协议层语义保证:`TokenUsage` 在 yield 时已经是 protocol-agnostic 4 字段

### R2 — Agent loop 累加

- agent loop 收到 `ChatEvent::Done { usage: Some(t), .. }` 时,UPDATE sessions 4 列累加
- 单条 SQL:`UPDATE sessions SET input_tokens_total = input_tokens_total + ?, ... WHERE id = ?`
- SQL 失败 `tracing::warn!` + skip,不阻塞主流程
- 收到 `usage: None`(cancel / error / 网络断)时**不**累加,tracing 记 info
- 4 列累加封装为新函数 `db::sessions::add_token_usage(session_id, TokenUsage)`

### R3 — DB schema

- `migrations.rs` ALTER 4 列(nullable INTEGER,默认 NULL):`input_tokens_total` / `output_tokens_total` / `cache_creation_total` / `cache_read_total`
- 用 `add_session_column_if_missing` 探针保持向后兼容
- 旧 session 读到 NULL → 前端渲染 "—"
- 新 session 第一 turn 后变数字 → 渲染百分比

### R4 — UI 组件(ChatInput hint 区)

- 重命名 `chat-input__hint-text` → `chat-input__token-usage`
- 渲染:百分比数字 + 进度条 `14.2K · 7% / 200K`
- 颜色阈值:0-49% 绿 / 50-74% 黄 / 75%+ 红
- null → "—" + 提示
- Pinia store 加 `currentSessionTokenUsage`,SSE `chat-event` 监听更新

### R8 — Hover 分项 tooltip(已选)

- 鼠标悬停 `chat-input__token-usage` 数字,弹 tooltip 显示分项
- tooltip 内容:`input: 10K / cache_read: 4K / cache_creation: 0 / output: 0.2K`
- tooltip 用 reka-ui 的 `Tooltip` 组件(项目已有 reka-ui 依赖,见 `app/package.json`)
- 数字按降序排列,大单位用 K/M 简写
- null → tooltip 内容: "升级前未统计"

### R5 — 跨层契约 spec

- `.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Token Usage Tracking" 段(code-spec depth)
- 包含:TokenUsage 字段定义、Anthropic / OpenAI 归一化映射、错误矩阵、Good/Base/Bad 三档、24 个必测项、Wrong/Correct 对照

### R6 — 决策日志

- `docs/IMPLEMENTATION.md §4` 追加 2026-06-10 条目:
  - `ChatEvent::Done` 携带 `usage` 字段(归一化边界选择)
  - 总用量口径 = `sum(input_tokens) per turn`(cache 双重计 trade-off)

## Acceptance Criteria

- [ ] Anthropic stream 结束后,`sessions.input_tokens_total` 累加正确(含 cache_creation / cache_read)
- [ ] OpenAI stream 结束后,`sessions.input_tokens_total` / `cache_read_total` 累加正确
- [ ] OpenAI `build_http_body` 含 `stream_options: { include_usage: true }`(单元测试断言)
- [ ] Cancel / error 场景不累加(收到 `usage: None` 跳过 SQL)
- [ ] 旧 session 4 列 NULL,UI 显 "—"
- [ ] 颜色阈值边界测试:49% 绿 / 50% 黄 / 74% 黄 / 75% 红 / 100% 红
- [ ] UI 数字与 LLM 回传 usage 累加一致(端到端测试)
- [ ] 164+ cargo test 仍全过,新增 ≥8 token 相关测试
- [ ] pnpm build 干净
- [ ] spec 文档沉淀到 `.trellis/spec/backend/llm-contract.md`

## Definition of Done

- Tests added/updated:Anthropic usage 解析、OpenAI usage 解析、agent loop 累加、UI 颜色阈值、NULL 渲染"—"
- Lint / typecheck / CI green:pnpm build + cargo test 全过
- Docs/notes updated:`docs/IMPLEMENTATION.md §4` 追加 2 条决策,`CONTEXT.md` 已建
- Rollout/rollback considered:无破坏性,4 列 nullable → 旧 session 兼容
- Spec 沉淀:`.trellis/spec/backend/llm-contract.md` Scenario 段

### 排除(已确认)

- ❌ SessionList 简略:每个 session 行不加 token 数字(用户决定)
- ❌ 美元成本换算(ROADMAP 价值定位)
- ❌ per-turn 颗粒度(后续 C3 / B6 阶段按需 ALTER messages 表)
- ❌ Token 估算(用 LLM 回传精确数字)
- ❌ 独立统计面板
- ❌ 缓存命中率展示(只落库,UI 通过 hover tooltip 看分项)
- ❌ 历史 session 回填
- ❌ 跨 session 聚合 / 全局统计

## Technical Notes

### 实施顺序(待确认 1 PR vs 拆 PR)

候选拆分:
- **1 PR**:LLM 解析 + DB schema + agent loop + UI + spec + 决策日志,全部合一个 commit
- **3 PR**:PR1 = LLM 层解析(后端);PR2 = DB schema + agent loop(后端);PR3 = 前端 UI + spec 沉淀
- **2 PR**:PR1 = 后端全套(LLM + DB + agent loop);PR2 = 前端 + spec

### 关键文件

- `app/src-tauri/src/llm/types.rs` — `TokenUsage` struct + `ChatEvent::Done` 扩展
- `app/src-tauri/src/llm/provider/anthropic.rs` — `message_delta.usage` 解析
- `app/src-tauri/src/llm/provider/openai.rs` — `build_http_body` + 末 chunk usage
- `app/src-tauri/src/agent/chat.rs` — 收到 `Done { usage }` 时累加
- `app/src-tauri/src/db/migrations.rs` — ALTER 4 列
- `app/src-tauri/src/db/sessions.rs` — `add_token_usage` 函数
- `app/src/components/chat/ChatInput.vue` — `chat-input__token-usage` 组件
- `app/src/stores/chat.ts`(或新 store)— token 用量 state
- `.trellis/spec/backend/llm-contract.md` — Scenario 段
- `docs/IMPLEMENTATION.md` — 决策日志
- `CONTEXT.md` — 术语表(已建)

## Decision (ADR-lite)

### 决策 1:1 PR 全部合一个 feat commit

**Context**:R1-R8 互相耦合(LLM 解析 → ChatEvent::Done 字段 → agent loop 读取 → DB schema → 前端 SSE 监听 → UI 渲染,任一环节缺失,中间态都不能跑测试)。grill 阶段已经把所有 design 锁死,中途不会出现"需要重决策"的迭代。

**Decision**:1 PR 全部合(项目 50% feat 风格,例如 step 6a / 06-07 工具集扩展 / 06-10 fix-provider-config-hot-reload)。

**Consequences**:diff 大(估计 8-12 文件,Rust 5+Vue 2+SQL 1+spec 1+决策日志 1)。review 难度上升,但 commit message 可一次说清。

### 决策 2:`ChatEvent::Done` 携带 `usage` 字段(归一化边界)

**Context**:Anthropic `message_delta.usage` 和 OpenAI 末 chunk `usage` 都是协议原生字段,如果让 agent loop 知道 protocol-specific 字段,会破坏 Provider 抽象。

**Decision**:在 `ChatEvent::Done` 上加 `usage: Option<TokenUsage>` 字段,Anthropic / OpenAI provider 在内部归一化到统一 schema(4 字段:`input_tokens` / `output_tokens` / `cache_creation_input_tokens` / `cache_read_input_tokens`)。agent loop 拿到的是 protocol-agnostic 4 字段。

**Consequences**:IPC 字段 BC break(下游需要适配新字段),但 Provider 抽象保持干净,未来 Gemini / Ollama 加进来只需要补一个归一化点。OpenAI 端必须发 `stream_options: { include_usage: true }`,否则末 chunk 不携带 usage。

### 决策 3:总用量口径 = `sum(input_tokens) per turn`

**Context**:Anthropic schema 中 `input_tokens` 已包含 `cache_creation_input_tokens` + `cache_read_input_tokens`。4 列 sum 会让 cache 命中 token 双重计(在 `input_tokens` + `cache_read_input_tokens` 中重复)。Anthropic 官方 statusline 取"current context usage"也是这个口径。

**Decision**:UI 显示口径 = `sum(input_tokens) over turns`(Anthropic 4 列 sum 的一个特殊子集),分母 = `ModelRow.context_window`(默认 200K)。`output_tokens` **不计入** context 压力(那是响应,不是 context)。4 列单独落库供未来使用。

**Consequences**:cache 命中越多,数字增长越慢,激励 cache 优化。`output_tokens` 在 UI 不展示(只入 DB),用户看不到响应消耗。后续 C3 / B6 阶段如需 per-turn 颗粒度,可 ALTER messages 表加列。
