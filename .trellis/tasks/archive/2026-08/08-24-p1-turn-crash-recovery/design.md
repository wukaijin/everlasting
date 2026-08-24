# Design: turn 流式持久化 + 崩溃恢复

> 需求见 `prd.md`,链路证据见 `research/persistence-path-map.md`。设计原则:**贴 subagent_runs 先例**(running 占位 + 终态覆盖 + 启动 reap),不发明新机制。

## 1. 数据模型

### 1.1 messages 新列 `status`

```sql
-- migrations/schema.rs 新增(幂等探针式,对齐既有 add_messages_column_if_missing 用法)
add_messages_column_if_missing(pool, "status", "TEXT")   -- NULL = 终态(全部存量行)
CREATE INDEX IF NOT EXISTS idx_messages_status
  ON messages(status) WHERE status IS NOT NULL;          -- 部分索引:空表况下零维护成本
```

取值域(`CHECK` 不加 —— 存量表加 CHECK 要表重建,收益低;由写入点保证):

| 值 | 语义 | 写入点 |
|---|---|---|
| `NULL` | 终态(默认,存量行/正常收尾行) | 最终落库 |
| `'in_progress'` | 流式检查点行 | 占位/检查点 upsert |
| `'interrupted'` | 崩溃后恢复过的行 | 启动恢复 pass |

`MessageRow`(db/models)加 `status: Option<String>`;wire 层面 `load_session` 返回体多一个可选字段,前端 TS interface 加 `status?: string | null`(snake_case 镜像约定,BACKLOG §5.2)。

### 1.2 状态机

```
(stream 就绪) ──占位──▶ in_progress ──周期 upsert(同 seq)──▶ in_progress
                            │
              正常/cancel/error 收尾:最终 upsert(内容+latency, status=NULL)
              空 turn 收尾:DELETE 该行
                            │
              daemon 崩溃(行停留 in_progress)
                            │
              启动恢复 pass:内容空 → DELETE;有内容 → +marker 块, status='interrupted'
```

一行只在一个 turn 的生命周期内流转;跨请求 seq 不复用(init max+1),不存在旧检查点行撞新请求的可能。

## 2. 写点设计(drive.rs)

### 2.1 `TurnCheckpoint` 小结构(drive.rs 内)

```rust
struct TurnCheckpoint {
    last_write: Instant,
}
const CHECKPOINT_INTERVAL: Duration = Duration::from_millis(1000);
```

三个调用点,全部 `!skip_persist` 门:

1. **占位**:stream ready 之后(drive.rs:1092 `match outcome` 后)→ `upsert_in_progress_turn(db, sid, seq, &[], speaker)`。
2. **周期检查点**:流式循环 `Delta` / `ThinkingDelta` 两臂(其余臂低频,不挂)开头:`if last_write.elapsed() >= INTERVAL { write }`。快照构造**只读克隆**累加器,不调用会变异的 `flush_*`:
   ```rust
   let mut snap = ordered_blocks.clone();
   if let Some(t) = &pending_text { snap.push(Text{ text: t.clone() }) }        // 顺序:text 在 thinking 后?
   if let Some(p) = &pending_thinking { snap.push(Thinking{ ... p.clone() }) }  // 见 §2.2 注
   ```
   写失败 `tracing::warn!` 后继续流式(best-effort,与 subagent insert_run 失败降级同族)。
3. **收尾**:现有 drive.rs:1567 落库点改调新 fn(§3.2);`assistant_blocks.is_empty()` 分支删占位行。

### 2.2 快照块序的正确性

流式循环的不变式:`pending_thinking` 在 `pending_text` **之前**开始(文本到达会先 finalize thinking;思考到达会先 flush text)。因此任意时刻至多一个 pending,且:
- 只有 `pending_text` → snap = blocks + [Text]
- 只有 `pending_thinking` → snap = blocks + [Thinking]
- 理论上两者非空并存不可能;若并存(防御),按 thinking→text 序追加(thinking 先开始必然先 finalize)

检查点行不追求块级完美(缺 signature 的 thinking 块、未配对的 pending text 均可接受)——它只是恢复兜底,终态行仍由收尾路径完整落库。

### 2.3 为什么不动 finalize_turn / tool_result 路径

tool_result 行 seq = assistant seq+1,与检查点行不同 seq,永不冲突;W2 丢失内容由启动孤儿修复兜底(PRD Out of Scope 已划界)。改动面收敛在 drive.rs 一个文件 + db 层。

## 3. DB 层设计(db/sessions/messages.rs)

### 3.1 新函数

```rust
upsert_in_progress_turn(pool, session_id, seq, blocks: &[ContentBlock], speaker: Option<&str>)
  // INSERT ... ON CONFLICT(session_id, seq) DO UPDATE SET
  //   content/text/has_tool_calls/has_tool_results/status='in_progress', speaker
  // 复用 persist_turn 的 JSON/文本派生逻辑(提取私有 helper content_columns())

finalize_turn_persist(pool, ..同 persist_turn 签名..)
  // INSERT ... ON CONFLICT(session_id, seq) DO UPDATE SET 全内容列 + latency 列 + status=NULL
  // 仅 drive.rs:1567 assistant 落库点使用
  // 注意:auto-title 分支保持(user 行永不走此 fn,实际不会触发)

delete_in_progress_turn(pool, session_id, seq)
  // DELETE FROM messages WHERE session_id=? AND seq=? AND status='in_progress'
  // 带 status 条件:只删自己的占位,误用时不吃终态行
```

### 3.2 persist_turn 本体不动

