<!-- Moved from memory/scenario-autonomous-memories.md 2026-09-13 (doc-split) -->

#### P5 quality-layer contract (verified soft-intercept + state-machine promotion + hygiene job) — 2026-06-29, 06-29-am-p5-quality

> **P5 是 P3 的质量收口层**(spike-007 §9 步 6 + §3 状态机 + §4 软拦截分档)。三块:(1) verified pitfall 软拦截重判(动 loop,兑现"第一时间规避");(2) 状态机自动晋升 candidate→active→verified(靠 hit_count + 存续时长);(3) 异步卫生 job(dedup/降权)。
>
> **P5 不改** P3/P4 的 seam 位置,**不改** `permissions::check()` 内部,**不改** `ToolResultPayload` shape —— 它把 P3 的 `recall_pitfall_footnote`(返 `Option<String>`)升级为分档 `recall_pitfall`(返 `PitfallRecall` enum),并在 chat_loop 两 path 的 pre-execute seam 加 SoftBlock 短路。

**Signatures**(`agent/permissions/check.rs` + `db/memories.rs` + `agent/memory_hygiene.rs`):

```rust
// agent::permissions::check — P5 分档
pub enum PitfallRecall {
    None,
    Footnote(String),                              // active / candidate / 二次命中(同 P3 注脚)
    SoftBlock { hint: String, memory_id: String }, // verified + is_full_match + 本 session 未拦过
}
pub const PITFALL_SOFT_BLOCK_ENABLED: bool = true;  // feature flag;false → 退回 P3 纯注脚
pub async fn recall_pitfall(
    db: &SqlitePool, tool_name: &str, tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,             // session 级 D1 防循环记账
) -> PitfallRecall;  // DB err → warn! + None(不阻断)

// db::memories — P5 状态机(嵌 bump_hit_count 同连接,避免 bump↔promote 竞态)
pub async fn promote_if_eligible(pool: &SqlitePool, memory_id: &str) -> Result<(), sqlx::Error>;
pub async fn update_status(pool, memory_id, new_status: MemoryStatus, demoted_reason: Option<&str>)
    -> Result<(), StatusTransitionError>;          // 事务 + 转换矩阵校验
pub async fn find_pitfalls_by_trigger_all_status(...) -> ...;  // P5 放宽版(含 candidate/active/verified)
pub async fn count_memories_by_scope_kind(pool, scope, kind) -> i64;  // 卫生 job 触发计数

// agent::memory_hygiene — P5 卫生 job
pub fn char_trigrams(s: &str) -> HashSet<String>;  // Unicode char(中文友好,非 byte)
pub fn jaccard(a: &str, b: &str) -> f32;            // char-trigram 集合 Jaccard 0.0..=1.0
pub async fn run_hygiene_pass(pool: SqlitePool);    // dedup_pass + age_out_pass,fire-and-forget
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 软拦截触发 | `status='verified'` AND `is_full_match` AND `memory_id ∉ already_blocked` AND `PITFALL_SOFT_BLOCK_ENABLED` | 首条命中 row 胜出 SoftBlock;其余进 Footnote |
| SoftBlock 回合改法 | **不调** `execute_tool`、**不写** `tool_executed` audit、构造 `ToolResult{content:hint, is_error:false}`、`emit_tool_result`、记 `memory_id` 入 set、`bump_hit_count` | 复用 `Decision::Deny` 的"不执行+回填"模式;下一轮 send LLM 重判 |
| `is_error=false` | 提示非错误 | 避免 LLM 误判"工具坏了"换工具;语义是经验提示,不是错误 |
| D1 死循环防护 | 每条 pitfall 每 session 软拦截 **1 次**;同坑二次(`already_blocked` 含)→ 降级 Footnote + 正常 execute | 保证不卡到 `MAX_TURNS`(50);`already_blocked` = session 级 `Arc<Mutex<HashSet<String>>>`,loop 顶部建(同 `FailureTracker` 生命周期) |
| `is_full_match` 语义 | 行上每个 `Some(_)` 字段(command_pattern 子串 + path_globs glob)都匹配,且至少一个 `Some` | ⚠️ 偏离 design 字面"三者皆中"——内置工具探针对称(Shell 无 path 探针、Path 工具无 command_pattern),字面不可行;宽泛 pitfall(皆 None)→ 永不 SoftBlock,降级 Footnote(比字面更保守) |
| 晋升 candidate→active | `hit_count ≥ 2`(被召回命中过 2 次) | `promote_if_eligible` 嵌 `bump_hit_count` 同连接读回 hit_count + `update_status` |
| 晋升 active→verified | `hit_count ≥ 5` AND `created_at` 距今 ≥ 3 天 | "存续时长"代理"未翻车"(v1 无跨 session 翻车信号,P4 `FailureTracker` 是 session 内) |
| 非法转换 | `update_status` 矩阵拒绝 → `StatusTransitionError::Illegal` | 合法集:candidate→{active,verified,demoted}, active→{verified,demoted}, verified→demoted, demoted→active |
| recall filter 方向 | session-start FTS **保持** `IncludeCandidate`;pre-tool **放宽** candidate+active+verified 分档 | ⚠️ P5 推翻 P2 注释"收紧到 ActiveVerifiedOnly"——收紧会掐断 candidate 晋升(candidate 靠被召回命中晋升,排除则永不命中) |
| 卫生 dedup | 同 `(scope,kind)`:pitfall 按 trigger_key 全等;其余按 char-trigram Jaccard >0.7 | 合并保留高 confidence/高 hit_count,`delete_memory` 删冗余 |
| 卫生 age-out | `status IN (candidate,active)` AND age(`last_used_at`‖`created_at`)>30 天 AND `hit_count<2` → `Demoted("aged_out")` | verified 豁免(已证明价值) |
| 卫生 job 触发 | `insert_memory` 后 `(scope,kind)` 计数 `%10==0` → spawn;app 启动(`lib.rs` setup)spawn 一次 | `if !cfg!(test)` 守卫防测试 flaky;fire-and-forget,失败 warn! 吞 |
| Decision 语义 | **不参与** `check()` 决策链 | SoftBlock 在 Allow 之后、execute 之前短路,非 Deny |

**P3 ↔ P4 ↔ P5 三层闭环**(集成测试 `agent_loop_p5_soft_block_short_circuits_execute` + `agent_loop_p5_soft_block_second_hit_degrades_to_execute` 锁定):
1. session A:P4 旁路 reflection 写 `active` pitfall(带 trigger_key)
2. session B+:P3/P5 pre-tool recall 命中 → bump hit_count → `promote_if_eligible` 晋升 active→verified(多次命中 + 存续 3 天)
3. session N:verified + `is_full_match` → P5 SoftBlock 短路 execute → LLM 重判(第一时间规避);同坑二次命中 → Footnote + 正常执行
