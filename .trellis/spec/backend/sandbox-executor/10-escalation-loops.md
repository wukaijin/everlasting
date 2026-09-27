<!-- Moved from sandbox-executor.md 2026-09-28 (doc-split): §10+§11 -->

# Sandbox 升级闭环 — 前台(P3c)+ 后台(P3d)

> hub:[sandbox-executor.md](../sandbox-executor.md)(§1-§9 核心契约)。
> sibling 分篇:[12-listen-gap-remediation](./12-listen-gap-remediation.md)(识别缺口与长期方案)/ [13-net-policy-bindonly](./13-net-policy-bindonly.md)(网络三态)/ [14-durable-prefix-grant](./14-durable-prefix-grant.md)(项目级前缀授权)。

## 10. P3c — 升级闭环(前台 shell,design §5)

- **触发**(§5.1):`sandbox_applied ∧ exit≠0 ∧ mode≠Plan ∧ classify_block`
  命中(写串先行,`Operation not permitted` = 断网;2026-09-21 起识别输入
  扩到 stdout,见 [12-listen-gap-remediation](./12-listen-gap-remediation.md))。**每 tool call 至多
  一次**(重跑在结构上不再进升级分支);Plan 排除 = D3 确定性只读身份。
- **流程**(§5.2):prefix-grant 先查(`escalation::prefix_grant_hit`,
  `has_structural_metachar` 复合闸同 Tier 4)→ 命中直接不沙盒重跑零弹卡;
  未命中 → `EscalationHandle::ask` 复用 `ask_path` 弹卡(`reason_override`
  新参:拦截原因 + 原命令 + stderr 证据行)→ AllowOnce/AllowAlways(grant
  经 ask_path 既有通道落库,kind↔类别矩阵天然合法)→ **逐字节同
  command/env/cwd 重跑**(RULE-E-001/002 不变,仅无 pre_exec)/ Deny →
  原失败 + 模式感知指引。
- **注入**:EscalationHandle(sink+store+PermissionContext+db+token+
  tool_use_id)由 serial dispatch **仅对 shell** 灌入(shell 永不进并行批;
  后台壳维持模型介导);`Default`(None)= 测试路径 → 退化为指引。
- **双执行边界**(D4 接受):升级仅在面外写/断网被拒后触发 —— 危险部分
  第一遍未发生;重跑失败按普通失败返回。审计零新 kind:ask 侧既有 kinds +
  首个 `sandboxed_shell_execution` 行 + `tool_executed` 终态。
- worker 路径免费成立(ask_path 的 worker store keying / transcript-only
  审计原样复用)。

## 11. P3d — 后台 shell 升级闭环(下轮注入时,B 案,2026-09-01)

- **载荷生成**(registry 等待任务,拥有全量 stderr):`trigger == Normal ∧
  outcome == Failed ∧ 沙盒启动 ∧ origin_tool_use_id 有 ∧ classify_block`
  命中(2026-09-21 起识别输入扩到 stdout,证据行 stderr 优先、stdout
  兜底,见 [12-listen-gap-remediation](./12-listen-gap-remediation.md))→ 通知带 `EscalationOffer { tool_use_id, block, stderr_evidence }`。
  Killed / TimedOut / SpawnFailed / Skip / 成功恒 `None`(超时 kill 的部分
  stderr 不可信,与前台 `!timed_out` 同门)。**start() 折叠**:`sandboxed`
  与 origin 在 start 时折叠成单一 `escalation_origin`——非沙盒 shell 的
  origin 是死重。origin 来源 = `ToolContext.tool_use_id`(P3d 新字段,
  dispatch 对**所有工具**统一盖章,仅 run_background_shell 消费;不选
  registry 事后注册——echo 类毫秒级完成,先 start 后注册有丢载竞争)。
- **解析时机 = 下轮 drain 之后、组装 turn_messages 之前**
  (`chat_loop/background_escalation.rs::resolve_all`;呈递方案用户裁定
  B 案,"完成即弹卡"的 A 案被否:detached ask 生命周期 / 失败通知抑制 /
  前端 120s 计时三座山的成本不值)。此时用户刚发消息必然在场,前台 120s
  超时原样复用;turn token 天然 cancel-safe,零 detached 生命周期。
- **流程**(复用前台 §5.2 原语,`ask`/`audit_grant_rerun` 已参数化
  tool_name=`run_background_shell`):Plan 门(当轮 mode,与前台同源;Plan
  中启动、下轮已切 Edit 的升级合法——审批卡即用户同意面)→
  `escalation_source()` 查重跑输入(entry 既有预留字段 command/cwd/
  max_runtime,inherent getter 不进 trait)→ prefix-grant 先查(grant
  命名空间跨 shell 族共享是有意语义,读侧 `IN ('shell',
  'run_background_shell')`)→ Ask 卡(挂**原调用卡**,前端按 toolUseId
  匹配)→ 批准 → registry `start(sandbox=None, origin=None)` 一次性不沙盒
  重跑(重跑结构性不再升级)。
- **注入文本契约**:每个通知恰好产一条终态文本——无 offer 走 legacy 格式
  (`plain_text` 钉死逐字节,AC5 回归锚);批准(ask/grant 两分支措辞
  区分)/ 拒绝 / 重跑 spawn 失败如实上报 / entry 已 sweep 降级 plain。
  LLM 永远只看到连贯故事,不在升级悬而未决时行动,也不自己发起重跑
  (审批绑定确切命令文本,D4 同源)。
- **前端唯一改动**(推翻 PRD 初稿"零改动"假设):ShellCard
  `isPendingApproval` 的前台 `!hasResult` 守卫对后台不适用——升级卡挂在
  **已有结果**的 run_background_shell 卡上;前台 ask 先于执行的
  hide-on-result 语义保留,后台卡按 `isBackground` 豁免(vitest 双向
  回归锚)。
- **测试基建 gotcha(P3d 新增)**:ask.rs 先 `emit_permission_ask` 后
  `register_ask`——mock resolver 首轮 resolve 可能合法落空
  (`resolve_ask` 返回 false),必须重试到成功,否则并行负载下 120s 超时
  赢竞态(tests_escalation 的 approve 例曾因此 flaky)。
- **环境边界(本仓库首例)**:`tests_escalation::escalation_approve_*`
  的探针前提是"OS 拒 `/proc/1/mem`",root 下不成立(rerun exit 0)——
  `geteuid()==0` 大声 SKIP,循内核探测 SKIP 同款纪律。
