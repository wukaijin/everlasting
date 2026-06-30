# Design — A+B: worker auto-commit + isolation 系统层自动决定

> Task: `06-30-ab-autocommit-shared-default` · Parent: `06-30-subagent-worktree-smooth`

## 1. 范围与边界

**改动文件**
- `agent/subagent/dispatch.rs` — A（auto-commit）+ B（`run_subagent` 加 `parallel` 参、新增 `worker_is_writable` helper、决策逻辑）。**`resolve_isolation` 不改签名。**
- `agent/subagent/mod.rs` — B（`general-purpose` isolation `Some(true)`→`None`；`dispatch_subagent` tool description 改写）
- `agent/chat_loop.rs` — B（`classify_dispatch_batch` 的 `Concurrent` / `Serial` 两个 `run_subagent` 调用点接线）
- `git/worktree.rs`（或 `git/mod.rs`）— A（新增 `commit_worker_changes` helper）

**不改**
- `agent/subagent/dispatch.rs::resolve_isolation` — 签名不变，旧 truth table 测试全保留
- `tools/merge_worker.rs` — A 让它「正确工作」，不改它的 FF/3-way 逻辑
- C（delegation 注入）/ D（publish UI）→ child2

## 2. A — worker auto-commit 兜底

**插入点**：`run_subagent` 中 `probe_worker_changes` 返回 `has_changes=true` 之后、保留 worktree 决策之前（`dispatch.rs:955-957` 附近）。

**实现**：新增 git2 helper

```rust
// git/worktree.rs
pub fn commit_worker_changes(worker_wt: &Path, run_id: &str) -> Result<git2::Oid, String>
```

- `Repository::open(worker_wt)`（与 `merge_worker.rs:414` open parent 同理，对 linked worktree 适用）
- `index.add_all(["*"], Default::default(), None)` + `index.write()` —— stage 全部 tracked + untracked 改动
- `tree_oid = index.write_tree()`
- `sig = Signature::now("Everlasting", "agent@everlasting")`
- `head = repo.head()?.peel_to_commit()`（worker 分支当前 tip）
- `repo.commit(Some("refs/heads/worker/<run_id>"), &sig, &sig, "worker <run_id>: auto-commit worker changes", &[&head], &tree)`
- 借鉴 `merge_worker.rs:605-630` 的 commit 模式

**失败处理**：commit 失败 → `tracing::warn!` + **保守保留 worktree**（与 `probe_worker_changes` 失败时的 fallback 一致，`dispatch.rs:189-192`），不阻断。罕见；此时 merge 行为退回旧路径。

**关键不变量**：`has_changes=true` 时，auto-commit 保证 `worker_tip` 真领先于 `parent_tip`（base）。从而 `do_merge_blocking` 的 `is_ancestor(parent, worker)` 命中真 FF 或 3-way，不再命中 `==` 空短路（`merge_worker.rs:651`）。

## 3. B — isolation 系统层 serial/parallel 自动决定（写型才隔离）

**3.1 `run_subagent` 加参数**（紧挨现有 `force_readonly: bool`，`dispatch.rs:252` 附近）：

```rust
parallel: bool,   // true → 本次 dispatch 来自并发批次（DispatchBatch::Concurrent）
```

**3.2 新增 helper**：

```rust
// dispatch.rs
fn worker_is_writable(def: &SubagentDef) -> bool
```

判定 def 的 toolset 是否含写工具：`tools.is_empty()`（继承全部 = 含写）→ `true`；含任一写工具 → `true`；纯只读 toolset（如 researcher 的 read_file/grep/glob/list_dir/web_fetch）→ `false`。复用 `filter_tools_readonly`（`dispatch.rs:448`）的只读集做差集判定（或基于 permissions 层 `ToolKind`）。

**3.3 决策逻辑**（`dispatch.rs`，最终实现版 — 实现中修正过一次）：

