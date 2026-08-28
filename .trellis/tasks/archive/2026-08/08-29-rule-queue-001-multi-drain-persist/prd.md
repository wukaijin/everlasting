# PRD — RULE-QUEUE-001 多 drain 丢消息根治

> 债源:`.trellis/reviews/DEBT.md` §RULE-QUEUE-001(P2,Agent Loop);发现于 F2 planning review(2026-08-28)。

## 背景与问题

F1 消息队列的驱动器(`run_queue_driver`)每轮把**全部** drained 条目喂给 LLM
(`turn_messages = reload_messages(db) ++ drained`),但 `run_chat_loop` init 段的
persist 点(`chat_loop/init.rs`)只写 `messages.iter().rev().find(role == User)`
命中的**尾条** user message。同轮 drain ≥2 条时,非尾条 user 消息:

- LLM 单次看到了(响应内容受其影响);
- 但没有 DB 行 —— reload 后从时间线**消失**。

F1-A 手动连发窗口小、概率低;F2 定时任务(F2b 六档)使队列条目常驻待消费,
调度 fire + 手动连发叠加把多 drain 从边缘场景变成常态场景,触发率被结构性放大。
F2 已做缓解(去重 / `lost` 审计 / 钉行为测试 `multi_drain_pins_current_tail_only_persist_rule_queue_001`),
本任务做根治。

## Goal

同轮 drain 的全部 user 条目持久化落库,reload 后时间线完整;非尾条的
scheduled 来源标记与附件 refs 随行落 metadata;既有路径行为逐字节不变。

## Requirements

- R1. 同轮 drain 的**全部** user 条目(非尾条 + 尾条)持久化进 `messages` 表,
  seq 连续递增、FIFO 顺序与 drain 顺序一致;reload 后时间线完整。
- R2. 非尾条的 F2 `origin`(scheduled 来源标记)不丢:带 origin 的行写
  `metadata.scheduled` 信封(镜像尾条 F2 契约),reload 后「定时」标识可见。
- R3. 非尾条携带的 B1 附件 refs 不丢:写 `metadata.attachments`(镜像尾条信封形状)。
- R4. 既有路径行为逐字节不变:单 drain(队列长度 1)、经典单聊、群聊、worker
  子代理路径的 persist 行为零变化;手动条目(无 origin 无附件)不写 metadata。
- R5. 持久化失败语义镜像尾条(RULE-A-003):非尾条写失败 → 可见 Error 终止本轮,
  不允许「LLM 答了 DB 没记过的消息」。
- R6. FTS 搜索索引同步覆盖新落库行(复用 `persist_turn` 即自动触发 AFTER INSERT trigger)。

## 约束

- C1. persist 循环放 init.rs(seq 契约单一归属地);**不**在驱动器侧预写
  (避免 driver 需自算 next_seq + reload/extend 去重的双份复杂度)。
- C2. `ChatLoopRequest` 需要携带 drained 全量(见 design §字段形态);
  其余 5 个构造点(经典 / 群聊 ×2 / worker / 测试 builder)传空。
- C3. worker 路径(`skip_persist`)跳过非尾条持久化,镜像尾条守卫。
- C4. seq 从 init 段既有的 `next_seq`(fresh `load_session` MAX+1)起算,
  循环内逐条 `seq += 1`;驱动器是队列唯一消费者,无并发写者。
- C5. 已知边界(接受,写入 spec):非尾条行的 @文件注入 manifest 不落
  metadata(`inject_at_tokens` 只返回尾条 manifest)——DB 行存原始
  `@relpath` 文本,reload 重展开,内容不丢;缺的只是 hint 行。
- C6. 跨客户端 live 同步不在本任务范围:非发起端在 finalize/reload 后看到
  补齐的行(今天则彻底消失),live 推送用户消息属独立需求。

## Acceptance Criteria

- [ ] AC1. 改写钉行为测试:multi-drain(scheduled + manual)后 DB 两行都在、
  顺序 = drain 顺序、scheduled 行 metadata 带 `scheduled` 三键、manual 行
  metadata 为 None。
- [ ] AC2. 三条 multi-drain(≥3 条,含多 manual)全落库,seq 连续有序。
- [ ] AC3. 单 drain / 纯定时轮 / 手动轮的既有测试全绿不改断言(逐字节不变量)。
- [ ] AC4. 全量 `cargo test -p everlasting --lib` 绿 + clippy `-D warnings` + fmt。
- [ ] AC5. DEBT.md §RULE-QUEUE-001 销账;三份 spec(driver pattern /
  scheduled-tasks origin 链 / signature-run-chat-loop)同步收口。
