# L3a: subagent 并发（只读 worker fan-out）

## Goal

把当前同步串行的单 worker dispatch（`chat_loop.rs:1703` 串行拦截 + `run_subagent` 阻塞）升级为**父 turn 阻塞 + 内部 fan-out**：父 LLM 一次发 N 个 `dispatch_subagent`，N 个 worker 并发跑、父 turn 等全部完成再进 turn+1。对标 Hermes 默认前台 `delegate_task`、CC `Agent` tool。

**为什么**：父 LLM 想"并行调研多个方向"时当前只能串行等 N 轮；串行 + 单 worker 是 self-acked 非终态（`chat_loop.rs:1697` "parallel fan-out is v2 / L3"）。

## What I already know

### Research（已就绪，见 Research References）
- 4 项目对照（CC Agent / OpenHands / LangGraph / Hermes）：通讯模式、并发模型、生命周期、上下文隔离、失败可见性、成本归集
- CC 5 层并行全谱 + 实时通信本质矛盾论证（父 LLM 看中间流 = 破坏 context 隔离 = subagent 失去意义）
- **Hermes 源码核实裁定**：默认同步阻塞（`background` 默认 False）、并发默认 3（`_DEFAULT_MAX_CONCURRENT_CHILDREN`）硬拒超限、`max_spawn_depth` 默认 1（禁嵌套）

### 已定范围决策（不推翻）
1. **只读 worker 并发**：researcher/探索类；worktree 隔离留 L3b（带写 worker 才需要）
2. **父 turn 阻塞 + 内部 fan-out**：不是"父 agent 不阻塞"（那是 daemon 化，L3b+）
3. **不做实时中间流**：保持 tool_result 回传，中间进度走现有 `subagent:event` IPC（前端可见、父 LLM 不可见）

### 现状代码锚点
- `chat_loop.rs:1703-1784`：dispatch_subagent 串行拦截点（走串行路径调 `run_subagent`）
- `run_subagent` @ `dispatch.rs:85`：单 worker 执行（阻塞 `.await`，所有参数共享引用 → 天然可并发 N 次，函数体无需改）
- `is_parallel_eligible` @ `chat_loop.rs:2117`：`NAME_ELIGIBLE = [read_file, grep, glob, list_dir, use_skill]`（只读名单，dispatch_subagent 不在 → 不能简单加入；L2 并行路径是为只读 batch 设计）
- **L2 并行模板** @ `chat_loop.rs:1439-1639`：`FuturesUnordered` + `result_slots[i]` 按 index 写回 + `Arc<AtomicBool>` 共享 cancel + 流式 `emit_tool_result`——L3a 照搬
- `filter_tools_for_subagent` @ `subagent/mod.rs:364` + `STRUCTURALLY_DISABLED`（mod.rs:348）：tool 剥离切入点
- `worker_rid = {parent_rid}-sub-{tool_use_id}`（dispatch.rs:171）+ `parent_token.child_token()`（dispatch.rs:172）+ `cancellations` map 注册
- `add_token_usage`（sessions.rs:374）/ `add_token_usage_streaming`（subagent_runs.rs:625）：`col = COALESCE(col,0) + ?` 原子增量
- worker `is_worker=true` → Tier 4 ask 塌缩 `Deny`（dispatch.rs:325）

## Requirements