```rust
let isolated = if force_readonly {
    false                                  // serial-only 开关，最高优先级（L3a legacy）
} else if let Some(explicit) = dispatch_isolation {
    explicit                               // 显式 dispatch input 总是赢（含 isolation:false opt-out）
} else if parallel && worker_is_writable(def) {
    true                                   // 并发写型 + 无显式 input → 默认隔离
} else {
    resolve_isolation(def.isolation, dispatch_isolation)  // serial / 并发只读 → def 默认
};
```

> **实现修正（2026-06-30）**：初版是「并发写型 → 无条件 force」（中间分支不看 `dispatch_isolation`）。但两个 L3b 测试在并发 dispatch 时传 `isolation: false` 显式 opt-out（免 git fixture），无条件 force 会无视它 → 建 worktree → fixture 失败。最终改为**显式 dispatch input 优先于并发 force**：并发把默认从 shared 提升为 isolated，但仍尊重显式 opt-out。这不削弱并发安全（不 opt-out 就隔离），且 `resolve_isolation` 签名不变 → 旧 truth table 测试（`dispatch.rs` `resolve_isolation_*`）全保留。

**3.4 chat_loop.rs 接线**（`classify_dispatch_batch` 两个分支）：

| 分支 | 位置 | `force_readonly` | `parallel` | 效果 |
|---|---|---|---|---|
| `Concurrent` | `chat_loop.rs:2216` | `false`（现状） | **`true`**（新） | 写型 worker 各自 worktree；只读 worker 共享 |
| `Serial` | `chat_loop.rs:2371` | `false`（现状） | `false`（新） | def 默认 = shared |

> `OverLimit` 分支不调 `run_subagent`（全拒绝），不受影响。

**3.5 默认值**：`general-purpose` `isolation: Some(true)` → `None`（`mod.rs:410`）；`researcher` 保持 `None`。

**3.6 tool description 改写**（`mod.rs:119-142` + schema `160-169`）：删「`general-purpose` defaults to isolated; pass `false` to force shared」；加「单 dispatch 共享主工作区、改动立即可见；同 turn 多 dispatch 时写型 worker 系统自动隔离到各自 worktree、并发安全；isolation 由系统自动决定，通常无需手动指定」。同步更新 `dispatch.rs` / `chat_loop.rs` 里引用旧默认的过时注释。

## 4. 并发安全等价性论证（B 的核心 review 点）

**现状（L3b PR2）**：`DispatchBatch::Concurrent` 的多个 worker 各自 isolated——靠 `general-purpose` 默认 `Some(true)`（`chat_loop.rs:2106-2108`）。`researcher` 默认 `None` → **并发 researcher 现状就是 shared**（只读无写竞争，且省 checkout）。

**迁移后效果矩阵**：

| 场景 | 决策 | 结果 |
|---|---|---|
| 并发 general-purpose（写型） | `parallel && writable` | isolated（等价旧"默认 isolated"） |
| 并发 researcher（只读） | `parallel && !writable` → else 分支 | **shared（现状不变，不浪费 worktree）** |
| serial 任意 | `!parallel` → else 分支 | def 默认（general-purpose=shared） |

**等价性**：
- 对**写型** worker，`parallel && writable` 覆盖了旧「`general-purpose` 默认 isolated」，且**更强**——自定义写型 agent 即便 frontmatter `isolation: false`，并发时仍被强制隔离 → 安全不回退。
- 对**只读** worker，行为与现状**完全一致**（共享 cwd），无回归、无多余 worktree。
- `force_readonly` 仍是 serial-only behavioral switch，在 `parallel && writable` 之前短路（`dispatch.rs:346`），L3a 回归测试 `l3a_single_dispatch_runs_serial_path_unchanged` 不受影响。

## 5. 兼容性 / Rollout / Rollback

- **兼容**：serial 单 dispatch = shared（对齐 Claude Code 默认）；并发写型 = 强制 isolated（安全等价）；并发只读 = shared（现状不变）。
- **Rollback**：`general-purpose` `None`→`Some(true)` + `Concurrent` 分支 `parallel=false`，即恢复 L3b PR2 现状。
- **旧测试**：`resolve_isolation` truth table（`dispatch.rs:1137-1174`）**签名未变，无需改**；只需新增 `parallel + writable` 用例。
