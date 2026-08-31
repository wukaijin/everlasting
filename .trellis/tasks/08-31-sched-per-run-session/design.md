# Design: sched-per-run-session

## 1. 数据层

### 1.1 表结构(greenfield DDL 与 rebuild DDL 保持逐字一致)

```sql
CREATE TABLE IF NOT EXISTS scheduled_tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  target_session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE, -- 可空化:per_run 恒 NULL
  target_mode TEXT NOT NULL DEFAULT 'fixed',      -- 'fixed' | 'per_run'
  name TEXT NOT NULL,
  prompt TEXT NOT NULL,
  schedule TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_by TEXT NOT NULL DEFAULT 'user',
  created_at INTEGER NOT NULL,
  last_fired_at INTEGER,
  next_fire_at INTEGER NOT NULL,
  run_count INTEGER NOT NULL DEFAULT 0,
  max_runs INTEGER,
  ends_at INTEGER,
  model_id TEXT,                                  -- per_run 每次建 session 的模型绑定
  last_run_session_id TEXT,                       -- 最近一次 run session(无 FK,防级联删任务)
  CHECK (target_mode = 'fixed' OR target_mode = 'per_run'),
  CHECK (target_mode = 'per_run' OR target_session_id IS NOT NULL)
);
```

### 1.2 迁移(`schema_helpers::rebuild_scheduled_tasks_for_target_mode`)

沿 `rebuild_turn_trace_with_run_id` 五步舞:probe 表存在 → probe `target_mode` 列
(存在即短路)→ residue guard → 事务内 rename→create 新形→显式列 copy
(`target_mode='fixed'`, `model_id=NULL`, `last_run_session_id=NULL`)→drop old→
recreate `idx_scheduled_tasks_due`。调用点在 `run_migrations` 的 F2b add-column
之后(保证旧库 run_count/max_runs/ends_at 已补齐,copy 可引用)。

## 2. Rust 行/载荷

- `ScheduledTaskRow`:`target_session_id: Option<String>` + 新三列
  `target_mode: String` / `model_id: Option<String>` / `last_run_session_id: Option<String>`。
- `NewScheduledTask` 同步;`UpdateScheduledTask.target_session_id: Option<Option<String>>`
  (double option:缺省不动 / `Some(None)` 清空 / `Some(Some)` 设置)+ `target_mode: Option<String>`
  + `model_id: Option<Option<String>>`。
- `mark_task_fired` 增参 `last_run_session_id: Option<&str>`,SQL 用
  `COALESCE(?, last_run_session_id)`(None = 保留旧值;dedup 路径传 None)。
- 常量:`db::scheduled_tasks::target_modes::{FIXED, PER_RUN}`。

## 3. 调度器

tick 循环内,fire 前 resolve 目标(替换现直取 `task.target_session_id`):

```
fixed( Some(sid) ) → 直接用 sid;走既有 defer(fired_sessions)/ dedup / queue-gate
per_run            → 跳过 defer / dedup / queue-gate(AC5 依据:全新空闲 session,
                     legacy cancel+replace 无害;队列去重无共享队列)
                   → create_run_session(db, task, now):
                       project.path ← db::get_project
                       create_session_in_pool(db, project_id, path, None, None, None)
                       rename_session(title = "{name} {format_local_hhmm(now)}")
                       model_id Some → update_session_model_id
                     Err → audit ERROR(reason=session_create_failed,锚 last_run_session_id,
                            NULL 则跳写)+ account(count=true, run_session=None)+ gate4 + continue
                   → ctx.target_session_id = 新 sid
```

- `FireContext` 形状不变(`target_session_id: String`)——fire seam 类型、
  `fire_via_chat_inner`、全部测试替身零改动。
- `account(...)` 增参 `run_session: Option<&str>` 透传给 `mark_task_fired`
  (per_run 真实 fire 传 Some(new sid) → 落 `last_run_session_id`)。
- `audit_task` 签名改为显式 `session_id: Option<&str>`;None → tracing warn 跳写。
  `complete_task` 锚点 = `target_session_id.or(last_run_session_id)`。
