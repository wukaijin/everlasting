# Review — 大文件拆分与文档同步

> 评审日期:2026-08-08。评审对象:`prd.md` / `design.md` / `implement.md`。
> 方法:对 PRD 中所有可量化声明逐条 `wc -l` / `grep` 核验,并对设计涉及的关键结构边界、可见性、既有约定做实测对照。

## 总体评价

三件套质量高——**事实基础扎实、风险识别到位、拆分顺序合理、可执行性强**。逐条核对后,行数/行号/测试区边界等硬事实**基本全部属实**(精确到 ±2 行)。

**结论:可以批准进入实现**,但建议先处理两个 P1 项再开工——它们在 Phase 1.2 / 1.5 执行时会立刻卡住。P2 口径问题建议在 PRD 顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| 9 个源码文件行数(5132/5674/3040/2290/2219/2081/1933/2683/2156/2054) | **完全一致**(`wc -l`) |
| `chat_loop.rs::run_chat_loop` 为 4317 行单体函数 | **精确**:L306–L4623 = 4317 行 |
| `group_chat_loop.rs` 纯函数簇 L129–423 + reload_messages@423 + run@482 + test@915 | **精确**(逐函数签名核对) |
| `dispatch.rs` 解析簇 L1733–1835 + worktree@1868 + test@1985 | **精确** |
| `wire.rs` test@969 / openai@997 / loader@1176 / group_chat@915 | **全部精确** |
| 6 个文档行数(3671/1498/1462/1423/1401/1318) | **完全一致** |
| `tests_*.rs` 同级子模块模式存在 | **确证**(permissions/ 9 个 + agent/ 10 个,均 `pub mod tests_*`) |
| `streamController.ts` return 块导出 6 项(`handleChatEvent/handleToolCall/finalizeRequest/putMessages/pinnedSessions/loadedFromDb`) | **确证**(R1.3 约束成立,L2619 return 块含全部 6 项) |
| `spec/backend/index.md` 是未维护模板 | **确证**(38 行,"To fill") |
| 拆分顺序"最干净→最复杂" | **合理**:wire 边界最清晰放第一;streamController/dispatch 风险最高放最后 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — `tests_group_chat.rs` 已存在(919 行),design 称"新建"会冲突

**位置**:`design.md` §1.2 / `implement.md` Phase 1.2。

**问题**:Design 写"其余内嵌测试并入 `agent/tests_group_chat.rs` 或**新建**(按内容判断)"。但 `app/src-tauri/src/agent/tests_group_chat.rs` **已存在**(919 行,08-04 group-chat orchestration 集成测试,`#![cfg(test)]`,mock 了完整多轮流程)。这不是"新建",是**往已有文件追加**——把 `group_chat_loop.rs` 内嵌的 ~1018 行测试直接并入,会和现有 919 行集成测试混杂,可见性与 mock 依赖都可能打架。

**建议**:
- 明确测试去向:(a) 纯函数单测随迁进 `group_chat_prompts.rs` 内 `mod tests`;(b) 其余循环逻辑测试单独新建 `tests_group_chat_loop.rs` 或拆进 `tests_group_chat.rs` 的独立 `mod`,而不是笼统"并入或新建"。
- Phase 1.2 执行前必须先 `wc -l tests_group_chat.rs` 确认现状。

### 🔴 P1-2 — `resolve_project_id` / `create_worker_worktree` 可见性与 design 描述不符

**位置**:`design.md` §1.5。

**问题**:Design 列 `resolve.rs` 五个函数时称"已 `pub(crate)`,互相独立"。实际:

```
1733: pub(crate) async fn resolve_model_by_name_or_id   ✓ pub(crate)
1754: pub(crate) async fn resolve_final_model            ✓ pub(crate)
1778: pub(crate) async fn resolve_worker_provider        ✓ pub(crate)
1814: async fn resolve_project_id                        ✗ 私有(无 pub)
1835: pub(crate) async fn resolve_project_main_path      ✓ pub(crate)
1868: async fn create_worker_worktree                   ✗ 私有(无 pub)
```

`resolve_project_id`(L1814)与 `create_worker_worktree`(L1868)是**私有**的。迁到 `resolve.rs` / `worktree.rs` 后,父模块 `dispatch.rs` 调用没问题(同 crate 同父级),但测试迁到 `tests_dispatch.rs` 就需升 `pub(crate)`。

**建议**:Design §1.5 补一句"迁出后 `resolve_project_id` / `create_worker_worktree` 升 `pub(crate)` 以满足 R1.2 测试可见性",否则执行时会在可见性上踩坑。

### 🟡 P2-1 — 范围外的 >1200 行文件未列入,PRD 自称"9 个"造成口径混乱

**位置**:`prd.md` Background。

**问题**:开头写"**9 个** >1200 行文件",实际非测试源码 >1200 行的有 **~22 个**(另含 `git/worktree.rs` 2039、`db/memories.rs` 1781、`subagent/sink.rs` 1679、`skill/loader.rs` 1660、`llm/provider/anthropic.rs` 1525、`tools/shell.rs` 1472、`llm/types.rs` 1229 ……)。

PRD 在 Requirements R1 已用"**范围已定:档位 1+2,8 个文件**"圈定了实际范围,所以**不是遗漏**,而是开头"9 个"统计口径(只统计了表格里列的那批)与全仓真实数字不符,容易让评审者误以为遗漏。`ChatPanel.vue`(1495 行)、`streamController.test.ts`(2136 行)也未在 Out of Scope 显式标注。

