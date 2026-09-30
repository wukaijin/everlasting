# Implement:everlasting-acp shim MVP

> 顺序 = design.md §9;每 PR 一个 commit 批次,独立可编译、测试绿再进下一个。全程不触碰 `app/src-tauri/`(daemon 零改动硬约束,AC 项)。

## PR1 生命周期(crate 骨架 + initialize + session 管理)

- [x] `crates/everlasting-acp/Cargo.toml`:bin target `everlasting-acp`,依赖按 design §2(agent-client-protocol = "1.3" 无 unstable feature、tokio、reqwest rustls+stream、serde/serde_json、thiserror、tracing 系);dev-dep wiremock + tokio process。
- [x] 根 `Cargo.toml`:`members` 加 `crates/everlasting-acp`(**不加 default-members**),补注释说明。
- [x] `src/main.rs`:env 解析(`EVERLASTING_ACP_DAEMON_URL` 或 `DAEMON_URL`,默认 `http://127.0.0.1:7456`;`RUST_LOG`)、tracing→stderr、daemon 健康检查(失败 → initialize 报错含 `daemon.sh` 指引,不 panic 退出——stderr 打指引后仍进入 serve 以便客户端看到结构化错误)。
- [x] `src/daemon.rs`:HTTP 客户端封装 + payload DTO(camelCase/snake_case 归一层初版:health、list_projects、create_project、create_session、list_sessions、load_session、set_session_mode;casing 按 design §5 + evl 验证形状)。
- [x] `src/handlers/`:`initialize`(loadSession=true、promptCapabilities 全 false、fs/terminal/mcp 不声明、authMethods=[])、`authenticate`/`logout` 空实现、`session/new`(cwd → project path 词规整比对 → 建缺失 project → create_session → set default mode)、`session/load`、`session/list`;session 表(mod.rs)。
- [x] cargo fmt 过;`cargo clippy -p everlasting-acp -- -D warnings` 过;wiremock 单测覆盖:健康检查失败路径、project 解析命中/未命中、session/new 全链。
- [x] 验证:`cargo build -p everlasting-acp` && `cargo test -p everlasting-acp`;根裸 `cargo build` 不构建本 crate(default-members 断言)。

## PR2 翻译层(SSE 消费 + prompt 全链 + 映射表)

- [x] `src/sse.rs`:GET `/api/v1/stream` 消费任务(进程级单例,main 启动即挂)、行解析(event:/id:/data:)、Last-Event-ID 重连(指数退避,重连失败不退进程)、`DaemonEvent` 归一 + broadcast channel;ping/`stream-resync` sentinel 处理。
- [x] `src/translate.rs`:ChatEvent kind → update 变体(delta→agent_message_chunk、thinking_delta→agent_thought_chunk、turn_usage→usage_update);tool:call→tool_call(pending)、tool:result→tool_call_update(completed/failed,rawInput/rawOutput 填充);stop_reason 值域表(daemon↔ACP,含 error→refusal 等,逐行注释依据);mode 映射。
- [x] `handlers/session.rs` 补 `session/prompt`:订阅 broadcast → POST agent_chat(非 `started` 受理 → JSON-RPC error)→ 按 request_id/session_id 过滤 → 逐事件 translate → `session/update` 通知 → done/error → respond(StopReason)。
- [x] 单测:translate 值域全表断言、SSE 行解析(含半行/多 data 行/断线重连状态)、prompt 状态机(用 wiremock SSE 流式响应驱动;permission:ask 事件此阶段先记日志跳过,PR3 接管)。
- [x] 验证:同 PR1 命令;`scripts/turn-smoke.sh` 思路的手工对照可选(daemon 真跑 + 手动 spawn shim 发 initialize/session/new/prompt,观察 stdout 帧序列)。

## PR3 交互桥(权限环 + cancel + mode)

