# CI 测试自动化管线 (GitHub Actions)

## Goal

为 1274 cargo + 718 vitest + vue-tsc 类型检查建立 push/PR 触发的 GitHub Actions CI，让任何"小改动"破坏测试 / 类型 / Rust 格式时在 PR 阶段就被挡住，而不是靠本地手动跑三件套（`PKG_CONFIG_PATH=... cargo test` + `pnpm test` + `vue-tsc`）事后发现。**主体加 workflow 文件 + 一次机械 `cargo fmt` 全量格式化（无逻辑改动）。**

## Background

- V2 第三档 E1 条目（ROADMAP §2 🟠）。当前 1100+ Rust 测试 + vitest + vue-tsc **全部手动跑**，无防回归。最近 A5+ 一轮（1274 cargo / 718 vitest）已显现手动验证摩擦；journal 记录"1266 全绿不准，Step 5 后即 fail"——正是 CI 该堵的回归类型。
- DEBT.md：0 open items，无债务压力，纯前瞻性基建。

### 已确认事实（代码可证）

- **完全从零**：无 `.github/`、无 `rust-toolchain*`、无 `.cargo/config`。
- **单 crate**：`app/src-tauri/Cargo.toml`（lib + bin，无 workspace）。
- **前端**：`pnpm` 在 `app/`，`pnpm test` = `vitest run`（jsdom，纯 Node，无浏览器）。
- **`pnpm build`** = `vue-tsc --noEmit && vite build`。
- **build.rs** = `tauri_build::build()` → 即使只跑 `cargo test --lib`，编译期仍需 pkg-config 找到 webkit2gtk / gtk。**CI 上 `apt install` 系统依赖是必选**，与 scope 无关。
- **关键依赖特性**：`git2` 用 `vendored-libgit2`（首次编译 30-90s 编 C 源 → cache 必要）；`reqwest` 用 `rustls`（省掉 openssl-sys）；无其他特殊系统需求。
- **WSL 本地 `PKG_CONFIG_PATH` hack**（HACKING-wsl 坑 1）在 ubuntu runner 不需要——原生 pkgconfig 已含 gtk/webkit。
- **`cargo fmt --check` 当前不干净**（`at_file.rs:71` 等多处 diff）→ 加 fmt gate 必须先全量 fmt 一次。
- 无 `tauri.conf.json` beforeBuild hook；lock 文件（`Cargo.lock` + `app/pnpm-lock.yaml`）已提交；前端无 eslint/prettier。

## Requirements

### R1 — CI workflow 文件 + 触发
- 新增 `.github/workflows/ci.yml`，触发：`push: [main]` + `pull_request: [opened, synchronize, reopened]` + `workflow_dispatch`。
- path filter 忽略纯文档改动（`**/*.md`、`docs/**`、`.trellis/**`），节省 CI 分钟；`.github/workflows/**` 改动**仍触发**（验证 workflow 自身）。

### R2 — 双 job 并行（rust + frontend）
- **rust job**：apt 装系统依赖 → setup Rust stable（含 rustfmt）→ rust-cache → `cargo fmt --check` → `cargo test --lib`。
- **frontend job**：setup pnpm（含 cache）→ `pnpm install --frozen-lockfile` → `pnpm test` → `pnpm build`（含 `vue-tsc --noEmit`）。
- 两 job 并行（frontend 不需 webkit，独立快跑）；总时长 ≈ rust job。

### R3 — Cache
- Cargo cache（`Swatinem/rust-cache@v2`）+ pnpm 内置 store cache。vendored-libgit2 的 C 编译产物必须命中 cache，否则每次 CI 多 60s+。

### R4 — 全量 cargo fmt（机械改动）
- 本 task 顺手 `cargo fmt` 一把清干净（Q3 决议 C），让 fmt gate 首次 CI 即绿。
- **单独 commit**（与 workflow 文件分开），review 友好；commit message 注明"机械格式化，无逻辑改动"。
- clippy **不进本 task**（留 follow-up，先本地清理再加 gate）。

### R5 — README badge
- README 顶部加 CI status badge（`https://github.com/<owner>/everlasting/actions/workflows/ci.yml/badge.svg`）。

## Acceptance Criteria

- [ ] AC1：`.github/workflows/ci.yml` 提交 + PR 触发 CI；本地全绿时 CI 也全绿（rust job: fmt + test --lib；frontend job: vitest + build）。
- [ ] AC2：故意制造一处类型错误或测试失败 push 上去，CI 红灯拦截（人工验证一次）。
- [ ] AC3：第二次 push 起 cargo + pnpm cache 命中（CI 时间显著下降，日志确认 vendored-libgit2 不再重编）。
- [ ] AC4：CI 不依赖任何本地 hack（无 `PKG_CONFIG_PATH`、无 `.cargo/config` 修改）。
- [ ] AC5：`cargo fmt --check` 在 CI 上通过（即全量 fmt 已落地）。
- [ ] AC6：docs/.trellis 纯文档 PR 不触发 CI（path filter 生效）。

## Out of Scope

- **clippy gate** —— 留独立 follow-up task（先本地清理 warning，再 PR 加 `--deny warnings`）。
- 多 OS matrix（macOS / Windows）—— WSL-first 项目，主战场 Linux。
- 多 Rust 版本 matrix —— 无 MSRV 声明，over-engineering。
- 自动 release / publish（tag-triggered `release.yml`）—— 独立 task。
- 代码覆盖率上报、Dependabot、自托管 runner。

## Open Questions（最终 review 时确认）

- **Q2 触发时机**（默认值）：`push: [main]` + `pull_request` + `workflow_dispatch`，path filter 忽略 `**/*.md`/`docs/**`/`.trellis/**`。如想只在 PR 触发（push main 不跑）或反过来，请告知。
- **Q4 README badge**（默认值）：加。如不想加（README 还没成型）请告知。

## Notes

- 任务复杂度：**中等** —— workflow 单文件 + 一次机械 fmt 改动。需 `design.md`（apt 包清单 / cache / fmt commit 策略）；`implement.md` 因步骤少可省，但会写一份简短的（验证命令 + commit 拆分）。
