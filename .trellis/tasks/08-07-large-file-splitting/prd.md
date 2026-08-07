# 大文件拆分与文档同步

## Goal

把超过 1200 行的源码文件与文档拆分为职责内聚的小文件，改善可导航性；拆分过程**保持功能完全不变**（测试全绿是硬性验收）。拆分后同步更新引用这些文件路径/行号的文档，并拆分超长文档本身、修复由此产生的失效链接。

## Background / 已确认事实（统计日期 2026-08-07）

**源码侧**:本任务**首批候选为档位 1+2 的 8 个文件**(共 ~23,600 行,均已做内部结构测绘,见会话调研记录);全仓另有 ~14 个 >1200 行非测试源码文件(如 `git/worktree.rs` 2039、`db/memories.rs` 1781、`subagent/sink.rs` 1679、`skill/loader.rs` 1660、`llm/provider/anthropic.rs` 1525、`tools/shell.rs` 1472 等)留待后续档位,不在本任务范围:

| 文件 | 行数 | 结构要点 |
|---|---|---|
| `app/src-tauri/src/agent/chat_loop.rs` | 5,132 | `run_chat_loop` 为 **4,317 行单体函数**（无内嵌 fn）；尾部 helper 簇 L4623-4943 可独立；内嵌测试仅 190 行 |
| `app/src-tauri/src/agent/tests_agent_loop.rs` | 5,674 | 独立测试文件（不在拆分候选） |
| `app/src-tauri/src/agent/subagent/dispatch.rs` | 3,040 | 尾部解析簇 L1733-1835 已 `pub(crate)` 互相独立 → `resolve.rs`；worktree 创建 L1868-1986 → 独立；**内嵌测试 1,055 行** |
| `app/src-tauri/src/agent/subagent/loader.rs` | 2,290 | frontmatter 解析器（纯函数）→ `frontmatter.rs`；扫描/缓存 → `cache.rs`；**内嵌测试 1,114 行（49%）** |
| `app/src-tauri/src/llm/provider/openai.rs` | 2,219 | tool-call delta 流式装配簇 L850-998 → `streaming.rs`；**内嵌测试 1,222 行（55%）** |
| `app/src-tauri/src/llm/provider/wire.rs` | 2,081 | 双向转换依赖边界最干净：`wire/out.rs`（Chat→wire L239-686）+ `wire/in.rs`（wire→Chat L757-970）+ `wire/types.rs`；**内嵌测试 1,112 行** |
| `app/src-tauri/src/agent/group_chat_loop.rs` | 1,933 | prompt/历史纯函数簇 L129-423（`moderator_system_prompt`/`role_history`/`group_chat_tool_defs`）→ `group_chat_prompts.rs`；**内嵌测试 1,018 行（53%）** |
| `app/src/stores/streamController.ts` | 2,683 | 事件处理块 L1042-2183（含 633 行 `handleChatEvent`）→ `streamEvents.ts`；return 块为测试暴露内部，拆分须保留导出 |
| `app/src/stores/chat.ts` | 2,156 | 已是薄门面（类型在 chat.types.ts），**建议不拆** |
| `app/src/components/chat/MessageItem.vue` | 2,054 | script 1,164 行；4 个卡片状态解析簇（ask/mode-change/task-state-transition，各 ~250 行纯 TS 无 Vue 依赖）→ `messageCards/*.ts` |

**文档侧**（>1200 行的 .md）：

- `.trellis/spec/backend/tool-contract.md` — 3,671 行（spec，被 check 流程引用，拆分须同步 spec index）
- `.trellis/workspace/*/journal-*.md` — 4 个 ~2,000 行（个人开发日志，workflow 自带 2000 行轮换规则）
- `.trellis/spec/backend/memory.md` — 1,498 行
- `.trellis/spec/backend/agent-loop-architecture.md` — 1,462 行（含群聊 Pattern，代码引用 `group_chat_loop.rs` 路径/行号）
- `docs/IMPLEMENTATION.md` — 1,423 行（决策日志，含 commit hash 与任务路径引用）
- `.trellis/spec/backend/multi-provider-contract.md` — 1,401 行
- `docs/WORKFLOW-INTEGRATION.md` — 1,318 行

**既有文档失同步先例**：docs/ARCHITECTURE/IMPLEMENTATION/ROADMAP 的群聊章节已于 26eb54b 同步至 HEAD；但代码拆分后，spec/文档中的**文件路径与行号引用**会再次过时（例如 `agent-loop-architecture.md` 的 "Group-chat transcript view" Pattern 引用 `group_chat_loop.rs` 路径）。

## Requirements

