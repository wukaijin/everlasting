<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: 摘要压缩旁路 usage(C3,2026-08-18)

- **口径**:LLM 摘要调用是**旁路 completion**(无 tools、禁 thinking 采集、
  4k 输出兜底)—— 其 `TokenUsage` **不混入**主 turn 的
  `update_last_turn_usage`(`context_input`/per-turn 记账口径不变),只进
  `compaction_json.summary_usage`(trace.rs 手工 json!,与 method 同写点)。
- **When this bites**:主 turn 的 token 统计永远不包含压缩开销 —— 想算
  真实成本要看 compaction_json;TracePanel 的 TurnCard token 字段因此
  不因压缩而跳变(展示的是请求上下文,不是总消耗)。
- 摘要求输入 = 模板 + prior-summary + transcript(预算 0.7×window,溢出
  丢最旧 + `[older transcript omitted]` 记号),输出 `clamp_summary_output`
  4k token 兜底。
