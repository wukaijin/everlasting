<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->


## Pattern: SSE `stream-resync` 哨兵决策表(compute_replay,2026-08-24)

`daemon/sse.rs::compute_replay(buffer, next_id, last_event_id)` 是重连
客户端拿什么回放的唯一裁决点。**前 WP4 语义(2026-08-24 之前)只有
"ring 淘汰"一个哨兵臂,daemon 重启后 reconnect 拿到空回放不发哨兵** ——
前端自愈回路因此永不触发(实证:check pass F1,当年 Explore 研究报告
的"重启必收哨兵"是错的,代码才是真相)。现行决策表:

| `last_event_id` | buffer | 结果 |
|---|---|---|
| `None` | 任意 | 空回放(首连,不变) |
| `Some(last)` | 空 | **哨兵 `reason=restart`**(重启后首连;误报安全:前端以哨兵为触发做 DB 死亡预言机,尾部 `in_progress` → no-op) |
| `Some(last)`,`last+1 < oldest` | 非空 | 哨兵 `reason=buffer_overrun`(淘汰,原语义) |
| `Some(last)`,`last >= next_id` | 非空 | **哨兵 `reason=restart`**(跨进程 id 别名:新进程 id 从 1 重计,`id > last` 过滤会"看似当前"地漏放,但客户端的陈旧高 id 与新进程即将发出的低 id 会错位 —— 不能证明连续性就发哨兵) |
| `Some(last)` ∈ `[oldest-1, next_id-1]` | 非空 | 正常回放 `id > last`(不变) |

**约束**:

- 哨兵帧 `id: 0`、payload `{"reason":"restart"|"buffer_overrun"}` —— reason
  仅排障信号,前端不消费,不构成契约面。
- 两个新哨兵臂**可以 liberal**:前端 `handleStreamResync` 的死亡预言机
  (force `load_session` 看末尾 assistant 行 status)把误报消化为 no-op,
  后端无需精确判定"是否真死"。
- `large_payload_skips_buffer`(>256 KiB 帧不入 ring)造成的 id 空洞**不**
  发哨兵(先在行为,客户端靠 snapshot 自愈)。
- 改 `compute_replay` 必须同步决策表注释与测试(14 例锁全臂)。

---

## Pattern: SSE 契约测试唯一 home 在 sse.rs 内联;e2e.rs 只留路由级用例(RULE-TEST-003,2026-08-30)

**事故形态**:`tests/e2e.rs` 的 e1b 模块曾放 4 个纯 `SseRegistry` pub-API 单元
副本;WP4 改 `compute_replay` 语义时内联套件同步更新、e2e 副本未跟上(空 buffer
+ `Some(last)` 从静默空回放改为发哨兵,副本还断言旧行为)→ 永久红。叠加 e2e
从未进 CI,B1 给 `AddModelRequest` 加必填 `supports_images` 令 fixture 422 后,
干净 HEAD 长红近两周无人知。

**规则**:

1. SseRegistry / `compute_replay` 契约测试只写 `sse.rs` 内联套件(15 例,
   含决策表全臂锁定);**禁止在 `tests/e2e.rs` 复制同名/同义单元副本** ——
   镜像必漂,一处更新另一处必烂。
2. `tests/e2e.rs` 的定位是路由级集成:经 `build_router` 走公开 HTTP 面
   (httpmock 假 LLM + 零特权 seeding),e1a chat / e1c snapshot / e1d health /
   e1e 路由清单。
3. e2e 已进 CI(2026-08-30,`cargo test --test e2e` 紧随 `--lib`);wire 必填
   字段演进(如 `AddModelRequest` 加字段)会即时打红 e2e fixture,不再静默烂。
   排查 e2e 422 第一嫌疑 = req struct 反序列化拒收,diff 对应 `routes/*.rs`
   的 Request struct 必填字段。

---
