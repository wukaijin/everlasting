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