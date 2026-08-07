# Implement — 大文件拆分批 2:loader / worktree / memories

> 执行计划。每步独立 commit、独立回滚。验证命令:后端
> `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib "<filter>"`
> (不要 `--test-threads=1`)。

## Phase 0:前置确认

- [ ] 确认在分支 `refactor/file-splitting-batch2`,工作树干净
- [ ] 确认 base 是 main 最新(`git log --oneline -1 main` 应为 `7b60b55` squash 或更新的 archive commit)

## Phase 1:skill/loader.rs 拆分(第一优先)

- [ ] 1.1 建 `skill/loader/` 目录,按 design §1.1 切出 `frontmatter.rs`/`paths.rs`/`scan.rs`/`cache.rs`(复制函数体,原样不改逻辑)
- [ ] 1.2 `skill/loader.rs` 改为 hub:删已迁出函数体,加 `mod frontmatter; mod paths; ...` + `pub(crate) use` re-export
- [ ] 1.3 被测私有 fn(`parse_frontmatter`/`apply_kv`/`scan_skill_dir` 等)升 `pub(crate)`
- [ ] 1.4 内嵌测试(852 行)→ `skill/tests_loader.rs`(`skill/mod.rs` 加 `#[cfg(test)] mod tests_loader;` 或同级声明),import 改 `use crate::skill::loader::*`
- [ ] 1.5 验证:`cargo test --lib "skill::loader"` + `cargo test --lib "tests_loader"` 全绿
- [ ] 1.6 行数核对:`wc -l skill/loader.rs skill/loader/*.rs` — hub <1200,各子模块 <1200
- [ ] 1.7 commit:`refactor(skill): loader 拆 frontmatter/paths/scan/cache + 测试迁出`

**回滚点**:`git revert <1.7>`。

## Phase 2:git/worktree.rs 拆分

- [ ] 2.1 建 `git/worktree/` 目录,按 design §1.2 切出 `naming.rs`/`create.rs`/`lifecycle.rs`/`sweep.rs`/`check.rs`
- [ ] 2.2 `git/worktree.rs` 改为 hub:`mod` 声明 + `pub use` 把全部 12 个 `pub fn` + 1 `pub async fn` re-export 上来(**R1.3**:`check_clean` 和 `destroy` 必须在列,因 `git/mod.rs` 有 `pub use worktree::{check_clean, destroy as destroy_worktree}`)
- [ ] 2.3 被测私有 fn(`self_heal_for_create`/`create_worktree_add`)升 `pub(crate)`
- [ ] 2.4 内嵌测试(964 行,含 fixture helpers)→ `git/tests_worktree.rs`(`git/mod.rs` 声明)
- [ ] 2.5 验证:`cargo test --lib "git::worktree"` + `cargo test --lib "tests_worktree"` 全绿
- [ ] 2.6 **R1.3 专项**:`cargo check` 确认 `git::check_clean`/`git::destroy_worktree` 调用点零改动
- [ ] 2.7 行数核对:hub <1200,各子模块 <1200
- [ ] 2.8 commit:`refactor(git): worktree 拆 naming/create/lifecycle/sweep/check + 测试迁出`

**回滚点**:`git revert <2.8>`。

## Phase 3:db/memories.rs 拆分

- [ ] 3.1 建 `db/memories/` 目录,按 design §1.3 切出 `types.rs`/`validation.rs`/`crud.rs`/`search.rs`/`lifecycle.rs`
- [ ] 3.2 `db/memories.rs` 改为 hub:`mod` 声明 + `pub use` re-export 全部 pub 类型/函数 + `pub(crate) use build_recall_fts_query`(保持可见性)
- [ ] 3.3 被测私有 fn 升 `pub(crate)`
- [ ] 3.4 内嵌 46 行 helper(`insert_raw`)随最相关簇迁或留原位;`memories_tests.rs`(2241 行)**不动**,确认 import 路径稳定
- [ ] 3.5 验证:`cargo test --lib "db::memories"` + `cargo test --lib "memories_tests"` 全绿
- [ ] 3.6 行数核对:hub <1200,各子模块 <1200
- [ ] 3.7 commit:`refactor(db): memories 拆 types/validation/crud/search/lifecycle`

**回滚点**:`git revert <3.7>`。

## Phase 4:文档引用同步(R2)+ 终验

- [ ] 4.1 sweep:`grep -rn "skill/loader.rs\|git/worktree.rs\|db/memories.rs" docs/ .trellis/spec/ AGENTS.md`(排除 archive),更新失效路径/行号 → 符号引用
- [ ] 4.2 全量终验:`PKG_CONFIG_PATH="..." cargo test --lib`(全绿,~1657)
- [ ] 4.3 `cargo clippy --lib`(零警告)+ `cargo fmt --check`(零差异)
- [ ] 4.4 AC 逐项核对(AC1-5)
- [ ] 4.5 commit(若有文档改动):`docs: 同步 loader/worktree/memories 拆分后引用`

## 验证命令速查

```bash
# 单模块过滤测试(避免全量)
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib "skill::loader"

# 全量终验
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib

# clippy + fmt
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo clippy --lib && cargo fmt --check

# 行数核对
wc -l app/src-tauri/src/skill/loader.rs app/src-tauri/src/skill/loader/*.rs
wc -l app/src-tauri/src/git/worktree.rs app/src-tauri/src/git/worktree/*.rs
wc -l app/src-tauri/src/db/memories.rs app/src-tauri/src/db/memories/*.rs
```
