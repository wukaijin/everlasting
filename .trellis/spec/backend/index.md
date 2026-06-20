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
| [Agent Loop Architecture](./agent-loop-architecture.md) | `run_chat_loop` 22-param shared entry point (production + test), CancellationGuard single-source via equivalence proof, B6 worker subagent pattern (nested run_chat_loop + **16** skip_persist gates [PR2a RULE-A-015 corrected from 18] + skip_session_active + is_worker 第 21 参 [PR2b RULE-A-014] + app_handle 第 22 参 [PR3 PR2 hotfix]), faithful-port drift hazard anti-pattern, new "Pattern: PR2a corrected PR1 over-broad skip_persist gate" (RULE-A-015), DEBT RULE-A-014 closed + RULE-A-016 closed (B6 PR3a) | Filled (2026-06-20, task 06-20-b6-pr2-subagent-persistence PR2b + 06-20-b6-pr3-frontend-expand PR3a) |
| [Tool Contract](./tool-contract.md) | tool definitions + ReadGuard + Bash Spillover + ⑨ 关 5-tier 决策合约 + 9 Scenarios: Tool Set Extension / web_fetch / update_checklist / dispatch_subagent tool (B6 PR1) / **subagent_runs persistence (B6 PR2, 2026-06-20)** / ⑨ 关 Permission Decision Layer / Path-based Permission Layer / L1a Background Shell Tools | Filled (2026-06-20, task 06-20-b6-pr2-subagent-persistence PR2) |
| [Database Guidelines](./database-guidelines.md) | sqlx patterns, idempotent migrations, naming conventions, enum / soft-FK / cascade / denormalized-list patterns, `app_config` key/value, test pool setup, **subagent_runs (B6 PR2) — new table pattern: parent_session_id FK CASCADE, soft-FK parent_request_id, CHECK-constrained status, JSON-typed payload columns, 4 MiB transcript cap, streaming token_usage, audit-not-pollute invariant** | Filled (2026-06-20, task 06-20-b6-pr2-subagent-persistence PR2) |

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
