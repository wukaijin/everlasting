# Design:everlasting-acp shim(bin)

> 依据:research/acp-integration-analysis.md(§3 全量映射表、§4.2 改动面、§4.3 取舍 8 项)。本文件把调研结论落成实施级设计:crate 落位、模块划分、并发模型、错误模型、测试策略。daemon 侧零改动是硬约束。

## 1. 架构总览

```
Zed(ACP client)                everlasting-acp(shim,本任务)              daemon(:7456)
  │  spawn 子进程,stdin/stdout     │                                        │
  │  按行 JSON-RPC(ACP v1)  ←──→  │  crate Builder dispatch + Stdio         │
  │                                │  ┌─ daemon.rs  HTTP 客户端(归一层)────→ /api/v1/*(agent/chat,
  │                                │  │                                      permissions/*, cancel/*,
  │                                │  │                                      sessions, projects, modes)
  │                                │  └─ sse.rs     全局流消费任务 ─────────→ GET /api/v1/stream(SSE)
  │                                │     (启动即挂、全程保持、永不主动断)       │
  │  ← session/update 通知流 ──────┤  translate.rs: ChatEvent/tool 事件 → update 变体
  │  ← session/request_permission ─┤  (permission:ask 经 SSE 到达,反向请求 Zed)
  │  ── 应答 optionId ────────────→│  → POST permission_response{rid, decision}
```

- 第五客户端形态:与 Tauri Thin / 浏览器 SPA / 远程 PWA / evl CLI 并列,消费同一 daemon HTTP/SSE 面。Rust 版「evl chat 消费循环」(cli/lib/chat.mjs 的翻译,通路已验证)。
- 单向依赖:shim → daemon HTTP/SSE;不链接 everlasting 主 crate,不依赖 agent core。

## 2. Crate 落位与依赖

- 新 workspace 成员 `crates/everlasting-acp/`,**加入 `members`,不加入 `default-members`**(根裸 `cargo build` 不拖它;测试由 CI 显式 `-p everlasting-acp` 接线,PR4)。
- `[[bin]] name = "everlasting-acp"`(无 lib target;测试用 `#[cfg(test)]` 模块 + dev-dependencies 起 mock)。
- 依赖(全部轻量,零系统库,无 PKG_CONFIG_PATH):
  - `agent-client-protocol = "1.3"`(锚 1.x = 协议 v1 stable;**不带任何 `unstable_*` feature**)——提供 `schema::v1` 全套类型(带 `JsonRpcRequest`/`JsonRpcNotification` derive)、`Builder` dispatch(`on_receive_*` handler)、`Stdio` transport、`ConnectionTo`(发通知/反向请求 + spawn 任务)。docs.rs 1.3.0 已核实存在;确切 API 签名实施时以 `~/.cargo/registry` 里 crate 源码为准。
  - `tokio`(rt + macros + process——测试 spawn shim 子进程用)+ `reqwest`(rustls,stream feature——SSE 消费要 `bytes_stream`)+ `serde`/`serde_json` + `thiserror` + `tracing` + `tracing-subscriber`(stderr 输出;ACP 规定 stdout 只准协议帧)。
  - dev-dependencies:HTTP mock(推荐 `wiremock`,支持流式 body 模拟 SSE;若评估不顺,降级手写最小 axum——二选一由实施者定,倾向 wiremock)。
- 版本策略:`Cargo.toml` 里 crate 版本从 workspace 根统一(现状 workspace 未统一依赖版本,各 crate 自管;保持自管)。

## 3. 模块划分