- [x] `src/permission.rs`:permission:ask(SSE,camelCase DTO)→ `session/request_permission` 反向请求(options:allow_once/allow_always/reject,optionId=kind 字符串)→ 应答 → `POST permission_response {rid, decision}`;reject / 反向请求被客户端取消 → 不重试,daemon 侧超时兜底;spawn 包裹 + 不阻塞 SSE 任务。
- [x] `session/cancel` 通知 → cancel_chat(rid 未知 no-op);cancel 后 done(cancelled) → prompt respond StopReason::Cancelled 路径验证。
- [x] `session/set_mode` → set_session_mode + `current_mode_update` 通知(edit/plan/yolo ↔ SessionMode;未知值回退 edit 与 daemon 行为对齐)。
- [x] 单测:权限环(mock 客户端应答三种选项各一 + wiremock 断言 permission_response 落点)、cancel 全链、set_mode 往返。
- [x] 验证:同上 + 真 daemon 手测一轮权限工具(如 Bash ask)。

## PR4 收口(session/load 重放 + 集成测试 + CI + 文档)

- [x] `session/load` 重放实现:messages → update 序列(文本/thinking 整块 chunk、tool_use→pending+completed、tool_result→completed/failed,见 design §4)。
- [x] `tests/integration.rs`:spawn 真子进程 + wiremock daemon + ACP client(同 crate Builder)全链用例:happy path 文本流 / thinking / tool 两态 / permission allow_once / permission reject / cancel / queued 拒绝 / daemon 不可达 initialize 报错 / load 重放。
- [x] `.github/workflows/ci.yml` Rust job:`cargo clippy -p everlasting-acp -- -D warnings` + `cargo test -p everlasting-acp`(零系统库,无需 PKG_CONFIG_PATH;fmt 已 workspace 级)。
- [x] `docs/ACP.md`:Zed `agent_servers` 注册指引(含 command 路径与 env)、daemon 前置条件、能力降级面与已知限制(tool 无流式中间输出、queued 拒绝、daemon 不自动拉起、text-only)、Zed 手测清单、调试通道(`dev: open acp logs`、RUST_LOG)。
- [x] `docs/BACKLOG.md`:N20 行 → 已交付(MVP 范围 + follow-up 清单指针);若 AGENTS.md 或 README 有客户端形态清单,同步第五形态一句。
- [x] 全量验证:`cargo test -p everlasting-acp` + `cargo test -p everlasting-remote`(确认 workspace 无回归)+ `cd app && pnpm test`(前端不受影响,快速过)+ fmt/clippy 全绿。

## Review gates(每 PR 末)

- dispatch `trellis-check` 子代理(spec 合规 + prd/design 对照 + lint/test)。
- PR2 后人工抽查一次 stdout 帧序列(对照 ACP schema 字段名,防手滑拼错 key——schema 严格客户端会整帧丢弃)。

## 回滚点

- 每 PR = 一个 commit 批次;PR2 起若 SSE 消费模型推翻,回滚到 PR1 重设计 sse.rs(design §4 备选:per-prompt 直接持有流,弃全局 broadcast)。
- 全量回滚 = 删 `crates/everlasting-acp/` + 根 Cargo.toml members 行 + CI 两行 + docs/ACP.md;无数据迁移、无 daemon 面。

## 已知风险

- crate 1.3.0 API 与 docs.rs 摘要有出入(Role/ConnectionTo 抽象实际形状)→ PR1 第一步先读 `~/.cargo/registry` 内 crate 源码 + `cargo doc`,若 dispatch 抽象过重则降级:只用 `schema::v1` 类型 + 手写 JSON-RPC 行循环(schema 类型仍省 80% 协议面)。
- wiremock SSE 流式支持不确定 → 备选手写最小 axum mock(#[cfg(test)] 内),或 hyper 直接响应;PR1 时定。
- Zed 手测(AC 第 4 条)本环境无 GUI → 清单化留用户执行,其余 AC 自动验证闭合。
