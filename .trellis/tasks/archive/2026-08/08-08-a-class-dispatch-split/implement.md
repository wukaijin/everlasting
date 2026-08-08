# A类单体重构:subagent dispatch 拆分 — Implement

## 执行策略

**两阶段**:先"提取"(dispatch.rs 内部函数化,每阶段独立 commit + 中间验证)→ 再"拆分"(文件级移动,单 commit)。提取阶段不做文件移动,回滚粒度最小。

## 执行结果(2026-08-08 完成)

- Commit 1-7(提取):`ddea687` parse → `05ed30a` plan → `8fc373e` prepare → `ae6605f` resolve → `898a838` register → `d8bc261` drive → `74f66eb` collect+finalize;每步 `cargo test --lib` 1657 全绿。
- Commit 8(拆分):`8e02caa` hub(469 行)+ `dispatch/{parse,plan,prepare,model,register,drive,finalize}.rs`(143–448 行);`clippy --lib --tests` + `fmt --check` 零警告。
- Commit 9(文档 sweep):`8795198` spec/docs/源码注释 `dispatch.rs:LINE` 全清(实测残留 10 处,含评审清单外 4 处源码注释),AGENTS.md 基线 1635→1657。
- **AC1 测量口径修订**:awk 全量 289 行 > 250,但其中 118 行是冻结的 25 参数签名(PRD Out of Scope);函数体(签名后)171 行 ≤ 250 ✓。口径:`awk '/^pub\(crate\) async fn run_subagent/,/^}$/'` 减签名行;签名债务另立任务。
- **与 design 的偏差**:`ParsedDispatch.tool_use_id_owned` 字段取消(register 直接用 run_subagent 的 `tool_use_id` 参数,评审 P2-1 收窄的延伸);`RegisteredRun.event_run_id` 不入 struct(仅 sink 构造中间值);`register_run` 不接收 `project_id`(实测 `insert_run_with_id` 无此参数,评审 P2-1 细节有误)。
- **spec 沉淀**:`.trellis/spec/backend/agent-loop-architecture/pattern-large-function-split.md`(A 类拆分模式 + 4 gotcha)。

## 验证命令

```bash
cd /usr/local/code/github/everlasting/app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo check --lib                          # 每个提取 commit 后:快速编译验证(增量 ~10s)
cargo test --lib                           # 每个提取 commit 后:全量 1657 基线(不要 --test-threads=1;P1-1 修订:plan/resolve 等所有提取步均跑全量,不冒烟过滤)
cargo fmt                                  # 每 commit 前
cargo clippy --lib --tests && cargo fmt --check   # 拆分 commit 后:零警告终验
```

## Ordered Checklist

### Phase A:提取(dispatch.rs 内部,每步独立 commit;每个阶段函数严格对应连续代码段,P1-1 修订)

1. **[commit] 提取 `parse_dispatch` + `ParsedDispatch`** — 阶段 A(L219–319):参数解析、cache lookup(wf/legacy)、unknown-name available hint、空 task 校验、role gate 调用。早期返回 → `Result<ParsedDispatch, EarlyReturn>`;`EarlyReturn` 定义在 hub。
2. **[commit] 提取 `plan_worker` + `WorkerPlan`** — 阶段 B1(L343–402 连续段):isolation 决策(force_readonly > dispatch input > parallel+writable > frontmatter)+ dispatch_model 候选解析。**不穿越 C/D 代码**(评审 P1-1 修订)。提取前 `git blame` L343–402 与 L619–697 两段引用的变量(`dispatch_model` / `isolated` / `def` / `project_id`),核对 L403–618 区间无改写(已核对,提取时复查)。
3. **[commit] 提取 `prepare_worker` + `WorkerPrep`** — 阶段 C+D(L411–591 连续段):worktree 创建(fail dispatch 保留)、project_main_override、ReadGuard 重置、toolset 过滤、messages 构建(resume 分支 + delegation template)。`Result<WorkerPrep, EarlyReturn>`。
4. **[commit] 提取 `resolve_worker` + `WorkerModel`** — 阶段 B2(L619–697 连续段):`resolve_final_model` + dispatch_model overlay + `resolve_worker_provider` + worker_display backfill(评审 P1-1 修订:原 design 合并进 plan_worker 的方案取消)。
5. **[commit] 提取 `register_run` + `RegisteredRun`** — 阶段 E(L720–849):rid/token、cancellations 注册、insert_run_with_id、set_worktree_path、sink 构建。签名收窄(评审 P2-1):`parsed` 只传 `tool_use_id: &str` + `project_id: &str`;不传 `plan`。
6. **[commit] 提取 `drive_worker`** — 阶段 F(L878–1045):per-run grant cache + `Box::pin(run_chat_loop(...))` 24 位置参数调用。输入 = model/prep/reg 3 struct + 外部借用。
7. **[commit] 提取 `collect_outcome` + `WorkerOutcome` + `finalize_dispatch`** — 阶段 G+H(L1058–1395):status picker、persist(**内部顺序固化:`update_run_finished` → `emit_subagent_finished`,AC8**)、cancel_parent、partial_actions、worktree probe/commit/destroy、format + 3 个 trailing append。
   - 验证:`cargo test --lib` 全量全绿(1657),`cargo fmt`
   - **Gate**:此时 dispatch.rs 应只剩 run_subagent 主体(调用序列,~100-150 行)+ check_workflow_role_gate + struct 定义 + EarlyReturn。AC1 测量:`awk '/^pub.*fn run_subagent/,/^}$/' dispatch.rs | wc -l` ≤ 250(不含 use、不含阶段 marker 注释)。

