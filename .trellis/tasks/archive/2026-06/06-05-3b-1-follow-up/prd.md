# 3b-1 follow-up 总结 + 文档更新

## Goal

3b-1 整组（PR1 + PR2 + 3 hotfix）落地收尾后，沉淀剩余工作 + 更新文档。3 类工作：

1. **新增 `docs/FOLLOW-UP.md`**：集中列 3b-1 期间发现的 known issues / TBDs / 候选修复（5 条）
2. **修改 `docs/HACKING-llm.md` + `docs/HACKING-wsl.md`**：3 个 hotfix 经验沉淀（重复会撞的坑）
3. **修改 `docs/BACKLOG.md` + `CLAUDE.md`**：标 3b-1 完整闭环（不是新加 §，是改 §0 / "当前状态" 段）

不做：再次改 backend / frontend 代码（follow-up 是设计记录，不实施）；不动 IMPLEMENTATION.md（已校准）。

## 已知 follow-up 列表

| # | Follow-up | 关联 commit | 优先级 | 估计工作量 |
|---|---|---|---|---|
| 1 | `cwd` 简化为 `~/`（chat header 显示） | hotfix-3 (post-PR2 squash) | 低 | ~30 行（PR1 backend 加 `get_home_dir` command + PR2 frontend 简化路径） |
| 2 | TS interface 字段 `snake_case` → `camelCase` | hotfix-3 | 低 | ~50 行（PR1 backend 加 `#[serde(rename_all = "camelCase")]` + PR2 frontend 字段改名） |
| 3 | `pick_project_dir` 改成前端 reka-ui 渲染 dialog | 用户偏好，标 follow-up | 中 | ~150 行（替换 Tauri native dialog 调用） |
| 4 | HACKING-wsl 增坑 11：Tauri 2 IPC arg camelCase 约定 | hotfix-1 | — | 文档 ~30 行 |
| 5 | HACKING-llm 增坑 N：Anthropic tool_result role 协议 | hotfix-3 | — | 文档 ~40 行 |
| 6 | HACKING-llm 增坑 N+1：Option<T> 字段 IPC null 行为 | hotfix-2 | — | 文档 ~20 行 |

## 文档动作清单

| 文件 | 动作 |
|---|---|
| `docs/FOLLOW-UP.md` | **新增** — 集中 follow-up 列表（6 条）+ 后续决策建议 |
| `docs/HACKING-wsl.md` | **增坑 11**：Tauri 2 IPC arg naming (camelCase vs snake_case) |
| `docs/HACKING-llm.md` | **增坑 N + N+1**：tool_result role + Option<T> null 行为 |
| `docs/BACKLOG.md` | **增 §10**：3b-1 follow-up（跟现 §1-§9 平级） |
| `CLAUDE.md` | **改"当前状态"段**：标 3b-1 完成，加 `is_legacy` 项目 + cwd 字段说明 |

## Acceptance Criteria

- [ ] `docs/FOLLOW-UP.md` 存在，列 6 条 follow-up（每条带 commit 关联 + 工作量估 + 优先级）
- [ ] `HACKING-wsl.md` 坑 11 写清楚：Tauri 2 command arg 默认 camelCase + 修复模式
- [ ] `HACKING-llm.md` 坑 N + N+1 写清楚：tool_result role 协议 + Option<T> null 行为
- [ ] `BACKLOG.md` §10 跟现 §1-§9 风格一致
- [ ] `CLAUDE.md` "当前状态" 反映 3b-1 完成 + 关键 schema 变化
- [ ] 改完 git diff 看一遍，git commit

## Out of Scope

- 不实施 follow-up（只是记录）
- 不动 IMPLEMENTATION.md（HANDOFF §2 / 路线图都已校准）
- 不重写现文档（只追加，不动结构）
- 不动代码（除 `CLAUDE.md` 顶部 "当前状态" 段的事实型更新）

## Definition of Done

- 5 个文件改动 commit 完成
- 文档 + 跟 3b-1 commit history 一致
- `git status` 干净
- task finish-work archive 到 `archive/2026-06/`
