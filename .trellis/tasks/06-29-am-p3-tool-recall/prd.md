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

---

## 代码现状(2026-06-29 落地后)

- `app/src-tauri/src/agent/permissions/check.rs` — 新增 `recall_pitfall_footnote(pool, tool_name, tool_input) -> Result<Option<String>, sqlx::Error>` + 私有 `extract_probe_args`(按 tool kind 选 path / command / url 探针)
- `app/src-tauri/src/agent/permissions/mod.rs` — re-export `recall_pitfall_footnote`
- `app/src-tauri/src/agent/chat_loop.rs` — 两处挂载:parallel-batch L2 path(≈line 1792) + serial path(≈line 2361),均位于 `permissions::check` 返回 Allow 之后、`execute_tool` 之前
- `app/src-tauri/src/agent/permissions/tests_check.rs` — 6 个 P3 test + `make_pool` helper(test 隔离)
- 1028 cargo test 全绿(基线 1022 + P3 新增 6)
- frontend 不动(P3 纯后端)

## 已定决策(P3 实施时锁档)

1. **挂在 `chat_loop` seam 而非 `permissions::check()` 内部** — 5-tier 拦截链纯净性 + P5 verified 软拦截需要从 check() 内部走(结构化 Decision),P3 active-only footnote 是旁路 hint 放 seam 简化 P5 落地
2. **active-only filter** — `find_pitfalls_by_trigger` SQL 返回 active + verified,但 P3 严格过滤 `status == 'active'`;verified 软拦截是 P5 范围(已在 spike-007 §4 命中分档表定档)
3. **`bump_hit_count` 走 `tokio::spawn` fire-and-forget** — 不阻塞 recall 步骤,匹配项目 audit-write 模式(非阻塞 metadata 更新);P5 状态机读 `hit_count` 决定晋升
4. **不阻断工具执行** — `Err(sqlx::Error)` → `tracing::warn!` + 返回 `Ok(None)`,工具照常执行(PRD hard rule);`Decision::Allow` 不变
5. **注脚 prepend 到 `tool_result.content` 在 envelope wrap 之前** — `tool_use_id` 配对 / `is_error` 语义 / envelope `{result, cwd}` shape 全部不变;前端 `extractToolResultDisplay` 兼容(plain text 在 result 字段内)

## Open Questions(原 prd "实施时定" 项)— 全部已解决

- ~~Tier1 挂载:怎么把"召回结果"传到 tool_result 注脚~~ → **挂 `chat_loop` seam,recall 返回 `Result<Option<String>>`,prepend 到 `content` 字符串**;不进 `check()` Decision 链
- ~~trigger_key 匹配规则:command_pattern 字符串包含? glob?~~ → **`command_pattern` 精确匹配 + `path` 精确匹配,走 `idx_am_pitfall` 索引,O(1)**;P1 `find_pitfalls_by_trigger` 已留接口,签名对齐
- ~~注脚注入位置:tool_result content 前缀 vs 单独块~~ → **plain text 前缀(不破坏 content 协议 + envelope 兼容)**;多命中用 `\n• [title] content` 多行 bullets

## Implementation Plan(实际 1 PR,1 个 spec sync commit,1 个 落地 commit)

- **PR1 work**:`feat(memory): P3 工具执行前召回 — Tier1 hook seam + active 注脚`(permissions/check.rs + permissions/mod.rs + permissions/tests_check.rs + chat_loop.rs 共 +488/-1)
- **PR1 docs**:`docs(spec): P3 自主记忆同步`(permission-layer.md §4.2 + memory.md §Scenario 2 P3 + tool-contract.md footnote + agent-loop-architecture.md seam,共 +131)
- **PR1 落地**:本 section + `task.json` status→completed(由本 commit 收尾)
- archive + journal:由 `task.py archive` 自动生成

## P3 → P4/P5 衔接

- **P4**(事件驱动自动写入)消费 P3 的 `trigger_key` 写入路径(走 `remember` tool,P3 召回的 pitfall 写入后立刻可被消费)
- **P5**(质量层)消费 P3 的 `active` 状态机 — verified 软拦截从 `check()` Tier 1 内部加,P3 的 `recall_pitfall_footnote` 函数签名 `Result<Option<String>, _>` 留 P5 扩展位
