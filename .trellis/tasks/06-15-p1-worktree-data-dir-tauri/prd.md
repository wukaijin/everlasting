# PRD: P1 worktree data_dir → Tauri app_data_dir (RULE-E-006)

> **DEBT 引用**: [.trellis/reviews/DEBT.md §RULE-E-006](../../../.trellis/reviews/DEBT.md)
> **DEBT 编排**: PR14,独立无依赖
> **类型**: bugfix(纯后端 Rust,零前端牵连,零 test 牵连)
> **优先级依据**: DEBT.md 文末点名「前移 RULE-E-006 工作数据丢失风险后建议尽快」

---

## 0. 给接手 agent 的话

这份 PRD 是**自包含交接文档**。所有 research(调用点分析、迁移评估、精确代码位置)已完成并固化在下面。你**不需要再 grep / 探索代码库**,按 §5 改动清单逐条执行即可。

实施完成后:
- **不要 git commit**(项目约定:实施 agent 不 commit,留给 carlos review + commit)
- 跑 §8 验证命令,把结果贴出来
- 按 §11 更新 DEBT.md 状态(否则债不闭环)
- 不确定时回到本 PRD 的 §6 决策点 + §10 风险

---

## 1. 问题陈述 + 根因

`app/src-tauri/src/git/worktree.rs:40-56` 的 `data_dir()` 三段式解析 app data 目录:

```
XDG_DATA_HOME/everlasting  →  HOME/.local/share/everlasting  →  /tmp/everlasting (fallback)
```

**`/tmp` fallback 在重启后被系统清空 = 用户 worktree 工作数据丢失。** 这是确定性行为(`tmpfs` / systemd-tmpfiles),非理论风险。

**根因**:同一个 app 两条持久化路径算法不一致 —— DB 早就用 Tauri `app_data_dir()`(`state.rs:156-160`),worktree 却用 env-based。代码注释(`worktree.rs:36-39`)已自认这是 TODO:

> Cross-platform will be added when we ship to Windows / macOS — the right primitive there is Tauri's `app.path().app_data_dir()` rather than `std::env::var`.

DEBT meta-review 把它从 P3 升级到 P1。

---

## 2. Research findings(已代劳,无需复做)

### 2.1 路径不一致(根治动机)

| 对象 | 当前路径 | 来源 |
|---|---|---|
| SQLite DB | `~/.local/share/com.wukaijin.everlasting/everlasting.db` | `state.rs:156-160` `app_data_dir()` ✅ |
| worktree | `~/.local/share/everlasting/worktrees/<project>/<session>` | `worktree.rs:40` env/home/`/tmp` ❌ |

- **identifier**(`tauri.conf.json:5`): `com.wukaijin.everlasting`
- Tauri Linux `app_data_dir()` = `$XDG_DATA_HOME/com.wukaijin.everlasting/` 或 `~/.local/share/com.wukaijin.everlasting/`
- 根治后 worktree 落在 `~/.local/share/com.wukaijin.everlasting/worktrees/`,跟 DB **同根**
- `app_data_dir` 在 `AppState::load`(`state.rs:156-159`)已算出,**当前仅用于拼 `db_path`,没存进 struct** —— 这是修复的关键支点

### 2.2 调用点(修复面极小)

```
git::data_dir()             唯一 production 调用 = commands/worktree.rs:66 (attach_worktree)
git::session_worktree_path  唯一调用 = commands/worktree.rs:67 (纯函数,接 data_dir 参数,不改)
```

- **destroy 路径(detach/delete)不重算 data_dir** —— 用 DB `sessions.worktree_path` 列存的绝对路径
- **`data_dir()` 未被任何 test 直调**(grep `#[cfg(test)]` 模块无命中)
- **session create 不自动建 worktree**(`commands/sessions.rs:47-51` 明确注释 "worktree is now opt-in... no longer auto-create")
- → 改完后 `data_dir()` **production 零调用 + test 零调用 = 纯死代码**,可安全删除

### 2.3 迁移影响(开发期 ≈ 0)

