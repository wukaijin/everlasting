# Review — A 类单体重构:subagent dispatch 拆分

> 评审日期:2026-08-08。评审对象:`prd.md` / `design.md` / `implement.md`(status=planning,实施前评审)。
> 方法:对 PRD 关键事实(`dispatch.rs` 行数、`run_subagent` 起止行 + 参数个数 + 返回 tuple、3 处调用点、`tests_dispatch.rs` 引用、stage 边界行号、merge_worker hub 模式)逐条 `wc -l` / `grep` / `awk` 核验;对 design 的阶段输出 struct 与函数签名做依赖核对;对 implement 的文档 sweep 范围与 grep 排除规则做实测复盘。

## 总体评价

三件套质量高——**事实基础扎实、阶段切分清晰、风险识别到位、行为零变化锚点(1657 测试基线)明确**。逐条核验后,行数/参数/调用点/测试引用等硬事实**全部属实**;7 阶段函数化 + hub re-export 方案与 `merge_worker` 现有惯例一致。

**结论:可批准进入实施**,但建议先处理 4 个 P1 项(1 处行为漂移高风险 + 1 处范围蔓延风险 + 2 处 doc sweep 漏点),其余 P2/P3 顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| `dispatch.rs` = 1472 行 | **精确**(`wc -l` = 1472) |
| `run_subagent` L100–1396,~1297 行 | **精确**:L100 `pub(crate) async fn run_subagent`;L1396 `}` |
| 25 参数签名 | **精确 25 个**:`provider / catalog / context_window / parent_rid / parent_session_id / memory_cache / read_guard / skill_cache / permission_asks / cancellations / _session_active_request / background_shells / db / current_ctx / tool_use_id / input / parent_token / _parent_sink / worker_event_sink / force_readonly / subagent_cache / app_data_dir / parallel / parent_question_store / workflow_ctx` |
| 返回 `(String, bool, bool, Option<i32>)` | **精确**:L227 `-> (String, bool, bool, Option<i32>)` |
| `check_workflow_role_gate` L1412–1472(61 行) | **精确**:L1411 注释块起;L1412 `pub(crate) fn check_workflow_role_gate`;文件 1472 收尾 |
| 3 处调用方:`chat_loop.rs:1313/3603/4151` 全路径 | **精确**:`crate::agent::subagent::dispatch::run_subagent` 三处,行号无偏移 |
| `tests_dispatch.rs` L15 `use super::dispatch::*` | **精确** |
| `check_workflow_role_gate` 11 处测试引用 | **精确**(`grep -c` = 11) |
| `merge_worker.rs` hub 模式 + `merge_worker/{execute,merge,finalize}.rs` | **精确**:`merge_worker.rs` + 三子文件;`pub use` 全量 re-export 与 design §3 写法一致 |
| 阶段 A 解析+校验 L219–319 | **精确**:L227 解析起点,L319 附近 `check_workflow_role_gate` 早返 |
| 阶段 C worktree+guard+toolset L411–527 | **精确**(连续段) |
| 阶段 D messages L537–591 | **精确**(连续段) |
| 阶段 E run 注册+sink L720–849 | **精确**(连续段) |
| 阶段 F drive L878–1045 | **精确**:`Box::pin(run_chat_loop(...))` 24 位置参数调用点在此区段 |
| `LoadedSubagent` 出现在 L172 / L512 | **精确**;design §5 已识别"字段化需 owned / Arc" |
| 历史拆分:批 1 `prep.rs` + 批 3 `mod.rs` 内联 cluster | **与 commit 记录一致**:`198ff50` 批 3、`dfcb9ba` 批 2;批 1 早于记录窗口但与 `prep.rs` 文件存在一致 |
| `tests_dispatch.rs` 用 `use super::dispatch::*` 经 hub re-export 解析 | **可行**:glob 在子模块重新 `pub use` 后仍能命中(merge_worker 先例) |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 阶段 B 跨不连续两段(L343–402 + L619–697),design 风险描述不足

