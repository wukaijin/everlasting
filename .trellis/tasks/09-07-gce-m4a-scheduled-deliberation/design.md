# GCE-M4a 定时审议 — 技术设计

> 决策依据见 [prd.md](./prd.md)(四项用户定案)+ [research/daemon-code-anchors.md](./research/daemon-code-anchors.md)(代码锚点)。
> **2026-09-07 经评审团修订**(session f60e1212,review 预设,5 P1 + 5 P2 全采纳,
> 消化记录见 [review.md](./review.md)):四态显式路由 / 半透传 sink / 计数矩阵 /
> catalog 预检 / GroupChatCtx.created_via / last_fire_outcome 列 / 双 toast 抑制等。
> 推进原则(沿 roadmap):① 独立可提交 ② 明确验证标准 ③ GUI/经典聊路径零行为变化。

## 1. 架构总览

```
scheduler tick(30s,daemon 常驻)
  └─ most_recent_due 命中(target_mode='group_chat' 的任务)
      └─ fire_group_chat(state, ctx)          ← 新 fire 分支,与 fire_via_chat_inner 平行
          ├─ last_run_session_id 四态显式路由(读 stop_reason + checkpoint + round):
          │   ├─ busy(内存 session_active_request)→ 跳过 + audit skipped_busy(不计数)
          │   ├─ interrupted + checkpoint:round<30 → resume_group_chat_inner(半透传 sink)
          │   │                                round≥30 → finalize(error)→ 开新场
          │   │                                五闸拒绝 → audit error 本期不动
          │   ├─ 停摆(NULL + 无 checkpoint)   → finalize(error)→ 开新场
          │   └─ 终态 / 无                      → 开新场
          └─ 开新场(catalog 预检 → 建群 → chat_inner):
              ├─ create_session_in_pool(session_type="group_chat",
              │     metadata={participants, moderator_model_id, created_via:"scheduled"})
              ├─ chat_inner(ChatEntry{topic})   ← 编排器 spawn 后台跑,fire 只等受理
              └─ last_run_session_id + last_fire_outcome 落账
```

边界:**不动** F2 既有 fire 路径(fixed/per_run 逐字节不变)、**不动**群聊编排器循环
(group_chat_loop.rs 主逻辑不变,只在终态点挂转录导出钩子)、**不动** MCP/脚本链路。

## 2. Schema 变更(scheduled_tasks)

- 新档:`target_mode = 'group_chat'`(白名单 `normalize_target_mode` 扩;
  常量 `target_modes::GROUP_CHAT`)。
- 新列:`group_chat_config TEXT NULL` —— 存**创建时展开的完整群聊配置** JSON:
  ```json
  {
    "moderator_model_id": "<uuid>",
    "participants": [{"name": "...", "model_id": "<uuid>", "persona_md": "..."}]
  }
  ```
  daemon fire 侧**零 preset 概念**(不存 preset 名;preset 展开发生在创建 UI)。
- CHECK 约束(改,需 table rebuild,沿 per_run 五步舞先例
  `schema_helpers::rebuild_scheduled_tasks_for_target_mode`):
  - `target_mode='fixed' ⇔ target_session_id IS NOT NULL`(既有)
  - `target_mode='group_chat' ⇒ target_session_id IS NULL AND group_chat_config IS NOT NULL`
  - `target_mode='per_run' ⇒ target_session_id IS NULL`(既有)
- 复用列:`prompt` = 议题文本(非空校验沿用,语义复述为 topic);
  `model_id` 不用(moderator 在 config JSON 里);`last_run_session_id` =
  容错锚点(fire 开新场时写入;**无 FK 不级联**语义恰好是我们要的——删场不删任务);
  `max_runs`/`ends_at`/`enabled`/调度档位全部复用。
- 新列(评审 P2-8):`last_fire_outcome TEXT NULL` —— 五值
  `started / resumed / skipped_busy / error / recovered`,随
  `mark_task_fired` **同一条 UPDATE 原子写**(既有 `count_fire` 参数旁增
  outcome 参数;fixed/per_run 调用点传 None,零行为变化);借 rebuild 便车
  加列 + `CHECK (last_fire_outcome IS NULL OR last_fire_outcome IN (...))`。
  用途:任务卡「上次 fire 怎么了」直读此列(替代审计三层查询——审计读面
  全 session 维度无 task 维度);与 last_run_session_id 两轴正交
  (「哪场」vs「上次怎么了」)。
