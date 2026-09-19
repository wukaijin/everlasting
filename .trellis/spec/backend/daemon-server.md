# Daemon HTTP Server Contract

> axum `everlasting-daemon` 进程的运维契约:serve loop、graceful
> shutdown(SSE 长连接 + agent loop drain)、shutdown 顺序。对应代码
> `app/src-tauri/src/daemon/` + `app/src-tauri/src/agent/helpers.rs`
> (`cancel_and_drain_all_agent_loops`)。
>
> daemon 化落地于 2026-07-20~23(remote-access epic),架构见
> [docs/ARCHITECTURE §4](../../../docs/ARCHITECTURE.md),编排放
> [docs/REMOTE-ACCESS-ROADMAP.md](../../../docs/REMOTE-ACCESS-ROADMAP.md)。

---


> **分篇**(2026-09-19):除 SessionSummary enrich(短小,留本文)外,各 Invariant/Scenario/Pattern 已按 tool-contract 模式拆至 `daemon-server/` 子目录(一章节一文件,原锚点以 stub 保留)。

## Part Index

- [invariant-orphan-guard.md](./daemon-server/invariant-orphan-guard.md) — daemon 生命周期绑定 GUI 进程(orphan-guard)
- [scenario-graceful-shutdown-sse.md](./daemon-server/scenario-graceful-shutdown-sse.md) — Graceful Shutdown 与 SSE 长连接(七节全)
- [pattern-shutdown-checklist.md](./daemon-server/pattern-shutdown-checklist.md) — 新增 streaming endpoint 的 shutdown 检查 + agent loop drain 闭合
- [pattern-ops-artifacts.md](./daemon-server/pattern-ops-artifacts.md) — 运维伴生物(备份 task + 日志文件,RULE-DAEMON-001)
- [pattern-sse-resync-and-tests.md](./daemon-server/pattern-sse-resync-and-tests.md) — `stream-resync` 哨兵决策表 + SSE 契约测试唯一 home(RULE-TEST-003)
- [scenario-new-ipc-command.md](./daemon-server/scenario-new-ipc-command.md) — 新增一个 IPC 命令(4 处齐全;八节全)
- [scenario-tunnel-node-id.md](./daemon-server/scenario-tunnel-node-id.md) — tunnel node_id 派生与自定义(含 set_tunnel_display_name 镜像契约)
- [pattern-deprecate-ipc-and-health.md](./daemon-server/pattern-deprecate-ipc-and-health.md) — 下线弃用 IPC 命令(RULE-SHIM-001)+ `/api/v1/health` stateless(RULE-HEALTH-001)

## Pattern: SessionSummary 运行时态 enrich(busy 字段,F6 2026-08-27)

DB 层恒 `busy:false`,**单点 enrich 在 `list_sessions_inner`**(`commands/sessions.rs`)——Tauri IPC 与 daemon REST 双入口共用,读 `session_active_request` map 置真。要点:

1. **运行时态不入库、不加列**:busy 是进程内存态(`session_active_request` 含即忙),DB schema 零改动;重启后自然 false(recover 链路另行标 interrupted)。
2. **claim 即注册 = busy 即亮**:F1-A 路由临界区 claim 后、F3 等闸期间也算在途(「已接受在途」语义),红点/关闭确认据此计数。
3. **enrich 只做单点**:严禁在 transport 各自的 handler 里分头 enrich(F1-A「路由口径统一」教训——双处 enrich 必漂移)。
4. **wire 是 additive 可选字段**:`SessionSummary.busy` 序列化恒出(新 daemon)、前端类型标 `busy?: boolean`(旧 daemon 无此字段不炸)。
5. 测试:daemon route 测试用 `serde_json::Value` + `is_boolean()` 断言(SessionSummary 无 Deserialize,不能走强类型往返);idle→busy→idle 三段断言。

