# Backend Development Guidelines

> Best practices for backend development in this project.

---

## Overview

This directory contains guidelines for backend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Database Guidelines](./database-guidelines.md) | sqlx patterns, idempotent migrations, naming conventions, enum / soft-FK / cascade / denormalized-list patterns, `app_config` key/value, test pool setup | Filled (PR1 of 06-08) |
| [Error Handling](./error-handling.md) | Error types, handling strategies | To fill |
| [LLM API Contract](./llm-contract.md) | Anthropic Messages API + extended thinking + SSE lifecycle + multi-provider dispatch (Anthropic/OpenAI) + test_model per-model connectivity probe + worktree/cancel + tool set | Filled (step 6, PR2-3, PR5 follow-up) |
| [Git Diff Contract](./git-diff.md) | Workdir-vs-branch-tip FileDiff source of truth, `git --numstat` over `Patch::line_stats()`, fallback policy, numstat subprocess contract | Filled (2026-06-08, step 4 follow-up Bug 2) |

> ℹ️ 8-PR4 cleanup (2026-06-10): 移除 3 个空骨架文件 (`directory-structure.md` / `quality-guidelines.md` / `logging-guidelines.md`) — 项目无对应填充需求,直接删除更清晰。

---

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.
