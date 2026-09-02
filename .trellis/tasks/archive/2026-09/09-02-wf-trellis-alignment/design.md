# 设计:wf-trellis-alignment

## 0. 现状链路(已核实,2026-09-02)

状态转移工具链(回环 R1 的验收依据):

```
LLM → request_task_state_transition::execute_blocking (tools/request_task_state_transition.rs)
      ├─ legality: can_transition(def, from, to) — workflow.json transitions 是唯一事实源
      └─ Allow 后 → resolve_task_state_transition_internal (commands/question.rs)
            └─ set_task_state (agent/workflow/state.rs)
                  └─ dispatch_hook: match (from,to)
                       (InProgress,Done)   → trigger_spec_distillation
                       (Planning,InProgress) → preflight_implement_check
                       _                   → no-op   ← (InProgress,Planning) 落这里,正确
```

结论:**引擎无需新代码即支持回环边**;要动的只有「声明 + 文案」。佐证:review workflow 的 `revising→reviewing` 回环边已在生产跑(同一条工具链),C5 后合法性校验用的是 `task_workflow_def`(任务所属插件,`chat_loop/tools.rs:1420`),非 session 插件。已核实的坑:

- `def.rs::default_workflow()` 是硬编码 fallback,`builtin.rs::builtin_dev_json_equals_default_workflow_constant` 锁死它与内置 workflow.json 逐字等价 → 任何 dev 定义改动必须**三处同步**(def.rs / resources/builtin-workflow/dev/ / .everlasting/workflow/dev/ 镜像),镜像用 `diff -r --exclude=README.md` 验收(dev/README.md 约定)。
- 工具 schema 描述写死 "transition to the next phase",前向偏置会压低 LLM 发起回滚的概率,必须改文案(`request_task_state_transition.rs::definition`)。
- 再次 `planning→in_progress` 时 `preflight_implement_check` 的 marker 幂等(`has_marker` 短路),回环重入安全;`trigger_spec_distillation` 同理。回滚本身 no-op,无 marker。
- `TaskItem.status` 复用 `TaskStatus`,回环后 items 不自动变;由主 LLM `update_checklist` 重拆(内容层指引,不建引擎逻辑)。

## 1. R1 回环边

### 1.1 三处同步的 dev 定义改动

`transitions` 追加:

```json
{"from": "in_progress", "to": "planning", "requires_user_confirm": true}
```

`breadcrumb` 文案调整(两处 workflow.json + def.rs 同步):

- `in_progress`(追加一行):`check 揭示 prd 缺陷或需求变化时,用 request_task_state_transition 申请回 planning 修 prd(用户确认);回 planning 后用 update_checklist 重拆 items(done 项可保留)`
- `planning`(追加一行):`若本任务是从 in_progress 回环回来的:先读最新 prd 与 items,只修缺陷部分,再申请回 in_progress`
- `done` 不加回环边(done 是终态侧;需要返工应新建任务,避免 done↔in_progress 振荡 —— Trellis 同样无 done→* 回退)。

`default_workflow()`(def.rs)同步 transitions + breadcrumb + description(评审 P3:描述仍是 `planning → in_progress → done`,改为 `planning ⇄ in_progress → done`),与 JSON 保持反序列化字段相等(mirror 测试兜底;注意等价性是反序列化后逐字段,HashMap 顺序无关,不是逐字 diff)。

### 1.2 工具文案(request_task_state_transition.rs::definition)

description 改写要点:删 "completed a workflow phase ... next phase" 前向表述,改为「目标 state 只要是当前 workflow 声明的边(前进或回滚,如 dev 的 in_progress→planning)都合法;典型回滚场景:check 揭示 prd 缺陷」。schema 里 `target_state` 的描述同步去掉方向性暗示。纯文案,不改 execute_blocking 逻辑。

### 1.3 内容层(wf-overview skill)