```
crates/everlasting-acp/src/
  main.rs        入口:解析 env(DAEMON_URL,默认 http://127.0.0.1:7456;日志级别),
                 tracing 初始化(stderr),健康检查,Builder 装配,Stdio serve
  daemon.rs      daemon HTTP 客户端封装 + payload 归一层(见 §5)
  sse.rs         SSE 消费:GET /api/v1/stream,行解析(event:/id:/data:),
                 重连(Last-Event-ID;复用 512 帧 replay buffer 语义),
                 内部事件枚举 DaemonEvent 归一后广播给订阅者(broadcast channel)
  handlers/      ACP 方法 handler(由 Builder on_receive_* 挂接)
    mod.rs         session 表(ACP sessionId → daemon session_id / project_id /
                   当前 request_id / 消费任务 handle)
    initialize.rs  initialize / authenticate(空)/ logout(空)
    session.rs     session/new, session/load, session/list, session/prompt,
                   session/cancel, session/set_mode
  permission.rs  permission:ask(SSE 事件)→ session/request_permission(反向请求)
                 → optionId 解码 → permission_response 应答环
  translate.rs   纯函数映射层(全部无 IO,单测主战场):
                   ChatEvent kind → session/update 变体 + ContentBlock
                   tool:call / tool:result → ToolCall(pending)/ ToolCallUpdate
                   stop_reason 值域表(daemon → ACP StopReason)
                   mode(edit/plan/yolo ↔ SessionMode)
                   permission 选项(daemon risk/decision ↔ PermissionOption 三态)
```

行数预算与调研一致:PR1 ~400-600,PR2 ~300-500,PR3 ~150-300(不含测试)。

## 4. 并发模型与关键流程

- **tokio current_thread runtime**(I/O 密集,无 CPU 并行需求;crate runtime-agnostic,若其内部 spawn 依赖多线程再升 multi_thread,实施时验证)。
- **SSE 任务 = 进程级单例**:main 里健康检查通过后立刻启动(「启动即挂、全程保持」,规避无订阅者快拒 + 在途 ask 无恢复面,缺口 1/§R2);断线自动重连(指数退避 + Last-Event-ID);重连失败不退出进程,持续重试(编辑器 session 可能长存)。
- **session/prompt 流程**(对应 evl chat.mjs 全链):
  1. 保证 SSE 任务活着(启动即挂,无需补挂);
  2. `POST /api/v1/agent/chat {request_id, session_id, messages:[user]}`;受理非 `started`(`queued`/`injected`)→ 返回 JSON-RPC error(取舍 3,拒绝排队);
  3. 订阅 broadcast 流,按 `request_id` 过滤(chat-event)与 `session_id` 过滤(permission:ask / tool 事件),逐事件 translate → `session/update` 通知;
  4. 收到 `done{stop_reason}` → 映射 StopReason,respond prompt 请求,结束过滤;
  5. 收到 `error` 事件 → 映射对应 StopReason(值域表)+ 可选 stderr 日志。
- **权限环**:SSE `permission:ask`(camelCase payload)→ 解码 → `session/request_permission`(options 固定三态:allow_once / allow_always / reject,optionId 用 kind 字符串本身)→ Zed 应答 optionId → `POST /api/v1/permissions/permission_response {rid, decision}`(decision 与 optionKind 同名);`resolved:false` / 客户端拒绝(选 reject 或请求被 cancel)→ daemon 侧自然超时/拒,shim 不重试。
- **session/cancel**(通知,无响应)→ `POST /api/v1/cancel/cancel_chat {request_id}`;rid 未知时 daemon 静默 no-op,shim 同样 no-op。
- **session/load** → `GET /api/v1/sessions/{id}/messages`(load_session 现有端点)→ 逐消息生成 update 序列:text → agent_message_chunk(整块一次);thinking → agent_thought_chunk;tool_use → tool_call(pending)紧跟 tool_call_update(completed,rawInput);tool_result → tool_call_update(completed/failed,rawOutput);重放完再发 usage_update(如有)。
- **反向请求并发**:prompt 进行中同时只有一轮 turn(daemon 串行),permission 反向请求通过 `ConnectionTo` 发出并 await Responder;SSE 任务绝不阻塞在反向请求上(用 spawn 包裹,超时保护 = daemon 侧 ask 超时已兜底)。

## 5. daemon.rs 归一层(payload 命名不对称,缺口 5)

