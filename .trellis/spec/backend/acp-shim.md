# ACP Shim(everlasting-acp crate)

> 2026-09-30(task 09-30-n20-acp-shim-mvp)新增:daemon 的第五客户端形态(ACP agent 侧 shim),Zed 等 ACP 客户端经 stdio JSON-RPC 驱动 everlasting。前置调研:task `09-29-n20-acp-integration-research`(归档)。本 spec 记 crate 的结构性契约与实施时踩实的坑;用户面文档在 `docs/ACP.md`。

## 1. 定位与硬边界

- `crates/everlasting-acp`:workspace member,**不在 default-members**(根裸 `cargo build/test` 不触它;CI 显式 `-p everlasting-acp` 两条:clippy `--tests -D warnings` + test)。
- 瘦翻译层:shim → daemon HTTP/SSE 单向依赖;**不链接 everlasting 主 crate / agent core**(零系统库,无需 PKG_CONFIG_PATH)。
- daemon 侧零改动是验收项:改 daemon 行为去迁就 shim = 走错方向,shim 侧适配。
- stdout 只准 JSON-RPC 帧(ACP 硬约束):全 crate 禁 `println!`/`print!`,tracing writer 钉死 stderr。check 时必须复查此点。

## 2. 版本锚定

- `agent-client-protocol = "1.3"`,**不带任何 `unstable_*` feature**(= 协议 v1 stable);schema 类型实际来自独立 crate `agent-client-protocol-schema 1.4.0`(锁 `=1.4.0`,经 `agent_client_protocol::schema::v1::*` 引用)。
- 2.x 线(session_fork / mcp_over_acp 等)全部 unstable,不跟;Zed 升 v2 时再评估。

## 3. SDK 实际 API 形状(1.3.0,与 docs.rs 摘要的出入)

- 完整**角色驱动 SDK**,不是薄 schema:`Agent.builder().on_receive_request(async |req, responder, cx| ..., on_receive_request!()).connect_to(Stdio::new())`。handler 是 Rust 2024 async closure(`AsyncFnMut`),edition 2021 下可用。
- **未知方法兜底 SDK 内建**:handler 链尾 `Handled::No` 自动回 `-32601`,通知静默忽略——不需要自写 fallback。
- **handler 返回 Err 会关掉整条连接** → daemon 错误一律 `responder.respond_with_result(Err(...))`,handler 闭包恒返回 Ok。
- **handler 不得占事件循环**(死锁约束):任何 await 的长操作(整轮 prompt 消费循环、cancel 的 HTTP POST)经 `cx.spawn` 出循环;responder 随行。实证坑:PR3 曾在通知 handler 直 await `cancel_chat`(最长 10s 超时),停摆全连接消息处理。
- `ConnectionTo::send_request().block_task()`:只准在 spawn 任务内调用(事件循环上调用死锁);反向请求(permission)的环任务 = plain `tokio::spawn`(恒返 `()`,不拖垮连接)。
- `ConnectionTo` cheap clone(全 clone 共享底层连接),spawn 任务持 owned clone 安全;客户端 clean EOF 后在途 `SentRequest` 全部 fail、再 send 立即失败——环任务两条路都 log+return 即可,无挂死。

## 4. 核心映射决策(改动前先读)

- **stop_reason 值域表**(translate.rs,全生产点实读钉死):end_turn/cancelled/max_turns→MaxTurnRequests、loop_terminated→MaxTurnRequests(系统停机闸归次数上限族,不归 Refusal)、max_tokens、tool_use·stop_sequence→EndTurn、refusal、content_filter→Refusal(openai other 臂透传值)、None→EndTurn、error 事件→Refusal(daemon 不补 done,error 即终态)、未知值→EndTurn+warn。
- **权限桥 rename 点**:ACP 侧 optionId = `allow_once/allow_always/reject_once`(schema 无裸 reject;还有 reject_always 档未用)→ daemon decision `allow_once/allow_always/deny`——**reject→deny 是唯一 rename**,未知 optionId → deny(安全默认);`option_id_to_decision` 单函数承担,测试锁死。
- **一次 turn 可多 ask 在途**(worker 子代理与主 loop 并发,PermissionStore 按 rid 多条目):权限环必须逐 ask spawn,**单槽方案会丢 worker ask**——这是正确性约束不是优化。
- **session/load 重放**:官方时序 = 全部 update 通知发完才 respond(user 行也重放为 user_message_chunk,官方 "entire conversation" 语义);cwd 不一致 strict 拒绝(invalid_params,历史路径与新工作区错位);tool_use→pending+completed 两帧依赖 daemon pair atomicity(llm-contract/gotcha-tool-result-pair-atomicity.md,启动恢复 pass 兜孤儿)。
- **queued/injected 受理一律拒绝**(JSON-RPC error):不做排队重放(Zed 单人场景,ACP prompt 本身串行)。

## 5. SSE 客户端模式(与 evl 共享,详见 daemon-server/pattern-external-sse-client.md)

启动即挂全程保持 / Last-Event-ID 重连 / 健康窗口判据(45s > 30s ping)/ Resync 非 buffer_overrun → prompt respond error / 失联自愈窗。shim 特有:`UpdateSink` / `PermissionOutbound` 两个 trait 接缝让消费循环与权限环脱离 ACP 传输可单测(测试用 VecSink / ScriptedOutbound)。

## 6. 测试模式

- 单测:wiremock(静态 body 够用——SSE 解析层按 `\n\n` 切帧,一次性下发无差);payload casing 用 `body_json` 精确匹配锚死(casing 漂移 = 404 = 红测)。
- 集成(tests/integration.rs,11 例):**手写 axum 状态化假 daemon**——wiremock 做不到「按测试时序推 SSE 帧 + 捕获请求体取动态 rid」(权限环/cancel 的因果排序);spawn 真子进程 `CARGO_BIN_EXE_everlasting-acp` + SDK Client role(`AcpAgent` transport);每例 60s 兜底超时(悬挂直接红)。
- live 冒烟:真 daemon 下触发权限 ask 的命令要选**需网络的命令**(如 curl);纯读命令(ls/rm)被沙箱静默放行(audit `tool_allowed reason:null`)根本不产生 ask——这是 Zed 手测清单里的实测坑。

## 7. 已知限制与 follow-up(改动边界)

tool 无流式中间输出(两态跳变,协议合法)/ text-only(图片拒绝)/ MCP over ACP 不声明不中转 / daemon 不自动拉起(健康检查失败 → initialize 报错含 `daemon.sh start` 指引)/ 在途 permission ask 无恢复面(依赖「启动即挂」规避;daemon 侧 Permission 入 pending_interaction 是结构解,follow-up)。follow-up 全录:docs/ACP.md §5 + docs/BACKLOG.md N20 行。
