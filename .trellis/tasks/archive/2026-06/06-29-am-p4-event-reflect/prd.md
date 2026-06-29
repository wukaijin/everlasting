# P4: 事件驱动自动写入 — 连续失败→成功 旁路 reflection

> child `06-29-am-p4-event-reflect` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §3 路径2 + §6 接入点 C](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 前置:P1(消费 `insert_memory`)。**自动闭环**:失败经验自动变 pitfall,不再依赖 agent 手动 remember。

## Goal

检测"连续 ≥2 次同名工具失败后成功"信号 → 触发**旁路 LLM reflection**(专门 prompt)→ 提炼一条带 `trigger_key` 的 pitfall → `insert_memory(status=active)`(事件天然高置信,直接 active,不经 candidate)。

配合 P3 的召回构成完整自动闭环(踩坑→记住→下次规避)。

## In Scope

- 接入点 C:`chat_loop.rs:1717 emit_tool_result` 旁路挂 in-session 状态机:记录 `(tool_name → 连续失败计数)`,连续失败 ≥2 后这次成功 → 触发 reflection
- 旁路 reflection:fire-and-forget 异步,不阻塞主 loop;调 provider 用专门 prompt(从失败+成功的 transcript 片段提炼:一句坑描述 + trigger_key + tags)
- 写入:`insert_memory(kind=pitfall, status=active, trigger_key=..., source_ref=tool_call_id)`,过 P1 安全网

## Acceptance Criteria

- [ ] 制造连续 2 次 shell 失败后成功 → 自动产出一条 pitfall(active,带 trigger_key)
- [ ] reflection 异步,不阻塞主 loop(主 loop 时延无明显增加)
- [ ] 单次失败不触发(需连续 ≥2)
- [ ] 产出的 pitfall 能被 P3 的工具执行前召回命中
- [ ] cargo test 全绿

## Technical Approach(方向,实现细节实施时定)

- 旁路 reflection 的 provider/cancel 生命周期:fire-and-forget 失败如何处理(丢掉 vs 重试)、与主 loop cancel 的关系——实施时定
- reflection prompt 模板:怎么从 transcript 片段提炼结构化 trigger_key(LLM 产出 JSON)——实施时定
- 状态机的 session 维度存储:内存 map 还是 DB——实施时定

## Out of Scope

- session 结束整体 reflection(v2)
- verified 软拦截(→ P5)
- 状态机晋升(→ P5;P4 写入直接 active)
- "用户纠正"等其他事件信号(留扩展,本 task 只做连续失败→成功)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3 路径2 + §6 接入点 C

---

## 代码现状(2026-06-29 落地后)

- `app/src-tauri/src/agent/auto_reflect.rs` — 新模块,含 `FailureTracker`(per-session 状态机)+ `try_record_outcome()`(fire-and-forget 公共入口)+ `reflect_to_pitfall()`(私有 LLM 提炼+insert 路径)+ 13 个单测
- `app/src-tauri/src/agent/mod.rs` — `pub mod auto_reflect;` 注册
- `app/src-tauri/src/agent/chat_loop.rs` — `run_chat_loop` 顶部构造 `failure_tracker: Arc<Mutex<FailureTracker::new()>>`(per-session 内存,跨 turn 共享);两处挂载:
  - parallel-batch L2 path(`FuturesUnordered` task 内,execute_tool 之后、audit 写之前)
  - serial path(`DispatchBatch::Serial` for-loop 内,execute_tool 之后、audit 写之前)
  - 都在 `!token.is_cancelled()` gate 后调用,与 audit-skip 对齐语义
- 1041 cargo test 全绿(基线 1028 + P4 新增 13)
- `cargo check --tests` 0 warning
- frontend 不动(P4 纯后端)

## 已定决策(P4 实施时锁档)

1. **挂在 `run_chat_loop` 局部 + Arc 共享** — 状态机是 per-session 内存,跨 turn 共享,但不持久化跨 session(spike-007 §10 扩展位);`Arc<Mutex<...>>` 让 parallel-batch L2 task + serial path 共享同一实例
2. **挂 emit 后而非 `emit_tool_result` 内** — 跟 P3 一致,挂在 `execute_tool` 完成后、envelope wrap 之前;这样 `content`(P3 注脚 prepend 后)+ `is_error` 是已知的最终值,避免 recall / reflection 时序竞争
3. **fire-and-forget 整段 reflection** — `tokio::spawn` 包裹 LLM 调 + JSON parse + `insert_memory`;主 loop 拿到 `try_record_outcome` 返回后立即继续。失败一律 `tracing::warn!` + 静默吞,不 panic / 不 retry / 不污染主 loop(PRD hard rule + 吸收 spike-007 §10 经验)
4. **复用 P1 `insert_memory` 安全网** — P4 不另写敏感过滤 / 长度 / 敏感路径检查,统一走 P1 兜底(单一权威);`source_ref` 设为 `<request_id>:<tool_name>` 供未来 hygiene job 溯源
5. **直接写 `status=active`(不经过 candidate)** — spike-007 §3 路径2 定档(事件驱动天然高置信,免一次晋升);`scope=Project` 强制(旁路 reflection 总有 project context)
6. **失败阈值 = 2** — `REFLECTION_FAILURE_THRESHOLD = 2`(单次失败不算,需连续 ≥2,PRD AC #3);counter 在成功或触发后清零,避免双触发
7. **独立 reflection prompt** — `REFLECT_SYSTEM_PROMPT` + `REFLECT_USER_TEMPLATE` 是 P4 私有常量,不污染 `DEFAULT_BEHAVIOR_PROMPT`;系统 prompt 极简(输出 JSON only)+ 用户 prompt 包含失败/成功 transcript 片段(2 KiB head cap);LLM 产出 `{title, content, trigger_key}` 三字段 JSON
8. **scope=Project only** — `try_record_outcome` 强制传 `project_id`;`User` scope 留 P2 remember tool(人写)路径,P4 旁路(agent 自动写)走 Project 不污染跨项目

## Open Questions(原 prd "实施时定" 项)— 全部已解决

- ~~旁路 reflection 的 provider/cancel 生命周期~~ → **fire-and-forget + 失败 `tracing::warn!` 静默吞**;无 retry,无 cancel token 传递(spawn future 在 session 结束 / 进程退出时随 task 一起 drop);主 loop 永不阻塞
- ~~reflection prompt 模板:怎么从 transcript 片段提炼结构化 trigger_key~~ → **独立 system+user 模板,LLM 产出 `{title, content, trigger_key: {tool, command_pattern, path_globs}}` JSON**;`strip_code_fence` 容忍 LLM 加 markdown 围栏;2 KiB truncate 防 prompt 膨胀
- ~~状态机的 session 维度存储:内存 map 还是 DB~~ → **per-session 内存 `Arc<Mutex<HashMap<tool_name, Entry>>>`**,`run_chat_loop` 局部,跨 turn 共享,跨 session 重置(v1 接受;v2 可加 persistent low-confidence hint)

## Implementation Plan(实际 1 PR)

- **PR1 work**:`feat(memory): P4 事件驱动自动写入 — FailureTracker + 旁路 reflection`(auto_reflect.rs 新建 + mod.rs 注册 + chat_loop.rs 两处挂载,共 +498/-1)
- **PR1 docs**(Phase 3):permission-layer.md §4.2 + memory.md §Scenario 2 P4 + tool-contract.md + agent-loop-architecture.md seam 同步(由主 session 走 Phase 3.3)
- **PR1 落地**:本 section + `task.json` status→completed(由本 commit 收尾)
- archive + journal:由 `task.py archive` 自动生成

## P4 → P5 衔接

- **P5**(质量层)消费 P4 写入的 `status=active` 行:`update_status` 事务化已经在 P1 落地(P5 状态机晋升:`active → verified` 靠 `hit_count` 自动晋升,demoted 靠老化 / 卫生 job)
- **P5** 的 verified 软拦截(decision-tree 在 `permissions::check` 内部)与 P4 的 active 注脚(旁路 seam)正交,两者在 P3 召回 seam 共存

## P4 → P2/P3 衔接

- **P2** 的 `remember` tool 与 P4 的 `auto_reflect` 是**两条**写入路径,共享 P1 `insert_memory` 入口 + P5 `update_status` / `bump_hit_count` 接口
- **P3** 的 `recall_pitfall_footnote` 在工具执行前 seam 召回,**消费** P4 写入的 `active` pitfall 行(spike-007 §4 layer 1 + layer 2 共用 `autonomous_memories` 表,端到端闭环已在 `reflected_pitfall_is_recallable_by_p3_helper` 单测覆盖)
