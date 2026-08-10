## 12. 与现有体系的接入点(实施时接缝清单)

| 接入点 | 现有位置 | 本功能改动 |
|---|---|---|
| workflow 开关 | `sessions` 表 + 顶栏 | 加 `workflow_enabled` 列 + toggle(B7 mode 切换同级) |
| state breadcrumb 注入 | [`memory_recall::inject_recall_into_turn`](../../.trellis/spec/backend/agent-loop-architecture.md)(append messages[0]) | 复用 seam,加 workflow breadcrumb block |
| plugin 配置加载 | `resource_loader.rs`(B3)/ `skill/loader.rs`(B4)模式 | 加 workflow loader(同 mtime fence),读 `.everlasting/workflow/*/` |
| task.json 加载 | `resource_loader.rs` / `subagent/loader.rs` 模式 | 加 task loader,读 `.everlasting/tasks/*/task.json` |
| task artifact 读写 | `read_file`(读) + 专用 task tool(写) | 读零改动;**写用专用 tool 非裸 write_file**(小问题6):裸 write_file 写 task.json 可能产出损坏 JSON(agent 手写) + 绕过 items coerce。加 `update_task` tool(只允许改特定字段:status/items/summary/progress,内部 serde 序列化保证 JSON 合法 + items coerce 复用 B12 逻辑) |
| task state 转移 | (新) | `set_task_state` 由确认门 resolve handler 自动调(M2,§8),非 agent tool |
| 内置 skill | B4 `skill/loader.rs` | wf-* skill 随 plugin 分发(放 `.everlasting/workflow/dev/skills/`),非全局 builtin;loader 加 plugin skills 解析层 |
| sub-agent 角色 | `builtin_subagents()`([`subagent/mod.rs:463`](../../.trellis/spec/backend/agent-loop-architecture.md)) | plugin 自带 agents/(Q5);workflow session 里 plugin agents 优先于全局,builtin general-purpose/researcher 不动作 fallback |
| state 门控 dispatch | `subagent/dispatch.rs::run_subagent` 内部(S-A 下沉) | 统一协商档(Q3+S3);三处调用点(串行~3286/并发~2937/测试~1000)都过;签名追加 `current_state` 参数 |
| checklist 升格(S2) | `tools/update_checklist.rs`(B12 loop-local) | workflow session 内改写 task.json.items(非 loop-local Vec);B12 coerce 保留;非 workflow session 行为不变 |
| state 转移 hook | (无现有 hook runner) | Phase 3 新建 Rust 固定逻辑(Q9 选 b) |
| spec 沉淀 | (无) | `.everlasting/spec/` 新目录 + wf-update-spec skill |
| 用户确认门 | `ask_user_question` tool | 零改动,复用 |
| UI plugin 切换 | B7 mode 切换同级 | 加 workflow plugin 选择器 |

---

## 13. 后续维护承诺

- **本文件改动时机**:
  - Phase 0-3 任一落地 → 对应节标 ✅ + commit hash
  - §14 Q1-Q9 任一改判 → 改本文 + [IMPLEMENTATION §4](../IMPLEMENTATION/decisions.md) 追 ADR
  - 评审流(§7)立项 → 展开 §7 为独立设计节
  - 跟 B8 边界重划 → 改 §3.2 + ROADMAP §2 第四档
- **不做的边界**:不列 commit/PR(走 git log);不做技术细节(走各 phase PRD + `.trellis/spec/backend/`);不做决策追溯(走 IMPLEMENTATION/decisions.md)
- **ROADMAP 接入**:稳定后在 [ROADMAP §2 第三档](../ROADMAP.md) 加 "W1 Workflow 集成(工作流引擎 + plugin)" 条目

---

## 14. 待对齐汇总

