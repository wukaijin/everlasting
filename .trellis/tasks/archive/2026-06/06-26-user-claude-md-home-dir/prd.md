# PRD: user 级 CLAUDE.md 改到 `~/.claude/CLAUDE.md`

## 1. Background & Motivation

### 1.1 现状

`app/src-tauri/src/memory/file.rs::user_dir()` 当前返回 `dirs::config_dir().join("everlasting")`,在 Linux 上解析为 **`~/.config/everlasting/`**。该目录同时承担 user 层两个 memory 文件:

- `~/.config/everlasting/CLAUDE.md`(User CLAUDE.md)
- `~/.config/everlasting/AGENTS.md`(User AGENTS.md)

project 层走 `<projects.path>/CLAUDE.md` / `AGENTS.md`,**不动**。

### 1.2 已知问题(P2-2 audit finding)

`docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md:196` 已记录:

| P2-2 | user_dir 路径与 Claude Code 实际路径不一致 | `file.rs:58-66` 用 `~/.config/everlasting/CLAUDE.md`;Claude Code 用户级在 `~/.claude/CLAUDE.md` | 用户从 Claude Code 切过来**用户层指令不共享** |

本任务关闭该 P2-2。

### 1.3 目标

User 层 **CLAUDE.md** 路径改为 Claude Code 同款 `~/.claude/CLAUDE.md`,与 Claude Code 共享用户级指令文件,减少双写的维护成本。

User 层 **AGENTS.md** 路径保留在 `~/.config/everlasting/AGENTS.md`(CLAUDE.md 是 Claude-Code 兼容文件名,AGENTS.md 是 Everlasting 自有命名空间,语义上不属于"Claude Code 生态")。

