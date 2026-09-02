# 实施计划:wf-trellis-alignment

前置:`trellis-before-dev` 加载 `.trellis/spec/backend` 相关 guideline。以下步骤按依赖排序;步骤 1-2 必须同批落地(三处 dev 定义等价性被测试锁定,拆开 commit 会红)。

## Step 1:R1 回环边 — 定义三处同步 + 工具文案

- [x] 1.1 `app/src-tauri/resources/builtin-workflow/dev/workflow.json`
  - `transitions` 追加 `{"from":"in_progress","to":"planning","requires_user_confirm":true}`
  - `description` 改为 `planning ⇄ in_progress → done` 表述(评审 P3)
  - `breadcrumb.planning` / `breadcrumb.in_progress` 按设计 §1.1 追加回环指引行
  - `delegation_templates.researcher` 按设计 §2 追加落盘提示句
  - `delegation_templates.implementer` 按设计 §2.1 追加「delegation 摘要优先 / 路径不可达写 Known issues」句
- [x] 1.2 `app/src-tauri/src/agent/workflow/def.rs::default_workflow()` 同步以上全部(反序列化字段相等)
- [x] 1.3 镜像同步:把 1.1 的改动复制到 `.everlasting/workflow/dev/`(workflow.json + skills),`diff -r --exclude=README.md app/src-tauri/resources/builtin-workflow/dev .everlasting/workflow/dev` 为空
- [x] 1.4 `app/src-tauri/src/tools/request_task_state_transition.rs::definition()` 文案去前向偏置(设计 §1.2);`state.rs` 补测试:`in_progress→planning` 不写任何 marker
- [x] 1.5 `def.rs` tests 补 `can_transition(dev,"in_progress","planning")` 断言
- [x] 验证:`cargo test -p everlasting --lib workflow::` 全绿(mirror 等价 + 新断言)——109 passed / 0 failed

## Step 2:R2 调研落盘 + 内容层工具指向修正

- [x] 2.1 `dev/skills/wf-brainstorm/SKILL.md`(builtin + 镜像两份):新增「调研落盘」节(设计 §2,含 §2.1「派 implementer 前把关键结论摘要进 delegation 文本」);**L22 的 ask_user_question 改为 `request_task_state_transition`**(评审 P2)
- [x] 2.2 `dev/skills/wf-overview/SKILL.md`(两份):文件表加 `research/` + `relevant-specs.jsonl` 行;「整体流程」改 `planning ⇄ in_progress → done` + 回环说明;**L18 与「门控」节的 ask_user_question 改为 `request_task_state_transition`**(设计 §1.3)
- [x] 2.3 planning breadcrumb 补「落盘 research/」提示(已在 1.1/1.2 顺带做掉则勾掉)

## Step 3:R3 spec 策展 — 引擎 + 内容层

- [x] 3.1 `app/src-tauri/src/agent/workflow/inject.rs`
  - `resolve_relevant_specs(project_path)` → `resolve_relevant_specs(project_path, task_slug: Option<&str>)`,策展优先 / 逐字 fallback(设计 §3.2,含坏行跳过、空文件回退);代码注释点明 project_path 可能是 session worktree 路径(与 current_task 的 DB project.path 分叉,设计 §3.2)
  - `compute_delegation_template` 传 slug
- [x] 3.2 测试:`resolve_relevant_specs` 纯函数用例放 `tests_inject.rs`;`compute_delegation_template` 集成用例放 `mod.rs` 既有 delegation tests 旁(~L692-780,评审 P3);策展命中 / 坏行跳过 / 空、缺失 fallback(设计 §5)
- [x] 3.3 `wf-brainstorm`(两份)加策展指引;`wf-overview`(两份)文件表加 `relevant-specs.jsonl` 行
- [x] 验证:`cargo test -p everlasting --lib agent::workflow::` + `tests_inject` 全绿

## Step 4:全量验收

- [x] 4.1 `cargo test -p everlasting --lib` 全绿(PKG_CONFIG_PATH 见 AGENTS.md;别加 --test-threads=1)——2230 passed / 0 failed / 1 ignored
- [x] 4.2 review workflow 回归:`cargo test -p everlasting --lib workflow::builtin`(review 相关断言)+ 人工确认 review 目录零改动(`git status` 只含 dev 路径)
- [x] 4.3 三处一致性终检:mirror diff 为空;`task.json` 等价测试绿
- [ ] 4.4 烟测(可选,有 GUI 环境):对 in_progress 任务调 `request_task_state_transition(target_state="planning")`,确认卡片出现、Allow 后 status=planning、task.json 无新 marker(本环境无 GUI,未跑;引擎侧由 state.rs `set_task_state_rollback_to_planning_writes_no_hook_marker` + can_transition 断言覆盖)

## 回滚点

- Step 1-2 纯声明/文案:revert 单个 commit 即可,无数据迁移。
- Step 3 引擎:fallback 行为保持现状,策展文件是增量约定,出问题删 jsonl 即回旧行为。

## Commit 规划

1. `feat(workflow): dev 声明 in_progress→planning 回环边 + 工具文案去前向偏置`(Step 1,三处同批)
2. `docs(workflow): wf-brainstorm/wf-overview 对齐调研落盘 + spec 策展约定`(Step 2 + 3.3)
3. `feat(workflow): relevant-specs.jsonl 按任务策展 {relevant_specs} 注入`(Step 3.1-3.2)
