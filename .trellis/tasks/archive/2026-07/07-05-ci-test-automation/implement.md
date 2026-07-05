# Implement — CI 测试自动化管线

> 对应 `prd.md` + `design.md`。本任务步骤少，执行计划简短。

## 实施步骤

### Step 1 — 全量 cargo fmt（机械改动）
```bash
cd app/src-tauri && cargo fmt
cargo fmt --check   # 验证已干净，应无输出
```
- **单独 commit**：`style(rust): cargo fmt 全量格式化 (CI fmt gate 前置)`
- review 时用 `git diff --stat` 扫一眼；rustfmt 不改语义，信任工具。

### Step 2 — 写 `.github/workflows/ci.yml`
按 `design.md` §1 的结构：
- 双 job 并行（rust + frontend）
- rust job：apt（design §2 清单）+ rust-toolchain + rust-cache + `cargo fmt --check` + `cargo test --lib`
- frontend job：pnpm + node cache + `pnpm install --frozen-lockfile` + `pnpm test` + `pnpm build`
- 触发 + path filter 按 design §5
- rust job 的 `working-directory: app/src-tauri`；frontend job 的 `working-directory: app`
- rust-cache 的 `workspaces: 'app/src-tauri'`

### Step 3 — README badge（R5）
- 在 README 顶部加：`![CI](https://github.com/<owner>/everlasting/actions/workflows/ci.yml/badge.svg)`
- owner 未知则留 `<owner>` 占位 + HTML 注释说明，不阻塞。

### Step 4 — 本地模拟 CI（防回归 dry-run）
在 push 前本地跑一遍 CI 等价命令，确认全绿：
```bash
cd app/src-tauri && cargo fmt --check                                                    # AC5
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib  # AC1 rust 部分
cd app && pnpm install --frozen-lockfile && pnpm test && pnpm build                      # AC1 frontend 部分
```
- 任一失败 → 修到全绿再 push（不要带着失败 push 让 CI 来发现）。

### Step 5 — commit 拆分
1. `style(rust): cargo fmt 全量格式化` —— Step 1 产物
2. `ci: add GitHub Actions workflow (cargo test + fmt + vitest + vue-tsc)` —— Step 2 + Step 3 产物
- 不混其他改动。

### Step 6 — push 观察 CI 实跑（AC1/AC2/AC3）
- push 后观察 GitHub Actions 实际跑通（这是 workflow 自身正确性的最终验证，本地无法 100% 复现 ubuntu runner 环境）。
- 第二次 push（任意小改动）确认 cache 命中（AC3）。
- 故意制造一处测试失败或类型错误验证 CI 红灯（AC2，可单独一个 throwaway commit 或在 PR 临时改）。

## 验证命令汇总

| AC | 命令 | 期望 |
|---|---|---|
| AC5 | `cd app/src-tauri && cargo fmt --check` | 无输出（exit 0） |
| AC1 rust | `PKG_CONFIG_PATH=... cargo test --lib` | 1274 passed |
| AC1 front | `cd app && pnpm test && pnpm build` | vitest 718 passed + vue-tsc 0 err |
| AC4 | grep workflow 文件无 `PKG_CONFIG_PATH` / `.cargo/config` | 命中 0 |
| AC6 | push 一个只改 `docs/*.md` 的 commit | CI 不触发 |

## Risky Files / Rollback Points

- **零 risky files**：不动 agent core / 前端业务代码。
- 唯一需谨慎：`cargo fmt` 改动面广，但纯机械。
- **Rollback**：`git revert <merge-commit>` 恢复到无 CI 状态。

## task.py start 前检查

- [ ] prd.md 已 convergence pass（无重复事实 / 残留 brainstorm 段）
- [ ] design.md apt 清单含 `libwebkit2gtk-4.1-dev`（**4.1 不是 4.0**）
- [ ] Q2 触发时机默认值用户已确认（或接受默认）
- [ ] Q4 README badge 默认值用户已确认（或接受默认）
- [ ] 用户 review 过 prd + design 后授权 `task.py start`
