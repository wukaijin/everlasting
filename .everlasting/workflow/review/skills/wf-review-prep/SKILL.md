---
name: wf-review-prep
description: intake 阶段方法:发现可用模型 + 用户多选 + 按任务种类推荐评审维度并确认
allowed-tools: []
---

# 评审准备(intake)

intake 阶段你要完成 3 件事:① 发现可用模型 ② 让用户多选评审模型 ③ 按任务种类推荐维度并确认。完成后请用户确认转 reviewing。

## 1. 读 current_task 理解评审对象

先读 task 目录:
- `prd.md`(评审主对象)
- `design.md`(若存在)
- `progress.md`(若存在,了解上下文)

理解这个 task 在评审什么,才能选模型 + 选维度。

## 2. 发现可用模型(关键澄清)

**你(主 LLM)无法直接调 `list_models` 内部 API**。发现可用模型的唯一渠道是 **`dispatch_subagent` 工具的 `model` 参数 enum** —— 该 enum 的 display_name 来自 `list_models` 快照,在 chat_loop 构建 dispatch_subagent 工具时动态填入。

操作:
1. 看 `dispatch_subagent` 工具 schema 的 `model` 参数 enum → 得到可用模型的 display_name 列表
2. 这些 display_name 就是你和用户能看到的全部可用模型

## 3. 用户多选评审模型

用 `askUserQuestion` 让用户从可用模型里多选(multiSelect: true)评审模型。

- 建议跨 provider(不同 provider 的模型盲区不同,分歧价值高),但**非强制** —— 用户决定
- 推荐至少 2 个(单一模型无分歧价值);常见 2-4 个
- 把多选结果**存内存**(本轮 turn 内),reviewing 派 reviewer 时用这些 display_name 作 `dispatch_subagent` 的 `model` 参数

## 4. 按任务种类推荐维度

维度不是写死的,按当前 task 的种类推荐基础维度组合。先判断任务种类(读 prd/title):

| 任务种类 | 基础维度(推荐) |
|---|---|
| 新功能 | 清晰度 / 范围边界 / 可行性 / 验收标准 |
| 重构 | 影响面 / 回归风险 / 兼容性 / 收益论证 |
| Bug | 根因分析 / 复现路径 / 修复方案 / 副作用 |
| 架构 | 抽象合理性 / 扩展性 / 一致性 / 取舍论证 |
| 文档 | 准确性 / 完整性 / 受众适配 / 可操作性 |

## 5. 可选维度池(用户增删)

除基础维度外,可选维度池供用户补充:
- 安全 / 性能 / 可读性 / 可测试性 / 可维护性 / 国际化 / 可观测性 / 一致性(与项目既有约定)

## 6. askUserQuestion 确认维度

用 `askUserQuestion`(multiSelect: true)向用户确认维度:
- 把推荐的基础维度作为已选项
- 让用户增删(用户可选可选维度池里的,也可自定义)
- 最终维度集 = 本轮 reviewing 所有 reviewer 共用的评审标尺

## 7. 完成后申请转 reviewing

维度 + 模型都确认后,用 ask_user_question 申请转 reviewing(用户确认才推进 state)。

## 约束

- intake 主 LLM 自己干(无子代理),不派 reviewer(reviewer 是 reviewing 阶段才派)
- 不要在 intake 修订 prd —— 修订是 revising 阶段的事
- 模型多选结果存内存即可,不必落盘(派 reviewer 时直接用)
