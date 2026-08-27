# Implement — RULE-TEST-002 workflow 角色门多轮 loop 集成测试

> 完成日期:2026-08-27。生产代码零 diff(AC4);改动仅测试文件 + 本文档。

## 新增/改动文件

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/agent/tests_agent_loop/role_gate_refresh.rs` | 新增集成用例 `role_gate_denies_then_allows_after_mid_loop_task_json_status_change`(单用例,R1 全场景) |
| `app/src-tauri/src/agent/tests_agent_loop/mod.rs` | 注册 `mod role_gate_refresh;`(1 行) |
| `.trellis/tasks/08-27-rule-test-002-role-gate-it/implement.md` | 本文 |

## 用例设计(R1 → 断言映射)

单一 `run_chat_loop`,脚本槽 `[parent_t1, parent_t2, worker_t1, parent_t3]`:

1. **setup**:`make_harness()` 项目 tempdir 预置 `.everlasting/tasks/gate-refresh-task/task.json`(status=planning,TaskJson serde 落盘);session 行开 workflow + plugin_name="dev";入口 ctx 用生产同款 `build_workflow_ctx` 解析(前置断言 entry snapshot == planning)。builtin dev def 状态词汇:planning→[researcher]、in_progress→[implementer, checker]。
2. **第 1 轮(denial 侧)**:mock LLM 同轮先 `write_file` 把盘上 status 翻到 in_progress(真实事故形态「loop 中途改盘」),再 `dispatch_subagent{subagent:"checker"}`。串行工具段保证改盘物理发生在第 2 轮刷新之前——确定性时序、无后台 watcher 竞态。断言:write_file 结果非 error(前提守护)+ dispatch 结果 is_error 且含 `"Role gate denied"` 与 `"planning"`、不含 worker marker(worker 未执行)。
3. **第 2 轮(放行侧)**:同一角色再派 → drive_turn 轮顶 `resolve_current_task` 刷新读盘(in_progress)→ 门放行。断言:dispatch 结果不含 denial 且含 worker 终文 marker(`format_final_text` Completed = 原文透传)、`call_count()==4`(worker 真实执行消费 slot 3;变异退化为 3 时此断言同拦)。

test id:`agent::tests_agent_loop::role_gate_refresh::role_gate_denies_then_allows_after_mid_loop_task_json_status_change`

## 变异验证证据(PRD R3 / AC3)

每个变异点流程:改生产代码 → `cargo test -p everlasting --lib tests_agent_loop::role_gate_refresh` 观测转红 → 复原 → 复跑转绿。

### 变异 A — 门输入误接入口快照(08-27 漂移事故原样重现)

- 变异点:`app/src-tauri/src/agent/chat_loop.rs` `DispatchCtx` 构造处
  `- workflow_ctx: &workflow_ctx,` → `+ workflow_ctx: &request.workflow_ctx, // MUTATION-A`
- **转红**(FAILED,0.73s):

  ```
  round-2 same-role dispatch must be allowed after the refresh, got:
  {"cwd":"/tmp/.tmp3GXKba","result":"Role gate denied: 'checker' is not allowed in state 'planning'
  (allowed: researcher). Either transition to a state that allows this role, or re-dispatch with
  force: true for a one-shot override. Current breadcrumb: see messages[0]."}
  ```

- 复原(单行还原,git diff 归零)后:**ok,1 passed(0.70s)** → 绿。

### 变异 B — 移除 drive_turn 轮顶 resolve_current_task 刷新

- 变异点:`app/src-tauri/src/agent/chat_loop/drive.rs` R4 块(~933)
  整块 `if let Some(ref mut ctx) = workflow_ctx { ctx.current_task = ...resolve_current_task... }` 注释掉(`// MUTATION-B`)。
- **转红**(FAILED,0.63s,报文与变异 A 同形):

  ```
  round-2 same-role dispatch must be allowed after the refresh, got:
  {"cwd":"/tmp/.tmpT3ywVG","result":"Role gate denied: 'checker' is not allowed in state 'planning' …"}
  ```

