# Design:N19 三件收口 + CJK 测试锚

> 依据:[n19-semantics-gap-analysis](../archive/2026-09/09-29-n19-mcode-semantics-research/research/n19-semantics-gap-analysis.md)。本文只讲边界/契约/取舍;行号为调研日 `427692f1` 基准,实施以当前代码为准。

## D1 件④A:daemon shutdown 链补 kill_all

### 改动点

`daemon/server.rs::shutdown_signal`(现 :482-536)步骤 2(agent loop drain)之后追加步骤 2.6:

```rust
// 步骤 2.6(N19, 2026-09-29):清杀后台 shell。
// 必须排在 drain 之后 —— in-flight 的 run_background_shell
// 启动调用在 drain 内完成,启动后 registry 才有 entry 可杀。
// kill_all 走 Immediate 档(不吃宽限):daemon.sh 的 SIGTERM→SIGKILL
// 窗口 15s,drain 8s + axum grace 3s 已占 11s,批量收尸优先确定性
// (与 GUI Full 模式 RunEvent::Exit 的 kill_all 语义一致)。
if let Err(e) = state.background_shells.kill_all().await {
    tracing::warn!(error = %e, "shutdown: background_shells.kill_all failed (non-fatal)");
}
```

### 契约与不变量

- **顺序不变量**:sse.shutdown → tunnel stop → scheduler cancel → disk governor cancel → agent loop drain → **background shell kill_all** → axum drain → exit。kill_all 在 drain 后、进程 exit 前;axum grace(3s)为 kill_all 的天然时间兜底(kill_all Immediate 档毫秒级完成,不受影响)。
- **错误语义**:log-only(non-fatal)——与 GUI Exit hook(lib.rs:584-587)同款;单个 shell 杀失败不阻塞其余清杀与进程退出(kill_all 内部逐 entry,已如此)。
- **模式覆盖矩阵**:Thin/sidecar(GUI 死→daemon SIGTERM→本链)✅、daemon.sh stop ✅、Ctrl+C ✅、standalone cargo run ✅。GUI Full 模式不走本链(其 kill_all 在 RunEvent::Exit,已有)。崩溃面(SIGKILL/panic)不在本件(缺口 B,C.3 记注)。

### 测试

`daemon::server::tests` 加一条 SIGTERM 集成测试(与既有两个 SIGTERM 测试共享 `SIGNAL_TEST_MUTEX` 串行):

- 起 serve_daemon(TCP 真实监听)→ 经 registry start 一个 `sleep 60` 后台 shell(等 Running)→ `kill(getpid(), SIGTERM)` → 断言 `serve_daemon` 在 grace 内返回且 shell 状态为 Killed;
- 超时兜底断言同既有测试样板(drain timeout + grace 内返回)。

## D2 件③:kill_and_collect 两段式(SIGTERM → 宽限 → SIGKILL)

### 语义与档位

`kill_and_collect(child, grace_ms: u64)`:

- `grace_ms > 0`:先 `libc::kill(-pid, SIGTERM)` → `tokio::time::timeout(grace, child.wait())`:
  - 宽限内退出 → 完成(trap 清理逻辑已执行);
  - 超时 → `libc::kill(-pid, SIGKILL)` + `child.wait()` 兜底;
- `grace_ms == 0`:直 SIGKILL(等价现状,批量路径)。

| 触发路径 | 档位 | 理由 |
|---------|------|------|
| 前台 shell 取消臂 / 超时臂(shell.rs) | 3000 | 用户单命令,trap 清理值得等 |
| 后台超时臂(in_memory.rs sleep 触发) | 3000 | 同上 |
| registry.kill(shell_kill tool) | 3000 | LLM 单杀,最常见场景 |
| kill_all_for_session(删 session) | 0 | 批量确定性 |
| kill_all(shutdown / GUI exit) | 0 | 时长预算(见 D1)+ GUI 退出不拖 3s |

常量 `SHELL_KILL_GRACE_MS: u64 = 3_000`(env `EVERLASTING_SHELL_KILL_GRACE_MS` 覆盖,测试用)。

### 后台 kill 通道改造

`kill_tx: oneshot::Sender<()>` → `oneshot::Sender<u64>`(载荷 = grace_ms):

- `registry.kill()`(trait 面)发 `SHELL_KILL_GRACE_MS`;
- `kill_all_for_session()` / `kill_all()` 发 `0`;
- `run_background_task` 的 `kill_rx` 臂把值传入 `kill_and_collect(&mut child, grace)`;超时臂直接传 `SHELL_KILL_GRACE_MS`。
- drop-semantics 保持:sender 被 drop(非显式 send)时 `kill_rx` 返回 Err——现有代码把 Err 也当 kill 信号(检查 `kill_rx` 分支现状,若 `_ = &mut kill_rx` 把 Err 一并吞掉则行为不变;若区分则保持原语义,Err 路径用 `SHELL_KILL_GRACE_MS`)。

### 前台改造

`tools/shell.rs::kill_and_collect`(现 :167)加 `grace_ms` 参数;取消臂/超时臂传 `SHELL_KILL_GRACE_MS`。签名 `pub(crate)`,调用点仅 shell.rs 内部(in_memory.rs 是独立同构实现,不共享——两处签名各自演进,与现状一致)。

### 不变量(写给 check)