- 循环尾 `fired_sessions.insert` 仅 fixed。

## 4. 命令层(create / update / route / Tauri)

### create_scheduled_task_in_pool 增参 `target_mode: Option<String>`

- 归一化:None/""/"fixed" → fixed;"per_run" → per_run;其它 → 400
  「target_mode 只支持 fixed / per_run」。
- per_run:`target_session_id` 必须为空(Some 非空 → 400);`model_id` 校验存在后
  存任务行(**不**建任何 session)。fixed:既有两分支不变(显式 sid 校验归属 /
  缺省建专用 + session 级模型覆盖)。

### update_scheduled_task_inner 增参 `target_mode: Option<String>` +
`target_session_id: Option<Option<String>>`(wire double option)+
`model_id: Option<Option<String>>`

- resolved_mode = patch.target_mode 或存量。
- → per_run:db 层 target 写 `Some(None)`(清空);同时带 `Some(Some(sid))` → 400 矛盾。
- → fixed:target patch `Some(Some(sid))` 且 ≠ 存量 → 既有三校验;patch 缺省且存量
  target 为 NULL(per_run 切回未选)→ 400「请先选择目标 session」;`Some(None)` → 400。
- wire:`UpdateScheduledTaskRequest.target_session_id` 用新
  `deserialize_double_option_string`;Tauri command 形参同步。

## 5. LLM tool(`tools/scheduled_task_family.rs`)

`create_scheduled_task_in_pool` 调用点补 `None`(target_mode);`row.target_session_id`
Option 化的两处读点(`unwrap_or_default` / `json!` 直吞 Option)。行为不变(AC10)。

## 6. 前端

### stores/scheduledTasks.ts

- `ScheduledTask`:`target_session_id: string | null` + `target_mode: 'fixed'|'per_run'`
  + `model_id: string | null` + `last_run_session_id: string | null`。
- Create 输入增 `targetMode?: 'per_run'`;Update 输入增 `targetMode?` /
  `targetSessionId?: string | null`(null = 清空,wire 显式 null)/ `modelId?: string | null`。

### ScheduledTasksTab.vue 目标 session 区重设计

- `form.targetMode: 'existing' | 'dedicated' | 'per_run'`(创建三档;编辑两档,
  dedicated 档不出现——专用 session 本质是 fixed)。
- reka `RadioGroupRoot` 卡片组(每卡 radio + 标题 + 一句话说明),选项:
  - 指定 session:注入到既有的某个会话 → 下方 session 下拉(仅 classic)。
  - 新建专用 session(仅创建):创建时建一个专属会话,之后固定注入它 → 下方模型下拉。
  - 每次新建 session:每次触发自动创建全新会话,结果互不干扰 → 下方模型下拉。
- 编辑回填:per_run 行 → 'per_run';fixed 行 → 'existing'(session 预选)。
- 提交映射:existing → `targetSessionId`(必填校验保留);dedicated → 不带
  targetSessionId + modelId;per_run → `targetMode:'per_run'` + modelId。
- 列表卡 meta:per_run → 「每次新建 session」+ 可解析时追加「最近:{last_run_session 标题}」。
- 样式:design token / `:deep()` portal 惯例 / 移动端 44px 触控与 DefaultTab radio 同款处理。

## 7. 测试

- db:per_run 行 roundtrip(NULL target + mode + model_id);update 三态 target;
  CHECK 不变式;fixed 级联删保留(AC7 由 last_run_session_id 无 FK 保证)。
- migration:旧形表手工重建 → 调 helper → 列/数据/索引保全 + 幂等。
- tick:per_run fire → FireContext.target_session_id = 新建 session(标题/模型断言);
  同 tick 双 per_run 都 fire(不 defer);pending 滞留不阻塞 per_run;queue-disabled
  照常 fire;create 失败 → error 审计 + 计数消费。
- route:create per_run(wire target_session_id null、不建 session);update
  fixed→per_run 清绑定;per_run→fixed 未选 session 400;非法 target_mode 400。
- 前端:format 不变;tab 表单三档切换提交形状 + 编辑回填 + per_run 卡 meta。
