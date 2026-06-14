# P0 — shell env_clear + 白名单注入

## Goal

关闭 RULE-E-001 P0 安全漏洞:`shell` tool 启动的子进程当前继承父进程全部环境变量(含 `ANTHROPIC_API_KEY` / `*_TOKEN` 等敏感凭证),LLM 一句 `env`/`printenv` 即可窃取。修复:`Command::new("sh").env_clear()` + 注入白名单变量。

## Decisions (ADR-lite)

**Context**: 当前 `Command::new("sh").arg("-c").arg(command).current_dir(...)`(`shell.rs:237-242`)无任何 env 操作,tokio 默认继承父进程全部 env。Permission 系统的 Tier 4 ask 拦截"该不该执行",拦截不了"执行后内部窃密"。

**Decisions**:

1. **env_clear() 而非 select-clear**: 选 `cmd.env_clear()` + 白名单重建,**不**用白名单匹配移除个别变量。原因:全清后重建语义清晰,漏匹配风险更低;白名单也包含后续审计/诊断需要的标识变量(`EVERLASTING_SESSION_ID` 等)
2. **白名单变量**(最小必需集):
   - `PATH`(命令解析必需)
   - `HOME` / `USER` / `LOGNAME`(部分命令依赖,如 `~/.bashrc`)
   - `LANG` / `LANGUAGE` / `LC_ALL` / `LC_*`(本地化)
   - `TERM`(终端类型,部分命令检测)
   - `TZ`(时区,影响 `date` 等命令)
   - `TMPDIR`(临时文件位置)
   - `EVERLASTING_SESSION_ID` / `EVERLASTING_PROJECT_ROOT`(诊断,可选)
3. **PATH 复用父进程**: 用 `std::env::var("PATH").unwrap_or_default()` 读取父进程 PATH 并注入,不重写默认路径(避免破坏 npm/python/go 等开发命令)
4. **不注入 API key/TOKEN**: `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `*_TOKEN` / `*_SECRET` **绝不在白名单中**
5. **跨平台**: Unix 用 `std::os::unix::process::CommandExt` 的 `env_clear`(tokio `Command` 通过 `as_std_mut()` 拿 std::Command)。Linux/macOS 同实现,Windows 上 `env_clear` 行为不同但 tokio 支持,**同代码路径**,靠 cfg(test) 验证跨平台
6. **不修改边界检查**: working_directory 仍走 `assert_within_root`(boundary.rs),与本修复正交

**Consequences**:
- `env` / `printenv` 在子进程看不到 `ANTHROPIC_API_KEY`
- `npm install` / `cargo build` / `pnpm tauri dev` 等开发命令正常(需要 PATH/HOME)
- 单元测试 `execute_echo` / `execute_stderr_command` / `timeout_kills_long_command` 等不应破
- 用户在 shell 提示中已配的自定义 env(如 `GOPROXY=direct`)需要手动在白名单加,**这是已知 UX 损失**,留待后续

---

## Requirements

### R1 — env_clear 实施

* `app/src-tauri/src/tools/shell.rs:237-242` `Command::new("sh")` 链上加 `env_clear()`
* 紧随其后调用 `.env("PATH", path)` / `.env("HOME", home)` / ...(按白名单列表)
* 提取 `apply_safe_env(cmd: &mut Command)` 私有函数,集中白名单逻辑,加 docstring 说明
* 提取常量 `SAFE_ENV_VARS: &[&str]`(或运行时列表)便于审计

### R2 — 测试覆盖

* 新单测 `execute_env_does_not_leak_api_key`:
  - 设置父进程 `ANTHROPIC_API_KEY=secret-value` (用 `std::env::set_var`,测试结束后 `remove_var`)
  - 执行 `env | grep API_KEY` 或 `printenv ANTHROPIC_API_KEY`
  - 断言 tool_result 不含 `secret-value`,exit code 非 0
* 新单测 `execute_preserves_path`:
  - 执行 `which sh` 或 `echo $PATH`
  - 断言 PATH 非空(继承父进程)
* 新单测 `execute_preserves_home_lang_term`:
  - 执行 `echo "$HOME $LANG $TERM"`(分别在 WSL 上能跑)
  - 断言三变量都存在(允许为空字符串)
* 现有测试 `execute_echo` / `cancel_kills_child_process` / `timeout_kills_long_command` 必须仍 pass(回归保护)

### R3 — 文档

* `shell.rs` 顶部 docstring 加"环境变量安全"段,说明白名单机制
* `shell` tool definition 的 description 加一句"Environment is restricted to a safe allowlist; API keys and tokens are not inherited"
* ARCHITECTURE.md §2.5.4 (或对应 shell tool spec) 引用 RULE-E-001

---

## Acceptance Criteria

* [ ] `shell.rs` `Command::new("sh")` 链上有 `env_clear()`
* [ ] `apply_safe_env` 函数集中管理白名单
* [ ] 至少注入 PATH/HOME/USER/LOGNAME/LANG/TERM/TZ/TMPDIR
* [ ] 单测覆盖"API key 不泄漏" + "PATH 保留"
* [ ] 现有 19 个 shell 单测全部仍 pass
* [ ] `cargo test --lib tools::shell` green
* [ ] `cargo check` 无新增 warning

---

## Definition of Done

* 上述 Acceptance Criteria 全 ✅
* PR merge 后更新 `docs/_reviews/DEBT.md` 中 `RULE-E-001` 条目:`Status: closed` + `Closed At: <commit hash>` + `Related PR: #N`
* SPEC-DRIFT.md 中 DRIFT-002 可关联到此(若涉及 web_fetch 重写 env_clear 文档同步)

