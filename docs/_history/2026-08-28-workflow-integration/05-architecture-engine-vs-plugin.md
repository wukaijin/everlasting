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
2. **注入**:task 元数据 + summary → append `messages[0]`(**per-turn,持久化不动**;非"常驻同步");state breadcrumb → append per-turn request clone(复用 [`inject_recall_into_turn`](../../../.trellis/spec/backend/memory.md) seam,`cache_control: None`)
3. **门控(统一协商档)**:dispatch 时按"当前 state→允许角色映射"校验;不允许时**不硬拒**,而是触发协商——engine 调 `ask_user_question` 问用户"允许这次破例 / 确认推进 state 吗"(Q3 + S3 决定:所有门控违规统一走协商,不分角色门控/state 转移)。**执行点下沉到 `run_subagent` 内部**(S-A 评审修正,见 §6.6.2)——三处调用点(串行/并发/测试)都过,避免并发 dispatch 绕过门控
4. **state 转移**:用户确认门(agent 用 `ask_user_question` 发起,带 `purpose="task_state_transition"` 标记;前端路由到专用 IPC `resolve_task_state_transition` 自动调 `set_task_state`,M-A);转移触发 task.json 更新 + hook
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
    "researcher":  "你正在为 task「{title}」调研(state={state})。{summary}\n相关 spec: {relevant_specs}\n调研范围: ...\n不要写代码。产出: ...",
    "implementer": "你正在为 task「{title}」实现一项(state={state})。{summary}\n相关 spec(必读): {relevant_specs}\n约束: ...\n验收标准: ...",
    "checker":     "你正在为 task「{title}」验收(state={state})。{summary}\n相关 spec: {relevant_specs}\n验收维度: lint/typecheck/跨层一致性\n通过标准: ..."
  },
  "coordination": "pipeline",
  "gather_strategy": {}
}
```

**Trade-off 已接受**:JSON 多行文本(breadcrumb)需 `\n` 转义,人手改门槛比 markdown 高。接受理由:workflow.json 低频手改(定义流程,不是日常产出)、高频被 engine 严格解析,机器友好 > 人友好。breadcrumb 模板较长时,可用 `\n` 拼接;engine 加载后渲染时自然换行。

**占位符全集(M-B)**:`delegation_templates` 支持 `{title}` / `{summary}` / `{state}` / `{relevant_specs}` 四个占位符。前三个从 task.json 直接取;`{relevant_specs}` 由 engine 按 task.summary 做 FTS5 过滤 `.everlasting/spec/` 索引返回候选 spec 路径列表(无匹配填 `(auto-detect via wf-before-dev)`)。这让 worker 进场就知道该读哪些 spec,贯彻"不靠自觉"(见 §6.6.1)。

**coordination 字段(M-C 修正)**:`"pipeline"`(默认,dev 用,串行/并发 dispatch 后各自返回)/ `"synthesis_round"`(review 用,每轮 dispatch 后必须 gather-reduce 再决定下一轮)。`gather_strategy: HashMap<state, Vec<Role>>` 声明每 state 收集哪些角色结果(仅 `synthesis_round` 用,`pipeline` 留空 `{}`)。原 M5 用 `"round-robin"` 字面是"轮流",但 review A 路径实质是"gather-reduce 再 dispatch",语义不贴;改 `synthesis_round` 更准(见 §4.2)。JSON 层是字符串,serde 反序列化到 Rust `enum Coordination { Pipeline, SynthesisRound }`(见 §5.4 struct),非法值走 M6 validate fallback。

**跟现有四子系统的关系**:commands/agents/skills 是"人手写、高频读"的规范文件,保 frontmatter+body;workflow.json 是"低频手写、高频精确解析"的配置文件,选 JSON。两者不冲突,各自贴其使用模式。

engine 读 JSON 用 `serde_json` 反序列化成 `WorkflowDef` struct(对标 [`SubagentDef`](../../../.trellis/spec/backend/agent-loop-architecture.md)),零自写解析。

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
    delegation_templates: HashMap<String, String>,   // M1: role → 模板(Q6);占位符 {title}/{summary}/{state}/{relevant_specs}(M-B)
    coordination: Coordination,                       // M-C: enum { Pipeline, SynthesisRound };默认 Pipeline(dev),review 用 SynthesisRound
    gather_strategy: HashMap<String, Vec<String>>,   // M-C: state → 收集哪些 role 结果(仅 SynthesisRound 用,Pipeline 留空)
}

enum Coordination { Pipeline, SynthesisRound }   // M-C: 替代原 String,语义明确

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
| 评审流加入(回合制 A) | 放第二个 workflow.json(`coordination: SynthesisRound` + `gather_strategy`) | ⚠️ engine 内部加 `Coordination` 分发分支(非接口改动,是 engine 内部能力扩展);接口 Phase 0 已预留 |
| 评审流加入(实时群聊 B,Q8) | 同上 + 新通讯原语 | ⚠️ 若 Q8 选 B 需独立立项(新通讯架构) |

> 注:`coordination` + `gather_strategy` 字段在 WorkflowDef 里**Phase 0 就预留**(默认 `Pipeline` + 空 map),review 加入时**接口不变**,只 engine 内部加 `Coordination` 分发分支——这是 M5/M-C 评审建议的"扩展不是重构"。

**fallback 策略 + 优先级**(小问题2 明示):
- **Phase 0**:builtin = `default_workflow()` 硬编码常量(写死在 engine);无 `.everlasting/workflow/` 时唯一数据源
- **Phase 2 起**:数据源优先级 = 项目 `.everlasting/workflow/<plugin>/workflow.json`(validate 通过)→ builtin `default_workflow()`(最后兜底)。builtin 此时降为"项目无 workflow.json 或 validate 失败时的兜底",不再是唯一数据源
- 项目放 `.everlasting/workflow/dev/workflow.json` → 覆盖 builtin;放第二个 `.everlasting/workflow/review/workflow.json` → 多一套可选

### 5.5 UI:workflow 切换(≠ task picker)

- session 顶栏:workflow toggle(on/off)+ 当前 plugin 名(点击切换)
- 切换的是**流程模板**(怎么干活),不是 task(干哪个 task)——性质完全不同于被否决的 task picker
- 切 plugin = 改当前会话的 in-memory plugin 选择 + 重新注入对应 breadcrumb
- 默认 plugin = dev;装了 review 后可在两者间切

---
