---
name: wf-brainstorm
description: planning 阶段调研 + 写 prd + 拆 checklist 的方法指导
allowed-tools: []
---

# 调研与需求拆解(planning)

## 调研方法
- 先 dispatch researcher 调研技术方案、踩坑点、相关 spec
- 调研要覆盖:现有实现、备选方案、风险、相关 `.everlasting/spec/` 规范

## 写 prd.md
- 背景 / 目标 / 非目标 / 技术方案 / 验收标准 / 风险
- prd 是给后续 in_progress(implement + check)看的,要可执行(明确"做完什么样算 done")

## 拆 task.json.items(实施阶段)
- 按 task 复杂度拆,**不要预定义模板**(每个 task 拆法不同)
- 例:后端实施 → 后端测试 → 前端实施 → 前端测试 → 联调 → 端到端
- 每项标是否 tdd(逻辑改动标 tdd,文档/配置/重命名不标)
- 拆完用 ask_user_question 跟用户对齐,再申请切 in_progress