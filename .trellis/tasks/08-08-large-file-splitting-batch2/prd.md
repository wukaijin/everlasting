# 大文件拆分批 2:loader / worktree / memories

## Goal

把上一批遗留的 3 个 >1500 行源码文件按职责拆为内聚子模块,改善可导航性;拆分过程**保持功能完全不变**(测试全绿是硬性验收)。延续 08-07 任务的纯搬迁铁律:复制 + 删源 + module 接线,禁止顺手改逻辑。

## Background / 已确认事实(统计日期 2026-08-08)

上一批(`08-07-large-file-splitting`,已 squash merge 进 main `7b60b55`)处理了 wire/group_chat/loader(subagent)/openai/dispatch/MessageItem/streamController 8 处。本批处理探查后确认的 **3 个干净纯搬迁目标**;另 2 个候选(`subagent/sink.rs`、`llm/provider/anthropic.rs`)经探查**剔出本批**——前者 13 个 mutex/atomic 字段 + thread_local 需 design,后者 `chat_stream_with_tools` 607 行单体需专项提取(见 Out of Scope)。

**前提纠正**:`skill/loader.rs` 的 frontmatter/cache **从未拆过**(08-07 拆的是 `agent/subagent/loader.rs`,同名不同模块)。本批首次拆 skill loader。

| 文件 | 行数 | 结构要点(探查结论) |
|---|---|---|
| `app/src-tauri/src/skill/loader.rs` | 1,660 | 5 簇:frontmatter(纯解析 109-286)/ paths(288-330)/ scan(331-483)/ cache(CachedScan+SkillCache 484-611)/ merge+lookup(613-807);内嵌测试 808-1660(852 行,过半);无单体 >300 行;`SkillCache` 3 个 RwLock 但封装在 `arc()` 后 |
| `app/src-tauri/src/git/worktree.rs` | 2,039 | 扁平自由函数,零 struct/零共享状态/零单体(最大 `sweep_stale_worker_worktrees` 159 行);5 簇:naming(47-97)/ create(99-390)/ attach+destroy(392-745)/ worker_sweep(747-1023)/ check_clean(1026-1073);内嵌测试 1075-2039(964 行);**注意** `git/mod.rs` 有 `pub use` re-export(`check_clean`、`destroy as destroy_worktree`) |
| `app/src-tauri/src/db/memories.rs` | 1,781 | 5 簇:types(76-375)/ validation 安全网(376-540)/ crud(565-826)/ search+recall FTS(828-1310)/ lifecycle 提升(1421-1734);无单体 >300 行;无模块级 static;`pub(crate) build_recall_fts_query` 有跨模块调用者;内嵌测试仅 46 行(`insert_raw` helper),重测试在外部 `db/memories_tests.rs`(2241 行) |

**测试位置现状**:
- `skill/loader.rs` — 全部内嵌(852 行),无同级 `tests_*.rs`
- `git/worktree.rs` — 全部内嵌(964 行),无同级 `tests_*.rs`
- `db/memories.rs` — 内嵌仅 46 行 helper;主体在 `db/memories_tests.rs`(2241 行,经 `db/mod.rs` 声明)

## Requirements

- R1 **拆 3 个源码文件**(顺序 loader → worktree → memories):按职责簇拆为内聚子模块;纯搬迁不改逻辑(复制 + 删源 + 接 module 声明),功能不变。
  - R1.1 测试搬迁:内嵌 `#[cfg(test)]` 测试按项目 `tests_*.rs` 同级子模块模式迁出(child module 可见父私有项,`use super::*` 语义不变)。skill 与 git 的内嵌测试体量大(852/964 行),迁出后主文件瘦身显著。
  - R1.2 可见性:被测私有函数升 `pub(crate)` 或经 `pub(crate) use` re-export;`pub(crate) build_recall_fts_query`(memories)保持可见性。不允许为可测性改公开 API。
  - R1.3 re-export 保持:`git/mod.rs` 的 `pub use worktree::{check_clean, destroy as destroy_worktree}` 拆分后必须仍可用(经子模块 re-export 或调整 use 路径)。
  - R1.4 簇划分见 design §1;实现时按实际行号微调边界,不强行切函数。
- R2 **更新过时文档**:每次拆分落地后,全仓扫描引用被搬走符号/路径/行号的活跃文档(`docs/`、`.trellis/spec/`、`AGENTS.md`),更新为拆分后新路径。`.trellis/tasks/archive/` 历史快照不改。
- R3 **功能不变(硬约束)**:拆分只做结构搬迁;后端 `PKG_CONFIG_PATH="..." cargo test --lib` 全绿(不要 `--test-threads=1`);clippy/fmt 零警告。本批无前端改动。

## Acceptance Criteria

- [ ] AC1:列入范围的 3 个源码文件拆分后均 <1200 行;`cargo test --lib` 全绿(~1657 测试)
- [ ] AC2:拆分是可量化的纯搬迁——逐 commit `git show <commit> --stat` 核对新增+删除 ≈ 原文件行数,`git diff -w` 对比函数体无语义改动
- [ ] AC3:全仓 `grep` 确认无活跃文档仍引用被搬走的旧路径/旧行号(archive 历史快照除外)
- [ ] AC4:`git/mod.rs` 的 `pub use` re-export 拆分后调用点零改动
- [ ] AC5:拆分后 clippy + fmt 零警告

## Out of Scope

- `agent/subagent/sink.rs`(1679 行)——13 个 mutex/atomic 字段 + `thread_local!` 测试收集器,拆需暴露 `pub(crate)` 字段 + 推理锁序,非纯搬迁。留专项 design 任务。
- `llm/provider/anthropic.rs`(1525 行)——`chat_stream_with_tools` 607 行单体是文件主体,搬了只是挪 blob;需先 per-stage 提取(build_request/drive_stream/parse_block/handle_tool_delta)。留专项提取任务。
- `chat_loop.rs` / `dispatch.rs` 单体函数重构(同 08-07 Out of Scope,高风险需独立任务)。
- 本批其余 >1200 行非测试源码(migrations 1569、workflow/task.rs 1527、shell_trust 1458、web_fetch 1371 等 ~11 个,留后续批)。
- 前端文件(ChatPanel.vue 1495、streamController.test.ts 2137 等)。
- 既有独立测试文件(`tests_agent_loop.rs`、`memories_tests.rs` 本身不拆,仅随被测符号路径变化适配 import)。
