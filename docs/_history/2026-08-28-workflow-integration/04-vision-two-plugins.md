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
>
> **立项 trigger(小问题4)**:dev plugin 跑通 ≥1 个完整 task(planning→done 全程)+ 沉淀 spec 起见过收益后,再决定评审流立项时机 + A/B 路径。即"先验证 dev 机制闭环有效,再投入第二个 plugin"。

### 4.3 plugin 化的最大价值就在这里

两个流程结构根本不同(线性 pipeline vs 迭代 fan-out/gather)。**engine 把任何一个写死,另一个就塞不进去。** 只有 engine 是"读配置驱动的 state machine + 支持多种协调模型",两者才能作为可切换 plugin 共存。这就是 §2.2 说的"plugin 化不是为了灵活而灵活,是为了让根本不同的工作流共享同一个 engine"。

---
