# GCE-M4a 定时审议 — 执行计划

> 依据 [design.md](./design.md)。每步独立可提交、可验证;fixed/per_run 与
> 经典聊路径零行为变化是全程红线。

## 有序清单

### Step 1:preset 抽共享 JSON(scripts 层,零 Rust 改动)
- [x] 新建 `scripts/group-chat-presets.json`(review/arch/retro 三预设:
  moderator 默认模型占位 + participants[name, model 占位, persona_md] +
  显示元数据)。**注意**:模型用**名字**占位还是 UUID?——查 M1 现状:
  preset 里模型引用怎么写的(名字,由脚本解析成 UUID,DAEMON-API.md:140
  「模型引用只认 UUID(名字由脚本解析)」)。共享 JSON 保持「名字」形态,
  **前端展开时也做名字→UUID 解析**(models store 已有目录)。
- [x] `scripts/group-chat-run.mjs` 改读 JSON(删内置常量;`presets` 子命令
  输出不变);`--dry-run` 冒烟。
- [x] 验证:`node --test scripts/group-chat-run.test.mjs` +
  `node scripts/group-chat-run.mjs presets`。

### Step 2:Schema + 校验(Rust db 层;评审加 last_fire_outcome 列)
- [x] migration:scheduled_tasks 加两列 `group_chat_config TEXT NULL` +
  `last_fire_outcome TEXT NULL`(CHECK 五值白名单/null)+ target CHECK
  更新(table rebuild,沿 `rebuild_scheduled_tasks_for_target_mode` 五步舞;
  rebuild 函数扩展或新增,存量行种子两列 NULL,幂等断言)。
- [x] `target_modes::GROUP_CHAT` 常量 + `normalize_target_mode` 白名单扩。
- [x] `GroupChatTaskConfig` 结构体(db 或 scheduler 侧;serde)。
- [x] `mark_task_fired` 增 outcome 参数(fixed/per_run 调用点传 None 零变化;
  count_fire 语义保持 F2b 契约)。
- [x] 校验矩阵(create_scheduled_task_in_pool + update 路径):
  group_chat 带 target → 400;缺 config / config 非法(参与者空、模型不存在)
  → 400;fixed/per_run 带 config → 400(互斥)。`validate_target_session`
  群聊拒绝**不动**。
- [x] 测试:db 层(rebuild 迁移幂等、行保全、roundtrip、CHECK 违反拒绝)、
  校验矩阵全臂(mock pool 单测,沿 per_run 先例)。
- [x] 验证:`cargo test -p everlasting --lib`(PKG_CONFIG_PATH 见 AGENTS.md)。

### Step 3:wire / Tauri command 层
- [x] `CreateScheduledTaskRequest` + update DTO 增 `group_chat_config`
  (双层 Option helper 沿 max_runs 同款);`ScheduledTaskPayload` 暴露。
- [x] `_inner` 参数链贯通(created_by 恒 "user" 不变;LLM tool 恒 fixed 不动,
  tool-contract 17 补边界一行)。
- [x] 测试:daemon route parity roundtrip(含 config 过 wire、显式 null 清空、
  矛盾 400 全臂)。
- [x] 验证:`cargo test -p everlasting --lib scheduled`。

### Step 4:fire_group_chat(scheduler 核心;评审 P1-1/2/3/5 + P2-6 修订)
- [x] `FireContext` 增 `group_chat_config: Option<..>`(additive)。
- [x] **四态显式路由**(读 stop_reason + checkpoint + round):
  busy→skip+audit `skipped_busy`(不计数);interrupted+checkpoint→
  round<30 resume / round≥30 finalize(error)+`recovered`+开新场 /
  五闸拒绝→audit error 本期不动(**不降级开新场**);停摆场(NULL+无
  checkpoint)→finalize(error)+`recovered`+开新场;终态/无→开新场。
- [x] **半透传 sink**(非 NullSink):内部持 SSE sink,只转发终态 Done 类
  事件,`has_live_observer()` 透传 inner(GC3 快拒 + AC4 toast 事件源都靠它);
  resume 双 sink(ChatEventSink + SubagentEventSink)。
- [x] **catalog 预检**:moderator+participants 全查 models,缺失→audit
  `error`(reason model_missing)+不建场不计数。
- [x] **计数矩阵**:三支消费 due;run_count 只计开新场受理/resume 受理/
  开新场 Err;skip 与拒绝不计数。
- [x] 开新场:`create_session_in_pool`(metadata 三键归因 + participants +
  moderator)→ `chat_inner`(topic 无注脚)→ last_run_session_id +
  last_fire_outcome 同 UPDATE 落账 + audit `fired_group_chat`。
- [x] tick 装配:group_chat 档绕过 fired_sessions / pending_by_task /
  queue-disabled gate(沿 per_run 三绕过)。
- [x] 审计常量(skipped_busy / resumed_group_chat / fired_group_chat /
  recovered)+ audit.rs 变体文档补条目。
- [x] 测试(评审五组):tick 路由组(四态 × 受理结局审计序列 +
  last_run_session_id 落点 COALESCE)/ 计数矩阵组 / resume 兜底组(五闸拒绝
  + 停摆场 finalize→开新场,含「spawn 后首轮前重启」构造)/ 迁移校验组 /
  对照组(fixed/per_run 全绿零改动)。
- [x] 验证:`cargo test -p everlasting --lib scheduler`。

### Step 5:转录自动导出(编排终态钩子;评审 P1-4 + sanitize 必做)
- [x] `GroupChatCtx` 增 `created_via: Option<String>`(additive;
  `build_group_chat_ctx` 从 metadata 读,缺省 None)。