- 复原(整块还原)后:**ok,1 passed(0.70s)** → 绿。

两变异红因均为「第 2 轮同一调用仍被拒在 planning」——正是本测试要抓的两类回归(活引用漂移 / 刷新缺失),与 PRD R3 的预测一致。

## 与研究笔记的偏差(以代码为准)

1. **状态/角色词汇**:research 草图的 `planning→[spec] / implementation→[dev]` 是示意对;实际按 builtin dev def(def.rs `default_workflow()` / `resources/builtin-workflow/dev/workflow.json`)使用 planning→[researcher]、in_progress→[implementer, checker],派发角色选 checker(只读免 worktree 隔离,plain `make_harness()` 即可,比 implementer 的 isolation:true 轻)。
2. **「两轮之间改盘」的实现方式**:未用 test 侧 spawn 后台任务轮询 `call_count_handle` 直写文件(research 草图步骤 3 字面形态),而由 mock LLM 第 1 轮自己的 `write_file` 工具调用完成。理由:串行工具执行给出严格确定时序(写盘先于第 2 轮刷新),消除 watcher 唤醒竞态的可 flake 面;且「模型中途改盘再派发」就是事故的真实发生路径。task.json 内容经 TaskJson serde 序列化,字段名/schema 从 `workflow/task/types.rs` 实读,未猜。
3. **file:line 锚点**:research 表全部核对成立(chat_loop.rs DispatchCtx 构造 :829、drive.rs R4 刷新块 ~933、parse.rs 门调用 :170、dispatch.rs 门函数 :411、inject.rs resolve_current_task :262)。

## 验证命令结果(AC1)

WSL 下统一 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`:

| 命令 | 结果 |
|---|---|
| `cargo test -p everlasting --lib tests_agent_loop::role_gate_refresh`(窄跑) | **1 passed**,~0.7s |
| `cargo fmt --check` | 干净 |
| `cargo clippy -p everlasting --lib -- -D warnings` | 干净(零 warning) |
| `cargo test -p everlasting --lib`(全量,多线程) | **2008 passed; 0 failed; 1 ignored(pre-existing)**,126s |
| `git status --porcelain`(收尾) | 仅:CI 注释改动(**pre-existing**,RULE-ARGS-001 文档批遗留,本任务未触碰)+ mod.rs(M)+ role_gate_refresh.rs(??)+ 任务目录(??)——生产代码零 diff ✔ |

## 备注

- 非 worker 路径下 drive.rs 的刷新/面包屑块整体居于 `!skip_persist` 分支内;用例走 `parent_role()`(skip_persist=false),符合生产父会话语义。
- S-B 面包屑注入在本用例中跳过(`test_messages()` messages[0] 为 Text 非 Blocks,S-B skip-not-prepend)——刷新逻辑先于且独立于面包屑 append,不影响靶子。

## trellis-check 复核记录(2026-08-27)

四项清单全过、零修复:① spec 合规(三套件具名拼装 + 生产同款 `build_workflow_ctx`,mod 注册字母序、`#![cfg(test)]` 与邻座一致);② PRD 对照(AC2 双侧真实成立——放行侧 marker 经 `format_final_text(Completed)` 原文透传核实、三路独立判据 is_error/marker/call_count);③ 无同义反复(变异 A/B 下 current_task 卡死 Some(planning),三判据同时破坏);④ AC4 零生产 diff 复核。三门重跑:2008 passed/0 failed/1 ignored(pre-existing)+ fmt/clippy 干净。

复核新增一条已知边界(超出 PRD 两强制变异点):若未来 `resolve_current_task` 中途恒返 None,门会静默开放且本用例仍绿——第三类回归,按主会话裁量不扩用例形态,已在 spec `tests-required.md` 新条目中注明为 known boundary。spec 同步已随本任务提交批完成。
