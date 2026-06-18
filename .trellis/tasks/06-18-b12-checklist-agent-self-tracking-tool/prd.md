# B12 Checklist(agent 自跟踪进度清单 — TodoWrite 式 `update_checklist` tool)

## Goal

给 agent 一个**自跟踪进度**的原语:LLM 跑复杂多步任务时维护一份结构化 checklist,边做边勾掉/加项,每轮把自己的计划重新注入 context,从而不丢失进度。对齐 Claude Code `TaskCreate/TaskList` / opencode `todowrite` / Cline plan-act。

定位:ROADMAP §2 🟠 第三档 **B12**,**先于 B6 Subagent 做**——它是 subagent 那套"每轮注入动态 agent-state"机制的**小面 warm-up**(注入列表是平凡实例,subagent 注入子 summary + 子预算是复杂实例)。

## What I already know

- **术语已落**:[`docs/CONTEXT.md`](../../../docs/CONTEXT.md) "Checklist (agent 自跟踪清单)" 段(2026-06-18),含三消歧义:**非** Trellis task(`.trellis/tasks/`)、**非** plan mode(`Mode::Plan`)、**非** subagent(B6)。
- **决策已落**:[`docs/IMPLEMENTATION.md §4` 2026-06-18 B12 ADR](../../../docs/IMPLEMENTATION.md#4-决策日志) —— 三条核心权衡 + 5 条被否决方案 + 排序理由。本 PRD 不重复权衡,只引用结论。
- **注入机制原型**:B5 Memory(`build_instructions_blocks` + `cache_control: ephemeral`,一次性头部插入,run 内静态)。Checklist **不同**:run 内动态 → 必须**每轮 ephemeral 重发**,不能照抄一次性插入。
- **C3 compaction 是 in-memory only**(`agent/context.rs:36`):DB 保留全部 message,reload 还原永远完整。这是"无新 DB 表"成立的根基。
- **RULE-A-004**:cancel 掉的 tool 不 commit tool_result。同一套保护罩住 checklist(cancel 的 update 不算数)。
- **DEBT**:实施前查 `.trellis/reviews/DEBT.md`,无新 P0/P1 阻塞再开工。

## Decisions (grill-with-docs 已收敛)

1. **✅ 概念 = TodoWrite 式 agent 自跟踪 tool**,命名 **Checklist**(见 CONTEXT.md 消歧义)。
2. **✅ 生命周期 = per-request**:挂在 agent loop 一次 run 上(一个 user message → 最多 50 turn)。新 user message → 新清单;D3 重发 fork 新 run → 全新空清单。
3. **✅ state + 注入 + 持久化 = loop-local `Vec` + 每轮 ephemeral 重发 + 无新 DB 表**:
   - state:`run_chat_loop` 作用域内 `Vec<ChecklistItem>`,不跨 run → **零 DB schema 变更、无 migration**。
   - 注入:每轮 `provider.send` 前,从 Vec 重建一份 synthetic user block(整张 list + 显式 in_progress 焦点),prepend 到**请求副本**(不写回持久化 messages),发完即弃。**不打 `cache_control`**(每轮必变);**不塞 system prompt**(会 bust system prompt cache)。空表跳过。
   - replay:`update_checklist` tool_result(本就在 history 持久化,带全表)→ 渲染 + reload 的 source of truth。reload 从 DB 全量 history 重建(C3 不碰 DB,永远完整)。
4. **✅ tool 形状 = 单 `update_checklist` 全量替换 + 三态 + 至多一 in_progress**:`{content, status}`,status ∈ `pending`/`in_progress`/`done`。全量替换(非细粒度)是 replay 的硬要求(最后一条 tool_result == 当前态)。至多一 `in_progress` 为 soft 约束,model 传多个时 coerce(保留最后一个、其余降 pending),不报错。
5. **✅ cancel / 切 session**:复用现有 cancel 路径 + RULE-A-004 → live Vec 与持久化 history 不打架;切回从 DB history 重建。
6. **✅ 前端 = 单张 live 浮层卡**:ChatPanel 内 `position: absolute` 覆盖层(不进消息流、不随滚动),可最小化为悬浮球,焦点(in_progress)项动效。从 run 内最新 `update_checklist` tool_result 派生当前态;抑制 `update_checklist` 的 per-call tool 卡渲染。

> 被 QT 的方案(细粒度 tool / 新 DB 表 / 塞 system prompt / 纯靠 history 无 ephemeral / 只发焦点)权衡详见 §4 ADR "Alternatives considered & rejected"。

## Requirements

- `update_checklist` 虚拟 tool 注册进 tool list,模型可自发调用(对齐 `use_skill` 注册模式)
- tool 入参 = 完整 items 数组(`[{content, status}, ...]`);执行后**原子替换** loop-local Vec,返回完整列表作 tool_result
- 至多一 `in_progress` coerce:model 传多个时保留最后一个、其余降 `pending`(不报错,不打断 loop)
- 每轮 agent loop:`compact_messages` 后、`provider.send` 前,从 Vec 重建 ephemeral synthetic user block prepend 到请求副本;Vec 空(turn 1 未调过 update)时跳过注入
- 前端 `<ChecklistCard>`:ChatPanel 浮层,展开(完整 checklist + 焦点动效)⇄ 最小化悬浮球(进度计数 `done/total` + in_progress 激活时呼吸/脉冲)
- 前端 checklist store:从 `tool:call`/`tool:result` 派生当前态(reload 从 history 最后一条 `update_checklist` tool_result 还原)
- 抑制 `update_checklist` 的普通 tool 卡渲染(走 checklist store,不进 ToolCallCard 流)

## Acceptance Criteria

- [ ] `update_checklist` 注册后,LLM 在多步任务中自发调用(集成测试:MockProvider emit tool_use → Vec 更新 → tool_result 回填完整列表)
- [ ] 全量替换语义:连续两次 update,Vec = 第二次的完整列表(不是 append)
- [ ] 至多一 `in_progress` coerce:传 2 个 in_progress → 保留最后 1 个、另 1 个降 pending,tool 不报错
- [ ] 每轮 ephemeral 注入:turn N 的请求副本头部有"当前 checklist"block(从 Vec 现造),持久化 messages **不含**该 ephemeral block
- [ ] 空表跳过:turn 1(Vec 空)不注入(对称 memory/skill 空 skip)
- [ ] 不打 `cache_control`:ephemeral block 无 cache 标记(每轮必变,标了也不命中)
- [ ] reload 还原:run 结束后重载 session,从 history 最后一条 `update_checklist` tool_result 重建 checklist 卡,状态一致
- [ ] C3 扛压:长 run 触发 compaction 后,live 数组里旧 tool_result 被压掉,agent 仍能从 ephemeral 重发看到当前 checklist(进行中扛压);reload 从 DB 全量 history 还原完整(回放扛压)
- [ ] cancel 一致性:cancel 打断一次 `update_checklist` 执行中途 → 该 update 不 commit tool_result(RULE-A-004),reload 重建 = 上一条已 commit 状态;live Vec 与 history 不打架
- [ ] 零 DB schema 变更(`cargo test --lib` 不需新 migration;sessions/messages 表不动)
- [ ] 前端浮层:展开/最小化切换;in_progress 焦点项有动效;最小化球显示 `done/total` + 激活脉冲
- [ ] Plan 模式放行 `update_checklist`(`filter_tools_for_mode` 黑名单制自动覆盖,确认无需额外代码)
- [ ] 切 session 再切回:checklist 卡从 DB history 重建,正确反映该 session 当前态

## Definition of Done

- 后端单测:`update_checklist` tool(coerce / 全量替换 / 空入参)+ 注入 seam(空跳过 / ephemeral 不入持久化 messages)
- 集成测试(`agent_loop_*`):tool_use → Vec 更新 → tool_result 全表回填 → 下轮 ephemeral 注入可见 + cancel 中途不 commit
- 前端:ChecklistCard 组件 + checklist store + ChatPanel 挂载 + tool 事件路由
- `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib` 绿
- `cd app && pnpm build`(vue-tsc + vite)绿
- ROADMAP §2 B12 进度更新 + IMPLEMENTATION §4 ADR 补"已实施"commit hash + DEBT 无新增

## Technical Approach

### 数据流

```
每轮 agent loop (chat_loop.rs)
  └─ [turn 开头] compact_messages → 得到当轮 messages 副本
  └─ checklist Vec 非空? → build ephemeral synthetic user block(整张 list + focus)
      → prepend 到 messages 副本(不写回持久化 messages)
  └─ provider.send(副本)
  └─ LLM 看 tool list 有 update_checklist + 头部当前 checklist
      → 进度推进 → emit tool_use("update_checklist", {items: [...]})
  └─ execute_tool "update_checklist"
      → coerce 至多一 in_progress
      → 原子替换 loop-local Vec
      → 返回 (完整列表, is_error=false) → ⑫ tool_result 回填(持久化进 history)
  └─ 下一轮:Vec 已是新态,ephemeral 重发新 checklist
```

### 精确接入点

| 层 | 文件 | 动作 |
|---|---|---|
| tool 定义 | `tools/update_checklist.rs`(新) | `update_checklist` tool:入参 items 数组,coerce,替换 Vec,返回全表 |
| tool 注册 | `tools/mod.rs` | `builtin_tools()` 加 `update_checklist` + `execute_tool()` dispatch |
| ToolDef | 注册处 | JSON schema:`{ items: [{ content: string, status: "pending"|"in_progress"|"done" }] }` |
| loop state | `agent/chat_loop.rs` | 加 loop-local `Vec<ChecklistItem>`;每轮 `compact_messages` 后 prepend ephemeral block 到副本 |
| tool↔loop 接线 | `ToolContext` / execute 路径 | **关键 wiring**:tool 执行需 mutate loop 的 Vec → 传 `Arc<Mutex<Vec<ChecklistItem>>>`(或 checklist handle)进 tool 执行上下文,对齐 `read_guard`/`memory_cache` 的传入模式 |
| 前端组件 | `components/chat/ChecklistCard.vue`(新) | ChatPanel 内 `position: absolute` 浮层,展开/最小化,焦点动效 |
| 前端 store | `stores/checklist.ts`(新) | 从 `tool:call`/`tool:result` 派生当前态;reload 从 history 最后一条 update tool_result 还原 |
| 前端挂载 | `components/chat/ChatPanel.vue` | 挂 `<ChecklistCard>` 覆盖层 |
| 事件路由 | `stores/streamController.ts` / `chat.ts` | `update_checklist` 事件路由到 checklist store,抑制 ToolCallCard 渲染 |

### 关键 wiring 风险(tool ↔ loop-local Vec)

tool 执行发生在 `execute_tool`,需 mutate `run_chat_loop` 作用域的 Vec。参照 `run_chat_loop` 现有 14 参(把 `read_guard` / `memory_cache` / `permission_asks` 传进 tool 上下文的模式),新增一个 checklist handle(`Arc<Mutex<Vec<ChecklistItem>>>`)传入。**注意**:`run_chat_loop` 是 production+test 单一权威入口(`agent-loop-architecture.md`),改签名要同步 9+ `agent_loop_*` 集成测试 call site —— 这是本任务最大的机械改动面,PR 拆分时优先把这条接线单独成一个 PR。

### UI 细节(已与用户确认)

- 浮层锚 ChatPanel 右下、向上避开输入栏;z-index 低于 PermissionModal 等弹窗
- 两态:展开卡(完整 checklist + 焦点项 marker 脉冲/spinner)⇄ 最小化悬浮球(`done/total` + in_progress 激活时呼吸)
- 显隐:首条 update 触发出现;run 全 done 展示绿色完成态一拍可收起;新 user message 按 per-request 自动重置

## Out of Scope

- 不做跨 session 的 checklist 看板(per-request 生命周期,跨 run 不持久)
- 不做细粒度 add/update/delete tool(全量替换已定,见 §4 ADR)
- 不做 checklist 的 DB 表(replay 走 history,见 §4 ADR)
- 不做 subagent 集成(B6 之后;Checklist 先独立闭环)
- 不做用户手动编辑 checklist 项(纯 agent 自管;UI 只读展示 + 最小化)
