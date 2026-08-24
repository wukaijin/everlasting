# PRD: turn 流式持久化 + 崩溃恢复(RULE-PERSIST-001 · P1)

## Goal

daemon 异常终止(kill -9 / OOM / 断电)时,正在流式生成的 assistant turn 内容不再全量丢失:重启后用户能看到崩溃前已生成的部分内容,且该 session 可以无损继续对话(不发 400)。

闭合 `.trellis/reviews/DEBT.md §RULE-PERSIST-001`(唯一剩余 P1)。

## 背景与确认事实

(证据全文见 `research/persistence-path-map.md`,此处只列需求相关)

1. assistant 消息只在流结束后一次性落库(drive.rs:1567);流式过程全部在内存累加器(`ordered_blocks`/`pending_text`/`pending_thinking`)。
2. 崩溃有两类窗口:**W1 流式中**(内容全丢)与 **W2 工具执行中**(tool_result 丢失 + 残留孤儿 `tool_use` 行 → 该 session 下次请求被 provider 400 拒绝,pair atomicity)。W2 今天无任何修复。
3. `messages` 表无 status 列;迁移体系是幂等探针式(`add_messages_column_if_missing`),无版本号。
4. 先例可循:`subagent_runs` 已实现"running 占位 + 启动 reap"(state.rs:311);error/cancel 路径已有 marker 块与合成 tool_result 手法(helpers.rs)。
5. seq 由 DB max+1 初始化 → 恢复行天然不与后续 seq 冲突。
6. worker(skip_persist=true)不落 messages 表 → 检查点天然只需覆盖主 chat。
7. 前端:daemon 重启后 EventSource 自动重连并必收 `stream-resync` 哨兵,但该事件**无消费者**;activeRequests 流式占位会永久卡死(无 watchdog)——本任务之前就存在的缺口。

## Requirements

### R1 流式检查点(backend)

- R1.1 LLM stream 就绪后立即写 `status='in_progress'` 占位行(当前 seq,内容为空)。
- R1.2 流式期间按时间间隔(≤1s 量级)把累积内容 upsert 到同一行;检查点为 best-effort:写失败只 warn,不打断流。
- R1.3 turn 正常/cancel/error 收尾时,最终落库覆盖检查点行并清除 in_progress 状态;空 turn(无任何内容)收尾时删除占位行,不留空行。
- R1.4 检查点仅主 chat(`!skip_persist`);worker 路径行为零改动。

### R2 启动恢复(backend)

- R2.1 daemon/GUI 启动时(紧随 `reap_orphaned_runs`)执行恢复 pass,best-effort(失败 log 不阻塞启动)。
- R2.2 `status='in_progress'` 残留行:有内容 → 追加中断 marker 块(与 `[已停止]`/`[生成出错中断]` 同族文案)并标 `status='interrupted'`;纯空占位 → 删除。
- R2.3 孤儿修复(覆盖 W2):每个 session 的尾部若是含 `tool_use` 的 assistant 行且无配对 tool_result 行 → 追加一条合成 is_error tool_result(user-role)行,保证下次请求不 400。
- R2.4 恢复影响的 session 更新 `updated_at`,UI 列表排序反映中断。

### R3 前端自愈(2026-08-24 用户确认纳入)

- R3.1 消费 `stream-resync` 哨兵:终结卡死的流式占位 + 重拉当前 session 快照,使"daemon 重启 → UI 自动恢复到检查点内容"闭环。

## 验收标准(AC)

- AC1(R1.2/R1.3)集成测试:正常多 turn 对话后,messages 表无 `status='in_progress'` 残留,内容与现状逐字节一致(既有 error_persist / turn_usage 等测试全绿即证)。
- AC2(R1.2)模拟崩溃:流式进行中(检查点已写 ≥1 次)后进程终止 → DB 中该 seq 行存在且含最后检查点内容,`status='in_progress'`。
- AC3(R2.2)对 AC2 的 DB 跑恢复 pass → 行含中断 marker、`status='interrupted'`;空占位行被删;无 in_progress 残留。
- AC4(R2.3)构造 W2 残留(尾部 assistant(tool_use) 无 tool_result)→ 恢复 pass 追加合成 tool_result 行;该 session 此后可正常发起 LLM 请求(pair atomicity 保持)。
- AC5(R1.4)worker 回归:现有 subagent 测试全绿,subagent_runs 行为不变。
- AC6(R2.1)恢复 pass 对干净 DB 为 no-op 且不报错;启动日志可 grep 到恢复条数(对齐 reap 的日志形态)。
- AC7(若 R3 纳入)前端测试:stream-resync 事件触发后 activeRequests 清空、当前 session 重新 load,流式占位被检查点内容替换。
- AC8 回归:后端全量 `cargo test -p everlasting --lib` 无新增失败;`scripts/turn-smoke.sh` 通过(检查点不干扰 live 链路);fmt/clippy 零新增。

## Out of Scope

- 工具执行中途(W2)的 tool_result 增量检查点 —— 靠启动孤儿修复兜底,内容级丢失接受
- turn_trace / sessions.last_* 对中断 turn 的补记(无 Done,无数据源)
- background_shell / QuestionStore(pending 卡片)的持久化 —— 纯内存是独立已知缺口
- daemon 崩溃后 sidecar 自动重启(GUI 不 respawn daemon,维持现状)
- `status` 字段的前端 UI 呈现(徽标/样式)—— marker 文本块已可见
- 检查点写 turn_trace run 行(worker 维度)

## Open Questions

(无 —— R3 纳入已于 2026-08-24 由用户确认,AC7 生效。)
