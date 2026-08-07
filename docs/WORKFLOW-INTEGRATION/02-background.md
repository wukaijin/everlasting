## 2. 背景与动机

### 2.1 现状:零件齐了,缺"机制"把它们串成标准流程

[ROADMAP §1.2](./ROADMAP.md#12-路线图外完成) 显示工作流的 ~80% 零件已落地,但它们目前是**被 agent 随机使用**的——agent 想起就 dispatch_subagent、想不起就自己一把梭。缺的是一个**机制**:在 session 打开 workflow 后,自动驱动 agent 按固定标准使用这些零件。

| engine 能力 | 已有零件(可复用) | 缺口(机制) |
|---|---|---|
> 注:本表引用 `.trellis/spec/` 路径是过渡期(本项目用 Trellis 开发,现有 spec 在那)。`.everlasting/spec/`(Q7)起来后**过渡期两份共存**(各自管各自职责:Trellis spec 管工具用法,Everlasting spec 管项目代码规范),不是"现有引用逐步迁移"——两目录独立演进。
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