**位置**:`prd.md` Background(L12 段 B 标 `L343–402, 619–697`)+ `design.md` §1 流程图 + `design.md` §2 `plan_worker` 签名 + `implement.md` Phase A 第 2 步。

**问题**:PRD 自身诚实地把阶段 B 标成不连续两段(其余 6 个阶段都是连续段),但 design §1 / §2 把 `plan_worker` 当作单段处理。提取时这是**整个计划里最易出行为漂移的提取点**:

- L343–402:`dispatch_isolation` 解析 + `dispatch_model` 候选解析
- L403–618:阶段 C/D 全部(worktree 创建、guard 重置、toolset 过滤、messages 构建)
- L619–697:`resolve_final_model` + `resolve_worker_provider` + `worker_display` backfill

`plan_worker` 提取 = 把 L343–402 与 L619–697 物理合并进一个函数,中间必须穿越 ~200 行 C/D 代码并保持其顺序与副作用。一旦手滑把 L619–697 中某段对 `def` / `project_id` / `worker_run_id` 的就地变更错位,isolation + model + display 三处决策都可能在不同阶段看到不同状态。

**建议**:
- `design.md` §2 `plan_worker` 描述加显式备注:"跨不连续 L343–402 + L619–697,提取 = 合并两段 + 校验中间 C/D 对 `def.model` / `project_id` 无副作用调用"
- `implement.md` Phase A 第 2 步验证升级为 `cargo test --lib`(不只 `cargo check`)
- 提取前先在 L343 / L619 两处分别 `git blame` 确认 PR1/2 提交,核对哪些变量在 L403–618 区段被改写

### 🔴 P1-2 — R4 测试组织表述含糊,易导致范围蔓延

**位置**:`prd.md` R4。

**问题**:R4 写"子模块 `#[cfg(test)]` 测试随代码迁移到同级 `tests_*.rs` 文件(文件级 `#![cfg(test)]` 门控)"。但**当前 `dispatch.rs` 没有 inline `#[cfg(test)] mod tests`**,所有测试在同级 `tests_dispatch.rs`(1078 行,文件级 `#[cfg(test)]` 门控)。R4 字面读法有两种:

- (a) 仅指"未来新增 per-stage 单元测试随代码就近放同文件 `#[cfg(test)] mod tests`"——design §6 倾向此
- (b) 暗示"把 `tests_dispatch.rs` 拆成 `dispatch/tests_*.rs` 多个文件"——范围显著膨胀

按 (b) 误读执行,`tests_dispatch.rs` 1078 行会被切到 `dispatch/tests_parse.rs` / `dispatch/tests_plan.rs` / ...,带来 ~10 个 test file 跨子目录的可见性 / `use super::*` 解析问题。

**建议**:R4 改写为"新提取阶段的单元测试(若加)随代码就近放同文件 `#[cfg(test)] mod tests`;`tests_dispatch.rs` 保持单文件,通过 hub re-export 解析符号,不拆分。"

### 🟡 P2-1 — `register_run` 函数签名里的 `parsed` 实际只消费两字段

**位置**:`design.md` §2 阶段函数签名。

**问题**:`register_run(cancellations, db, parent_rid, parsed, plan, prep, parent_token, worker_event_sink, parent_session_id)`,但实际:
- `parsed` 仅用 `tool_use_id_owned`(`worker_rid = format!("{}-sub-{}", parent_rid, tool_use_id)`)和 `project_id`(`insert_run_with_id(... project_id ...)`)
- `plan` 在 register 阶段**未消费任何字段**(isolation / final_model / provider / ctx / display 都在后续阶段才用)

`register_run` 接受整个 `parsed` + `plan` 是不必要的耦合。

**建议**:签名收窄为 `register_run(cancellations, db, parent_rid, tool_use_id: &str, project_id: &str, prep: &WorkerPrep, parent_token, worker_event_sink, parent_session_id)`,去掉 `parsed` 与 `plan` 形参;或显式注释"仅消费 prep.worker_run_id / worker_branch / worker_worktree_opt / project_main_override,以及 tool_use_id/project_id 用于 rid 构造"。