| # | 问题 | 倾向 |
|---|---|---|
| ~~Q1~~ | ~~plugin 配置格式~~ | ✅ **JSON**(`workflow.json`)— 流程把控精准,schema 严,serde_json 解析零自写 |
| ~~Q2~~ | ~~Phase 0-1 先硬编码默认还是直接配置驱动?~~ | ✅ **Phase 0 预留 plugin 接口**(WorkflowDef struct + 三访问函数,engine 只认接口);数据源先硬编码常量,Phase 2 换读 workflow.json。后续变更是扩展不是重构 |
| ~~Q3~~ | ~~state 枚举加 blocked 吗?~~ | ✅ **不加**。补充(S3 后统一):门控违反 → **engine 主动 ask_user_question**(统一协商档,不分角色/state,见 §5.2.3/§6.1/§6.6.2;非权限 Mode)。用户允许则放行/推进 state,拒绝则回 breadcrumb。无硬拒档 |
| ~~Q4~~ | ~~state 转移自动 inject skill body 吗?~~ | ✅ **不自动**。靠 breadcrumb 写死 skill 名 + agent 自觉 use_skill;保 B4 三层渐进披露。漏加载再考虑 per-skill `auto_inject_on_state` flag |
| ~~Q5~~ | ~~implementer 改名还是新增?~~ | ✅ **workflow plugin 自带 `agents/` 子目录**定义三角色(researcher/implementer/checker);plugin agents 在 workflow session 优先于全局;builtin general-purpose 不动。命名:plugin 目录/builtin 一律英文(`dev`/`review`,非中文) |
| ~~Q6~~ | ~~worker 上下文注入深度?~~ | ✅ **plugin delegation 模板**:task meta(`{title}`/`{summary}`/`{state}`,engine 填)+ delegation 模板(plugin 定义,指导主 LLM 写委托语)。不用 `{current_item}`(主 LLM 写委托细节)。prd 全文给 path。跟 checklist 升格解耦 |
| ~~Q7~~ | ~~spec 沉淀进 .everlasting/spec/ 还是合并 .trellis/spec/?~~ | ✅ **新目录 `.everlasting/spec/`**(职责独立于 .trellis/spec/;借鉴结构不借鉴位置;过渡期两份共存) |
| Q8(延迟) | 评审流走回合制 A 还是实时群聊 B,何时立项? | 延迟讨论。**立项 trigger**:dev plugin 跑通 ≥1 完整 task + 沉淀 spec 起见过收益后再决 |
| ~~Q9~~ | ~~hook 选 (a)纯skill/(b)Rust固定/(c)runner?~~ | ✅ **(b) Rust 固定逻辑**(嵌在 `set_task_state` 写入路径;沉淀闭环是机制保证不靠 agent 自觉;不做脚本 runner 避免安全面)。Phase 3 |

---

## 15. 附录 §A:dev plugin 完整示例

