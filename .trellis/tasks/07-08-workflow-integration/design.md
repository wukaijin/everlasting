# Design — Workflow 集成:工作流引擎 + plugin(dev/review)

> 配套 `prd.md` + `docs/WORKFLOW-INTEGRATION.md`(单一引用源,两轮评审已过)。本文聚焦**实施视角**:接缝定位、契约变更、关键决策的代码落地。完整需求/动机/决策论证见设计文档,不重复。

## 1. 现状回顾(已验证代码位置)

| 事实 | 位置 |
|---|---|
| `sessions` 表无 migration 目录,走 `run_migrations` 内联过程式 SQL + `add_session_column_if_missing` 探针 | `db/migrations.rs:50,96-109,958` |
| 现有列探针:`color_tag`(migrations.rs:348)、`mode`(migrations.rs:412)—— `workflow_enabled` 照此模式加 | `db/migrations.rs:348,412` |
| `SessionRow` struct(映射 sessions 表) | `db/types.rs:246-287`;`Mode` enum `types.rs:204` |
| `run_chat_loop` 实际 **29 参**(注释写"26"已过期) | `agent/chat_loop.rs:173-393`;末参 `question_store` @ 392 |
| `inject_recall_into_turn` 有两分支:user-role Blocks @ messages[0] → **append**;否则 → **prepend 新建 synthetic user message(破坏 cache)** | `agent/memory_recall.rs:259-278` |
| `run_subagent` 签名 24 参,返回 `(String, bool, bool, Option<i32>)` | `agent/subagent/dispatch.rs:233-334` |
| `run_subagent` **3 处调用点**:forced-dispatch short-circuit(chat_loop.rs:1000)、L3b 并发 path(2937,`parallel=true`)、串行 LLM-driven(3286,`parallel=false`) | `agent/chat_loop.rs:1000,2937,3286` |
| B12 `ChecklistHandle = Arc<Mutex<Vec<ChecklistItem>>>`,每 `run_chat_loop` 重建(loop-local,不持久) | `tools/update_checklist.rs:50,54,71`;handle 创建 `chat_loop.rs:530` |
| B12 `execute` 做原子全替换(clear+extend),`coerce_at_most_one_in_progress` 保留 | `tools/update_checklist.rs:124,204` |
| `resolve_mode_change` IPC:"apply mode BEFORE resolve"——`set_session_mode_internal` 写 DB + audit,然后 `store.resolve(Answered)` unblock agent | `commands/question.rs:165,199-319`;apply @ 248-249,resolve @ 271-273 |
| `builtin_subagents()` 用 `OnceLock<Vec<SubagentDef>>`,有 researcher + general-purpose 两个 | `agent/subagent/mod.rs:463-548`;researcher @ 470,general-purpose @ 509 |
| `SubagentDef` struct:`name/description/system_prompt/tools/isolation/model` | `agent/subagent/mod.rs:403` |
| skill loader:三源 `SkillSource::{User, Project}`(无 Plugin 变体);`list_skill_infos` + `find_skill` 是 by-name 解析点 | `skill/loader.rs:46,482-498,503-519` |
| subagent loader:`SubagentSource::{Builtin, User, Project}`(无 Plugin);`list()` @ 623 是 merge 入口,`merge_with_inheritance` @ 943 source-agnostic | `agent/subagent/loader.rs:84,623,943` |
| `build_instructions_blocks` 产 messages[0] banner block,**仅 block 0 带 `cache_control: Some(Ephemeral)`**(cache breakpoint),其余 `None` | `memory/loader.rs:347,360-363` |
| `load_for_session` 返回 4 层(User Claude/Agents + Project Claude/Agents) | `memory/loader.rs:204-221` |
| `.everlasting/` 是项目命名空间,四子系统共用(commands/agents/skills/outputs) | 各 loader 的 `PROJECT_NAMESPACE = ".everlasting"` |

**关键洞察**:所有注入(breadcrumb / delegation 模板 / recall / checklist)都依赖 `messages[0]` 是 user-role Blocks message(B5 指令文件加载保证)。workflow session 默认满足此前置约束(S-B);`inject_recall_into_turn` 的 fallback prepend 分支破坏 cache,**禁触发**。

## 2. 方案总览(engine vs plugin 分层)

