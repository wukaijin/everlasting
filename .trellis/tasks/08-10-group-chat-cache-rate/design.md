# Design — 群聊参与者/主持人最近一次缓存率显示

## 架构与边界

三层,全部基于现有模式,零 schema 改动:

```
GroupChatConfigModal (edit 模式打开时)
  └─ transport.invoke("group_chat_cache_rates", { sessionId })
       └─ commands::sessions::group_chat_cache_rates  (Tauri 命令, lib.rs 注册)
            └─ db::trace::list_speaker_cache_usage (SQL join, 纯 db 层, 可单测)
```

- 后端返回**原始计数字段**(`cache_read` / `context_input`),不返回百分比——百分比计算放前端纯函数(`utils/tokenUsage.ts` 同文件追加或新 `utils/cacheRate.ts`),延续项目"业务逻辑放纯函数可单测"的惯例(tokenUsage.ts 先例)。
- 前端 `GroupChatConfigModal.vue` 持有 `SpeakerCacheUsage[]` 状态,按 `speaker` 映射到参与者行 + 主持人区。

## 数据流与 SQL

`turn_trace(session_id, seq, token_usage_json)` 与 `messages(session_id, seq, role, speaker)` join:

```sql
SELECT m.speaker,
       json_extract(t.token_usage_json, '$.cache_read_input_tokens') AS cache_read,
       json_extract(t.token_usage_json, '$.context_input_tokens')   AS context_input
FROM messages m
JOIN turn_trace t ON t.session_id = m.session_id AND t.seq = m.seq
WHERE m.session_id = ?1
  AND m.role = 'assistant'
  AND m.speaker IS NOT NULL          -- 普通聊天/worker 行 speaker 为 NULL,天然排除
  AND t.token_usage_json IS NOT NULL -- 该轮无 usage(取消/出错/只写了其他维度)
  AND m.seq = (
      SELECT MAX(m2.seq) FROM messages m2
      WHERE m2.session_id = m.session_id
        AND m2.speaker = m.speaker
        AND m2.role = 'assistant'
  )
```

语义保证(均为既有事实,查询只是投影):
- assistant 消息行 seq == 该轮 turn seq(`drive.rs` push 时用当前 seq),join 不偏移。
- 群聊内 seq 全局连续(`init.rs:100-107` 每调用从 DB max(seq)+1 起),每个 speaker 的 max(seq) 即 TA 最近一次发言轮。
- rewrite 产物(user role 带 speaker)被 `role='assistant'` 过滤。
- 同一轮重试:turn_trace 按 (session, seq) 覆盖(`trace.rs:66-67`),天然取最后一次 usage。

**分母保护**:`context_input_tokens` 带 `#[serde(default)]`(`llm/types/usage.rs:65`),legacy 行可能为 0 → 前端 `cacheRatePercent` 对 `context_input <= 0` 返回 `null`(显示 "—"),不在 SQL 里过滤(保留数据,前端决定展示)。

## 契约

```rust
// db/trace.rs (或 db 层新函数)
pub struct SpeakerCacheUsage {
    pub speaker: String,
    pub cache_read: u32,     // 该 speaker 最近一次有 usage 轮次的 cache_read_input_tokens
    pub context_input: u32,  // 同上轮的 context_input_tokens
}
pub async fn list_speaker_cache_usage(pool: &SqlitePool, session_id: &str)
    -> Result<Vec<SpeakerCacheUsage>, sqlx::Error>
```

```rust
// commands/sessions.rs
#[tauri::command]
pub async fn group_chat_cache_rates(state: ..., session_id: String)
    -> Result<Vec<SpeakerCacheUsage>, AppCommandError>
// inner + wrapper 模式(照抄 commands/permissions.rs:397-411 list_turn_traces)
```

```ts
// 前端类型(chat.types.ts 或 GroupChatConfigModal 内)
interface SpeakerCacheUsage { speaker: string; cache_read: number; context_input: number }
```

```ts
// utils/tokenUsage.ts 追加(纯函数,单测)
cacheRatePercent(cacheRead: number, contextInput: number): number | null
// contextInput <= 0 → null;否则 Math.round(cacheRead / contextInput * 100)
```

## 前端展示

`GroupChatConfigModal.vue`,仅 `mode="edit"`:

- 打开时(`onOpenChange` 或 `watch(open)` 变 true 且 mode=edit)invoke 一次,结果存 `cacheRates: Record<speaker, SpeakerCacheUsage>`。
- 每个参与者行(名字/模型字段之后)追加只读行:`` 缓存率 <X>% `` 或 `` 缓存率 — ``(`context_input=0` / 无记录 / 请求失败)。
- 弹窗底部新增只读"主持人"区:`主持人 · <modelLabel(session.model_id)> · 缓存率 X%`。model 从 `currentSession.model_id` 取;主持人在参与者数组中不存在,key 固定为 `"moderator"`(与后端 speaker 值一致,`group_chat_loop.rs:277`)。
- `mode="create"` 不 invoke 不显示(新群聊无轮次)。
- 请求失败静默:显示 "—" 并 `console.error`(与弹窗现有错误处理分离——缓存率是只读辅助信息,不阻塞编辑)。

## 兼容性 / 边界

- `clear_session_trace` 清空 turn_trace → 查询返回空 → 全部显示 "—"。预期行为,PRD AC4。
- 旧群聊(07-29 之前?)可能无 turn_trace 行 → 同上。
- 跨 provider:OpenAI 无 cache_creation,但 cache_read 存在(`cached_tokens` 归一化,`usage.rs:37-38`);兼容代理可能 cache 字段全 0 → 缓存率 0%(真实数据,非错误)。
- 不修改 `turn_trace` / `messages` / `sessions` schema;不新增迁移。

## 回滚

- 纯新增:新命令 + 弹窗只读展示。回滚 = 移除命令注册 + 弹窗代码。无数据迁移、无既有行为改变。
