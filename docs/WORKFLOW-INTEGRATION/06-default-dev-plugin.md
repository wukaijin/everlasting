## 6. 默认 plugin `dev`(开发流)内容详解

本节是 MVP plugin 的具体内容。Phase 2+ 外置成 `.everlasting/workflow/dev/workflow.json` 后,这里就是那份文件的草稿。

### 6.1 state 枚举

| state | 允许角色 | breadcrumb 提示 |
|---|---|---|
| `planning` | researcher | "你在 planning。先 dispatch researcher 调研,产出 prd.md + 拆 task.json.items(实施阶段);不要写实现代码。完成后问用户确认进 implement" |
| `implement` | implementer, checker | "按 checklist 逐项 dispatch implementer;tdd item 强制 red→green;每项后派 checker 验收。全部 done 后问用户确认进 check" |
| `check` | checker | "派 checker 做最终全量验收(跨层一致性+lint+typecheck)。通过后进入沉淀+done" |
| `done` | (agent 做沉淀) | "task 完成。把本次决策/教训提炼写进 .everlasting/spec/,然后归档 task" |

> **Q3 决定(2026-07-07)**:**不加 blocked**。理由:blocked 在 Everlasting 场景无真需求——等用户走 `ask_user_question`(且 state 转移本身就是用户确认门);等 worker 走 B6 dispatch 的 await;workflow 本地无外部异步等待。state 枚举保持四态。未来真有跨 task 编排(B8)再加,是加法不破坏现有。
>
> **Q3 补充决定(门控协商档,S3 后统一)**:门控违反时**统一走协商**——engine 主动 `ask_user_question` 问用户(角色违规 + state 转移共用一套协商,不分层)。例:task 在 planning,agent 想 dispatch implementer 写代码 → 弹窗"这个 task 还在 planning,允许派 implementer / 确认推进到 implement 吗?"。用户允许 → 放行(角色一次性越权,或推进 state);拒绝 → 回 breadcrumb 提示。**S3 评审修正后:不存在硬拒档**(原 Q3 表述提过"区别于硬拒 tool_error",S3 决定统一协商,删除硬拒档)。执行点在 `chat_loop.rs` dispatch 拦截处(见 §6.6.2),engine 调用,非 agent 自觉。注意:协商的是 **task 的 state / 角色越权**(workflow 内),不是权限 Mode(edit/plan/yolo,跟 workflow 正交,agent 不动权限旋钮)。

### 6.2 task 文件态记账

```
<project>/.everlasting/tasks/<slug>/
  ├── task.json          # 元数据 + checklist items(见下 schema)
  ├── prd.md             # planning 写
  ├── progress.md        # state 转移时更新(交接叙述)
  ├── design.md          # 可选
  └── spec-diff.md       # 可选,沉淀草稿
```

**checklist 同步方案(S2 评审修正,选 c)**:**checklist items 内嵌进 task.json**(单一数据源),不再用独立 `checklist.md`。理由:B12 现有 `update_checklist` 维护 loop-local `Vec<ChecklistItem>`(每 request 重建,不写文件),跟 workflow 需要的"跨 session 续 task + worker 看进度"完全割裂。选方案 (c):

- task.json 加 `items: [{id, content, status, tdd?}]` 字段
- workflow session 内,`update_checklist` tool 改写 task.json 的 items(而非 loop-local Vec)
- worker 读 task.json 即拿到 checklist 进度(无需独立文件,无需 B12 Vec 同步)
- B12 的"仅一个 in_progress"约束保留(coerce 逻辑不变,只是写目标从 Vec 换成 task.json)
- 非 workflow session:B12 行为不变(loop-local Vec)

**task.json schema**(含 items):

```json
{
  "id": "uuid",
  "title": "...",
  "slug": "...",
  "status": "planning",
  "created_at": "...",
  "updated_at": "...",
  "parent": null,
  "summary": "...",
  "items": [
    { "id": "backend-impl", "content": "后端实施", "status": "done" },
    { "id": "backend-test", "content": "后端测试", "status": "done" },
    { "id": "frontend-impl", "content": "前端实施", "status": "in_progress", "tdd": true }
  ]
}
```

**无 DB 表**。task 列表 = `list_dir .everlasting/tasks/` + 解析 task.json,agent 同款发现 task。"current task" 是会话内 in-memory 状态,不入 sessions 表。session 关掉重开,agent 读目录找 status≠done 的自然续上(items 进度跟着 task.json 跨 session)。

