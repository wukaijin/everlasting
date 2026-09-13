<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: Group-Chat Per-Speaker Cache Rate (2026-08-10, task 08-10-group-chat-cache-rate)

### 1. Scope / Trigger

- Trigger: 群聊会话中每个参与者(含主持人)展示各自**最近一次 LLM 调用**的缓存率。
- 为什么不能读 `sessions.last_*`:群聊每轮(主持人/参与者)都覆盖写同一 session 的 `last_*` 快照,最后一位发言者胜出——**group chat 的 `last_*` 是"最后一位发言者"的值,不是聚合也分不出说话人**。per-speaker 数据只能走 `turn_trace`。

### 2. 数据模式:turn_trace × messages.speaker join(关键)

`turn_trace(session_id, seq, token_usage_json)` 行**没有 speaker 列**,但群聊里 `messages` 的 assistant 行带 `speaker`(主持人 = `"moderator"`,参与者 = `participant.name`),且 **assistant 行 seq == 该轮 turn 的 seq**(`chat_loop/drive.rs` push 时用当前 seq 后 `seq += 1`)。seq 群聊内全局连续(`chat_loop/init.rs` 每次调用从 DB max(seq)+1 起)。因此:

```sql
SELECT m.speaker,
       COALESCE(json_extract(t.token_usage_json, '$.cache_read_input_tokens'), 0) AS cache_read,
       COALESCE(json_extract(t.token_usage_json, '$.context_input_tokens'), 0)   AS context_input
FROM messages m
JOIN turn_trace t ON t.session_id = m.session_id AND t.seq = m.seq
WHERE m.session_id = ?1
  AND m.role = 'assistant'
  AND m.speaker IS NOT NULL          -- 普通聊天/worker 行 speaker 为 NULL,天然排除
  AND t.token_usage_json IS NOT NULL -- 该轮无 usage(取消/出错/只写了其他维度)
  AND m.seq = (                       -- 每 speaker 最近一次发言轮
      SELECT MAX(m2.seq) FROM messages m2
      WHERE m2.session_id = m.session_id AND m2.speaker = m.speaker
        AND m2.role = 'assistant'
  )
```

- rewrite 产物(user role 带 speaker)被 `role='assistant'` 过滤。
- 同一轮重试:`turn_trace` 按 (session, seq) 覆盖(`trace.rs` upsert),天然取最后一次 usage。
- **不回退语义**(locked):某 speaker 最新一轮无 usage(如取消)→ 该 speaker 整行不返回,前端显示 "—";**不**回退到更早的有 usage 轮次。理由:缓存率 = "最近一次调用"的单次语义,最近一次调用没有 usage 就没有可算的数。

### 3. 缓存率口径(单次)

- `cache_rate = cache_read_input_tokens / context_input_tokens`,单次调用语义,非多轮聚合。
- 分母**必须**用 `context_input_tokens`(跨 provider 归一化总输入),不能用 `input_tokens`:Anthropic 的 `input_tokens` 不含 cache read/creation,OpenAI 的 `prompt_tokens` 已含 cached——用 `input_tokens` 会让两 provider 的命中率口径不一致。
- `context_input <= 0`(legacy 4 字段行,`#[serde(default)]` 补 0)→ 前端显示 "—",不在 SQL 过滤(保留数据,前端决定展示)。

### 4. 契约

```rust
// db/trace.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeakerCacheUsage {
    pub speaker: String,     // "moderator" 或 participant.name
    pub cache_read: u32,     // 该 speaker 最近一次有 usage 轮次的 cache_read_input_tokens
    pub context_input: u32,  // 同上轮的 context_input_tokens
}
pub async fn list_speaker_cache_usage(pool: &SqlitePool, session_id: &str)
    -> Result<Vec<SpeakerCacheUsage>, sqlx::Error>
```

IPC: `group_chat_cache_rates(sessionId)` → `Vec<SpeakerCacheUsage>`,三处注册(tauri invoke_handler + daemon route + `http.ts` CMD_TO_DOMAIN)。

### 5. 边界 / 测试要点

| 边界 | 行为 |
|------|------|
| `clear_session_trace` 清空 turn_trace | 查询返回空 → 全部 "—"(与回看功能共用数据,预期) |
| speaker 最新轮无 usage | 整行不返回(不回退,见 §2) |
| legacy 4 字段 JSON 缺 context_input | `COALESCE(json_extract(...), 0)` → 0 → 前端 "—" |
| 兼容代理 cache 字段全 0 | 缓存率 0%(真实数据,非错误) |
| 主持人 | speaker 固定 `"moderator"`(`group_chat_loop.rs` emit 值),model = `sessions.model_id`(前端 `SessionSummary` 已有) |

测试要点:每 speaker 只返回 max seq 轮的数字、无 usage 轮被跳过、最新轮无 usage 不回退(用"seq 9 有 usage + seq 10 无 usage"的 fixture 锁定,否则测试在两种 SQL 解释下都通过)、user/speaker-NULL 行排除。前端百分比计算是纯函数(放 `utils/tokenUsage.ts` 或同类),`context_input <= 0 → null` 可单测。
