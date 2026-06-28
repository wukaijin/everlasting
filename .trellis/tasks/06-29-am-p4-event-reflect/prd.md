# P4: 事件驱动自动写入 — 连续失败→成功 旁路 reflection

> child `06-29-am-p4-event-reflect` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §3 路径2 + §6 接入点 C](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 前置:P1(消费 `insert_memory`)。**自动闭环**:失败经验自动变 pitfall,不再依赖 agent 手动 remember。

## Goal

检测"连续 ≥2 次同名工具失败后成功"信号 → 触发**旁路 LLM reflection**(专门 prompt)→ 提炼一条带 `trigger_key` 的 pitfall → `insert_memory(status=active)`(事件天然高置信,直接 active,不经 candidate)。

配合 P3 的召回构成完整自动闭环(踩坑→记住→下次规避)。

## In Scope

- 接入点 C:`chat_loop.rs:1717 emit_tool_result` 旁路挂 in-session 状态机:记录 `(tool_name → 连续失败计数)`,连续失败 ≥2 后这次成功 → 触发 reflection
- 旁路 reflection:fire-and-forget 异步,不阻塞主 loop;调 provider 用专门 prompt(从失败+成功的 transcript 片段提炼:一句坑描述 + trigger_key + tags)
- 写入:`insert_memory(kind=pitfall, status=active, trigger_key=..., source_ref=tool_call_id)`,过 P1 安全网

## Acceptance Criteria

- [ ] 制造连续 2 次 shell 失败后成功 → 自动产出一条 pitfall(active,带 trigger_key)
- [ ] reflection 异步,不阻塞主 loop(主 loop 时延无明显增加)
- [ ] 单次失败不触发(需连续 ≥2)
- [ ] 产出的 pitfall 能被 P3 的工具执行前召回命中
- [ ] cargo test 全绿

## Technical Approach(方向,实现细节实施时定)

- 旁路 reflection 的 provider/cancel 生命周期:fire-and-forget 失败如何处理(丢掉 vs 重试)、与主 loop cancel 的关系——实施时定
- reflection prompt 模板:怎么从 transcript 片段提炼结构化 trigger_key(LLM 产出 JSON)——实施时定
- 状态机的 session 维度存储:内存 map 还是 DB——实施时定

## Out of Scope

- session 结束整体 reflection(v2)
- verified 软拦截(→ P5)
- 状态机晋升(→ P5;P4 写入直接 active)
- "用户纠正"等其他事件信号(留扩展,本 task 只做连续失败→成功)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3 路径2 + §6 接入点 C
