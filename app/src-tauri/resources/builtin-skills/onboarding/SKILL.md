---
name: onboarding
description: Orient a new user with a capability map of this app (modes, permission approvals, tools, skills, scheduled tasks, group-chat deliberation, worktree multi-project, memory) and the shortest path to start a first project. Use when the user is new, asks "what can this app do / where do I start / 带我逛逛", or seems lost about features.
allowed-tools: []
---

# 新手引导（onboarding）

给新用户一张能力地图 + 开第一个项目的最短路径。点到为止：具体操作以界面为准，不要展开细节（功能会演进）。

## 能力地图

- **三档模式**（会话级，状态栏可切）：`edit`（默认——写操作逐次审批）/ `plan`（只读调研，先出计划再动手）/ `yolo`（自动执行写操作，慎用）。
- **权限审批**：写文件、shell、网络等敏感操作在 edit 模式会弹审批卡，同意/拒绝都在你手里；操作有审计记录。
- **工具族**：文件读/写/编辑、grep / glob / list_dir、shell（前台 + 后台）、web 搜索与网页抓取、跨会话历史检索（search_history）、技能加载（use_skill）等。
- **skill 发现**：输入 `/` 弹出面板（内置 + 用户 + 项目层）；任务描述匹配到某个 skill 时模型也会自动加载。本会话内置可用：`llm-setup`（配模型）、`doctor`（排障）、本 skill。
- **定时任务**：到点自动跑 agent 任务（Settings → 定时任务创建；模型也能用 schedule_task 家族工具创建/查询/取消）。注意：每场任务消耗 token，周期别设太密。
- **群聊审议**：多个模型围炉讨论拿共识（建群、发议题、跨模型交叉验证），适合评审、架构决策、复盘。
- **worktree 多项目**：不同项目各自独立；同一项目可开多个 worktree 会话并行干活互不干扰。
- **记忆系统**：模型可把经验/决策沉淀为长期记忆，跨会话自动召回；也可用 search_history 直接搜历史对话原文。

## 开第一个项目

1. 新建项目，指向一个本地目录作为工作区根。
2. 确认模型已配好：没配 → 加载 `/llm-setup`；发不出消息 → 加载 `/doctor`。
3. 在会话里描述任务：小任务直接说；大任务建议先切 plan 模式出方案，确认后再回 edit 动手。

## 交叉指引

- 配模型 / 换提供商 / 改 base_url → `/llm-setup`。
- 报错 / 连不上 / 行为异常 → `/doctor`。