其余调用点(user 消息、finalize_turn 的 tool_result、合成行)保持裸 INSERT:UNIQUE 冲突在今天承载"seq 漂移 bug"的告警语义(RULE-A-003 家族),全量 upsert 会把真 bug 静默成覆盖。只有 assistant 落库点知道自己"前面有检查点行",由它独占 upsert。

### 3.3 启动恢复 pass

落点:`state.rs:311` reap_orphaned_runs 之后,同款 best-effort 壳(match + 日志形态对齐)。

```rust
// db/sessions/messages.rs
pub async fn recover_interrupted_messages(pool) -> sqlx::Result<RecoveryReport>
```

两步:

**Step A(in_progress 残留)**:
```sql
SELECT id, session_id, seq, content FROM messages WHERE status='in_progress'
```
逐行:解析 content JSON → blocks;空(blocks 空 or 仅空文本)→ DELETE;否则 → blocks.push(Text{INTERRUPTED_MARKER})(独立块 + `\n\n` 前缀,对齐 drive.rs:1491 marker 约定)→ UPDATE content/text/status='interrupted'。

**Step B(尾部孤儿 tool_use,覆盖 W2)**:
```sql
-- 每 session 尾行(UNIQUE(session_id,seq) 走索引)
SELECT m.* FROM messages m
JOIN (SELECT session_id, MAX(seq) AS s FROM messages GROUP BY session_id) t
  ON m.session_id = t.session_id AND m.seq = t.s
WHERE m.role='assistant' AND m.has_tool_calls=1
```
逐行:解析出 ToolUse{id} 列表 → 构造合成 tool_result blocks(复用/镜像 `build_synthetic_tool_result_message` 的 is_error 语义,内容注明"daemon 异常中断,工具结果丢失")→ 按 seq+1 INSERT user 行(bare INSERT 安全:该 seq 必不存在,否则就不是尾行)。Step A 标成 interrupted 的行若本身含 tool_use,同样需要此修复(Step B 在 Step A 之后跑,能吃到)。

受影响 session 批量 `touch_session`。返回 `RecoveryReport { interrupted: usize, deleted: usize, orphan_repaired: usize }` 供日志。

**事务性**:逐行独立写,不包大事务(与 reap 一致;SQLite 单写者下中途失败 = 下次启动重跑,幂等)。

## 4. 前端(R3,若纳入)

`app/src/transport/http.ts` 已按事件名分发;新增 `streamController.ts` 一处监听:

```
on("stream-resync"):
  对每个 streaming 中的 activeRequests 条目 → 本地终结占位(标 interrupted,不弹 error toast)
  → 对当前 session 调 ensureLoaded(force) 重拉(load_session 返回已含恢复行)
```

复用 `reloadAfterFinalize`(streamEvents.ts:1165)的形态但走"中断"而非 done/error 终态分支,避免触发配额刷新链副作用。`stream-resync` 也在 daemon 空重启(无崩溃)时触发 → 该路径对无 activeRequests 的情形是 no-op,安全。

## 5. 兼容与迁移

- 新列 + 部分索引,幂等;存量行 status=NULL 语义即终态,零回填。
- 崩溃时占位行可能出现在旧版二进制读到的 DB 上(降级):旧版 load_session SELECT * 多列无害,唯一行为差异是那行会被当普通空/部分消息渲染——可接受(比丢数据好)。
- 回滚:列留在表里无人写即惰性;代码回退后 in_progress 残留行会被下次启动的恢复 pass……不在(旧版无此 pass)→ 手动 `UPDATE messages SET status=NULL WHERE status='in_progress'` 一句话清干净,写进 implement.md 回滚节。

## 6. 权衡记录

| 决策 | 取 | 舍 | 理由 |
|---|---|---|---|
| 检查点载体 | messages 行内(status 列) | 独立 checkpoint 表 | 少一张表一个 join;subagent_runs 先例就是行内 status |
| 检查点触发 | 时间门(1s) | 每 N delta / 每 token | 有界丢失窗口;delta 大小差异大,时间是最公平的度量 |
| W2 覆盖 | 启动孤儿修复 | 工具执行中途检查点 | 工具结果在执行完才产生,中途无可查内容;修复 400 是硬需求,内容丢失已划出 scope |
| upsert 范围 | 仅 assistant 落库点 | persist_turn 全量 upsert | 保住 UNIQUE 冲突的 bug 告警语义(RULE-A-003) |
| marker 呈现 | 文本块(前端零改动) | status 字段驱动徽标 | MVP;status 字段已入库,未来 UI 化只加不减 |
| 快照构造 | 只读克隆 | 复用 flush_* 变异式 | flush 会破坏流式循环的 pending 状态机 |

## 7. 测试设计

- **单元(db)**:upsert 幂等/覆盖顺序、finalize 清 status、delete 只吃 in_progress、Step A 空删/有内容标 interrupted、Step B 尾孤儿修复 + 干净 DB no-op + 多 session。file-backed 建池(backup.rs 教训:sqlx :memory: 池有静默 no-op 坑)。
- **集成(tests_agent_loop)**:① 正常 turn 后无 in_progress 残留 + 行内容与既有断言一致(改造 error_persist 一例加 status 断言);② MockProvider 发 Delta 后 cancel → 检查点行被终态覆盖;③ AC4:构造孤儿尾行 → 跑 recover → run 第二个请求不炸(pair 层断言)。
- **时间门单测**:checkpoint 间隔逻辑提出纯函数(`should_checkpoint(elapsed)`)或直接测 `TurnCheckpoint`,不起 1s 真等待。
- **前端(若 R3)**:fake stream-resync 事件 → activeRequests 清空 + load_session 被调(vitest,对齐既有 streamController 测试形态)。
- **live**:turn-smoke.sh(检查点写不干扰 usage 断言)。
