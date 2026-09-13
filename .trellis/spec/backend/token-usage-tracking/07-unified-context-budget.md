<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: 统一估算 + at_files/system/window 三新列 + 实发口径(unified-context-budget,2026-08-19)

`turn_trace` 新增 `at_files_token`(全部 user message 的 @-token 注入正文 est 之和;@图走 images_token 不重复计)/ `system_token`(system prompt 本体 + skill listing 归因)/ `context_window`(请求时窗口快照,TurnCard 预算行分母,旧行 NULL 前端回退 200_000)。写点与既有切片同一 Done upsert(`!skip_persist` gate,worker 轮 None;零注入 → at_files NULL)。

- **两类口径永不互相加计**(任务 prd D8,评审 F1 教训):总量 =
  `budget::estimate_request_tokens(system, tools_json, messages)` 三部件
  加法 —— memory 头对/skill listing/@文件/图片物理在 messages 里,公式
  上再单独加计任何一项即重复计数;归因切片只做占比条,之和 ≤ 总量。
- **压缩触发口径统一切换**:摘要触发(0.85)/ postcheck(0.95)/ 机械
  `compact_messages`(经 `extra_tokens` 参,无 gate 群聊/worker 同受益)
  从 messages-only 改统一总量 —— 修 tools+system 挤窗漏计洞(小窗口
  模型下请求可整体超窗而压缩不触发)。
- **实发口径**(prd D9):关卡⑤硬卡裁剪发生时,trace 各切片列记
  `预裁 − freed`(臂 3 触发时 memory_token 改记目录态值)—— 与
  provider `context_input` 可比;预裁值只进 audit payload。
- turn-smoke 报告列加 at_files/system/ctx_win + at_pct。
- 完整闸门语义见 [pattern-budget-gate](../agent-loop-architecture/pattern-budget-gate.md)。
