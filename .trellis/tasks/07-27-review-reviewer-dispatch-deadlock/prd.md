# review plugin reviewer 派发死锁 + 指引断裂 (C5)

> 父任务：`07-26-workflow-review-plugin`（review epic，第 6 个 child，C5）
> 依赖：**C4（intake 可用性）** 已落地。C4 让模型能"进入" review 流程,本任务解决它"卡死"在第一个真正要用 review 能力的地方。
> 来源：session `04c62fab-d79f-48f7-911b-b7ed02d92e4a`（2026-07-27 17:33–17:52）的 E2E。模型正确走完 intake（证明 C4 生效），但进 reviewing 派 reviewer 时撞死锁,最终被迫 edit_file 改 task.json + workflow.json。

## Goal

让 review workflow 的核心能力——**派多模型 reviewer 评审**——真正跑通。E2E 验收:在 `pnpm tauri dev` 下走完整 review 流,模型进 reviewing 后能从 `dispatch_subagent` 工具 enum 看到 `reviewer`,派出后 role gate 放行,无需任何手动 edit_file。

## Background(E2E 暴露的死锁链路)

session 04c62fab 在 C4 修复后,intake 阶段完全顺畅(seq=7→49 正确 discover model + 用户选 + 维度确认 + transition)。但 seq=53 派 reviewer 开始死锁,直到 seq=71 用户停止。完整死法:

```
seq=37  create_task 建 task(status=planning,dev 视角)     ← B1
seq=53  按 wf-review-method 想派 reviewer
        dispatch_subagent enum 没有 reviewer(只有            ← B2
        researcher/general-purpose/spec-auditor)
        模型只能派 researcher → role gate 拒
        (status=planning + 要 reviewer)
seq=63  模型被迫 edit_file 把 task.json status 改成 intake   ← B1 症状
seq=65  request_task_state_transition → reviewing(成功)
seq=67  再派 researcher → role gate 仍拒
        (reviewing 只许 reviewer,enum 还是没有 reviewer)     ← B2
seq=68  ⚠️ loop detected
seq=71  模型被迫 edit_file 改 review/workflow.json 的
        roles_by_state 想扩大允许角色                          ← 指引崩塌
```

seq=35 还暴露一个独立问题:模型在 review plugin session 里读了 `.trellis/workflow.md`(Trellis 给人看的开发流程文档),混进对 review 状态机的理解。thinking 原文:"让我看 docs/trellis/workflow.md 了解 review plugin 的状态转移规则"。← A(命名碰撞)

## 根因(三层叠加 + 一个命名问题)

### B2(P0 核心):dispatch_subagent enum 不加载 plugin 层 agent

两条 agent 发现路径不一致:

| 路径 | 用在哪 | 层数 | 看到 reviewer? |
|---|---|---|---|
| `cache.list(project_path)` | `definition_with_cache`(mod.rs:323)→ 构建 dispatch_subagent 的 subagent enum(LLM 看到的可选值) | 3 层(builtin+user+project),**无 plugin 层** | ❌ |
| `list_with_workflow(project_path, workflow_name)` | dispatch 执行时的 lookup | 5 层(含 builtin-plugin + project-plugin) | ✅ |
| `roles_by_state` | role gate 检查(dispatch.rs:1821) | 读 workflow_def | 要求 reviewer |

- `chat_loop.rs:1647` 调 `definition_with_cache(&subagent_cache, &project_path, &model_briefs)` —— **没传 workflow_name**。
- `definition_with_cache`(mod.rs:318)签名无 workflow_name 参数,内部调 `cache.list`(loader.rs:717)。
- `cache.list`(loader.rs:717)只有 builtin+user+project 三层,**漏了 plugin 层**。
- 而 `list_with_workflow`(loader.rs:780)有 5 层(含 plugin),**但工具 enum 没用它**。

`reviewer.md` 存在、`BUILTIN_REVIEW_AGENTS`(builtin.rs:89)注册了,dispatch 执行时能查到,但 LLM 选 subagent 的工具 enum 查不到。**两套数据源不一致 → 死锁**。

### B1(P0):create_task 写死 dev 的 "planning"

`create_task.rs:44,104` —— task.json 的 status 永远 "planning"(dev plugin 初始态),即使当前 session 是 review plugin。后果:
- review plugin session 建出的 task 初始状态对不上 review 状态机(intake)。
- role gate 读 status="planning",查 review workflow.json 的 `roles_by_state.planning` → 不存在 → allowed=(none),全 deny。
- 模型被迫手动 edit_file 改 status(seq=63)。

### B3(P1):skill 指引和实际工具 enum 不一致

`wf-review-method/SKILL.md:26`:
```
- `role` = `reviewer`(review plugin 的唯一角色)
```
skill 告诉模型派 `reviewer`,但(因 B2)dispatch_subagent enum 没有 reviewer。指引和可用工具矛盾,模型无所适从。

### A(P1):命名碰撞 — `.trellis/workflow.md` 误导