路径 `~/.local/share/everlasting/worktrees/` → `~/.local/share/com.wukaijin.everlasting/worktrees/`。

- worktree opt-in + session-bound(destroy 即 `remove_dir_all` + `prune`),无跨 session 长期数据
- 已 attach session 的 DB 存旧 `worktree_path`,detach/destroy 用 DB 路径不重算,**不受影响**
- attach 是新建 worktree(`create_worktree`),不依赖旧目录,**也不失效**
- 唯一残留:旧目录占盘 → 手动 `rm -rf ~/.local/share/everlasting/worktrees/` 即可

---

## 3. 方案概述

`AppState` 加一个 `app_data_dir` 字段(把 `state.rs:156` 已算的值存下来),`attach_worktree` 从 state 取,然后删掉 env-based `git::data_dir()`。worktree 从此跟 DB 同根,`/tmp` 数据丢失路径消失。

`worktree_path(data_dir, project_id, session_id)`(`worktree.rs:64`)是纯函数,接受 `data_dir` 参数,**不改**。

---

## 4. ✅ 精确改动清单(6 处,按此执行)

> 行号是 2026-06-15 快照,执行时以实际为准(代码近期稳定,不会大幅漂移)。

### 改动 1 — `state.rs` AppState struct 加字段

**文件**: `app/src-tauri/src/state.rs`
**位置**: struct 定义 `:68-123`,在 `pub db: SqlitePool,`(`:76-78`)之后插入一个新字段。

**上下文(当前)**:
```rust
    pub db: SqlitePool,
    /// Grill decision #3: pre-built provider catalog keyed by
```

**改成**:
```rust
    pub db: SqlitePool,
    /// RULE-E-006: Tauri-resolved app data dir. Same root as the
    /// SQLite db above; worktree storage lives under
    /// `<app_data_dir>/worktrees/<project_uuid>/<session_uuid>`.
    /// Replaces the old env-based `git::data_dir()` whose `/tmp`
    /// fallback risked data loss on reboot.
    pub app_data_dir: std::path::PathBuf,
    /// Grill decision #3: pre-built provider catalog keyed by
```

> 用 `std::path::PathBuf` 全路径,**不要动 state.rs 顶部的 use 块**(最小改动)。

### 改动 2 — `state.rs` load() 构造处赋值

**文件**: `app/src-tauri/src/state.rs`
**位置**: `Self {...}` 构造块 `:227-237`。`app_data_dir` 局部变量已在 `:156-159` 定义(`let app_data_dir = app.path().app_data_dir().expect(...)`),直接 move 进字段。

**上下文(当前)**:
```rust
        Self {
            config,
            tools,
            db,
            catalog: Arc::new(RwLock::new(catalog)),
```

**改成**(在 `db,` 后加一行):
```rust
        Self {
            config,
            tools,
            db,
            app_data_dir,
            catalog: Arc::new(RwLock::new(catalog)),
```

> 注意:`app_data_dir` 在 `:156` 之后还被 `:160` `app_data_dir.join("everlasting.db")` 用过(只读借用,move 在最后构造时),不影响。如果编译器报 "used after move",把 `:160` 改成先 `let db_path = app_data_dir.join(...)` 之后再无引用即可 —— 但按现状顺序(join 是借用,不 move),构造时 move 是安全的。

### 改动 3 — `commands/worktree.rs` attach_worktree 改取值

**文件**: `app/src-tauri/src/commands/worktree.rs`
**位置**: `:66`

**当前**:
```rust
    let data_dir = git::data_dir();
    let wt_path = git::session_worktree_path(&data_dir, &project.id, &session_id);
```

**改成**:
```rust
    let data_dir = state.app_data_dir.clone();
    let wt_path = git::session_worktree_path(&data_dir, &project.id, &session_id);
```

> `state: State<'_, Arc<AppState>>` 已在函数签名(`:20-21`),无需加 `AppHandle` 参数。`:67` 的 `session_worktree_path` 调用不变。

