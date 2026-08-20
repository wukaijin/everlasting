# Design — TurnUsage 实时事件 + 5h 窗口配额视图

## 总体架构

```
后端(drive.rs Done 臂,单点)
  ├─ upsert_turn_trace_token(... + provider_id)   ← 既有,扩 1 参
  └─ emit ChatEvent::TurnUsage ──SSE──▶ streamEvents.ts "turn_usage" case
                                         └─ traceStore.applyEvent upsert
                                              └─ TurnCard token cells 即时可见

WP2 聚合(零新表)
  turn_trace(created_at + token_usage_json + run_id + provider_id 新列)
    └─ usage_window IPC(SQL 聚合)──▶ useQuotaStore ──▶ AppHeader <QuotaChip> + 弹层
```

## WP1 — ChatEvent::TurnUsage

### Wire 契约

`app/src-tauri/src/llm/types/event.rs` 新变体(serde tag `kind = "turn_usage"`,snake_case 字段):

```rust
TurnUsage {
    request_id: String,          // activeRequests gate 路由键(同 BudgetTrim)
    seq: i64,
    run_id: String,              // '' 主 loop;worker = subagent_runs.id
    usage: TokenUsage,           // 五字段结构体复用
    tools_token: Option<u32>,
    memory_token: Option<u32>,   // worker 行 None(列契约 drive.rs:1315-1321)
    images_token: Option<u32>,
    at_files_token: Option<u32>,
    system_token: Option<u32>,
    context_window: u32,
}
```

read-only / 非持久化(同 `Recall` / `BudgetTrim` / `Retrying` 族)。

### Emit 点与不变量

- **位置**:`drive.rs` Done 臂 `upsert_turn_trace_token` 旁(`drive.rs:1322` 所在块)。emit 与 DB 写同点同值 → AC1 一致性天然成立;**emit 不放进 DB 写的 `if let Err` 分支内**(DB 失败事件照发,AC3)。
- **双层门边界(评审 P2)**:emit 与 upsert 同在 `if let Some(t) = usage` + `if !skip_persist || !run_key.is_empty()` 双层门内——`usage=None`(取消/错误终止)与 worker `insert_run` 失败降级(run_key='')的 Done **不发事件**,前端该轮 cells 保持 "—"(与"事件缺失退化为现状"自洽)。AC1 只覆盖正常完成轮。
- **worker 路径**:worker(run_key 非空)同点 emit,但 `SubagentBufferSink::emit_chat_event` 对 `TurnUsage` **早退跳过**(实现期实证修正:不挡则 worker 每轮多一条无人渲染的 transcript 行,`persists_subagent_run` 的 3→4 计数断言破——评审 §2 的"容忍"预警实际是"必须挡")。早退在 `record()` 之前,同时挡掉 transcript 落录与 `subagent:event` live 转发(record() 一身兼两职);worker 维度的持久记录走 turn_trace run 行的 Done 臂 upsert(不经 sink)。主 chat 无 activeRequest 对应 worker rid → 不路由(AC2)。emit 侧(drive.rs)不做 worker 特判。
- **时序**:emit 在 upsert 前后皆可(无消费者依赖顺序);建议 upsert 后,与"落了才报"的语义对齐。

### 前端路由(budget_trim 模板)

- `streamEvents.ts` 新 `case "turn_usage"`:防御性字段检查(malformed drop,同 `budget_trim` 的 `streamEvents.ts:705-713` 模式)→ `useTraceStore().applyEvent({ kind: "turn_usage", ... })`。
- `traceStore.applyEvent` 新分支:**merge 语义** upsert(`.set(seq, { ...existing, tokenUsage, toolsToken, ... })`)——同 seq 行可能已有 compaction/loopHint/breadcrumb(live 事件先到),不得覆盖。
- `turnTrace.ts` 加 `TurnUsageEvent` wire 类型 + `applyEvent` 联合类型扩员。
- TurnCard 零改动:`TurnTrace` 字段已齐(`turnTrace.ts:187-227`)。

## WP2 — 5h 窗口配额视图

### 归因列:`turn_trace.provider_id TEXT NULL`

- **加列**:`add_turn_trace_column_if_missing` 幂等 helper(`migrations/columns.rs:190` 先例)追加在 migrate 链尾;**三处 CREATE 同步**(评审 P3):`schema.rs:944` 绿地 CREATE、`schema_helpers.rs:290` 重建模板(`rebuild_turn_trace_with_run_id` 拷贝列清单是显式的,**加列须在重建迁移之后执行,或重建模板列清单同步补列**——顺序以 `list_worker_turn_traces` 那次为模板)、`db/trace.rs:851` 测试内 CREATE。
- **provider_id 穿参路径(评审 P1 修正,不走 LoopInit)**——`provider` / `context_window` 本就是 `run_chat_loop`(`chat_loop.rs:320-321`)→ `drive_turn`(`drive.rs:105-106`)的既有参数,`Provider` trait 无 id 成员(`provider/mod.rs:66`),须显式解析:
  - **主路径**:`lookup_provider_for_session`(`chat.rs:650`)内 `list_models` 已取 ModelRow → `ResolvedChatProviderWrapper`(`chat.rs:770`)加 `provider_id: Option<String>` 字段 → `chat_inner` → `run_chat_loop` 新增第 N 参(仿 `worktree_override` pass-through 先例)→ `drive_turn`。
  - **worker 路径**:`resolve_worker_provider`(`resolve.rs:151`)catalog 命中分支内 `get_model(db, mid)` 已 fetch(`resolve.rs:168`),`model_row.provider_id` 顺手可得——返回值从三元组扩为含 `Option<String>` provider_id;`WorkerModel`(`dispatch/model.rs:45`)加同名字段;`run_subagent` → worker 的 `run_chat_loop` 调用点穿入。catalog miss / inherit-parent 分支 = None(落 NULL)。
  - None(catalog miss / 极端)= 落 NULL,聚合时归 "unknown" 桶。
