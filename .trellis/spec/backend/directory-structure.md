# Directory Structure

> How backend code is organized in this project.

---

## Overview

<!--
Document your project's backend directory structure here.

Questions to answer:
- How are modules/packages organized?
- Where does business logic live?
- Where are API endpoints defined?
- How are utilities and helpers organized?
-->

(To be filled by the team)

---

## Directory Layout

```
<!-- Replace with your actual structure -->
src/
├── ...
└── ...
```

---

## Module Organization

<!-- How should new features/modules be organized? -->

(To be filled by the team)

---

## Naming Conventions

<!-- File and folder naming rules -->

(To be filled by the team)

---

## Examples

<!-- Link to well-organized modules as examples -->

(To be filled by the team)

---

## Large-File Splitting — 收官对照 (2026-08-10)

### 守则

- **行数目标**:源码文件与测试文件目标 **< 1200 行**;超限按职责簇拆为内聚子模块(hub + 子文件),纯搬迁铁律——复制 + 删源 + module 接线,**禁止顺手改逻辑**,每簇独立 commit、独立回滚(`git revert`),`cargo test --lib` 全绿是硬性验收。
- **总纲来源**:批 1 `.trellis/tasks/archive/2026-08/08-07-large-file-splitting/prd.md`(统计日期 2026-08-07)是全仓 >1200 行文件拆分总纲(首批表格 8 个 + 正文点名 ~14 个留待档位 + Out of Scope)。**后续各批均为该总纲划出范围的按批落地——Out of Scope 仅批次区分,非永久排除**。

### 收官状态表

| 总纲条目(拆分前行数) | 消化批次 | 现状(2026-08-10) |
|---|---|---|
| `agent/chat_loop.rs` 5132 | A 类 batch4(`08-08-a-class-chat-loop-split`) | ✅ hub 1376(降 75%);run_chat_loop 726 行 |
| `agent/tests_agent_loop.rs` 5674 | `08-09-tests-agent-loop-split` | ✅ tests_agent_loop/ 目录(hub 39 + 9 簇);41 测原样,最大 basic.rs 1085 |
| `agent/subagent/dispatch.rs` 3040 | A 类 batch1 | ✅ 469(run_subagent → 7 阶段函数) |
| `agent/subagent/loader.rs` 2290 | 批 1(`7b60b55`) | ✅ 319(扁平拆 `frontmatter.rs` 240 + `cache.rs` 616;测试 → `tests_loader.rs`) |
| `skill/loader.rs` 1660 | 批 2(`dfcb9ba`) | ✅ 649(子模块 `loader/frontmatter.rs` 167;测试 → `tests_loader.rs`;同名不同域,纠正原表错配) |
| `agent/subagent/sink.rs` 1679 | A 类 batch3 | ✅ 493 |
| `llm/provider/openai.rs` 2219 | 批 1 | ✅ 848 + streaming.rs 382 |
| `llm/provider/wire.rs` 2081 | 批 1 | ✅ wire/ 目录(types/from_wire/to_wire) |
| `llm/provider/anthropic.rs` 1525 | A 类 batch2 | ✅ 584 |
| `agent/group_chat_loop.rs` 1933 | 批 1 | ✅ 628 + group_chat_prompts.rs 292 |
| `git/worktree.rs` 2039 | 批 2 | ✅ 54 |
| `db/memories.rs` 1781 | 批 2 | ✅ 83 |
| `tools/shell.rs` 1472 | 批 3 | ✅ 586 |
| `agent/tests_subagent.rs` 4270 | `08-09-tests-subagent-split` | ✅ tests_subagent/ 目录(hub 82 + 10 簇);30 测原样,最大 dispatch_main.rs 969 |
| `db/memories_tests.rs` 2241 | `08-09-tests-memories-split` | ✅ memories_tests/ 目录(hub 35 + 9 簇);49 测原样,最大 list_delete_search.rs 657 |
| `db/sessions_tests.rs` 1493 | `08-09-tests-sessions-split` | ✅ sessions_tests/ 目录(hub 35 + 5 簇);35 测原样,最大 latency_message.rs 564 |
| `app/src/stores/streamController.ts` 2683 | 批 1 | ✅ 1121 + streamEvents.ts 1200(临界) |
| `app/src/components/chat/MessageItem.vue` 2054 | 批 1 | ✅ 1125 + messageCards/*.ts |
| 文档 >1200(6 个 .md,含 tool-contract 3671 / agent-loop-architecture 1462) | 批 1 R3 | ✅ 全部目录化 hub <100 行 |

### 已知遗留(非缺陷,后续可选)

- `agent/chat_loop.rs` hub 1376 行(超目标 176):A 类收官状态,run_chat_loop 骨架 + 已提取函数 + re-export,再拆收益小,暂缓。
- `agent/chat_loop/drive.rs` 1653 / `agent/chat_loop/tools.rs` 1648:子模块化产物;design 预留 tools.rs 内再拆 `execute_parallel`/`execute_serial` 私有函数(可选)。
- `run_chat_loop` 34 参数签名债务:冻结,PRD 约定"另立任务"(未立)。
- `db/subagent_runs_tests.rs` 1219(超 19):**范围外漏网**——不在 08-09 四个 tests_* 拆分任务范围内(group-chat Phase 4 `35e631c` + subagent resume `703ab7d` 多轮迭代长成),可选后续拆,沿用 tests_* 目录化模式。
- 4 个测试任务进度:agent_loop ✅ / memories ✅ / sessions ✅ / subagent ✅(4/4 完成);**批范围内**源码/测试文件现已全部 <1200 行。范围外遗留:`subagent_runs_tests.rs` 1219(见上)+ 前端 4 文件(`chat.ts` 2156 / `streamController.test.ts` 2137 / `ChatPanel.vue` 1495 / `SubagentDrawer.test.ts` 1249,总纲 PRD line 61 显式 Out of Scope)。