- [x] `group_chat_loop.rs` 终态点(finalize 后,锚点 :950-994)挂
  `export_scheduled_transcript`(守卫 `gc_ctx.created_via=="scheduled"`;
  失败仅 warn)。
- [x] 导出实现:读 session messages + role_history → markdown(头部最低集
  任务名/session_id/起止时间/参与者/stop_reason;正文轮次 = seq+speaker
  连续归组近似,tool 调用含名+参数摘要不含 blobs;尾部 summary)→
  `{app_data_dir}/discussions/{date}-{task_name-sanitized}-{sid8}.md`;
  **task_name 白名单清洗**(CJK 保留、剥路径分隔符/控制字符、长度截断)。
- [x] 测试:单测(MockProvider 终态后文件落盘断言 + 非 scheduled 场不导出
  对照 + 写失败不影响终态 + sanitize 用例)。
- [x] 验证:`cargo test -p everlasting --lib group_chat`。

### Step 6:Settings UI + 收官通知(前端;评审 P2-9/10)
- [x] ScheduledTasksTab 目标区第四档「定时审议」:preset 下拉(读共享
  JSON,**默认选中 review**)+ moderator 模型下拉 + participants 只读预览
  (模型显示名 + 「N 参与 × ≤30 轮」成本标注)+ 议题框;编辑回显展开
  结果(preset 占位「未选择(使用存档配置)」+ 快照语义提示);调度档位区
  复用。transport http.ts 域映射确认(锚点 :202-205)。
- [x] store(scheduledTasks.ts)+ format(scheduledTaskFormat.ts):类型扩、
  「审议」徽标、last_fire_outcome 状态行、最近场状态、转录路径链接。
- [x] 收官通知:streamEvents 终态 + created_via=="scheduled" → 专用全局
  toast + 跳转 session;**双弹抑制**(路由专用、抑制通用
  maybeNotifyTurnFinished,锚点 :1280-1297;adoptForeignRequest :63-77)。
- [x] 测试:pnpm test(format/store/tab 用例 + 双弹抑制,沿 F2b 前端测试形态)。
- [x] 验证:`cd app && pnpm test`。

### Step 7:文档 + live 验证
- [x] DAEMON-API.md:scheduled_tasks 群聊任务类型一节(校验矩阵 + fire
  语义 + metadata 归因 + last_fire_outcome)。
- [x] GROUP-CHAT-API-ROADMAP.md §5:定时审议条目收口(✅ + 交付记录)。
- [x] AGENTS.md 速查补一行(已补:转录落盘位置 + last_fire_outcome 速查)。
- [x] live 验证(2026-09-07 用户确认后实跑,daemon 新二进制 + 表重建
  迁移落位;主持 glm-5.3 + arch=deepseek-v4-flash / product=glm-5.3-flash /
  backend=MiniMax-M3,议题均为快速收敛型):
  - **完整周期** ✅:interval 2min max_runs=1 任务 fire(审计 action=
    `fired_group_chat`,run_count=1,outcome=started)→ session metadata
    三键(created_via=scheduled + task_id + task_name)标题「任务名
    HH:MM」→ 讨论收官 group_chat_end → 转录落盘
    `discussions/2026-09-07-M4a-live-完整周期-17a474ea.md`(头部最低集 +
    轮次正文 + 尾部 summary)→ SSE 订阅者收到该 session 的 done 事件
    (半透传 sink = toast 数据源)→ max_runs 自动停用。
  - **SIGKILL 中断续跑** ✅:checkpoint round=0 在场时 kill -9 daemon →
    重启后 boot sweep 4s 内标 interrupted → 下个 due 走 Resume 臂(审计
    `resumed_group_chat` + outcome=resumed + run_count 2→completed)→
    续跑至 group_chat_end;转录起止时间取**原场开工**(checkpoint
    started_at 链),18 条消息横跨 kill 前后无缝。
  - **僵尸场恢复(零 token)** ✅:SQL 构造 interrupted+round=30 场 +
    group_chat_config 塞不存在模型 UUID → RecoverThenOpenNew 命中:僵尸
    finalize(error)+ 审计 `recovered` → 开新场死在 catalog 预检
    (`model_missing`,审计 error,outcome=error)→ run_count 保持 0、
    消息总数不变(零 token 实证)。附带:构造时漏设
    last_run_session_id 的失误反向验证了「无锚 → OpenNew」防御臂。
  - **P3 观察项(非缺陷,沿 F2b per_run 既有形态)**:max_runs=1 任务
    首场 fire 的 `completed` 审计因内存快照 last_run_session_id=NULL
    被跳过(WARN 落日志,停用行为正确;≥2 场任务尾场审计有锚正常落)。

## 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `app/src-tauri/src/scheduler/mod.rs` | tick 核心逻辑,改坏影响 F2 全部任务 | Step 4 独立提交;对照组测试兜底 |
| `app/src-tauri/src/agent/group_chat_loop.rs` | 终态钩子动编排器主干 | 钩子放 finalize 之后、失败仅 warn,不动主语义 |
| migration(rebuild) | 存量任务表 | additive 加列,旧代码可跑;幂等断言 |
| `app/src/components/settings/ScheduledTasksTab.vue` | 1880 行大文件 | 表单分支隔离,既有三档模板不动 |

## start 前检查

- [x] prd.md 收敛 pass 完成
- [x] implement.jsonl / check.jsonl 已填真实条目
- [x] 用户审阅三件套通过
