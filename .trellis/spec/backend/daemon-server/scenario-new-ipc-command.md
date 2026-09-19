<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->

## Scenario: 新增一个 IPC 命令(2026-08 + Phase 4 group-chat 沉淀)

### 1. Scope / Trigger

> 当需要新增一个前端 → 后端的命令(`transport.invoke("xxx", { ... })`),
> 必须在 **至少 4 处** 同步注册,缺一处即破该路径下的功能。

触发场景: Tauri Full 模式 + sidecar 模式 + 浏览器模式三条路径并行,
每条路径的「命令注册表」是独立的(各自维护自己的 cmd→domain 映射),
但所有路径都调用同一个 `#[tauri::command]` handler 的镜像实现。

### 2. Signatures

| 路径 | 入口 | 文件 |
|---|---|---|
| **后端实现** | `pub async fn xxx_inner(...)` | `app/src-tauri/src/commands/<domain>.rs` |
| **Tauri command 注册** | `#[tauri::command] pub async fn xxx(...)` | 同上文件 |
| **tauri 命令清单** | `commands::domain::xxx` 加进 `tauri::generate_handler!` | `app/src-tauri/src/lib.rs` (invoke_handler 块) |
| **daemon POST 路由** | `pub async fn xxx_handler(...)` + `.route("/xxx", post(xxx_handler))` | `app/src-tauri/src/daemon/routes/<domain>.rs` |
| **daemon router** | `xxx` 加进 `Router::new().route(...)` 链 | `app/src-tauri/src/daemon/routes/<domain>.rs`(`pub fn router(...)`) |
| **前端 transport 映射** | `xxx: "<domain>"` 加进 `CMD_TO_DOMAIN` | `app/src/transport/http.ts` |
| **前端 façade** | `async function xxx()` in store | `app/src/stores/<store>.ts` |

> **例外(不走此表)**:`routes/mcp.rs` 的 `/mcp` MCP 端点(2026-09-14)—— 绝对路径
> `merge` 装配(照抄 `stream.rs` 模式,非 `/api/v1/{domain}` nest)、无 Tauri command
> 对应物(消费方是 MCP 宿主客户端,不是前端),天然不进 `CMD_TO_DOMAIN` /
> routes-sync 守卫;JSON-RPC wire 契约与八工具语义见 docs/DAEMON-API.md §6.5,
> 协议层 + 工具单测内联在 mcp.rs。

### 3. Contracts

- **请求体**:snake_case(Rust 序列化)→ 经 `transformArgsTopLevel` 转 camelCase
  → 前端 store 用 camelCase 传。**不要在协议层混用两种命名**。
- **响应体**: 同上,反向。`#[serde(rename_all = "...")]` 决定具体命名。
- **错误体**: `AppCommandError` 统一 → REST 端 `IntoResponse` 序列化
  → 前端 `TransportError.body` 透传(`app/src/transport/types.ts`)。

### 4. Validation & Error Matrix

| 模式 | 缺 `CMD_TO_DOMAIN` 注册 | 缺 `lib.rs` `invoke_handler` 注册 | 缺 daemon route |
|---|---|---|---|
| **Tauri Full** | ✅ 走 IPC 通道,不查表 | ❌ 报 "command not found" | ❌ 路由 404(此模式不查 daemon) |
| **Sidecar(spawn daemon)** | ❌ 报 "unknown cmd ..." | ✅ 走 IPC(由 sidecar spawn 的 daemon) | ❌ 路由 404 |
| **浏览器模式(跨域访问 daemon)** | ❌ 报 "unknown cmd ..." | ✅ 直接调 daemon | ❌ 路由 404 |

**关键点**: Sidecar 模式 + 浏览器模式 **都** 走 httpTransport + `CMD_TO_DOMAIN`。
只有 Tauri Full 模式(Tauri 进程内 IPC)走 `invoke_handler` 注册。
**新加 IPC 命令时,最容易漏的就是 `CMD_TO_DOMAIN` 一行** ——
sidecar 模式下侥幸报错还好,纯浏览器模式下用户首次访问才发现。

### 5. Good/Base/Bad Cases