```
┌─────────────────────────────────────────────────────────┐
│ engine(Rust 固定,Phase 0 预留接口)                       │
│  • sessions.workflow_enabled 开关                          │
│  • WorkflowDef struct + 4 访问函数(Q2)                    │
│  • 注入 seam(复用 inject_recall_into_turn append 模式)     │
│  • 门控(下沉 run_subagent 内部,S-A)                       │
│  • state 转移(set_task_state + Rust 固定 hook,Q9/M-A)     │
│  • plugin loader(workflow.json / agents / skills)         │
│  • task CLI(create_task / archive_task / update_task)     │
└─────────────────────────────────────────────────────────┘
            │ 读 WorkflowDef + plugin 内容
            ▼
┌─────────────────────────────────────────────────────────┐
│ plugin 内容(文件态,可切换)                                │
│  .everlasting/workflow/<name>/                            │
│    ├── workflow.json   (state machine + breadcrumb +      │
│    │                     delegation_templates +            │
│    │                     coordination + gather_strategy)   │
│    ├── agents/*.md     (researcher/implementer/checker)    │
│    └── skills/*/SKILL.md (wf-overview/brainstorm/          │
│                          before-dev/check/update-spec)     │
│                                                           │
│  builtin 默认 = dev(Phase 0 硬编码常量;Phase 2 外置文件)   │
│  review plugin = 愿景(Q8 延迟,dev 跑通 ≥1 task 再立项)     │
└─────────────────────────────────────────────────────────┘
            │ agent 自动产出(副产物)
            ▼
┌─────────────────────────────────────────────────────────┐
│ task 记账(文件态,无 DB/无 UI)                             │
│  .everlasting/tasks/<slug>/                               │
│    ├── task.json   (元数据 + items 内嵌,S2)               │
│    ├── prd.md / design.md / progress.md                   │
│    └── spec-diff.md(可选,沉淀草稿)                        │
│                                                           │
│  .everlasting/spec/   (沉淀闭环,Q7;独立于 .trellis/spec/) │
│  .everlasting/tasks/archive/<YYYY-MM>/  (归档)             │
└─────────────────────────────────────────────────────────┘
```

## 3. 核心数据结构

### 3.1 WorkflowDef(Phase 0 预留,Q2)

```rust
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
    delegation_templates: HashMap<String, String>,   // 占位符 {title}/{summary}/{state}/{relevant_specs}(M-B)
    coordination: Coordination,                       // M-C: enum,默认 Pipeline
    gather_strategy: HashMap<String, Vec<String>>,   // M-C: 仅 SynthesisRound 用
}

enum Coordination { Pipeline, SynthesisRound }

// Phase 0:硬编码常量;Phase 2:serde_json 从 workflow.json 反序列化
fn default_workflow() -> WorkflowDef { /* dev 四态常量 */ }
fn load_workflow(name: &str) -> WorkflowDef { /* Phase 2:serde + validate(M6) + fallback */ }

// 4 访问函数(engine 全程只认这些,不内联 match state)
fn breadcrumb_for(def: &WorkflowDef, state: &str) -> &str;
fn allowed_roles(def: &WorkflowDef, state: &str) -> &[String];
fn can_transition(def: &WorkflowDef, from: &str, to: &str) -> bool;
fn delegation_template_for(def: &WorkflowDef, role: &str) -> Option<&str>;
```

### 3.2 task.json(含 items,S2)

```json
{
  "id": "uuid", "title": "...", "slug": "...",
  "status": "planning",
  "created_at": "...", "updated_at": "...",
  "parent": null, "summary": "...",
  "items": [
    { "id": "backend-impl", "content": "后端实施", "status": "done" },
    { "id": "frontend-impl", "content": "前端实施", "status": "in_progress", "tdd": true }
  ]
}
```

workflow session 内 `update_checklist` 改写 `task.json.items`(非 loop-local Vec);B12 coerce 保留;非 workflow session B12 行为不变。

## 4. 契约变更(碰现有代码的地方)

