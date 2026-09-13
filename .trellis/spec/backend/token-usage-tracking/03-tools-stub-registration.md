<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario:tools Stub 注册(D,2026-08-14)

> 配套 task `08-14-c7d-tools-stub-registration`(C7 Phase 2 之 D,触发线
> 已过:首轮 tools 占 context 38.5% > 15%)。完整契约(tool 形态 /
> `load_tool_schemas` 拦截 / 粘性 registry / 开关 / 红线)见
> [tool-contract/14-stub-registration.md](../tool-contract/14-stub-registration.md)。
> 本文只记与本 scenario(C7 度量)的交点 + 落地验证数据。

### 与 C7 度量的交点

- `turn_trace.tools_token` 度量链路**不改**:stubify 是 drive.rs 过滤链
  第 4 环(mode→workflow→session_type 之后、dispatch append 之后),
  `tools_token` 估算点在完整过滤链 + 两个 append 之后 — stub 后
  `tools[]` 体积自然缩小,tools_token 如实反映,AC1 用它验证。
- tools_token 占比如实变小是预期(前端 TracePanel 无改动)。

### 验证数据(2026-08-14)

- 基线(前):tools_token=6773 / context_input=17602 = 38.5%。
- 目标:开关开、经典 chat、首轮无 load 调用,tools_token ≤ 3700(2026-08-14 用户拍板;实测 3677,基线 6773 → 3677,省 3096,-45.7%)。
- 回滚通道:app_config `tools_stub_enabled = "false"` → 第 4 环直通 +
  不 append `load_tool_schemas`,tools_token 回 ~6773。

### 预算校准(静态度量单测,用户拍板)

静态线 ≤3700(= AC1,原设计 ≤3000 在 Edit 模式下数学不可达:核心 9
工具全量 2261 + dispatch_subagent 真实 def 984(生产 5 模型 enum,实测;
原预估 ~500 低估近半)= 3245,零 stub 已超 3000)。stub 描述走「极短
摘要 + load 指引」方案(10 个含 JSON 包装 330),Edit 合计 3675、live
实测 3677 — AC1 线随用户拍板定 3700。