- 校验矩阵增补(create + update):
  - `group_chat` 档带 `target_session_id` → 400 矛盾(沿 per_run 同款);
  - `group_chat` 档缺 `group_chat_config` 或 config JSON 非法(参与者空 /
    moderator_model_id 或任一 participant model_id 不存在)→ 400;
  - 切换保护:fixed↔group_chat 切换时 target/config 显式清空(wire 双层 Option 同款)。
- **`validate_target_session` 的群聊 400 拒绝不动**(fixed 档仍然禁止群聊目标;
  group_chat 档根本不走 target 解析分支)。

## 3. preset 单一事实源(防双实现漂移)

- M1 的 preset 定义从 `scripts/group-chat-run.mjs` 内置常量抽到
  `scripts/group-chat-presets.json`(review / arch / retro;形状 =
  moderator 默认模型占位 + participants 名单 + persona_md 文本)。
- M1 脚本读 JSON(import);前端 Settings 表单读同一 JSON(vite import,配
  fs.allow / alias)。
- **展开时机 = 创建/编辑提交时**:表单选 preset → 前端把 preset 展开成
  moderator_model_id(用户可改选)+ participants(含 persona_md)→ 提交展开结果。
  DB 与 daemon 全程无 preset 名。
- MCP `start_discussion` 的 `preset` 参数不受影响(它传给引擎的是名字,
  scripts 侧解析——实现层若与 JSON 载体有出入,M1 单测兜住)。

## 4. fire_group_chat 契约(scheduler/mod.rs;评审 P1-1/3/5 + P2-6 修订)

- `FireContext` 增 `group_chat_config: Option<GroupChatTaskConfig>`(additive,
  fixed/per_run 路径 None,零行为变化)。
- **四态显式路由**(开新场前对 last_run_session_id 所指场**显式读**
  stop_reason + checkpoint 存在性 + round;不做「终态/无」笼统兜底):
  1. **busy**(内存 session_active_request 含)→ 跳过 + audit `skipped_busy`
     + outcome=`skipped_busy`;**消费 due 不计 run_count**(计数矩阵见下)。
  2. **interrupted 且 checkpoint 在**:
     - 预检 `checkpoint.round < 30` 且非 busy → 直调
       `resume_group_chat_inner(state, sid, sink, sub_sink)`(chat.rs:145;
       **双 sink**:ChatEventSink + SubagentEventSink)→ 受理 → audit
       `resumed_group_chat` + outcome=`resumed`;
     - round≥30 僵尸场(编排器死于第 30 轮中途,boot sweep 标 interrupted 但
       轮帽终态无人写)→ 顺手 `finalize_group_chat_lifecycle(error)`
       (session_crud.rs:953)→ audit `recovered` + outcome=`recovered`
       → 开新场;
     - resume 被五闸拒绝(busy 竞态 / checkpoint 已删等)→ **统一兜底 =
       audit error + 本期不动**(due 已消费,下周期 tick 重新路由;
       **不降级开新场**——闸②拒绝时降级 = 双活场并行烧 token,闸④拒绝时
       降级 = 僵尸场永久假 chip)。
  3. **停摆场**(stop_reason=NULL 且无 checkpoint 行——编排器 spawn 后首轮
     checkpoint upsert 前 daemon 重启;boot sweep 条件是 checkpoint 行存在,
     不治此类)→ 补 `finalize_group_chat_lifecycle(error)`(落既有 error
     词表,**不发明新 stop_reason**;GUI 假「进行中」消失、变真 chip 可手动续)
     → audit `recovered` + outcome=`recovered` → 开新场。
  4. **终态 / 无 last_run_session_id** → 开新场。
- **开新场**(含 catalog 预检,评审 P2-6):
  0. 预检:moderator_model_id + 全部 participants model_id 查 models 表,
     任一缺失 → audit `error`(reason `model_missing`)+ 不建场
     (due 消费,outcome=`error`)**不计 run_count**——否则 model 被删后
     每周期落空壳 session + 编排 resolve 失败无限循环。
     预检与建场之间的 TOCTOU 极小窗口**接受**(建场/chat_inner Err 有审计兜底)。
  1. `create_session_in_pool(db, project_id, project.path, moderator_model_id,
     Some("group_chat"), metadata)`,metadata = `{participants,
     created_via: "scheduled", scheduled_task_id, scheduled_task_name}`;
  2. `chat_inner(state, ChatEntry{request_id: uuid, session_id, messages:
     [{role:"user", content: topic}]})` —— 编排器自动从 metadata 解析
     `build_group_chat_ctx`(chat.rs:444),无人值守 ask 由 GC3 8s 快拒兜底
     (前提见下方 sink 设计);
  3. 受理(Started/Injected 之外的 Err)→ audit `error`(reason 透传)+
     outcome=`error`(计 run_count,与 F2b「Err 也计数」对齐);
  4. `last_run_session_id = 新 sid` 落账。