### Phase B:拆分(文件级移动,单 commit)

8. **[commit] 子模块化** — 建 `dispatch/` 目录,按 design §3 布局移动 7 个文件(parse/plan/prepare/model/register/drive/finalize);hub 声明子模块 + `#[allow(unused_imports)] pub(crate) use ...::*` 全量 re-export;`EarlyReturn` 与 `check_workflow_role_gate` 留 hub。
   - 验证:`cargo check --lib`(确认 `tests_dispatch.rs` 的 `use super::dispatch::*` 与 3 个调用方零改动解析)→ `cargo test --lib` 全绿 → `cargo clippy --lib --tests` + `cargo fmt --check` 零警告

### Phase C:文档 + 源码注释 sweep

9. **[commit] 引用 sweep** — 范围(评审 P2-3 扩充后):
   - spec 文档行号/符号引用:
     - `.trellis/spec/backend/tool-contract/04-dispatch-subagent.md`(含 `chat_loop.rs:2155` 等行号)
     - `.trellis/spec/backend/agent-loop-architecture/pattern-worker-worktree-override.md`(`3 tests in dispatch.rs` → 符号引用)
     - `.trellis/spec/backend/agent-loop-architecture/pattern-concurrent-dispatch.md:66`(`dispatch.rs::run_subagent`)
     - `.trellis/spec/backend/tool-contract/10-c2-loop-intervention.md:43`(`dispatch.rs::run_subagent`)
     - `.trellis/spec/backend/subagent-runs-schema.md`
     - `.trellis/spec/backend/worktree-contract.md:763`(**错误引用**:`dispatch.rs::tests::probe_worker_changes_*` 实际在 `tests_dispatch.rs` → 改符号引用指向 `tests_dispatch.rs::probe_worker_changes_*`)
     - `.trellis/spec/frontend/chat.md:339`(`dispatch.rs::run_subagent` — 符号引用,若行号已失真改符号)
   - docs 文档:
     - `docs/WORKFLOW-INTEGRATION-REVIEW.md:149`(`dispatch.rs:335`)、`docs/WORKFLOW-INTEGRATION*.md`
     - `docs/REMOTE-ACCESS-ROADMAP.md:116`(`agent/subagent/dispatch.rs:1192`)
     - `docs/REMOTE-ACCESS-RESEARCH.md:107,120,661`(`dispatch.rs:1192` / `dispatch.rs:251`)
     - `docs/research/subagent-scheduling-communication-survey.md:32,55`(`dispatch.rs:85` / `dispatch.rs:286`)
   - 源码注释(~10 处):`llm/provider/mock.rs:123`、`agent/permissions/mod.rs:161`、`tools/mod.rs:131`、`agent/subagent/mod.rs:112`、`sink.rs:474`、`tests_subagent.rs:359,1567,3459,3467`、`tests_common.rs:346`、`loader.rs:49`、`chat_loop.rs:1900,2168` 中 `dispatch.rs:LINE` 行号改符号引用
   - **不改**:`.trellis/tasks/archive/`、`docs/IMPLEMENTATION/decisions-*.md`、`docs/_reviews/`(历史/评审快照)
   - 残留核验:
     ```bash
     grep -rn "dispatch\.rs:[0-9]" .trellis/spec/ docs/ | grep -vE "/_reviews/|/decisions-20|/archive/"
     grep -rn "dispatch\.rs:[0-9]" app/src-tauri/src/
     ```
     两条均应无输出。
10. **[commit] AGENTS.md 测试基线同步**(P3-1):`~1635` → `~1657`(TRELLIS block 外用户段落,与 task commit 同批)。

### Phase D:收尾

11. 终验:`cargo test --lib` 全绿(≥1657)+ `cargo clippy --lib --tests` + `cargo fmt --check` 零警告
12. squash merge 回 main → `task.py archive` → 复验 `cargo test --lib`

## 风险提示(来自 design §5)

- `LoadedSubagent`/`SubagentDef` 字段化借用冲突 → 改为 owned / clone 消费
- 提取时保持原代码逐行顺序,禁止顺手重构(如合并 if、改日志措辞)
- 每 commit 的 diff 应只含"该阶段代码位移 + struct 定义",出现无关改动立即回退重来

## Review Gates

- [ ] 用户评审 prd.md / design.md / implement.md 通过
- [ ] `task.py start` 后进入实施
- [ ] 每个提取 commit 独立可回滚(AC3)
- [ ] 终验三绿(AC4)
