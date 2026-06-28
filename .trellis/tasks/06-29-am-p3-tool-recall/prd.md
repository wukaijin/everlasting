# P3: 工具执行前召回 — trigger_key 精确匹配 + active 注脚

> child `06-29-am-p3-tool-recall` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §4 层2](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 前置:P1(消费 `find_pitfalls_by_trigger`)。本阶段做 **active 注脚档**(零 loop 改动);**verified 软拦截重判档**因涉及 loop 改动(实现细节),按"实现细节延后"原则放 P5。

## Goal

agent 执行工具(尤其 shell/edit/grep)前,用当前 `tool_name + tool_input` 精确匹配 pitfall 的 `trigger_key`,命中按 status 分档:
- **active(本 task)**:不阻断,把 pitfall 作为 tool_result 前置注脚回填("⚠️ 此前在本项目执行类似操作时踩过坑:…")
- verified 软拦截重判 → P5

这是兑现"第一时间规避"的核心机制(另两份 spike 都没到"工具执行那一刻")。

## In Scope

- 接入点 B:`permissions/check.rs` Tier1 Hooks(当前 no-op)挂"工具执行前召回",复用现有 5-tier 拦截链,不另起炉灶
- 调 `find_pitfalls_by_trigger(pool, tool_name, command, path)` 精确匹配
- active 命中:构造注脚文本,作为 tool_result 前置内容回填(零 loop 结构改动)
- pitfall 注入措辞:imperative 强提示("执行 X 前先 Y"),紧贴 tool_use context

## Acceptance Criteria

- [ ] 手写/P4 产出一条 pitfall(trigger_key={tool:shell, command_pattern:"cargo test"})
- [ ] agent 跑 `cargo test` → 工具执行前命中 → tool_result 注脚回填可见
- [ ] 不误命中:无关命令不触发注脚
- [ ] 不阻断工具执行(注脚只是提示,Decision 仍 Allow)
- [ ] cargo test 全绿

## Technical Approach(方向,实现细节实施时定)

- Tier1 挂载:怎么把"召回结果"传到 tool_result 注脚(注入 ToolContext? check 返回附带 hint?)——实施时定
- trigger_key 匹配规则:command_pattern 字符串包含? glob? ——实施时定(P1 `find_pitfalls_by_trigger` 已留接口)
- 注脚注入位置:tool_result content 前缀 vs 单独块——实施时定

## Out of Scope

- verified 软拦截重判(动 loop 结构,→ P5)
- 事件驱动自动写入 pitfall(→ P4;P3 只消费已存在的 pitfall)
- 状态机晋升(→ P5)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §4 层2 + §6 接入点 B