- **计数矩阵**(评审 P1-3,对齐 F2b 契约 spec:98「只计真正送入 chat_inner 的
  fire」):
  - **三支全部消费 due**(`last_fired_at = due`)——防同 due 重复 fire;
  - run_count **只计**:开新场受理(Queued/Started 类比)+ resume 受理 +
    开新场 Err;**不计**:busy 跳过、resume 被拒、catalog 预检不过
    (与 F2b dedup 同类「prompt 未送达」——否则 weekly+max_runs=52 的任务
    撞 10 次 busy 提前烧完预算)。
- **sink = 半透传,不是 NullSink**(评审 P1-2):新增实现,内部持 SSE 广播
  sink,只转发终态 Done 类事件(收官 toast 的唯一事件源 = 编排器经 sink emit
  的终态事件,group_chat_loop.rs:1004+;纯丢弃会掐死 AC4 信号);
  `has_live_observer()` 透传 inner(GC3 无人值守 8s 快拒依赖
  `has_live_observer()==false`,而 trait 默认 true——恒 false 会误伤盯场
  用户,消费方 ask.rs:122-127;HttpSseSink 动态覆写先例 sse.rs:364-366)。
  resume 需双 sink(ChatEventSink + SubagentEventSink,chat.rs:145-149)。
- **topic 不加注脚**(F2 的「(本条由定时任务…自动触发)」注脚会污染议题文本);
  归因走建群 metadata 三键 + 审计。
- **per_run 三绕过同样适用**(group_chat 每次新场/旧场恢复,不经注入队列):
  不进 `fired_sessions`、不进 `pending_by_task`、不受 queue-disabled gate 影响。
- 编排终态后无人回写任务行(任务行只有调度元数据 + last_fire_outcome 快照;
  「最近场状态」经 last_run_session_id 联查 session 实况)。

## 5. 转录自动导出(daemon 原生,仅定时场;评审 P1-4 + 未决 3/4 修订)

- **守卫承载**:`GroupChatCtx` 增 `created_via: Option<String>` 字段
  (additive;`build_group_chat_ctx` 从 metadata 读,缺省 None)——终态块
  (group_chat_loop.rs:950-994)是全通道共享的,钩子守卫
  `gc_ctx.created_via == Some("scheduled")` 必须挂在 ctx 字段上,
  否则一挂就把 GUI/MCP 场也导出。
- 挂点:编排器终态写入处(`finalize_group_chat_lifecycle` 调用后),
  失败仅 warn,不影响终态落库(沿 M2「导出失败降级不污染轮询」先例)。
- 落点:`{app_data_dir}/discussions/{YYYY-MM-DD}-{task_name-sanitized}-{sid 前 8}.md`
  (编排器签名已有 `app_data_dir` 参数;不落仓库 out/ —— daemon 原生不依赖
  源码检出)。**task_name sanitize 必做**(用户输入文本,CJK 保留、剥路径
  分隔符/控制字符、长度截断)。
- 内容:头部最低集 = 任务名 / session_id / 起止时间 / 参与者清单 /
  stop_reason;正文 = 轮次 + per-speaker 发言(**轮次 = seq+speaker 连续
  归组近似**——messages 表无 round 列,migrations/schema.rs:393;tool 调用
  明细取舍实现期定,默认含调用名 + 参数摘要、不含 blobs);尾部 =
  discussion_summary。
- GUI 入口:收官通知 / 任务卡片「最近场」带转录路径(打开 = 系统文件打开,
  沿 GUI 既有文件打开通道;若无可复用通道则显示路径文本,v1 不做内嵌查看器)。

## 6. GUI 通知(收官触达;评审 P2-9 细化)

- 前端沿 P1a 先例形态扩展:定时场终态(stop_reason 非空且
  created_via=="scheduled")→ 全局 toast「定时审议『{task_name}』已收官
  ({stop_reason})」+ 点击跳转该 session。
- 事件面:streamEvents 既有 SSE 消费(`adoptForeignRequest`
  streamEvents.ts:63-77 已认领非当前 session 的全局事件)——终态信号经
  半透传 sink 进入全局 SSE 流(§4);若全局流订阅只覆盖当前打开的
  session,则实现期补「后台 session 终态」监听(锚点 :1430-1447)。
