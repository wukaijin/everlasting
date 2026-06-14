# P0 — shell process_group + kill PGID

## Goal

关闭 RULE-E-002 P0 安全漏洞:`shell` tool 当前 `child.kill()` 只杀直接子进程,通过 `sh -c` 派生的孙子进程(`sleep 60 &` / 管道 / `nohup` / `&`)在 cancel / timeout 后仍继续运行,产生孤儿进程累积。修复:启动时 `process_group(0)` 创建新进程组,cancel/timeout 时 kill 整个进程组(PGID = -PID)。

## Decisions (ADR-lite)

**Context**: 当前 `Command::new("sh")` 不设 process group,tokio 的 `child.kill()` 只对 sh 直接子有效。`sh -c "sleep 60 & exit"` 的 `sleep 60` 会被 `sh` fork 出来成为独立 PID,`sh` 被 kill 后 `sleep` 仍继续。

**Decisions**:

1. **Unix 优先**: 项目 WSL-first + macOS dev,先修 Unix 路径;Windows 上 `CREATE_NEW_PROCESS_GROUP` flag 等价但本期不实施,留 P2
2. **`process_group(0)` 而非 `process_group(pid)`**: `CommandExt::process_group(0)` 让子进程成为新进程组的 leader(PGID = 自己的 PID),kill 时传 `-PID` 给 `kill` syscall 即可清理整组
3. **kill 实现**: Unix 上通过 `nix::sys::signal::killpg(Pid::from_raw(-pid as i32), Signal::SIGKILL)`,或直接 `libc::kill(-pid as i32, SIGKILL)`,不引新依赖
4. **跨平台兼容**: 用 `#[cfg(unix)]` 分支,Windows 上维持现有 `child.kill()`(MVP 不破)
5. **不静默吞 ESRCH**: kill 失败(进程已退出)不算错,记录 `tracing::debug!` 即可;真错才 warn
6. **现有测试不破**: `cancel_kills_child_process` 测试 `sleep 60` 后 token.cancel(),本修复让 PGID 内 sleep 60 也被杀,断言"is_error + cancel marker"仍成立

**Consequences**:
- 取消 / timeout 后无孤儿进程累积
- `nohup` / `disown` / `&` 启动的后台任务随 cancel 一起被杀(可能出乎用户意料,但符合"agent 在 session 内有完全控制权"的预期)
- 管道链 `cat foo | grep bar | wc -l` 整链被杀干净
- Windows 平台行为暂不变(RULE-E-002 Windows 部分标 P2 后续)

---

## Requirements

### R1 — process_group 实施

* `app/src-tauri/src/tools/shell.rs` 在 `Command::new("sh")` 链上(Unix 路径)加 `.process_group(0)`
* 使用 `std::os::unix::process::CommandExt`
* 注释说明 PGID = 子进程 PID,后续 kill 用 `-PID`

### R2 — 杀进程组替换

* `kill_and_collect()` 函数(`shell.rs:79-99`)中 `child.kill().await` 替换为 PGID kill
* 实际实现:获取 child.id() 作为 PID,调用 `libc::kill(-pid, SIGKILL)`
* 失败时 ESRCH(进程已退出)吞掉,其他 errno 走 `tracing::warn!`
* 加 `#[cfg(unix)]` 守卫;非 Unix 走 `child.kill().await` 旧逻辑

### R3 — 测试覆盖

* 新单测 `cancel_kills_backgrounded_grandchildren`:
  - 执行 `sh -c "sleep 60 & echo $$; wait"` 或 `bash -c "sleep 60 &"`(背景化 sleep)
  - token.cancel()
  - 用 `ps` 或 `/proc/<pid>` 验证 sleep 子进程已不存在(轮询 ≤ 2s)
* 新单测 `timeout_kills_pipeline_grandchildren`:
  - 执行 `yes | head -c 1000000`(管道链)timeout=200ms
  - 验证 `yes` 进程已被 kill(PID 探测)
* 现有 `cancel_kills_child_process` / `timeout_kills_long_command` 必须仍 pass

### R4 — 文档

* `shell.rs` 顶部 docstring 加"进程组"段,说明 cancel/timeout 杀整组
* ARCHITECTURE.md §2.5.4 引用 RULE-E-002

---

## Acceptance Criteria