**首次起 task 的触发点**(小问题4 评审修正):workflow session 里 agent 收到第一个 user message 后,先 `list_dir .everlasting/tasks/`:
- 有 status≠done 的 task → 读取其 progress.md,问用户"是否续上 task X"(自然交接)
- 无未完成 task / 用户要做新事 → agent 判断需要起 task 时(用户表达了一个开发意图),写 task.json + 空 prd.md,进 planning state
- 用户只是闲聊/问问题(非开发意图)→ 不起 task,workflow 开关虽开但不激活 state machine(等用户表达开发意图)

即:**起 task 不是"开 workflow 就起",是"agent 判断用户表达了开发意图才起"**。这条判断由 agent 在 wf-overview skill 指导下做。

### 6.3 skill 包(plugin 自带 + 全局三层覆盖)

wf-* skill 由 **workflow plugin 自带**(放 `.everlasting/workflow/dev/skills/`,对标 Q5 plugin 自带 agents/),实现 plugin 自洽可移植。dispatch 时解析顺序:plugin skills → project 全局(`.everlasting/skills/`)→ user 全局 → builtin(skill 无 builtin fallback 则报 skill not found)。用户想自定义 wf-* skill → 在 plugin 目录覆盖对应 `SKILL.md`。

| skill | state | 作用 |
|---|---|---|
| `wf-overview` | 进 workflow 时 / 任意时刻自查 | **workflow 全貌说明**:整体流程 + 角色 + 每 state 该干嘛 + 怎么用 checklist。agent 进 workflow 时 breadcrumb 提示加载,建立全局意识(回答"我处于哪个 workflow、整体怎么干") |
| `wf-brainstorm` | planning | 调研+写 prd+拆 checklist(借鉴 trellis-brainstorm) |
| `wf-before-dev` | implement 入口 | 加载项目 spec 规范后再写代码(借鉴 trellis-before-dev) |
| `wf-check` | implement 每项后 / check | 验收:lint/typecheck/跨层一致性(借鉴 trellis-check) |
| `wf-update-spec` | done | 把决策/教训提炼进 `.everlasting/spec/`(借鉴 trellis-update-spec) |

skill 是**描述性规范**(markdown body),agent 通过 `use_skill` 加载,不执行代码。breadcrumb 提示"现在该加载 wf-before-dev",但不替 agent 调用(保 B4 三层渐进披露语义)。

> **Q4 决定(2026-07-07)**:**不自动 inject skill body**。靠 breadcrumb 提示(把 skill 名写死,如"use_skill wf-before-dev")+ agent 自觉 use_skill。理由:(1) 保 B4 三层渐进披露(L0 清单常驻/L1 回填/L2 按需拉),自动 inject 等于把 L2 内容常驻,破坏分层;(2) 让 agent 判断"我需要加载规范"本身就是该培养的能力,替它 inject 会让 agent 变懒;(3) breadcrumb 把 skill 名写死后几乎不可能漏。实测若仍漏加载,再给特定 skill 加 `auto_inject_on_state` flag(非全局开关)。

### 6.4 artifact 查阅(已定:两条机制)

1. **加载机制**:task.json 元数据 + `summary` → append `messages[0]`(**per-turn 注入,持久化不动**;小,~50 tokens)。prd/design/progress **全文不放**,只给 path,agent `read_file` 自取(避免大文件撑爆 context,复用 [`RECALL_TOKEN_BUDGET`](../.trellis/spec/backend/memory.md) 预算思维)。
2. **skill 查阅**:agent 通过 wf-* skill 知道"该读哪些 artifact";也通过通用 read_file 主动查。

### 6.5 sub-agent 三角色(plugin 专属,自带 agents/ 子目录)

**Q5 决定(2026-07-07)**:三角色**由 workflow plugin 自带**,定义在 plugin 目录的 `agents/` 下,不在全局 `.everlasting/agents/`。builtin `general-purpose` / `researcher` 不动。workflow session 里 **plugin agents 优先于全局**解析。

**目录结构**:

```
<project>/.everlasting/workflow/dev/
  ├── workflow.json          # state machine + roles_by_state
  └── agents/                # ← plugin 专属 sub-agent(随 plugin 可移植)
        ├── researcher.md
        ├── implementer.md
        └── checker.md
```

