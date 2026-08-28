# F2b 定时任务调度模型扩展(两类调度 + 结束条件)

## Goal

F2 交付的 preset 档位(每天 / 间隔分钟 / 每周)扩展为完整调度模型:两类任务
(固定时间触发 / 固定频率触发)+ 结束条件(次数上限 / 结束日期),覆盖
「每小时 / 每天 / 每工作日 / 每周 / 每月」与「每 N 分钟/小时/天/周」。

## 需求

- R1 固定时间档位新增:每小时(设分钟)、每工作日(设时分)、每月(设几号+时分)。
- R2 固定频率档位的单位扩展:每 N 分钟/小时/天/周(纯 UI 换算,后端仍存 `every_min`)。
- R3 结束条件(类型 A · 固定时间):永不结束 / 次数上限 N。
- R4 结束条件(类型 B · 固定频率):永不结束 / 指定日期结束。
- R5 达限/到期后自动停用并保留(UI 显示「已完成」,审计记 completed)。
- R6 wire 双 transport(Tauri command + daemon HTTP)同步扩展,存量行零迁移损失。

## 已裁定决策(2026-08-28 用户确认)

- **D7 每月短月跳过**:day=29/30/31 遇无该日的月份(如 2 月)当月不触发,
  下月同日恢复 —— cron 语义,与既有 DST「跳过不存在时刻」防御一致。
- **D8 完成后自动停用保留**:任务留在列表显示「已完成 N/M」或「已结束」,
  可手动重新启用(重新启用计数清零,与既有「重启用不补跑、基准重置」同语义);
  不自动删除。
- **D9 结束日期含当日**:ends_at = 指定日 23:59:59.999 本地,当天到期点照常触发。
- **D10 后端结束条件两列通用**:`max_runs` / `ends_at` 对所有档位可用;
  UI 按任务类型限制展示(固定时间只出次数、频率只出日期)。

## Out of Scope

- cron 表达式语法(F2 D2 定案不引,档位 additive)。
- LLM `schedule_task` tool / detached dispatch(F2 D5 follow-up)。
- 节假日日历(每工作日 = 周一至五,零依赖)。
- 时区字段(全 Local,沿 F2 定案)。
- fs 事件 / webhook 触发源(F2 D3 follow-up)。

## AC

- AC1 `{"kind":"hourly","minute":30}` 等三新档位可创建,`most_recent_due` /
  `next_fire_display` 判定正确(含 DST、短月)。
- AC2 每工作日档周一至五触发,周末顺延到下周一(判定窗口覆盖周五)。
- AC3 每月 31 号在 2 月/4 月等短月跳过,3 月/5 月恢复。
- AC4 「每 2 小时」「每 3 天」「每 2 周」在 UI 可配,存库为 every_min 换算值。
- AC5 max_runs=N 的任务第 N 次 fire 后自动 enabled=0,审计 completed(max_runs),
  只发一次。
- AC6 ends_at 任务:当日到期点照常 fire;due > ends_at 不 fire 并自动停用;
  停机跨 ends_at 重启后未消费且 <= ends_at 的 due 仍补跑一次。
- AC7 重新启用后 run_count 清零、last_fired_at 重置。
- AC8 存量库升级:scheduled_tasks 三新列 probe+ALTER 幂等,存量任务行为不变。
