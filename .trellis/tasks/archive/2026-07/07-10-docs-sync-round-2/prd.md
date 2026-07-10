# 07-10-docs-sync-round-2 — 正式文档同步 P1 + P2

> **本任务复用 `07-10-docs-sync-sweep/` 完整 plan**:prd.md / design.md / implement.md 三份 artifact 已写好 P0+P1+P2 全量内容,本轮直接执行 P1+P2 6 个 commit,P0 已收。
>
> P0 收官 commit:`37e403f` (impl §4 ADR) + `93a7b80` (roadmap B8 迁移) + `4b20e56` (structure 后端树补 5 块)。

## 本轮范围

**P1(3 文档 3 commit)** — 主代码段 + tool 数修正:
- `CLAUDE.md` — Architecture 段后端树补 workflow / background_shell / commands/task / commands/subagents / llm retry+wire;tools 19→21;核对 `LLM_MODEL` 默认值
- `docs/ARCHITECTURE.md` — §1.1 Tool Registry 工具数 19→21;merge/discard worker 标 `ToolKind::GitMutation`;新增 "Workflow 系统" 子节
- `docs/DESIGN.md §3.1` — B8 从"未做"段迁至"已具备";工具数 19→21;补 `agent/workflow/` 模块描述

**P2(3 文档 3 commit)** — 修补 + 笔记:
- `docs/TECH.md` — §1.4 补 workflow 工具 / retry 层依赖归属
- `docs/HACKING-llm.md` — 补 A5+ retry 策略(retry_open + Full Jitter + 首字节前重试)
- `docs/HACKING-wsl.md` — 顶部 WSL 版本注脚;补 PKG_CONFIG_PATH 完整说明;坑 1~11 校对

**总**:6 commit,跨文档一致性 review + sanity cargo check。

## 引用 P0 artifact

完整 prd / design / implement 见:

- `../07-10-docs-sync-sweep/prd.md`(141 行)
- `../07-10-docs-sync-sweep/design.md`(184 行)
- `../07-10-docs-sync-sweep/implement.md`(185 行,Step 4-9 即本轮执行清单)

## Acceptance Criteria

### P1 验收

- [ ] `CLAUDE.md` Architecture 段后端树包含 `agent/workflow/` + `background_shell/` + `commands/task.rs` + `commands/subagents.rs`
- [ ] `CLAUDE.md` tools 列表反映 21 个 builtin tool
- [ ] `CLAUDE.md` `llm/` 树包含 `retry.rs`;`provider/` 提到 `wire.rs`
- [ ] `CLAUDE.md` Environment Variables `LLM_MODEL` 默认值与 `config::from_env` 实现一致
- [ ] `docs/ARCHITECTURE.md` §1.1 Tool Registry 工具数 = 21
- [ ] `docs/ARCHITECTURE.md` 有 "Workflow 系统" 子节
- [ ] `docs/ARCHITECTURE.md` Tool Registry 中 `merge_worker` / `discard_worker` 标注为已落地的 `ToolKind::GitMutation`
- [ ] `docs/DESIGN.md §3.1` "未做"段不包含 B8
- [ ] `docs/DESIGN.md §3.1` 工具数 = 21,包含 `agent/workflow/` 模块描述

### P2 验收

- [ ] `docs/TECH.md` 提到 workflow 工具依赖(指向 `llm/retry.rs` 等)
- [ ] `docs/HACKING-llm.md` 包含 A5+ retry 策略(retry_open + Full Jitter + 首字节前重试)条目
- [ ] `docs/HACKING-wsl.md` 顶部 WSL 版本号正确(或注明"截至 2026-07-10")
- [ ] `docs/HACKING-wsl.md` 包含 PKG_CONFIG_PATH 完整说明

### 跨文档一致性

- [ ] `git diff --stat` 共 6 个文件被改动,无其他文件
- [ ] 6 个文件的总 diff 行数 < 1400 行
- [ ] 所有 commit message 遵循 `docs(<scope>): <action>` 风格

## Notes

- 顺序:P1 先做(P1 包含 tool 数 / workflow 子节 / LLM_MODEL 等关键事实修正),P2 后做(纯增补)
- 文档之间互引:CLAUDE.md 改完回头核 ARCHITECTURE / DESIGN 是否仍引用旧路径(以本轮 commit 为准)
- HACKING-wsl 坑 1~11 校对:扫一遍现有 11 条坑,过期项更新;准确项不动;无法核对的项标 "TBD 下一轮"