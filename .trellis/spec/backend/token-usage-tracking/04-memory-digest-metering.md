<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario:memory 指令块度量 + digest(WP1/WP2,2026-08-15)

> 来源:`08-15-memory-block-governance`(BACKLOG §3.1)。与 tools_token
> 完全同构的「切片单列」口径;digest 是内容侧手段,不改度量链路。

### memory_token 口径(与 tools_token 对称,逐条对齐)

- `turn_trace.memory_token`:cl100k 估算**实际注入的** memory 指令块
  (banner + wrappers + 层 body;digest 开启时即 digest 后体积 — WP1 的
  计算点在 init.rs 注入处,`LoopInit` 穿到 drive.rs Done 写点)。
- **per-request 常量**:memory 块每 request 组装一次,同一 request 的
  所有 turn 行同值(区别于 tools_token 每 turn 重估 — dispatch enum 等
  可能变)。
- 占比 = `memory_token / context_input_tokens`,**不 double-count**
  (context_input 已含 memory)。前端 `TurnCard.memoryPct` 同 toolsPct。
- `None` 语义:pre-column 行 + worker turn(worker 注入走
  `subagent/prompt.rs`,不在度量面 — design §3.5a)。
- upsert 契约:与 tools_token 同一 `upsert_turn_trace_token` 写点、同
  second-writer-wins(冲突双写)。

### 验证数据(2026-08-15,本仓库 live)

- 基线(digest off):memory_token=10124 / ctx=14079(72%)— 高于
  08-14 估算 ~7-8k,cl100k 对 CJK 实际计数 + wrapper 开销。
- digest on:memory_token=2080(28%),-79.5%;首轮 ctx -47%;tools
  3664→3805(Δ141 = load_memory_sections def,净收益仍 -6.6k)。
- 双轮 cache 率:on 99.8% vs off 99.7% — 不劣化(AC4)。
- turn-smoke `--turns N`:AC4 类双轮对比的标准入口(08-15 加)。
