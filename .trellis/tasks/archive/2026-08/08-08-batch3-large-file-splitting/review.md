# Review — 大文件拆分批 3:12 处 B 类纯搬迁

> 评审日期:2026-08-08。评审对象:`prd.md` / `design.md` / `implement.md`(status=planning,实施前评审)。
> 方法:对 PRD 表格行数、内嵌测试起止行、特殊状态、re-export 清单、跨模块消费点、文档引用清单逐条 `wc -l` / `grep` 核验;对 3 处结构拆做逐函数边界实测(起止行由括号平衡推算)。

## 总体评价

三件套质量高——**事实基础扎实、风险识别到位、顺序与回滚设计合理**。20+ 条硬事实核验**全部属实**(行数精确到 ±2 行),12 个目标文件与全仓 >1200 行清单无遗漏(4 个 A 类单体、5 个独立测试文件均正确排除)。

**结论:可以批准进入实现**,但建议先处理两个 P1 项再开工——它们分别在 Phase 11(tests_check.rs 编译失败)与 Phase 12(schema.rs 超 1200 行,AC1 失败)处卡住。P2 项为文档级小修,顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| 12 个目标文件行数(1569/1527/1506/1472/1458/1389/1371/1365/1297/1251/1229/1222) | **完全一致**(`wc -l`) |
| 全仓 >1200 行清单无遗漏 | **确证**:仅余 4 个 A 类(`chat_loop` 5132 / `sink` 1679 / `dispatch` 1472 / `anthropic` 1525)+ 5 个独立测试文件,均正确归 Out of Scope |
| 9 文件内嵌测试起止行 | **全部吻合**(±1 行):types 578 / task 794 / request_task 736 / shell 588 / shell_trust 790 / subagent 928 / web_fetch 855 / merge_worker 914 / inject 708 |
| 9 个"纯测试搬迁即 <1200" + 3 个"需结构拆" | **确证**(migrations 1453 / check 1365 / sessions 1297 无测试可迁) |
| `merge_worker` `static LOCKS` L304 + 锁与合并体同区 | **精确**:`merge_lock_for` L303-304,`do_merge_blocking` 393 / `merge_session_into_main` 643 / `is_ancestor` 805 / `collect_conflict_paths` 830 同区;`finalize_merge` 862 调 `destroy_worker`(doc L11) |
| `check` 517 行单体 | **精确**:L53-570 = 518 行;Tier4 helpers 全 `pub(crate)`(ToolKind 578 / classify_tool 595 / extract_path_arg 622 / check_path_grant 639 / sqlite_glob_match 683 / check_tool_grant 765 / check_prefix_grant 782 / match_value_for_allow_always 822) |
| `run_migrations` 1006 行单体 | **精确**:L81-1091 = 1011 行(含 doc comment 1014);6 个 `add_*` helper 1093-1356 |
| inject.rs 物理错位 | **确证**:`build_breadcrumb_block` 617 / `breadcrumb_body` 629 物理在 delegation 簇后,逻辑属 breadcrumb 簇 |
| 3 处路径漂移 | **确证**:`shell_trust`/`check` 在 `agent/permissions/`、`inject` 在 `agent/workflow/` |
| `llm/mod.rs` 7 符号 re-export(L34-36) | **确证**:Pattern-2 调用方实测 **12 个**文件(`use crate::llm::{...}`) |
| `workflow/mod.rs` 13 符号(L106) | **精确**:与 design §2 清单逐项一致 |
| `permissions/mod.rs` 5 符号(L124-150) | **精确** |
| 跨模块消费点 | **全部属实**:`ask.rs:13`(`super::check::match_value_for_allow_always`)、`question.rs:53` + `request_task_state_transition.rs:91`(`::task::` 直路径)、`chat_loop.rs:4073`(execute_blocking)、`src/error.rs:22`(`WebFetchError`)、`background_shell/in_memory.rs:196`(`apply_safe_env`) |
| `sessions_tests.rs` 经 `crate::db::sessions::{...}` 导入 18 个符号 | **确证**(L28-31);另注意其 L24 还导入 `crate::db::migrations::run_migrations` |
| 文档引用清单(§13) | **全部存在**(含 `WORKFLOW-INTEGRATION-REVIEW.md`/`-2.md`、tool-contract 01-12) |
| 批1/2 hub 先例、归档机制、base 提交 | **确证**:`git/worktree.rs` hub 模式(`mod check/create/lifecycle/...`)、`.trellis/scripts/task.py` 存在、main 在 `2eb901a` |
| 拆分顺序(风险递增:9 纯测试搬迁 → 3 结构拆) | **合理** |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — check.rs hub re-export 清单不完整,会破坏"tests_check.rs 不动"

**位置**:`design.md` §11.2 / `implement.md` Phase 11.2。

**问题**:hub 只列了 5 个 pub 符号 + `match_value_for_allow_always`,但 `tests_check.rs:7-13` 实际从 `crate::agent::permissions::check::` 导入 **8** 个符号——漏了 4 个 `pub(crate)` 项:

```rust
// tests_check.rs 实际导入
classify_tool, extract_path_arg, match_value_for_allow_always,
recall_pitfall, recall_pitfall_footnote, sqlite_glob_match, PitfallRecall, ToolKind
```

按当前清单执行 → 4 个符号无 re-export → `tests_check.rs` 编译失败,直接违反 implement 11.4 的"tests_check.rs 不动"验证与 R1.3 调用点零改动。

**建议**:hub 补 `pub(crate) use permission::{classify_tool, extract_path_arg, sqlite_glob_match, ToolKind};`(或 `pub(crate) use permission::*`)。注:Tier4 helpers 已是 `pub(crate)`(实测),无需再升可见性,只缺 hub 转发。

