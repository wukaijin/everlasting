# Research: turn 持久化链路全景(RULE-PERSIST-001)

> 2026-08-24 主会话研究沉淀。来源:主会话精读核心文件 + Explore 子代理扫周边链路(报告全文要点并入本文)。

## 1. 当前持久化时序(问题所在)

一个 user 请求的生命周期(`run_chat_loop`,`app/src-tauri/src/agent/chat_loop.rs:319`):

```
loop init (init.rs)
  ├─ load_session → seq = max(existing seq)+1   (init.rs:163-170)
  └─ user 消息落库(先于 LLM 调用,commands/init 层)
每 turn:
  drive_turn (drive.rs:82)
    ├─ 关卡⑤ budget gate(drive.rs:950-1035)
    ├─ retry_open → LLM stream(drive.rs:1076-1110)
    ├─ 流式循环(drive.rs:1112-1453):Delta/ThinkingDelta/ToolCall 等事件
    │    全部累积在内存:ordered_blocks + pending_text + pending_thinking
    │    (交错思考,drive.rs:1038-1062 注释块)
    └─ 流结束后:assistant 行一次性落库 ← drive.rs:1567 persist_turn(裸 INSERT)
       cancel/error 路径也走同一落点点(先追加 marker 块,drive.rs:1502-1522)
  dispatch_tool_calls (tools.rs) — 工具执行,结果全部在内存
  finalize_turn (tools.rs:1779) — tool_result 行(user-role)落库
```

**崩溃窗口**(daemon kill -9 / OOM / 断电,SIGTERM 有 drain 保护见 §5):

| 窗口 | 内存中丢失 | DB 残留 | 后果 |
|---|---|---|---|
| W1: LLM 流式中 | 整个 assistant turn 已生成内容 | 只有 user 行 | **内容全丢**(本任务主目标) |
| W2: 工具执行中 | tool_results | assistant(tool_use) 行已落,无配对 tool_result | 内容丢 + **孤儿 tool_use → 下次请求 400**(pair atomicity) |

W2 的 400 风险有实证先例:error 路径专门为此合成 is_error tool_result(drive.rs:1685-1697 注释,OpenAI 400 / Anthropic 2013)。崩溃路径今天无任何修复。

## 2. 关键代码锚点

- **assistant 落库唯一点**:drive.rs:1567(`persist_turn`);`seq += 1` 在 1616;空 turn(`assistant_blocks.is_empty()`)不落库
- **persist_turn**:db/sessions/messages.rs:28,裸 INSERT INTO messages(13 列),user 行触发 auto-title;**无 ON CONFLICT**
- **seq**:caller-managed,`UNIQUE(session_id, seq)`;init 从 DB max+1 → 恢复的检查点行天然被计入下一个 seq
- **marker 常量**:helpers.rs:386 `CANCELLED_MARKER="[已停止]"`、:397 `ERROR_MARKER="[生成出错中断]"`;追加为独立 Text 块 + `\n\n` 前缀(drive.rs:1491-1522)
- **build_synthetic_tool_result_message**:helpers.rs:78(error/cancel 路径复用,W2 修复可同款)
- **messages 表**:无 status/lifecycle 列(schema.rs:155-186);加列走 `add_messages_column_if_missing`(columns.rs:70,幂等探针式,**无版本号 migration 体系**,run_migrations 每次 startup 全量跑,schema.rs:25-1239)
- **落库失败语义**:RULE-A-003 —— assistant 落库失败 emit Error + abort(drive.rs:1550-1589);cancel/error 路径 log-only

## 3. worker / subagent 路径(不受影响)

- worker 复用 run_chat_loop 但 `skip_persist=true`(dispatch/drive.rs:143-149)→ 中间 turn 不进 messages 表,记录在 `subagent_runs`
- **subagent_runs 就是本任务要的先例**:running 占位行(dispatch/register.rs:115-148 insert_run_with_id)+ 终态 update_run_finished + **启动 reap**(state.rs:311 `reap_orphaned_runs`,best-effort log-only non-fatal,把残留 running 行标 error)
- worker 的 turn_trace 走 run 行(`!skip_persist || !run_key.is_empty()` 门,drive.rs:1319)——检查点**不需要**这个:worker 无落库需求,门在 `!skip_persist` 即可