**建议**:把 Background 的"9 个"改为"**首批档位 1+2 的 8 个候选**(全仓另有 ~14 个 >1200 行非测试源码,留待后续档位)";Out of Scope 显式补 `ChatPanel.vue`、`streamController.test.ts`、其余 >1200 行源码。

### 🟡 P2-2 — 引用计数口径与 design §2 的"18/10/8/50"不符,需核实

**位置**:`design.md` §2。

**问题**:Design 写"原路径引用(**18/10/8/50 处**活跃引用)不失效"。我实测的活跃引用**文件数**(排除 archive):

| 文档 | 实测引用文件数 |
|---|---|
| tool-contract.md | 21 |
| memory.md | 12 |
| agent-loop-architecture.md | 15 |
| multi-provider-contract.md | 11 |
| docs/IMPLEMENTATION.md | 13 |
| docs/WORKFLOW-INTEGRATION.md | 5 |

数字对不上(且"18/10/8/50"是 4 个数对应 6 个文件,数量都不匹配)。可能 design 写的是"引用次数"而本次数的是"引用文件数",但口径没说明。

**建议**:Design §2 把这串数字改成实测值并标注口径("N 个活跃文件引用 / M 处引用"),否则执行 R4 链接修复时无法用作核对清单。

### 🟡 P2-3 — `tool-contract.md` 内部有 `request_mode_change` Scenario 重复,拆分前应先去重

**位置**:`.trellis/spec/backend/tool-contract.md` L2599 / L3063。

**问题**:L2599 与 L3063 是**两个同名 `## Scenario: request_mode_change tool (B6+ A)`**,共占 ~460 行。按 design §2 "按 `## Scenario:` 分组"拆分时,这两段会分到同一 part,但内容重叠(一个偏 audit 语义、一个偏 Signature)。

**建议**:拆分 part 前先合并去重这两个 Scenario,否则拆出来的 part 自带冗余。

### 🟢 P3-1 — `state-management.md` 是空模板,design 却当"Stream Controller Pattern"参考引用

**位置**:`design.md` §1.7 / `implement.jsonl` / `check.jsonl`。

**问题**:三处都把 `.trellis/spec/frontend/state-management.md` 当作"Stream Controller Pattern"的约束来源。但该文件 **51 行全是 `(To be filled by the team)` 模板**,根本没有 Stream Controller 内容。trellis-check 时按这个 spec 校验会"通过但无意义"。

**建议**:要么删掉对这个 spec 的引用(承认约束不存在),要么把"return 块导出保留"这条约束**就地写进** state-management.md 的 Stream Controller 小节(顺便填一个真实 spec,R3 的精神正是文档同步)。

### 🟢 P3-2 — `RULE-A-006` 引用点散落,check.jsonl 只点了 agent-loop-architecture.md

**位置**:`check.jsonl`。

**问题**:check.jsonl 要"校验 RULE-A-006 未被触碰",但 RULE-A-006 实际被 `tool-contract.md:1073`、`docs/IMPLEMENTATION.md:486`(含行号 `chat_loop.rs:657`)、`debt-status-evolution-guide.md` 等多处引用。本任务不碰 `chat_loop.rs`,所以这些引用**本身不会失效**,但 `docs/IMPLEMENTATION.md` 若被拆分(Phase 3.9),其 §4 里的 `chat_loop.rs:657` 行号引用要随迁到 `IMPLEMENTATION/decisions.md` 并保持锚点。

**建议**:Phase 3.10 的链接 sweep 显式把"RULE-A-006 / chat_loop.rs 行号引用"列入检查清单。

## 💡 可选优化

1. **Phase 0 增加基线计数**:`cargo test --lib` 输出末尾 `test result: ok. N passed` 记录到 task notes,Phase 4.1 对照 N 值不变,比"全绿"更硬。
2. **AC2"纯搬迁"验收可量化**:把"逐文件 `git diff -w` 无语义改动"落到具体命令——Rust 拆分用 `git show <commit> --stat` 看"新增文件 + 删除文件行数≈原文件",配合 `git diff -w --ignore-all-space` 对比函数体。
3. **前端 typecheck 命令未确认**:Implement Phase 2 反复出现"vue-tsc 或项目现有 typecheck 命令",但没给确切命令。建议 Phase 0 确认 `app/package.json` 里的 typecheck script 名,写进速查。

## 修正优先级总览

| 级别 | 项 | 动工前必改? |
|---|---|---|
| 🔴 P1-1 | `tests_group_chat.rs` 已存在的冲突 | 是(Phase 1.2 卡点) |
| 🔴 P1-2 | `resolve_project_id`/`create_worker_worktree` 可见性补正 | 是(Phase 1.5 卡点) |
| 🟡 P2-1 | 范围统计口径("9 个"→"8 候选+14 后续") | 否(顺手改清) |
| 🟡 P2-2 | 引用计数口径核实 | 否(R4 执行前核实) |
| 🟡 P2-3 | `request_mode_change` Scenario 去重 | 否(Phase 3 拆 tool-contract 时处理) |
| 🟢 P3-1 | `state-management.md` 空模板引用 | 否(实现中修) |
| 🟢 P3-2 | RULE-A-006 引用点补全 | 否(Phase 3.10 加清单) |
