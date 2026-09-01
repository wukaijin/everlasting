# Design — A2+ P3d 后台 shell 升级闭环(B 案:下轮注入时弹卡)

> 前置:prd.md(用户 2026-09-01 裁定 B 案)。上游机制:P3c §5 升级闭环
> (`.trellis/spec/backend/sandbox-executor.md` §10)。

## 0. 数据流总览

```
run_background_shell 调用(第 T 轮)
  └─ dispatch 点(tools.rs)per-call 构造 tool_ctx:
       tool_ctx.tool_use_id = Some(id)          ← 新 ToolContext 字段(全工具注入)
  └─ execute():
       sandbox::decide → Some(spec) / Skip
       registry.start(..., sandbox, origin_tool_use_id = ctx.tool_use_id)

registry 等待任务(命令退出时,持有全量 stderr)
  └─ trigger==Normal ∧ outcome==Failed ∧ sandboxed ∧ origin 有
     ∧ classify_block(stderr) 命中
       → notification.escalation = Some(EscalationOffer {
             tool_use_id, block: Write|Network, stderr_evidence })
       (否则 escalation = None,现状路径)

第 T+k 轮开始(drive.rs drain 之后、组装 turn_messages 之前)
  └─ background_escalation::resolve(notifications, env):
       对每个带 offer 的通知(顺序执行):
         门:mode != Plan ∧ escalation_source() 可查 ∧ token 未取消
         prefix_grant_hit(db, sid, command)
           ├─ 命中 → 不沙盒重跑(零卡)+ audit_grant_rerun
           └─ 未命中 → EscalationHandle.ask(tool_name="run_background_shell",
                        input={"command"}, ...) 弹卡(挂原 bsh 卡,120s)
                          ├─ Approved → 不沙盒重跑
                          └─ Denied/超时/取消 → 不重跑
       每个通知产出恰好一条终态注入文本(见 §3)
```

## 1. 载体改动

### 1.1 `ToolContext.tool_use_id: Option<String>`(新字段)

- dispatch 点(tools.rs serial loop,现有 `name == "shell"` 注入点同处)对
  **所有工具**统一 `tool_ctx.tool_use_id = Some(id.clone())`——无条件注入,
  只有 `run_background_shell` 消费;其余工具零行为变化。
- E0063 引导补全 ~30-55 处 struct literal(测试为主),P3c 加 `escalation`
  字段同款做法;`init.rs` 构造点(无 per-call id)填 `None`。
- 不选 registry 事后 `set_origin`:echo 类命令毫秒级完成,先 start 后注册
  有丢载竞争;不选 execute 签名扩展:波及全部工具。

### 1.2 `BackgroundShellRegistry::start` 尾参 `origin_tool_use_id: Option<String>`

- 唯一实现方 in_memory;调用方 = run_background_shell.rs + 既有测试
  (in_memory tests 3 处 + tests_sandbox 后台面测试),编译器引导。
- entry 新字段 `origin_tool_use_id` 保留;`run_background_task` 增参
  `sandboxed: bool` + `origin_tool_use_id: Option<String>`(start 时已知,
  无需新查询)。

### 1.3 通知载荷 `BackgroundShellNotification.escalation: Option<EscalationOffer>`

```rust
pub struct EscalationOffer {
    pub tool_use_id: String,        // 原 run_background_shell 调用 id(卡挂回)
    pub block: EscalationBlock,     // Write | Network(serde snake_case)
    pub stderr_evidence: String,    // 复用 escalation::stderr_evidence_line(改 pub(crate))
}
```

- 生成条件(等待任务内,拥有全量 stderr):`trigger == Normal` ∧
  `outcome == Failed` ∧ `sandboxed` ∧ `origin_tool_use_id.is_some()` ∧
  `classify_block(stderr)` 命中。Killed / TimedOut / SpawnFailed / Skip
  / 成功 → `None`(AC5:文本逐字节现状;serde 结构 additive 不破坏 wire)。
- 通知保持精瘦:command / cwd / max_runtime **不进通知**,经新 inherent
  getter 查询(entry 既有预留字段 `command` / `cwd` / `max_runtime_ms`)。

### 1.4 `InMemoryBackgroundShellRegistry::escalation_source`(inherent,非 trait)

```rust
pub async fn escalation_source(&self, session_id, shell_id)
    -> Option<EscalationSource { command, cwd, max_runtime_ms }>
```

- inherent 方法零 trait 波及(drive 侧持有具体类型 `DefaultRegistry`);
  drain 与查询间条目被 sweep → `None` → 降级普通失败文本(R2.1)。

### 1.5 `escalation.rs` 参数化

- `EscalationHandle::ask` 增首参 `tool_name: &str`(前台传 `"shell"` 现状
  不变);`audit_grant_rerun` 同增;`stderr_evidence_line` 改 `pub(crate)`
  供等待任务复用。ask 侧 grant 经 ask_path 既有通道落库
  (tool_name=`run_background_shell`,ToolKind::Shell → prefix 合法,
  RULE-SMOKE-001 矩阵天然通过;读侧 `IN ('shell','run_background_shell')`
  本就跨工具,前台批的后台 grant 命中是有意语义)。

## 2. 注入点闭环(`agent/chat_loop/background_escalation.rs` 新模块)