---

## Out of Scope

* Windows 平台特定 env_clear 行为差异(MVP 不修,跨平台测试若失败留待后续)
* 用户自定义 env(如 GOPROXY/HTTP_PROXY)的白名单扩展入口(后续产品决策)
* 其他 tool 的 env_clear(grep/glob/edit_file 等 std::process 不直接调用 env,本修复仅针对 shell)
* shell `safe_env_allowlist` 配置化(后续 P2 债务)

---

## Technical Approach

### 实施步骤

**Step 1: 抽 `apply_safe_env` 函数**

```rust
/// Apply a safe-allowlist environment to `cmd`.
///
/// `env_clear()` removes every inherited variable (including
/// `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `*_TOKEN` / `*_SECRET`
/// from the parent process). The allowlist below is the minimum
/// required for common dev commands (`npm`, `cargo`, `pnpm`,
/// `make`, `git`, `ls`) to keep working.
///
/// Adding a variable here is an intentional trust decision — it
/// becomes readable by every command the LLM runs. Update
/// `docs/ARCHITECTURE.md §2.5.4` if you add one.
fn apply_safe_env(cmd: &mut Command) {
    cmd.env_clear();
    // PATH: required for command resolution. Inherit from parent.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    // Identity / locale — most commands probe these.
    for var in &[
        "HOME", "USER", "LOGNAME",
        "LANG", "LANGUAGE", "LC_ALL",
        "TERM", "TZ", "TMPDIR",
    ] {
        if let Ok(v) = std::env::var(var) {
            cmd.env(var, v);
        }
    }
}
```

**Step 2: 在 `execute()` 调用 spawn 前调用**

```rust
let mut cmd = Command::new("sh");
cmd.arg("-c")
    .arg(command)
    .current_dir(&validated_cwd)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
apply_safe_env(&mut cmd);  // ← 新增
```

**Step 3: 测试**

按 R2 加 3 个新单测。**注意**:`std::env::set_var` 在多线程测试中不安全,需用 `serial_test` crate 或 `cargo test --test-threads=1`。最简单:`#[test]` 不并发时 setup env,test 末尾 `remove_var` 清理。

---

## Technical Notes

### 关键文件

* `app/src-tauri/src/tools/shell.rs:237-242` — 修复点
* `app/src-tauri/src/tools/shell.rs:441-947` — tests 块,加新单测
* `app/src-tauri/src/llm/types.rs` — `ToolDef.description`(更新 shell description)
* `docs/ARCHITECTURE.md` — 若有 shell tool spec,引用此修复

### tokio Command env_clear 的可用性

* `tokio::process::Command::env_clear()` — ✅ 存在,等价于 `std::process::Command::env_clear()`
* `tokio::process::Command::env(key, val)` — ✅ 存在
* 不需要 `as_std_mut()` 转换

### 跨平台注意

* Unix 上 `env_clear()` 后注入 HOME 等,`sh -c "echo $HOME"` 能输出值
* Windows 上 cmd 的 env 模型略有不同,但 tokio `env_clear` 行为是跨平台的,注入逻辑也通用
* CI 跑 `cargo test --lib` 在 Linux 上验证;Windows 验证可在后续 P1 跨平台 task 做

### 与 RULE-E-002 的协作

本修复只管 env,进程组 kill 是 RULE-E-002 子 task 范围。两者独立 PR。

### SPEC-DRIFT 同步

shell.rs 顶部 docstring 需更新(原 docstring 没提 env 限制)。SPEC-DRIFT.md 本条不涉及。

---

## Research References

* `.trellis/reviews/DEBT.md` — RULE-E-001 完整 finding 描述
* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — §2.5 + §3.1 原始论据
* OWASP SSRF / LLM Agent 攻击面: env_clear 是 LLM agent 工具的 standard hardening
* Anthropic prompt injection threat model: LLM 能 `env` 读取凭证是经典 attack vector