# 设计评审消化记录(2026-09-07,session f60e1212)

> 评审场:review 预设(moderator MiniMax-M3;架构 glm-5.3 / 产品 GLM-5.3-Flash /
> 后端 deepseek-v4-flash),23m27s,66 messages,正常收官。
> 转录:[out/group-chat-评审-gce-m4a-定时审议-的技术设计-scheduled-tasks-新增-20260906172349.md](../../../../out/group-chat-评审-gce-m4a-定时审议-的技术设计-scheduled-tasks-新增-20260906172349.md)
> 结论:**11 项共识 / 5 P1 + 5 P2 + 3 P3 + 4 未决项,全数采纳**(无驳回——
> 本轮锚点均经核实,未发现 M2 式评审员误读;与 M2「9 成采纳」相比本轮质量更高)。

## P1 必改(全采纳,已织入 design.md)

| # | 缺陷 | 修正 |
|---|---|---|
| P1-1 | resume 五闸拒绝后「降级开新场」→ 闸②(busy)拒绝时双活场并行烧 token;闸④(round≥30)拒绝时僵尸 interrupted 永久假 chip | 预检分流 + 统一兜底 = audit error + 本期不动(due 已消费,下周期重新路由);round≥30 僵尸场预检时顺手 finalize 再开新场 |
| P1-2 | 纯 NullSink → GC3 无人值守 8s 快拒失效(has_live_observer trait 默认 true)+ AC4 toast 无事件源(终态 Done 是唯一信号)+ 漏记双 sink(ChatEventSink + SubagentEventSink) | **半透传 sink**:内部持 SSE sink,只转发终态 Done 类事件,has_live_observer() 透传 inner;新增实现,生产形态无现成 |
| P1-3 | 「跳过也算 due 消费」混淆 due 消费与 run_count 计数 → weekly+max_runs 任务撞 N 次 busy 提前烧完预算(违反 F2b「只计真正送入 chat_inner」契约 spec:98) | 计数矩阵:三支都消费 due(last_fired_at=due);run_count 只计 started/resumed 受理 + Err;skip 与拒绝只消费不计数 |
| P1-4 | 转录钩子守卫 created_via 无承载——终态块全通道共享,一挂就误导 GUI/MCP 场 | GroupChatCtx 增 created_via 字段(additive,build_group_chat_ctx 从 metadata 读,缺省 None) |
| P1-5 | 「终态/无 → 开新场」漏停摆场(spawn 后首轮 checkpoint 前重启:stop_reason=NULL 且无 checkpoint,boot sweep 不治)→ 永久假「进行中」 | 四态显式路由:停摆场先补 finalize(error 词表)再开新场;GUI 假 chip 消失 |

## P2 建议(全采纳)

- **P2-6** fire 前置 catalog 预检:moderator+participants 全查 models 表,不过 →
  audit error + 不建场(否则 model 被删后每周期落空壳 session + 无限失败循环)。
- **P2-7** 回滚 SOP:版本回滚前先 delete 全部 group_chat 任务行(旧代码
  is_per_run 精确匹配,group_chat 行落 fixed 分支 target NULL → 每 30s tick
  warn+continue 刷屏 + 任务死寂)。
- **P2-8** `last_fire_outcome` 快照列(五值 started/resumed/skipped_busy/error/
  recovered),随 mark_task_fired 同一条 UPDATE 原子写(fixed/per_run 调用点传
  None 零变化);借 CHECK rebuild 便车加列 + CHECK 白名单;任务卡「上次 fire
  怎么了」直读此列,替代审计三层查询(审计读面无 task 维度,db/permissions.rs:304-331)。
- **P2-9** 收官双 toast 抑制:adoptForeignRequest(streamEvents.ts:63-77)认领
  全局 SSE 事件后,通用 maybeNotifyTurnFinished(:1280-1297)会与专用定时收官
  toast 双弹;按 created_via=='scheduled' 路由专用、抑制通用。
- **P2-10** 表单三细节:preset 默认选中 review;participants 预览加
  「N 参与 × ≤30 轮」成本标注(提交前唯一事前闸口);编辑态 preset 下拉占位
  「未选择(使用存档配置)」+ 快照语义提示(显式重套才应用最新 preset)。

## P3 可选(采纳为实现注意事项)

- 转录「轮次」重建 = seq+speaker 连续归组近似(messages 表无 round 列,
  migrations/schema.rs:393);字节级一致性已接受不强求。
- preset JSON 的 vite 落地路径(scripts/ 在 app/ 外):dev 与 build 两态均需
  验证 fs.allow/alias。
- recovered outcome 的卡片文案措辞(纯展示)。

## 未决项定夺

1. 转录内容边界(tool 调用明细取舍)→ **实现期定**(Step 5 首个决策点,
   默认含 tool 调用名+参数摘要,不含 blobs)。
2. catalog 预检与建场之间 TOCTOU(预检后 model 被删)→ **接受极小窗口**:
   建场/chat_inner Err 有审计 error 兜底,不做二次锁定。
3. 导出文件名 sanitize(task_name 用户输入)→ **升为必做**:白名单清洗
   (CJK 保留、剥路径分隔符/控制字符、长度截断),进 Step 5 验收。
4. 转录头部字段集 → 实现期定,最低集 = 任务名/场次 session_id/起止时间/
   参与者清单/stop_reason。

## 测试计划补充(并入 AC5)

五组:tick 路由(三分支 × 受理结局审计序列 + last_run_session_id 落点,
COALESCE 保留旧值最易漏)/ 计数矩阵 / resume 兜底(五闸拒绝 + 停摆场
finalize→开新场,含「spawn 后首轮前重启」构造)/ 迁移校验(rebuild 行保全 +
CHECK 新臂)/ 对照组(既有 fixed/per_run tick+route 全绿零改动)。