- **回填**:migration 一次性 `UPDATE turn_trace SET provider_id = (SELECT m.provider_id FROM sessions s JOIN models m ON s.model_id = m.id WHERE s.id = turn_trace.session_id) WHERE provider_id IS NULL`——历史行按 session 当前模型近似(session model 指向已删模型时 join 不到 → NULL → unknown 桶兜底),可接受。

### 聚合 IPC:`usage_window`

- **db 层** `db/usage.rs`(新):
  ```sql
  -- per-provider 累计 + 主/worker 拆分
  SELECT provider_id, run_id = '' AS is_main,
         SUM(json_extract(token_usage_json, '$.input_tokens')),
         SUM(json_extract(token_usage_json, '$.output_tokens')),
         SUM(json_extract(token_usage_json, '$.cache_read_input_tokens')),
         SUM(json_extract(token_usage_json, '$.cache_creation_input_tokens'))
  FROM turn_trace
  WHERE created_at >= datetime('now', ?window_hours_modifier)
    AND (provider_id = ? OR ? IS NULL)   -- 可选 provider 过滤
  GROUP BY provider_id, is_main;

  -- 小时分布 / top sessions 同表 GROUP BY strftime / session_id LIMIT N
  ```
  `token_usage_json` 用 `json_extract`(SQLite 内建);`window_hours` 经 config 读(缺省 5),**不拼字符串**(绑参 `-{h} hours` modifier)。
- **command 层** `commands/usage.rs`:`usage_window()`(POST)+ `set_quota_settings(window_hours, limit_tokens)`(写 `config` 表 `quota_window_hours` / `quota_limit_tokens`,模板 `commands/config.rs:211`)。**注册链 5 处 + 测试锚**(评审修正,原写 6 处):daemon routes 注册 + `post()` 映射、`lib.rs` invoke、`commands/mod.rs` 白名单、`http.ts CMD_TO_DOMAIN`(D2 P1 先例)、`http.test.ts` 回归锚(handoff_session 先例 `http.test.ts:136`)。
- 返回 shape:`{ windowHours, limitTokens?, generatedAt, providers: [{ providerId, displayName?, totals, mainTotals, workerTotals, hourly: [...] }], topSessions: [{ sessionId, title, inputTotal, outputTotal, windowInput }] }`(displayName join providers 表,join 不到用 provider_id 兜底)。

### 前端:QuotaChip + 弹层

- **`<QuotaChip>`**(AppHeader,trace toggle 旁):常驻显示窗口总量(缩写 `1.2M`);设了 limit 则加占比条。数据 `useQuotaStore`。
- **刷新触发**:① 弹层打开时;② 每次 request finalize 后(streamController `reloadAfterFinalize` 尾部 fire-and-forget 重查,一次轻查询);③ chip 挂载时。**不做定时轮询、不做客户端增量推算**(窗口是滑动的,客户端推算会漂;5h 尺度下"下一轮后刷新"足够)。
- **`<QuotaPopover>`**(reka-ui Popover):per-provider 段(总量 + 主/worker 拆分 + 占比条)+ 小时分布(轻量 CSS 柱,不引图表库)+ top sessions(点击 `openSessionInProject` 跳转,D2 已有组合 action)+ 配额设置行(window hours / limit 输入,调 `set_quota_settings`)。
- **移动端降级**(S6a 窄屏档先例):<430px chip 收缩为纯图标或并入溢出区,弹层降级全屏。

## 关键取舍记录

| 决策 | 备选 | 取舍理由 |
|------|------|----------|
| 新事件变体 vs 扩 Done payload | 扩 `Done` 加 optional 字段 | Done 由 provider stream 发出,切片值在 agent loop 层才拼齐;扩它会扭曲事件归属(TurnUsage 是 loop 层观测) |
| worker live 不做 | TurnUsage 冒泡到 drawer | 08-20 刚落 DB 点查 + 粘性缓存,够用;冒泡要动 sink 隔离面,收益低 |
| provider_id 落列 vs join sessions 聚合 | 纯 join | join 对 worker 换 provider 归因失真;落列一次写准,聚合 SQL 也简单 |
| 聚合实时性 = finalize 后重查 | 定时轮询 / 客户端增量 | 滑动窗口客户端推算必漂;轮询空转;finalize 后查一次语义刚好("跑完这轮烧了多少") |
| limit 可选(NULL=只显示) | 强制设额度 | 套餐额度本身不透明,强制设会逼用户编数字 |

## 兼容 / 回滚

- 全部 additive:新事件变体(旧前端 drop)、新列(NULL)、新 IPC。无既有行为改写。
- 回滚 = revert 单 commit 粒度(WP1 / WP2-migration+IPC / WP2-前端 三段);`provider_id` 列留着无害。
- 风险点:upsert 12 参 + provider_id 穿参链(`ResolvedChatProviderWrapper` / `resolve_worker_provider` 扩返回值 → `run_chat_loop` 新参 → `drive_turn`)是机械扩参,测试锁双路读(`db/trace.rs` 既有测试模式);`json_extract` 对 NULL `token_usage_json` 行返回 NULL,SUM 忽略,天然安全。
