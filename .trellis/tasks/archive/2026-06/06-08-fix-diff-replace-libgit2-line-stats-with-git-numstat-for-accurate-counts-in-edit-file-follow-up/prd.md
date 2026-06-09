# PRD — fix(diff): replace libgit2 line_stats with git numstat for accurate +/- counts

## Background

Step 4 后续（Bug 2）：agent 用 `edit_file` 在文件中间或末尾插入 N 行后，前端 diff 卡片同时显示 `+N` 和 `-M`（M>0）。用户主观只新增了行，期望 `+N/-0`，但 libgit2 的 `Patch::line_stats()` 对 `diff_tree_to_workdir_with_index` 已知 under-count / 错报（见 `app/src-tauri/src/git/diff.rs:431-439` 的注释，典型场景是 `"v1\n" → "v2\n"` 时返回 `added: 0, removed: 1`）。

现场确认（2026-06-08）：bug 出现在「插入 N 行」场景，分支 (a)「整文件重写」不成立 —— `edit_file` 已经是 in-place 字符串替换（`edit_file.rs:202-219`），无法通过改 edit_file 实现来回避；走分支 (b)：用 `git diff --numstat` 替代 `line_stats` 作为 added/removed 的真实数据源。

## Goal

- `diff_worktree` 返回的 `FileDiff.added` / `FileDiff.removed` 在常规插入 / 替换场景下与 `git diff --numstat` 一致
- 旧测试 (`diff_worktree_modified_tracked_file_unchanged`) 由「断言 diff_text」升级为「断言 added/removed 数字」（line_stats 修好后，line_stats 数字也应对）
- 新增测试 pin 住「插入 N 行」场景下 `added=N, removed=0` 这条不变量

## Non-goals

- 不动 `FileDiff` / `DiffResult` 的 public API（前端 diff 卡片结构不变）
- 不动 `diff_text` 字段的内容（libgit2 渲染的 unified diff 是正确的，UI 一直在用）
- 不解决 untracked 文件的 stat（已经有自己的 `build_untracked_diff` 按行数计，不在本次 scope）
- 不把 `git` 升级成硬依赖：若 `git diff --numstat` 失败（git 不在 PATH、subprocess error），fallback 到旧的 `line_stats`，保持现有行为

## Design

在 `app/src-tauri/src/git/diff.rs` 新增 private helper：

```rust
fn git_numstat(worktree: &Path, path: &str) -> Result<(usize, usize), std::io::Error>
```

实现要点：
- `Command::new("git").args(["diff", "--no-color", "--numstat", "HEAD", "--", path]).current_dir(worktree).output()`
- 用 `HEAD` 显式指定 base（等价于 libgit2 的 `diff_tree_to_workdir_with_index(Some(&base_tree), None)` —— worktree 里 HEAD 就是 session branch tip）
- 输出每行格式：`<added>\t<removed>\t<path>`；binary 文件两列是 `-`
- 返回首个非空解析结果（每次只查一个 path，最多一行）
- subprocess 失败 / parse 失败 → 返回 `Err`，让调用方 fallback

在 `diff_worktree` 现有的 `git2::Patch::from_diff(...)` 分支里，把：
```rust
let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
```
替换为：
```rust
let (a, d) = match git_numstat(worktree_path, &path) {
    Ok(v) => v,
    Err(e) => {
        tracing::warn!(path = %path, error = %e,
            "diff_worktree: git numstat failed, falling back to libgit2 line_stats");
        let (a, d, _) = patch.line_stats().unwrap_or((0, 0, 0));
        (a, d)
    }
};
```

## Tests

### 1. 加强现有 `diff_worktree_modified_tracked_file_unchanged` (diff.rs:440-474)

旧测试只断言 `diff_text` 含 `-v1` / `+v2`。改写为同时断言：
- `file.added == 1`
- `file.removed == 1`
- `file.status == "modified"`
- 保留 `diff_text` 的两条断言

### 2. 新增 `diff_worktree_insert_lines_purely_added`（pin 住 bug 修复）

```text
// 初始: a.txt = "line1\nline2\nline3\n"
// commit, make worktree
// worktree 里 edit_file-style append: a.txt = "line1\nline2\nline3\ninserted1\ninserted2\n"
// 断言: file.added == 2, file.removed == 0, file.status == "modified"
```

这正是用户报告的「插入 N 行」场景；通过 = bug 修复落地。

### 3. (可选) 新增 `diff_worktree_numstat_unavailable_falls_back_to_line_stats`

- 把 `git` 临时从 PATH 移除（或用 `Command::new` 注入一个永远失败的 stub）
- 跑现有 modified 场景，断言 `line_stats` 仍能给出非空结果（不要求数字正确，只要求不 crash、`(0,0,0)` 不会发生）

考虑到 CI 环境 git 一定可用，这条可以先放 fall-back 列表，等第一次实际遇到 git 缺失再做。本期不做。

## Risk

- **subprocess 成本**：每次 `diff_worktree` 调用对 N 个 changed file 各起一次 `git` 进程。最坏情况（session 改了 50 个文件）= 50 次 git 启动 ~ 50×5ms = 250ms。Step 4 之前没有过 diff 调用，step 4 也是 on-demand，不会高频触发。可接受。如果未来成瓶颈，再考虑 `git diff --numstat` 一次拿全。
- **git 不可用**：fallback 到 line_stats，行为同修复前。最差情况是 bug 没修，但不引入新 crash。
- **rename 路径**：numstat 在旧 git 上 output 是 `{old => new}`，新 git 是 `new`。我们只解析前两列，不依赖 path 字段，无影响。
- **worktree 不在 git 内部**：`git diff` 会失败，fallback 路径触发。Pre-step-4 session 在 `lib.rs:321-333` 已经提前 return empty result，不会走到这里。

## Open questions

无（用户已确认走「改 diff.rs: line_stats → git apply --numstat」分支）。
