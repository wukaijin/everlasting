# brainstorm: 统一 .trellis/workspace/ 路径大小写 (carlos → Carlos)

## Goal

本地开发机 `.trellis/.developer` 写的是 `name=Carlos`（uppercase），但远端 19 个 commit 把 journal/index 写到了 `carlos/`（lowercase）—— 因为 WSL 大小写敏感，git 把两个目录都 track 成不同文件。导致：

1. workspace 下同时存在 `carlos/` 和 `Carlos/` 两个目录
2. 远端 19 个 commit 累积的 journal 内容（81k / 31 sessions）被孤立在 `carlos/`，本地 `Carlos/` 只有 36k / 17 sessions
3. 1 处文档 bug（`06-07-6-ui-bug-markdown-sse/prd.md` 引用了 `carlos/`），其余 7 处都引用 `Carlos/`（uppercase 是规范）

**目标**：以本地 `Carlos/` 为 ground truth，把 `carlos/` 下的内容按可恢复方式并入 `Carlos/`，清理全部跨机器漂移。

## What I already know

### 事实（已验证）
- `.trellis/.developer` 文件内容：`name=Carlos`，initialized_at=2026-06-05T11:11:45
- `.trellis/.developer` 在 `.trellis/.gitignore` 里 → **每台机器独立**（解释为什么远端 19 个 commit 用 lowercase）
- `git ls-files` 输出：
  - `.trellis/workspace/Carlos/index.md` (596B)
  - `.trellis/workspace/Carlos/journal-1.md` (36k, 17 sessions, last 2026-06-16 17:28)
  - `.trellis/workspace/carlos/index.md` (5.2k, 31 sessions)
  - `.trellis/workspace/carlos/journal-1.md` (81k, last 2026-06-16 latest journal 追加)
- 远端首条 lowercase commit：`eb69e06` (2026-06-05 02:31:24)
- 本地首条 uppercase commit：`ce1a893` (更早)
- 没有任何 commit 做过 `Carlos → carlos` 或反之的 rename —— 两条线是**独立工作流**
- `a0d91ef` "scrub wukaijin + rename bundle id" 那个 commit 只是文字 scrub 提了一嘴，没动路径
- diff 两个 index.md：内容不同（不同 sessions 计数 + 不同最后活跃日期）
- diff 两个 journal-1.md：内容不同

### 受影响文件
**7 处 uppercase `Carlos/` 引用**（全部在 `.trellis/tasks/archive/2026-06/` + `STRUCTURE.md`）：
- `STRUCTURE.md`
- `.trellis/tasks/archive/2026-06/06-09-06-09-audit-files-and-docs/prd.md` (3 处)
- `.trellis/tasks/archive/2026-06/06-09-06-09-audit-files-and-docs/{implement,check}.jsonl` (各 1 处 + audit 包 7 文档)
- `.trellis/tasks/archive/2026-06/06-08-06-08-step-4-follow-up-2013-reappears-diff-counter-bug-diff-worktree/prd.md`
- `.trellis/tasks/archive/2026-06/06-08-06-08-step-4-followup-bugfix-2013-tool-use-orphan/prd.md` + `check.jsonl`
- `.trellis/workspace/Carlos/journal-1.md` 自引用

**1 处 lowercase `carlos/` bug 引用**（需要修）：
- `.trellis/tasks/archive/2026-06/06-07-6-ui-bug-markdown-sse/prd.md`

### 约束
- WSL/Linux 工作环境 → 大小写敏感 → 不会撞 case-folding
- trellis scripts (`add_session.py` / `common/paths.py`) 路径完全由 `.trellis/.developer` 的 `name=` 字段决定（动态模板 `<developer>`）
- 不能直接 `git mv` —— journal 内容已分叉（81k vs 36k），需要做内容合并决策
- 远端 19 个 commit 都已 push 到 origin/main → 改写历史需要 force push（不可逆）

## Assumptions (temporary)

- 远端另一台机器的 `.developer` 是 `carlos`（lowercase），原因待验证（可能是 `init_developer.py carlos` 在远端机器跑过）
- journal 内容是 AI 生成的 session 记录（"record journal" 模板），不是代码 → 合并两个 journal 是**信息合并**而非代码合并
- 用户愿意接受 `git push --force-with-lease`（不是普通 fast-forward）来收口远端历史