## 4. SSE / 前端(崩溃时的 UI 表现)

- daemon 进程内全局 `SseRegistry`(sse.rs:107-117):live senders + **512 帧 replay ring**;`GET /api/v1/stream`(routes/stream.rs:52-69);重连协议 Last-Event-ID → replay,gap 时发 `stream-resync` 哨兵(sse.rs:78-80)
- **daemon 重启后**:replay buffer 空 → 客户端 EventSource 自动重连(auto-reconnect,http.ts:275-279)→ 必收 stream-resync 哨兵
- **但 stream-resync 无任何消费者**(仅 http.ts:27-29 注释提及)——设计好的"resync → GET snapshot 重拉"回路没接上线
- chat POST 立即返回(fire-and-forget,routes/agent.rs:44-72)→ 前端 fetch-catch 不触发;无 done/error 到达 → **activeRequests 占位 + streaming 光标永久卡死**(无 watchdog)
- snapshot 端点已存在:`GET /api/v1/sessions/:id/snapshot`(routes/sessions.rs:104-114 = load_session + pending_interaction)
- 崩溃后重载页面/切 session:load_session 读 messages 表 → 恢复的检查点行自然可见

## 5. 启动序列与既有恢复

- `AppState::load_inner`(state.rs:286):migrations(state.rs:301)→ **reap_orphaned_runs(state.rs:311,唯一现存恢复逻辑)** → provider catalog → backup task(daemon/server.rs:71-116)
- 新恢复 pass 落点:reap_orphaned_runs 之后,同款 best-effort 语义
- SIGTERM 优雅停机已有 drain:`shutdown_signal`(server.rs:360-402)→ cancel_and_drain_all_agent_loops(8s 预算,server.rs:272)——本任务只管 drain 覆盖不到的 kill -9/OOM
- GUI 进程路径:state.rs:286 `app: Option<AppHandle>`,daemon/GUI 共用 load_inner → 恢复 pass 两边都生效

## 6. 上下文构造 / pair atomicity

- `group_droppable_turns`(context.rs:497):压缩侧把 assistant(tool_use)+user(tool_result) 视为原子组;RULE-A-001 不变量
- **请求构造侧无孤儿修复**——W2 孤儿直接打到 provider 400(见 §1)
- 恢复必须维持 pair atomicity:合成 tool_result 行(is_error + 中断说明)是 error 路径已验证的手法

## 7. turn_trace / usage 写点(不受影响,记录备查)

- turn_trace token 行:Done 臂 upsert(drive.rs:1325),崩溃 turn 无 Done → 无 trace 行(可接受,不在本任务范围)
- usage 无独立表:db/usage.rs 是 turn_trace 上的只读聚合
- sessions.last_*(snapshot):Done 臂 update_last_turn_usage(drive.rs:1281,`!skip_persist` 门)

## 8. 测试基建(集成锁模式)

- `make_harness()`(agent/tests_common.rs:239):TempDir + 真实 pool + run_migrations + 真实 project/session;MockProvider 脚本化响应
- 断 DB 行先例:tests_agent_loop/error_persist.rs(`load_assistant_rows` :22);emit==DB 同点同值锁:tests_agent_loop/turn_usage_event.rs
- daemon 级:server.rs:639 引用 basic.rs:678 cancel 测试

## 9. 设计直接结论

1. 检查点只覆盖 **主 chat 的 assistant 流式行**(`!skip_persist` 门);W2 工具执行窗口不做中途检查点,靠启动时孤儿修复(MVP 边界)
2. 检查点行与最终行同 seq → assistant 落库点(drive.rs:1567)必须改 **upsert**(否则 UNIQUE 冲突误报 RULE-A-003);其余落库点(user 行 / tool_result 行)seq 不同,永不冲突
3. 启动恢复 = reap 同款:① in_progress 行 → 空删 / 有内容加 marker + status='interrupted';② 全 session 尾部孤儿 tool_use → 合成 is_error tool_result 行(覆盖 W2,独立于 ① 扫描)
4. 前端 stream-resync 消费者是补完"崩溃 → 重连 → UI 自愈"回路的自然钩子(哨兵 daemon 重启后必发),范围决策待用户
