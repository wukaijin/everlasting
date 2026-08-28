# Design — RULE-QUEUE-001 多 drain 丢消息根治

## 1. 缺口机理(读码确认,2026-08-29)

```
驱动器每轮(chat.rs run_queue_driver):
  drained = drain_all(queue)                     // Vec<QueuedMessage>,FIFO
  turn_messages = reload_messages(db) ++ drained // 全部喂 LLM
  run_chat_loop(ChatLoopRequest { messages: turn_messages, origin: drained.last().origin, .. })

init 段(chat_loop/init.rs):
  next_seq = load_session(db).messages MAX(seq) + 1   // fresh 读,自算
  tail = messages.iter().rev().find(role == User)      // ← 只认尾条
  persist_turn(tail, seq); seq += 1
  …metadata 信封(update_message_metadata @ last_user_seq)
```

非尾条 drained 只存在于请求内存,无任何写入点 → reload 消失。
FTS5 external-content trigger 挂在 `messages` AFTER INSERT 上,走 `persist_turn`
即自动同步(无需额外处理)。auto-title 有 `title = '新对话'` CASE 守卫,
非尾条写入不会污染标题。

## 2. 方案取舍

| 方案 | 说明 | 裁决 |
|---|---|---|
| A. 驱动器侧预写 | drain 后先 persist 非尾条再 reload;需 driver 自算 next_seq(DB MAX 查询)+ extend 只留尾条,seq 契约逻辑出现第二份 | ✗ |
| B. init 段 persist 循环 | `ChatLoopRequest` 带 drained 全量,init 在尾条 persist 块**之前**循环补写非尾条;seq/失败语义/信封全部复用既有单点 | ✓ |
| C. persist 点改扫尾段 | init 对 `messages` 尾部切片循环——需注入 pass 后的 index 算术,脆 | ✗ |

DEBT fix 栏的两个方向(「驱动器/init 补持久化」或「persist 循环覆盖 drained 全体」)
取后者;落点是 init 段、数据源是请求字段,二者合一。

## 3. 字段形态

`ChatLoopRequest.origin: Option<TaskOrigin>` **替换**为
`drained: Vec<QueuedMessage>`(全量 drain 批次):

- 尾条 origin 派生:`request.drained.last().and_then(|qm| qm.origin.clone())`,
  `task_origin` 的两个消费点(persist 门控放宽 + 信封 `scheduled` 键)不变;
- 驱动器是唯一非空调用点:`turn_messages.extend(...)` 之后 move 进请求,
  零额外 clone;其余 5 个构造点(经典 chat.rs、群聊 ×2、worker drive.rs、
  tests_common builder)一律 `drained: Vec::new()`;
- 单字段承载完整 drain,避免「origin(尾)+ prior(非尾)」两字段对不上号的漂移面;
- QueuedMessage 的 `id/enqueued_at/priority` 对 init 冗余但无害,
  换取「这就是队列来的批次」的自文档性,不另造平价结构体。

不变量(写字段 doc):`drained` 非空 ⇒ 尾条 == `messages` 尾条 user
(驱动器 append 顺序保证);非驱动器路径恒空。

## 4. init 段 persist 循环(核心 diff)

位置:紧贴 `let (last_user_snapshot, last_user_seq) = …` 块**之前**
(seq 顺序:非尾条先落,尾条沿用既有块)。

```rust
if let Some((_, prior)) = request.drained.split_last() {
    for qm in prior {
        if qm.message.role != Role::User { continue; }      // 防御;队列今日恒 user
        if !skip_persist {
            persist_turn(.., &qm.message.content, seq, None, qm.message.speaker.as_deref())
                .await 失败 → emit_persist_failure + return Err(())   // RULE-A-003 同理
            // metadata 信封镜像尾条形状(gate:有附件或带 origin 才写)
            if !qm.message.attachments…is_empty() || qm.origin.is_some() {
                meta = {"injections": []} [+ "attachments"] [+ "scheduled"]
                update_message_metadata(.., seq, &meta)  失败 → warn 不致命(镜像尾条)
            }
        }
        seq += 1;
    }
}
```

要点:
- **seq**:`next_seq` 起算逐条自增;循环后既有尾条块无缝接上。init 开头的
  `load_session` 在任何写入前完成,驱动器为队列唯一消费者,无交错写者。
- **失败语义**:镜像 RULE-A-003 —— 宁可本轮可见失败,不让 LLM 答 DB 没记的话
  (这正是本债的病灶,不允许在新代码里重现)。
- **信封**:非尾条 gate 与尾条 F2 放宽同构(附件 ∨ origin 才写);
  `injections: []` 恒在,与尾条信封形状逐键对齐,前端 rehydrate 无歧义。
- **worker**:`skip_persist` 跳过整段(含 seq 自增照走,内存 loop 连贯性镜像尾条)。
- **群聊**:驱动器路径 `group_chat_state` 恒 None,与 dd_guard 无交叠;
  `drained` 非空 ⇒ 驱动器路径,数据空保证即防御。

## 5. 已知边界(有意接受)

1. 非尾条 @文件注入 manifest 不落 metadata(C5):`inject_at_tokens` 只产出
   尾条 manifest;DB 行存原始 `@relpath`,reload 重展开,内容无损,缺 hint 行。
2. 跨客户端 live 用户气泡不因本修复出现(C6):finalize 后 reload 可见。
3. `resend_seq` / `forced_dispatch` 仍为 round-0 尾条语义,非尾条不涉及。

## 6. 测试设计(改写 + 新增,tests_message_queue.rs)

- 改写 `multi_drain_pins_current_tail_only_persist_rule_queue_001`
  → `multi_drain_persists_all_drained_user_rows_rule_queue_001`:
  scheduled+manual 双条,断言两行都在且顺序 = drain 顺序、scheduled 行
  metadata `scheduled` 三键(task_id/task_name/fired_at)、manual 行 metadata None。
- 新增 ≥3 条 multi-drain(全 manual):三行全落、seq 连续有序、metadata 全 None
  (对照组,R4/防 additive 漂移,scheduled-tasks spec §5 同款要求)。
- 既有单 drain / 纯定时 / lost / dedup 测试零改动全绿 = AC3。

## 7. 回滚

单 commit(后端 + 测试 + spec + DEBT 销账);回滚 = revert 单点。
无 schema 变更、无 wire 变更(`ChatLoopRequest` 进程内值对象)、无迁移。
