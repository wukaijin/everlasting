<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: 群聊 per-discussion / per-speaker 计费核算(GCE M4c,2026-09-08,task 09-08-gce-m4c-cost-governance-modal-redesign)

### 1. Scope / Trigger

- 触碰任何一处计费聚合实现:db 查询 / 讨论库 subquery / 脚本客户端聚合 / 预算硬停口径。
- 为什么需要 code-spec 深度:**同一口径有四个实现点**(见 §2),漂移任一处 = 两处 UI
  数字对不上或预算语义分裂;且历史上已有一次误判记录(见 §7)。

### 2. Signatures(四个实现点,必须钉同一口径)

| 实现点 | 位置 | 消费方 |
|---|---|---|
| db 查询 | `db/trace.rs::group_chat_token_usage(pool, session_id)` → `GroupChatTokenUsage{total:u64, by_speaker:Vec<SpeakerTokens{speaker,tokens}>}`(tokens DESC) | Tauri cmd + `POST /api/v1/sessions/group_chat_token_usage`;GUI edit 弹窗成本区 |
| 场级 subquery | `db/search_group_chat.rs` list/search 的 `total_tokens` 列(`Option<u64>`,NULL=无计费轮) | 讨论库 hit(前端「—」) |
| 客户端聚合 | `scripts/group-chat-run.mjs::aggregateTokens(turnTraces, messages)` → `{total, per_speaker}` | M1 转录统计行 + MCP `discussion_result.stats.tokens` |
| 预算硬停 | C1.2 轮头累计(`stop_reason=budget`,09-08-gc-c1-stoploss) | 声明面 = metadata `token_budget` 四通道 |

### 3. Contracts(口径本体,逐字对齐)

- **计费 = 四字段求和**:`input_tokens + output_tokens + cache_creation_input_tokens +
  cache_read_input_tokens`。`context_input_tokens` 是观测口径(与 input 重叠),
  **计入即双计**。
- **行过滤(全实现点同集)**:`m.role='assistant'` + `m.speaker IS NOT NULL` +
  `t.token_usage_json IS NOT NULL` + `t.run_id=''`(worker 行隔离,08-20 契约)。
- **对齐**:trace × messages 按 `(session_id, seq)` JOIN;retry 经 UPSERT 覆盖同键,
  SUM 不双计。
- `COALESCE(json_extract(...), 0)`:legacy 4-field usage JSON(无 context_input)照常求和。

### 4. Validation & Error Matrix

- 调用不存在的 session → 空结果(total=0, by_speaker=[]),不报错(核算为读侧增强面)。
- script/MCP 客户端聚合 `list_turn_traces` 失败 → M1 转录省略计费行 / MCP result 省略
  `tokens` 键(降级不污染既有字段);GUI 两查询失败 → 成本区「—」,不阻塞编辑。

### 5. Good/Base/Bad Cases

- Good:多 speaker 多轮 + worker 行同 seq + retry 覆盖 + legacy JSON → 各 speaker 精确
  四字段和(db fixture 总 2067;client fixture 总 450,均精确数字断言)。
- Base:从未开场的群 → `total=0` / hit `total_tokens=NULL`(与「跑了但 0 成本」区分)。
- Bad:`SUM` 只扫 turn_trace 不 JOIN messages(worker/无归属混入)→ 与 db 侧数字漂移;
  加 `context_input` → 双计。

### 6. Tests Required

- db:`group_chat_token_usage_sums_four_billed_fields_per_speaker` /
  `..._empty_session_returns_zero`(trace.rs);`browse_carries_total_tokens_...`
  (search_group_chat_tests.rs)。
- script:`aggregateTokens` 口径用例(worker/无归属/bad-JSON/降序;run.test.mjs);
  MCP `coreResult` tokens + 失败省键(mcp.test.mjs)。
- 生效链:fire 透传两态(scheduler tests_tick)+ C1.2 三破坏剧本(tests_group_chat.rs)。

### 7. Wrong vs Correct

- **Wrong(历史误判,M1 交付时记录)**:「per-speaker token 延后——turn_trace 行按 LLM
  调用段落落库,与 speaker 对齐有歧义,需先在 trace 行打 speaker 标签」。
- **Correct(2026-09-08 勘误)**:不需要任何 schema 变更——`messages.speaker` 按
  `(session_id, seq)` JOIN 即可(08-10 cache-rate 查询同模式先例,M4c 三个实现点实证)。
  再遇到「按段落归属」的聚合需求,先查 messages 侧已有维度,再谈加列。
