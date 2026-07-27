---
name: wf-synthesize
description: revising 阶段方法:综合 N 份评审 + triage 决策 + 修订 prd + 写 review-state.json + convergence 评估 + 问用户循环
allowed-tools: []
---

# 综合与修订(revising)

revising 阶段你综合 N 份 reviewer 输出,做 triage 决策,修订 prd,写 review-state.json,然后问用户「再评一轮还是定稿」。**按 6 步顺序执行,不要漏步**(漏写 review-state.json 是最常见错误)。

## 步骤 1:综合(按维度横向对比)

按维度横向对比 N 份 reviewer 输出:
- **标注分歧**:同维度不同模型看法(如模型 A 说「范围清晰」,模型 B 说「范围模糊」) —— 分歧处是价值所在
- **提炼共识**:多模型共同指出的问题(高置信)
- **去重**:多个模型提同一问题,合并成一条 finding

每条 finding 记录:
- `dimension`:维度名
- `severity`:critical / high / medium / low / info
- `issue`:问题描述
- `suggestion`:建议(来自 reviewer,或你提炼)
- `location`:位置(如 prd.md§2)
- `source_run_id`:来自哪个 reviewer run(跳转原始 final_text,用 source_run_id 非 display_name 做稳定 key)

## 步骤 2:triage 决策(评审回流,关键)

对每条 finding 标 triage 决策(adopt / reject / defer)+ reason:

- **adopt**:采纳,本轮回写进 prd
- **reject**:拒绝 —— 评审者常缺决策上下文,其"合理建议"可能撞墙于项目已知约束(brainstorm 决策、项目既定方向)。reject 要写**对照已知约束的理由**
- **defer**:暂缓(本轮不处理,记入 follow-up)

**triage 是主 LLM 的职责,不是 reviewer 的**。你带 brainstorm 上下文做判断,不能照单全收 reviewer 意见 —— 这是评审回流的核心(源于外部评审实践)。

## 步骤 3:修订 prd

据 adopt 的 finding 修订 prd.md(以及 design.md,若相关):
- 你在 workflow_enabled 时有写工具(`write_file` / `update` 等),直接改
- 修订点记入本轮 `change_log`(供 review-state.json + 下轮 reviewer resume 时的「changes_since_last」)

**若 prd 可能已被 review session 修订过(回环场景),修订前先读最新 prd**,不要假设你记忆里的版本是最新的。

## 步骤 4:写 review-state.json(C2 视图数据源,跨任务契约)

用通用 `write_file` 工具写 `<task>/review-state.json`(`<task>` = current_task 目录)。**每轮 revising 重写整个文件**,`rounds` 数组累积历轮(不丢失历史)。

**schema**(字段名英文,字段值跟随 prd 语言):

```jsonc
{
  "schema_version": "1.0",
  "task_id": "<task slug>",
  "current_round": 2,
  "rounds": [
    {
      "round": 1,
      "dimensions": ["清晰度", "范围边界", "可行性"],
      "models_present": ["model-id-a", "model-id-b"],
      "models": {
        "<model_id 稳定 id>": {
          "model_display": "claude-sonnet-4",
          "run_id": "<subagent_runs.id>",
          "status": "completed",
          "verdict": "revise",
          "findings": [
            {
              "finding_id": "r1-m-modela-1",
              "dimension": "清晰度",
              "severity": "high",
              "issue": "...",
              "suggestion": "...",
              "location": "prd.md§2",
              "source_run_id": "<subagent_runs.id>",
              "triage": {
                "decision": "adopt",
                "reason": "命中范围边界缺失,需补"
              }
            }
          ]
        }
      },
      "change_log": ["§2 澄清范围边界(reviewer A 反馈)", "§4 补错误处理(reviewer B)"],
      "convergence_note": "本轮无新增关键问题,建议定稿"
    }
  ]
}
```

**字段细则**:
- `models` 的 key 用 **model_id**(稳定 id),非 display_name
- `status`:running / completed / cancelled / error / incomplete(对齐 DB subagent_runs.status CHECK 约束;失败模型标 error/incomplete)
- `verdict`:pass / pass_with_minor / revise / reject
- `severity`:critical / high / medium / low / info
- `finding_id`:稳定 id(如 `r1-m-modela-1`),便于跨轮追踪同一问题
- `triage.decision`:adopt / reject / defer

**写入约束**(design §4):
- 用通用 `write_file`(非原子 tmp+rename,实际是 tokio::fs::write)。写入频率低(每轮 revising 一次),C2 前端可加「读到的 JSON 解析失败则忽略本轮」兜底。**不在本任务改 write_file 全局行为**。
- schema 合法性由你(prompt)约束兜底,引擎不强校验。务必按上面字段名/枚举值写。

## 步骤 5:convergence 评估(软引导)

对比本轮 vs 上轮(若非首轮):
- finding 数趋势(下降 = 收敛)
- severity 趋势(high severity 是否消除)
- 是否有**新增关键问题**(无新增 = 强收敛信号)

基于评估,主动给收敛建议(写入 `convergence_note`):
- 强收敛(无新增关键 + high severity 已解决)→ 建议定稿
- 仍有新增关键 → 建议再评一轮

这是**软引导**(非硬 cap)—— 最终由用户决定。

## 步骤 6:askUserQuestion 问用户循环

用 askUserQuestion 问用户「再评一轮还是定稿」,附 convergence_note:
- 选「再评一轮」→ 回 reviewing(你申请转回 reviewing,reviewer 下轮用 `resume_from` 续接;派下轮 reviewer 时构造 resume clarification:`current_state` = 修订后 prd 摘要,`changes_since_last` = 本轮修订点,`this_round_purpose` = 验证上轮 high severity 是否解决)
- 选「定稿」→ 申请转 reported

## 约束

- revising 是主 LLM 自己干(无子代理),不派 reviewer
- 必须写 review-state.json(步骤 4 不可漏,它是 C2 视图的数据源)
- triage 不可照单全收(reject 要对照已知约束)
- prd 修订前先读最新版(回环场景防 stale)