1. 父 LLM 一次发 N（≤ 上限）个 `dispatch_subagent`（**纯 dispatch 批**）→ N 个 worker 并发执行（`FuturesUnordered`，复用 L2 模板）→ 全部完成后 N 个 tool_result 按序回填 → turn+1
2. 并发 worker **运行时强制剥写**：并发分支对每个 worker toolset 过滤只保留只读工具 `[read_file, grep, glob, list_dir]`，任何 subagent 类型（含 general-purpose）进并发都被剥成只读。researcher 是 no-op，general-purpose 自动降级只读。安全底线仍由 `is_worker` Deny 兜底
3. 并发上限：env `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 **3**，超限**硬拒**（返回 tool_error，不截断不排队）——对齐 Hermes
4. 取消传播：父 cancel → 经 `parent_token.child_token()` fan-out 取消所有并发 worker（**现成能力**）
5. 并发下 token usage / 审计 / `subagent_runs` 持久化正确（`col=col+?` 原子增量 → 不丢；worker 各自 rid/row → 不冲突）
6. 前端 `SubagentDrawer` 支持 N 个并发 worker 的并发展示（store 改支持 N concurrent running run）

## Acceptance Criteria

- [ ] 父 LLM 发 3 个 dispatch_subagent（纯批），3 个 worker 真并发（wall-clock ≈ max(单) 而非 sum）
- [ ] 并发 worker toolset 只含只读工具（general-purpose 进并发也被剥掉 write_file/edit_file/shell/web_fetch）
- [ ] 发 4 个超上限 → 返回 tool_error（不截断不排队），4 个全不执行
- [ ] 父 cancel → 所有并发 worker 进入 cancelled 路径（per-worker status 反映）
- [ ] 并发下 `sessions.*_total` token 计数无丢失（原子增量）、`subagent_runs` 各 worker row 独立持久化、审计无 corrupt
- [ ] N 个 worker 部分成功部分失败 → 各自独立 tool_result + 各自 status（per-worker，天然支持）
- [ ] 前端能同时展示 N 个 worker 的运行状态/进度
- [ ] 混批（dispatch + read_file）仍正确（走原 serial path，不崩）

## Definition of Done

- Rust 单测 + 集成测试覆盖：并发执行、运行时剥写、超限硬拒、取消 fan-out、并发持久化（token/audit/runs）、混批不崩
- `PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig cargo test --lib` 绿
- 前端 vitest 覆盖 N 并发 run 展示
- ROADMAP / ARCHITECTURE / IMPLEMENTATION 更新（L3a done，L3b 仍 pending）
- spec（tool-contract.md）记录并发 dispatch contract（纯批并发 + 剥写 + 硬拒）

## Technical Approach

**核心改动**：在 serial path（`chat_loop.rs:1640` 的 else 分支）入口加一个"纯 dispatch 批并发"前置判断：
- 统计本批 `dispatch_subagent` 个数 `d`，非 dispatch tool 个数 `o`
- 若 `d >= 2 && o == 0 && d <= 上限` → 走**新并发分支**：`FuturesUnordered` 把 N 个 `run_subagent` 包进 task（每个 task 内先调只读剥写过滤再跑），`result_slots[i]` 按序写回，`Arc<AtomicBool>` 共享 cancel，流式 emit。复用 L2 路径（1439-1639）的结构
- 若 `d > 上限` → 整批返回 tool_error（硬拒，全不执行）
- 其余（`d <= 1` 或 `o > 0` 混批）→ 走原 serial path 不变

**只读剥写过滤**：新增一个 `filter_tools_readonly(Vec<ToolDef>) -> Vec<ToolDef>`，只保留 `[read_file, grep, glob, list_dir]`。在并发分支对每个 worker 的 `worker_tool_defs` 调用（在现有 `filter_tools_for_subagent` 之后）。复用 `STRUCTURALLY_DISABLED` 同款模式。

**`run_subagent` 无需改**：它已是可并发的共享引用 async fn。并发分支直接调它 N 次。

**取消/权限/持久化**：全部复用现有（竞态已消解，见 Technical Notes）。

**前端**：`subagentRuns.ts` store 支持 N 个 concurrent running run（现有 single-run 逻辑扩展为 map/list）；`<SubagentDrawer>` 渲染 N 个并发 run（标签/列表式）。

## Decision (ADR-lite)

**Context**: L3a 要把单 worker 串行 dispatch 升级为并发 fan-out。research 阶段担心 3 个竞态点需额外并发控制。

**Decision**:
1. 并发模型 = **父 turn 阻塞 + 内部 fan-out**（`FuturesUnordered`），复用 L2 只读 batch 并行模板
2. auto-context 查证：**3 个竞态点在只读范围下全部被现有架构消解**（`is_worker` Deny + 原子增量 SQL + `child_token` fan-out），无需额外并发控制代码
3. 上限 **3 硬拒**（env 可配，对齐 Hermes）
4. 只读保证：**运行时强制剥写**（保留 `[read_file,grep,glob,list_dir]`），安全底线仍由 `is_worker` Deny 兜底
5. MVP 只优化**纯 dispatch 批**（≥2 全 dispatch），混批走原 serial

**Consequences**: 实现范围大幅收窄——核心改动是 serial path 加一个并发子分支 + 一个只读过滤函数；`run_subagent` 函数体不动；并发安全靠现有设计兜底，风险低。带写 worker 并发仍需 worktree（L3b）。

## Out of Scope (explicit)

- **worktree 隔离**（L3b，带写 worker 才需要）
- **后台不阻塞父 turn**（daemon 化范畴，L3b+）
- **实时中间流注入父 LLM 上下文**（破坏 context 隔离核心价值）
- **混批优化**（dispatch + read_file 同批并发）——MVP 走原 serial，后续可优化
- **结构化 wire 升级**（`[status:X]\n<text>` → typed JSON，独立项）
- **多层 delegation**（嵌套 depth >1，L3b+）
- **resume/checkpoint**（OpenHands `resume=task_id` 范式，远期）
- **per-worker timeout 三字段**（run/idle/heartbeat，LangGraph 范式，远期）

## Research References

- [`docs/research/subagent-communication-survey.md`](../../../docs/research/subagent-communication-survey.md) — 4 项目通讯/并发/失败可见性/成本归集对照 + Phase 1-5 建议
- [`docs/research/subagent-scheduling-communication-survey.md`](../../../docs/research/subagent-scheduling-communication-survey.md) — CC 5 层并行全谱 + 实时通信本质矛盾（§3 已修正 Hermes 错误）
- Hermes 源码核实裁定见 scheduling-survey §3 ERRATA（默认同步/并发 3/硬拒/depth 1）

## Technical Notes

### 3 个竞态点查证结论（auto-context 后）—— 全部消解
| 竞态点 | 查证 | 结论 |
|---|---|---|
| permission:ask 并发 | worker `is_worker=true` → Tier 4 ask 塌缩 `Deny`；只读工具走低 Tier | **不存在**（只读范围下） |
| token usage 并发归集 | `add_token_usage`/`add_token_usage_streaming` 都是 `col = COALESCE(col,0) + ?` 原子增量 SQL，SQLite 单写锁串行化 | **不丢更新** |
| cancellations fan-out | `parent_token.child_token()` × N + 各 `worker_rid`（`{parent_rid}-sub-{tool_use_id}`）注册进 map | **现成能力**，父 cancel 一次触发全部 |

**关键发现**：三个竞态点在"只读 worker 并发"范围下全部被现有架构消解，L3a 的并发安全性是"免费的"。

### 只读保证三层
1. **SubagentDef allowlist**（researcher 纯只读，mod.rs:205）
2. **运行时强制剥写**（L3a 新增，并发分支 `filter_tools_readonly` 保留 4 只读工具）
3. **is_worker=true 权限层 Deny**（安全底线，已存在）

### 上限
env `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 3，硬拒超限（对齐 Hermes）。

## Implementation Plan (small PRs)

- **PR1 — 后端并发核心**：serial path 加"纯 dispatch 批并发"前置分支（`FuturesUnordered` 复用 L2 模板 + `result_slots[i]` + `Arc<AtomicBool>`）+ `filter_tools_readonly` 只读剥写函数 + env `DELEGATION_MAX_CONCURRENT_CHILDREN` 上限硬拒。单测：纯批并发、剥写（general-purpose 进并发被剥）、超限硬拒、cancel fan-out、并发 token 不丢、混批走原 serial。
- **PR2 — 前端 N 并发展示**：`subagentRuns.ts` store 扩展支持 N concurrent running run + `<SubagentDrawer>` 渲染 N 个并发 run（标签/列表）。vitest。
- **PR3 — 集成测试 + 文档**：端到端并发 dispatch 集成测试 + tool-contract.md spec + ROADMAP/ARCHITECTURE/IMPLEMENTATION 更新（L3a done）。

## Open Questions

（全部收敛）
- Q1 只读策略 → **运行时强制剥写**（已定）
- 混批 → MVP 只优化纯 dispatch 批，混批走原 serial（已定）
- 上限配置 → env `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 3（已定）
