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

**M-A 修正**:确认门走专用 IPC `resolve_task_state_transition`(对标 `resolve_mode_change` 双 IPC pattern),agent 用 ask_user_question 发起(purpose 标记),engine 在 IPC 内 apply `set_task_state` + resolve oneshot。agent 只申请不执行(见 §8)。

### 10.7 注入一律 append 到 messages[0],不碰持久化

**决策**:所有 workflow 注入 append 到 per-turn request clone 的 messages[0],`cache_control: None`,绝不新开 user message。保 Anthropic prompt cache breakpoint 不失效(5-10× 成本)。这是 [`memory_recall`](../.trellis/spec/backend/memory.md) + [B12 checklist](../.trellis/spec/backend/agent-loop-architecture.md) 已验证的硬规则。

**S-B 评审加注**:`inject_recall_into_turn` 在 `messages[0]` 非 user instruction 时有 **fallback 分支会 prepend 新建 synthetic user message**(memory_recall.rs:259-278),破坏 cache。**本工作流不允许触发该 fallback**——workflow session 默认 B5 指令文件加载保证 messages[0] 是 user-role Blocks message(见 §6.6.1 前置约束);`load_for_session` 返回空 layers 的 workflow session 属配置异常,engine warn + 降级非 workflow 行为,不走 fallback prepend。

### 10.8 UI workflow 切换 ≠ task picker

**决策**:UI 可切 workflow plugin(选"怎么干活"),但无 task picker(否决"选干哪个 task")。两者性质不同。

---