- 「整体流程」改为 `planning ⇄ in_progress → done`,注明回环边存在与触发条件。
- 「门控」一节补:回环也走协商档(ask → 用户确认)。
- **工具指向修正(评审 P2)**:wf-overview L18/L75 与 wf-brainstorm L22 现写"用 ask_user_question 申请切 state"——ask_user_question 只提问不落盘,真正翻转 `task.json.status` 的工具是 `request_task_state_transition`(Allow 后由 IPC handler 写盘)。三处表述统一改为 `request_task_state_transition`;R1 回环指引同样指向它。
- 文件表补 R2 的 `research/` 与 R3 的 `relevant-specs.jsonl` 行(见下)。

## 2. R2 调研落盘

角色职责不变(researcher 只读、findings 回主 LLM),**落盘责任放主 LLM**(改内容层,不动 researcher 的 tools/isolation 声明):

- delegation 模板 `researcher`(workflow.json + def.rs):结尾追加一句「输出请按主题分节,主 LLM 会把 findings 落盘到 task 的 research/ 目录」。
- `wf-brainstorm` 新增「调研落盘」节:dispatch researcher 后,主 LLM 必须把 findings 写 `.everlasting/tasks/<slug>/research/<topic>.md`(一主题一文件),prd.md 技术方案节引用之;回环场景追加写不覆盖。
- `wf-overview` 文件表加:`research/ | planning | 调研 findings(主 LLM 从 researcher 返回值落盘)`。
- planning breadcrumb 已有「调研」字样,补「落盘 research/」四字提示。

### 2.1 隔离 worker 可见性(评审 P1,已核实并修正机制)

**事实**:`.everlasting/tasks/` 整体 gitignored(`.gitignore:68`)。implementer 声明 `isolation: true`(agents/implementer.md frontmatter,经 `subagent/cache.rs` 解析),worker 由 `create_worker` 基于parent HEAD 检出真 git worktree(`tests_worktree.rs:385` 钉死该行为)→ tasks/ 下任何文件(含**已提交的也不存在**,目录根本不进 git)对 implementer 结构性不可见。checker / researcher 无隔离(共享 cwd),不受影响。

**推论**:research 文件对 implementer 的不可见是**默认情况**,`{relevant_specs}` 里的 research 条目对 implementer 只是提示性引用。对策全部走内容层,不做引擎改动(绝对路径注入会撞 permission 边界,不值):

- `wf-brainstorm` 调研落盘节补一句:派 implementer 前,把 research 关键结论**摘要进 delegation 文本**(`{summary}` 填充)——这是唯一可靠通道。
- implementer delegation 模板(workflow.json + def.rs)补:「delegation 文本中的调研摘要优先于 research/ 路径;路径 read 不到(隔离 worktree 无 `.everlasting/tasks/`)时在 Known issues 注明,勿臆测内容」。
- `{relevant_specs}` 注入文本由父进程侧替换(compute_delegation_template 在 parse/prepare.rs 完成),策展清单文件本身不需要 worker 可读——worker 只需读到其中 **spec 条目**指向的文件(`.everlasting/spec/` 不在 gitignore,已提交即在 worktree)。

## 3. R3 spec 上下文策展

### 3.1 文件约定

`.everlasting/tasks/<slug>/relevant-specs.jsonl`,每行:

```json
{"file": ".everlasting/spec/backend/agent/index.md", "reason": "本次改 agent loop 注入层"}
{"file": ".everlasting/tasks/<slug>/research/auth-comparison.md", "reason": "方案选型结论"}
```

与 Trellis 的 implement/check 双清单相比做了减法:**单清单**(builtin 的 implementer/checker 共享 delegation 注入通道,拆两份收益低;checker 的 wf-check 自带验收维度)。`_example` 种子行不需要——文件缺失即 fallback,无需种子。

### 3.2 inject.rs 改动

`resolve_relevant_specs(project_path)` → `resolve_relevant_specs(project_path, task_slug: Option<&str>)`:

1. `Some(slug)` 且 `<project>/.everlasting/tasks/<slug>/relevant-specs.jsonl` 存在:逐行 serde_json 解析,输出 `file — reason` 逐行列表;**空文件 / 全部行解析失败 → 回退 2**(不输出空串,保证 worker 始终有可行动提示);部分行失败 → 跳过坏行,不整文件回退。
2. fallback:现有全树递归罗列,行为逐字不变(含 `(auto-detect via wf-before-dev)` 兜底)。

`compute_delegation_template` 从 `workflow_ctx.current_task.as_ref().map(|t| t.slug)` 取 slug 传入。调用方只有 delegation 注入一处(`subagent/dispatch/prepare.rs:275`)。**已知路径分叉(评审 P3,实施时以代码注释点明)**:调用方传入的 `project_path` 源自 `current_ctx.worktree_path`(`dispatch/parse.rs:82`),而 `ctx.current_task` 经 `build_workflow_ctx` 从 DB `project.path` 解析——session 本身跑在 session worktree 时两路径分叉,策展查找会 miss,fallback 全树罗列兜底(可接受,不为此引入 DB 依赖)。

### 3.3 内容层

- `wf-brainstorm` 拆 items 步骤后追加:「写 relevant-specs.jsonl 策展 spec + research 清单(file+reason),给 implementer/checker 的 delegation 注入用」。
- `wf-overview` 文件表加:`relevant-specs.jsonl | planning | spec/research 策展清单(injection 用)`。

## 4. 兼容性 / 风险

| 风险 | 对策 |
|---|---|
| 三处 dev 定义漂移 | mirror 测试 + `diff -r` 验收;implement.md 列强制顺序(def.rs 与两个 JSON 同一 commit) |
| 老 in_progress 任务突然多出回环边 | 边只增不改;旧 breadcrumb 无指引但工具合法即可,无破坏。先例:review 的 revising→reviewing 已在跑 |
| **research/ 与 relevant-specs.jsonl 对 implementer 不可见**(tasks/ gitignored + implementer isolation:true,结构性、commit 无解) | 见 §2.1:关键结论摘要进 delegation 文本(wf-brainstorm 指引 + implementer 模板);{relevant_specs} 的 spec 条目不受影响(spec/ 已提交即在 worktree) |
| 策展清单里的路径失效(文件删了 / session worktree 路径分叉) | 注入时仅文本引用,worker read 失败自会处理;不做存在性校验(保持 inject 纯文本、无 IO 逐文件检查);分叉场景 fallback 兜底(§3.2) |
| 回环后 preflight/spec marker 状态 | 已核实幂等,无需清理逻辑 |
| review workflow 误伤 | 只改 dev 目录;review 测试回归 |

## 5. 测试设计

- `def.rs` / `builtin.rs`:既有 mirror 测试自动覆盖新边;补断言 `can_transition(dev, "in_progress", "planning") == true`(放 tests 定义处或 def tests)。
- **delegation 模板新用例放 `mod.rs` 既有 delegation tests 旁(~L692-780,评审 P3 指正:compute_delegation_template 用例现状在 mod.rs 而非 tests_inject.rs)**;`tests_inject.rs` 只放 `resolve_relevant_specs` 纯函数用例:
  - 有 relevant-specs.jsonl(2 好行 + 1 坏行)→ 注入含好行 `file — reason`,不含坏行、不含全树罗列;
  - 空文件 / 无该文件 → 与现状输出一致(fallback,断言时勿复制粘贴构造逻辑,直接对拍函数输出);
  - `compute_delegation_template` 传入带 slug 的 ctx → 模板含策展内容(mod.rs)。
- 状态转移链路:沿用 `tests_request_task_state_transition.rs` + `state.rs` tests 风格,补「in_progress→planning 无 hook marker」用例(state.rs)。
- 内容层验收:`diff -r --exclude=README.md` 双向一致 + wf-overview/wf-brainstorm 人工核对(无自动化)。