- 内部类型统一 snake_case;两处边界显式转换并写测试锚死:
  - `permission:ask` SSE 事件 data 是 camelCase(`{rid, sessionId, toolUseId, toolName, toolInput, risk, ...}`)→ serde `rename_all = "camelCase"` 的专用 DTO;
  - `chat-event` data 是 snake_case(`{kind, request_id, session_id, ...}`)→ snake_case DTO;
  - HTTP 请求/响应体按各自端点实际 casing 定义(evl 已验证的形状为准)。
- 请求超时:chat 受理 POST 短超时(10s);permission_response 短超时;SSE 流无超时(长连接 + 30s ping 天然保活)。

## 6. 错误模型

- `thiserror` 内部错误枚举:`DaemonUnreachable`(initialize 时 → JSON-RPC error + message 含 `daemon.sh` 启动指引)/ `DaemonApi{status, body}` / `SseClosed` / `Protocol{...}`。
- JSON-RPC 错误响应用 crate `ErrorCode`;queued 受理、daemon 5xx、未知 session 等一律转 error response(prompt 请求必须最终被 respond,不得悬挂——除 cancel 后仍正常 respond cancelled)。
- shim 自身 panic = 进程死,Zed 会显示 agent 崩溃:handler 内 catch 不了的不留侥幸,关键路径(事件循环)对 malformed payload 用 `serde_json::Value` 先探后解,解失败记日志丢帧不 panic(「turn 不死」同款约束)。

## 7. 测试策略(PR4 主战场)

- **纯函数单测**(translate.rs / payload DTO):值域映射表逐行断言(stop_reason 全值域、mode 三态、permission 三态、camelCase/snake_case 归一)。
- **集成测试**(`tests/`,dev-dep tokio process spawn 真子进程):
  - 起 wiremock 模拟 daemon:健康检查、projects/sessions CRUD、agent/chat 受理、**SSE 流式响应**(chunked body 逐帧推 chat-event/tool/permission/done)、permission_response、cancel;
  - 测试侧用同 crate `Builder` 组 ACP client(ChildProcess/ByteStreams transport 连 shim 的 stdin/stdout),断言:update 通知序列与类型、stopReason、权限环(收到 request_permission → 应答 → wiremock 收到 permission_response)、cancel、session/load 重放序列;
  - 用例组:happy path 文本流 / thinking / tool 两态 / permission allow_once / permission reject / cancel / queued 拒绝 / daemon 不可达 initialize 报错。
- **CI 接线**(PR4):ci.yml Rust job 加 `cargo test -p everlasting-acp`(零系统库,无需 env)+ `cargo clippy -p everlasting-acp -- -D warnings` + fmt 已是 workspace 级自动覆盖。
- **Zed 手测清单**(PR4 文档化,AC 第 4 条):settings.json `agent_servers` 注册 `{type:"custom", command:"<path>/everlasting-acp"}` → 建 session → 跑 turn → 权限审批 → cancel → 重载;`dev: open acp logs` 看调试。本环境无 GUI 则清单留用户执行。

## 8. 兼容与回滚

- 纯新增 crate + CI 行 + 文档:回滚 = 删目录;daemon/GUI/CLI 零影响(git diff 不触及 app/src-tauri 即验收项)。
- 版本锚定:协议 v1 + crate 1.x;Zed 若升 v2,crate 2.x 的 `unstable_protocol_v2` 门下再议(不在本任务)。
- 发布面:MVP 不进 Tauri bundle / sidecar(用户手动 build + 注册);打包分发记增强。

## 9. 实施顺序与 PR 切分(与调研 §4.2 一致)

1. **PR1 生命周期**:crate 骨架 + main + daemon.rs(健康检查/project/session API)+ initialize + session/new/load/list + Stdio serve 起来(wiremock 初版)。
2. **PR2 翻译层**:sse.rs 消费任务 + session/prompt 全链 + translate.rs 全量映射 + stopReason 值域。
3. **PR3 交互桥**:permission.rs 反向请求环 + session/cancel + set_mode/current_mode_update。
4. **PR4 收口**:集成测试全套 + CI 接线 + docs(注册指引/前置条件/已知限制)+ BACKLOG N20 行更新。

线性依赖,不并行;每 PR 独立可编译、已有测试绿。