项目里"workflow"两个指代:
- `.trellis/workflow.md` — Trellis 框架给人读的开发流程文档(AGENTS.md 里的 TRELLIS 段落引用它)
- `.everlasting/workflow/<plugin>/workflow.json` — 引擎读的插件状态机定义

模型在 breadcrumb/skill 看到"workflow"+ AGENTS.md 的 Trellis 段落,自然翻 `.trellis/`。无任何指引告诉它"别去看 .trellis"。

## Requirements

### R1(B2,核心):dispatch_subagent enum 加载 plugin 层 agent

`definition_with_cache` + `cache.list` 感知 workflow_name,改用 `list_with_workflow`:

- `chat_loop.rs:1647` 传入 `workflow_ctx.as_ref().map(|c| c.workflow_def.name.as_str())`。
- `definition_with_cache`(mod.rs:318)签名加 `workflow_name: Option<&str>`,内部按需调 `cache.list_with_workflow`。
- review plugin session 下 enum 里出现 `reviewer`。

**验收**:单测覆盖 — review plugin session 的 dispatch_subagent enum 含 `reviewer`;dev plugin session 仍含 researcher/implementer/checker(无回归);非 workflow session 行为不变。

### R2(B1):create_task 感知 session plugin

`create_task` 建 task 时,status 用当前 session plugin 的 `initial`(review → intake,dev → planning),不再写死 "planning":

- `create_task.rs` 读 ToolContext 里的 workflow_ctx(若 plugin=review,initial=intake)。
- review plugin session 建出的 task 自动在 intake 状态,role gate 一开始就认 review 状态机。

**验收**:单测覆盖 — review plugin 下 create_task 的 task.status == intake;dev 下 == planning。

### R3(B3):wf-review-method skill 指引对齐工具

- 把"role = reviewer"改成"subagent 参数派 `reviewer`(在 dispatch_subagent enum 里)"。
- 加兜底:"如果 enum 里没有 reviewer,说明 plugin 层 agent 未加载(R1 bug),请报告而非手动 edit 配置"。
- 两份同步改:builtin(`app/src-tauri/resources/...`) + 项目级(`.everlasting/workflow/review/...`)。

**验收**:skill 文本含正确派发指引 + anti-edit-file 兜底。

### R4(A):wf-overview skill 加 anti-trellis 指引

wf-overview 开头加明确指引:
> "review 状态机定义在 `.everlasting/workflow/review/workflow.json`,但**你不需要手动读它** —— breadcrumb 和 skill 已给你全部状态信息。**不要**去读 `.trellis/workflow.md`,那是 Trellis 给人看的开发流程文档,与 review plugin 无关。"

两份同步改。

**验收**:skill 文本含 anti-trellis 指引。

## Non-goals

- **不改 review plugin 的 4-state 设计 / roles_by_state 语义**(那是 C3 决策;reviewing 只许 reviewer 是对的,B2 修好 enum 就能跑)。
- **不做 ask_user_question cancelled 的 isErr 修复**(那是 C4 标记的 follow-up `ask-user-question-skip-semantics`)。
- **不重命名 `.trellis/workflow.md`**(改动面太大,用 R4 的 skill 指引规避即可)。

> 注:R5(下方)落地后,"不改 role gate 逻辑本身"这条 Non-goal 被部分打破 —— role gate 的**状态机查表逻辑**必须改为读 task 的归属 plugin。但 role gate 的**deny/allow 判定本身**不变,只是数据源从 session.plugin 换成 task.workflow_plugin。

## 实施顺序

R1(B2)是解锁整个 review workflow 的关键,改完就能跑通。建议顺序:

1. **R1** —— 改 `definition_with_cache`/`cache.list` 感知 workflow。改完 GUI 即可验证派 reviewer 是否过 gate。✅ 已完成
2. **R2** —— 改 create_task 感知 plugin。消除模型手动改 status 的需要。✅ 已完成
3. **R3 + R4** —— 改 skill 文本(两份同步)。✅ 已完成
4. **R5** —— task.json 加 `workflow_plugin` 字段,role gate / transition / breadcrumb 按 task 归属 plugin 查表。解决跨 plugin 切换死锁(下方)。⬜ 待实施

## R5(P0,新增 2026-07-28):task 记录归属 plugin,支持跨 plugin 切换

### 场景(用户补充,2026-07-28)

**同一 session 内 dev↔review 切换** —— 这是 review plugin 的核心使用场景(wf-overview skill 写的"review 改 prd,dev 读同一 prd 实施"):

```
session(同一个)
  ├─ dev plugin:  create_task → 写 prd(status=planning)
  ├─ 切 review:   评审 prd → 修订(status: intake→reviewing→revising→reported)
  └─ 切回 dev:    读修订后的 prd 实施(status: planning→in_progress→done)
```

### 当前代码的死锁(状态机推演验证)

task.json 只有 `status` 单值,没有 plugin 归属。切 session plugin 后,role gate / transition 用**新 plugin 的状态机**查**旧 plugin 留下的 status** → 查表失配 → 全 deny:

