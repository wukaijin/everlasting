## 1. 文档目的

跟 [DESIGN.md](./DESIGN.md) 一样,这是**给自己看的工程决策备忘录**。用来:

- 在写 workflow 代码前,把"engine 由什么组成 / plugin 由什么组成 / 两者怎么分 / 怎么沉淀 spec"想清楚
- 记录关键岔路选/不选的理由(为什么 engine/content 分离、为什么不建 task UI、为什么 hook 用固定逻辑而非脚本、为什么评审流可能需要新通讯原语)
- 给后续每个实施阶段的 PRD 提供单一引用源

讨论中产生的关键决策沉淀到 [IMPLEMENTATION.md §4](./IMPLEMENTATION/decisions.md);本文档不记决策追溯。

> ❓ *待对齐*:凡不能从已确认方向推导、需你拍板的点,用 ❓ 标出。文末 §14 汇总。

---