**角色定义**(plugin agents/*.md frontmatter):

| 角色 | tools | state |
|---|---|---|
| `researcher` | `[read_file,grep,glob,list_dir,web_fetch]` 只读 | planning |
| `implementer` | 全集(除 dispatch_subagent,防嵌套) | implement |
| `checker` | 只读 + `shell` | implement 每项后 / check |

**dispatch 解析顺序**(workflow session):

```
plugin agents (workflow/<current-plugin>/agents/*)
  → project 全局 (.everlasting/agents/*)
    → user 全局 (~/.config/everlasting/agents/*)
      → builtin (general-purpose / researcher)
```

plugin 目录有的角色用 plugin 的;没有的 fallback 到全局。用户想自定义 implementer → 在 plugin 目录覆盖 `agents/implementer.md`,或改它。非 workflow session 完全不看 plugin agents,走现有三层。

**为什么不让 workflow 复用全局 agents/**:plugin 自洽(state 定义 + 角色 + breadcrumb 一起可移植,复制 `dev/` 目录到别的项目即整套流程);不污染全局(workflow 专属角色不混进全局列表)。

**命名约束**:plugin 目录名、builtin 名一律**英文**(如 `dev`,非"开发流";`review`,非"评审流")。用户项目可自定义任意名,但 builtin/示例用英文。

### 6.6 worker 上下文注入(plugin delegation 模板)

**Q6 决定(2026-07-07)**:worker 的 delegation prompt 由 **两部分**拼成,来源不同:

| 部分 | 内容 | 来源 | 性质 |
|---|---|---|---|
| **A. task meta** | `{title}` / `{summary}` / `{state}` | task.json(engine 自动填) | 数据 |
| **B. delegation 模板** | "告诉 sub-agent 要做什么、不该做什么、验收标准..."的框架性指导 | workflow plugin 配置(`delegation_templates` 字段) | 模板 |

**机制**:workflow.json 加 `delegation_templates` 字段(每角色一个模板)。**主 LLM 读模板后,把本次具体委托内容写进 delegation task 描述**。engine 只负责把 task meta 占位符填进模板,不替主 LLM 写委托语。

**三层分工**:
- **模板**(plugin 定义)给框架——角色规范(调研范围/验收维度/约束)
- **主 LLM** 填细节——这次具体委托什么
- **task meta** 给上下文——task 是什么

```json
"delegation_templates": {
  "researcher":  "你正在为 task「{title}」调研(state={state})。{summary}\n调研范围: ...\n不要写代码。产出: ...",
  "implementer": "你正在为 task「{title}」实现一项(state={state})。{summary}\n约束: ...\n验收标准: ...",
  "checker":     "你正在为 task「{title}」验收(state={state})。{summary}\n验收维度: lint/typecheck/跨层一致性\n通过标准: ..."
}
```

**不用 `{current_item}` 占位符**:具体委托哪一项是**主 LLM 在 dispatch 时写进 delegation task 描述**的(本来就该主 LLM 写),不靠占位符机械填充。这把 delegation 模板跟 checklist 升格(B12 → task-scoped)**解耦**——模板只依赖 task.meta,不依赖 checklist 运行时状态,Q6 不阻塞 checklist 升格时机。

**prd/design 全文仍只给 path**:worker 需要细节时自己 read_file,按需不撑爆。delegation message 里是 task meta + 模板框架,不是 prd 全文。

**解决了 Trellis 的 jsonl 注入空洞**:Trellis 靠平台 hook 把 spec + prd + design 自动注入 sub-agent prompt;Everlasting 没有 hook,这里用"plugin 模板 + task meta + 主 LLM 填委托"达成等价效果——worker 进场就知道角色规范 + task 上下文,不再只收到 main agent 一段手写描述。

#### 6.6.1 模板注入机制(S1 评审修正 + S-B 前置约束 + M-E 时机明确 + M-B 占位符扩展)

**问题**:engine 填好 task meta 占位符后,模板文本如何到达 agent 上下文?

**决定**:走方案 (a)——engine 把填好的 delegation 模板 append 到 **per-turn request clone 的 `messages[0]` block 数组**(复用 [`inject_recall_into_turn`](../.trellis/spec/backend/memory.md) 同款 seam,`cache_control: None`)。

**注入时机(M-E 评审修正)**:**仅 dispatch turn 追加**。当 turn 内出现 `dispatch_subagent` tool_use 时,engine 在 `run_subagent` 内部拦截点(见 §6.6.2)解析目标 role → 取 `delegation_templates[role]` → 填占位符 → append messages[0]。**非 dispatch turn 不追加 delegation 模板**(只有 breadcrumb + task meta,见 §6.4)。这与 §10.7"per-turn 都 append,但内容因 turn 而异"一致:大多数 turn 只有 breadcrumb+meta,dispatch turn 多一块 delegation 模板。

**占位符全集(M-B 评审修正)**:`{title}` / `{summary}` / `{state}` / `{relevant_specs}`。前三个从 task.json 直接取;`{relevant_specs}` 由 engine 按 task.summary 做轻量 FTS5 过滤 `.everlasting/spec/` 索引,返回候选 spec 路径列表(无匹配则填 `(auto-detect via wf-before-dev)`)。这让 implementer 进场就知道该读哪些 spec,贯彻"不靠自觉"——否则 worker 只收到"实现 item X"却不知项目有哪些规范可读。

**前置约束(S-B 评审修正)**:本机制依赖 `messages[0]` 存在 user instruction message(workflow session 默认满足:B5 指令文件 CLAUDE.md/AGENTS.md 固定加载,`build_instructions_blocks` 产出非空 → messages[0] 是 user-role Blocks message)。`inject_recall_into_turn` 在 `messages[0]` 非 user instruction 时会 **prepend 新建 synthetic user message,破坏 prompt cache breakpoint**(memory_recall.rs:259-278 fallback 分支)——**本工作流不允许触发该 fallback**。实施时 `load_for_session` 返回空 layers 的 workflow session 属配置异常,engine 应 warn 并降级到非 workflow 行为(不注入 breadcrumb),不走 fallback prepend。

**为什么不靠 agent use_skill 自觉加载**(否决评审 S1 方案 b):delegation 模板是 dispatch 的**基础设施**(类似 system prompt 的角色规范),不是按需知识。靠 agent 自觉 = agent 可能漏加载 → worker 收到无框架的 delegation,角色化失效。engine 注入保证 worker 进场一定有角色规范。

**跟 Q4 不冲突**:Q4 决定不自动 inject **skill body**(wf-* 是按需知识,保 B4 三层渐进披露);delegation 模板是 dispatch 基础设施,性质不同,engine 注入。

#### 6.6.2 门控执行点(M3 + S-A 评审修正:门控下沉 run_subagent 内部)

**问题(S-A)**:`run_subagent` 在 `chat_loop.rs` 有 **3 处调用点**——串行 path(~3286)、L3b 并发 dispatch path(~2937,`FuturesUnordered` batch)、测试/内部显式派(~1000)。原 §6.6.2 只锁"chat_loop.rs dispatch 拦截处"= 仅串行 path,**并发 dispatch path 完全绕过门控** = 角色违规可经并发 path 逃逸。Q8 评审流 A 路径(synthesis_round fan-out)本质就用 L3b 并发,门控不覆盖 = 评审流一上就漏。

**决定(拍板 2 选 i)**:**门控下沉到 `run_subagent` 内部**(`subagent/dispatch.rs`),三处调用点都过。`run_subagent` 签名追加 `current_state: &WorkflowState` 参数(对标 Q2"扩展不是重构"——只增参不改结构);调用方传当前 task state。门控逻辑在 `run_subagent` 入口:

1. 查 `allowed_roles(def, current_state)` 是否含目标 role
2. **允许** → 注入 delegation 模板(§6.6.1,engine 在 dispatch.rs 内 append 调用方的 turn_messages)→ 继续原 dispatch 逻辑
3. **不允许** → 触发协商档(§6.1 Q3):engine 调 `ask_user_question` 问用户"这个 task 还在 {state},允许派 {role} 吗 / 确认推进 state 吗" → 用户允许则放行(一次性越权,不改 workflow.json)→ 拒绝则返 tool_error 提示回 breadcrumb

**为什么不三处各放一份 gate**(否决拍板 2 选 ii):三处复制 = 漏改一处就漏门控(S-A 正是此问题);下沉 `run_subagent` 内部是单一真相,新增调用点自动覆盖。

**`run_subagent` 签名变更**:`fn run_subagent(..., current_state: &WorkflowState)` —— 三个调用点(chat_loop.rs:1000/2937/3286)传同一 task state。这是 Q2"扩展不是重构"的体现:加参数不破坏现有调用语义。

### 6.7 沉淀闭环(长跑价值)

task done → agent(在 `wf-update-spec` 指导下)把新 pattern/convention/坑+修复/决策提炼写进 `.everlasting/spec/`(借鉴 `.trellis/spec/` 结构:`<package>/<layer>/index.md` + guideline 文件)。下次 implement → wf-before-dev → 读 spec → 按规范写 → 又沉淀。**闭环**。

> **Q7 决定(2026-07-07)**:**新目录 `.everlasting/spec/`**,不合并 `.trellis/spec/`。理由:职责不同——`.trellis/spec/` 管"怎么用 Trellis 开发"(Trellis 工具自己的规范);`.everlasting/spec/` 管"这个项目本身怎么写代码"(agent 跑 workflow 沉淀的代码规范/pattern/坑)。两者规范对象不同,合并会打架。借鉴结构(`<package>/<layer>/index.md` + guideline 文件)不借鉴位置。本项目"用 Trellis 开发 everlasting"期间 `.trellis/spec/` 已有 everlasting 代码规范,跟 `.everlasting/spec/` 性质重叠——这是过渡问题,接受两份共存(分阶段看各自),未来可迁移;现在分开保留迁移可能,比合并锁死好。沉淀不强制结构(agent 自由写 markdown),无效沉淀接受为代价。

### 6.8 task CLI 脚本(借鉴 Trellis task.py)

**目的**:task 的**创建(种模板)+ 归档(移动 + 状态)**走**脚本**,不走 LLM 手写 JSON。借鉴 [Trellis `task.py`](https://github.com/mindfold-ai/Trellis) 的 create/archive 子命令体验。

**为什么脚本而非纯 agent 写文件**:
- **task.json schema 稳定**:agent 手写 JSON 易出格式错(M6 评审小问题6 已指出);脚本用固定模板种,保证 schema 合法
- **归档体验一致**:done 时移动到 `archive/YYYY-MM/` + 写 status=completed + 可选 git commit——这套固定流程交给脚本比交给 agent 可靠(对标 Q9"不靠 agent 自觉")
- **LLM 按需编辑**:脚本只种**骨架**(task.json 空模板 + prd.md 大纲),内容(prd 正文 / items / progress)由 agent/用户按需编辑

**两个子命令**(放 `.everlasting/scripts/task.py` 或 Rust 内置 Tauri command):

```bash
# 创建 task:种 .everlasting/tasks/<slug>/task.json(空模板)+ prd.md(大纲)
python3 .everlasting/scripts/task.py create "<title>" --slug <slug>
#   → 生成 task.json: {id, title, slug, status:"planning", items:[], summary:"", ...}
#   → 生成 prd.md: "# <title>\n\n## 背景\n## 目标\n## 验收标准\n..."(大纲,agent 填)

# 归档 task:移动到 archive/ + status=completed + 可选 git commit
python3 .everlasting/scripts/task.py archive <slug> [--no-commit]
#   → 移动 .everlasting/tasks/<slug>/ → .everlasting/tasks/archive/<YYYY-MM>/<slug>/
#   → task.json.status = "completed", 补 completedAt
#   → (默认)git add + git commit "chore(task): archive <slug>"
```

**跟 engine 的衔接**:
- **创建**:§6.2"首次起 task 触发点"里,agent 判断要起 task 时,**调脚本 tool**(不是裸 write_file)。engine 提供 `create_task` Tauri command 包装脚本,agent 走 IPC 调用
- **归档**:§8 hook(Check→Done)触发后,engine 调 `archive_task` command(done 的归档不靠 agent,跟 M2 set_task_state 同源逻辑)
- **状态转移**:state 变更(planning→implement 等)走 `update_task` command(小问题6,S2 items 也走这个),不裸写 task.json

**Trellis 对标**:
| Trellis task.py | Everlasting 对应 | 差异 |
|---|---|---|
| `create <title> --slug` | `create_task` command | Everlasting 的 task.json 字段更少(无 assignee/priority/dev_type,Trellis 团队协作字段个人项目不需要) |
| `archive <name> [--no-commit]` | `archive_task` command | 同,默认 git commit |
| `start/finish/current` | Everlasting 不需要 | state 转移走确认门(M2),不走 `start` 命令;current task 是会话内状态不需要 `current` 命令 |
| `list [--status]` | 可选 follow-up | agent 用 `list_dir` 即可,UI 不展示 task(§3.2 无 task UI) |

**Phase 落地**:脚本/command 随 Phase 0(engine 骨架需要 create_task)+ Phase 3(归档随沉淀闭环)分两批落地。Phase 0 先 `create_task`,Phase 3 加 `archive_task`。

---