* [ ] `shell.rs` `Command::new("sh")` 链(Unix)上有 `.process_group(0)`
* [ ] `kill_and_collect()` 在 Unix 上用 `libc::kill(-pid, SIGKILL)` 杀进程组
* [ ] 至少 2 个新单测覆盖"杀孙子进程"
* [ ] 现有 19+ shell 单测全部仍 pass
* [ ] `cargo test --lib tools::shell` green
* [ ] `cargo check` 无新增 warning

---

## Definition of Done

* 上述 Acceptance Criteria 全 ✅
* PR merge 后更新 `docs/_reviews/DEBT.md` 中 `RULE-E-002` 条目:`Status: closed` + `Closed At: <commit hash>` + `Related PR: #N`

---

## Out of Scope

* Windows 平台 process group 实施(`CREATE_NEW_PROCESS_GROUP` + `GenerateConsoleCtrlEvent`)— 后续 P2
* `nohup` / `setsid` 等特殊情况下保留的子进程(setsid 创建新 session,本期不处理)
* 进程组审计日志(谁启动了哪些 PGID)— 后续 P2
* SIGTERM 优雅退出(MVP 直接 SIGKILL,符合 cancel 紧急语义)

---

## Technical Approach

### 实施步骤

**Step 1: 加 process_group (Unix)**

```rust
use std::os::unix::process::CommandExt;

let mut cmd = Command::new("sh");
cmd.arg("-c")
    .arg(command)
    .current_dir(&validated_cwd)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
// Create a new process group so kill_and_collect can clean up
// the entire shell tree (sh + all descendants from `&` / pipes
// / nohup). PGID == child.id() == the sh PID.
#[cfg(unix)]
cmd.process_group(0);
apply_safe_env(&mut cmd);  // 来自 RULE-E-001 修复
```

**Step 2: 改 kill_and_collect**

```rust
async fn kill_and_collect(child: &mut Child) -> ShellResult {
    // Kill the entire process group, not just the direct child.
    // `child.id()` is the sh PID = PGID because of process_group(0).
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            let pid_raw = pid as i32;
            // ESRCH (3) = no such process (already exited); treat as success.
            let ret = unsafe { libc::kill(-pid_raw, libc::SIGKILL) };
            if ret != 0 {
                let errno = std::io::Error::last_os_error();
                if errno.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(
                        error = %errno,
                        pid = pid_raw,
                        "shell: killpg failed"
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }

    // Wait for the process to exit so we don't leave a zombie.
    let status = child.wait().await.ok();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_end(&mut stdout).await;
    }
    if let Some(err) = child.stderr.take() {
        let _ = err.read_to_end(&mut stderr).await;
    }
    ShellResult {
        stdout, stderr,
        exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
        cancelled: true,
        timed_out: false,
    }
}
```

**Step 3: 测试**

按 R3 加 2 个新单测。**注意**:验证子进程被杀的探测方法:
- 写一个 sh 脚本,把孙子进程 PID 输出到临时文件:`sh -c "sleep 60 & echo $! > /tmp/test-pid; wait"`
- cancel 后读取该文件,验证 `/proc/<pid>` 不存在(`std::path::Path::exists`)

---

## Technical Notes

### 关键文件

* `app/src-tauri/src/tools/shell.rs:79-99` — `kill_and_collect` 改 PGID
* `app/src-tauri/src/tools/shell.rs:237-242` — `Command` 链加 `process_group(0)`
* `app/src-tauri/src/tools/shell.rs:441-947` — tests 块,加新单测
* `Cargo.toml` — `libc` 已在依赖中(若不在需加)

### libc crate 依赖

项目 `app/src-tauri/Cargo.toml` 大概率已有 `libc`(Tauri 自带)。如无,加 `libc = "0.2"`。

### 进程组 kill 的限制

- `setsid` 创建新 session 后,杀 PGID **不会**影响 setsid 出去的进程(它们在新 session)
- 这是合理的:`setsid` 是显式 detach 信号,LLM agent 不应主动用
- 后续 P2 文档化"已知无法追到的边界情况"

### 与 RULE-E-001 的协作

两者独立 PR,但都在 shell.rs。本 task 可顺序合并到 RULE-E-001 之后(若 RULE-E-001 先 merge)。

---

## Research References

* `.trellis/reviews/DEBT.md` — RULE-E-002 完整 finding 描述
* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — §2.5 + §3.1 原始论据
* Linux man `kill(2)` / `setpgid(2)` — PGID 模型
* tokio `process::Command` Unix extension 文档