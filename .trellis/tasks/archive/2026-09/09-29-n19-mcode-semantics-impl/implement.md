# Implement:N19 三件收口 + CJK 测试锚

> 顺序 = D1 → D2 → D3 → D4(件间独立,先做最实的 shutdown 缺口);每步后跑对应验证,D5 spec 回填在代码全绿后做。

## 步骤清单

### Step 1(D1)daemon shutdown 补 kill_all

- [ ] `daemon/server.rs::shutdown_signal` 步骤 2 后加 `state.background_shells.kill_all().await`(log-only,注释含顺序理由与 Immediate 档理由)
- [ ] `daemon/server.rs::tests` 加 SIGTERM 集成测试(共享 `SIGNAL_TEST_MUTEX`):活跃后台 shell → SIGTERM → Killed + serve_daemon 返回
- [ ] 验证:`cargo test -p everlasting --lib "daemon::server::tests::" `(PKG_CONFIG_PATH 按 AGENTS.md)

### Step 2(D2)kill_and_collect 两段式

- [ ] `tools/shell.rs::kill_and_collect` 加 `grace_ms: u64` 参数;SIGTERM→timeout(grace)→SIGKILL;grace=0 直杀;ESRCH 容错保持;Windows 分支忽略 grace(注释)
- [ ] shell.rs 取消臂/超时臂传 `SHELL_KILL_GRACE_MS`(新常量,env `EVERLASTING_SHELL_KILL_GRACE_MS` 覆盖;env 读取放 lazy/OnceLock 单源)
- [ ] `background_shell/in_memory.rs`:`kill_tx: oneshot::Sender<()>` → `Sender<u64>`;`registry.kill` 发宽限值、`kill_all_for_session`/`kill_all` 发 0、超时臂传宽限值;`kill_rx` 臂传值到 kill_and_collect;run_background_task 本地 kill_and_collect 同步加 grace 参数
- [ ] 单测×3(trap 宽限内退 / trap 忽略 TERM 强杀 / grace=0 直杀),前台后台两处覆盖(后台经 registry.kill 测,前台直接构造 Child 或经 execute)
- [ ] 验证:`cargo test -p everlasting --lib "tools::shell\|background_shell\|shell_kill"` 一条命令内过滤

### Step 3(D3)finalize_turn 差集补齐

- [ ] `FinalizeFrame` 加 `tool_calls` 字段;chat_loop.rs 调用点在 move 进 DispatchCtx 前 clone
- [ ] `finalize_turn` 取消臂:真 result id 集合 → 差集追加 synthetic is_error block(顺序:真×N → synthetic×M → hint Text);单源构造(扩 `build_synthetic_tool_result_message` 或提 per-call helper)
- [ ] worker 模式 skip_persist 行为不变(messages.push 同样补齐)
- [ ] 单测:5 tool_calls 中途取消 → 落库消息 tool_result 数 == 5;无取消零变化;既有 `agent_loop_cancel_*` 族若断言 tool_result 计数需同步语义升级
- [ ] 验证:`cargo test -p everlasting --lib "tests_agent_loop"` + `agent::chat_loop`

### Step 4(D4)CJK 测试锚

- [ ] `tools/edit_file.rs` tests:`cjk_fullwidth_roundtrip_preserved`(全角标点/全角字母数字/智能引号/全角空格;edit 邻近片段;断言未触及区域逐字节保留)
- [ ] 验证:`cargo test -p everlasting --lib "tools::edit_file"`

### Step 5 全量回归 + spec 回填(D5)

- [ ] `cargo test -p everlasting --lib` 全绿(多线程默认,勿加 --test-threads=1)
- [ ] spec 三处回填(design D5 表);grep 校验外部引用无需同步(纯增量语义,无结构变更)
- [ ] BACKLOG 附录 C.1 N19 行划掉+交付描述;ROADMAP §1.2 加行

### Step 6 收尾

- [ ] `git status` 干净度核对 → 按 workflow 3.4 批量 commit 计划
- [ ] `/trellis:finish-work`(archive + journal)

## 验证命令速查

```bash
# 全部后端测试(AGENTS.md 前置)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 或从根:cargo test -p everlasting --lib(PKG_CONFIG_PATH 仍需)
# 范围过滤(一条命令内,避免多次 relink 税)
cargo test -p everlasting --lib "daemon::server::tests"
cargo test -p everlasting --lib "background_shell"
```

## 回滚点

- 每 Step 独立可 revert;D6:整 PR revert 无 schema/wire/迁移面。
