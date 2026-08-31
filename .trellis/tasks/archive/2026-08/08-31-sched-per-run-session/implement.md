# Implement: sched-per-run-session

执行顺序(每步可独立编译/测试):

1. [ ] DB:schema.rs greenfield DDL 新形 + `schema_helpers::rebuild_scheduled_tasks_for_target_mode` + `run_migrations` 接线;`db/scheduled_tasks.rs` 行/载荷/CRUD Option 化 + 三新列 + `target_modes` 常量 + `mark_task_fired` 增参;db 单测。
2. [ ] 调度器:`scheduler/mod.rs` tick 内 target resolve + `create_run_session` + account/audit 签名扩展;tests_tick 增 per_run 用例。
3. [ ] 命令层:`commands/scheduled_tasks.rs` create/update 增参 + 校验;Tauri command;`daemon/routes/scheduled_tasks.rs` DTO + double-option-string helper;route 测试;`tools/scheduled_task_family.rs` 两处读点适配。
4. [ ] 前端:`stores/scheduledTasks.ts` 类型/入参;`ScheduledTasksTab.vue` 目标区重设计 + 列表卡 meta;测试更新。
5. [ ] 全量验证:`cargo test -p everlasting --lib` + `cd app && pnpm test` + clippy + vue-tsc。
6. [ ] spec 更新:`.trellis/spec/backend/scheduled-tasks.md` 增 per_run 契约节。

回滚点:每步一个 commit;迁移向后兼容(旧代码读新表:多余列被 SELECT 显式列名忽略,`target_session_id` NULL 行对旧代码不可见——仅 per_run 行,可接受)。
