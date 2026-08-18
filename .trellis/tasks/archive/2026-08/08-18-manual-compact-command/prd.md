# 手动 /compact 命令入口

## Goal

给用户一个主动收窗入口:`/compact` 内置命令,触发与自动路径同源的 LLM 摘要压缩(C3+,08-18 已归档)。不受 0.85×window 自动触发线限制,可带 focus 参数定向摘要,已有水位时增量合并。

## Background(已确认事实)

- 父任务(已归档)`.trellis/tasks/archive/2026-08/08-18-llm-context-compaction/`:摘要生成(`build_compaction_prompt` / `compute_preservation_region`)、落库(`insert_compaction_summary`)、水位替换(`apply_compaction_watermark`)、熔断(`CompactionRegistry`)全部现成;spec 见 `pattern-llm-compaction.md`。
- 代码已预留位:`resource_loader.rs:98-100` 注释明确 "`/compact` is deferred"。
- 内置命令框架:builtin 定义在 Rust `BUILTIN_COMMANDS`(help/clear/new),handler 是 `ChatInput.vue:474-515` 的 switch;**无带参数先例**;直输 `/xxx`+回车目前**不走命令分发**(当普通消息发 LLM),只有面板选中才执行。
- 后端压缩函数 `attempt_summary_compaction` 是 drive.rs 私有、依赖 loop 内存态;空闲期(loop 未跑)手动入口需新开编排函数,seq 用 `MAX(seq)+1`(无并发 persist 时安全)。
- 命令层获取 provider/context_window 有现成路径:`lookup_provider_for_session`(agent/chat.rs:650),与 chat 主路径同源(session model override → global default)。
- in-flight 检测现成:`AppState.session_active_request`(state.rs:545)。

## 决议(brainstorm 2026-08-18)

| # | 决策 | 依据 |
|---|------|------|
| D1 | 命令名 `/compact` | 代码预留注释 + Claude Code 先例 |
| D2 | focus 语法 = 直输 rest-of-line 自由文本(`/compact 聚焦 API 变更`);面板选中 = 无参立即执行(同 /clear /new 惯例) | builtin 无带参先例,palette 路径保持简单;focus 走直输解析 |
| D3 | 直输拦截做成**通用内置命令分发**:提交路径首 token 匹配任意 builtin 名 → 走与面板选中同一 handler;顺带修正 /help /clear /new 直输发 LLM 的不一致 | 一处拦截覆盖四个命令,消除面板/直输行为分叉 |
| D4 | 有轮次进行中(streaming)→ **拒绝 + toast 提示先停止**;不做排队/自动取消 | 无 turn 间队列机制;取消丢在途工作违背落库无损气质 |
| D5 | 手动摘要失败 → 清晰报错 toast,**零 DB 写入**;机械丢组降级链自然发生在下一次请求(空闲期无 in-loop context,机械丢组无持久化语义) | 修正原 R4 草案表述 |
| D6 | 熔断:手动入口**不查** `is_tripped`;失败照常 `record_failure`(共享失败信号),成功 `record_success`(顺带解熔断) | R4"手动不受熔断限制";信号共享让 auto 路径也能感知 LLM 故障 |
| D7 | 观测:摘要行 metadata `trigger:"manual"` + `focus` 字段;无 turn 上下文 → 不写 turn_trace `compaction_json`;命令响应载荷携带 before/after token 供前端 toast | 手动压缩发生在 turn 边界外 |

## Requirements

- R1 `/compact` 触发一次摘要压缩:复用自动路径的保留区计算/摘要/落库;无视 0.85 阈值(低 context 也允许,用户主动收窗);
- R2 可选 focus 参数注入摘要 prompt(定向指令);
- R3 已有水位(最新 `kind=compaction_summary` 行)→ prior-summary 增量合并,`prior_summary_seq` 非空;
- R4 失败 → 用户可见报错(D5);in-flight 拒绝(D4);熔断绕过 + 信号共享(D6);
- R5 前端:builtin 注册 + palette case + 通用直输分发(D3)+ 压缩中/完成/失败 toast 提示;完成后刷新消息流(摘要行以现有 `kind=compaction_summary` 渲染,PR3 已落地);
- R6 scope gate:仅单聊主 loop session;worker / 群聊 session 拒绝(口径同 C3+ gate)。

