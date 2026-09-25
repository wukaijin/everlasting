# Implement — sandbox fail-open 审计可区分(09-26-sandbox-failopen-audit)

> 执行顺序 = 依赖序:真源结构化 → 词表/通道 → 落表点 → 前端 → CI 门禁。
> 落表点勘误(评审):shell 唯一串行 `tool_executed` 落表点 = **tools.rs:1786**
> (:407 L2 / :773 L3a 是只读并发批,shell excluded——不消费不填)。
> 验证命令均为仓库既有门禁(AGENTS.md 口径)。

## Checklist

### A. 后端真源(sandbox/mod.rs + policy.rs)

- [x] `OffCause` / `SkipReason` 枚举落地;`Policy::Off { cause }` 形变(全仓 25 处匹配,
      编译器兜底);`permission.rs:357` 比较式改 `!matches!(.., Off { .. })`。
- [x] `resolve_policy` 六支带 cause;`resolve_session_policy` 首支 FailOpen、无 session
      分支 NoSession。
- [x] `DURABLE_GRANT_SKIP_REASON` 常量退役,shell.rs grant-hit 匹配改枚举
      (grant-hit 审计行为逐字节不变)。
- [x] ~~`build_spec` net 降级摘要标记~~ **勘察修正:已存在**(09-21-sandbox-net-bindonly 交付,
      `mod.rs` summary 带 `net=bind_only(...)->block(degraded)` + tests_sandbox.rs:856 锚)——
      本任务零代码,R3/AC6 採既有实现。
- [x] 单测:resolve_policy cause 六支矩阵;Skip reason 断言更新(既有
      `DURABLE_GRANT_SKIP_REASON` 断言两处改枚举形);net 降级摘要断言。

### B. 词表 + 通道 + 落表点(tools/mod.rs + shell.rs + audit.rs + chat_loop/tools.rs)

- [x] `SandboxAttribution` 扁平 enum(8 值,wire snake_case)+ `From<OffCause>` +
      `From<&SkipReason>`(Grant 直映)。
- [x] `ToolContextUpdate.sandbox_attribution: Option<SandboxAttribution>`(Default 派生)。
- [x] shell / run_background_shell:**归因计算在 §4b 升级重跑 settle 后单一出口**——
      Sandbox 无重跑 → `Some(Sandboxed)`;重跑发生 → `Some(Escalation)`;Skip →
      SkipReason 映射。禁止 :580 构造点早赋值。
- [x] `record_tool_executed_audit` 尾参 `sandbox: Option<&str>`,payload additive。
- [x] 消费点 = 串行 `tools.rs:1786`(:1800 new_cwd 同点);L2 :407 / L3a :773 不动。
- [x] worker 例外(R7 / 5(a)):`:1782` 门加「shell 族且归因为裸跑类(非 Sandboxed)
      时破例写行」一处条件。
- [x] 单测:payload 值矩阵(经 :1786 消费侧断言);escalation 终态 = `escalation` 且
      首跑 attempt 行照写;worker 例外两臂(裸跑写 / 沙箱不写)。

### C. capability 升维(commands/config.rs)

- [x] `sandbox_capability_detail: { landlock, landlock_net, seccomp }` additive;
      `sandbox_capability: bool` 保留(= ok())。
- [x] 单测:detail 三维与 probe 一致(bool 派生关系)。

### D. 前端(audit.ts + 审计面板 + GeneralTab)

- [x] `ToolExecutedPayload.sandbox?: string`(去前缀值域 8 值)+ 人话文案映射;
      字段缺席 = 旧版本行 / 非 shell 工具。
- [x] 审计面板归因行:**异常类(failopen / escalation)高亮,常态类低亮度纯文本**
      (Yolo 下每行显示不吵)。
- [x] GeneralTab 徽标升维三维 + net 降级黄态文案。
- [x] vitest:payload 解析 / 文案映射 / 归因行两态渲染 / 徽标三态。

### E. N6 CI 门禁(tests_sandbox.rs + ci.yml)

- [x] `require_sandbox!` 宏(env `EVERLASTING_SANDBOX_TESTS_REQUIRED=1` → panic,
      **消息带三维 capability 明细**;否则 SKIP 保留);替换能力类 SKIP(779 / 1520
      行等;主机形状类不动)。
- [x] ci.yml **rust job** 设 env。
- [x] 验证:不设 env 照旧 SKIP(dev 不阻断);**panic 分支本机不可达**(probe ok,OnceLock
      不可注入)—— 由 CI runner 真实守门(runner probe 失败即触发,panic 带三维明细),
      如实记录不虚构本地验证。

### F. spec 沉淀(trellis-update-spec 阶段)

- [x] `.trellis/spec/backend/sandbox-executor.md` §5(可观测)更新:tool_executed
      归因字段 + 词表(双层类型化)/ per-attempt 与终态两行分工 / net 降级标记 /
      capability 三维 / worker 例外 / CI 门禁 env + runner 镜像变更复查 SKIP=0 条款。
- [x] BACKLOG N5/N6 行标 ✅(指向本任务);ROADMAP §1.2 补行。

## 验证命令

```bash
# 后端(WSL 需 PKG_CONFIG_PATH,AGENTS.md 坑 1)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
# 沙箱模块定向(失败先看这里)
cargo test -p everlasting --lib "sandbox::"
# 前端
cd app && pnpm test
# live 冒烟(改了 shell 工具链路;两轮:默认 + kill-switch 配方)
scripts/turn-smoke.sh                 # 抽查 tool_executed payload 带 sandboxed
# kill-switch 配方:置 sandbox_enabled=false 后再跑一轮,抽查归因值 kill_switch,
# 跑完恢复配置(sqlite3 直写前先 ./scripts/daemon.sh stop,AGENTS.md WAL 约定)
```

## 风险文件 / 回滚点

- `sandbox/mod.rs`(Policy/Decision 形变,波及 permission.rs Tier 4 短路与全部沙箱测试
  匹配——形变即清单,编译器全量兜底)。
- `tools/shell.rs`(grant-hit 匹配 + §4b 后单一出口赋值;两 spawn 路径都要)。
- `chat_loop/tools.rs:1782`(worker 例外条件——唯一行为增量点)。
- 纯 additive 无迁移;单 commit revert 回滚。

## 收尾前检查(task.py start 前完成)

- [x] PRD 收敛 pass(评审决议已折入 R1-R8 / AC1-AC11,无遗留 Open Questions)。
- [x] design.md / implement.md 用户过目(评审回填版)。
