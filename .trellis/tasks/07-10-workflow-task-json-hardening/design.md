# Design — workflow-task-json-hardening

## 1. 总体策略

五项改动按「止血 → 补合规入口 → 修读侧冻结 → 文案」顺序,每项独立可验证、独立提交:

| # | 改动 | 文件 | 风险 |
|---|---|---|---|
| R1 | read_task lenient 解析 | `task.rs` | 低(纯解析层) |
| R2 | create_task LLM tool + workflow 过滤 | `tools/create_task.rs` + `tools/mod.rs` + `chat_loop.rs` | 中(新 filter 机制) |
| R3 | transition 拦截即时 resolve | `chat_loop.rs` + `inject.rs` | 中(pub 化 helper) |
| R4 | breadcrumb 注入即时 resolve | `chat_loop.rs` + `inject.rs` | 中(读侧统一) |
| R5 | bootstrap hint 文案 | `inject.rs` | 低 |

R3 与 R4 合并为「**读侧统一即时 resolve**」(见 §5 决策),不改动 `workflow_ctx` 的可变性 —— 这是本设计的关键取舍,避开了高风险的 `&mut` 借用改造。

## 2. R1 — read_task lenient 解析

**方案**:给 `TaskStatus` 加自定义 `Deserialize`(delegate to `from_str_opt`,非法→Planning),给 `TaskJson.created_at`/`updated_at` 加 `#[serde(default)]`(默认空串),给 `TaskItem.content` 加 `#[serde(default)]`。

**为什么不用「read_task 内 best-effort 二次解析」**:那样要维护两套解析(严格 serde + 手动 Value 补 default),且 `resolve_current_task` / `set_task_state` / `update_checklist` 多处调 `read_task` 都应受益。在**类型层**加 default + 自定义 Deserialize 是单点修复,所有 read_task 调用点自动受益。

**TaskStatus 自定义 Deserialize**(参照 `def.rs::Coordination` 的范式):
```rust
// 去掉 derive 的 Deserialize,保留 Serialize + 其他 derive
impl<'de> serde::Deserialize<'de> for TaskStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str_opt(&s))  // 非法 → Planning
    }
}
```
顶层 `status`(task 级)与 `items[].status` 都用 TaskStatus,**一处修复全覆盖**。

**created_at/updated_at default = 空串 `""`**:read_task 是 sync、无时钟注入,空串对下游无害(breadcrumb 不显示时间;archive 不依赖 created_at,它用自己的时间戳)。`write_task`/`create_task_init` 写出的仍是真实 RFC3339(default 只兜底手写,不影响内部写路径)。

## 3. R2 — create_task LLM tool

**新文件 `tools/create_task.rs`**:
- `definition()`:name=`create_task`,input_schema = `{title, slug, parent?}`(project 路径从 `ctx.worktree_path` 取,LLM 不传)。slug 校验 `[a-z0-9-]{1,64}`。
- `execute(input, ctx) -> (String, bool)`:从 `ctx.worktree_path` 取 project_path,调 `create_task_init(project_path, title, slug, parent)`,返回 fresh TaskJson 摘要 + `[persist] → task.json created` 提示。错误(`AlreadyExists`/`InvalidSlug`)→ `is_error:true` + 中文消息。
- 注册:`tools/mod.rs::builtin_tools()` 加 `create_task::definition()`;`execute_tool_inner` 的 `match name` 加 `create_task` arm(非 blocking,走正常分发,与 read_file 同级)。

**不拦截 chat_loop**:create_task 无需 QuestionStore / cancel,正常 execute_tool 即可。

**与 IPC 共存**:`commands/task.rs::create_task` IPC 保留(前端驱动),tool 是 LLM 路径。两者都调 `create_task_init`(单一写入真源)。

**不进并行集**:`is_parallel_eligible` 不含 create_task —— 建档串行(与其它写类一致)。

**可见性策略(2026-07-10 决策:新增 workflow 过滤)**:不走「全局可见 + 执行 gate」(那是 request_task_state_transition 的现状,有 schema 污染缺陷 —— 非 workflow session 的 LLM 看到 schema、误调才报错)。新增 `filter_tools_for_workflow(tool_defs, workflow_enabled) -> Vec<ToolDef>`,白名单过滤 workflow-only 工具集 `{create_task, request_task_state_transition}`:仅 `workflow_enabled=true` 的 session 保留这俩,非 workflow session 从 `turn_tool_defs` 剥掉。
- **放哪**:`tools/mod.rs`(tool 集合的可用性过滤,不涉权限决策;与 `builtin_tools` 同文件,对称于 `filter_tools_for_mode`/`filter_tools_for_subagent`)。
- **调用点**:`chat_loop.rs:1540`,与 `filter_tools_for_mode` 链式:`filter_tools_for_workflow(filter_tools_for_mode(tool_defs, mode), workflow_enabled)`。
- **顺带收编 `request_task_state_transition`**:行为一致化 —— 它现在的「执行 gate」(`execute_blocking` 里 `current_slug=None` → is_error)可保留作 defense-in-depth,但 schema 层先过滤掉更干净。
- **worker 侧**:`filter_tools_for_subagent` 的 `STRUCTURALLY_DISABLED` 集补 `create_task`(worker 不该跨 session 建档)。

## 4. R3 — transition 拦截即时 resolve

