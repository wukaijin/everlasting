# WORKFLOW-INTEGRATION — Workflow 集成需求设计

> **一句话**:给 Everlasting 加一个 **session 级 opt-in 的"工作流引擎(workflow engine)"**。打开后,agent 不再随机发挥,而是按一个**可切换的 workflow plugin**(状态机 + skill + sub-agent + 沉淀闭环)驱动的标准流程干活;长跑下来自然沉淀出项目 spec 规范。
>
> **架构核心:engine 与 content 分离。** Rust 后端只提供 engine(注入 seam、门控、state 转移、UI 切换);workflow 的"内容"——state 枚举、breadcrumb 文本、角色映射、协调模型——是 **plugin**,落在 `.everlasting/` 文件态,项目可改可换。一个 Rust engine 承载多个结构不同的 workflow plugin。
>
> **MVP plugin = `dev`(开发流)**(planning→implement→check→done,调研/实施/验收角色分工)。**愿景 plugin = `review`(评审流)**(创建需求→多 sub-agent 多轮评审→用户介入→收敛,可能需要新通讯架构,延迟讨论)。
>
> **主角是"机制",不是 task。** task 是 plugin 运转时 agent 自动产出的文件态记账副产物(`.everlasting/tasks/<slug>/`),用户**不感知、不操作** task 实体。用户唯一的操作:开 session、开 workflow、**选/切 workflow plugin**、说话。
>
> **UI 表现 = workflow 可切换**(注意:这跟被否决的"task picker"是两回事——选 workflow 是选"怎么干活",不是选"干哪个 task")。
>
> **参考实现**:本项目用 [Trellis](https://github.com/mindfold-ai/Trellis) 管理开发,其整套 task 元数据 + state machine + skill + sub-agent + spec 沉淀架构都是借鉴对象。Trellis 本身就是"内容文件态可改"的 plugin 化设计——借鉴它 = 借鉴其可定制性。
>
> 需求边界见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),路线图归 [ROADMAP.md](./ROADMAP.md),决策追溯走 [IMPLEMENTATION.md §4](./IMPLEMENTATION.md#4-决策日志)。

---

## 1. 文档目的

跟 [DESIGN.md](./DESIGN.md) 一样,这是**给自己看的工程决策备忘录**。用来:

- 在写 workflow 代码前,把"engine 由什么组成 / plugin 由什么组成 / 两者怎么分 / 怎么沉淀 spec"想清楚
- 记录关键岔路选/不选的理由(为什么 engine/content 分离、为什么不建 task UI、为什么 hook 用固定逻辑而非脚本、为什么评审流可能需要新通讯原语)
- 给后续每个实施阶段的 PRD 提供单一引用源

讨论中产生的关键决策沉淀到 [IMPLEMENTATION.md §4](./IMPLEMENTATION.md#4-决策日志);本文档不记决策追溯。

> ❓ *待对齐*:凡不能从已确认方向推导、需你拍板的点,用 ❓ 标出。文末 §14 汇总。

---

## 2. 背景与动机

### 2.1 现状:零件齐了,缺"机制"把它们串成标准流程

[ROADMAP §1.2](./ROADMAP.md#12-路线图外完成) 显示工作流的 ~80% 零件已落地,但它们目前是**被 agent 随机使用**的——agent 想起就 dispatch_subagent、想不起就自己一把梭。缺的是一个**机制**:在 session 打开 workflow 后,自动驱动 agent 按固定标准使用这些零件。

| engine 能力 | 已有零件(可复用) | 缺口(机制) |
|---|---|---|
> 注:本表引用 `.trellis/spec/` 路径是过渡期(本项目用 Trellis 开发,现有 spec 在那)。未来 `.everlasting/spec/`(Q7)沉淀起来后,新 spec 进 `.everlasting/spec/`,现有引用逐步迁移;两目录共存期(见 Q7)引用 `.trellis/spec/` 有效。
| breadcrumb 注入 seam | [`build_instructions_blocks`](../.trellis/spec/backend/agent-loop-architecture.md) + [`memory_recall::inject_recall_into_turn`](../.trellis/spec/backend/memory.md)(append 到 messages[0],`cache_control: None`) | engine 读 plugin 配置 → 拼 breadcrumb → 注入 |
| skill 规范 | [B4 skill](./ROADMAP.md)(`use_skill` + `.everlasting/skills/` 三层覆盖) | wf-* skill 包(plugin 自带 `.everlasting/workflow/dev/skills/`,见 §6.3) |
| sub-agent 角色 | [B6 subagent](./ROADMAP.md) + [L3d loader](./ROADMAP.md)(`builtin_subagents()` 已有 researcher / general-purpose) | checker 角色 + state→角色映射(plugin 配置) |
| 沉淀 | [V2 2 期自主记忆](./ROADMAP.md) + B5 指令文件 | task done 时把决策/教训升级进 `.everlasting/spec/` |
| 协调模型 | L3b 并发 dispatch(`FuturesUnordered` + per-worker worktree) | (评审流用,见 §7) |

### 2.2 为什么必须 plugin 化(不只是"能")

回到最初诉求:"自动化机制注入**项目执行规范**,让 AI 每次都按标准写代码"。**"项目执行规范"是项目自己的规范,不是 Everlasting 强加的**。若把 state 枚举、角色分工、沉淀目标写死在 Rust,那注入的是"Everlasting 官方工作流",所有项目一个样——违背"适配项目"初衷。

更要命的是:**不同任务需要结构不同的工作流**。本文档定义的两个 workflow plugin 形状根本不同(见 §4):

| 维度 | dev / 开发流(MVP) | review / 评审流(愿景) |
|---|---|---|
| state 形状 | planning→implement→check→done(线性) | create→clarify→draft→**多轮评审**→finalize(迭代) |
| 协调模型 | orchestrator + pipeline(角色依次/并发派) | orchestrator + **轮次 fan-out/gather** 或 peer 群聊 |
| 角色集 | researcher/implementer/checker | reviewer(可能多视角:架构/安全/性能) |
| 用户介入 | state 转移确认门 | 评审轮次之间、打破僵局 |

**若 engine 把 `dev` 的 state 枚举和 pipeline 写死,评审流就塞不进去。** 只有当 engine 是"读配置驱动的 state machine + 支持多种协调模型"时,两个流程才能作为两个 plugin 共存。plugin 化的真实价值 = 让根本不同的工作流共享同一个 engine。

### 2.3 借鉴 Trellis = 借鉴其可定制性

Trellis 的 `workflow.md` / `spec/` / `skills/` / `agents/` 全是文件态可改;fork 它、改 `.trellis/`,就得到一套定制工作流。借鉴 Trellis 不只是借目录结构,是借这套"内容文件态、引擎中立"的 plugin 化设计。Trellis 已在本项目验证了这套机制跑得通。

---

## 3. 项目能力边界(本功能)

### 3.1 做(engine + 第一个 plugin 范围内)

1. **workflow engine**(Rust 固定):session 级 workflow 开关 + 注入 seam + state 门控 + state 转移 + plugin 加载/切换
2. **第一个 plugin `dev`(开发流)**(MVP):planning/implement/check/done 四态 + researcher/implementer/checker 三角色 + 沉淀闭环
3. **plugin 内容文件态**:`.everlasting/workflow/<name>/`(state 定义 + breadcrumb 模板 + 角色映射);项目可改,覆盖 builtin 默认
4. **task 文件态记账**:agent 自动产 `.everlasting/tasks/<slug>/`;**无 task DB 表、无 task UI、用户不操作 task**
5. **wf-* skill 包**:作为 plugin 默认内容,走 B4 三层覆盖
6. **沉淀闭环**:task done 时把决策/教训升级进 `.everlasting/spec/`
7. **任务交接**:task 跨 session 靠文件位置天然达成
8. **UI workflow 切换**:session 开 workflow 时选 plugin;会话中可切

### 3.2 不做(硬约束)

| 不做 | 原因 |
|---|---|
| ❌ **任何 task UI**(picker/list/board/切换器) | task 是 agent 记账副产物,用户不操作 |
| ❌ **task DB 表 / session↔task 关联** | task 纯文件态;session 不绑 task |
| ❌ **B8 DAG 拓扑** | 留 B8(第四档);engine 不做 task 间编排 |
| ❌ **强制全局 workflow** | opt-in;不开 = 现有行为零改动 |
| ❌ **强制全局 TDD** | per-checklist-item opt-in |
| ❌ **workflow 绑定 Mode** | Mode 是权限旋钮,workflow 是流程旋钮,正交;state 不替用户切 Mode |
| ❌ **新建 session 强制选 task** | session 创建不被 task 打断 |
| ❌ **重写 agent core** | 复用 [`run_chat_loop`](../.trellis/spec/backend/agent-loop-architecture.md) 26 参;走注入 seam + tool 层 |
| ❌ **第二个 plugin `review`(评审流)+ 新通讯架构**(本阶段) | 作为愿景写入 §7,延迟讨论 |

---

## 4. 愿景:两个 workflow plugin

### 4.1 第一个 plugin `dev`(开发流)—— MVP

把"agent 聊天"变成"agent 遵循标准开发流程"。典型生命周期:

```
用户在 workflow session 里说"我要做 X"
        ↓
[agent 自动起 task]                      ← .everlasting/tasks/<slug>/task.json + 空 prd
   state = planning                       ← breadcrumb:"你在 planning,先调研+写 prd"
[skill: wf-brainstorm]                    ← agent 加载调研规范
[sub-agent: researcher ×N]                ← state 门控:planning 只派 researcher
        ↓ 产出 prd.md + checklist items(task.json,实施阶段拆分)
[用户确认门] planning→implement
        ↓
   state = implement                      ← breadcrumb:"按 checklist 派 implementer,每项 TDD"
[skill: wf-before-dev]                    ← agent 加载写代码规范
[sub-agent: implementer]                  ← state 门控
[sub-agent: checker]                      ← 每项后派 checker 验收
        ↓ checklist items 逐项 done
[用户确认门] implement→check
        ↓
   state = check                          ← 最终全量验收(跨层一致性+lint+typecheck)
[sub-agent: checker ×full-scope]
        ↓
[沉淀闭环] agent 把决策/教训写进 .everlasting/spec/
   state = done                           ← task 归档
```

**实施阶段不在 state machine 里**:task 内部"后端实施→后端测试→前端实施→联调"是 agent 在 planning 写进 **task.json.items** 的实施阶段拆分,由 agent 自己推进。这避免 state 爆炸,把"怎么拆"还给 LLM(见 §6.2 S2)。

### 4.2 第二个 plugin `review`(评审流)—— 愿景,延迟讨论

把"写需求文档"变成"多视角评审 + 用户参与的收敛过程":

```
用户:创建一个需求(澄清需求)
        ↓
[agent 写 prd 草稿]
        ↓
[发起多个 sub-agent 评审]  ← 关键差异点
   reviewer-架构 / reviewer-安全 / reviewer-性能 ...
   过程中来回通讯,保持各自上下文,用户也参与
        ↓ (多轮收敛)
[最终需求文档]
```

**这个流程的硬骨头**:B6 当前是 **one-shot dispatch**(parent 派 → worker 跑完 → 回一个 summary),worker 间隔离、不能实时通讯。"评审来回通讯、保持上下文"需要 B6 不支持的协调模型。两种实现路径,差异巨大:

| 路径 | 描述 | 可行性 |
|---|---|---|
| **(A) 回合制**(轮次 + orchestrator 中介) | 每轮并行 dispatch N 个 reviewer,收集后把"当前文档 + 各 reviewer 上轮意见"喂回去再 dispatch;reviewer 靠喂回历史"记得"自己说过什么 | **B6 + L3b 今天就能做**(并发 dispatch 已有);代价是回合制非实时,延迟高 |
| **(B) 真正实时群聊**(持久 agent + 共享 channel) | reviewer 是持久 agent,实时看到彼此消息(像群聊) | 需要**新通讯原语**——可移植 Trellis `trellis-channel` 概念("cross-agent review" 跟本流程几乎一字不差);是 Everlasting 没有的新子系统,独立大件 |

> ❓ **延迟讨论**:评审流走 A 还是 B,何时立项。本阶段只把愿景写入,不展开。A 可在 engine 稳定后作为 Phase 4-5 的第二个 plugin 落地;B 若需要则单独立项(新通讯架构),不阻塞主 workflow。

### 4.3 plugin 化的最大价值就在这里

两个流程结构根本不同(线性 pipeline vs 迭代 fan-out/gather)。**engine 把任何一个写死,另一个就塞不进去。** 只有 engine 是"读配置驱动的 state machine + 支持多种协调模型",两者才能作为可切换 plugin 共存。这就是 §2.2 说的"plugin 化不是为了灵活而灵活,是为了让根本不同的工作流共享同一个 engine"。

---

## 5. 架构:engine vs plugin

### 5.1 分层

| 层 | 性质 | 内容 |
|---|---|---|
| **engine(Rust,固定)** | 机制 | session workflow 开关;读 plugin 配置;拼 breadcrumb + 注入(append seam);state 转移门控;dispatch 角色/协调模型门控;task 文件 IO;UI plugin 列表/切换 |
| **plugin 内容(文件态,可配)** | 规范 | `.everlasting/workflow/<name>/`:`workflow.json`(state 枚举 + transitions + breadcrumb 模板 + 角色映射 + delegation 模板)+ `agents/*.md`(Q5:plugin 自带角色)+ `skills/*/SKILL.md`(plugin 自带 wf-* skill) |
| **全局内容(已可配,plugin 外)** | 规范 | `.everlasting/skills/*`(B4 全局 skill);`.everlasting/agents/*`(L3d 全局 agent)。workflow session 里 plugin 的 agents/skills 优先于全局 |

**关键**:skill 和角色两层**本来就 plugin 化了**(B4/L3d 的 project > user > builtin 三层覆盖)。本功能让 plugin 再自带一份专属 agents/skills(plugin 优先),实现 plugin 自洽可移植。真正新增的 plugin 层是 **`workflow.json`**(state machine 定义 + delegation 模板)。

### 5.2 engine 能力(engine 对所有 plugin 通用)

1. **读 plugin 配置** → 拿到 states / transitions / breadcrumb 模板 / 角色映射 / 协调模型
2. **注入**:task 元数据 + summary → append `messages[0]`(常驻);state breadcrumb → append per-turn request clone(复用 [`inject_recall_into_turn`](../.trellis/spec/backend/memory.md) seam,`cache_control: None`)
3. **门控(统一协商档)**:dispatch 时按"当前 state→允许角色映射"校验;不允许时**不硬拒**,而是触发协商——engine 调 `ask_user_question` 问用户"允许这次破例 / 确认推进 state 吗"(Q3 + S3 决定:所有门控违规统一走协商,不分角色门控/state 转移)。执行点在 `chat_loop.rs` 的 dispatch_subagent 拦截处(`run_subagent` 前,见 §6.6.2)
4. **state 转移**:用户确认门(复用 `ask_user_question`);转移触发 task.json 更新
5. **task 文件 IO**:agent 通过专用 tool 写 task.json/prd/checklist/progress
6. **plugin 列表/切换**:扫 `.everlasting/workflow/*/`;UI 选/切

### 5.3 plugin 配置格式:JSON

**决定(Q1,2026-07-07)**:plugin 配置用 **JSON**(`workflow.json`)。理由:整体流程把控更精准——state machine 是结构化数据(states/transitions/角色映射/breadcrumb 模板),JSON 的 schema 严、机器生成/校验易、转义无歧义。

```
<project>/.everlasting/workflow/<name>/workflow.json
```

**schema 草稿**(实施时以 Rust struct + serde 为准):

```json
{
  "name": "dev",
  "description": "标准开发流程",
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
    "planning":  "你在 planning 阶段。先 dispatch researcher 调研,产出 prd.md + 拆 task.json.items(实施阶段)...\n不要写实现代码。完成后问用户确认进 implement。",
    "implement": "你在 implement 阶段。按 checklist 逐项 dispatch implementer...",
    "check":     "你在 check 阶段。派 checker 做最终全量验收...",
    "done":      "task 完成。把本次决策/教训提炼写进 .everlasting/spec/..."
  },
  "delegation_templates": {
    "researcher":  "你正在为 task「{title}」调研(state={state})。{summary}\n调研范围: ...\n不要写代码。产出: ...",
    "implementer": "你正在为 task「{title}」实现一项(state={state})。{summary}\n约束: ...\n验收标准: ...",
    "checker":     "你正在为 task「{title}」验收(state={state})。{summary}\n验收维度: lint/typecheck/跨层一致性\n通过标准: ..."
  }
}
```

**Trade-off 已接受**:JSON 多行文本(breadcrumb)需 `\n` 转义,人手改门槛比 markdown 高。接受理由:workflow.json 低频手改(定义流程,不是日常产出)、高频被 engine 严格解析,机器友好 > 人友好。breadcrumb 模板较长时,可用 `\n` 拼接;engine 加载后渲染时自然换行。

**跟现有四子系统的关系**:commands/agents/skills 是"人手写、高频读"的规范文件,保 frontmatter+body;workflow.json 是"低频手写、高频精确解析"的配置文件,选 JSON。两者不冲突,各自贴其使用模式。

engine 读 JSON 用 `serde_json` 反序列化成 `WorkflowDef` struct(对标 [`SubagentDef`](../.trellis/spec/backend/agent-loop-architecture.md)),零自写解析。

### 5.4 默认即 fallback + Phase 0 预留 plugin 接口

**决定(Q2,2026-07-07)**:Phase 0 **就预留 plugin 接口**,数据源先用硬编码常量,Phase 2 换成读 workflow.json。engine 主体从 Phase 0 起只通过接口访问流程数据,后续变更是**扩展不是重构**。

**理由**:plugin 化和评审流是**已知一定要做的**(不是"也许 someday")。若 Phase 0-1 硬编码成散落的 if-else,Phase 2 外置 + 评审流加入会是**侵入式重构,碰已稳定的注入 seam / 门控 / UI**。既然抽象边界已知(就是 §5.3 的 JSON schema),现在就把接口留对,把后续变更风险从"重构现有功能"转移到"Phase 0 多写点 struct"(后者安全得多)。

**预留接口的具体形态**(Phase 0 就做到):

```rust
// WorkflowDef 是 workflow.json 的镜像,Phase 2 加 #[derive(Deserialize)] 即可外置
struct Transition {
    from: String,
    to: String,
    requires_user_confirm: bool,
}

struct WorkflowDef {
    name: String,
    description: String,
    states: Vec<String>,
    initial: String,
    transitions: Vec<Transition>,
    roles_by_state: HashMap<String, Vec<String>>,
    breadcrumb: HashMap<String, String>,
    delegation_templates: HashMap<String, String>,   // M1: role → 模板(Q6)
    coordination: String,                             // M5: 默认 "pipeline";review 用 "round-robin"/"fan-out-gather"(Q8 待决)
}

// Phase 0:数据源是硬编码常量(dev 四态)
fn default_workflow() -> WorkflowDef { /* 常量值 */ }
// Phase 2 之后:数据源换成读文件(一行替换,engine 主体零改动)
// fn load_workflow(name: &str) -> WorkflowDef { serde_json::from_reader(...).unwrap() }

// engine 全程只认 WorkflowDef + 这四个访问函数,绝不直接 match state { "planning" => ... }
fn breadcrumb_for(def: &WorkflowDef, state: &str) -> &str;
fn allowed_roles(def: &WorkflowDef, state: &str) -> &[String];
fn can_transition(def: &WorkflowDef, from: &str, to: &str) -> bool;
fn delegation_template_for(def: &WorkflowDef, role: &str) -> Option<&str>;   // M1: 第四访问函数
```

**关键不变量**:engine 主体(注入 seam 调用、门控判断、UI 渲染)**从 Phase 0 起只通过 `WorkflowDef` 和这四个函数访问流程数据**。绝不内联 state 字符串判断。

**workflow.json validate + fallback**(M6 评审修正):`load_workflow` 不是裸 serde,要带校验 + 回退:

```
1. 读 workflow.json → serde_json 解析失败 → log warn → 回退 default_workflow()
2. 解析成功 → validate:
   - states 非空
   - initial ∈ states
   - transitions 的 from/to ∈ states
   - roles_by_state keys ⊆ states
   → 任一失败 → log warn + 回退 default
3. delegation_templates / breadcrumb 某键缺失 → 该 role/state 用空字符串(warn),不阻塞加载
```

**后续变更影响范围**(预留接口后的收益):

| 后续动作 | 改动范围 | 碰 engine 主体? |
|---|---|---|
| Phase 2 外置默认 | `default_workflow()` → `load_workflow()`(含 validate) | ❌ 零改动 |
| Phase 2 加 UI 切换 | 加 plugin 列表/选择器,多加载几个 WorkflowDef | ❌ 零改动 |
| 评审流加入(回合制 A) | 放第二个 workflow.json(`coordination` ≠ pipeline) | ⚠️ engine 内部加 `coordination` 分发分支(非接口改动,是 engine 内部能力扩展) |
| 评审流加入(实时群聊 B,Q8) | 同上 + 新通讯原语 | ⚠️ 若 Q8 选 B 需独立立项(新通讯架构) |

> 注:`coordination` 字段在 WorkflowDef 里**Phase 0 就预留**(默认 "pipeline"),review 加入时**接口不变**,只 engine 内部加分发分支——这是 M5 评审建议的"扩展不是重构"。

**fallback 策略**:builtin 默认 plugin `dev` 随 app 分发(Phase 0 = `default_workflow()` 常量);项目无 `.everlasting/workflow/` 时用默认;项目放 `.everlasting/workflow/dev/workflow.json` → 覆盖默认(validate 失败回退默认);放第二个 `.everlasting/workflow/review/workflow.json` → 多一套可选(Phase 2 后)。

### 5.5 UI:workflow 切换(≠ task picker)

- session 顶栏:workflow toggle(on/off)+ 当前 plugin 名(点击切换)
- 切换的是**流程模板**(怎么干活),不是 task(干哪个 task)——性质完全不同于被否决的 task picker
- 切 plugin = 改当前会话的 in-memory plugin 选择 + 重新注入对应 breadcrumb
- 默认 plugin = dev;装了 review 后可在两者间切

---

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

1. **加载机制**:task.json 元数据 + `summary` → append `messages[0]`(常驻,小)。prd/design/progress **全文不放**,只给 path,agent `read_file` 自取(避免大文件撑爆 context,复用 [`RECALL_TOKEN_BUDGET`](../.trellis/spec/backend/memory.md) 预算思维)。
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

#### 6.6.1 模板注入机制(S1 评审修正)

**问题**:engine 填好 task meta 占位符后,模板文本如何到达 agent 上下文?

**决定**:走方案 (a)——engine 把填好的 delegation 模板 append 到 **per-turn request clone 的 `messages[0]` block 数组**(复用 [`inject_recall_into_turn`](../.trellis/spec/backend/memory.md) 同款 seam,`cache_control: None`)。

**注入时机**:当 turn 内出现 `dispatch_subagent` tool_use 时,engine 在拦截执行点(见 §6.6.2)解析目标 role → 取 `delegation_templates[role]` → 填 `{title}`/`{summary}`/`{state}` 占位符 → append 到 messages[0]。主 LLM 下一轮看到模板,照框架写委托语。

**为什么不靠 agent use_skill 自觉加载**(否决评审 S1 方案 b):delegation 模板是 dispatch 的**基础设施**(类似 system prompt 的角色规范),不是按需知识。靠 agent 自觉 = agent 可能漏加载 → worker 收到无框架的 delegation,角色化失效。engine 注入保证 worker 进场一定有角色规范。

**跟 Q4 不冲突**:Q4 决定不自动 inject **skill body**(wf-* 是按需知识,保 B4 三层渐进披露);delegation 模板是 dispatch 基础设施,性质不同,engine 注入。

#### 6.6.2 门控执行点(M3 评审修正)

**dispatch 拦截点**:`chat_loop.rs` 的 `dispatch_subagent` 拦截处(`run_subagent` 调用前,对标 [agent-loop-architecture.md "Tool interception"](../.trellis/spec/backend/agent-loop-architecture.md) 的现有 pattern)。engine 在此:

1. 解析 tool_use input → 拿到目标 role
2. 查 `roles_by_state[当前 task.state]` 是否含该 role
3. **允许** → 注入 delegation 模板(§6.6.1)→ 调 `run_subagent`
4. **不允许** → 触发协商档(§6.1 Q3):engine 调 `ask_user_question` 问用户"这个 task 还在 {state},允许派 {role} 吗 / 确认推进 state 吗" → 用户允许则放行(一次性越权,不改 workflow.json)→ 拒绝则回 breadcrumb 提示

**不改 `run_subagent` 签名**:门控 + 模板注入都在拦截点完成,`run_subagent` 收到的是已放行 + 已带模板上下文的 dispatch。

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

## 7. 第二个 plugin `review`(评审流)(愿景,延迟讨论)

见 §4.2。本阶段只记录愿景 + open question(回合制 A / 实时群聊 B),不展开设计。A 可在 engine 稳定后作为第二个 plugin 落地;B 若需要则单独立项(新通讯架构,可移植 Trellis channel 概念),不阻塞主 workflow。

> ❓ **Q8(延迟)**:评审流走 A 还是 B,何时立项。

---

## 8. hook:Everlasting 没有 hook runner

**现状**(code 查证):Everlasting Rust 后端**没有 task.json hooks / 生命周期脚本执行器**。Trellis 的 `after_create/after_start/after_finish/after_archive` 是 Python 脚本,跑在 `.trellis/` 体系里,与 app 后端无关。

| 方案 | 描述 | 成本 |
|---|---|---|
| (a) 不做 hook,全靠 skill + agent 主动 | task 文件写入/state 转移动作由 agent 在 skill 指导下主动调用 | 零新机制,但太软(沉淀闭环会失效) |
| **(b) Rust 固定逻辑 hook**(Q9 选定) | state 转移时 Rust 跑固定动作(done 时触发沉淀、planning→implement 时自动前置检查) | 中 |
| (c) 新建 hook runner | 仿 Trellis 配脚本,state 事件触发执行 | 高(新子系统+安全面) |

> **Q9 决定(2026-07-07)**:选 **(b) Rust 固定逻辑 hook**。理由:(a) 太软,沉淀闭环(task done → 写 spec)是机制价值保证,不能靠 agent 自觉——agent 可能忘写 spec,闭环断;`wf-update-spec` skill 教 agent 怎么写,但"触发写"这个动作不能交给 agent 发挥。(c) 过重,脚本执行 = 任意代码执行,违背 [DESIGN.md "本地优先"](./DESIGN.md#22-关键约束) 不爱外部脚本执行;为几个固定动作造整个脚本 runner 杠杆不足。(b) 几个关键 state 转移动作写成 Rust 固定代码,改动作要改代码,但这几个动作低频稳定。
>
> **具体落点**:`task.json.status` 写入函数(state 转移 tool)里嵌固定 hook 分支:
>
> ```rust
> fn set_task_state(task: &mut Task, new_state: State) -> Result<()> {
>     let old_state = task.status;
>     task.status = new_state;
>     write_task_json(task)?;
>     // 固定 hook 逻辑(几个分支,不扩展成 runner)
>     match (old_state, new_state) {
>         (Check, Done) => trigger_spec_distillation(task)?,  // 沉淀闭环
>         (Planning, Implement) => preflight_implement_check(task)?,  // 可选前置
>         _ => {}
>     }
>     Ok(())
> }
> ```
>
> **跟 Q2 接口的关系**:hook 嵌在 state 转移函数里,该函数是 Q2 锁定的 engine 接口一部分。Phase 3 加 hook = 给 `set_task_state` 加几个 match 分支,不破坏 WorkflowDef 接口。**hook 动作是机制核心(沉淀),不是流程内容**(不由 plugin 配);未来 plugin 若要自定义 hook 动作,再做 trait 抽象,但默认 hook(沉淀)仍 Rust 固定。Phase 3 实施。
>
> **hook 触发路径(M2 评审修正,决定3)**:`set_task_state` **由 user 确认门自动调用,不由 agent tool 调用**。理由(对标 Q9"不靠 agent 自觉"):若由 agent 调用,agent 可能忘记 → 沉淀闭环断。现有 `ask_user_question` 是纯问答,resolve 后只返回 answer JSON 无副作用([`tools/ask_user_question.rs`](../STRUCTURE.md) + `question_store.rs::resolve`)。改造:engine 在 state 转移确认门的 resolve handler 里,**返回确认答案后**主动调 `set_task_state(task, new_state)` → 触发 hook。调用链:
>
> ```
> agent ask_user_question("确认进 implement?")
>   → user 确认
>     → engine resolve handler
>       → set_task_state(task, Implement)   ← 自动,不依赖 agent
>         → write_task_json + hook 分支(preflight_implement_check)
> ```
>
> 这把 state 转移 + hook 触发都收敛到 engine 的确认门 resolve 路径,agent 只负责"申请转移"(ask_user_question),不负责"执行转移"。跟 §6.6.2 门控执行点一致(都是 engine 在 dispatch/resolve 拦截处主动作为)。

---

## 9. 分阶段实施计划

> 排期归 [ROADMAP.md](./ROADMAP.md)。本节是**功能落地依赖拓扑**,每阶段独立可验证。

### Phase 0:engine 骨架(state 注入 + workflow 开关)

**目标**:workflow 开关 + state machine + breadcrumb 注入跑通,验证"agent 读 breadcrumb 改变行为"。

- `sessions` 表加 `workflow_enabled` 列 + migration
- 顶栏 workflow toggle(B7 mode 切换同级)
- engine 读"硬编码默认 plugin"(dev 四态,架构留位待 Phase 2 外置)
- task.json 读写 + `.everlasting/tasks/<slug>/` 目录约定
- state breadcrumb 注入(复用 `inject_recall_into_turn` append seam)
- agent 在 workflow session 自动起/续 task
- state 转移用户确认门(复用 ask_user_question)

**完成标准**:开 workflow → agent 自动起 task 写 task.json → 推进 state(用户确认)→ breadcrumb 随 state 变化可见。

### Phase 1:skill 规范包 + plugin skill loader

**目标**:"规范"落地,agent 知道"标准做法";plugin skill 加载路径就位(避免 Phase 2 迁移债,M4 决定4选 b)。

- **plugin skill loader**:`skill/loader.rs` 加 workflow plugin skills 解析层(`.everlasting/workflow/<plugin>/skills/*/SKILL.md`),解析顺序 plugin skills → project 全局 → user 全局 → builtin(见 §6.3)。即使 Phase 1 时 workflow.json 还是硬编码,skill 走 plugin 目录
- 5 个 wf-* skill(wf-overview / wf-brainstorm / wf-before-dev / wf-check / wf-update-spec,借鉴 Trellis 同名,**去 Trellis Python hook 依赖换成 Everlasting Rust hook 触发**,纯描述性);放 plugin 目录 `.everlasting/workflow/dev/skills/`
- artifact 查阅机制:task.json 元数据+summary 进 messages[0];prd/design/progress 给 path

**完成标准**:进 workflow 时 agent use_skill wf-overview 建立全局意识;planning state agent use_skill wf-brainstorm;各 skill body 被读到。

### Phase 2:plugin 外置 + sub-agent 角色 + 上下文注入

**目标**:engine/content 真分离;调研/实施/验收真分工;worker 看得到 artifact。

- 硬编码默认外置成 `.everlasting/workflow/dev/workflow.json`;engine 改读配置驱动(`default_workflow()` → `load_workflow()`,engine 主体零改动,见 §5.4)
- plugin agents/ 落地(Q5:plugin 自带 researcher/implementer/checker 三角色;builtin general-purpose 不动)
- state 门控 dispatch(统一协商档,见 §5.2.3 + §6.6.2;执行点 chat_loop dispatch 拦截处)
- worker 上下文注入(Q6:delegation 模板,engine 填 task meta 占位符 + 主 LLM 填委托细节)
- checklist 同步(S2 选 c):`update_checklist` 在 workflow session 内改写 task.json.items(非 loop-local Vec);B12 coerce 逻辑保留;非 workflow session 行为不变
- UI:plugin 切换(此时只有一个 plugin,但切换机制就位)

**完成标准**:改 workflow.json 能改流程;planning 只派 researcher;worker 收到的 delegation 带 task meta + 角色模板框架;checklist items 跨 session 持久(task.json)。

### Phase 3:hook + 沉淀闭环

**目标**:task done 自动沉淀 spec;长跑积累项目规范。

- state 转移 Rust 固定 hook(Q9 选 b:done 时触发沉淀)
- `.everlasting/spec/` 目录 + wf-update-spec 落地
- progress.md 交接叙述
- task 归档(done → `.everlasting/tasks/archive/`)

**完成标准**:task done → spec 写进 .everlasting/spec/;下次 implement 读得到;跨 session 续 task 接上。

### Phase 4+:第二个 plugin `review`(评审流)+ B8 衔接(愿景/远期)

- review 作为第二个 workflow.json(Q8:回合制 A 可直接做;实时群聊 B 需新通讯原语,独立立项)
- B8 DAG(归 ROADMAP 第四档)

> Phase 4 不在 MVP 范围;本文档只记愿景,设计届时展开。

### 9.5 分步实施指导(每个 Phase 拆成可独立验证的小步)

> 上面的 Phase 0-3 是粗粒度阶段划分。实际开发时每个 Phase 体量不小(尤其 Phase 1/2),拆成可独立验证、可独立提交的小步,降低风险 + 加快反馈。每步标注**验证手段**(怎么确认这步做对了)和**依赖**(必须先完成哪步)。
>
> 每个小步 = 一个 git commit + 一个可复现的验证(测试 / 手动操作 + 预期)。**不累积多步一起提交**——出问题好回滚。

#### Phase 0 拆步(5 步,每步独立提交)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **0.1** | `sessions` 表加 `workflow_enabled` 列 + migration;Pinia store + Tauri command 读写 | cargo test migration 通过;前端读得到字段 | — |
| **0.2** | 顶栏 workflow toggle UI(B7 mode 切换同级组件)+ 绑 store | 手动点 toggle,DB 列翻转;关 session 重开状态保持 | 0.1 |
| **0.3** | `WorkflowDef` struct + 4 访问函数 + `default_workflow()` 硬编码常量(dev 四态)——**纯 Rust,无 UI** | cargo test:`breadcrumb_for(planning)` 返回期望文本;`can_transition(planning, implement)=true` | — |
| **0.4** | task.json 读写 + `.everlasting/tasks/<slug>/` 目录约定 + `create_task` command(§6.8) | 手动调 create_task → 目录 + 文件生成,JSON 合法;读回来字段对 | 0.3 |
| **0.5** | state breadcrumb 注入(复用 `inject_recall_into_turn` append seam)+ agent 在 workflow session 自动起/续 task(§6.2 触发点) | 集成测试:workflow session 发开发意图消息 → task.json 生成 → breadcrumb 随 state 变化出现在 messages[0] | 0.2, 0.4 |

**Phase 0 完成标志**:0.1-0.5 全过 → 开 workflow → agent 起 task → breadcrumb 可见。

#### Phase 1 拆步(4 步,体量大但每步聚焦)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **1.1** | plugin skill loader(`skill/loader.rs` 加 workflow plugin skills 解析层,M4 选 b) | cargo test:plugin skills 优先于全局解析;无 plugin 时 fallback 全局 | Phase 0 |
| **1.2** | wf-overview skill body 完整(§A.4)——这一个先做,是 agent 进 workflow 的入口 | 手动:workflow session agent use_skill wf-overview → body 返回 | 1.1 |
| **1.3** | wf-brainstorm / wf-before-dev / wf-check / wf-update-spec 4 个 skill body(§A.5 outline 填肉) | 各 skill 被对应 state 的 agent 加载 | 1.2 |
| **1.4** | artifact 查阅机制:task.json 元数据+summary append messages[0];prd/design/progress 给 path | 集成测试:messages[0] 含 task meta;agent read_file prd 成功 | 1.2 |

**Phase 1 完成标志**:进 workflow agent 读 wf-overview;planning 用 wf-brainstorm;各 skill body 可读;task meta 注入可见。

#### Phase 2 拆步(6 步,最重,分两批)

**批 A:plugin 外置 + 接口换源(2 步)**

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **2.1** | workflow.json 外置:`default_workflow()` → `load_workflow()` + validate + fallback(M6) | cargo test:改 workflow.json states → breadcrumb 跟着变;malformed JSON → fallback default + warn | Phase 1 |
| **2.2** | UI plugin 切换(顶栏 plugin 名点击 → 切换;此时只有 dev 但机制就位) | 手动切 plugin → breadcrumb 重新注入 | 2.1 |

**批 B:sub-agent 角色 + 门控 + 注入(4 步)**

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **2.3** | plugin agents/ 落地(researcher/implementer/checker frontmatter + body,§A.3);loader 加 plugin agents 解析层(Q5) | cargo test:workflow session 解析 implementer 用 plugin 的;非 workflow 用全局 | 2.1 |
| **2.4** | 门控执行点(§6.6.2):chat_loop dispatch 拦截处加 role×state 校验 + 协商档(ask_user_question) | 集成测试:planning 派 implementer → 弹协商;用户允许 → 放行;拒绝 → breadcrumb | 2.3 |
| **2.5** | delegation 模板注入(§6.6.1):engine 填 task meta 占位符 → append messages[0] | 集成测试:dispatch 时 messages[0] 含填好的 delegation 模板;worker 读到角色规范 | 2.3 |
| **2.6** | checklist 同步(S2):`update_checklist` workflow session 内改写 task.json.items;B12 coerce 保留 | 集成测试:跨 session 续 task → items 进度持久;worker 读 task.json 拿进度 | 2.3 |

**Phase 2 完成标志**:改 workflow.json 改流程;planning 只派 researcher;delegation 模板注入;checklist 跨 session 持久。

#### Phase 3 拆步(3 步)

| 步 | 内容 | 验证手段 | 依赖 |
|---|---|---|---|
| **3.1** | state 转移 Rust 固定 hook(§8):`set_task_state` 加 match 分支;user 确认门 resolve handler 自动调(M2) | 集成测试:Check→Done 触发 hook;planning→implement 触发 preflight | Phase 2 |
| **3.2** | `.everlasting/spec/` 目录 + wf-update-spec 落地(沉淀闭环) | 手动:task done → spec 文件生成;下次 implement 读得到 | 3.1 |
| **3.3** | progress.md 交接叙述 + `archive_task` command(§6.8) | 手动:done → task 移到 archive/YYYY-MM/;新 session 读 progress 续上 | 3.2 |

**Phase 3 完成标志**:done 自动沉淀 spec + 归档;跨 session 续 task 完整闭环。

#### 总步数 + 依赖图

```
Phase 0 (5 步):  0.1 → 0.2 ─┐
                  0.3 → 0.4 ─┤→ 0.5
                             ↓
Phase 1 (4 步):  1.1 → 1.2 → 1.3
                         ↘ 1.4
                             ↓
Phase 2 (6 步):  2.1 → 2.2          (批 A)
                  2.3 → 2.4 / 2.5 / 2.6  (批 B,2.4/2.5/2.6 并行)
                             ↓
Phase 3 (3 步):  3.1 → 3.2 → 3.3
```

**关键并行点**:Phase 2 批 B 的 2.4(门控)/ 2.5(delegation 注入)/ 2.6(checklist)三步都只依赖 2.3,可并行做。

**风险最高的步**:2.4(门控执行点,改 chat_loop 拦截)、2.6(checklist 同步,改 B12 写路径)——这两步碰现有稳定代码,做完立即跑全量 cargo test + vitest。

---

## 10. 关键决策(岔路记录)

### 10.1 engine 与 content 分离(plugin 化)

**决策**:Rust 只提供 engine(注入/门控/转移/切换);workflow 内容(state/breadcrumb/角色映射/协调模型)是 `.everlasting/workflow/<name>/` 文件态 plugin,项目可改可换。

**理由**(§2.2):"项目执行规范"是项目自己的;不同任务需要结构不同的工作流(开发流 vs 评审流);engine 写死任何一个,另一个就塞不进去。

### 10.2 主角是机制,不是 task

**决策**:核心是"engine + plugin 机制";task 是 plugin 运转时 agent 自动产出的文件态副产物,用户不感知不操作。

**否决**:"以 task 为中心"(建 task 表/UI/board)——task 是 agent 记账,加 task UI = 悬空抽象,违背"让 AI 自动按规范做事"初衷。

### 10.3 session 不绑 task,task 不入 DB

**决策**:task 纯文件态;session 不持有 task 引用;"current task" 是会话内 in-memory 状态。task 跨 session 靠文件位置天然达成。

### 10.4 workflow 是 session 级开关,正交于 Mode

**决策**:workflow = `sessions.workflow_enabled`;[Mode](./DESIGN.md) 是独立权限旋钮,两者正交,state 不替用户切 Mode。

### 10.5 state machine 元流程(固定枚举)+ 实施阶段(LLM 拆)

**决策**:元流程 state(planning/implement/check/done)是 plugin 配置(默认固定,可改);task 内部实施阶段是 LLM 在 planning 写进 **task.json.items** 的实施拆分(见 §6.2 S2),不进 state 枚举。

### 10.6 state 转移用户确认门,agent 不能自翻

**决策**:planning→implement / implement→check / check→done 需用户确认。agent 自翻 = 流程失去外部校验,可能跳过验收直接 done。

### 10.7 注入一律 append 到 messages[0],不碰持久化

**决策**:所有 workflow 注入 append 到 per-turn request clone 的 messages[0],`cache_control: None`,绝不新开 user message。保 Anthropic prompt cache breakpoint 不失效(5-10× 成本)。这是 [`memory_recall`](../.trellis/spec/backend/memory.md) + [B12 checklist](../.trellis/spec/backend/agent-loop-architecture.md) 已验证的硬规则。

### 10.8 UI workflow 切换 ≠ task picker

**决策**:UI 可切 workflow plugin(选"怎么干活"),但无 task picker(否决"选干哪个 task")。两者性质不同。

---

## 11. 风险与权衡

### 11.1 技术风险

| 风险 | 严重度 | 缓解 |
|---|---|---|
| breadcrumb 软门控,agent 绕过流程 | 中 | opt-in session 内 agent 配合度高;门控违规统一走协商档(Q3+S3:engine 主动 ask_user_question,见 §5.2.3/§6.6.2),非纯软提示 |
| worker 上下文注入(Q6 delegation 模板落地) | 中 | Phase 2 实施;模板 + task meta + 主 LLM 填委托,达成 Trellis jsonl 注入的等价效果(见 §6.6) |
| 沉淀闭环失效(agent 忘写 spec) | 中 | Phase 3 Rust 固定 hook(Q9 选 b)强制 done 时触发 |
| plugin 配置格式不稳定(过早抽象) | 中 | Phase 0-1 硬编码默认先验证,Phase 2 再外置;第二个 plugin 出现才逼出抽象 |
| workflow session token 预算(breadcrumb+元数据+delegation+recall 都进 messages) | 中 | 严格 append + cache_control + 预算;prd 全文不入。per-turn 估算(见下表) |
| 评审流需要新通讯架构(Q8 选 B) | 低(愿景阶段) | 延迟讨论;A 回合制先用 B6+L3b 落地,B 独立立项不阻塞 |

**token 预算 per-turn 估算**(M7 评审修正):

| 注入项 | per-turn 估算 | cache 命中时成本 |
|---|---|---|
| breadcrumb(state 模板) | ~400 tokens | 0(append 到 messages[0],cache_control: None,不破坏缓存) |
| task.json metadata(title/summary/state) | ~50 tokens | 0(同 messages[0] block append) |
| delegation template(dispatch 时,见 §6.6.1) | ~200 tokens | 0(同上,仅 dispatch turn) |
| checklist items(task.json.items,agent 主动 read_file) | 按需,不入常驻 | N/A |
| memory recall(已有) | ≤[`RECALL_TOKEN_BUDGET`](../.trellis/spec/backend/memory.md) | 0(已有预算约束) |

**结论**:常驻注入(breadcrumb + task meta)~450 tokens,dispatch turn 额外 +200。全部 cache_control: None 不破坏 prompt cache breakpoint。可控,无需特殊预算机制。

### 11.2 工程权衡

**机制硬度 vs 灵活性**:opt-in session 内做硬约束,普通 session 零改动。代价:workflow session 行为分叉,要多写 spec。

**engine/content 分离时机**:Phase 0 就预留 plugin 接口(WorkflowDef struct + 三访问函数,见 §5.4),数据源先硬编码常量;Phase 2 换读 workflow.json。代价:Phase 0 多写点 struct;收益:Phase 2 外置 + 评审流加入是扩展不是重构,不碰已稳定的 engine 主体。

**沉淀质量 vs 自动化**:沉淀全靠 agent 写,不强制结构。代价:早期沉淀参差;长跑后有用 spec 浮出来。

---

## 12. 与现有体系的接入点(实施时接缝清单)

| 接入点 | 现有位置 | 本功能改动 |
|---|---|---|
| workflow 开关 | `sessions` 表 + 顶栏 | 加 `workflow_enabled` 列 + toggle(B7 mode 切换同级) |
| state breadcrumb 注入 | [`memory_recall::inject_recall_into_turn`](../.trellis/spec/backend/agent-loop-architecture.md)(append messages[0]) | 复用 seam,加 workflow breadcrumb block |
| plugin 配置加载 | `resource_loader.rs`(B3)/ `skill/loader.rs`(B4)模式 | 加 workflow loader(同 mtime fence),读 `.everlasting/workflow/*/` |
| task.json 加载 | `resource_loader.rs` / `subagent/loader.rs` 模式 | 加 task loader,读 `.everlasting/tasks/*/task.json` |
| task artifact 读写 | `read_file`(读) + 专用 task tool(写) | 读零改动;**写用专用 tool 非裸 write_file**(小问题6):裸 write_file 写 task.json 可能产出损坏 JSON(agent 手写) + 绕过 items coerce。加 `update_task` tool(只允许改特定字段:status/items/summary/progress,内部 serde 序列化保证 JSON 合法 + items coerce 复用 B12 逻辑) |
| task state 转移 | (新) | `set_task_state` 由确认门 resolve handler 自动调(M2,§8),非 agent tool |
| 内置 skill | B4 `skill/loader.rs` | wf-* skill 随 plugin 分发(放 `.everlasting/workflow/dev/skills/`),非全局 builtin;loader 加 plugin skills 解析层 |
| sub-agent 角色 | `builtin_subagents()`([`subagent/mod.rs:463`](../.trellis/spec/backend/agent-loop-architecture.md)) | plugin 自带 agents/(Q5);workflow session 里 plugin agents 优先于全局,builtin general-purpose/researcher 不动作 fallback |
| state 门控 dispatch | `chat_loop.rs` dispatch_subagent 拦截处(`run_subagent` 前,见 §6.6.2) | 统一协商档(Q3+S3):不允许时 engine 调 ask_user_question,不硬拒 |
| checklist 升格(S2) | `tools/update_checklist.rs`(B12 loop-local) | workflow session 内改写 task.json.items(非 loop-local Vec);B12 coerce 保留;非 workflow session 行为不变 |
| state 转移 hook | (无现有 hook runner) | Phase 3 新建 Rust 固定逻辑(Q9 选 b) |
| spec 沉淀 | (无) | `.everlasting/spec/` 新目录 + wf-update-spec skill |
| 用户确认门 | `ask_user_question` tool | 零改动,复用 |
| UI plugin 切换 | B7 mode 切换同级 | 加 workflow plugin 选择器 |

---

## 13. 后续维护承诺

- **本文件改动时机**:
  - Phase 0-3 任一落地 → 对应节标 ✅ + commit hash
  - §14 Q1-Q9 任一改判 → 改本文 + [IMPLEMENTATION §4](./IMPLEMENTATION.md#4-决策日志) 追 ADR
  - 评审流(§7)立项 → 展开 §7 为独立设计节
  - 跟 B8 边界重划 → 改 §3.2 + ROADMAP §2 第四档
- **不做的边界**:不列 commit/PR(走 git log);不做技术细节(走各 phase PRD + `.trellis/spec/backend/`);不做决策追溯(走 IMPLEMENTATION §4)
- **ROADMAP 接入**:稳定后在 [ROADMAP §2 第三档](./ROADMAP.md) 加 "W1 Workflow 集成(工作流引擎 + plugin)" 条目

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
| Q8(延迟) | 评审流走回合制 A 还是实时群聊 B,何时立项? | 延迟讨论 |
| ~~Q9~~ | ~~hook 选 (a)纯skill/(b)Rust固定/(c)runner?~~ | ✅ **(b) Rust 固定逻辑**(嵌在 `set_task_state` 写入路径;沉淀闭环是机制保证不靠 agent 自觉;不做脚本 runner 避免安全面)。Phase 3 |

---

## 15. 附录 §A:dev plugin 完整示例

> 本附录是 Phase 0-1 实施的参考物料。按 [问题 2 决定](#问题-2dev-workflow-有没有-templates-示例-skill-说明):workflow.json 完整填 + wf-overview 完整 body + 三角色 frontmatter + 其他 skill outline。复制 `.everlasting/workflow/dev/` 目录即整套流程。
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
    "researcher":  "你正在为 task「{title}」做调研(state={state})。\nTask 摘要: {summary}\n\n你的角色是 researcher(只读,不能写代码)。请:\n- 调研 [主 LLM 在此填本次委托的具体调研方向]\n- 读 .everlasting/tasks/<slug>/prd.md 了解需求(用 read_file)\n- 产出调研结论(技术方案/踩坑点/相关 spec 引用)\n- 不要修改任何文件\n\n验收:产出结构化调研报告,main agent 据此写 prd/checklist。",
    "implementer": "你正在为 task「{title}」实现一项(state={state})。\nTask 摘要: {summary}\n\n你的角色是 implementer。请:\n- 实现 [主 LLM 在此填本次委托的具体 item]\n- 读 .everlasting/tasks/<slug>/prd.md 和 design.md 了解上下文(用 read_file)\n- 遵守项目 spec 规范(若未加载,先 use_skill wf-before-dev)\n- 不要 dispatch_subagent(防嵌套)\n\n验收:[主 LLM 在此填该项验收标准]。",
    "checker":     "你正在为 task「{title}」验收(state={state})。\nTask 摘要: {summary}\n\n你的角色是 checker(只读 + 可跑 shell 测试)。请:\n- 验收 [主 LLM 在此填本次委托的验收对象]\n- 跑 lint / typecheck / 相关测试(用 shell tool)\n- 检查跨层一致性 / spec 合规\n- 不要修改文件,只产出验收报告(通过/不通过 + 具体问题)\n\n验收维度:lint、typecheck、测试通过率、跨层一致性、spec 合规。"
  }
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
tools: []   # 空数组 = 全集(同 general-purpose 语义);engine 在 workflow dispatch 拦截处自动剥离 dispatch_subagent(防嵌套),实际可用 = 全集减 STRUCTURALLY_DISABLED。与 general-purpose 仅靠 system_prompt 区分角色
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
