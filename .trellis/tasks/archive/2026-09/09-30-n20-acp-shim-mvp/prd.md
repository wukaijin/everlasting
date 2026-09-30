# N20 ACP shim MVP:everlasting-acp bin(Zed 可用)

## Goal

新增 `everlasting-acp` shim 二进制(ACP agent 侧实现,stdio JSON-RPC),把 everlasting daemon 暴露给 ACP 客户端(参考客户端 Zed)——Zed 注册后可直接用 everlasting 当 agent 后端:建 session、跑 turn(文本/thinking/tool 流可见)、权限审批经编辑器 UI、可取消、可重载历史。架构 = 第五客户端形态的瘦翻译层,连回已运行的 daemon HTTP/SSE,**agent core 与 daemon 侧 MVP 零改动**。

前置调研:task `09-29-n20-acp-integration-research`(已归档),报告已复制到本任务 `research/acp-integration-analysis.md`(三段式结论:缺口 5 项 / 改动面 4 PR / 取舍 8 项,ACP ↔ daemon 全量映射表在 §3)。

## Requirements

### R1 协议面(v1 stable,crate `agent-client-protocol` 锚 1.x)

- `initialize` 握手:声明 `loadSession=true`;`promptCapabilities` 全 false(基线 text-only);fs / terminal / mcp / elicitation 能力一律不声明(合法降级,零实现);`authMethods=[]`(本机零鉴权)。
- agent 侧方法:`session/new`(cwd → project path 解析 → daemon session)、`session/prompt`(SSE 先挂 → POST chat → 按 request_id 过滤消费)、`session/cancel`(→ cancel_chat)、`session/load`(load_session → messages 生成 update 重放)、`session/set_mode`(→ set_session_mode edit/plan/yolo + `current_mode_update`)、`session/list` / `session/new` 配套的关闭语义(`session/close` 不声明能力则不实现,见 design 定夺)。
- client 侧反向调用:`session/request_permission`(permission:ask → 反向请求;options = allow_once / allow_always / reject;应答回 `permission_response`)。
- `session/update` 变体落地:`agent_message_chunk`(delta)/ `agent_thought_chunk`(thinking_delta)/ `tool_call`(pending)/ `tool_call_update`(tool:result → completed/failed 两态跳变,合法)/ `current_mode_update` / `usage_update`(turn_usage);`user_message_chunk` 可选 echo。不发的变体:`plan` / `available_commands_update` / `config_option_update` / `session_info_update`(无源,记增强)。
- prompt 响应 `stopReason`:SSE `done{stop_reason}` 值域映射(cancelled→cancelled、正常 end→end_turn、error→refusal 等,实施时对照两值域表逐一钉死并写测试)。

### R2 生命周期与部署

- shim 是被编辑器 spawn 的子进程:stdin/stdout 按行 JSON-RPC,stderr 留日志。
- daemon 前置:MVP 不自动拉起——启动时健康检查失败即在 `initialize` 报错,错误信息给出 `daemon.sh` 启动指引;自动 spawn 记增强(取舍 4/缺口 4)。
- SSE 订阅「启动即挂、全程保持」(evl 同款):规避无订阅者快拒 + 在途 permission ask 无恢复面的结构缺口(缺口 1,MVP 规避;daemon 侧恢复面记 follow-up 不在本任务)。

### R3 结构约束

- 落位:workspace 新成员 `crates/everlasting-acp`(轻依赖:agent-client-protocol 1.x + reqwest + tokio + serde 系;不依赖 everlasting 主 crate / agent core)。
- queued 受理(`ChatAcceptance::queued`):拒绝并返回错误,不做排队(取舍 3)。
- payload 命名不对称(permission:ask camelCase vs chat-event snake_case)在 shim 内部归一层统一,测试锚钉死(缺口 5)。
- worktree 恒 none(直连 Zed 工作区目录);MCP over ACP 不声明不中转;图片 text-only——均为协议内合法降级,不是缺陷。

### R4 测试与文档

- 测试:ACP 客户端 mock(spawn shim 子进程对驱动),断言 update 序列、stopReason 映射、权限环(ask→反向请求→应答→resolved)、cancel、session/load 重放;纯函数单测(值域映射、payload 归一)。
- 文档:Zed `agent_servers` 注册指引、daemon 前置条件、能力降级说明与已知限制(tool 无流式中间输出等)。

## Out of Scope(follow-up / 增强,不阻塞验收)

- 在途 permission ask 恢复面(`pending_interaction` 增 Permission 变体,~20-40 行 daemon 侧改动)。
- tool 流式中间输出;`plan` 事件;图片(image=true + ContentBlock 转换);MCP over ACP;daemon 自动拉起;fs/terminal/elicitation 反向调用;远程暴露(须安全评审)。

## Acceptance Criteria

- [x] `crates/everlasting-acp` 加入 workspace(default-members 不含,不拖累根裸构建),`cargo build -p everlasting-acp` 通过。
- [x] PR1-4 按 implement.md 顺序落地,每 PR 独立可编译、测试绿。
- [x] ACP mock 客户端集成测试全绿:update 序列 / stopReason 映射 / 权限环 / cancel / session/load 重放 / payload 命名归一。
- [x] Zed 实测清单走通:注册 `agent_servers` → Agent Panel 建 session → 跑一轮 turn(文本/thinking 流式可见、tool_call 两态可见)→ 权限审批经 Zed UI 应答生效 → cancel 生效 → 重开线程重载历史。
  - 本环境无 GUI,按 Notes 预案记录为 manual:六步清单落在 docs/ACP.md §7 留用户执行(注册片段/前置条件/调试通道齐备);daemon 沙箱 live 冒烟已做(实测坑记 BACKLOG N20 行:纯读命令静默放行,需网络类命令才触发 ask)。
- [x] daemon 与 agent core 代码零改动(git diff 不触及 app/src-tauri/src)。
- [x] 文档落地(注册指引 + 前置条件 + 已知限制);BACKLOG N20 行状态更新为已交付。

> 2026-09-30 final pass 全量验证闭合(cargo test -p everlasting-acp 60 单测 + 11 集成全绿 / clippy -D warnings / fmt / cargo test -p everlasting-remote 89 绿 / 前端 2096 绿 / git status app/src-tauri 空 / cargo metadata 实证 default-members 不含本 crate)。

## Notes

- 对照量级:N2 checkpoint 同级(4 PR)。实施顺序:PR1 生命周期 → PR2 翻译层 → PR3 交互桥 → PR4 测试收口 + Zed 实测(线性依赖,不并行)。
- Zed 实测(AC 第 4 条)需要 GUI 环境,若本环境不可行则记录 manual 步骤清单留用户执行,其余 AC 全自动验证。