### 🟡 P2-2 — `EarlyReturn` 不变量未在 design 显式固化

**位置**:`design.md` §2 `EarlyReturn` 定义。

**问题**:`type EarlyReturn = (String, bool, bool, Option<i32>);` 复用 `run_subagent` 返回 tuple。已实测现有早期返回(`check_workflow_role_gate` 拒绝、空 task、worktree 创建失败 等)全部 `is_error: true`,但 design 没写这条不变量。`parse_dispatch` / `prepare_worker` 实现时若手滑构造"友好早返"(`is_error: false`),调用方的 `chat_loop.rs` 三处对 `is_error` 的判断会改变行为路径,直接违反 R3 行为零变化。

**建议**:design §2 `EarlyReturn` 段加一句不变量:

> `EarlyReturn` 三态必须满足 `is_error == true`(现有 L319 角色门控、L255 空 task、L478 worktree 创建失败等所有早返点均为此值;`cancel_parent` 与 `exit_code` 沿用正常路径的初值)。

并在 AC 加一条:`parse_dispatch` / `prepare_worker` 的早返路径 `is_error` 必为 `true`。

### 🟡 P2-3 — implement.md 文档 sweep 漏点 + grep 排除规则有歧义

**位置**:`implement.md` Phase C 第 8 步。

**问题 A — 漏点**:已实测仓内仍有 `dispatch.rs:LINE` 引用但未列入 sweep 清单:

- `.trellis/spec/backend/worktree-contract.md:763`:`See dispatch.rs::tests::probe_worker_changes_*` —— 这条**本身就是错误引用**(`probe_worker_changes_*` 测试实际在 `tests_dispatch.rs` 160-220 行,不在 `dispatch.rs::tests`),sweep 时应改为符号引用指向 `tests_dispatch.rs::probe_worker_changes_*`
- `.trellis/spec/backend/agent-loop-architecture/pattern-concurrent-dispatch.md:66`:`dispatch.rs::run_subagent`
- `.trellis/spec/backend/tool-contract/10-c2-loop-intervention.md:43`:`dispatch.rs::run_subagent`
- `docs/WORKFLOW-INTEGRATION-REVIEW.md:149`:`dispatch.rs:335`
- `docs/REMOTE-ACCESS-ROADMAP.md:116`:`agent/subagent/dispatch.rs:1192`
- `docs/REMOTE-ACCESS-RESEARCH.md:107,120,661`:`agent/subagent/dispatch.rs:1192` / `dispatch.rs:251` / `sink.rs:279,698` + `dispatch.rs:1192`
- `docs/research/subagent-scheduling-communication-survey.md:32,55`:`dispatch.rs:85` / `dispatch.rs:286`
- `.trellis/spec/frontend/chat.md:339`:`dispatch.rs::run_subagent`
- `app/src-tauri/src/llm/provider/mock.rs:123`、`agent/permissions/mod.rs:161`、`tools/mod.rs:131`、`agent/subagent/mod.rs:112`、`sink.rs:474`、`tests_subagent.rs:359,1567,3459,3467`、`tests_common.rs:346`、`loader.rs:49`、`chat_loop.rs:1900,2168`:共 ~10 处源码注释行号

**问题 B — grep 排除**:`grep -rn "dispatch\.rs:[0-9]" .trellis/spec/ docs/`(archive/ 除外)字面执行,`archive/` 实际在 `.trellis/tasks/archive/`,**不在**该 grep 扫描范围;但 `docs/_reviews/` 与 `docs/IMPLEMENTATION/decisions-*.md` 明确"不动",当前 grep 会误命中。

**建议**:
- Phase C 清单显式加入上述文件
- grep 改为:
  ```bash
  grep -rn "dispatch\.rs:[0-9]" .trellis/spec/ docs/ \
    | grep -vE "/_reviews/|/decisions-20|/archive/"
  ```
