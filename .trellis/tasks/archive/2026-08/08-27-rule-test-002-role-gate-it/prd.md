# PRD — RULE-TEST-002 补 workflow 角色门多轮 loop 集成测试

> 台账来源:`.trellis/reviews/DEBT.md` P3 `RULE-TEST-002`(2026-08-27 RULE-ARGS-001 trellis-check 复核发现,migration log §复核记录 F-1/O-1)。

## 背景

W1 workflow 角色门(`check_workflow_role_gate`)在**多轮 agent loop 中 task 状态变更后门判定刷新**这条链路上没有任何集成断言。2026-08-27 RULE-ARGS-001 迁移期间出现过一处真实的活引用↔入口快照漂移(`DispatchCtx::workflow_ctx` 若误接 `request.workflow_ctx` 入口快照,轮顶刷新将永远不可见),全量测试一套未抓、纯靠人工 diff 审计拦下——正是该测试面缺口的存在性证明。本任务只补测试,不改任何生产代码行为。

## 需求

- **R1**(核心场景):同一 `run_chat_loop` 内的多轮集成用例——mock LLM 第 1 轮发起与当前 task 状态不匹配角色的 `dispatch_subagent` 工具调用 → 断言收到 "Role gate denied" 类 denial;两轮之间把 worktree 下 `.everlasting/tasks/` 的 task.json status 改为允许该角色的状态;第 2 轮发起同一角色调用 → 断言门按盘上最新状态放行(dispatch 不再被拒)。
- **R2**:用例置于 `app/src-tauri/src/agent/tests_agent_loop/`(建议新文件 `role_gate_refresh.rs`,或在既有最贴切文件内加,由实现判断),复用 `tests_common` 的 TestHarness / MockProvider 多轮基建,不引入新框架、不连真实 LLM。
- **R3**(防回归有效性):该测试必须能抓住本次事故的重现路径。implement 阶段做两个变异点的手工验证并记录结果:(a) 把门的输入改回入口快照;(b) 移除 `drive_turn` 轮顶的 `resolve_current_task` 刷新。每个变异点测试须转红,验证后复原生产代码(工作树最终不得残留生产代码改动)。
- **R4**:遵守 `.trellis/spec/backend/test-model-contract.md` 与邻座测试文件的命名/断言风格;WSL 下跑法见 AGENTS.md(PKG_CONFIG_PATH 导出)。

## 验收标准

- [ ] **AC1**:`cargo test -p everlasting --lib` 全绿(含新用例);`cargo clippy --lib -- -D warnings` 与 `cargo fmt --check` 干净。
- [ ] **AC2**:新用例同时覆盖"第一轮 denial 文本断言 + 第二轮放行的可观测结果断言"两侧。
- [ ] **AC3**:变异验证结果(R3 两个点各:转红 → 复原 → 转绿)记录进任务目录(如 `implement.md` 或研究笔记附录)。
- [ ] **AC4**:零生产代码 diff 提交(仅新增/调整测试文件;DEBT.md 销账属文档单独提交批,不在本 AC 内)。

## 非目标

- 不改 `check_workflow_role_gate` 或 dispatch 链路的生产逻辑。
- `force=true` 旁路语义不强制覆盖(若顺手可作为第二用例,非验收项)。
- 不涉及 frontend。

## 用户已确认事项

2026-08-27 会话中用户明确指示"做 RULE-TEST-002"(任务创建 + 进入实施一并授权;lightweight 任务,PRD-only)。
