# 大文件拆分批 3:12 处 B 类纯搬迁

## Goal

把剩余 12 个 >1200 行源码文件按职责拆为内聚子模块 / 迁出内嵌测试,改善可导航性;拆分过程**保持功能完全不变**(测试全绿是硬性验收)。延续批 1(08-07)批 2(08-08)的纯搬迁铁律:复制 + 删源 + module 接线,禁止顺手改逻辑。

## Background / 已确认事实(统计日期 2026-08-08,批 2 之后)

前两批(已 squash merge 进 main `7b60b55`/`dfcb9ba`)共完成 11 个源码文件纯搬迁。本批处理探查后确认的 **12 个干净纯搬迁目标**。A 类高风险单体(`chat_loop.rs` 5132、`dispatch.rs` 1472、`sink.rs` 1679、`anthropic.rs` 1525)留专项任务,见 Out of Scope。

**关键探查结论**:12 个文件中,**9 个迁出内嵌测试即可 <1200 行**(纯测试搬迁);**3 个需结构性拆分**(测试迁出后仍 >1200):

| 文件 | 行数 | 内嵌测试 | 测试迁出后 | 是否需结构拆 |
|---|---:|---:|---:|:---:|
| `db/migrations.rs` | 1569 | 116(1454-1569) | 1453 | **是**(run_migrations 1011 行单体) |
| `agent/workflow/task.rs` | 1527 | 733(794-1527) | 794 | 否 |
| `tools/request_task_state_transition.rs` | 1506 | 771(736-1506) | 735 | 否 |
| `tools/shell.rs` | 1472 | 885(588-1472) | 587 | 否 |
| `agent/permissions/shell_trust.rs` | 1458 | 668(790-1458) | 790 | 否 |
| `agent/subagent/mod.rs` | 1389 | 461(928-1389) | 928 | 否 |
| `tools/web_fetch.rs` | 1371 | 517(855-1371) | 854 | 否 |
| `agent/permissions/check.rs` | 1365 | 0(外部 tests_check.rs) | 1365 | **是**(check 路径 vs pitfall 路径双簇) |
| `db/sessions.rs` | 1297 | 0(外部 sessions_tests.rs) | 1297 | **是**(session CRUD vs message 持久化) |
| `tools/merge_worker.rs` | 1251 | 337(914-1251) | 914 | 否 |
| `llm/types.rs` | 1229 | 651(578-1229) | 578 | 否 |
| `agent/workflow/inject.rs` | 1222 | 514(708-1222) | 708 | 否 |

**测试位置现状**:
- 内嵌 `#[cfg(test)] mod tests`:9 个文件(migrations/task/request_task/shell/shell_trust/subagent-mod/web_fetch/merge_worker/types/inject)
- 全外部 `tests_*.rs`:3 个文件——`check.rs`→`tests_check.rs`、`sessions.rs`→`sessions_tests.rs`、`migrations.rs` 主体迁移在外部 fixtures(但 `init_pool` pragma 测试内嵌)

**特殊状态**(探查发现,影响拆分策略):
- `tools/merge_worker.rs:304` 有 `static LOCKS: OnceLock<Mutex<HashMap<...>>>`(per-session 合并锁)——`merge_lock_for` + 两个合并函数体必须落同一子模块
- `agent/subagent/mod.rs` 已是 Rust 2018 hub(与 `subagent/` 13 个兄弟文件共存),本身是目录模块的 hub,拆分 = 把内联类型/函数搬出到子文件
- `agent/workflow/inject.rs` 物理顺序与逻辑簇不一致(`breadcrumb_body` 629 行物理在 delegation 簇后、测试前,但逻辑属 breadcrumb 簇)
- 3 处路径在批 1/批 2 后漂移:`shell_trust`/`check` 已在 `agent/permissions/`(非 `permissions/`)、`inject` 已在 `agent/workflow/`(非 `workflow/`)

## Requirements

