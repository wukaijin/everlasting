<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario: worker per-turn 行 + run 维度唯一键(2026-08-20,task 08-20-worker-turn-trace-persist)

`turn_trace` 加 `run_id TEXT NOT NULL DEFAULT ''` 列,唯一键重建为
`UNIQUE(session_id, run_id, seq)`(`''` 哨兵 = 主 loop 行;worker 行 =
`subagent_runs.id`)。worker(`skip_persist=true`)的 Done upsert 开闸写
(父 sid, run UUID, seq)行;`update_last_turn_usage` 的 `!skip_persist` 门
**不动**(snapshot 隔离,RULE-A-015 reversal)。

- **seq 空间冲突是根因**(为什么必须动唯一键):worker loop 的 seq 从父
  DB messages max+1 起(`init.rs`),与父后续轮次**共享同一区间** —— 旧
  `UNIQUE(session_id, seq)` 下 worker 行会被父后续 Done upsert 撞行覆写
  (并发 fan-out 的多个 worker 亦互撞)。run 维度并入锚点后三方共存。
- **worker 行切片语义**(与列文档契约一致):usage_json + tools_token +
  system_token + context_window 落值;memory_token **按契约记 NULL** ——
  注意 worker 经共享 init 路径同样注入 memory banner(有层级时是真
  Some),在写点显式归 NULL(度量面排除 worker,非"估算为零");
  images/at_files worker 恒 NULL(无附件/无 @注入)。
- **读侧路由**:`list_turn_traces` 只回主行(`WHERE run_id = ''`,前端
  `Map<seq, TurnTrace>` 契约零变化);worker 行走
  `list_worker_turn_traces(run_id)`(SubagentDrawer「Token 明细」)。
  `list_speaker_cache_usage` 的 join 也要 `AND t.run_id = ''`(worker 行
  与父 messages 共享 seq 区间,不排除会误配)。
- **旁路写点归位**(本任务顺带修的既有跨归因 bug):机械压缩
  `record_compaction` 与 C2 软提示 `record_loop_hint` 原本**无 worker 门**,
  worker 撞线时以 (父 sid, worker seq) 写主行 —— 父后续同 seq 的 Done
  upsert 合并进该行,父卡片显示从未发生的压缩/loop 提示。现两写点按
  `run_key` 路由(worker 写 run 行,主路不变)。db 层 4 个 upsert 统一
  加 `run_id: &str` 参。
- **降级**:`insert_run` 失败 → `worker_run_id=None` → run_key='' →
  worker 写点自然不写(不造孤儿命名空间)。
- worker 行的 seq 是 loop 内游标,**勿当父 messages 全局 seq 消费**。
