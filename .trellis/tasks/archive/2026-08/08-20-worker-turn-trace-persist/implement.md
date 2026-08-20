# Implement — worker turn_trace 度量盲区闭合

> 3 PR,顺序执行;每 PR 独立可测可 revert。验证命令见文末。

## PR1 — 存储层:run_id 列 + 表重建 + db API + 旁路 upsert 归位(后端)

- [ ] `db/migrations/schema.rs`:turn_trace CREATE 加 `run_id TEXT NOT NULL
  DEFAULT ''` + `UNIQUE(session_id, run_id, seq)`;删旧 `idx_turn_trace_session_seq`
  的重建(保留 IF NOT EXISTS 语句无害亦可,以新旧两查询都能走索引为准);加
  partial index `idx_turn_trace_run`。
- [ ] `db/migrations/schema_helpers.rs`:`rebuild_turn_trace_with_run_id`
  (探针 pragma_table_info → 五步舞,**显式列清单拷贝**,事务包裹,见 design §1.2);
  挂进 migrations 链(位置:turn_trace CREATE 之后)。
- [ ] `db/trace.rs`:4 个 upsert 加 `run_id: &str` 参 + SQL 扩;`TurnTraceRow`
  加 `run_id`;`list_turn_traces` 加 `AND run_id = ''`;新
  `list_worker_turn_traces(pool, run_id)`;既有测试改调用点(传 `""`),
  新测试按 design §5。
- [ ] `agent/trace.rs`:`record_compaction` / `record_loop_hint` 加 `run_id`
  参(record_breadcrumb 不动,内部传 `""`)。
- [ ] `agent/chat_loop/drive.rs`:两处 record_* 调用传 `run_key`(穿参见 PR2
  —— 本 PR 先以 `worker_run_id` 未穿入的现状传 `""` 保持行为,PR2 接线;**或**
  PR1 直接连带穿参一步到位,实施时按改动体量择一,择后者则 PR1/PR2 边界以
  "Done 臂是否开闸"划分)。
- [ ] 测试:AC1 全部断言(迁移幂等/哨兵/共存/回读)。

## PR2 — 写点开闸 + 集成回归(后端)

- [ ] `agent/chat_loop.rs` → `drive_turn` 穿 `worker_run_id: Option<&str>`;
  `drive_turn` 顶 `run_key`。
- [ ] Done 臂(drive.rs:1254-1279):`update_last_turn_usage` 门不动;
  `upsert_turn_trace_token` 门改 `!skip_persist || !run_key.is_empty()` 传
  run_key;顺手删 images_tok/at_files_tok 里已死的 `skip_persist ||` 分支
  (整个块在 `!skip_persist` 内,条件恒真)。
- [ ] `agent/trace.rs` record_* 两调用点接 run_key(PR1 遗留的接线)。
- [ ] 集成测试:researcher dispatch(多轮)→ AC2(run 行:usage_json/tools_token
  非空、context_window 落值、memory/images/at_files NULL)+ AC3(run_id='' 无
  worker 行、list_turn_traces 不含 worker 行、sessions.last_* 未被 worker 写)。
  worker compaction/loop_hint 集成构造若成本高:以 db 单测 + 代码门记录处置
  (prd AC4 允许)。
- [ ] IPC:`list_worker_turn_traces` Tauri command + lib.rs + daemon route +
  transport 两侧(`grep -rn "list_turn_traces"` 镜像全部注册点,含
  CMD_TO_DOMAIN);oneshot 路由冒烟测试(daemon 侧若有 trace 路由测试簇则
  加一行,无则照 D2 先例补最小 oneshot)。

## PR3 — 前端:drawer per-run Token 明细

- [ ] `types/turnTrace.ts`:TurnTraceRow 加 `runId: string`(解析容错
  `?? ''`,旧后端 payload 无字段)。
- [ ] transport 前端侧方法 + `useSubagentsStore.loadRunTurnTraces(runId)`
  (`runTracesByRunId` reactive Map + per-run loading;复用 parseTurnTraceRow)。
- [ ] `SubagentDrawer` 子组件 `WorkerTurnTraceList.vue`:折叠区默认收起、
  展开按需拉取一次;行格式 `#seq · in/out · cache-read(率) · tools · ctx%`;
  空态「无 per-turn 记录」(旧 run / 迁移前 binary)。
- [ ] 测试:store action(mock IPC)+ 组件渲染(空态/行);traceStore 既有
  测试零改动跑绿。

## 收口

- [ ] 全量后端 + 前端 + clippy/fmt/vue-tsc(命令见下);live 可选(单轮 smoke
  不 dispatch worker,集成已覆盖;若手测:真机 dispatch 一个 researcher 后
  drawer 看 Token 明细)。
- [ ] spec 回写:`database-guidelines`(turn_trace run 维度 + 空串哨兵决策 +
  重建迁移显式列清单教训)、`token-usage-tracking`(worker 行切片语义)、
  `subagent-runs-schema`(与 turn_trace 的 run 关联)、`frontend/state-management`
  (runTracesByRunId 粘性缓存,如有新模式)。

## 验证命令

```bash
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd app && pnpm test && pnpm build
```

## 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|------|------|------|
| `db/migrations/schema_helpers.rs`(重建) | 拷贝列序错位 → 数据错列 | PR1 revert;迁移探针幂等,重跑安全 |
| `db/trace.rs` upsert 签名 | 4 处调用点漏改 → 编译期暴露 | 编译器闭环 |
| `drive.rs` Done 臂 | 门逻辑写反(worker 行写进主行) | AC3 集成锁;PR2 revert |
| `drive_turn` 穿参 | 46 参签名,调用点唯一(chat_loop.rs) | 纯透传 |

## start 前检查

- [x] prd/design/implement 三件齐备;research 落盘(worker-path-analysis.md)。
- [x] implement.jsonl / check.jsonl 真实条目(非 seed)。
- [x] 基线(2026-08-20 实测):后端全量 1867/1870,3 失败
      (`dispatch_main::..._guard_does_not_evict_parent_session_active` /
      `plan_mode::..._write_denied` / `serve_daemon_shutdown_completes_with_active_sse`)
      两轮同集合复现、**隔离单跑全过** —— 负载型既有 flaky(与 journal 104/39
      记录同类),归因 main 非本任务;前端 1122/1122 全绿。
