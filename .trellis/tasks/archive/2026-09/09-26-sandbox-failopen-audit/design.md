# Design — sandbox fail-open 审计可区分(09-26-sandbox-failopen-audit)

> 决议来源:2026-09-26 brainstorm 三决议 + 群聊评审(session `150e853d`,12 结论)回填。
> 关键勘误(评审第 0 位必改):shell 的 `tool_executed` 唯一串行落表点是
> `chat_loop/tools.rs:1786`——`:407` 在 L2 并行只读批(shell excluded)、`:773` 是
> L3a 并发批;早期稿写 :407/:773 属勘误,照改会「单测全绿、真链路恒 None」。

## 1. 架构与边界

```
sandbox::decide() ──Decision::Skip{reason: SkipReason}(结构化,原 &'static str)
       │                          │
       │ Sandbox(spec)            │ Skip(含 OffCause)
       ▼                          ▼
  prepare/apply(不变)      shell 工具层(durable-grant 审计照旧;net 降级标记)
       │                          │
       ▼                          ▼
  sandboxed_shell_execution   归因计算在 §4b 升级重跑 settle 后【单一出口】:
  审计行(per-attempt;          SandboxAttribution(扁平 enum)
  ruleset 摘要+net-degraded)      │
       │                          ▼
       │                ToolContextUpdate.sandbox_attribution(串行路径现成载体:
       │                :1800 就在此消费 new_cwd)
       ▼                          ▼
                     agent loop 串行落表点 chat_loop/tools.rs:1786
                     record_tool_executed_audit → payload "sandbox": <值>(shell 族必写)
                     worker 例外(R7):裸跑归因时破例写行(绕过 skip_persist 一处条件)
```

- **不改**:fail-open 降级行为本身、`SandboxedShellExecution` kind、§2.2「skips are not
  security events」(不新增审计行,只增强既有行的 payload)。
- **两行分工(评审结论 4)**:`sandboxed_shell_execution` = **per-attempt** 行(每次沙箱
  spawn 成功一条,含升级重跑前的失败首跑);`tool_executed.sandbox` = **终态**归因(最终
  实际以何种 sandbox 状态跑完)。escalation 场景:首跑 attempt 行照写 + 终态行
  `escalation`——交叉印证链因此不再说谎。

## 2. 数据流与契约

### 2.1 SkipReason / OffCause(sandbox/mod.rs)

```rust
pub enum OffCause { FailOpen, Yolo, KillSwitch, ProjectOff, NoSession }
pub enum Policy {
    Off { cause: OffCause },          // 原 unit Off;全仓 25 处匹配改 Off { .. }
    Face(Face),
}
pub enum SkipReason { PolicyOff(OffCause), DurableGrant }
pub enum Decision {
    Sandbox(SandboxSpec),
    Skip { reason: SkipReason },       // 原 &'static str
}
```

- `resolve_policy` / `resolve_session_policy` 六支求值各自带 cause(求值序零改动,§1 精神)。
- `DURABLE_GRANT_SKIP_REASON` 常量退役,shell.rs grant-hit 匹配改 `SkipReason::DurableGrant`
  (grant-hit 审计行为逐字节不变)。
- fail-open 的进程级一次性 `tracing::info!` 保留(启动可见性)。
- `permission.rs:357` 的 `resolve_session_policy(..) != Policy::Off` 比较式改
  `!matches!(.., Policy::Off { .. })`(形变即清单,编译器兜底)。

### 2.2 SandboxAttribution 词表(双层类型化,评审结论 3)

**payload 扁平 enum——与 OffCause 不同型**(grant / escalation 不经 `Policy::Off`),
`From<OffCause>` 提供降维转换:

```rust
// sandbox/mod.rs(或 tools/mod.rs,落点以最小依赖定)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxAttribution {   // wire = snake_case 变体名
    Sandboxed,                  // 沙箱执行(终态)
    FailOpen,                   // probe 失败裸跑
    Yolo, KillSwitch, ProjectOff, NoSession,   // 有意裸跑(From<OffCause>)
    Grant,                      // durable prefix-grant 直跑
    Escalation,                 // §4b 用户批准的 unsandboxed 重跑(终态)
}
```

wire 值(去前缀,评审结论 11):`sandboxed | failopen | yolo | kill_switch | project_off |
no_session | grant | escalation`。**shell 族必写**(显式 `sandboxed` 消灭「缺席=沙箱执行」
default-safe 反模式,评审结论 6);非 shell 工具字段缺席(值域不适用)。

### 2.3 通道:ToolContextUpdate.sandbox_attribution(tools/mod.rs → shell.rs → tools.rs:1786)

```rust
pub struct ToolContextUpdate {
    pub new_cwd: Option<PathBuf>,
    pub sandbox_attribution: Option<SandboxAttribution>,  // None = 非 shell 族工具
}
```

