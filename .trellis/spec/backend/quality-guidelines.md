# Quality Guidelines

> Code quality standards for backend development.

---

## Overview

<!--
Document your project's quality standards here.

Questions to answer:
- What patterns are forbidden?
- What linting rules do you enforce?
- What are your testing requirements?
- What code review standards apply?
-->

(To be filled by the team)

---

## Forbidden Patterns

<!-- Patterns that should never be used and why -->

(To be filled by the team)

---

## Required Patterns

<!-- Patterns that must always be used -->

### Convention: `cargo fmt` 必须通过,提交前自动拦截

**What**: 任何 `app/src-tauri/**/*.rs` 改动,提交时必须通过 `cargo fmt --check`(无 `rustfmt.toml`,全默认风格)。

**Why**: 2026-07-27 一次 push 让 CI 的 `cargo fmt --check` 失败(~50 个 `.rs` 在提交前没跑 fmt,大多是 `use` 排序与长行折行)。fmt 是机器能 100% 判定的检查,不该靠人记,也不该等 CI 兜底——本地提交时就拦。

**How (已被自动化)**: `lefthook.yml` 的 `pre-commit.cargo-fmt` job 对暂存的 `.rs` 跑 `cargo fmt --check`,失败即拦截。开发机执行 `pnpm install` 时由 `prepare` 脚本自动 `lefthook install`,clone 即生效。手动修复命令:

```sh
cargo fmt --manifest-path app/src-tauri/Cargo.toml
git add -u   # 重新 add 格式化后的文件
```

> hook 故意用 `--check` 而非直接 `cargo fmt`:避免 hook 偷偷改暂存内容、制造混乱。失败时提示作者手动跑。

**相关**: 见仓库根 `lefthook.yml`;CI 的 `cargo fmt --check` 步骤在 `.github/workflows/ci.yml`。

---

## Testing Requirements

<!-- What level of testing is expected -->

(To be filled by the team)

---

## Code Review Checklist

<!-- What reviewers should check -->

(To be filled by the team)
