# sandbox fail-open 审计可区分

> 来源:BACKLOG 附录 B 候选 N5(P1,安全+测试视角;2026-09-05 群聊 session `082add5c` 共识)。
> 2026-09-26 立项;同日 brainstorm 三决议 + 群聊评审(session `150e853d`,12 结论全 verified,
> 转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-26-# 评审任务规划09-26-sandbox-failopen-audit(沙箱-150e853d.md`)回填。
> 技术方案见 [design.md](./design.md),执行清单见 [implement.md](./implement.md)。

## Goal

审计/可观测面能区分「命令在沙箱内执行」与「命令裸跑,以及为什么裸跑」——事后归因不再混淆。配套:沙盒测试在 CI 上的覆盖失守可归因(N6)。

## 背景与痛点

- fail-open(能力探测失败 → 裸跑)完全静默:`resolve_policy` 首支 `!cap.ok() → Off`,与三种**有意**裸跑(Yolo / kill-switch / 项目 off)共用同一 skip reason,trace 与审计两层均不可区分;可观测仅剩进程内一次 `tracing::info!` + 设置面一维 bool。
- 第二类静默降级:`landlock_net=false`(ABI<4)时 BindOnly 档降级 Block——Session 59 本机(ABI 3)实测触发,同样仅 warn。
- 归因手法实证失效:Session 59 以「Block 档沙箱审计行 1 条」验证新二进制在跑,fail-open 机器上该手法静默给错答案;09-25-dns-block-detect 整个任务即沙箱拦截归因工作。
- N6:`tests_sandbox.rs` 能力类 SKIP 在 Linux 上 probe 失败时静默 pass,CI 沙盒覆盖率无门禁。
- 评审实锤两处归因缺陷:①P3c 升级重跑(沙箱首跑失败→Ask 批准→unsandboxed 重跑)后,终态 `tool_executed` 行按旧设计字段缺席会被读作「沙箱执行」,且首跑的 `sandboxed_shell_execution` 行交叉印证错误归因;②worker 模式明文跳过 `tool_executed`(B6 PR1b),工具层审计行却不受门控——worker 的 Skip 路径裸跑(Yolo 下每条命令)审计层零痕迹。

## Requirements

### R1 — tool_executed 审计归因字段(决议 ② + 评审结论 2/6/11)

shell 族命令的 `tool_executed` 行 payload 携带**必写**(shell 族)的 `sandbox` 归因字段,
wire 值域(去前缀——字段存在已表达裸跑;显式 `sandboxed` 值消灭「缺席=沙箱执行」的
default-safe 反模式):

```
sandboxed | failopen | yolo | kill_switch | project_off | no_session | grant | escalation
```

零新审计行、零新 kind,不翻 §2.2「skips are not security events」哲学(audit UI 的
kindFilter 是 kind 级筛选,不受 payload 字段污染)。`escalation` = P3c/§4b 升级重跑
(用户批准后 unsandboxed 重跑为最终执行);词表双层类型化见 design §2.2。

### R2 — skip 真源结构化

`Policy::Off` 带 cause、`Decision::Skip` reason 结构化;fail-open 与三类有意 off 在
类型层不可混淆。求值序零改动。形变波及面 = 全仓 25 处(编译器兜底;
`permission.rs:357` 的 `!= Policy::Off` 比较式改 `matches!`)。

### R3 — net 降级显式化(决议 ③)

BindOnly→Block 降级在 `sandboxed_shell_execution` 行 ruleset 摘要带
`net-degraded=bindonly→block` 标记。

### R4 — capability 暴露升维(决议 ③)

`get_app_config` additive 新增 `sandbox_capability_detail: { landlock, landlock_net,
seccomp }`(只读派生);`sandbox_capability: bool` 保留向后兼容。设置面 GeneralTab 徽标
升维三维:全绿 / 「BindOnly 档将降级断网」黄 / 「已回退(fail-open)」红。

### R5 — N6 CI 门禁(决议 ① 并包 + 评审结论 8)

能力类 SKIP 收敛为 `require_sandbox!` 宏(panic 消息带三维 capability 明细,防 runner
镜像变更后门禁静默死亡);env `EVERLASTING_SANDBOX_TESTS_REQUIRED=1`(ci.yml **rust
job** 设)时 probe 失败 panic;本地 dev 无能力照旧 SKIP 不阻断。主机形状类 SKIP
(`/init`、`/mnt/c`、`$HOME`)不动。spec §5 记运营条款:runner 镜像变更后复查 SKIP
计数=0。

### R6 — 前端消费

`audit.ts` payload 类型 + 人话文案映射(去前缀值域);审计面板归因行渲染带噪音标准:
**异常类(failopen / escalation)高亮,常态类(sandboxed/yolo/grant/…)低亮度纯文本**
(Yolo 下每行显示但不吵)。GeneralTab 徽标三态。

### R7 — worker 反转例外(评审疑点 B,用户裁定 5(a))

worker 模式的 `tool_executed` 跳过门控(tools.rs:1782 `skip_persist`)加一条例外:
**仅当 shell 族携带裸跑归因(failopen / off 族 / grant)时破例写行**——与工具层
`sandboxed_shell_execution` / grant 行不受 worker 门控的现状对称(worker 审计面:
沙箱跑→工具层行;裸跑→本例外行)。改动约 :1782 一处条件。

### R8 — 边界声明(design 承载)

`run_background_shell`:注册时 `tool_executed` 走串行落表点(通道成立);spawn 时写
共用 kind 的 `sandboxed_shell_execution`;**后台升级重跑发生在 registry 完成时,无
`tool_executed` 行可挂归因**——design 写明该边界(升级 provenance 走通知侧 ask 审计,
与前台 §4b 语义平行)。

## Acceptance Criteria

- [x] AC1:OffCause 归因矩阵单测(failopen/yolo/kill_switch/project_off/no_session +
      grant),经**串行落表点 tools.rs:1786** 消费侧断言 payload 值正确;failopen 端到端
      仅受限环境 live 验证(OnceLock probe 不可伪造,措辞如实)。
- [x] AC2:沙箱执行路径的 `tool_executed` 行带 `sandbox: "sandboxed"` **显式值**,且伴随
      既有 `sandboxed_shell_execution` 行;既有测试升级为显式值断言(不只「不破」)。
- [x] AC3:escalation 场景(沙箱首跑失败→批准→unsandboxed 重跑)终态 `tool_executed` 行
      = `sandbox: "escalation"`(非缺席/非 sandboxed),首跑 `sandboxed_shell_execution`
      行照写(per-attempt 与终态分工)。
- [x] AC4:worker 模式下 shell 裸跑归因行破例落表(5(a));worker 沙箱执行仍无
      `tool_executed` 行(既有 B6 行为不变)。
- [x] AC5:`run_background_shell` 注册即返路径的 `tool_executed` 行归因值正确;后台升级
      重跑无 tool_executed 行的边界在 design 声明并在测试注释锚定。
- [x] AC6:~~实现~~ 勘察修正:`net=bind_only(...)->block(degraded)` 已由 09-21 任务交付
      (summary + tests_sandbox.rs:856 既有锚),本任务零代码,验收採既有实现。
- [x] AC7:`get_app_config` 返回三维 detail;GeneralTab 徽标三态渲染(vitest)。
- [x] AC8:审计面板归因行渲染:异常类高亮 / 常态类低亮度(vitest);payload 解析测试。
- [x] AC9:`EVERLASTING_SANDBOX_TESTS_REQUIRED=1` + probe 失败 → 测试 fail(panic 含
      三维明细);不设 env → 照旧 SKIP(单测/受限环境冒烟)。
- [x] AC10:门禁全绿:`cargo test -p everlasting --lib`(含沙箱模块定向)+ `pnpm test`
      + `scripts/turn-smoke.sh` live 一轮(sqlite3 -readonly 抽查 tool_executed payload
      带 sandboxed 值;**附 kill-switch 配方**:置 `sandbox_enabled=false` 跑一轮,抽查
      归因值 `kill_switch`)。
- [x] AC11:spec 沉淀:sandbox-executor.md §5 更新(归因字段/词表/net 降级标记/
      capability 三维/CI 门禁 env + runner 镜像复查条款);BACKLOG N5/N6 标 ✅;
      ROADMAP §1.2 补行。

## Out of Scope

- 不改 fail-open 降级行为本身(既定设计,本任务只做可观测)。
- 不做 bwrap / 网络白名单 / egress 代理(既有 follow-up)。
- 不为 skip 新增独立审计 kind(决议 ② 已否;理由见 design §4)。
- 后台 shell 升级重跑的 tool_executed 归因(R8 边界,走通知侧 provenance,不扩)。
- macOS runner 沙盒覆盖(N6 只门禁 Linux 能力类 SKIP;docker-runner 不做)。
