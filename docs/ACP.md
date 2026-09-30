# ACP 接入:`everlasting-acp` shim(Zed 可用)

> `everlasting-acp` 是 ACP(Agent Client Protocol,agentclientprotocol.com)的
> **agent 侧** shim 二进制:被编辑器(Zed 等 ACP 客户端)spawn 为子进程,
> stdin/stdout 按行 JSON-RPC(ACP v1 stable),把 everlasting daemon 的
> HTTP/SSE 面翻译给客户端 —— **第五客户端形态**(与 Tauri GUI / 浏览器 SPA /
> 远程 PWA / `evl` CLI 并列),daemon 与 agent core **零改动**。
>
> 依据:task `09-30-n20-acp-shim-mvp`(调研 → PR1-4);crate 落位
> `crates/everlasting-acp/`,协议 crate `agent-client-protocol` 锚 1.x
> (v1 stable,不带任何 `unstable_*` feature)。

## 1. 前置条件

1. **daemon 必须在跑**(shim 不自动拉起;连接失败会在 initialize 报结构化
   错误,信息含启动指引,进程不退出 —— daemon 稍后起好即可重试):

   ```bash
   ./scripts/daemon.sh start        # 或 bg 后台运行;默认 :7456
   ```

2. **编译 shim**(workspace 成员,不在 default-members,须显式 `-p`;
   零系统库,无需 PKG_CONFIG_PATH):

   ```bash
   cargo build -p everlasting-acp
   # 产物:target/debug/everlasting-acp(release 档在 target/release/)
   ```

3. **环境变量**(可选):
   - `EVERLASTING_ACP_DAEMON_URL` — daemon 地址,Zed `agent_servers.env`
     里配;默认 `http://127.0.0.1:7456`。
   - `DAEMON_URL` — 兼容 evl 的次选(前者优先)。
   - `RUST_LOG` — 日志级别(默认 `info`)。**日志恒走 stderr** —— ACP 规定
     stdout 只准 JSON-RPC 协议帧,这是硬约束。

## 2. Zed 注册

`settings.json`(`zed: open settings`):

```json
{
  "agent_servers": {
    "Everlasting": {
      "type": "custom",
      "command": "/absolute/path/to/everlasting/target/debug/everlasting-acp",
      "env": {
        "EVERLASTING_ACP_DAEMON_URL": "http://127.0.0.1:7456"
      }
    }
  }
}
```

- `command` 用**绝对路径**(上面 `cargo build` 产物;发布档建议
  `target/release/everlasting-acp` + `--profile daemon` 日常迭代产物在
  `target/daemon/` 时自行取对应路径)。
- Agent Panel 新建线程菜单出现 "Everlasting" 即注册成功。
- 调试通道:Zed 里 `dev: open acp logs` 看 shim 的 stdout/stderr 帧与日志;
  `RUST_LOG=debug` 可加详。

## 3. 能力面(协商结果)

`initialize` 声明(全是协议内合法降级,不是缺陷):

| 能力 | 值 | 说明 |
|---|---|---|
| `loadSession` | true | 重开线程走 `session/load`,历史以 update 通知重放(先发完全部 update 再 respond,官方时序) |
| `promptCapabilities` | 全 false | text-only:image / audio / embedded-context 的 prompt 内容块会被 `invalid_params` 拒绝 |
| fs / terminal / elicitation | 不声明 | agent 自持工具(read_file/shell 等)承担,零反向调用 |
| `mcpCapabilities` | 不声明 | 不中转 MCP over ACP |
| `sessionCapabilities.list` | 声明 | `session/list` 直映 `list_sessions`(同 project 过滤;cursor 不分页) |
| `authMethods` | `[]` | 本机零鉴权;`authenticate`/`logout` 恒成功空实现 |

## 4. 会话与 mode

- `session/new {cwd}` → cwd 词规整(不解析符号链接)比对既有 project →
  缺则 `create_project` → `create_session` → **显式锚定 edit 档** → 响应带
  三档 modes(edit / plan / yolo)。worktree 恒走 daemon 默认 none 态 ——
  工具 cwd 直落 project.path,即 Zed 打开的工作区目录。
- `session/set_mode` → daemon `set_session_mode`(未知值 shim 先归一 edit,
  与 daemon lenient 行为对齐)→ 成功后发 `current_mode_update` 通知。
- `session/load` → **cwd 必须与 session 所在目录一致**(不一致报
  `invalid_params`:重放的历史 tool_call 路径与新工作区错位)→ 历史消息按
  user/assistant 文本、thinking、tool_call(两态)重放为 update 通知,
  **全部发完后**才响应(官方时序)。