#### Good — 4 处齐全

```typescript
// app/src/transport/http.ts
"update_session_metadata": "sessions",  // (1) ✓
```

```rust
// app/src-tauri/src/lib.rs
.invoke_handler(tauri::generate_handler![
    commands::sessions::create_session,
    commands::sessions::update_session_metadata,  // (2) ✓
    // ...
])
```

```rust
// app/src-tauri/src/daemon/routes/sessions.rs
.route("/update_session_metadata", post(update_session_metadata))  // (3) ✓
```

#### Bad — 漏 `CMD_TO_DOMAIN`(Phase 4 group-chat 真实案例)

```typescript
// app/src/transport/http.ts:148  ← 漏!
export const CMD_TO_DOMAIN = {
  create_session: "sessions",
  load_session: "sessions",
  // ... 没有 update_session_metadata
};
```

**症状**: sidecar + 浏览器模式报 `TransportError(0, "unknown cmd \"update_session_metadata\" — no domain mapping ...")`。Tauri Full 模式侥幸(走 IPC 不查表)。

**Fix**: 在 `http.ts` 加 `"update_session_metadata": "sessions"` 一行。
trellis-check 报告问题;commit `002ac90` 修复。

### 6. Tests Required

- **后端 handler 单测**: 在 `commands/<domain>.rs` 或 `db/<domain>_tests.rs`
  直接调 `xxx_inner(...)`,验证 SQL/状态变更。
- **Tauri command 注册检测**: `cargo build --lib` 0 error(若漏 `lib.rs`,
  编译失败但错误不直观)。
- **daemon route 调用**: `daemon::server::tests::*` 用 `#[tokio::test]` + axum
  的 `Router` 直接调用,验证路由 + handler 联动。
- **前端 transport 映射**: **已有结构化守卫**(2026-08-25 落地):
  `app/src/transport/http.routes-sync.test.ts` 解析 daemon Rust 路由源码
  (`routes/mod.rs` 的 nest 映射 + 各域文件的 `.route(..., post(...))`),
  断言每条 POST 路由都在 `CMD_TO_DOMAIN` 中且 domain 一致 —— 新增命令漏
  加映射会在 `pnpm test` 直接红,不再依赖手动 e2e。历史同类遗漏四次:
  `update_session_metadata`(group-chat)、`save_attachment`(B1)、
  `get_web_search_config`(F4)、F1 三条 queue 命令。GET 路由(附件下载、
  session 快照、health、SSE)走 `<img>`/EventSource 直连,不进该表,守卫
  天然排除。仍建议 sidecar/浏览器模式各跑一次新命令做 e2e(守卫不校验
  handler 行为)。

### 7. Wrong vs Correct

#### Wrong — 只加 handler + lib.rs command,漏 daemon route

```rust
// commands/sessions.rs: 写好 handler (1) ✓
// lib.rs: 加 invoke_handler (2) ✓
// daemon/routes/sessions.rs: 漏 .route("/update_session_metadata", ...) (3) ✗
```

Tauri Full 模式工作,sidecar 直接 404。

#### Correct — 4 处齐全

```rust
// (1) commands/sessions.rs::update_session_metadata_inner
// (2) lib.rs 含 commands::sessions::update_session_metadata
// (3) daemon/routes/sessions.rs::router() 含 .route("/update_session_metadata", post(handler))
// (4) app/src/transport/http.ts::CMD_TO_DOMAIN["update_session_metadata"] = "sessions"
```

任何新 IPC 命令上线前,验证四条全在 — 推荐跑 `grep -rn "<cmd_name>" app/src-tauri/src/{lib.rs,daemon/routes} app/src/transport/`。

### 8. 真实案例

**Phase 4 (07-29-group-chat, 2026-07-31)**: 加 `update_session_metadata` 用来
runtime 编辑群聊参与者配置。代码层 + Tauri command + daemon route 三处齐全,
但 `CMD_TO_DOMAIN` 漏了一行。trellis-check 跨层 review 报告 L3 severity 1,
commit `002ac90` 修复。在此之前,sidecar / 浏览器模式下的群聊配置重编辑
全部失败。

---
