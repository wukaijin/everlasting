# Design — 大文件拆分批 2:loader / worktree / memories

> PRD: `prd.md`。本设计覆盖 3 个源码文件的簇拆分 + 测试迁出 + 文档引用同步。无文档拆分(本批纯源码)。

## 0. 拆分总原则(延续 08-07)

- **纯搬迁**:内容原样复制 → 删源 → 新 module 接线;禁止顺手改逻辑(R3)。
- **验证节奏**:每个文件拆分后跑对应模块测试(`cargo test --lib "<filter>"`);最后一轮全量终验。
- **回滚**:每个拆分独立 commit,`git revert <commit>` 即回滚。
- **调用点不变**:经 re-export 保持既有 `use crate::git::worktree::X` 等调用点零改动。
- **链接更新范围**:只改活跃文档(`docs/`、`.trellis/spec/`、`AGENTS.md`);`.trellis/tasks/archive/` 是历史记录不改。

## 1. 源码拆分(3 个文件,顺序:loader → worktree → memories)

### 1.1 `skill/loader.rs`(1660 行)→ `skill/loader/` 目录模块 — 第一优先,纠正错误前提

- 现状(探查测绘):
  - `SkillSource`(enum,59)、`SkillResource`(struct,87)、`SkillInfo`(struct,123)、`Frontmatter`(private struct,109)、`CachedScan`(private struct,484)、`SkillCache`(struct,505)
  - frontmatter 簇(109-286):`Frontmatter` + `parse_frontmatter`/`apply_kv`/`parse_allowed_tools`(纯解析零 IO)
  - paths 簇(288-330):`user_skills_dir`/`project_skills_dir`/`plugin_skills_dir`(纯路径)
  - scan 簇(331-483):`current_mtimes`/`scan_skill_dir`/`parse_skill_content`/`load_skill_file`/`builtin_plugin_skills`
  - cache 簇(484-611):`CachedScan` + `SkillCache` + `impl SkillCache`(`arc`/`list_user`/`list_project`/`list_plugin`/`read_through`)
  - merge/lookup 簇(613-807):`resource_to_info`/`list_skill_infos`(+workflow 变体)/`merge_skill_layers`/`find_skill`(+workflow 变体)/`find_skill_in_layers`/`build_skill_listing_block`
  - 内嵌测试(808-1660,852 行)
- 目标目录结构(`skill/loader/` 目录 + `skill/loader.rs` → `skill/loader/mod.rs`,或保留 `skill/loader.rs` 作 hub + `skill/loader/` 子目录 —— Rust 2018 允许 `loader.rs` 与 `loader/` 共存,与 08-07 wire 同模式):
  - `skill/loader.rs`(hub,留 merge/lookup 簇 + module 声明 + re-export)
  - `skill/loader/frontmatter.rs` — `Frontmatter` + 解析函数(全部 `pub(crate)`)
  - `skill/loader/paths.rs` — 3 个路径函数
  - `skill/loader/scan.rs` — 目录扫描簇
  - `skill/loader/cache.rs` — `CachedScan` + `SkillCache` + impl
  - `skill/mod.rs` — `pub mod loader;` 不变(路径稳定)
- 测试:全部内嵌测试 → `skill/tests_loader.rs`(skill/mod.rs 声明,参照 agent/mod.rs 的 tests_* 惯例);被测 fn 已 `pub(crate)` 或升 `pub(crate)`(R1.2)。
- 验证:`cargo test --lib "skill::loader"` + `cargo test --lib "tests_loader"`。
- 预期 hub <800 行(merge/lookup ~195 行 + 类型定义 re-export + mod 声明)。

### 1.2 `git/worktree.rs`(2039 行)→ `git/worktree/` 目录模块

- 现状:扁平自由函数(无 struct/impl/共享状态),5 簇:
  - naming(47-97):`worktree_path`/`branch_name`/`worker_branch_name`/`worker_worktree_path`
  - create(99-390):`create`/`create_worker`/`self_heal_for_create`/`create_worktree_add`
  - attach+destroy(392-745):`attach_session`/`destroy`/`destroy_worker`
  - worker_sweep(747-1023):`commit_worker_changes`/`sweep_stale_worker_worktrees`/`resolve_cleanup_period_days`
  - check_clean(1026-1073):`check_clean`
  - 内嵌测试(1075-2039,964 行,含 fixture helpers `init_repo`/`commit_all`/`setup_project_with_worker`/`backdate_dir`/日期 helpers)
- 目标(`git/worktree.rs` hub + `git/worktree/` 子目录):
  - `git/worktree.rs`(hub)— module 声明 + re-export 全部 `pub` 函数(保持 `git::worktree::X` 路径)
  - `git/worktree/naming.rs` — 4 个路径/分支名函数
  - `git/worktree/create.rs` — create 簇
  - `git/worktree/lifecycle.rs` — attach + destroy 簇
  - `git/worktree/sweep.rs` — worker sweep 簇
  - `git/worktree/check.rs` — check_clean