Project 层(CLAUDE.md / AGENTS.md / agents/*.md)全部保留 `<project>/.everlasting/` 命名空间,**不动**。

## 2. Scope

### 2.1 In-Scope

| 类别 | 内容 |
|---|---|
| 后端代码 | 新增 `user_claude_dir()` helper(返回 `~/.claude/`),`resolve_path()` / `all_paths()` 按 `MemorySource` 分派 user 层目录 |
| 测试 | 新增 `UserClaudeDirGuard`,更新 `app/src-tauri/src/memory/tests.rs` 中所有把 CLAUDE.md 写到 `user_dir` 的 fixture |
| 文档 | 同步 `.trellis/spec/backend/memory.md` / `frontend/memory-ui.md` / `docs/IMPLEMENTATION.md` 中"4 文件路径"表与示例 |
| 审计 | 关闭 REVIEW-agent-loop-full-audit-2026-06-14.md P2-2 |

### 2.2 Out-of-Scope(显式不动)

| 类别 | 路径 | 备注 |
|---|---|---|
| User AGENTS.md | `~/.config/everlasting/AGENTS.md` | 保留 |
| Subagent user 层 | `~/.config/everlasting/agents/*.md` | 保留 |
| Subagent project 层 | `<project>/.everlasting/agents/*.md` | 保留 |
| Project CLAUDE.md / AGENTS.md | `<project>/.everlasting/{CLAUDE.md,AGENTS.md}` | 保留 |
| Skill / Role 路径 | 各自的 `~/.config/everlasting/{skills,roles}/` | 保留 |

### 2.3 文档顺手清理(同 PR 内,独立 doc drift)

`docs/subagent-loader.md`(8 处)、`docs/ROADMAP.md`(1 处)、`docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md`(1 处)仍写旧字面量 `~/.everlasting/agents/`,实际代码已用 `~/.config/everlasting/agents/`。本 PR 顺手修正(与 CLAUDE.md 路径改动无关,纯文档)。

## 3. Path Mapping

| Layer | Source | Before | After |
|---|---|---|---|
| User | CLAUDE.md | `<config_dir>/everlasting/CLAUDE.md` | `<home_dir>/.claude/CLAUDE.md` |
| User | AGENTS.md | `<config_dir>/everlasting/AGENTS.md` | **不变** |
| Project | CLAUDE.md | `<project.path>/CLAUDE.md` | **不变** |
| Project | AGENTS.md | `<project.path>/AGENTS.md` | **不变** |

`<config_dir>` = `dirs::config_dir()`(XDG / macOS / Windows 标准路径)。
`<home_dir>` = `dirs::home_dir()`(Claude Code 同款 — 在 Linux/macOS/Windows 上分别是 `~/.claude/` / `~/.claude/` / `%USERPROFILE%\.claude\`)。

## 4. Design

### 4.1 拆分 user_dir 的语义

当前 `user_dir()` 服务 CLAUDE.md 和 AGENTS.md 两个 user 层文件。改动后语义拆为:

- `user_claude_dir()` → 仅 user 层 CLAUDE.md 的目录(`~/.claude/`)
- `user_dir()` → 仅 user 层 AGENTS.md 的目录(`~/.config/everlasting/`)

两个 helper 各自有 test override(`USER_CLAUDE_DIR_OVERRIDE` / `USER_DIR_OVERRIDE`),互不干扰。

### 4.2 `resolve_path` 分派

`memory::file::resolve_path()` 在 `MemoryKind::User` 分支按 `MemorySource` 二选一:

```rust
match kind {
    MemoryKind::User => {
        let dir = match source {
            MemorySource::Claude => user_claude_dir()?,
            MemorySource::Agents => user_dir()?,
        };
        Some(dir.join(source.filename()))
    }
    ...
}
```

`Project` / `Session` / `Runtime` 分支不动。

### 4.3 `all_paths` 分派

`memory::loader::all_paths()` 同步拆分:

- User CLAUDE.md → `user_claude_dir()?.join("CLAUDE.md")`
- User AGENTS.md → `user_dir()?.join("AGENTS.md")`
- Project 两文件不动

保持 canonical 顺序(0: User Claude, 1: User Agents, 2: Project Claude, 3: Project Agents)不变 — 这是 `load_for_session` 的 contract,前端 `MemoryCache.peek_user(source)` / banner 索引都依赖。

### 4.4 测试覆盖

新增 `UserClaudeDirGuard`(对称于 `UserDirGuard`),`tests.rs` 中:

- `loader_load_for_session_with_all_files_present` — 把 CLAUDE.md 写到 `user_claude_dir`,AGENTS.md 写到 `user_dir`
- `loader_load_for_session_partial_files` — 同上拆分
- `loader_mtime_fence_*` 4 个测试 — 把 CLAUDE.md 写到 `user_claude_dir`
- `all_paths_yields_four_entries_in_canonical_order` — 断言两个 user dir 都存在时 entries.len() == 4
- `loader_different_projects_have_independent_caches` — 不动(只动 project 层)

新增边界测试:

- `resolve_path_user_claude_uses_home_dir` — 直接断言 `resolve_path(User, Claude, None)` 返回的路径以 `<...>/.claude/CLAUDE.md` 结尾(在 test override 场景下断言 `user_claude_dir` override 路径生效)。
- `user_claude_dir_uses_home_dir` — 不设 override 时断言返回 `dirs::home_dir().join(".claude")`。

## 5. Code Touchpoints

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/memory/file.rs` | 新增 `USER_CLAUDE_DIR_OVERRIDE` thread-local + `set_user_claude_dir_for_test()` + `user_claude_dir()`;`resolve_path` 加 `MemorySource` 分派;头部 doc-comment 把 4 路径表更新 |
| `app/src-tauri/src/memory/loader.rs` | `all_paths()` 改为分别用 `user_claude_dir()` / `user_dir()`;导入 `user_claude_dir` |
| `app/src-tauri/src/memory/types.rs` | `MemoryKind::User` doc-comment 更新(提两路径分家) |
| `app/src-tauri/src/memory/tests.rs` | 新增 `UserClaudeDirGuard`;更新 7+ 个测试 fixture;新增 2 个边界测试 |
| `.trellis/spec/backend/memory.md` | 行 73 / 180-181 / 322 / 334 路径表与示例更新 |
| `.trellis/spec/frontend/memory-ui.md` | 行 97 / 105 / 230 示例路径更新 |
| `docs/IMPLEMENTATION.md` | 行 569 决策描述更新(CLAUDE.md 不再走 `dirs::config_dir()`) |
| `docs/subagent-loader.md` | 8 处 `~/.everlasting/agents/` → `~/.config/everlasting/agents/` |
| `docs/ROADMAP.md` | 1 处 `~/.everlasting/agents/` → `~/.config/everlasting/agents/` |
| `docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md` | 1 处 `$HOME/.everlasting/agents/` → `$HOME/.config/everlasting/agents/` |
| `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` | P2-2 标记已修复 + 链接本任务 |

## 6. Migration / Compatibility

**无自动迁移**。已安装用户若在 `~/.config/everlasting/CLAUDE.md` 有内容,改动后:

- `~/.claude/CLAUDE.md` 不存在 → User CLAUDE.md 层 `Missing`(agent loop 静默吸收,见 RULE-C-001)
- 旧文件仍保留在 `~/.config/everlasting/CLAUDE.md`(用户可手动 `mv` 到 `~/.claude/CLAUDE.md` 或保留作备份)

提示文案:不在本任务范围(可后续单独写一个 migration 命令)。

## 7. Risks

| 风险 | 缓解 |
|---|---|
| 用户已有 `~/.config/everlasting/CLAUDE.md` 不会被自动搬 | §6 明确说明 + 后续 migration PR;短期影响是 user 层 CLAUDE.md 变 Missing,banner 不再出现该行 |
| `~/.claude/` 与 Claude Code 冲突 | **目的即此**;两边共享同一文件,symlink 一份即同步 |
| 测试忘记加新 guard | `tests.rs` 全量重审,grep `user_dir.path().join("CLAUDE.md")` 残留 |
| docs 漂移残留 | `grep -rn '~/.config/everlasting/CLAUDE' .` + `grep -rn '~/.everlasting/' docs/` 在 PR 内验证 |

## 8. Acceptance Criteria

- [ ] `cargo check` 通过(WSL PKG_CONFIG_PATH trick)
- [ ] `cargo test --lib` 全绿(WSL PKG_CONFIG_PATH trick)
- [ ] `pnpm tauri build` 前端 type-check 通过(vue-tsc)
- [ ] 新增的 `resolve_path_user_claude_uses_home_dir` / `user_claude_dir_uses_home_dir` 测试通过
- [ ] 旧 fixture `user_dir.path().join("CLAUDE.md")` 在 tests.rs 中 grep 为零(`UserClaudeDirGuard` 替换完毕)
- [ ] `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` P2-2 标记为已修复
- [ ] 本任务 journal entry 记录到 `.trellis/workspace/Carlos/journal-1.md`

## 9. Out of Scope(明确不做)

- Project 层 `.everlasting/` 改名
- 自动迁移旧 `~/.config/everlasting/CLAUDE.md`
- 增加新的 CLAUDE.md 加载路径(如 fallback 到旧路径)
- 修改 `<project>/.everlasting/` 任何东西
- 修改 subagent / skill / role 任何路径