- R1 **拆 12 个源码文件**(顺序见 design §0):按职责簇拆为内聚子模块;纯搬迁不改逻辑(复制 + 删源 + 接 module 声明),功能不变。
  - R1.1 测试搬迁:内嵌 `#[cfg(test)] mod tests` 迁出 → 同级 `tests_*.rs`(父 mod.rs 声明,`#![cfg(test)]` 文件级门控,`use super::*` 改 `use crate::<module>::*`)。child module 可见父私有项语义不变。
  - R1.2 可见性:被测私有函数升 `pub(crate)` 或经 `pub(crate) use` re-export;既有 `pub(crate)` 符号(如 `apply_safe_env`、`WebFetchError`)保持可见性。禁止为可测性改公开 API。
  - R1.3 re-export 保持:所有既有 `pub use`(各父 mod.rs)拆分后必须仍可用(经子模块 re-export 或 hub `pub use` 全量 re-export)。关键:`llm/mod.rs` 的 `pub use types::{ChatEvent, ChatMessage, ContentBlock, LlmErrorCategory, MessageContent, Role, ToolDef}`(~10 个 Pattern-2 调用方依赖)、`permissions/mod.rs` 的 5 个 check re-export、各 tools mod.rs `pub mod` 路径。
  - R1.4 簇划分见 design §1-12;实现时按实际行号微调边界,不强行切函数。单体函数(check 518 行、execute_blocking 390 行、run_migrations 1011 行)整体搬迁不切。
  - R1.5 3 处结构拆(migrations/sessions/check)需把跨簇共享 helper 显式 `use super::...` 引入(如 sessions 的 `edit_user_message` 依赖 cluster E 的 `find_message_id_by_seq`)。
- R2 **更新过时文档**:每次拆分落地后,全仓扫描引用被搬走符号/路径/行号的活跃文档(`docs/`、`.trellis/spec/`、`AGENTS.md`),更新为拆分后新路径/符号引用。`.trellis/tasks/archive/` 历史快照不改。已识别 ~20 个文档引用目标文件(见 design §13)。
- R3 **功能不变(硬约束)**:拆分只做结构搬迁;后端 `PKG_CONFIG_PATH="..." cargo test --lib` 全绿(不要 `--test-threads=1`);clippy/fmt 零警告。本批无前端改动。

## Acceptance Criteria

- [ ] AC1:列入范围的 12 个源码文件拆分后均 <1200 行;`cargo test --lib` 全绿(当前 ~1657 测试)
- [ ] AC2:拆分是可量化的纯搬迁——逐 commit `git show <commit> --stat` 核对新增+删除 ≈ 原文件行数,`git diff -w` 对比函数体无语义改动
- [ ] AC3:全仓 `grep` 硜认无活跃文档仍引用被搬走的旧路径/旧行号(archive 历史快照除外)
- [ ] AC4:所有既有 `pub use` re-export 拆分后调用点零改动(llm/mod.rs 7 符号、permissions/mod.rs 5 符号、各 tools mod.rs `pub mod` 路径)
- [ ] AC5:拆分后 clippy + fmt 零警告
- [ ] AC6:3 处结构拆(migrations/sessions/check)的跨簇 helper 引用(`super::`/hub re-export)编译通过,无 dangling private 项

## Out of Scope

- **A 类高风险单体函数重构**(留专项 design 任务):
  - `agent/chat_loop.rs`(5132 行)——run_chat_loop 4298 行单体,核心生产入口,守 spec RULE-A-006。需先按 turn 阶段提取 per-turn 函数再拆。
  - `agent/subagent/dispatch.rs`(1472 行)——run_subagent 1295 行单体,同策略,规模小一半可作 chat_loop 练手。
  - `agent/subagent/sink.rs`(1679 行)——13 个 mutex/atomic 字段 + `thread_local!` 测试收集器,拆需暴露 `pub(crate)` 字段 + 推理锁序,非纯搬迁。
  - `llm/provider/anthropic.rs`(1525 行)——chat_stream_with_tools 607 行单体,需 per-stage 提取。
- 前端文件(`stores/chat.ts` 2156 已评审为薄门面建议不拆、`ChatPanel.vue` 1495)。
- 既有独立测试文件本身不拆(`tests_*.rs` 仅随被测符号路径变化适配 import)。
- `db/migrations.rs` 的 `run_migrations`(1011 行单体)——**本批只搬不切**:连同 helpers 一起搬到 `migrations/schema.rs` 子模块,保持单体完整性。per-stage 切分属 A 类重构,留专项。