## Non-Goals

- MAX_TURNS 软卡化(独立任务 `08-18-max-turns-softcap`);
- handoff 接力(独立任务 `08-18-handoff-mechanism`);
- 自定义命令(/command 资源)的直输分发(后续可按 D3 模式扩展);
- chat 流内摘要 UI 卡片(展示升级,后置);
- streaming 中排队/取消后压缩(D4 明确不做)。

## Acceptance Criteria

- [x] AC1:palette 选中 `/compact` 与直输 `/compact` 回车均触发压缩;低 context(未超 0.85 线)同样成功;待压区为空(水位后历史全进保留区)→ 明确报错而非误写 —— mock 测试 `manual_compact_succeeds_below_trigger_line…` / `manual_compact_rejects_when_nothing_to_compress`;直输分发 `slashCommand.test.ts`(7 测)+ palette case 同落 `executeBuiltin`;
- [x] AC2:带 focus 时摘要 prompt 含定向指令 —— `manual_compact_injects_focus_into_prompt`(mock 断言 prompt 文本 + metadata.focus);
- [x] AC3:已有水位的手动压缩产出增量合并摘要(metadata `prior_summary_seq` 指向旧摘要行;旧摘要行保留)—— `manual_compact_merges_with_existing_watermark`;
- [x] AC4:摘要 LLM 失败 → 错误返回用户、messages 表零新增行;session in-flight → 拒绝(命令层 `session_active_request` guard);熔断 tripped 状态下手动仍可执行且成功后解熔断 —— `manual_compact_failure_writes_nothing_and_counts_breaker` / `manual_compact_bypasses_tripped_breaker_and_untrips_on_success`;
- [x] AC5:前端四命令直输分发正确;压缩中/结果 toast —— `executeBuiltin` 共享 handler + `matchBuiltinCommandInput` 拦截;1107 FE 测试全绿;
- [x] AC6:worker/群聊 session 调用被拒绝 —— 群聊:`compact_session_route_rejects_group_chat`(daemon route oneshot)+ 命令层 gate;worker 见 PRD 限制说明(session 行不可判定,命令面向单聊);
- [x] AC7:mock 端到端 + live 验证 —— mock:`manual_compact_succeeds…`(水位 Applied + 保留区存活断言);live:`scripts/turn-smoke.sh --compact`(real LLM:摘要行 seq=MAX+1 / trigger=manual / focus / cutoff 精确;压缩后下一请求**无 watermark_miss**、保留区大消息对存活、turn 正常完成)。

## Technical Notes

- 新后端命令 `compact_session`(sessions domain):Tauri command + daemon HTTP route + `CMD_TO_DOMAIN` 三处注册(参照 `group_chat_cache_rates` 全链路);daemon 路由补 oneshot 冒烟测试(backend/daemon-server.md §6 惯例)。
- 摘要行 metadata 复用 C3+ §2.1 契约,新增 `trigger:"manual"` 与 `focus` 字段(serde 兼容旧回看)。
- 关联 spec:`pattern-llm-compaction.md`(数据契约/gate/降级链 + 新增 §手动入口)、`daemon-server.md`(路由注册)。
- live 验证方法:`turn-smoke.sh --compact` 小轮 + 大消息轮(~20k token,撑过保留区预算)→ idle 压缩 → 全量 wire 续跑一轮(单条 wire 会让水位对齐 fail-open,验不到 Applied;--compact 模式自动从 DB 重建全量 wire)。
- 已知边界(接受):待压区极小时摘要可能比被压内容"胖"(context 净增长,smoke 实测 before=38535 → after=38672)——用户主动行为,后续增量合并吸收;spec 已记录。