- **赋值点 = shell.rs §4b 后单一出口**(评审结论 4/10):spawn + 至多一次升级重跑都
  settle 后才计算——Sandbox 路径无重跑 → `Some(Sandboxed)`;重跑发生 →
  `Some(Escalation)`;Skip 路径 → `From(SkipReason)` 映射。禁止在 :580 构造点早赋值
  (陈旧归因正是评审实锤的缺陷)。
- shell / run_background_shell 两工具同款;`record_tool_executed_audit` 加尾参
  `sandbox: Option<&str>`(payload additive)。
- **消费点 = 串行 `tools.rs:1786`**(:1800 同点消费 new_cwd,通道顺路);L2 :407 /
  L3a :773 两处调用点是只读并发批(shell excluded),透传恒 None,不消费不填。
- worker 例外(R7,5(a)):`:1782` 的 `!skip_persist` 门加一条——shell 族且归因为
  **裸跑类**(非 `Sandboxed`)时破例写行。worker 审计面对称:沙箱跑→工具层
  `sandboxed_shell_execution` 行(本就不受门控);裸跑→本例外行。

### 2.4 net 降级标记(sandboxed_shell_execution 行内)

- `build_spec` 发生 BindOnly→Block 降级(`!cap.landlock_net` 且配置 BindOnly)时,
  `SandboxSpec` 摘要追加 `net-degraded=bindonly→block` 段;降级 `tracing::warn!` 保留。
- 不加新字段不翻 kind——归因消费点 = 审计行 ruleset 摘要 + 设置面徽标(§2.5)。

### 2.5 capability 升维(commands/config.rs + GeneralTab)

- `get_app_config` additive:`sandbox_capability: bool` **保留**(= `cap.ok()`,旧消费方
  零破坏)+ 新增 `sandbox_capability_detail: { landlock, landlock_net, seccomp }`
  (只读派生,不落盘,同款注释口径)。
- GeneralTab 徽标升维:全绿 = 沙盒生效;landlock_net 缺 = 「BindOnly 档将降级断网」黄;
  landlock/seccomp 缺 = 「已回退(fail-open)」红(现文案沿用)。

### 2.6 N6 CI 门禁(tests_sandbox.rs + ci.yml)

- `require_sandbox!` 宏:probe `!ok()` 时,若 env
  `EVERLASTING_SANDBOX_TESTS_REQUIRED=1` → panic(**消息带三维 capability 明细**——
  runner 镜像变更导致门禁死亡时可归因);否则 eprintln SKIP 返回(本地 dev 不阻断)。
- 替换能力类 SKIP(779 / 1520 行等;`$HOME` / `/init` / `/mnt/c` 类主机形状 SKIP 不动)。
- ci.yml **rust job**(ubuntu-latest,`cargo test --lib`)设该 env;非 Linux
  `#[cfg(target_os="linux")]` 已存在,不动。
- spec §5 运营条款:runner 镜像变更后复查 SKIP 计数=0。

## 3. 兼容与迁移

- 审计 payload 对 shell 族从无字段 → 必写;旧行无字段 = 历史语义,前端按「字段缺席 =
  旧版本行」渲染(不回填)。非 shell 工具恒缺席。
- `Policy::Off` 形变是 crate 内部 API,wire 面零变化;`ToolContextUpdate` 加字段 derive
  Default,非 shell 工具构造点零改动。
- worker 例外是行为增量(worker 裸跑从无行 → 有行),B6 PR1b 的主裁剪(transcript 为主
  记录)不变。

## 4. 权衡记录

- **不用新 kind 审计行**(评审结论 2 维持):kindFilter 是 kind 级服务端筛选不受 payload
  污染;Yolo 高频裸跑下新 kind 行数翻倍的噪音论据成立;fail-open 是进程级不变量,
  机器级状态由设置面徽标承担,两层正交。
- **显式 `sandboxed` 值**(评审结论 6 首选):消灭缺席语义的 default-safe 风险(未来
  writer 忘设字段 = 行自称沙箱执行)。
- **归因单一出口在 §4b 后**(评审结论 4):escalation 终态语义只有重跑 settle 后才确定;
  早赋值 = 陈旧归因,正是本任务要消灭的缺陷类别。
- **worker 走 5(a) 反转例外**(用户裁定):裸跑归因是安全语义,与 B6「transcript 为
  worker 执行记录」的裁剪初衷不冲突;增量一处条件。
- **L3a/L2 并发批不消费不填**:read-only by construction,shell 被排除,填了也是
  永远 None 的死代码。

## 5. 回滚

- 纯 additive(payload 字段、config 字段、worker 一处条件例外;skip 结构化是内部 API):
  单 commit revert 即回滚,无 DB 迁移无 wire 破坏。
