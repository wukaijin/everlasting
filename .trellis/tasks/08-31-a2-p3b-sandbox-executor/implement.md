# Implement — A2+ P3b 执行期沙盒执行器

> 每个 PR 独立可合并、独立可回滚、合并后全绿。PR 边界按「不留死代码」切:
> 每一批新代码当批接线(DEBT 历史教训:模块级 dead_code 伞被批量清理过)。

## PR1 — sandbox 模块 + config + 前台 shell 接入

- [x] `src/sandbox/` 五文件(mod / landlock / seccomp / policy / tests)落地,
      ABI 常量对齐内核 UAPI 头并单测钉死数值(AC1 前置)。
- [x] `commands/config.rs` + `daemon/routes/config.rs`:`sandbox_enabled`(默认 true)、
      `sandbox_extra_writable`(默认 [],读取时并入 `~/.cargo`)、只读派生字段
      `sandbox_capability`(`Capability::probe()` 结果,不落盘);`get_app_config`
      双 transport additive(C3)。
- [x] `tools/shell.rs` 接入:`sandbox::decide`(四项判定)+ `prepare`/`apply`
      (父进程准备 + pre_exec 纯 syscall),spawn 管线其余不动;后验拦截指引文案
      (§2.5,单测钉死要点,复用本次判定结果、不二次查询)。
- [x] `ToolContext` 加 `mode` 字段(dispatch 层 `DispatchCtx.session_mode` 灌入,
      评审 B1/D5)——前台触发判定的数据来源。
- [x] 集成测试 `tests_sandbox.rs`:AC1(写拒/放行/interop 拒/读不控)、AC2(断网 +
      AF_UNIX 放行)、AC3(SideEffect/Ask/Yolo 不沙盒)、AC4(开关关 = 现状)、
      AC5(探测桩 fail-open)。
- [x] 审计:`AuditKind::SandboxedShellExecution` + 写手(D2)。
- 验证:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
    cargo test --lib && cargo clippy --lib && cargo fmt --check
  cargo test -p everlasting --test e2e   # 路由清单若动则同步
  ```
- 回滚点:revert 本 PR → 无 config 字段消费方,前端尚未引用,行为整体回现状。

## PR2 — background_shell 接入

- [x] `BackgroundShellRegistry::start` trait 签名加 `sandbox: Option<SandboxSpec>`
      参数(全部 impl 同步改,评审 B1/D5);`run_background_shell` tool 层算好下传,
      registry spawn 点只消费不判定。
- [x] `crate::background_shell` spawn 点同款施加;`shell_status` /
      `shell_kill` / 通知 / sweeper 语义不变(RULE-SHELL-001 契约)。
- [x] 集成测试:`run_background_shell` ReadOnly 档受沙盒(AC6)——后台命令写
      worktree 外被拒 + status 输出可见错误。
- 验证:同 PR1 命令;另跑后台专项:
  ```bash
  cargo test -p everlasting --lib "background"
  ```
- 回滚点:revert 后前台路径不受影响(PR1 独立成立)。

## PR3 — 设置面 + spec 收编 + live 验收

- [x] 前端 Settings:`sandbox_enabled` 开关(既有 `SETTABLE_APP_FLAGS` 通道)+
      `sandbox_extra_writable` 列表编辑(**新增 `set_app_config_list` 写命令**,
      daemon route + Tauri 双端,评审 W1;vitest 覆盖 store 读写与默认值)+
      `sandbox_capability` 只读徽标(R8,沙盒生效 / 已回退)。
- [x] spec 新增 `.trellis/spec/backend/sandbox-executor.md`:规则集契约(可写根
      来源铁律 / exec 允许面 / 设备清单)、五条陷阱、pre_exec 信号安全纪律、
      fail-open 语义、kill-switch、seccomp 断网契约、interop socket 残余面记录
      (D4)。
- [x] ROADMAP:A2+ P3 行移 §1.2 已实施(P3b 部分),P3c 余留第四档。
- [x] live 验收:`scripts/turn-smoke.sh` 真实 LLM 轮(AC8)——ReadOnly 工具命令
      无误杀;脚本补 `SandboxedShellExecution` kind 计数断言(新增 kind 默认被
      脚本静默忽略,评审小建议 2);手动项:WSL 真机 `.exe` 拒执行抽查。
- 验证:
  ```bash
  cd app && pnpm test && pnpm build && npx vue-tsc --noEmit
  node scripts/remote-e2e-smoke.mjs   # remote 链路冒烟(config 新字段 additive)
  ```
- 回滚点:前端与文档层,随时可退。

## 完成门(全 PR 后)

- [x] AC1–AC9 逐项勾;DEBT 无新增;journal 记录(含 pre_exec 纪律、CVE-2025-59532
      铁律的落地位置);任务归档。
- 全量回归口径(AGENTS.md):后端 `cargo test -p everlasting --lib` + e2e +
  前端 vitest + Playwright e2e + build 全绿(测试数为当期实际值,不写死口径)。

## 风险与预案

| 风险 | 预案 |
|---|---|
| pre_exec 内非信号安全操作 → 死锁/UB | 设计 §2.3 父进程全准备;评审时对闭包逐行核对只含 syscall |
| 误杀真实工作流(ls 类命令写缓存等边缘) | AC8 turn-smoke live + kill-switch 一键关;拦截指引文案引导 |
| 老 WSL / 异常内核探测失败 | fail-open(AC5 桩测试)+ 探测结果进 get_app_config 可见 |
| CI(GitHub Actions ubuntu runner)内核过老 | runner 为 24.04(6.8+),Landlock v1 必在;若断网测试受 CI 网络策略影响,AC2 断言改 `/dev/tcp` 连接失败即可(EPERM 与不可达都算失败路径) |

## 完成记录(2026-08-31,实际执行回填)

- 三 PR 均在 main 直接提交(P3a 先例):PR1 `8c2c881`、PR2 `bba8dd2`、PR3 见 task.json commit 字段。
- **实现期发现并修正的 design 缺口(已记入 spec sandbox-executor.md §2/§6)**:
  1. exec 面补 `/lib` `/lib64` `/usr/lib` 静态根 — 动态链接 ELF 解释器由内核在
     execve 时打开需 EXECUTE;spike 探针硬编码了 lib 根而 design 的 PATH 条款
     静默丢失,实测(PATH 无 lib 目录)所有动态二进制 spawn 即 EACCES。
  2. PATH 解析过滤 `/mnt/` 前缀 — WSL 把 Windows 盘挂载进 PATH,不过滤则
     exec 面重开 interop 逃逸(与「显式不含 /mnt/c」冲突);worktree 位于
     /mnt/c 的用户经可写根条款保留 exec(有意例外,spec 已记录)。
- 回归实测:后端 `cargo test -p everlasting --lib` 2162 绿(含真内核 AC1/AC2
  集成矩阵 + AC6 后台沙盒);clippy/fmt 干净;e2e 6 绿;前端 vitest 1492 绿 +
  build + vue-tsc 干净;remote E2E 冒烟通过;`turn-smoke.sh --sandbox-probe`
  live 过(1 条 sandboxed_shell_execution 审计行,无误杀,AC8)。WSL 真机
  `.exe`/`/init` 拒执行由集成测试 `integration_interop_exec_denied` 在本机覆盖。
- 审计 kind 落表 live 验证:`turn-smoke.sh` 新增 `--sandbox-probe` 旗标 +
  `--assert-turn-usage` 同款的 3.6 节断言(内核不支持时降级 WARN,fail-open)。