- R1 **拆源码文件**（范围已定：档位 1+2，8 个文件）：按职责把超长源码文件拆为内聚子模块；纯搬迁不改逻辑（复制+删源+接 module 声明），功能不变。
  - 范围清单：`group_chat_loop.rs`（prompt/历史簇 → `group_chat_prompts.rs`）、`wire.rs`（→ `wire/{types,out,in}.rs`）、`loader.rs`（→ `frontmatter.rs` + `cache.rs`）、`openai.rs`（流式装配簇 → `streaming.rs`）、`dispatch.rs`（解析簇 → `resolve.rs`）、`streamController.ts`（事件块 → `streamEvents.ts`）、`MessageItem.vue`（4 个卡片解析簇 → `messageCards/*.ts`）+ 5 个 Rust 文件内嵌测试搬迁。`chat_loop.rs` 与 `chat.ts` 不拆。
  - R1.1 测试搬迁：文件内嵌 `#[cfg(test)]` 测试按项目现有 `tests_*.rs` 同级子模块模式迁出（child module 可见父模块私有项，`use super::*` 语义不变）。
  - R1.2 可见性：被测私有函数可升 `pub(crate)` 或经 `pub(crate) use` re-export，不允许为了可测性改公开 API。
  - R1.3 前端约束：`streamController.ts` 拆出事件模块后，`return` 块暴露给测试的内部函数（`handleChatEvent`/`handleToolCall`/`finalizeRequest`/`putMessages` 等）必须保留，两个测试文件（`streamController.test.ts` / `streamController.review.test.ts`）同步适配。
- R2 **更新过时文档**：每次拆分落地后，全仓扫描引用被搬走符号/路径/行号的文档（spec + docs + AGENTS.md），更新为拆分后的新路径。
- R3 **拆分文档**（范围已定：docs + spec 全拆，6 个文件）：对 >1200 行的文档按主题拆为 hub+parts（原文件路径保留为 <500 行 hub 总览+链接，内容迁入同名目录 parts 文件——既有路径引用不失效）；拆分为 `tool-contract.md`（按 Scenario 分组）/ `memory.md` / `agent-loop-architecture.md` / `multi-provider-contract.md` / `docs/IMPLEMENTATION.md` / `docs/WORKFLOW-INTEGRATION.md`。注意 `.trellis/spec/backend/index.md` 是未维护模板（不列指引文件），spec 文件的实际引用方是 docs/PRD/check.jsonl 中的**直接路径引用**——拆分后需逐一同步（归入 R4）。
- R4 **更新过时链接**：拆分与重命名后，修复所有失效的相对路径引用与锚点链接（含跨文件链接、决策日志中的任务路径、spec 互引）。
- R5 **功能不变（硬约束）**：拆分只做结构搬迁，禁止顺手改逻辑；后端 `cargo test --lib` 全绿（~1650 测试，单线程约束见 AGENTS.md 坑 1：需 `PKG_CONFIG_PATH`），前端 `pnpm test` 全绿；clippy/fmt 零警告。

## Acceptance Criteria

- [x] AC1：列入范围的每个源码文件拆分后 <1200 行；**豁免**：`dispatch.rs`（1467 行，`run_subagent` 1354 行单体函数主导，纯搬迁后仍超 1200——用户 2026-08-08 批准豁免，单体拆分留待后续任务，与 chat_loop.rs 同策略）、`chat_loop.rs`（不拆）。`cargo test --lib` + `pnpm test` 全绿
- [x] AC2：拆分是**可量化的纯搬迁**——逐 commit `git show <commit> --stat` 核对"新增文件行数 + 删除 ≈ 原文件行数"，`git diff -w` 对比函数体无语义改动
- [x] AC3：全仓 `grep` 确认无文档仍引用被搬走的旧路径/旧行号
- [x] AC4：列入范围的每个文档拆分后 <1200 行，spec index 指引文件列表与实际文件一致
- [x] AC5：拆分后运行 `trellis-check`（spec 合规 + lint + 测试 + 跨层一致性）通过

## Out of Scope

- `chat_loop.rs` 4,317 行单体函数的**逻辑重构**（需先重构再拆，风险高，建议单列后续任务；本任务完全不碰 chat_loop.rs）
- `chat.ts`（薄门面，拆无收益）
- 全仓其余 >1200 行非测试源码（`git/worktree.rs`、`db/memories.rs`、`subagent/sink.rs`、`skill/loader.rs`、`anthropic.rs`、`tools/shell.rs` 等 ~14 个，留待后续档位）
- `ChatPanel.vue`（1,495 行）、`streamController.test.ts`（2,136 行）等 >1200 行前端文件（本轮不拆）
- `tests_agent_loop.rs` / `tests_subagent.rs` 等既有独立测试文件的拆分
- 生成文件（`gen/schemas/*.json`、lockfile、`app/dist`）
- journal 开发日志的拆分（已有 2000 行轮换规则）
- `.trellis/tasks/archive/` 历史记录中的引用更新（历史归档不改）
