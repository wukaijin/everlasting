# PRD — worker turn_trace 度量盲区闭合

## Goal

subagent(worker)的每轮 LLM 调用目前不落 `turn_trace` 行(`skip_persist` 门),
per-turn 用量/切片/context 窗口对用户与后续成本聚合(如 A4+)完全不可见;且
worker 路径两处既有旁路写点(机械压缩 / loop 软提示)正以父 session 名义写主行,
构成跨归因污染。本任务把 run 维度并入 turn_trace 唯一键,补齐 worker per-turn
度量,归位旁路写点,并在 SubagentDrawer 提供 per-run 明细视图。

**用户价值**:worker 每轮 token 消耗(in/out/cache-read/tools 占比/窗口占用)
可查 —— 长任务 worker 的成本与上下文增长不再黑箱;父 session 的 trace 不再被
worker 行污染。

**来源**:unified-context-budget(08-19)PRD Out-of-Scope 明示「worker turn_trace
度量盲区(skip_persist 不写行)——独立小额 follow-up」;实现中发现的 seq 空间
冲突与旁路污染使「直接开闸」不可行,详见 `research/worker-path-analysis.md`。

## Background(代码探索 2026-08-20,详见 research/worker-path-analysis.md)

- worker 复用父 session_id + `skip_persist=true`(dispatch/drive.rs:111);
  run 级累计用量已有(`subagent_runs.token_usage_json`),per-turn 零记录。
- 切片数据在 worker 路径已算好(tools_token drive.rs:322 无条件;system/window
  经 LoopInit),只是 Done 臂写点(drive.rs:1254-1279)整体被 `!skip_persist` 挡掉。
- worker loop seq 从父 DB messages max+1 起(init.rs:163),与父后续轮次**共享
  seq 区间** → 现约束 `UNIQUE(session_id, seq)` 下直接开闸会互相覆写。
- 既有污染:record_compaction(drive.rs:568)与 record_loop_hint(drive.rs:1771)
  无 worker 门,worker 触发时以 (父 sid, worker seq) 写主行;父后续同 seq 的
  Done upsert 合并进该行 → 父卡片显示未发生的压缩/loop 提示,worker 行为记到
  父头上。

## Requirements / Decisions

- **R1(存储,D1)**:`turn_trace` 加 `run_id TEXT NOT NULL DEFAULT ''` 列,唯一键
  重建为 `UNIQUE(session_id, run_id, seq)`;主行哨兵空串(NULL 会破坏 upsert
  语义,SQLite UNIQUE 视 NULL 互异);run_id 不加 FK('' 非法外键值;subagent_runs
  行无独立删除路径,生命周期由 session_id CASCADE 兜底)。表重建迁移幂等,旧库
  旧行全量 run_id='';greenfield CREATE 同步含新列。
- **R2(worker per-turn 行,D2)**:Done 臂对 worker 写 (父 sid, run UUID, seq)
  行:token_usage_json + tools_token + system_token + context_window 落值;
  memory_token / images_token / at_files_token 保持 NULL(worker 语义:
  prompt.rs 注入 / 无附件 / 无 @注入,与列文档一致)。`update_last_turn_usage`
  门不动(snapshot 隔离,RULE-A-015 reversal 不碰)。
- **R3(旁路归位,D3)**:record_compaction / record_loop_hint 加 run 维度 ——
  worker 触发时写 run 行,主路行为不变;db 层 4 个 upsert API 统一加 run_id
  参数(record_breadcrumb 签名不动,worker 恒不触发)。
- **R4(读路径兼容,D4)**:`list_turn_traces` 只回主行(`run_id = ''`),既有
  前端 `Map<seq, TurnTrace>` 契约零改动;新增 `list_worker_turn_traces(run_id)`
  (seq ASC,复用 TurnTraceRow/parseTurnTraceRow),IPC 注册链按 `list_turn_traces`
  现有链镜像(Tauri command / invoke_handler / daemon route / transport)。
- **R5(前端视图,D5)**:SubagentDrawer per-run「Token 明细」折叠区:每轮
  in/out/cache-read/cache 率/tools_token/窗口占比的紧凑行,经 useSubagentsStore
  新 action 按需拉取;复用 types/turnTrace.ts 解析,无新类型族。
- **D6(降级语义)**:`worker_run_id_opt=None`(insert_run 失败)→ worker 写点
  自然不写(run_key 空),与 permission key 降级先例一致;不造孤儿命名空间。

## Out of Scope

- TracePanel 混排 worker 轮(run 维度视图属 drawer;如需全局时间线另立任务)。
- worker memory 切片度量(prompt.rs 注入路径,§3.5a 语义维持)。
- worker 接入关卡⑤ BudgetTrim / LLM 摘要压缩(gate 排除 worker 是既有设计)。
- A4+ 成本聚合(消费方,独立任务;本任务只保证数据齐备)。
- 历史 worker 旁路污染行的清洗(存量行无法区分来源,不回填不清洗;新写入归位)。

## Risks / Deferred

- **表重建风险**:turn_trace 行数量级小(每轮一行),重建事务轻;参照
  widen_subagent_runs 先例(探针短路幂等);拷贝必须显式列清单(先例
  `SELECT *` 依赖列序不变,本任务加了列,不可照抄)。
- **drive_turn 穿参**:又 +1 参(现 ~45);已知债务,不为小额任务重构签名。
- **worker 行 seq 语义**:loop 内递增、非父 messages seq —— drawer 按序展示即
  可,文档写明勿当全局 seq 用。

## Acceptance Criteria

- **AC1 迁移**:含既有 turn_trace 行的旧库升级后 —— run_id 列存在、旧行全部
  run_id=''、`UNIQUE(session_id, run_id, seq)` 生效(主行同 (sid,'',seq) 二写
  仍 upsert;主行与 worker 行同 seq 共存不冲突);重复跑迁移 no-op;greenfield
  建库即含 run_id。
- **AC2 worker 行落值**:集成测试(MockProvider + researcher dispatch)断言:
  run 的每个真实 LLM turn 有 (父 sid, run UUID, seq 递增) 行,token_usage_json
  非空、tools_token 非空、context_window 落值;memory/images/at_files 列 NULL。
- **AC3 主行隔离**:同测试断言 worker 轮不产生 run_id='' 的行;父 session
  `list_turn_traces` 返回不含 worker 行;worker 不写 `sessions.last_*`(既有
  行为回归锁)。
- **AC4 旁路归位**:worker 路径 compaction / loop_hint upsert 写 (sid, run,
  seq) 行(db 单测锁 upsert 路由 + 集成测试覆盖至少 compaction 或 loop_hint
  之一,若构造成本过高则以代码门 + db 单测为准并在 review 记录);主路径两写点
  行为不变(既有测试绿)。
- **AC5 读路径 + 前端**:`list_worker_turn_traces(run_id)` IPC 全链可用
  (Tauri + daemon HTTP);SubagentDrawer 展示 per-run 明细(空态/加载态齐);
  `list_turn_traces` 前端既有测试零改动通过。
- **AC6 回归**:后端全量测试绿(基线以 start 时 main 实测为准,已知既有 flaky
  除外);前端 vitest 全绿 + vue-tsc 0;clippy/fmt 净。
