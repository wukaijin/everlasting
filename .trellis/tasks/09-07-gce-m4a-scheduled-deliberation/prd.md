# GCE-M4a 定时审议:cron 定期召集群聊审议

> 来源:[GROUP-CHAT-API-ROADMAP.md §5 M4](../../../docs/GROUP-CHAT-API-ROADMAP.md) 第一子项;
> 2026-09-07 用户选定从定时审议切入 M4 并逐题定案(本文 Requirements 即决策记录);
> 同日经评审团修订(5 P1 + 5 P2 全采纳,消化记录见 [review.md](./review.md))。

## Goal

定时(每周架构复盘 / 每日站会式复盘 / 发布前评审等)自动召集一场跨模型群聊审议,
无人值守跑完并产出可查阅、可感知的结论——把「手动想起来才开一场」变成
「基建化定期跑」。

## Background(代码证据,详见 [research/daemon-code-anchors.md](./research/daemon-code-anchors.md))

- F2 调度器已就绪(30s tick、7 档 + Once、max_runs/ends_at、catch-up 补跑一次、
  审计、kill switch;spec [backend/scheduled-tasks.md](../../../.trellis/spec/backend/scheduled-tasks.md)),
  但 fire 语义 = 向单聊 session 注入 prompt,且 `validate_target_session`
  对群聊目标 400 拒绝(db/scheduled_tasks.rs:421-423)——定时审议需要新接法。
- 群聊启动原语 daemon 内直调可行:`create_session_in_pool`(F2 create_run_session
  已有 tick 内建 session 先例)+ `chat_inner`(群聊 ctx 自动从 metadata 解析);
  `resume_group_chat_inner`(P1a 五类校验)可直接复用做自动续跑。
- P1a 容错地基就绪:SIGKILL → `interrupted` 档 + checkpoint;无人值守安全
  (GC3 ask 8s 快拒 + working directory 注入)两场 live 已验证。
- M1 驱动 + MCP 六工具 + 嵌套消费(AC5)均已落地——本任务不再走这些消费方,
  daemon 原生直调。
- 成本事实:一场 5-15 分钟、数十万 token。

## Requirements(全部为 2026-09-07 用户定案)

- **R1 链路 = daemon 原生**:scheduled_tasks 新增群聊任务类型
  (`target_mode='group_chat'` + `group_chat_config` 列存展开后的
  moderator+participants 配置),fire 时 daemon 直接建群 + 发题。
  否决 agent 间接驱动(外层 LLM 不确定 + 双层成本)与外部 cron
  (脱离 F2 管理面、无 catch-up)。
- **R2 容错 = 跳过 + 自动续**(fire 查自己 `last_run_session_id` 的场,
  2026-09-07 评审后精确为**四态路由**):busy → 跳过 + 审计(**不计
  run_count**);interrupted → 自动 resume 旧场(P1a 地基的预期消费方),
  resume 被五闸拒绝时兜底 = 审计 error 本期不动(绝不双开场);僵尸场
  (round≥30)与停摆场(NULL+无 checkpoint)→ 补 finalize 后开新场;
  终态/无 → 开新场。范围限任务自己的场,不改 P1a 全局手动 resume 决议。
- **R3 议题 = v1 静态文本**(可含「自行调研 git log 近一周」指令,群聊自己
  会跑工具);不做模板变量、不做 LLM 预生成议题。
- **R4 产物 = 落盘 + GUI 通知**:daemon 在定时场收官时自动导转录(XDG data
  `discussions/`,不依赖源码检出)+ summary 落库(既有)+ GUI 收官 toast
  (沿 P1a 通知先例形态)。飞书挂 B10 自己立项。
- **R5 成本防线(v1 默认定案)= 现有件拼装**:F2b max_runs + enabled 开关 +
  群聊 30 轮帽 + kill switch `scheduled_tasks_enabled`。预算硬停属 M4 成本
  治理子项(上游 C1.2),本任务不做。
- **R6 零回归红线**:fixed/per_run 任务、经典聊、群聊编排器主循环、MCP/脚本
  链路行为零变化(对照组测试兜底)。
- **R7 preset 单一事实源**:M1 preset 定义抽 `scripts/group-chat-presets.json`,
  前端创建表单读同一 JSON,提交展开结果;daemon 零 preset 概念(防双实现漂移)。
- **R8 面边界**:LLM `schedule_task` 工具不开放群聊档(沿 per_run「用户 UI 面
  能力」定案);wire 与 Tauri 同权(Q0 单源)。

## Acceptance Criteria(2026-09-07 评审后修订)

- [ ] **AC1 无人值守开跑**:Settings 建 weekly 群聊任务(preset + 议题 + 调度)
  → 下一 due 点自动建群开跑,全程零审批、零人工;任务卡与审计可见 fire 记录。
- [ ] **AC2 撞场跳过不烧预算**:该任务上一场仍 busy 时 fire 跳过 + 审计
  `skipped_busy`,不排队不补;**消费 due 但不计 run_count**(max_runs 预算
  不被跳过烧穿,对齐 F2b「只计真正送入 chat_inner」契约)。
- [ ] **AC3 断点自动续(四态路由)**:SIGKILL → 重启标 interrupted → 下一
  due 点自动 resume 旧场(不开新场、轮预算继承、summary 不丢);resume 被
  五闸拒绝时兜底 = 审计 error + 本期不动(**绝不双开场**);round≥30 僵尸场
  与停摆场(NULL+无 checkpoint)先补 finalize(error)再开新场,GUI 假
  「进行中」消失。
- [ ] **AC4 产物闭环(单弹)**:定时场收官 → 转录自动落
  `{app_data_dir}/discussions/`(task_name 白名单清洗),summary 落库;
  GUI 收官 toast **恰好一条**(通用轮次通知按 created_via 抑制)+ 点击跳转;
  导出失败仅降级 warn 不影响终态。
- [ ] **AC5 零回归 + 测试五组**:fixed/per_run 对照组 + 既有全部测试套绿
  (cargo --lib / node --test 两脚本 / pnpm test);新增五组 = tick 路由 /
  计数矩阵 / resume 兜底 / 迁移校验 / 对照组。
- [ ] **AC6 同权与边界**:daemon route 与 Tauri command 过同一 `_inner`
  (parity 测试);LLM tool 面无群聊档;校验矩阵新臂(带 target 400 /
  config 非法 400 / 切换清空)全过。
- [ ] **AC7 preset 单源**:`group-chat-presets.json` 为唯一事实源,M1
  `presets` 子命令输出与 `--dry-run` 冒烟不回归,前端展开同源(vite dev
  与 build 两态均验证)。
- [ ] **AC8 成本护栏生效**:fire 前置 catalog 预检(moderator+participants
  模型全查,缺失 → 审计 error + 不建空壳场);无人值守 ask 走 GC3 8s 快拒
  (半透传 sink 的 has_live_observer 透传,不误伤盯场观察者)。

## Out of Scope

- 预算硬停 / per-discussion 成本核算(M4 成本治理子项,依赖群聊内部 C1.2)
- 远程暴露认证(M4 另一子项)
- 讨论库与检索(M4 另一子项;转录文件不进检索面)
- 飞书/外部推送(B10 自己立项)
- participants 自定义编辑(persona 级)、LLM tool 面开放群聊任务(真实需求出现再议)