### 改动 4 — `worktree.rs` 删 data_dir() 函数 + docstring

**文件**: `app/src-tauri/src/git/worktree.rs`
**位置**: `:34-56`(连同上面的 docstring 一起删)

**删除整块**(`:34-56`):
```rust
/// Compute the platform-appropriate app data dir for our worktrees.
///
/// WSL/Linux first (the project's primary dev target per
/// `docs/HACKING-wsl.md`). Cross-platform will be added when we
/// ship to Windows / macOS — the right primitive there is
/// Tauri's `app.path().app_data_dir()` rather than `std::env::var`.
pub fn data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("everlasting");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local").join("share").join("everlasting");
        }
    }
    // Last-resort fallback. Should not happen on supported platforms.
    tracing::warn!(
        "neither XDG_DATA_HOME nor HOME is set; falling back to /tmp/everlasting"
    );
    PathBuf::from("/tmp/everlasting")
}
```

> 删完后检查 `worktree.rs` 顶部 `use std::path::{Path, PathBuf};`(`:22`)—— `PathBuf` 可能不再被本文件使用(只剩 `worktree_path` 用 `&Path` 参数和返回 `PathBuf`)。`worktree_path` 返回 `PathBuf`,所以 `PathBuf` 仍需要,**use 不动**。若 `cargo check` 报 unused import 再处理(预期不需要)。

### 改动 5 — `git/mod.rs` 删 re-export

**文件**: `app/src-tauri/src/git/mod.rs`

**5a. 删 re-export**(`:29-31`):

当前:
```rust
pub use worktree::{
    check_clean, create as create_worktree, data_dir, destroy as destroy_worktree,
    worktree_path as session_worktree_path,
};
```
改成(去掉 `data_dir,`):
```rust
pub use worktree::{
    check_clean, create as create_worktree, destroy as destroy_worktree,
    worktree_path as session_worktree_path,
};
```

**5b. 删 module docstring 行**(`:12`):

当前:
```rust
//! - [`data_dir`]: XDG-compliant app data dir for worktree storage.
```
直接删这一行(`worktree_path` 那行 `:14-15` 保留)。

### 改动 6(可选,推荐)— `worktree.rs` worktree_path docstring 措辞

**文件**: `app/src-tauri/src/git/worktree.rs`
**位置**: `:58-66` `worktree_path` 的 docstring

当前提到"data_dir"。语义不变(app_data_dir join "worktrees"),但措辞可顺带更新,避免读者疑惑 data_dir 从哪来。可选,不改也能编译。建议把 `:59` "Layout: `<data_dir>/worktrees/...`" 改成 "Layout: `<app_data_dir>/worktrees/...`"。

---

## 5. 决策点(均已默认,执行时遵循;若 carlos 另有指示再推翻)

| ID | 决策 | 默认 | 理由 |
|---|---|---|---|
| D1 | 删 `data_dir()` vs 保留 deprecated | **删** | DEBT 反对 env-based;改完零调用,留着是 footgun |
| D2 | 旧路径 worktree 是否迁移 | **不迁移** | 开发期 opt-in + session-bound,无价值数据;已 attach 用 DB 旧路径不受影响 |
| D3 | 字段类型写法 | `std::path::PathBuf` 全路径 | 最小改动,不动 use 块 |

---

## 6. 实施顺序

1. 改动 1(struct 加字段)
2. 改动 2(load 赋值)—— 此时 `app_data_dir` 字段已就位
3. 改动 3(attach_worktree 切到 state 取)
4. 改动 4 + 5(删 data_dir + re-export + docstring)—— 此时 `git::data_dir` 不再存在
5. 改动 6(可选 docstring)
6. §8 验证

> 顺序 1→2→3→4 保证任何中间点 `cargo check` 都不会留下"用了已删函数"的悬空引用。若先删(4)再改(3)会临时编译失败,不推荐。

---

## 7. 验证命令

> **PKG_CONFIG_PATH 必填**(见 CLAUDE.md "WSL 环境"块),否则撞 `gdk-pixbuf-2.0 not found`。

