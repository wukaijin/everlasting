<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->

## Invariant: daemon 生命周期绑定 GUI 进程(orphan-guard, 2026-07-27)

**问题**:`tauri dev` 的 Rust live reload 是**强制 kill GUI 进程**(不走
`RunEvent::Exit`),所以 `SidecarHandle::kill()` 永远跑不到。daemon(因
`serve_daemon` 修复后能稳定存活)会成孤儿继续占 7456 → 下次 sidecar 探测
端口冲突 exit 1 → 前端 "daemon 不可用"。GUI crash / 被强杀同理。

**契约**:daemon 进程的生死**必须**绑死其父进程(GUI sidecar 持有者)。
无论 GUI 怎么死(正常 `RunEvent::Exit` / live reload 强杀 / crash / kill -9),
daemon 都必须自动退出,绝不留孤儿占端口。

**实现**(`bin/everlasting-daemon.rs::main`,Linux):
`prctl(PR_SET_PDEATHSIG, SIGTERM)` —— 内核在父进程终止时自动给 daemon 发
SIGTERM,走 [`shutdown_signal`](#) 的优雅退出路径。两个 race 防护:
1. `getppid() == 1`(父进程已死被 init 收养)→ 立即 exit 1,不 bind 端口。
2. prctl 只在调用一刻设置;daemon 不会 reparent,故无需重设。

**不变量**:
- **禁止**移除 prctl 调用(或改成 no-op)除非有等价的孤儿清理机制
  (如 PID 文件 + 启动时清理)。移除后,任何 GUI 异常退出都会留孤儿。
- standalone 启动(`cargo run --bin everlasting-daemon` / `daemon.sh`)也安全:
  父进程是 shell,shell 退出 daemon 跟着退,符合"前台跑、关终端即停"。
- 非 Linux 平台(macOS/Windows)无 prctl —— 需另外实现(`proc_exit` /
  Job Object),当前 sidecar 模式仅 Linux 有此问题。

**验证**:手动 `kill -9` daemon 父进程后,daemon 应在 ~1s 内收到 SIGTERM 并
走完整 graceful shutdown(日志:`received SIGTERM` → `shutdown complete`
→ `exited cleanly`),端口自动释放。

---
