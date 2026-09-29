# N19 实施:mcode 语义吸收三件收口(daemon shutdown kill_all/SIGTERM 宽限两段式/取消配对差集补齐)+CJK 测试锚

## Goal

按调研结论([n19-semantics-gap-analysis](../archive/2026-09/09-29-n19-mcode-semantics-research/research/n19-semantics-gap-analysis.md))实施 N19 收窄后的三件 + 一条测试锚,单 PR 交付:

1. **件④缺口 A(主体)**:daemon graceful shutdown 链补 `background_shells.kill_all()`,修 Thin/sidecar/daemon.sh 优雅退出孤儿化后台 shell 的实锤缺口;
2. **件③**:`kill_and_collect` 两段式(SIGTERM → 宽限 → SIGKILL),让脚本 trap 清理逻辑有机会执行;
3. **件⑤**:`finalize_turn` 取消臂补齐未执行 tool_use 的 synthetic tool_result(对齐 drive.rs send 阶段取消的落库语义);
4. **件①(测试锚)**:edit_file CJK 全角字符往返保真单测。

## Requirements

### R1 daemon shutdown 补 kill_all(件④A)

- `shutdown_signal`(daemon/server.rs)在 `cancel_and_drain_all_agent_loops` **之后**加 `state.background_shells.kill_all().await`(错误 log-only,与 GUI Exit hook 同款);
- 顺序理由:drain 让 in-flight 的 run_background_shell **启动**完成(启动后 registry 有 entry),再 kill_all 才收得到刚启动的;
- kill_all 走 **Immediate(SIGKILL 直杀)** 档,不吃宽限——daemon.sh 的 SIGTERM→SIGKILL 窗口是 15s(drain 8s + axum grace 3s 已用 11s),批量收尸优先确定性,GUI 退出也不拖宽限;
- spec 回填 `daemon-server/pattern-shutdown-checklist.md` 顺序图 + 不变量 + 测试要求。

### R2 kill_and_collect 两段式(件③)

- 前台(tools/shell.rs)/后台(background_shell/in_memory.rs)两处同构改造:先 `kill(-pid, SIGTERM)`,`tokio::time::timeout(grace, child.wait())`,超时再 `kill(-pid, SIGKILL)` + wait;
- **宽限档位**:单杀路径(shell_kill tool / 前台取消 / 前后台超时)= 3s;批量路径(kill_all_for_session / kill_all)= 0(直 SIGKILL,等价现状)——后台 registry 的 kill 通道(`kill_tx: oneshot::Sender<()>`)需改为携带 grace 值(如 `Sender<u64>`);前台 `kill_and_collect` 增加 grace 参数;
- ESRCH 既有容错语义保持;`ShellExitTrigger`/`BackgroundShellOutcome` 语义不变(killed 就是 killed,不区分是否经过宽限);
- RULE-E-002「必死」不变量保持:最坏情形 = 现状 + 3s;
- spec 回填 `tool-contract/06-background-shell.md`(RULE-E-002 语义升级注记:SIGKILL → 先礼后兵)。

### R3 取消配对差集补齐(件⑤)

- `FinalizeFrame` 增加 tool_calls 信息(id+name);`finalize_turn` 取消臂在落库前对 `tool_calls` 中没有 result_block 的 id 追加 synthetic is_error ToolResult block(文案复用 helpers 语义),与部分真 result 同一条 user 消息落库;
- 只补 serial 路径的取消缺口;L2 并行路径 slot 结构天然全配对(差集为空时零行为变化,幂等);
- wire 层 `chat_request_to_wire` 自愈**不动**(降级为真异常兜底);
- dispatch 并发批(subagent)不在范围(其取消走 `status=cancelled` dispatch tool_result 既有契约);
- 测试:N tool_use 中途取消 → 落库消息 tool_result 数 == tool_use 数(部分真 + 差集 synthetic)。

### R4 CJK 测试锚(件①)

- edit_file 单测:文件含全角字符/智能引号/全角空格,edit 邻近片段后断言未触及的全角字符逐字节保留;
- 不引入任何 fuzzy/归一化实现(精确匹配是现状优点,不是缺口)。

## Acceptance Criteria

- [ ] AC1(R1):SIGTERM 集成测试——植入活跃后台 shell → 发真实 SIGTERM → 断言 shell 被 kill(复用 `SIGNAL_TEST_MUTEX` 串行样板);`pattern-shutdown-checklist.md` 顺序图含 kill_all 步
- [ ] AC2(R2):单测覆盖——trap SIGTERM 脚本宽限内退出(killed)、无 trap 脚本宽限满强杀(killed)、批量 kill_all 零宽限;前台/后台两处实现均测
- [ ] AC3(R2):`cargo test --lib` 相关模块(shell_kill/shell/background_shell/daemon)全绿
- [ ] AC4(R3):单测——serial 批量 tool_calls 中途取消,finalize 落库消息 tool_result 数 == tool_use 数,差集 block 为 synthetic is_error;无取消路径零行为变化
- [ ] AC5(R4):CJK 往返保真单测绿
- [ ] AC6:`cargo test -p everlasting --lib` 全绿(PKG_CONFIG_PATH 按 AGENTS.md);纯后端,无前端改动
- [ ] AC7:spec 三处回填(pattern-shutdown-checklist / 06-background-shell / 取消语义段——落点 implement 时按实际改动定,候选 `agent-loop-architecture/pattern-cancellation-guard.md`)

## Notes

- 调研依据:[n19-semantics-gap-analysis.md](../archive/2026-09/09-29-n19-mcode-semantics-research/research/n19-semantics-gap-analysis.md)(行号证据基于调研日 main `427692f1`,实施时以当前代码为准)
- 件②(tracked patch 无 per-file 上限)与件④缺口 B(崩溃面守卫)明确不在本任务范围(BACKLOG C.3 已记注)