| 步骤 | session plugin | task.status | role gate 结果 | transition 结果 |
|---|---|---|---|---|
| dev create | dev | planning | 🟢 researcher 允许 | — |
| 切 review | review | planning | 🔴 **全 deny**(review 无 planning 键) | 🔴 review 无 planning→任何 |
| 强改 intake | review | intake | 🔴 intake 允许角色=空(设计如此) | 🟢 intake→reviewing OK |
| 评审完 | review | reported | — | — |
| 切回 dev | dev | reported | 🔴 **全 deny**(dev 无 reported 键) | 🔴 dev 无 reported→任何 |

**两个状态机完全不重叠**(dev: planning/in_progress/done;review: intake/reviewing/revising/reported),没有任何桥接转换。

### 根因

task 的 status 字段是单值,但**没有记录"这个 status 属于哪个 plugin"**。C0(`07-26-taskstatus-custom-state`)让 TaskStatus 支持 Custom(String),让 review 状态能 round-trip,但没解决归属问题。C5 的 R2 让 review task 建 intake,放大了这个潜伏 bug。

### 修复方案(用户拍板:task 记录归属 plugin)

task.json 加 `workflow_plugin: String` 字段。所有读 task 状态的门控点改为**按 task 的归属 plugin** 查状态机,而非 session plugin:

1. **create_task**:`workflow_plugin` = ctx 当前 plugin(dev/review),与 initial_status 一起写入。
2. **role gate**(dispatch.rs:1830):`allowed_roles` 用 `load_workflow(task.workflow_plugin)` 而非 session plugin。
3. **request_task_state_transition**:transition 校验用 task 归属 plugin 的 workflow_def。
4. **breadcrumb**(inject.rs:breadcrumb_body):state 查表用 task 归属 plugin。session plugin 只影响**展示哪个 skill / 工具集**(dispatch enum 等),不影响 task 推进。
5. **resolve_current_task**:读 task 时一并读 workflow_plugin 字段(lenient 兜底:缺字段时按 session plugin 推断,兼容旧 task)。

**跨 plugin 共享 task 的语义**:同一 project 下,dev session 和 review session 共享同一 current_task(resolve_current_task 按 project 扫)。但:
- dev session 看到的是 task 的 **dev 状态视角**(planning/in_progress/done)。
- review session 看到的是 task 的 **review 状态视角**(intake/reviewing/...)。
- 如果一个 task 是 dev 建的(planning),review session 打开它时,role gate 按 task.workflow_plugin=dev 查表 → review 的 reviewer 派不出去(dev 的状态机不允许 reviewer)。

→ 这意味着**同 session 切换**和**跨 session 共享**是两种不同语义:
- **同 session 切换**(本场景):task 归属 plugin 不变,session plugin 切换只改 skill/工具展示。但 task 的状态推进仍受限于它归属的 plugin 状态机 —— **dev task 切到 review plugin 后,无法直接进 review 的 reviewing 状态**(因为 task 属于 dev)。

→ **决策(用户拍板 2026-07-28):选项 A1 —— task 归属跟随 session plugin 切换**。

切 session plugin 时,`set_session_plugin_name` 同步:
1. task.workflow_plugin 改成新 plugin。
2. task.status 重映射到新 plugin 的 `initial`(dev→planning,review→intake)。
3. 记一条 transition log 到 task.json(可选,future)。

status 重映射会丢"上次切到哪个状态",但 **prd.md 是单一真源**(内容不丢),所以 dev→review→dev 全程的产物(prd 修订)始终保留,符合"prd 即产物"的设计。

完整场景走通(状态机推演):
```
dev create        → task(plugin=dev, status=planning)  写 prd
切 review         → task(plugin=review, status=intake)  prd 不变
  intake→reviewing→revising→reported                   修订 prd
切回 dev          → task(plugin=dev, status=planning)   prd 已含评审修订
  planning→in_progress→done                            实施
```

**验收**:状态机推演验证 dev→review→dev 全程无死锁;单测覆盖 task.workflow_plugin round-trip + role gate 按归属 plugin 查表。

## Open questions

1. ~~R1 签名变更的影响面~~(已确认:1 生产调用点 + 7 测试,已改)。
2. ~~R2 的 workflow_ctx 传递~~(已确认:ToolContext.workflow_name 已存在,直接用)。
3. ~~R5 选项 A1 vs A2~~(已决策:A1 跟随切换 + status 重映射)。


## Open questions

1. **R1 签名变更的影响面**:`definition_with_cache` 有多个调用方(chat_loop.rs:1647 + 其他)。需确认所有调用点都改。倾向加 `workflow_name: Option<&str>` 参数(非 workflow session 传 None,行为不变)。
2. **R2 的 workflow_ctx 传递**:create_task 的 ToolContext 是否已有 workflow_ctx?若无,需从 chat_loop 注入。需先查 ToolContext 结构。