> 本附录是 Phase 0-1 实施的参考物料。按 [§14 Q2 决定](#14-待对齐汇总):workflow.json 完整填 + wf-overview 完整 body + 三角色 frontmatter + 其他 skill outline。复制 `.everlasting/workflow/dev/` 目录即整套流程。
>
> 注:这是**草稿样板**,实施时按项目实际调整。breadcrumb 文本 / delegation 模板 / skill body 都是建议,非强制。

### A.1 目录结构

```
<project>/.everlasting/workflow/dev/
  ├── workflow.json
  ├── agents/
  │     ├── researcher.md
  │     ├── implementer.md
  │     └── checker.md
  └── skills/
        ├── wf-overview/SKILL.md
        ├── wf-brainstorm/SKILL.md
        ├── wf-before-dev/SKILL.md
        ├── wf-check/SKILL.md
        └── wf-update-spec/SKILL.md
```

> skill 放 plugin 目录的 `skills/` 下(对标 Q5:plugin 自带 agents/,skill 同理自带)。dispatch 解析顺序:plugin skills → project 全局 → user 全局 → builtin(skill 无 builtin fallback 则报 skill not found)。

### A.2 workflow.json(完整填)

```json
{
  "name": "dev",
  "description": "标准开发流:调研 → 实施 → 验收 → 沉淀。把'agent 聊天'变成'agent 按标准流程开发',长跑沉淀项目 spec 规范。",
  "states": ["planning", "implement", "check", "done"],
  "initial": "planning",
  "transitions": [
    { "from": "planning",  "to": "implement", "requires_user_confirm": true },
    { "from": "implement", "to": "check",     "requires_user_confirm": true },
    { "from": "check",     "to": "done",      "requires_user_confirm": true }
  ],
  "roles_by_state": {
    "planning":  ["researcher"],
    "implement": ["implementer", "checker"],
    "check":     ["checker"],
    "done":      []
  },
  "breadcrumb": {
    "planning":  "[dev workflow · planning] 你在 planning 阶段。\n- 先 use_skill wf-overview 了解整个 workflow(若还没读)\n- dispatch researcher 调研技术方案、踩坑点、相关 spec\n- 产出 .everlasting/tasks/<slug>/prd.md(需求) + 拆 task.json.items(实施阶段,如:后端实施→后端测试→前端实施→联调)\n- 不要写实现代码(planning 只允许 researcher 角色)\n- 完成后用 ask_user_question 问用户确认进入 implement",
    "implement": "[dev workflow · implement] 你在 implement 阶段。\n- 先 use_skill wf-before-dev 加载项目 spec 规范\n- 按 task.json.items 逐项推进;每项 dispatch implementer 实现\n- implementer 完成后 dispatch checker 验收该项\n- tdd 标记的 item 强制 red(写失败测试)→ implement → green(测试通过)\n- 全部 item done 后用 ask_user_question 问用户确认进入 check",
    "check":     "[dev workflow · check] 你在 check 阶段。\n- dispatch checker 做最终全量验收:lint / typecheck / 跨层一致性 / 端到端\n- 发现问题则回 implement 修(用 ask_user_question 协商切回)\n- 通过后用 ask_user_question 问用户确认进入 done",
    "done":      "[dev workflow · done] task 完成。\n- use_skill wf-update-spec 把本次决策/坑/新 pattern 提炼进 .everlasting/spec/\n- 更新 progress.md 写交接叙述\n- 归档 task(移动到 .everlasting/tasks/archive/)"
  },
  "delegation_templates": {
    "researcher":  "你正在为 task「{title}」做调研(state={state})。\nTask 摘要: {summary}\n相关 spec: {relevant_specs}\n\n你的角色是 researcher(只读,不能写代码)。请:\n- 调研 [主 LLM 在此填本次委托的具体调研方向]\n- 读 .everlasting/tasks/<slug>/prd.md 了解需求(用 read_file)\n- 产出调研结论(技术方案/踩坑点/相关 spec 引用)\n- 不要修改任何文件\n\n验收:产出结构化调研报告,main agent 据此写 prd/checklist。",
    "implementer": "你正在为 task「{title}」实现一项(state={state})。\nTask 摘要: {summary}\n相关 spec(必读): {relevant_specs}\n\n你的角色是 implementer。请:\n- 实现 [主 LLM 在此填本次委托的具体 item]\n- 读 .everlasting/tasks/<slug>/prd.md 和 design.md 了解上下文(用 read_file)\n- 遵守项目 spec 规范(若未加载,先 use_skill wf-before-dev)\n- 不要 dispatch_subagent(防嵌套)\n\n验收:[主 LLM 在此填该项验收标准]。",
    "checker":     "你正在为 task「{title}」验收(state={state})。\nTask 摘要: {summary}\n相关 spec: {relevant_specs}\n\n你的角色是 checker(只读 + 可跑 shell 测试)。请:\n- 验收 [主 LLM 在此填本次委托的验收对象]\n- 跑 lint / typecheck / 相关测试(用 shell tool)\n- 检查跨层一致性 / spec 合规\n- 不要修改文件,只产出验收报告(通过/不通过 + 具体问题)\n\n验收维度:lint、typecheck、测试通过率、跨层一致性、spec 合规。"
  },
  "coordination": "pipeline",
  "gather_strategy": {}
}
```

### A.3 三角色 agents/*.md(frontmatter 完整 + body 提纲)

#### agents/researcher.md

```markdown
---
name: researcher
description: 只读调研 agent,用于 planning 阶段技术方案调研、踩坑点排查、相关 spec 检索
tools: [read_file, grep, glob, list_dir, web_fetch]
---

# Researcher

你是只读调研 agent,在 dev workflow 的 planning 阶段被派发。

## 你的职责
- 调研技术方案、对比选项、识别风险
- 检索 `.everlasting/spec/` 找相关规范
- 读 prd/design 理解需求,产出结构化调研报告

## 约束
- **只读**:不能 write_file / edit / shell(写)
- 不做实现,不写代码
- 产出是文字报告(技术方案/踩坑点/建议),不是代码

## 产出格式
- 现状 / 选项对比 / 推荐方案 / 风险 / 相关 spec 引用
```

#### agents/implementer.md

```markdown
---
name: implementer
description: 实施 agent,用于 implement 阶段按 checklist item 实现代码
tools: []   # 空数组 = 全集(同 general-purpose 语义);engine 在 run_subagent 内部自动剥离 dispatch_subagent(防嵌套,见 §6.6.2 S-A),实际可用 = 全集减 STRUCTURALLY_DISABLED
# M-D 评审 follow-up(不阻塞 Phase 0):implementer 与 general-purpose 同 toolset,仅靠 system_prompt 区分角色——
# 违背"角色 = tools + body"。未来可加 SubagentDef.tools_negation: Vec<String> 字段(空白名单 + 减号表达"全集去掉 X"),
# 或 frontmatter 列显式白名单(全部 builtin 工具名,但脆:工具增减要改)。Phase 0 暂留此注释,实测角色混淆再改 L3d。
---

# Implementer

你是实施 agent,在 dev workflow 的 implement 阶段被派发。

## 你的职责
- 按 main agent 委托的具体 item 实现代码
- 读 prd/design 了解上下文
- 遵守项目 spec 规范(先 use_skill wf-before-dev)

## 约束
- 不要 dispatch_subagent(防嵌套,engine 已剥离该 tool)
- 实现完一项就返回,不擅自扩大范围
- 遵守 tdd item 的 red→green 流程(若该项标了 tdd)

## 产出
- 修改的文件 + 完成哪一项 checklist item
- 自测结果(跑了什么测试,通过情况)
```

#### agents/checker.md

```markdown
---
name: checker
description: 验收 agent,只读 + 可跑测试,用于 implement 每项后 / check 阶段全量验收
tools: [read_file, grep, glob, list_dir, shell]
# 小问题7 follow-up:shell 命令前缀白名单(可选字段,engine 在 §6.6.2 dispatch 校验时叠加一层)
# 当前 frontmatter 不强制,Phase 2 实测 checker 跑危险命令再考虑加 shell_allow_prefixes: ["cargo ", "pnpm ", "pytest"]
---

# Checker

你是验收 agent,在 implement(每项后)和 check(全量)阶段被派发。

## 你的职责
- 验收 implementer 的产出:lint / typecheck / 跨层一致性 / spec 合规
- 跑测试(shell tool),报告通过率
- 检查是否遵守 `.everlasting/spec/` 规范

## 约束
- **只读代码**:不能 write_file / edit_file(只跑测试、读代码)
- **shell 只跑 lint/test 命令**(cargo clippy / cargo test / pnpm test 等),不修改源码文件、不跑写类命令(rm/cp/mv/sed -i/git commit 等)
- 不做实现,不写代码
- 产出是验收报告(通过/不通过 + 具体问题列表)

## 产出格式
- 通过/不通过
- 不通过项:具体问题(文件:行 + 描述 + 建议修法)
- 验收维度覆盖表:lint ✓/✗、typecheck ✓/✗、测试 ✓/✗、跨层 ✓/✗、spec ✓/✗
```

### A.4 skills/wf-overview/SKILL.md(完整 body)

```markdown
---
name: wf-overview
description: dev workflow 全貌说明。进 workflow 时加载,建立"我在哪个 workflow、整体怎么干"的全局意识
allowed-tools: []
---

# Dev Workflow 全貌

本 skill 是 `dev` workflow(标准开发流)的完整说明。当你处于 workflow session 且 breadcrumb 显示 dev workflow 时,读这个 skill 理解整个流程。

## 整体流程(4 个 state,线性推进)

```
planning → implement → check → done
  调研        实施       验收      沉淀
```

每个 state 转移都需要**用户确认**(你用 ask_user_question 申请,用户同意才推进)。

## 各 state 该做什么

### planning(调研 + 写 prd + 拆 checklist)
- 只能 dispatch **researcher** 角色(只读调研)
- 产出 `.everlasting/tasks/<slug>/prd.md`(需求文档)+ 拆 task.json.items(实施阶段,见 §6.2)
- **不要写实现代码**
- 完成后 ask_user_question 申请切 implement

### implement(按 checklist 实施)
- 先 `use_skill wf-before-dev` 加载项目 spec 规范
- 按 task.json.items 逐项 dispatch **implementer** 实现
- 每项完成后 dispatch **checker** 验收该项
- tdd item 走 red(写失败测试)→ implement → green(测试通过)
- 全部 item done 后 ask_user_question 申请切 check

### check(全量验收)
- dispatch **checker** 做最终全量验收(lint/typecheck/跨层/端到端)
- 有问题 → ask_user_question 协商切回 implement 修
- 通过后 ask_user_question 申请切 done

### done(沉淀 + 归档)
- `use_skill wf-update-spec` 把决策/坑/新 pattern 提炼进 `.everlasting/spec/`
- 更新 `progress.md` 写交接叙述
- 归档 task(移到 `.everlasting/tasks/archive/`)
- (done 的沉淀由 Rust 固定 hook 触发,保证不漏)

## 三个角色(sub-agent)

| 角色 | 用在 | 能力 |
|---|---|---|
| researcher | planning | 只读(read/grep/glob/web_fetch),产出调研报告 |
| implementer | implement | 全集 tools(除 dispatch_subagent),写代码实现 |
| checker | implement 每项后 / check | 只读 + shell 跑测试,产出验收报告 |

dispatch 时你会用到 delegation 模板(plugin 配置),模板告诉你"告诉 sub-agent 要做什么、不该做什么、验收标准"。你填本次委托的具体内容。

## task 文件态(你的记账)

每个 task 在 `.everlasting/tasks/<slug>/`:

| 文件 | 何时写 | 内容 |
|---|---|---|
| task.json | 起 task 时 | 元数据(id/title/slug/status/summary) |
| prd.md | planning | 需求文档 |
| task.json.items | planning | 实施阶段拆分(内嵌 task.json,见 §6.2 S2;LLM 拆,如 后端→测试→前端→联调) |
| progress.md | state 转移时 | 交接叙述(下次 session 续 task 时读它) |
| design.md | planning(复杂 task) | 技术设计(可选) |

**task 跨 session**:你(或下次 session 的 agent)读 `.everlasting/tasks/` 找 status≠done 的 task,读 progress.md 续上。

## 门控:违反流程时怎么办

若你想做当前 state 不允许的事(如 planning 想写代码),不要硬闯:
- 用 ask_user_question 跟用户协商:"这个 task 还在 planning,确认进 implement 吗?"
- 用户同意 → 推进 state 继续
- 用户拒绝 → 回 breadcrumb 提示,继续当前 state 该做的事

这是**协商档**(所有门控违规统一走这个,不存在硬拒档,见 §6.1 Q3 + S3)——流程有默认,但允许例外(用户背书)。

## 闭环价值

长跑下来,你每次 done 都沉淀 spec → 下次 implement 读 spec → 按规范写 → 又沉淀。**这是 dev workflow 的核心价值:让 AI 每次都按标准写代码,而不是随机发挥。**

## 何时 use_skill 哪个

| skill | 何时 |
|---|---|
| wf-overview(本 skill) | 进 workflow 时,或忘了流程时自查 |
| wf-brainstorm | planning(调研方法) |
| wf-before-dev | implement 入口(加载 spec) |
| wf-check | implement 每项后 / check(验收方法) |
| wf-update-spec | done(沉淀 spec) |
```

### A.5 其他 wf-* skill body outline

> 这几个借鉴 [Trellis 同名 skill](https://github.com/mindfold-ai/Trellis),去掉平台 hook 依赖,纯描述性。实施时按项目实际填 body,以下给 outline。

#### skills/wf-brainstorm/SKILL.md(借鉴 trellis-brainstorm)

```markdown
---
name: wf-brainstorm
description: planning 阶段调研 + 写 prd + 拆 checklist 的方法指导
allowed-tools: []
---

# 调研与需求拆解(planning)

## 调研方法
- 先 dispatch researcher 调研技术方案、踩坑点、相关 spec
- 调研要覆盖:现有实现、备选方案、风险、相关 `.everlasting/spec/` 规范

## 写 prd.md
- 背景 / 目标 / 非目标 / 技术方案 / 验收标准 / 风险
- prd 是给后续 implement 和 check 看的,要可执行(明确"做完什么样算 done")

## 拆 task.json.items(实施阶段)
- 按 task 复杂度拆,**不要预定义模板**(每个 task 拆法不同)
- 例:后端实施 → 后端测试 → 前端实施 → 前端测试 → 联调 → 端到端
- 每项标是否 tdd(逻辑改动标 tdd,文档/配置/重命名不标)
- 拆完用 ask_user_question 跟用户对齐,再申请切 implement
```

#### skills/wf-before-dev/SKILL.md(借鉴 trellis-before-dev)

```markdown
---
name: wf-before-dev
description: implement 入口加载项目 spec 规范,确保按标准写代码
allowed-tools: []
---

# 写代码前加载 spec(implement)

## 必读
- `list_dir .everlasting/spec/` 看有哪些规范
- read_file 跟本次 task 相关的 spec(按包/层)
- 对照 spec 检查:命名 / 数据流 / 错误处理 / 测试约定

## 委托 implementer 前
- 把相关 spec 引用塞进 delegation message(让 implementer 也读)
- delegation 模板里有 `{summary}`,你填时带上"遵守 .everlasting/spec/xxx"

## 若 spec 缺失
- 按现有代码风格推断(读相邻文件)
- 本次 task done 时通过 wf-update-spec 把新发现的规范沉淀
```

#### skills/wf-check/SKILL.md(借鉴 trellis-check)

```markdown
---
name: wf-check
description: 验收方法:lint/typecheck/测试/跨层一致性/spec 合规
allowed-tools: []
---

# 验收方法(implement 每项后 / check)

## 验收维度
- lint(cargo clippy / eslint 等,按项目)
- typecheck(cargo check / tsc)
- 测试(相关单测 + 集成测试)
- 跨层一致性(前后端数据流、Rust↔TS interface、wire shape)
- spec 合规(对照 `.everlasting/spec/`)

## 委托 checker
- delegation 填本次验收对象(哪一项 / 全量)
- checker 只读 + shell,产出验收报告

## 不通过怎么办
- 报告具体问题(文件:行 + 描述 + 建议修法)
- 回 implement 修(ask_user_question 协商,或继续 implement state 内修)
```

#### skills/wf-update-spec/SKILL.md(借鉴 trellis-update-spec)

```markdown
---
name: wf-update-spec
description: task done 时把决策/坑/新 pattern 提炼进 .everlasting/spec/
allowed-tools: []
---

# 沉淀 spec(done)

## 何时沉淀
- task done 时(Rust 固定 hook 会触发你加载本 skill)
- 只沉淀**可复用**的:新 pattern / convention / 踩坑 + 修复 / 技术决策
- 不沉淀 task 一次性细节(那个进 progress.md)

## 沉淀到哪
- `.everlasting/spec/<package>/<layer>/index.md` + 具体 guideline 文件
- 借鉴 `.trellis/spec/` 结构,但物理独立(见 Q7)

## 格式
- 自由 markdown,不强制结构
- 标题 + 场景 + 规范 + 反例(若有)

## 质量
- 不强求每次都沉淀(没东西可沉淀就不写)
- 无效沉淀(空/重复)接受为代价——长跑后有用的会被引用,无用的沉底
```

### A.6 示例 task 目录

> 一个填好的 task 示例,展示文件态记账的实际样子。

```
<project>/.everlasting/tasks/add-task-export/
  ├── task.json          # 元数据 + checklist items(内嵌,见 S2)
  ├── prd.md
  ├── progress.md
  └── design.md
```

#### task.json(填好,含 items)

```json
{
  "id": "01HZX...",
  "title": "Task 列表导出为 markdown",
  "slug": "add-task-export",
  "status": "implement",
  "created_at": "2026-07-07T10:00:00Z",
  "updated_at": "2026-07-07T12:30:00Z",
  "parent": null,
  "summary": "把 .everlasting/tasks/ 下的 task 导出为单个 markdown 文件,含元数据 + prd + checklist 状态,方便分享和归档",
  "items": [
    { "id": "backend-impl",  "content": "后端实施:export_tasks Tauri command", "status": "done" },
    { "id": "backend-test",  "content": "后端测试:export 集成测试",            "status": "done" },
    { "id": "frontend-impl", "content": "前端实施:export command + UI 入口",   "status": "in_progress", "tdd": true },
    { "id": "frontend-test", "content": "前端测试:导出按钮交互测试",           "status": "pending",    "tdd": true },
    { "id": "integration",   "content": "联调:端到端跑一遍导出流程",           "status": "pending" },
    { "id": "e2e-test",      "content": "端到端测试",                           "status": "pending" }
  ]
}
```

#### prd.md(片段)

```markdown
# Task 列表导出为 markdown

## 背景
用户想把多个 task 的产出汇总成一份 markdown 分享/归档。当前 task 是分散文件,无汇总机制。

## 目标
- 提供 `export_tasks` 命令,把指定 task(或全部)导出为单个 markdown
- 导出含:task.json 元数据 + prd 全文 + checklist 状态 + progress 摘要

## 非目标
- 不做导出为 HTML/PDF(留 follow-up)
- 不做云端分享(本地优先)

## 验收标准
- 命令存在且可调用
- 导出 markdown 结构清晰,可读
- 测试覆盖(至少 1 个集成测试)

## 技术方案
- 后端 Rust 加 Tauri command `export_tasks(task_slugs: Vec<String>) -> String`
- 前端 B3 command `export` 调用
...
```

#### progress.md(state 转移时更新)

```markdown
# Progress: Task 列表导出

## 2026-07-07 planning→implement
- 调研完成:现有 task 文件结构清晰,导出本质是读 + 拼 markdown
- prd 写完,验收标准明确
- checklist 拆为 后端→测试→前端→测试→联调→端到端
- 前端两项标 tdd(逻辑改动)

## 2026-07-07 implement 进行中
- 后端 command + 测试 done
- 进行前端实施
- 踩坑:Tauri command 参数序列化要注意 Vec<String>(见 spec-diff 草稿)
```

---

> 本文档随实施演进。任一 phase 开工前,先改本文(确认 scope/接缝/完成标准 + 对齐 Q1-Q9),再写该 phase PRD。