## Open Questions

- [x] **Q1 (BLOCKING)** — 决定：**A. merge 两个 journal**（保留全部 31 个 sessions 历史）
- [ ] **Q2 (PREFERENCE)**：journal merge 粒度？
    - 1. 全自动按 `### Session N` 标题 + 日期解析去重插入
    - 2. 手动复制粘贴，让用户审一遍
- [ ] **Q3 (BLOCKING)**：远端 19 个 commit 怎么处理？那些 commit 的 tree 里都包含了对 `carlos/journal-1.md` 的修改
    - A. `git push --force-with-lease` 把 commit 改写（commit hash 改，但 message/语义不变）
    - B. 接受历史漂移，新加一个 "chore: 统一 workspace 路径" commit 在最新 —— 但 `carlos/` 目录还会留着，git 视为删除
    - C. 不动远端，本地只做 `rm -rf carlos/` + git rm（远端 push 时产生 conflict）

## Requirements (evolving)

- R1: `.trellis/workspace/` 下最终只存在 `Carlos/`（uppercase，本地规范）
- R2: `Carlos/journal-1.md` 合并两个工作流的历史 session 记录
- R3: 7 处 uppercase 引用 + 1 处 lowercase bug 引用保持一致（全 uppercase）
- R4: 远端 history 收口（不再有 `carlos/` 路径的 track）
- R5: 未来再 `init_developer.py carlos` 也不会再创建 lowercase 副本（约束在 docs 里说明）

## Acceptance Criteria (evolving)

- [ ] AC1: `ls .trellis/workspace/` 只显示 `Carlos/` 目录
- [ ] AC2: `git ls-files | grep workspace/` 只显示 `Carlos/` 下的文件
- [ ] AC3: `git ls-files | grep -E 'carlos/'` 返回空（除了引用文本）
- [ ] AC4: `rg '\.trellis/workspace/carlos' .` 在 .md/.json/.jsonl 里 0 命中（除 changelog/历史 commit message）
- [ ] AC5: `Carlos/index.md` 的 `Total Sessions` 反映合并后的总数
- [ ] AC6: `pnpm build` 通过
- [ ] AC7: 远端 push 成功，`origin/main` 上 `carlos/` 路径无 track
- [ ] AC8: `docs/STRUCTURE.md` 不需要改（已经是 uppercase）

## Definition of Done (team quality bar)

- Tests / typecheck: pnpm build 通过（不需要 cargo test，因为 journal 改的是 markdown 不影响 Rust 编译）
- Docs 更新：docs/HANDOFF.md + docs/HACKING-*.md 加一段 "workspace 大小写约定"
- Spec 同步：`.trellis/spec/` 里没有遗漏的小写引用
- Rollback：force-push 前先 `git tag backup-pre-workspace-case-fix` 留回退点

## Out of Scope (explicit)

- 不修远端另一台机器的 `.developer` 配置（那个需要远端机器自己跑 `init_developer.py Carlos`）
- 不动 `.trellis/scripts/common/paths.py` 的常量（它们是正确的，case 由 .developer 决定）
- 不动 `.trellis/workspace/index.md`（全局 workspace 索引，不受影响）

## Technical Notes

### 关键 commit / 文件
- `.trellis/.developer` (gitignored, local only) — `name=Carlos`
- `.trellis/scripts/common/paths.py` — 路径常量定义（`DIR_WORKSPACE = "workspace"`, `FILE_JOURNAL_PREFIX = "journal-"`），动态模板 `<developer>` 来自 `.developer`
- `.trellis/scripts/common/developer.py` — `init_developer(name)` 创建 `workspace/<name>/`
- `.trellis/scripts/add_session.py` — 调 `get_workspace_dir()` 解析路径
- 远端首条 lowercase commit: `eb69e06` (2026-06-05 02:31:24)
- 本地首条 uppercase commit: `ce1a893`

### 风险
- **force-push 风险**：如果远端 19 个 commit 已经被其他人 / CI / branch 引用，force-push 会断引用
- **journal 内容合并风险**：merge 两个 journal 是文本操作，要保证不破坏 `### Session N` 编号连续性 + 日期排序
- **macOS/Windows 风险**：本地 WSL 不会撞 case，但用户如果换 mac 工作，文件系统默认 case-insensitive 就会撞 —— 必须留 doc 警告