- **双弹抑制**:通用 `maybeNotifyTurnFinished`(:1280-1297)会与专用定时
  收官 toast 各弹一条;按 created_via=='scheduled' 路由专用 toast、
  抑制通用通知。
- Settings 任务卡片:「审议」类型徽标 + `last_fire_outcome` 直读显示
  (上次 fire 怎么了)+ 最近场状态(联查 session)+ 转录路径链接。

## 7. wire / Tauri command

- `CreateScheduledTaskRequest` / update DTO 增 `group_chat_config:
  Option<GroupChatTaskConfigDto>`(snake_case;update 用双层 Option——显式 null
  = 清空,沿 max_runs/ends_at 同款 helper)。
- `ScheduledTaskPayload` 暴露 `group_chat_config`(additive,前端编辑回显用)。
- Tauri command 与 daemon route 同一 `_inner`(Q0 单源不动)。
- LLM `schedule_task` 工具:**不开放**群聊任务(恒 fixed 语义,沿 per_run
  「用户 UI 面能力」定案;tool-contract 17 补一行边界说明)。

## 8. Settings UI(ScheduledTasksTab.vue;评审 P2-10 细节)

- 目标区 radio 第四档「定时审议」(既有三档:指定/新建专用/每次新建);
  选中后表单切换为:preset 下拉(读共享 JSON,**默认选中 review**——AC1
  建单压到两步)+ moderator 模型下拉(默认取 preset)+ participants 只读
  预览(模型**显示名** + 「N 参与 × ≤30 轮」成本标注——提交前唯一事前
  闸口;**自定义编辑 = follow-up 不做**)+ 议题文本框(placeholder 引导
  「议题质量直接决定产出质量」)+ 调度档位区完全复用。
- 编辑态:回显存档的展开结果(config JSON);preset 下拉占位
  「未选择(使用存档配置)」+ 一行快照语义提示(显式重套才应用最新
  preset,防「改 JSON 自动生效」误解);换 preset = 重新展开覆盖(提示会覆盖)。
- 列表卡片:「审议 · review · 每周五 18:00」+ `last_fire_outcome` 状态行
  + 最近场状态 + 转录路径链接。

## 9. 兼容性 / 迁移 / 回滚(评审 P2-7 补 SOP)

- migration:scheduled_tasks 加列(group_chat_config + last_fire_outcome)+
  CHECK 变更 = table rebuild(存量行种子:两列 NULL;幂等断言沿 per_run 先例)。
- fixed/per_run 路径零行为变化(spec Bad Cases 全数保持);GUI 经典聊零变化。
- **回滚 SOP**:版本回滚前先 delete 全部 group_chat 任务行——旧代码
  `is_per_run` 精确匹配(mod.rs:306),group_chat 行落 fixed 分支、target
  NULL 命中防御分支(mod.rs:343-351 warn+continue)→ 不烧 token 但每 30s
  tick 重判重 warn 无限刷屏 + 任务死寂无感知。
- 代码级回滚:两新列 additive,旧代码不读;CHECK 更严格但旧值域不违反。
  任务级回滚 = delete 任务行。

## 10. 已接受的权衡

- **daemon Rust 与 JS 双侧都有「建群编排」知识**:daemon 只掌握
  create_session + chat_inner 原语(不复制 M1 的轮询/导出编排),可接受。
- **转录导出格式双实现**(Rust 自动导 vs M1 脚本导):核心结构对齐,
  字节级一致性不做保证;两者消费场景不同(人查阅 vs headless 消费方);
  轮次列缺失 → seq+speaker 归组近似(评审 P3)。
- **撞 busy 跳过不排队**(跳过不计数、不补):语义简单,审计可见;
  排队会造成堆积 + 成本失控。
- **catalog 预检与建场之间的 TOCTOU 极小窗口接受**(评审未决 2):
  建场/chat_inner Err 有审计 error 兜底,不做二次锁定。
- preset JSON 在 scripts/ 而 app/ 外:vite dev 与 build 两态均需验证
  fs.allow/alias(评审 P3,Step 1 验收项)。

## 11. 明确不做(follow-up 记账)

- participants 自定义编辑(persona 级)——v1 preset + moderator 可选。
- 飞书/外部推送(B10 自己立项)。
- 预算硬停 / per-discussion 成本核算(M4 成本治理子项)。
- LLM tool 面开放群聊任务(agent 建群任务的真实需求出现再议)。
- 历史审议检索(M4 讨论库子项)。