```bash
cd app/src-tauri

# 1. 编译检查(0 warning 是硬指标)
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check

# 2. 单元测试全 pass(含 9 个 agent_loop_* + worktree 相关,484+ tests)
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib

# 3. 确认 /tmp fallback 已消失(grep 应无命中)
grep -rn "/tmp/everlasting\|git::data_dir\|fn data_dir" src/

# 4. (可选)前端 type-check 不受影响(纯后端改动),跳过
```

**WSL 运行时验证**(需要真实 Tauri 环境 + GUI,carlos 做):
```bash
cd app && pnpm tauri dev
# 起一个 session → 对 git project attach worktree
# 确认落点在 ~/.local/share/com.wukaijin.everlasting/worktrees/<project>/<session>/
ls ~/.local/share/com.wukaijin.everlasting/worktrees/
```

---

## 8. Definition of Done

- [ ] `/tmp` fallback 从代码消失(`grep -rn "/tmp/everlasting" src/` 无命中)
- [ ] worktree 与 DB 同根(统一 `app_data_dir()` 来源)
- [ ] `git::data_dir()` env-based free function + re-export 已删
- [ ] `cargo check` **0 warning**
- [ ] `cargo test --lib` 全 pass(不新增失败)
- [ ] §11 DEBT.md / ROADMAP 文档更新

---

## 9. 风险 + 回滚

| 风险 | 概率 | 缓解 |
|---|---|---|
| `state.rs:156` `app_data_dir` 被 move 后 `:160` 再用报错 | 低 | `:160` 是 `join` 借用,不 move;若报错则把 db_path 算完再进构造块 |
| WSL `app_data_dir()` 解析异常 | 低 | DB 已用同 API 数月无问题,间接证明;§7 步骤 4 实测 |
| `PathBuf` use 误删导致 worktree.rs 编译失败 | 低 | `worktree_path` 仍返回 `PathBuf`,use 保留;`cargo check` 会立刻报 |
| 旧 worktree 目录残留占盘 | 极低 | 手动 `rm -rf ~/.local/share/everlasting/worktrees/` |

**回滚**:改动全是局部增删,`git checkout -- app/src-tauri/src/{state.rs,commands/worktree.rs,git/worktree.rs,git/mod.rs}` 即恢复。无 schema migration,无 DB 变更,回滚零成本。

---

## 10. 交付后要更新的文档(闭环)

实施 + 验证通过后,**必须**更新这些,否则债不闭环:

1. **`.trellis/reviews/DEBT.md` §RULE-E-006**:
   - `Status: open` → `Status: **closed (2026-06-XX)** — <一句话修复说明>`
   - 填 `Closed At: <commit hash>`(commit 由 carlos 填)
   - 填 `Related Task: .trellis/tasks/06-15-p1-worktree-data-dir-tauri`
2. **`.trellis/reviews/DEBT.md` Re-evaluation Log** 末尾加一行:
   `| 2026-06-XX | RULE-E-006 | open | **closed** | worktree data_dir 从 env/home/tmp 改 Tauri app_data_dir,对齐 DB,消除 /tmp 数据丢失 | .trellis/tasks/06-15-p1-worktree-data-dir-tauri |`
3. **`docs/ROADMAP.md` §1.2** 补一行(worktree 路径对齐,日期 06-15)—— 可选,跟其他 P1 修复归类
4. **`.trellis/tasks/06-15-p1-worktree-data-dir-tauri/task.json`** `status` → `completed` + 填 `completedAt` + `commit`

---

## 11. 不在本任务范围(明确边界)

- ❌ RULE-E-005(worktree destroy await cancel)—— 另一个独立 P1,本任务不碰
- ❌ RULE-E-011(worktree create self-heal remove_dir_all)—— P2,不碰
- ❌ 跨平台(Windows/macOS)`app_data_dir()` 实测 —— 本项目 WSL-first,Windows/macOS 部署时再做
- ❌ 旧 worktree 目录迁移脚本 —— D2 决定不迁移
