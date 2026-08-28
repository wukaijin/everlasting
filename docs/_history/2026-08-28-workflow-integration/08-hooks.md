## 8. hook:Everlasting 没有 hook runner

**现状**(code 查证):Everlasting Rust 后端**没有 task.json hooks / 生命周期脚本执行器**。Trellis 的 `after_create/after_start/after_finish/after_archive` 是 Python 脚本,跑在 `.trellis/` 体系里,与 app 后端无关。

| 方案 | 描述 | 成本 |
|---|---|---|
| (a) 不做 hook,全靠 skill + agent 主动 | task 文件写入/state 转移动作由 agent 在 skill 指导下主动调用 | 零新机制,但太软(沉淀闭环会失效) |
| **(b) Rust 固定逻辑 hook**(Q9 选定) | state 转移时 Rust 跑固定动作(done 时触发沉淀、planning→implement 时自动前置检查) | 中 |
| (c) 新建 hook runner | 仿 Trellis 配脚本,state 事件触发执行 | 高(新子系统+安全面) |

> **Q9 决定(2026-07-07)**:选 **(b) Rust 固定逻辑 hook**。理由:(a) 太软,沉淀闭环(task done → 写 spec)是机制价值保证,不能靠 agent 自觉——agent 可能忘写 spec,闭环断;`wf-update-spec` skill 教 agent 怎么写,但"触发写"这个动作不能交给 agent 发挥。(c) 过重,脚本执行 = 任意代码执行,违背 [DESIGN.md "本地优先"](../../DESIGN.md#22-关键约束) 不爱外部脚本执行;为几个固定动作造整个脚本 runner 杠杆不足。(b) 几个关键 state 转移动作写成 Rust 固定代码,改动作要改代码,但这几个动作低频稳定。
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
> **hook 触发路径(M2 + M-A 评审修正,拍板 1 选 i)**:`set_task_state` **由专用 IPC 自动调用,不由 agent tool 调用**。理由(对标 Q9"不靠 agent 自觉"):若由 agent 调用,agent 可能忘记 → 沉淀闭环断。
>
> **M-A 修正**:原 M2 写"复用 ask_user_question 的 resolve handler"——但 `resolve_tool_question`(commands/question.rs:92)当前签名 `(session_id, tool_use_id, answer, cancelled)` **无字段区分"普通询问" vs "state 转移申请"**;`question_store.rs::resolve`(line 387)仅 oneshot 送答案无副作用。两种在现有系统是同一种 tool,形态无法区分。强行在 ask_user_question resolve 里加分支 = 污染通用问答 tool 的 schema。
>
> **决定(拍板 1 选 i)**:**新开 IPC `resolve_task_state_transition`**,对标现有 `resolve_mode_change`(commands/question.rs:137-176)的双 IPC pattern——该 pattern 已验证"apply 副作用 + resolve oneshot 同一处收口"(line 140-150 设计注释明说"apply mode BEFORE resolve")。state 转移确认门走同款:
>
> ```
> agent ask_user_question("确认进 implement?", purpose="task_state_transition")  ← 普通问答 tool,带 purpose 标记
>   → user 确认
>     → 前端调 resolve_task_state_transition IPC(非 resolve_tool_question)
>       → engine: set_task_state(task, Implement)   ← apply 副作用,自动
>         → write_task_json + hook 分支(preflight_implement_check)
>       → resolve oneshot(把答案送回 agent turn)
> ```
>
> **跟 `resolve_mode_change` 的差异**:mode change 是改 session 权限旋钮;task state transition 是改 task 生命周期 + 触发 hook。两者结构同款(双 IPC + apply-before-resolve),但作用对象不同,故各开一个 IPC 而非共用。
>
> **agent 侧**:agent 仍用 `ask_user_question` 发起确认(purpose 字段标识"这是 state 转移申请"),前端按 purpose 路由到 `resolve_task_state_transition`。agent 不调 `set_task_state`,只发起申请。跟 §6.6.2 门控一致(engine 主动作为,agent 只申请)。

---
