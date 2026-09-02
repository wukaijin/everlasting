---
name: wf-brainstorm
description: planning 阶段调研 + 写 prd + 拆 checklist 的方法指导
allowed-tools: []
---

# 调研与需求拆解(planning)

## 调研方法
- 先 dispatch researcher 调研技术方案、踩坑点、相关 spec
- 调研要覆盖:现有实现、备选方案、风险、相关 `.everlasting/spec/` 规范

## 调研落盘
- researcher 返回 findings 后,**由你(主 LLM)把结论落盘**到 `.everlasting/tasks/<slug>/research/<topic>.md`,一个主题一个文件——对话会被压缩,文件不会;prd.md 技术方案节引用这些文件
- 回环场景(从 in_progress 退回 planning)追加写新文件,不覆盖已有调研

## 写 prd.md
> **注意**:prd 可能已被 review session 修订过(回环场景),写之前先 `read_file` 读最新 prd,不要假设你是在从零写。
- 背景 / 目标 / 非目标 / 技术方案 / 验收标准 / 风险
- prd 是给后续 in_progress(implement + check)看的,要可执行(明确"做完什么样算 done")

## 拆 task.json.items(实施阶段)
- 按 task 复杂度拆,**不要预定义模板**(每个 task 拆法不同)
- 例:后端实施 → 后端测试 → 前端实施 → 前端测试 → 联调 → 端到端
- 每项标是否 tdd(逻辑改动标 tdd,文档/配置/重命名不标)
- 拆完写 `.everlasting/tasks/<slug>/relevant-specs.jsonl` 策展清单:每行 `{"file": "<repo 相对路径>", "reason": "<为什么相关>"}`,收录本次相关的 spec 与 research 文件,给 implementer/checker 的 delegation 注入(`{relevant_specs}`)用
- 跟用户对齐后,用 `request_task_state_transition` 申请切 in_progress

## 派 implementer 前须知
- `.everlasting/tasks/` 整体 gitignored,implementer 跑在隔离 worktree 里,**结构性看不到** research/ 与 relevant-specs.jsonl(commit 也救不了)
- 对策:派 implementer 前,把 research 关键结论**摘要进 delegation 文本**(填 `{summary}` / task 文本)——这是唯一可靠通道;spec 文件不受影响(`.everlasting/spec/` 已提交即在 worktree)