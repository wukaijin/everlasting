<!-- Moved from memory/scenario-autonomous-memories.md 2026-09-13 (doc-split) -->

#### Pre-tool pitfall recall contract (P3, layer 2 of 2) — 2026-06-29, 06-29-am-p3-tool-recall

> **Layer 2** = 工具执行前召回(spike-007 §4)。**与 layer 1(P2 session-start FTS5)是两套独立检索**:
> - Layer 1:per-turn FTS5(query = most-recent user message text,模糊召回多种 memory kind)
> - Layer 2:per-tool `trigger_key` 精确匹配(只召回 `kind = 'pitfall'` + `status = 'active'`)
>
> Layer 2 **不**走 FTS5,**不**消费 layer 1 的 query 文本,**不**产出新 message 块 — 它产出的是一个 plain-text 注脚,prepend 到 `tool_result.content`。

**Signatures**(已在 `db::memories` 由 P1 产出;08-08 拆分后函数在 `db/memories/search.rs`):

```rust
// db::memories::find_pitfalls_by_trigger
pub async fn find_pitfalls_by_trigger(
    pool: &SqlitePool,
    tool_name: &str,
    command_pattern: Option<&str>,   // shell 命令字符串片段(精确匹配)
    path: Option<&str>,              // 文件路径(精确匹配)
) -> Result<Vec<AutonomousMemoryRow>>

// agent::permissions::recall_pitfall_footnote (P3 新增)
pub async fn recall_pitfall_footnote(
    pool: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,  // 完整 tool_input
) -> Result<Option<String>, sqlx::Error>  // 命中 → Some("⚠️ Memory: ...") / 不命中 → None
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 触发时机 | `chat_loop` 拿到 `Decision::Allow` 之后、`execute_tool` 之前 | 不在 `permissions::check()` 内部(见 permission-layer.md §4.2) |
| 召回对象 | `kind = 'pitfall'` AND `status IN (candidate, active, verified)` | P3 落地时 active-only;**P5 放宽**到三态分档(`recall_pitfall`):verified+`is_full_match`→SoftBlock,active/candidate→Footnote。见 P5 contract |
| 匹配方式 | `find_pitfalls_by_trigger` 的 `tool_name` + `command_pattern` / `path` **精确匹配** | 命中 `idx_am_pitfall` 索引(migration.rs:756);O(1) 不是 O(n) |
| 注脚格式 | `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n• [title] content\n...` | imperative 强提示;多命中时多行 bullets |
| 注入位置 | `tool_result.content` 前缀(plain text),**envelope wrap 之前** | `tool_use_id` 配对 / `is_error` 语义 / envelope `{result, cwd}` shape 全部不变 |
| `bump_hit_count` 时机 | 命中后 fire-and-forget(`tokio::spawn`) | 不阻塞 recall 步骤;P5 状态机读取 `hit_count` 决定晋升 |
| 召回失败 | `Err(sqlx::Error)` → `tracing::warn!` + 返回 `None` | 工具照常执行(降级放行);**永不阻断工具执行** |
| Decision 语义 | **不参与**决策链,`check()` 仍返回 `Decision::Allow` | 注脚是 hint,不是 gate |

**为什么 layer 2 是 `trigger_key` 精确匹配而非 FTS5**:
- 工具执行前的"我要不要做这个"是 yes/no 决定,精确率优先(漏一条能用 layer 1 补,注入一条错的污染工具输出)
- FTS5 bm25 在 trigger_key 字段上召回会引入与本工具无关的 pitfall(噪音)
- `command_pattern` + `path` 双键命中让"同类操作"语义无歧义
