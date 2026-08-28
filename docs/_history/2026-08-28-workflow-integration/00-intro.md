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
> 需求边界见 [DESIGN.md](../../DESIGN.md),架构见 [ARCHITECTURE.md](../../ARCHITECTURE.md),路线图归 [ROADMAP.md](../../ROADMAP.md),决策追溯走 [IMPLEMENTATION/decisions.md](../../IMPLEMENTATION/decisions.md)。

---