| 变更 | 位置 | 性质 | Phase |
|---|---|---|---|
| sessions 加 `workflow_enabled` 列 | `db/migrations.rs`(探针)+ `db/types.rs:SessionRow` + `db/sessions.rs`(读写) | 加列,跟 `color_tag`/`mode` 同模式 | 0 |
| `run_chat_loop` 加 workflow 上下文参数 | `agent/chat_loop.rs:173` 签名追加(可选 `workflow_ctx: Option<WorkflowCtx>`) | 增参(29→30),不破坏现有调用(传 None) | 0 |
| breadcrumb 注入 | `agent/chat_loop.rs` per-turn,复用 `inject_recall_into_turn` append 逻辑(抽出共享 helper 或直接调) | 新增注入点,不改 memory_recall | 0 |
| `run_subagent` 签名加 `current_state: &WorkflowState`(S-A) | `agent/subagent/dispatch.rs:233` + 三处调用点(chat_loop.rs:1000/2937/3286) | 增参(24→25),三处传同一 task state | 2 |
| 门控逻辑下沉 `run_subagent` 内部 | `agent/subagent/dispatch.rs` 入口加 role×state 校验 + 协商档 | 新增,不改现有 dispatch 逻辑 | 2 |
| `update_checklist` workflow session 内改写 task.json.items | `tools/update_checklist.rs:execute` 加分支(workflow session → 写文件;否则 loop-local Vec) | 加分支,非 workflow 路径不变 | 2 |
| skill loader 加 plugin skills 层 | `skill/loader.rs`:`SkillSource` 加 `Plugin` 变体 + `list_skill_infos`/`find_skill` 插入 plugin 优先层 | 扩 enum + 两处插入 | 1 |
| subagent loader 加 plugin agents 层 | `agent/subagent/loader.rs`:`SubagentSource` 加 `Plugin` 变体 + `list()` @ 623 加 plugin 层 push | 扩 enum + 一处 push;`merge_with_inheritance` source-agnostic 不改 | 2 |
| `set_task_state` + Rust 固定 hook | 新增(无现有 hook runner);嵌 match 分支 | 全新 | 3 |
| `resolve_task_state_transition` IPC | `commands/question.rs` 新增,对标 `resolve_mode_change` 双 IPC pattern | 全新 | 3 |
| `create_task` / `archive_task` / `update_task` IPC | `commands/` 新增(包装 task CLI 逻辑) | 全新 | 0/3 |

## 5. 关键决策(代码落地视角)

### 5.1 为什么门控下沉 `run_subagent` 而非 chat_loop 拦截(S-A)

`run_subagent` 有 3 处调用点(1000 forced-dispatch / 2937 L3b 并发 / 3286 串行)。在 chat_loop 任一处拦截都漏另外两处。下沉 `run_subagent` 内部 = 单一真相,新增调用点自动覆盖。签名加 `current_state` 参数,三处传同一 task state(Q2"扩展不是重构":增参不破坏现有语义)。

### 5.2 为什么 state 转移走专用 IPC 而非复用 ask_user_question resolve(M-A)

`resolve_tool_question`(commands/question.rs:92)签名 `(session_id, tool_use_id, answer, cancelled)` 无字段区分"普通询问 vs state 转移申请";`question_store::resolve` 仅 oneshot 送答案无副作用。强行加分支 = 污染通用问答 schema。`resolve_mode_change`(commands/question.rs:165)的"apply BEFORE resolve"双 IPC pattern 已验证,照搬新开 `resolve_task_state_transition`。

### 5.3 为什么 coordination 用 enum 而非 String(M-C)

原 M5 用 `"round-robin"` 字面是"轮流",但 review A 路径实质是"gather-reduce 再 dispatch"。改 `enum {Pipeline, SynthesisRound}` 语义贴;`gather_strategy` map 声明每 state 收集哪些角色结果(仅 SynthesisRound 用)。JSON 层是字符串,serde 反序列化到 enum,非法值走 M6 validate fallback。

## 6. 回滚 / 兼容

- **workflow 开关默认关**:不开 = 现有行为零改动。所有 workflow 逻辑走 `if workflow_ctx.is_some()` 分支
- **`run_subagent`/`run_chat_loop` 增参**:用 `Option` 或默认值,现有调用点传 None/默认,不破坏
- **B12 双路径**:workflow session 写 task.json.items;非 workflow session 保持 loop-local Vec 原行为
- **plugin 加载失败 fallback**:workflow.json serde 失败 / validate 失败 → warn + 回退 `default_workflow()`(M6)
- **task.json 损坏**:专用 `update_task` tool 内部 serde 序列化保证 JSON 合法(非裸 write_file,小问题6);agent 不直接手写 task.json

## 7. 风险点(实施时盯紧)

| 风险 | 位置 | 缓解 |
|---|---|---|
| `run_subagent` 签名改动碰三处调用点 | dispatch.rs:233 + chat_loop.rs:1000/2937/3286 | Step 2.4 做完立即跑全量 cargo test |
| B12 checklist 改写路径碰现有测试 | update_checklist.rs:execute | Step 2.6 双路径分支,非 workflow 路径走原逻辑保测试通过 |
| 注入 seam fallback prepend 破坏 cache | memory_recall.rs:271-277 | S-B:engine 校验 messages[0] 是 user-role Blocks,否则降级非 workflow 不走 fallback |
| plugin skill/agent loader 加层影响现有优先级 | skill/loader.rs:482 + subagent/loader.rs:623 | plugin 层只在 workflow session 生效;非 workflow session 不查 plugin 目录 |