- **R1.3 re-export 关键约束**:`git/mod.rs` 现有 `pub use worktree::{check_clean, destroy as destroy_worktree}`。拆分后 hub `worktree.rs` 必须 `pub use` 从子模块把这两个符号 re-export 上来,保证 `git/mod.rs` 的 `pub use` 零改动。其余 11 个 `pub fn` 同理经 hub re-export。
- 测试:全部内嵌测试 → `git/tests_worktree.rs`(git/mod.rs 声明);fixture helpers 随迁(被多个测试用,同迁保持 `use super::*` 或在 tests 文件内定义)。被测私有 fn(`self_heal_for_create`/`create_worktree_add`)升 `pub(crate)`。
- 验证:`cargo test --lib "git::worktree"` + `cargo test --lib "tests_worktree"`。
- 预期 hub <100 行(纯 re-export + mod 声明)。

### 1.3 `db/memories.rs`(1781 行)→ `db/memories/` 目录模块

- 现状:5 簇 + 7 enum + 3 struct:
  - types(76-375):`MemoryScope`/`MemoryKind`/`MemoryStatus`(+3 impl)+ `MemoryRow`/`MemoryInput`/`MemoryInsertError`/`MemoryUpdateError`
  - validation(376-540):`generalize_home_path`/`find_sensitive_path`/`find_temporary_path`/`apply_safety_net`/`validate_memory_text`
  - crud(565-826):`insert_memory`/`get_memory_by_id`/`list_memories`/`delete_memory`/`count_memories_for_session`/`count_memories_by_scope_kind`
  - search/recall(828-1310):`RecallStatusFilter`/`search_memories_fts`/`build_recall_fts_query`(**pub(crate)**)/`search_memories_fts_recall`/`find_pitfalls_by_trigger`(+all_status 变体)/`escape_fts5`/`glob_matches_path`/`glob_match_inner`
  - lifecycle(1421-1734):`bump_hit_count`/`promote_if_eligible`/`update_memory`/`update_status`/`StatusTransitionError`
  - 内嵌测试(1735-1781,仅 46 行 `insert_raw` helper);重测试在 `db/memories_tests.rs`(2241 行,经 `db/mod.rs:60` 声明)
- 目标(`db/memories.rs` hub + `db/memories/` 子目录):
  - `db/memories.rs`(hub)— module 声明 + re-export 全部 `pub` 类型/函数(保持 `db::memories::X` 路径,因 memories_tests.rs 全路径引用)
  - `db/memories/types.rs` — 7 enum + 3 struct + impl
  - `db/memories/validation.rs` — 安全网簇
  - `db/memories/crud.rs` — CRUD 簇
  - `db/memories/search.rs` — search/recall/FTS 簇(含 `pub(crate) build_recall_fts_query`,保持可见性)
  - `db/memories/lifecycle.rs` — bump/promote/update 簇
- **可见性约束**:`build_recall_fts_query` 是 `pub(crate)`,有跨模块调用者;经 hub `pub(crate) use` re-export 保持。所有 `pub` 类型/函数经 hub re-export,`memories_tests.rs` 的 `crate::db::memories::MemoryRow` 全路径零改动。
- 测试:内嵌 46 行 helper 留原位或随迁;`memories_tests.rs` 不动(路径稳定)。被测私有 fn 升 `pub(crate)`。
- 验证:`cargo test --lib "db::memories"` + `cargo test --lib "memories_tests"`。
- 预期 hub <150 行(纯 re-export + mod 声明)。

## 2. 测试搬迁统一规则(延续 08-07 §1.6)

- 内嵌 `#[cfg(test)] mod tests` 全部迁出 → 同级 `tests_*.rs`(父 mod.rs 声明,`use super::*` 改 `use crate::<module>::*`)。
- 被测私有 fn 升 `pub(crate)` 或经 `pub(crate) use` re-export(R1.2);禁止改公开 API。
- fixture helper 跟随主测试迁(同文件内定义,保持内聚)。
- 既有外部 `memories_tests.rs`(2241 行)不搬,仅确认 import 路径稳定(hub re-export 保证)。

## 3. 文档引用同步(R2)

- 每次代码拆分落地后 sweep:`grep -rn "<旧路径/被搬符号>" docs/ .trellis/spec/ AGENTS.md`(排除 archive)。
- 已知关注点:`.trellis/spec/` 若引用 `skill/loader.rs`/`git/worktree.rs`/`db/memories.rs` 内具体函数行号,需更新为符号引用(行号必然漂移)。参照 08-07 的 AC3 教训:行号引用改符号引用,不写新行号。
- `db/memories_tests.rs` 的 import 不属文档,归 R1 处理。

## 4. 兼容性与风险

- **零运行时变化**:全部编译期搬迁;module 路径经 re-export 保持。
- **风险点**:
  - `git/mod.rs` re-export(R1.3)——拆分后若 hub re-export 遗漏,`git::check_clean` 调用点会编译失败。缓解:hub 用 `pub use` 显式列全。
  - `skill/loader.rs` 的 `SkillCache`(3 RwLock)跨簇共享——cache 簇整体搬迁,字段不暴露,风险低。
  - memories 的 `pub(crate)` 符号——hub re-export 层级要对(`pub(crate) use`,不是 `pub use`)。
- **不涉及**:前端、单体函数重构、sink/anthropic(已 Out of Scope)。
- **顺序**:loader(纠正前提、最干净)→ worktree(扁平最安全、验证 re-export 模式)→ memories(5 簇收尾、验证 pub(crate) re-export)。