`resolve(notifications, env) -> Vec<String>`,env = { registry, sink,
permission_asks, permission_ctx, db, token, mode }(drive_turn 全部现成:
deps.background_shells / deps.permission_asks / carry.permission_ctx /
deps.token / current_ctx.mode / sink / db / session_id)。

对每个通知(顺序;多offer串行弹卡,v1 不设上限):

1. `escalation = None` → 现状格式串,**逐字节不动**(AC5,格式字面量留在
   drive.rs 或原样搬迁,以测试钉死)。
2. 有 offer:
   - 门 a:`mode == Plan` → 终态文本 = 现状格式 + `\n` +
     `sandbox::failure_guidance(evidence, Plan)`(设计使然指引)。不弹卡。
   - 门 b:`escalation_source()` None → 现状格式(降级)。
   - prefix_grant_hit → `registry.start(sid, command, cwd, max_runtime,
     None, None)`(重跑 origin=None:重跑自身不再升级,结构性一次性);
     `audit_grant_rerun(json!({"command"}), "run_background_shell")`。
     start Err → 见 5。
   - ask:`EscalationHandle::new(sink, store, ctx, db, token,
     offer.tool_use_id)` → `.ask("run_background_shell",
     json!({"command"}), &command, block→SandboxBlockKind, evidence)`。
     120s / 取消 / 拒绝 → Denied。Approved → 重跑(同上)。
3. 终态文本(§3)。
4. 重跑成功 → 新 `bsh_*` 自行走完整生命周期(完成时再注入一条现状格式
   通知,sandboxed=false 无 offer)。
5. 重跑 `start` Err(spawn fail / cwd 失踪)→ 按 denied 处理 + 追加
   `\n[escalation] 重跑启动失败: {err}`。

## 3. 注入文本(全部为 user-role 单条,现有格式为基准)

- 现状(无 offer / 降级):`[system] 后台 shell {id} 已完成,exit code {N}。
  调 shell_status(session_id="{id}") 看输出。` ——不动。
- 批准(ask):`[system] 后台 shell {old} 因沙盒拦截({写面外|断网})失败,
  exit code {N}。已经用户批准,同一命令不沙盒重跑 → {new}。完成后将另行
  注入通知;可 shell_status(session_id="{new}") 查询。`
- 批准(grant-hit):同上,「已经用户批准」→「依既有『总是允许』授权」。
- 拒绝/超时/取消:现状格式 + `\n[escalation] 已向用户请求不沙盒重跑,
  未获批准(拒绝/超时)。` + `\n` + `failure_guidance(evidence, mode)`。
- Plan:现状格式 + `\n` + `failure_guidance(evidence, Plan)`。
- 文案细节(标点/英文词)实现期定稿,原则:LLM 可直接行动、不含内部路径、
  与 shell_status 指引格式一致。

## 4. 不变量与边界

- **Plan 绝不放行**(D3):门在 ask 时求值(当轮 mode = current_ctx.mode,
  与前台 ToolContext.mode 同源)。Plan 中启动(RO 面)的 shell,若下轮已
  切 Edit,升级合法——审批卡本身即用户同意面。
- **重跑不再升级**:重跑 sandbox=None → 等待任务 `sandboxed=false` → 无
  offer;即使再来一轮 drain 也走现状路径。
- **审计零新 kind**:ask 侧既有 kinds;grant-hit ToolAllowed(带 reason,
  与前台同形);重跑无 `sandboxed_shell_execution` 行(不沙盒)。
- **一次性语义**:每个通知恰好产一条文本;升级不产生额外轮次/消息。
- **worker**:后台 shell session-scoped(Q7),升级发生在持有 session 的
  turn 内;EscalationHandle 的 worker 兼容性 P3c 已验证,免费成立。
- **across-turn 时序**:升级只可能发生在 drain 时刻(下一轮),不引入
  detached 任务;turn token 取消 → ask 立即 Denied → 终态文本照常注入?
  ——不:token 取消意味着整个 turn 在放弃,注入与否由既有 turn 终止路径
  决定,升级函数只需保证 ask 短路返回,不吞取消错误。

## 5. 测试计划(锚点)

- `background_shell` 单测:offer 生成矩阵(trigger × outcome × sandboxed ×
  origin × classify);None 路径 serde 现状(AC5);escalation_source 查询
  (含 sweep 后 None)。
- `chat_loop` 集成(仿 `tests_escalation.rs` 四路 + share-sink gotcha:
  handle 的 sink 与测试捕获必须同一 Arc):approve 弹卡重跑(AC1)/ deny
  终态文本 + 指引(AC2)/ grant-hit 零卡 + ToolAllowed 审计 + 复合不享
  grant(AC3)/ Plan 门(AC4)/ 重跑 start Err 降级。
- 回归锚:`tests_sandbox::sandboxed_background_shell_enforces_write_face`
  及 L1 既有通知格式测试逐字节不动(AC5)。
- live:`scripts/turn-smoke.sh` 手工冒烟(沙盒开启 + 后台写面外命令)。

## 6. 不做(重申 prd Non-goals 的实现含义)

- 不加 ChatEvent / 前端改动;不加超时提示;不做完成即弹卡;不动
  `filter_tools_for_mode`(run_background_shell 在 Plan 已暴露,PR2 遗产)。
