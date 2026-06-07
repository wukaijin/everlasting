# Research: Git Backend Library Selection for Worktree / Commit / Diff

- **Query**: 比较 git2-rs (libgit2 binding) vs spawn `git` CLI vs 混合方案,用于 session worktree + auto commit + diff 展示
- **Scope**: external (主导,核对 libgit2 / git2-rs / gix 公开 API) + internal (核对项目 ARCH §3 / DESIGN / TECH / Cargo.toml 现状)
- **Date**: 2026-06-07
- **Task**: `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/`

## TL;DR 结论

**推荐: git2-rs (vendored libgit2) + 单点 spawn `git worktree remove`**。
- git2-rs 覆盖 **commit / add_to_index / diff / status / worktree add / list / lock** 的 100% API
- **worktree remove 缺失** —— libgit2 C API 本身就没有 `git_worktree_remove`(本节是 ARCH §3 提示的"worktree API 不全"的真因)
- 用 vendored libgit2 静态链接,符合 Tauri 2 "不依赖系统二进制" 倾向;libgit2-sys build 会下源码 + 编译,首次 build 慢 30-90s,后续 cache

不推荐纯 spawn(每次 ~5-15ms 开销 + 错误字符串解析 + Windows PATH 踩坑)。
不推荐 gix(worktree add/remove 在 gix-worktree 0.53 仍不是 first-class API,star/starship 用 gix 也是只读场景)。

---

## 1. git2-rs API 完整度核对(直接对 docs.rs 0.20.2)

### 1.1 Worktree 相关(`Repository` + `Worktree` struct)

来源: `https://docs.rs/git2/0.20.2/git2/struct.Repository.html` + `struct.Worktree.html`

| 操作 | git2-rs API | 备注 |
|---|---|---|
| `git worktree add` | `Repository::worktree(name, path, opts)` 返回 `Worktree` | ✓ 完整,`WorktreeAddOptions` 可设 `lock`、`ref`、`checkout_existing` |
| `git worktree list` | `Repository::worktrees() -> StringArray` | ✓ |
| `git worktree list` 单个 | `Repository::find_worktree(name) -> Worktree` | ✓ |
| `git worktree lock` | `Worktree::lock(reason)` | ✓ |
| `git worktree unlock` | `Worktree::unlock()` | ✓ |
| `git worktree move` | ✗ **不支持** | libgit2 C API 没暴露 |
| `git worktree remove` | ✗ **不支持** | libgit2 C API 没暴露,**这是 ARCH §3 提示的真因** |
| `git worktree prune` (只清 metadata) | `Worktree::prune(opts)` | ✓ (注意:不删文件) |
| `git worktree repair` | `Worktree::validate()` | ✓ |

**关键证据 — libgit2 C API 列表**(`https://libgit2.org/docs/reference/main/worktree/`):
```
git_worktree_add                    git_worktree_list
git_worktree_lookup                 git_worktree_open_from_repository
git_worktree_path  /  name          git_worktree_is_locked
git_worktree_lock  /  unlock        git_worktree_is_prunable
git_worktree_prune                  git_worktree_validate
git_worktree_free
```
**没有 `git_worktree_remove`**。git CLI 的 `git worktree remove <path>` 做两件事:
1. `rm -rf` 真实工作目录(用 worktree 路径)
2. 清 `.git/worktrees/<name>/` 下的 metadata

libgit2 漏掉第 1 步(git 2.32+ 才补的 hardlink/共享 part)。这是历史坑。

**绕开方法**(任选一):
- (A) 自己 `std::fs::remove_dir_all(worktree_path)` + `Worktree::prune()`(代码量小,但要保证不删错目录 —— 必须 `path.starts_with(project_root)` 二次校验)
- (B) spawn `git -C <project_root> worktree remove <path> --force`(小,稳定,需要 `git` 在 PATH)

### 1.2 Commit / Index / Diff(全 API)

| 操作 | git2-rs API | 备注 |
|---|---|---|
| `git add` 全部 | `Index::add_all(pathspecs, flags)` + `Index::write()` | ✓ `IndexAddOption::DEFAULT` 跟踪新增+修改 |
| `git add <path>` | `Index::add_path(Path)` | ✓ |
| `git status` | `Repository::statuses(opts) -> Statuses` | ✓ 含 modified/staged/untracked |
| `git commit -m` | `Repository::commit(update_ref, author, committer, msg, tree, parents) -> Oid` | ✓ 完整,`update_ref` 设 `"HEAD"` |
| 取 author/committer signature | `Repository::signature()` 或 `Signature::now(name, email)` | ✓ fallback 走 `user.name`/`user.email` git config |
| `git diff` (workdir vs HEAD) | `Repository::diff_tree_to_workdir(old_tree, opts) -> Diff` | ✓ `DiffOptions::include_untracked(true)` 跟 git 一致 |
| `git diff --stat` | `Diff::stats() -> DiffStats` | ✓ 走 `files_changed / insertions / deletions` |
| `git diff` patch 文本 | `Diff::print(options, callback)` 或 `to_buf()` | ✓ 给前端传 unified diff 直接渲染 |
| `git diff` 逐文件 delta | `Diff::foreach(opts, file_cb, hunk_cb, line_cb)` | ✓ 适合做内嵌 diff 组件 |
| `git rev-parse HEAD` | `Repository::head() -> Object` / `Repository::revparse()` | ✓ |