- sweep 后另跑 `grep -rn "dispatch\.rs:[0-9]" app/src-tauri/src/`,处理源码注释中的行号引用

### 🟢 P3-1 — AGENTS.md 测试基线数与 PRD 轻微不一致

**位置**:`prd.md` R7 / AC4 vs `AGENTS.md` "Tests" 段。

**问题**:AGENTS.md 写 `~1635 unit tests`,PRD 写 `1657 基线`。1657 反映批 2/批 3 拆分后现状,AGENTS.md 已陈旧。

**建议**:在 Phase D 收尾时同步 `AGENTS.md` 把 `~1635` 改为 `~1657`;或 R7 加注"基线数取 `cargo test --lib 2>&1 | grep 'test result'` 实时值,文中数字为最近一次执行快照"。

### 🟢 P3-2 — `collect_outcome` 副作用顺序未在 AC 体现

**位置**:`design.md` §2 `collect_outcome` 描述 + AC1–AC6。

**问题**:design 已标注 `collect_outcome`(含 persist 副作用),即函数内执行 `update_run_finished` → `emit_subagent_finished`。R3 写"DB 写入顺序、事件 emit 顺序均不变",但 AC 没断言这条顺序。

**建议**:AC 加一条:
> AC4.x:`update_run_finished` 必须在 `emit_subagent_finished` 之前调用;`collect_outcome` 内部按原代码顺序:`status picker → transcript/messages 截断 → update_run_finished → emit_subagent_finished`。

或在 design §2 `collect_outcome` 签名注释里固化"内部按原代码顺序"。

### 🟢 P3-3 — `drive_worker` 14 参数的债务未在 AC 留接口

**位置**:`design.md` §2 "已知债务" 段。

**问题**:design §2 已诚实标注 `drive_worker` ≈ 14 参数为已知债务,但 AC1 只看 `run_subagent` 主体行数,`drive_worker` 自身无可读性指标约束。

**建议**:design §2 末加一句:"本任务不为 `drive_worker` 进一步结构化(用户已确认阶段输出 struct 方案);若后续 A 类任务后该函数仍 > 250 行,另立任务"。

### 🟢 P3-4 — D2 验收行数"≤250 行"建议加测量口径

**位置**:`prd.md` AC1 + 已决决策 D2。

**问题**:AC1 写"run_subagent 主体 ≤ ~250 行",tilde 本身合理,但 tilde 含义会因测量口径而变化(是否含 `use` 语句 / 阶段注释 marker 块 / 空白行)。

**建议**:AC1 改写为"≤ 250 行(不含 use 语句、不含 `// ===` 阶段注释 marker 块、含函数体),通过 `awk '/^pub.*fn run_subagent/,/^}$/' dispatch.rs | wc -l` 测量"。

## 🟦 其他备注(可不动)

- `_session_active_request` 与 `_parent_sink` 仍列在 25 参数里(签名冻结,合理);"明确不做"段已隐式覆盖。
- `merge_worker.rs` hub 现状(已 `pub use` 全量 re-export):写法与 `design.md` §3 一致,✅。
- 提取阶段不引入 `cargo nextest`(AGENTS.md 提到作为 profiler 选项);当前 `cargo test --lib`(多线程,默认)OK。
- D3 执行节奏 + implement.md Ordered Checklist 互相印证,Phase A → B → C → D 顺序合理;每 commit 独立可回滚,AC3 满足。
- 行号引用 sweep 与提取 commit 的耦合:6 个提取 commit 期间 `dispatch.rs` 行号会全变,doc 引用行号全失真。属可接受中间态,Phase C 一次性清。

## 复评建议

修订完 P1-1 / P1-2 / P2-1 / P2-2 / P2-3 后可 `task.py start`。P3-1 ~ P3-4 可在实施前顺手合并修订(单 commit doc 修),不必单独立项。