- `session/cancel`(通知)→ `cancel_chat`(按在途 rid);无在途 turn 静默
  no-op;cancel 后当轮 prompt 以 `stopReason: cancelled` 收束。

## 5. 已知限制与 UI 语义

| 限制 | 表现 | 依据 |
|---|---|---|
| tool 无流式中间输出 | tool_call(pending)→ tool_call_update(completed/failed)两态跳变,长命令在 Zed 里无增量输出(协议合法,daemon 侧 tool 事件只有终态) | 调研报告缺口 2 |
| queued 拒绝 | daemon busy 时新 prompt 直接 JSON-RPC error(注明 busy 与队位),不排队 —— 单人编辑器场景等当前 turn 结束重试即可 | 取舍 3 |
| daemon 不自动拉起 | daemon 未起时 initialize 报错(含 `daemon.sh start` 指引);daemon 中途宕机,在途 turn 以 error 收束(失联自愈窗 30s + 健康窗 45s,`stream-resync reason=restart` 同样收束) | 缺口 4 / R2 |
| error → Refusal | daemon turn 出错(`kind:error` 事件)时 prompt 以 `stopReason: refusal` 收束 —— Zed 会按"agent 拒答"渲染,实际可能是网络/限流等基础设施错误,看 shim stderr(RUST_LOG=info)定位 | 值域表 translate.rs |
| 权限 ask 无恢复面 | shim 与 daemon 间断线期间到达的 ask 会丢(daemon 对零订阅者快拒;shim 全程保持 SSE 连接规避,但 shim 自身重启瞬间的在途 ask 无快照) | 缺口 1,follow-up |
| `ask_no_timeout` 开关 | daemon 侧开启后权限 ask 无 120s 超时:客户端不应答时 ask 永挂,该 turn 不推进(cancel 可解) | permission-layer §7 |
| 群聊不达 | `session/new` 只建经典会话;`ChatAcceptance::injected`(群聊注入)路径被拒绝 | 取舍 3 |

## 6. 权限审批(核心桥)

daemon 的 `permission:ask` → ACP `session/request_permission` 反向请求,
三个固定选项(optionId = daemon decision 值):

| optionId | Zed 按钮 | 回给 daemon 的 decision |
|---|---|---|
| `allow_once` | 允许一次 | `allow_once` |
| `allow_always` | 始终允许 | `allow_always` |
| `reject_once` | 拒绝 | **`deny`**(rename 点) |

- 审批与流式**并行**:ask 在途时 delta/tool 流继续转发,turn 终态不等审批。
- 客户端以 `cancelled` outcome 应答(turn 取消)或连接死亡 → 不回
  `permission_response`,daemon 侧 120s ask 超时兜底(`ask_no_timeout`
  开启时永挂,见上表)。
- cancel turn 后的迟到 optionId 应答照发 —— rid 已失效返回
  `resolved:false`,自然收敛。

## 7. Zed 手测清单(AC 项,需 GUI 环境)

1. daemon 起跑 + shim 注册(§1-2);Agent Panel 出现 "Everlasting"。
2. 新建线程 → 随便问一句 → 文本/thinking 流式可见,`end_turn` 收束。
3. 让它跑一个**需要审批**的命令(如 `curl`——纯读命令会被沙箱层静默放行
   不弹卡):Zed 弹权限卡 → 允许一次 → 命令执行、结果可见。
4. turn 进行中点 Stop(cancel)→ 该轮以 cancelled 收束,session 保留。
5. 关掉线程重开(loadSession)→ 历史按原序重放(用户气泡/回复/thinking/
   tool 卡)。
6. `dev: open acp logs` 检查无 stdout 污染(协议帧之外零输出)。

## 8. 测试与 CI

- 单测(crate 内,60):值域表(stop_reason 全表 / mode / tool_kind)、
  casing 归一锚、SSE 解析、prompt 消费状态机、权限环、cancel/set_mode。
- 集成(`tests/integration.rs`,11):spawn 真子进程 + axum 假 daemon
  (真流式 SSE)+ SDK Client role 全链:文本/thinking/tool 流、权限环
  allow_once/reject→deny、cancel 全链、queued 拒绝、daemon 不可达、
  load 重放时序、set_mode、list 归一。
- CI:Rust job 追加 `cargo clippy -p everlasting-acp --tests -- -D warnings`
  与 `cargo test -p everlasting-acp`(零系统库,无需 PKG_CONFIG_PATH)。