**pub 化**:`inject.rs::resolve_current_task`(当前私有 async fn)改为 `pub`,或在 `workflow/mod.rs` re-export 一个 pub helper。二者皆可;倾向直接 pub(`resolve_current_task(project_path)`)。

**chat_loop.rs:3426 改写**(伪代码):
```rust
let project_path = current_ctx.worktree_path.clone();
let fresh = crate::agent::workflow::resolve_current_task(&project_path).await;
let (current_state, current_slug) = match fresh {
    Some(t) => (Some(t.status), Some(t.slug.clone())),
    None => (None, None),
};
```
不再读冻结的 `workflow_ctx.current_task`。

**为何不直接用 ctx**:ctx 是 IPC 入口快照,agent 可能在本 loop 内 create_task/update_checklist 改了盘,ctx 过时。transition 是状态机门控,必须基于盘上真实状态 —— 与 apply 侧 `resolve_task_state_transition` 的 "read fresh off disk"(`question.rs:695`)一致。

## 5. R3+R4 决策 —— 读侧统一即时 resolve(不 mut ctx)

**问题**:`workflow_ctx: &Option<WorkflowCtx>` 在 `run_chat_loop` 内是不可变借用,transition 成功后无法就地改 `current_task`,导致 breadcrumb(ctx 驱动)读不到新状态。

**方案 A(弃用)**:改 `&mut Option<WorkflowCtx>`,在 transition/create_task 成功后重 resolve 写回 ctx。**风险高** —— workflow_ctx 在 loop 内多处被 `&` 借(`append_workflow_breadcrumb`、subagent dispatch 的 `workflow_ctx.as_ref()`、loop_detection 等),改 mut 要审计全部借用点,易出错。

**方案 B(采用)**:不改 ctx 可变性,而是让**所有读 current_task 的热路径**都即时 resolve,不读 ctx 快照:
- transition 拦截(R3)即时 resolve ✓
- breadcrumb 注入(R4):`append_workflow_breadcrumb` 改成接收「即时 resolve 的 current_task」而非整个 ctx。具体:在调用处(`chat_loop.rs` 每 turn 注入点)先 `resolve_current_task` 得 fresh,传 fresh task 给一个新签名 `append_workflow_breadcrumb(&mut msgs, &workflow_def, &Option<TaskJson>)`。

**性能**:`resolve_current_task` 是 `read_dir` + 每文件小 parse(task 目录通常 0-1 个非终态 task),每 turn 一次,成本可接受。注意它**不读 DB**(DB 部分是 `build_workflow_ctx` 的 `workflow_enabled` gate,已在 IPC 入口做完;resolve_current_task 本身纯文件)。

**保留 ctx.current_task**:delegation template、subagent dispatch 等不频繁路径仍可读 ctx.current_task(它们对「刚 transition 完」不敏感);只有状态机门控(transition)+ 状态提示(breadcrumb)这两个「对最新状态敏感」的读点改即时。这样改动面最小。

## 6. R5 — hint 文案(软推荐,不禁止 write_file)

**2026-07-10 设计修正**:原方案"禁止 write_file task.json"与 R1 lenient 哲学冲突 —— R1 已在读取侧让 write_file 不崩,R5 再禁就自相矛盾;且真"禁止"得在 write_file path guard 特判 `.everlasting/tasks/*/task.json`,过度严格(妨碍 LLM 灵活操作)+ 失去扩展性(未来 schema 加字段,LLM 无法直接写,得等 tool 跟进)。收敛为「**软推荐 + 读取侧韧性兜底**」:

`inject.rs:620-625` bootstrap branch 改为:
```
no active task — call the create_task tool to start one (省事、字段全、自带 prd skeleton)。
(write_file 也行,但 task.json schema 有约束;read_task 会 lenient 兜底,优先用 create_task tool 更稳。)
```

create_task tool 定位为「便捷建档助手」(**非唯一合规路径**):价值 = 省 LLM token + 生成完整模板(task.json + prd.md skeleton,write_file 做不到)。description 说明"推荐用它建档,write_file 也可,read_task 兜底解析"。**韧性在读取侧(R1),不在写入侧设卡。**

`inject.rs` tests 的 bootstrap hint 断言同步更新(含 "create_task",去 "IPC";不再断言"禁 write_file")。

## 7. 兼容性 / 回滚

- R1 的 lenient 是**纯放宽**(合法 JSON 更易解析),不会让原本合法的路径失败。回滚 = revert TaskStatus 自定义 Deserialize + 去掉 default。
- R2 新 tool,不改现有 tool 行为。
- R3/R4 读侧即时 resolve,写侧(`set_task_state`)不变。
- R5 纯文案。
- 每项独立 commit,任一 revert 不影响其余。**R1 是最关键止血** —— 若时间紧,只做 R1 也能让 workflow 不再崩(但 LLM 仍会手写,只是不再致命)。

## 8. 测试策略

- R1:`task.rs` tests 加 lenient case(缺 created_at / 缺 updated_at / item status=in_progress / item status=pending / item 缺 content)。
- R2:`tools/create_task.rs` 内联 tests(建档成功 / AlreadyExists / InvalidSlug)。
- R3/R4:测 pub helper `resolve_current_task` 的现有 inject.rs tests 已覆盖核心;chat_loop 侧若无合适集成测试夹具,以 pub helper 单测 + 手动 e2e(improve-readme 场景)兜底。
- R5:更新 inject.rs bootstrap hint 断言。
