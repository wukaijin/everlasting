# builtin workflow 对齐 Trellis 优秀设计:回环边 + 调研落盘 + spec 策展

## 背景

对 builtin `dev` workflow(`app/src-tauri/resources/builtin-workflow/dev/`)与 Trellis 流程(`.trellis/workflow.md`)做过差距分析,确认 builtin 的骨架(状态机 + 角色门控 + 对抗式 implement/check)更强,但三类 Trellis 过程机制缺失,长任务下有实际代价:

1. **无回环**:状态机只有 `planning→in_progress→done` 前向边;check 揭示 prd 缺陷时实施中任务没有合法回头路(Trellis Rules 明确 "Phases can roll back")。
2. **调研不落盘**:researcher findings 只返回给主 LLM(聊天),无 `research/` 目录约定;对话被压缩后调研结论只剩 prd 里的二手摘要(Trellis: "conversations get compacted, files don't")。
3. **spec 注入是全树罗列**:`{relevant_specs}` 递归列出 `.everlasting/spec/` 全部 `.md` 路径(`inject.rs::resolve_relevant_specs`),不按任务策展;spec 树越大噪声越大(Trellis 用 per-task jsonl 清单 + reason)。

非本期(已做或明确不做):
- 提交纪律 breadcrumb 已由 `336a164e`(09-02-wf-dev-breadcrumb-commit-hint)落地,不重复。
- commit 协议细化(未识别文件分类 / 一次性批量确认)、break-loop skill、planning ready gate、任务树指引、session journal:列为后续任务候选,本期不做(见 非目标)。

## Goal

把上述三项 Trellis 机制适配进 builtin dev workflow 的引擎与内容层,保持三处 dev 定义源(内置资源目录 / `.everlasting` 镜像 / `def.rs::default_workflow()`)一致,不破坏现有测试与 review workflow。

## Requirements

### R1 回环边 `in_progress → planning`

- dev workflow 声明回环转移 `in_progress→planning`,`requires_user_confirm: true`。
- 回滚语义:仅状态回退,不触发任何 `(from,to)` hook(沉淀 / preflight 不因回滚误触发,再次 `planning→in_progress` 时 preflight marker 幂等跳过)。
- 工具链可达:LLM 通过 `request_task_state_transition` 申请,用户 Allow 后落盘;工具描述文案去掉 "next phase" 前向偏置,明确"声明的边(含回滚)都合法"。
- 内容层工具指向修正(评审 P2):wf-overview / wf-brainstorm 里"用 ask_user_question 申请切 state"的表述改为 `request_task_state_transition`(ask_user_question 只提问不落盘 task.json.status)。
- `description` 字段三处同步为含回环的表述(`planning ⇄ in_progress → done`)。
- 内容层指引:planning / in_progress breadcrumb + wf-overview 说明回环触发条件(check 揭示 prd 缺陷、需求变化)与回环后 items 的处置(主 LLM 用 `update_checklist` 重拆,done 项可保留)。

### R2 调研落盘 `research/`

- 新增约定:`.everlasting/tasks/<slug>/research/<topic>.md`,一个主题一个文件。
- researcher 保持只读(不改其隔离/worktree 约束);findings 仍返回主 LLM,由**主 LLM** 在 planning 阶段把 findings 落盘到 research/(在 delegation 模板、wf-brainstorm、wf-overview 中写明)。
- 隔离可见性对策(评审 P1,内容层):`.everlasting/tasks/` 整体 gitignored(`.gitignore:68`),research/ 对隔离 worker(仅 implementer,`isolation: true`)的 worktree **结构性不可见**(commit 也救不了);checker/researcher 共享 cwd 可见。对策:派 implementer 前把关键调研结论摘要进 delegation 文本,wf-brainstorm + implementer delegation 模板写明。
- wf-overview 的 task 文件表补 `research/` 行。

### R3 spec 上下文按任务策展 `relevant-specs.jsonl`

- 新增约定:planning 阶段主 LLM 写 `.everlasting/tasks/<slug>/relevant-specs.jsonl`,每行 `{"file": "<repo 相对路径>", "reason": "<为什么相关>"}`,收录 spec 与 research 文件。
- `inject.rs` 注入 `{relevant_specs}` 时优先读当前任务的 relevant-specs.jsonl(输出 `path — reason` 列表);缺失 / 为空 / 解析失败时回退现有全树罗列行为(向后兼容,老任务不受影响)。
- wf-brainstorm 在 planning 收尾步骤中加策展指引;wf-overview 文件表补该文件。

## Acceptance Criteria

- [ ] `request_task_state_transition` 对 `in_progress→planning` 返回合法(不再 `invalid_transition`),用户 Allow 后 `task.json.status=planning`,无任何 hook marker 写入;再次 `planning→in_progress` 正常(已有 preflight marker 不重复)。
- [ ] `request_task_state_transition` 对 `in_progress→planning` 返回合法(不再 `invalid_transition`),用户 Allow 后 `task.json.status=planning`,无任何 hook marker 写入;再次 `planning→in_progress` 正常(已有 preflight marker 不重复)。
- [ ] `builtin_dev_json_equals_default_workflow_constant` 测试通过(def.rs 常量与内置 JSON **反序列化后字段相等**——HashMap 顺序无关,非逐字 diff);内置目录与 `.everlasting` 镜像 **byte-identical**:`diff -r --exclude=README.md app/src-tauri/resources/builtin-workflow/dev .everlasting/workflow/dev` 无差异。
- [ ] review workflow(intake/reviewing/...)不受影响,其现有测试全绿。
- [ ] 有 relevant-specs.jsonl 的任务:delegation 提示词中 `{relevant_specs}` 为策展列表(含 reason);无该文件的任务:行为与现状逐字一致(全树罗列 fallback)。tests_inject 覆盖两种路径 + 损坏 JSON 行回退。
- [ ] 内容文件(两个 dev workflow.json 的 breadcrumb / delegation 模板 / description、wf-overview、wf-brainstorm)同步了 R1/R2/R3 的指引,review workflow 文件不动。
- [ ] wf-overview / wf-brainstorm 不再引用 ask_user_question 作为状态转移通道;implementer delegation 模板含「delegation 内调研摘要优先、路径不可达写 Known issues」指引。
- [ ] `cargo test -p everlasting --lib` 全绿。

## 约束

- researcher / checker 的只读约束与隔离理由(agents/*.md frontmatter 注释)不得破坏。
- 不改 `TaskJson` schema、不做 DB 迁移;新增约定全部走文件(sidecar jsonl / research 目录)。
- 引擎改动限于 `inject.rs`(relevant_specs 解析)与工具描述文案;`state.rs` / `def.rs::validate` / IPC resolve 路径不改逻辑。
- 项目 spec 对齐:`.trellis/spec/backend` 相关 guideline(实现前经 trellis-before-dev 加载)。
