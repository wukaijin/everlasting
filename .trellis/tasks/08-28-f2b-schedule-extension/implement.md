# F2b 实施计划(2026-08-28,经用户批准)— 执行记录

全部 WP 已完成,验证门全绿:

- `cargo test -p everlasting --lib`:2072 passed / 0 failed(sched 组 57:
  compute 24 + tick 17 + db 11 + routes 3 + lost 等)。
- `cd app && pnpm test`:96 文件 1328 用例全绿(F2b 新增 19 例)。
- `vue-tsc --noEmit` 零错误;`cargo clippy`(本次相关文件)零告警
  (存量 at_file/budget/trace 等告警非本次引入);`cargo fmt` 已过。

## WP0 任务登记 ✅

## WP1 compute.rs 三新档位 ✅

- `Hourly { minute }` / `Weekdays { at }` / `Monthly { day, at }` 变体;
  validate(minute 0-59 / day 1-31 / at 走 parse_hh_mm)。
- most_recent_due 回看:hourly 0..=2 钟点、weekdays 0..=3 天跳周末、
  monthly 0..=2 月(from_ymd_opt 无效跳过该月,D7);next_fire_display 镜像。
- 测试:三档窗口边界 / 两函数一致 / parse 矩阵 / monthly 短月跳过
  (确定性锚点 local_ms,固定工作日避开 DST 切换日)。

## WP2 结束条件 ✅

- schema.rs CREATE TABLE 三列 + columns.rs probe+ALTER(存量库升级)。
- db 层:三列入 Row/New/SELECT/insert;Update 双层 Option<Option<i64>>;
  mark_task_fired 带 count_fire(dedup 不计数);mark_task_completed 只翻
  enabled;false→true 重置 run_count=0。
- 命令层:create/update 加 max_runs/ends_at(校验 ≥1 / > now),
  `#[allow(clippy::too_many_arguments)]`(providers.rs 先例,扁平标量铁律)。
- daemon DTO:create 直加;update serde double-option(缺省=不动,
  null=清空,map(Some) helper)。
- tick 四道 gate:①达限完成;②ends_at 过且无未消费 due 完成;③due>
  ends_at 不 fire 完成(反之照常 fire,含 catchup,D9);④fire 后达限或
  next>ends_at 即时完成。actions::COMPLETED + completion_reasons。
- 前端 audit:AuditLogItem「已完成」+ reason 人话(达次数上限/已达结束日期)。

## WP3 前端 ✅

- store:union 三新分支;ScheduledTask 三新字段;create/update 入参
  maxRuns/endsAt(null 显式清空)。
- ScheduledTasksTab:kind 6 档(KIND_OPTIONS);随档位字段(hourly 分钟 /
  weekdays 时分 / monthly 几号+时分 / interval 数量+单位换算
  [splitEveryMin 回填取能整除的最大单位]);结束条件 radiogroup
  (固定时间:永不/次数;频率:永不/日期[提交转当日 23:59:59.999 本地]);
  切换类型时 endMode 回落 never;update 恒显式带两字段防旧值残留;
  卡片「已触发 N/M 次 · 至日期」+「已完成/已结束」徽章(区别手动停用)。
- scheduledTaskFormat:describeSchedule 三新档 + 单位人话;INTERVAL_UNITS /
  splitEveryMin / describeRunCount / describeEndDate / completedBy*。
- 测试:tab 7 新例(档位 JSON、单位换算提交+回填、结束条件 args、校验、
  完成态卡片)+ format 8 新例 + store 2 新例。

## WP4 验证收口 ✅

- 全量测试见顶部;spec backend/scheduled-tasks.md 增补 §F2b(窗口表、
  四道 gate、计数语义、double-option wire、Bad 案例、测试清单);
  ROADMAP §1.2 F2b 行 + §2.4 F2 条目注记。
