# L3b PR3 permission/concurrency hardening (B1/B2/B3)

## Goal

修复全面审查 L3b 4 PR 时发现的 PR3（commit `d23ff9a`，`merge_worker`/`discard_worker` tool + sweep）三个 Blocker。三者均已逐行验证（审查报告 + plan 见 `~/.claude/plans/snug-marinating-sloth.md`）。

## Context — 三个 Blocker

- **B1 权限模型脱节**：`merge_worker`/`discard_worker` 在 `classify_tool` 落 `ToolKind::Other` → `check.rs:329` Tier 5 **silent Allow**；`risk_for_tool`（types.rs:78）返 `Risk::Low`；`filter_tools_for_mode`（mode.rs:51）不过滤 → **Plan 模式（应只读）可执行 merge 改写 parent session 分支**。而 `merge_worker.rs:25-30` / `tools/mod.rs:139-145` / `tool-contract.md` 注释**虚假声称 Tier 4 / Risk::High** —— 文档描述了一个根本不存在的权限门。
- **B2 并发 merge 无锁**：`do_merge_blocking`（merge_worker.rs:234）被两处 `spawn_blocking` 调用（tool `execute` + IPC `merge_worker_run`），无互斥 → 同一 parent session 分支并发 `repo.merge` 损坏 git index（libgit2 不保证线程安全）。
- **B3 worker 越权**：`STRUCTURALLY_DISABLED`（mod.rs:538）不含 merge/discard → general-purpose worker 能调 `merge_worker`，用兄弟 worker 的 run_id（dispatch tool_result 可见）merge 其 branch 到 parent。

## Decision (ADR-lite)

- **B1 — 新增 `ToolKind::GitMutation`（WebFetch 式：tool-level grant + ask），不归 `Shell`**：Shell 分支用 `command` 字段做 prefix-grant 匹配，merge_worker 无 `command` → 用户"始终允许"会写空前缀 grant，被所有空-token shell 命令误命中（安全隐患）。GitMutation 用 tool-level grant，modal 不渲染 path-scope 行。6 触点：`risk_for_tool`→High / ToolKind enum + `classify_tool` / decide match 复刻 WebFetch（`check_tool_grant` 传 `tool_name` 形参，不硬编码 —— 涵盖两个 tool）/ `match_value_for_allow_always`→`("tool", None)` / `filter_tools_for_mode` Plan 过滤。新增变体让 check.rs 两个 match 非穷尽 → 编译强制两处补 arm。
- **B2 — `do_merge_blocking` 入口 per-`parent_session_id` 串行**：`merge_lock_for` helper（`OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>>`，项目惯用 `std::sync::OnceLock`）+ 入口持 `std::sync::Mutex` guard（同步函数、0 await）。覆盖两个 spawn_blocking 入口；独立 session 仍并行。外层 HashMap 锁在 helper return 时释放、再取内层 per-session 锁 —— 固定顺序，无死锁。`discard_worker` **不加锁**（`do_discard` 只调 `destroy_worker`，不动 parent index，并发幂等）。
- **B3 — `STRUCTURALLY_DISABLED` 加 `merge_worker`/`discard_worker`**：`filter_tools_for_subagent` 自动剥离，worker 工具集不再含这两个 tool。

附带订正：`merge_worker.rs`/`discard_worker.rs`/`tools/mod.rs` 的虚假注释 + `tool-contract.md` §3 权限段 + §6 并发段，让声明与实现一致。

## Verification

`cargo test --lib` **957 passed / 1 failed** —— 唯一失败是 pre-existing C3 压缩测试（`RULE-A-017`：pre-PR1 main 同样失败，与本次无关）。permissions + l3b 子集 **119 passed / 0 failed**，零回归。+2 新测试函数（`risk_for_tool_includes_merge_discard_high`、`filter_tools_for_mode_drops_merge_discard_in_plan`）+ classify/worker-工具集断言扩展。

## Out of Scope（留 follow-up）

- PR3 Major：merge 前未检查 parent worktree 未提交 WIP（force checkout 会丢）；run_id 输入未做 UUID 格式校验（纵深防御）。
- PR1：缺独立 e2e isolation 测试（PR2 已间接覆盖）。
- PR2：spec `tool-contract.md` 残留 L3a 只读表述（1780 / 1815，文档债）。
- PR4：IPC 错误对象 `String(e)` 健壮性；i18n（PRD 前提错误，非回归）。
- pre-existing C3（`RULE-A-017`）。