- RULE-E-002「进程组必死」保持:两段式只是把死亡时刻推迟至多 grace_ms;
- ESRCH 容错保持(SIGTERM/SIGKILL 两段均非 ESRCH 才 warn);
- `ShellExitTrigger::Killed`/`BackgroundShellOutcome::Killed` 不区分是否经过宽限(审计面无新维度,判定就是"被杀");
- Windows 路径(`child.kill()`)不变,grace 参数忽略(注释说明)。

### 测试

- trap 宽限路径:`sh -c 'trap "echo cleanup; exit 0" TERM; sleep 60'` → kill(grace=2000) → Killed,耗时 < 宽限满(证明 SIGTERM 生效未到 SIGKILL);
- 无 trap 强杀:`sh -c 'sleep 60'` 不 trap TERM(sh 默认行为是 TERM 即死,故本例宽限内退出)——强杀路径用 `sh -c 'trap "" TERM; sleep 60'`(trap 空串=忽略 TERM)→ 宽限满 SIGKILL,Killed;
- grace=0 直杀等价现状;
- 时序断言用宽限期余量判定(如 grace=1500 时 trap 路径 <1.5s 内完成、ignore 路径 ≥1.5s),CI 慢机容忍度放宽(断言区间而非精确值)。

## D3 件⑤:finalize_turn 取消臂差集补齐

### 改动点

1. `FinalizeFrame`(tools.rs,现 :1877 附近的结构体)加字段 `tool_calls: &'a [(String, String, serde_json::Value)]`(id, name, input——与 `build_synthetic_tool_result_message` 签名对齐;input 不用但保持类型一致省转换);
2. 调用点(chat_loop.rs:890 前后):`tool_calls` 被 move 进 `DispatchCtx` 前 clone 一份绑定(每个并行 task 本来就 clone,成本同量级);
3. `finalize_turn` 取消臂(cancelled == true 分支):构造 tool_result_msg 前计算差集——`result_blocks` 中 `ToolResult.tool_use_id` 集合 ∪(loop_hint Text block 不算)→ 对 `tool_calls` 中不在集合的每个 (id, name, _) 追加 synthetic block(文案与 `helpers::build_synthetic_tool_result_message` 同句式,可提一个 per-call block 构造 helper 或对该函数加 `exclude_ids` 变体——实现自选,单源优先);
4. 合成 block append 在真 result 之后、loop_hint Text 之前(顺序:真 result×N → synthetic×M → hint text;hint 在末尾的既有 wire 顺序约束不变——见 finalize_turn :1896-1924 注释,OpenAI tool 消息紧邻性)。

### 不变量

- **幂等**:差集为空(L2 并行全配对/无取消)时零行为变化;
- send 阶段取消路径(drive.rs:1993)**不动**——那是另一层(全量 synthetic),两路径共同保证任意取消后 DB 尾部配对完整;
- wire 层自愈(`chat_request_to_wire` orphan 注入)不动,不删——它兜的是不可枚举逃逸(群聊 max_turns 类),本件落地后该 warn 出现即回归信号;
- worker 模式(skip_persist)不落库的行为不变(补齐只影响落库内容,messages.push 内存态同样补齐——与 drive.rs send 阶段路径对称,worker transcript 也该配对完整)。

### 测试

- 单测(finalize 层):构造 5 tool_calls + 2 真结果 blocks + cancelled=true → 断言产出消息含 5 个 ToolResult(2 真 3 synthetic is_error)、hint Text 仍在末尾;
- 无取消路径回归:cancelled=false 时消息只含真结果(差集逻辑不触发);
- 既有 agent_loop 取消测试(`agent_loop_cancel_*` 族)全绿——它们断言 partial results 落库,新增 synthetic 不破坏既有断言的方式:若既有测试断言"消息数/tool_result 数==已完成数",需同步更新为"==tool_use 数"(实现时逐个核对,语义升级是本件目的)。

## D4 件①:CJK 测试锚(edit_file)

`tools/edit_file.rs` tests 加一条:

- 文件内容混合:全角标点(，。：；！？)、全角字母数字(ＡＢＣ １２３)、智能引号("" '' )、全角空格(\u3000)、半角混排;
- 对邻近片段做 edit(old/new 均不含全角字符);
- 断言:写回文件中所有全角字符序列逐字节保留(比较 edit 前后未触及区域的 bytes)。

## D5 spec 回填清单(实施完成后)

| spec | 增量 |
|------|------|
| `daemon-server/pattern-shutdown-checklist.md` | 顺序图补 kill_all 步 + 模式覆盖矩阵(Thin/sidecar/daemon.sh ✅,崩溃面→C.3 记注)+ 新不变量「drain 后 kill_all 前,后台 shell 启动 entry 必可达」 |
| `tool-contract/06-background-shell.md` | RULE-E-002 语义升级注记:SIGKILL → SIGTERM(3s 宽限)→SIGKILL;档位表(单杀宽限/批量 Immediate);env 覆盖钩子 |
| 取消语义(候选 `agent-loop-architecture/pattern-cancellation-guard.md` 或 turn-boundary-persist,按实际改动落) | 执行中途取消的配对完整性不变量:「任意取消后,DB 尾部 assistant(tool_use×N) 的下一 user 消息必含 N 个 tool_result(真+synthetic 差集)」——send 阶段(drive.rs)与执行阶段(finalize_turn)双层对齐 |

## D6 回滚

三件相互独立,单一 commit 内按 D1/D2/D3/D4 分 hunk;回滚单元 = 整个 PR(revert 即可,无 schema/无 wire 变更、无迁移、无配置面新增;env 钩子 EVERLASTING_SHELL_KILL_GRACE_MS 缺省即 3000,不设不影响)。
