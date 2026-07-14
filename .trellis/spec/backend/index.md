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
| [LLM API Contract](./llm-contract.md) | Anthropic Messages API + extended thinking + SSE lifecycle + multi-provider dispatch (Anthropic/OpenAI) + test_model per-model connectivity probe + worktree/cancel + tool set + **A5+ retry strategy (07-05: Full Jitter + first-byte retry + retry-after parsing + 双向熔断 + `LlmError` headers extension + Retrying event)**, **E2 trace ChatEvent variants are server-emitted (07-14: 3 new variants `ContextCompacted` / `LoopHint` / `WorkflowBreadcrumb`, defensive drop arm in SSE consumer, wire snake_case + payload shape table)** | Filled (2026-07-14, task 07-14-e2-backend-trace-pipeline) |
| [Git Diff Contract](./git-diff.md) | Workdir-vs-branch-tip FileDiff source of truth, `git --numstat` over `Patch::line_stats()`, fallback policy, numstat subprocess contract | Filled (2026-06-08, step 4 follow-up Bug 2) |
| [Agent Loop Architecture](./agent-loop-architecture.md) | `run_chat_loop` 23-param shared entry point (production + test), CancellationGuard single-source via equivalence proof, B6 worker subagent pattern (nested run_chat_loop + **16** skip_persist gates [PR2a RULE-A-015 corrected from 18] + skip_session_active + is_worker 第 21 参 [PR2b RULE-A-014] + app_handle 第 22 参 [PR3 PR2 hotfix] + system_prompt_override 第 23 参 [B6 review defect A fix 2026-06-21]), faithful-port drift hazard anti-pattern, new "Pattern: PR2a corrected PR1 over-broad skip_persist gate" (RULE-A-015), DEBT RULE-A-014 closed + RULE-A-016 closed (B6 PR3a) + B6 review defect A fixed, **E2 trace pipeline pattern (07-14): 4 write points (C3 compaction / C2 soft hint / workflow breadcrumb / per-turn token) + double-write best-effort + `PermissionContext.turn_seq` audit-alignment hook + RULE-A-015 per-turn token worker gate** | Filled (2026-07-14, task 07-14-e2-backend-trace-pipeline) |
| [Tool Contract](./tool-contract.md) | tool definitions + ReadGuard + Bash Spillover + ⑨ 关 5-tier 决策合约 + 9 Scenarios: Tool Set Extension / web_fetch / update_checklist / dispatch_subagent tool (B6 PR1) / **subagent_runs persistence (B6 PR2, 2026-06-20)** / ⑨ 关 Permission Decision Layer / Path-based Permission Layer / L1a Background Shell Tools / **BuiltinPlugin source layer (07-09-workflow-builtin-plugin, 2026-07-09)** | Filled (2026-07-09, task 07-09-workflow-builtin-plugin) |
| [Database Guidelines](./database-guidelines.md) | sqlx patterns, idempotent migrations, naming conventions, enum / soft-FK / cascade / denormalized-list patterns, `app_config` key/value, test pool setup, **subagent_runs (B6 PR2) — new table pattern: parent_session_id FK CASCADE, soft-FK parent_request_id, CHECK-constrained status, JSON-typed payload columns, 4 MiB transcript cap, streaming token_usage, audit-not-pollute invariant**, **turn_trace (E2) — wide-row accumulator with `UNIQUE(session_id, seq)` + per-dimension column-scoped UPSERT (signals arrive at different write points during a turn, single-row 回看)** | Filled (2026-07-14, task 07-14-e2-backend-trace-pipeline) |

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