### 🔴 P1-2 — schema.rs 体量预计 ~1185 行,贴着 AC1(<1200)上限

**位置**:`design.md` §12 / `implement.md` Phase 12.1 / 12.7。

**问题**:按 design 的归簇,`migrations/schema.rs` = `run_migrations`(L81-1091, **1011 行**)+ `migrate_provider_api_keys_to_encrypted`(1168-1226, 59)+ `widen_subagent_runs_status_check_for_incomplete`(1358-1445, 88)+ `home_dir_or_dot`(1447-1453, 7)+ 头部 ≈ **1185 行**,余量仅 ~15 行。搬迁时夹带任何注释/空行即超线,AC1 失败。

**建议**:把 `migrate_provider` / `widen_subagent` / `home_dir_or_dot` 单列 `migrations/schema_helpers.rs`,schema.rs 只留 run_migrations 单体(~1035 行);或至少把 `home_dir_or_dot` 留 hub。implement 12.7 已标注"可能接近 1200"但未给兜底方案,需在执行前定案。

### 🟡 P2-1 — `db/tests_migrations.rs` 命名与 db/ 目录惯例不一致

**位置**:`design.md` §12 / `implement.md` Phase 12.4。

**问题**:db/ 下 8 个既有测试文件全为 `*_tests.rs` 后缀(`memories_tests.rs`、`sessions_tests.rs`、`permissions_tests.rs`……),design 却计划 `db/tests_migrations.rs` 前缀。tools/llm 无先例不受影响,但 db/ 目录内出现第三种命名。

**建议**:改 `db/migrations_tests.rs`,保持目录内一致。

### 🟡 P2-2 — `subagent/mod.rs:104` 的测试专用 re-export 未在设计中显式标注

**位置**:`design.md` §5 / `implement.md` Phase 5.2。

**问题**:`agent/subagent/mod.rs:104` 有 `#[cfg(test)] pub(crate) use event_sink::{arm_test_collector, clear_test_collector};`(2026-07-21 加的 test-only re-export,消费者是 `sink.rs` 的 cfg(test) 块)。它不在"内嵌测试 928-1389"范围内,但属于测试相关代码——拆分时若被误当内嵌测试一并挪走,`sink.rs` 测试编译失败。

**建议**:Phase 5.2 显式写"该 `#[cfg(test)]` re-export 保留在 hub mod.rs",与 DISPATCH_TOOL_NAME 等一并不动。

### 🟢 P3-1 — 若干数字小误差(均不阻塞,建议顺手改)

- `design.md` §11:check.rs 拆缝写 859,实际 pitfall 簇首函数 `recall_pitfall_footnote` 在 **924**(859-924 是注释区;R1.4 允许微调,但按实际 924 写更准)
- `design.md` §10:sessions.rs 拆缝 17-734/736-1297,实际 session 簇末 fn `insert_system_event` 689-753、message 簇首 `persist_turn` **754**
- `design.md` §5 标题 "1389 → ~870":测试迁出后是 **928**,hub 拆后约 350,"~870" 两个数都不成立
- `design.md` §4 / `implement.md` 4.2:`first_token` 重复列了两次;"`max_of` 中的私有项"表述含糊
- `prd.md` §12:`run_migrations` "1006 行(78-1087)" → 实际 1011 行(81-1091)
- 各文件内嵌测试行数 ±1(1589-1454=135 等均一致,仅四舍五入差异,无影响)

### 🟢 P3-2 — 两个"首批"事实需执行时知晓

- `tools/mod.rs` 目前**无** `#[cfg(test)] mod tests_xxx;` 先例(L69 是 `test_default_pool` 辅助函数,非测试模块声明)——tools/ 下 4 个 `tests_*.rs` 是本批新增,implement 需在 tools/mod.rs 新增声明,非沿用
- `llm/mod.rs` 目前无 cfg(test) 段(provider 测试声明在 `provider/mod.rs`)——`tests_types` 声明为新增,无冲突

## 💡 可选优化

1. **P1-2 的 schema.rs 行数预算先算后做**:Phase 12 开工前用"目标文件预计行数 = Σ函数体行数 + 头部"公式(见本 review 实测数据)预演一遍,把每个子文件预算写进 implement 12.7 核对表,避免临场发现超线。
2. **sessions_tests.rs 的 `migrations::run_migrations` 导入**(`db/sessions_tests.rs:24`)是比 `state.rs` 更隐蔽的 migrations hub 依赖——Phase 12 验证命令可加 `cargo test --lib "sessions_tests"` 一并覆盖。
3. **基线测试计数**:Phase 0.3 把 `cargo test --lib` 末尾 `test result: ok. N passed` 的 N 值记录到 task notes,终验对照 N 不变,比"全绿"更硬(延续批 1 建议)。

## 修正优先级总览

| 级别 | 项 | 动工前必改? |
|---|---|---|
| 🔴 P1-1 | check.rs hub 补 4 个 `pub(crate)` re-export(classify_tool/extract_path_arg/sqlite_glob_match/ToolKind) | 是(Phase 11 卡点,tests_check.rs 编译失败) |
| 🔴 P1-2 | schema.rs 超线预案:helpers 单列 `schema_helpers.rs` | 是(Phase 12 卡点,AC1 失败) |
| 🟡 P2-1 | `db/tests_migrations.rs` → `db/migrations_tests.rs` | 否(顺手改) |
| 🟡 P2-2 | subagent/mod.rs:104 `#[cfg(test)]` re-export 显式标注留 hub | 否(Phase 5 执行时注意) |
| 🟢 P3-1 | 数字小误差(924/754/928/1011/去重 first_token) | 否(顺手改) |
| 🟢 P3-2 | tools/mod.rs、llm/mod.rs 无测试声明先例 | 否(执行时知晓) |