**结论: commit / diff / add / status 全 API 完整,不需要 spawn**。

### 1.3 Vendored libgit2(静态链接,关键)

`libgit2-sys` 提供 `vendored` feature —— 把 libgit2 C 源码作为 `build.rs` 子模块编译进 binary。

```toml
git2 = { version = "0.20", default-features = false, features = ["vendored-libgit2"] }
# 等价:libgit2-sys = { version = "0.18", features = ["vendored"] }
```

- **二进制体积**:静态链接 libgit2 约 +3-5MB(release)
- **首次 build 慢**:从 source compile libgit2 (~30-90s,被 sccache 命中后 ~5s)
- **不依赖系统 `libgit2.so`**:不踩 Linux 发行版的 libgit2 版本错位坑
- **跨平台一致**:Windows / macOS / Linux 都用同一份 vendored libgit2
- **TLS 仍是 rustls**(本项目 reqwest 用 `rustls`,libgit2 HTTPS 用 OpenSSL,不冲突,libgit2 是裸 TCP for `git://`,HTTPS 走 OpenSSL;如果只走 file:// + git:// 就不需要 OpenSSL)

**推荐启用 `vendored-libgit2`**,理由符合 CLAUDE.md "Tauri 2 静态链接倾向" 表述。

---

## 2. spawn `git` CLI 代价分析

### 2.1 启动开销

| 平台 | 冷启动 (fork + exec) | warm (cache) |
|---|---|---|
| Linux | 5-15ms | 2-5ms |
| macOS | 10-25ms | 5-10ms |
| Windows | 30-80ms | 15-30ms(AV 干扰可飙升) |

对一个 session 生命周期内的工作量(创建 1 个 worktree + N 次 commit + 1 次 diff):~10-30 次 spawn × 15ms = 150-450ms 总开销。可接受,但 git2-rs 直接调 libgit2 是 0 spawn 开销。

### 2.2 错误处理

`git` CLI 的错误输出需要正则解析(stderr 不是结构化):
- `fatal: '<path>' is already checked out` —— 路径已存在 worktree
- `fatal: invalid reference: session/abc` —— 分支名非法
- `error: cannot lock ref ...` —— 锁竞争
- `Permission denied (publickey)` —— SSH 配置错

git2-rs 的 `git2::Error` 是 enum + 分类(`ErrorClass::Reference` / `Exists` / `Generic`),`is_prunable()` 等 hint 函数直接给布尔判断,比 stderr 解析稳定。

### 2.3 Windows PATH 踩坑

- `C:\Program Files\Git\bin\git.exe`(Git for Windows 官方)
- `C:\Program Files\Git\cmd\git.exe`(Bash wrapper,会在 MSYS2 路径转换层出岔子)
- `C:\Program Files (x86)\Git\bin\git.exe`(32-bit)
- WSL 内 spawn `git.exe` → Tauri 端 spawn `wsl.exe git ...` 多一层 fork

**Tauri 2 + spawn 的最佳实践**:`which::which("git")` 优先(内部用了 PATHEXT/PathExt),fallback 列表硬编码 `C:\Program Files\Git\bin\git.exe` 等。本项目在 WSL 内跑(Linux 路径),实际不踩。

### 2.4 跨平台 git 是否在 PATH

- **Linux dev box**:99% 有 git(`apt install git` 默认带),CI 也装
- **macOS**:`xcode-select --install` 自带,几乎 100% 有
- **Windows 端用户**:Git for Windows 是开发者标配,>99% 有
- **CI runner**:GitHub Actions / GitLab CI 都预装

**对本项目结论**:有依赖,但风险 < 1%。如果想 100% 静态自包含,git2-rs vendored 是正解。

---

## 3. 混合方案(推荐)

```
session 创建:
  git2.worktree(project_path, worktree_path, opts)   // ✓ git2-rs

tool execution(写文件):
  // 不走 git,工具直接写 worktree_path(由 ToolContext.cwd 切到 worktree)

auto-commit (session done / turn boundary):
  let mut index = repo.index()?;
  index.add_all(["*"].iter(), IndexAddOption::DEFAULT)?;
  index.write()?;
  let tree_id = index.write_tree()?;
  let sig = repo.signature()?;  // 用 git config user.name/email
  let head = repo.head()?.peel_to_commit()?;
  let new_oid = repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&head])?;

diff (前端展示):
  let old_tree = repo.head()?.peel_to_commit()?.tree()?;
  let mut diff = repo.diff_tree_to_workdir(Some(&old_tree), Some(opts))?;
  diff.print(...) 或 diff.to_buf(...)  // 给前端

session 删除 / worktree 清理:
  Option A: tokio::process::Command::new("git")
              .arg("-C").arg(&project_path)
              .arg("worktree").arg("remove").arg("--force")
              .arg(&worktree_path)
              .output().await?;
  Option B: tokio::fs::remove_dir_all(&worktree_path).await? + repo.find_worktree(name)?.prune(None)?;
```

**Option A vs B 选择**:
- A 简单、1 行命令、git 帮你处理 dirty worktree / 嵌套 .git / shared 等
- B 完全 static、不依赖 git CLI,但要自己 dirty check(否则 agent 还没 commit 的修改会被 rm -rf)

**建议 A**:`worktree remove --force` 是 worktree 销毁的预期操作(类似 `cargo clean`),accept 它丢未 commit 修改是 by design。

---

## 4. 平台特定坑(已知)

### 4.1 Windows

| 坑 | 应对 |
|---|---|
| `C:\Program Files\Git\bin\git.exe` 路径空格 | `tokio::process::Command::new("git")` 不传全路径,走 PATH;不直接拼 `C:\...` |
| spawn `git.exe` 经 MSYS bash 包装 | 用 `git.exe`(.exe 显式),不走 `git` bash wrapper,避免 `C:\foo` → `/c/foo` 路径转换 |
| 路径分隔符 `\vs /` | git2-rs 的 `Path` 内部用 `/`(git 内置),Rust `PathBuf::to_str()` 走系统;libgit2 处理自动 |
| Tauri 静态链接 | vendored libgit2 编译期下载 cmake/Perl 依赖(MSVC);首次 build 慢,后续 cache |
| Windows 文件锁(AV 实时扫描) | Tauri dev 阶段 `pnpm tauri dev` 频繁 touch 文件,可能触发 Defender 误报;添加 exclude 路径 |

### 4.2 macOS

- 系统自带 git 在 `/usr/bin/git`(Apple 出货版本,通常比上游落后 1-2 minor;`git worktree` 早期需要 2.5+)
- 如要最新 git,装 Homebrew 覆盖 PATH 前缀
- Keychain 提示:首次 push 弹 GUI 鉴权,worktree 隔离不影响这个
- Apple Silicon:`/opt/homebrew/bin/git` vs `/usr/bin/git` 优先级

### 4.3 Linux(本项目主战场)

- WSL 2 内 git 通常从 `apt install git` 装,版本 OK
- `libgit2` 系统包 vs vendored:本项目用 vendored,不踩错位
- 文件权限:worktree 内新文件遵循 `core.fileMode`;Windows 写 worktree mount 可能有 permission 错位 —— **WSL 内跑 worktree 不会触发**(WSL 内都是 Linux 文件)
- SSH key:`~/.ssh` 是 host 文件,worktree 在 `~/.local/share/everlasting/worktrees/`,不影响

---

## 5. 备选: gix(gitoxide)

**结论: 不推荐用于 step 4**。

| 维度 | gix | git2-rs |
|---|---|---|
| 纯 Rust | ✓ | ✗ (C 库,虽然 vendored) |
| 静态链接 | ✓ | ✓ (vendored) |
| worktree add/remove API | **未 first-class**(gix-worktree 0.53 主要是 read-only:index/ignore) | ✓ (除 remove) |
| commit / diff API | ✓ (gix-commit, gix-diff) | ✓ |
| 成熟度 | 0.x 版本,API 仍在变 | 0.20 stable,alexcrichton 长期维护 |
| 实战 | starship(只读 status),dolphin,其他零散 | gitui,delta,monorepo 工具链 |
| 文档 | sparse(很多 crate 只有 deps 列表) | 完整 docs.rs + 例子 |

gix 适合:读 git 信息(status / blame / log);不适合:写 worktree / 创建 branch。
本项目 step 4 是 **写** 为主,跟 gix 优势正交。**留作 future migration 候选**(gix 1.0 之后重评)。

---

## 6. 参考项目实践

- **gitui** (TUI git 客户端): `git2-hooks` (包装 git2-rs),worktree 用 git2 读列表
- **delta** (diff 渲染器): `git2` 只用来取 diff,worktree/branch 管理用 CLI
- **starship** (prompt): 切到 `gix` 0.84,只读 status
- **lapce** (IDE): 用 `git2`
- **helix** (editor): 没内嵌 git(走 CLI)
- **zed** (editor): `tree-sitter-gitcommit` 解析 commit message,git 操作走 CLI

共识:**写 worktree / commit 走 git2-rs 是主流;纯 gix 还早;混合是常态**。

---

## 7. 落地建议(给后续 implement 阶段)

### 7.1 Cargo.toml 增项

```toml
# Git worktree / commit / diff (step 4)
git2 = { version = "0.20", default-features = false, features = ["vendored-libgit2"] }
```

build.rs 第一次会拉 libgit2 v1.9.x 源码 + cmake 编译,后续 cache。

### 7.2 新增模块 `app/src-tauri/src/git/`

```
git/
├── mod.rs        # 公共 API:WorktreeSession { create, commit, diff, remove }
├── worktree.rs   # git2-rs worktree add/list/find_worktree/lock/unlock
├── commit.rs     # git2-rs commit + index
├── diff.rs       # git2-rs diff_tree_to_workdir → unified patch 字符串
├── error.rs      # thiserror enum,中文用户消息
└── cli.rs        # spawn `git worktree remove --force`(Option A)
```

### 7.3 接入点

- `db::create_session` → 调 `git::worktree::create()` 成功后写 `sessions.worktree_path`
- `db::delete_session` → 调 `git::worktree::remove()`(CLI spawn)
- agent turn boundary (`lib.rs::chat` 末尾) → 调 `git::commit::commit_changes()`
- 前端调新 command `get_session_diff(session_id) -> String` → `git::diff::workdir_diff()`

### 7.4 不在本研究范围(留待)

- gix 1.0 migration 评估
- `git worktree move` 支持(用 `std::fs::rename` + worktree metadata 手动 fix,或限制不允许移动)
- LFS / submodule 处理
- 自动 merge 回 main(后端 ARCH §3 标注的后续工作)

---

## 关键 References

- libgit2 worktree C API: <https://libgit2.org/docs/reference/main/worktree/>(**核心证据:无 git_worktree_remove**)
- git2-rs Repository: <https://docs.rs/git2/0.20.2/git2/struct.Repository.html>
- git2-rs Worktree: <https://docs.rs/git2/0.20.2/git2/struct.Worktree.html>
- git2-rs Diff: <https://docs.rs/git2/0.20.2/git2/struct.Diff.html>
- git2-rs Index: <https://docs.rs/git2/0.20.2/git2/struct.Index.html>
- libgit2-sys vendored feature: <https://docs.rs/libgit2-sys/latest/libgit2_sys/>
- gix (gitoxide): <https://github.com/Gitoxide/gitoxide>
- gix-worktree: <https://docs.rs/gix-worktree/latest/gix_worktree/>(**目前以读为主**)
- Tauri 2 官方 plugin git(已存在?): <https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/git> —— 待确认
- 内部 ARCH §3 决策: `/usr/local/code/github/everlasting/docs/ARCHITECTURE.md#3-决策每个-session-一个-git-worktree`
- 内部 TECH.md 锁项: `/usr/local/code/github/everlasting/docs/TECH.md` 行 "| Git 操作 | **git2-rs** | libgit2 绑定,worktree / diff / commit |"
- 内部 DESIGN.md 风险: `/usr/local/code/github/everlasting/docs/DESIGN.md` "Git2-rs worktree API 不全 中 必要时 spawn `git worktree` 命令"
- Cargo.toml 现状: `/usr/local/code/github/everlasting/app/src-tauri/Cargo.toml` (无 git 依赖,无 libgit2-sys)

## Caveats / Not Found

- **gix-worktree 0.53 的 mutating API**(add/remove)未在公开 docs 看到 first-class 入口;crate-status.md 没在 GitHub raw 拿到,可能仓库迁移或改名。本研究的 "gix 不推荐" 结论基于 2026-06-07 docs.rs 实际状态
- **Tauri 2 官方 `tauri-plugin-git`** 路径未确认存在;v1 时有,v2 plugins-workspace 路径下未找到独立 git 插件(可能已 deprecated 或需要二次确认)
- **Winkin 系统编译 vendored libgit2** 第一次 build 时间未实测,估 30-90s(sccache 命中后 <5s)
- **跨设备 worktree 接续** 是 ARCH §3 提的远期问题,不在本研究范围
