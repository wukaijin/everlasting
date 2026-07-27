# Quality Guidelines

> Code quality standards for frontend development.

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

### Convention: 改 `app/package.json` 必须同步 `pnpm-lock.yaml`,提交前自动拦截

**What**: 任何 `app/package.json` 的依赖改动,提交时 `app/pnpm-lock.yaml` 必须与之同步——即 `pnpm install --frozen-lockfile` 能成功。

**Why**: 2026-07-27 CI 的 `pnpm install --frozen-lockfile` 失败,因为 `app/package.json` 加了 `concurrently@^9.1.0` 但没同步进 lockfile。本地和 CI 用同一把尺子(`--frozen-lockfile`),才能避免"本地能装、CI 挂"。

**How (已被自动化)**: `lefthook.yml` 的 `pre-commit.pnpm-lockfile-sync` job 在改了 `app/package.json` 时,于 `app/` 目录跑 `pnpm install --frozen-lockfile`,失败即拦截。开发机执行 `pnpm install` 时由 `prepare` 脚本自动 `lefthook install`,clone 即生效。手动修复命令:

```sh
cd app && pnpm install    # 会更新 pnpm-lock.yaml
git add app/pnpm-lock.yaml
```

**相关**: 见仓库根 `lefthook.yml`;CI 的 frozen-lockfile 步骤在 `.github/workflows/ci.yml`。

---

## Testing Requirements

<!-- What level of testing is expected -->

(To be filled by the team)

---

## Code Review Checklist

<!-- What reviewers should check -->

(To be filled by the team)
