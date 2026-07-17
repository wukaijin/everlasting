# A6 README 打磨

> **状态**:planning — PRD ready for review(2026-07-17,user 决策已固化)。Scope = **方案 B 中等重写**(5 段结构),demo 留 backlog。

## Goal

把 `README.md` 从"2026-06-18 的过期快照"重构为"5 分钟第一印象 + 5 步上手 + 现状 + 文档索引",作为顶层入口,**不重复** `docs/` 深度设计文档(避免内容漂移)。

## Background — 现状事实(2026-07-17 Evidence)

### 已有资产

- **顶层 README.md**(67 行,中文,日期 2026-06-18):缺 E2 / B9+ / V2-2+ / C2+ / A2+ / A5+ / B6+ / B8 workflow 等 8 个近期大里程碑。
- **`docs/README.md`**(63 行):设计文档 5 维索引,组织良好,本次不动。
- **DESIGN.md §2.1 一句话定位**: "给个人用的 vibe coding 桌面工作台,WSL 是真正的家,agent core 自己写,顺带学 harness engineering。"
- **DESIGN.md §3.1 能力清单**(完整):25 个已实施项。
- **DESIGN.md §3.2 明确不做**:5 类硬约束。
- **CLAUDE.md §Common Commands**:完整开发命令(`pnpm tauri dev` / `pnpm tauri build` / `cargo check` + PKG_CONFIG_PATH WSL note / `RUST_LOG=debug`)。
- **GitHub Actions CI badge**:README 已显示(`ci.yml`),07-05 E1 落地。

### Gap

- 顶层 README 与 docs/README 关系不清晰(README 没明示"设计文档去 docs/README")
- "Quick start" 缺失(用户新克隆后,无 5 步上手)
- "能力矩阵" 散落(DESIGN §3.1 太长,不适合放 README)
- "当前状态 / 最近里程碑"过期 1 个月
- "代码结构" 过于简陋(只 3 个目录)

## Requirements

### 结构(5 段)

```
1. 项目是什么 + 一句话定位 + 3 个核心差异点(WSL/自研/持久工作环境)
2. 为什么存在 / 不是另一个 Claude Code 替代品(短,3-5 行)
3. 5 分钟上手(Quick Start):前提(prereqs)+ 5 步(clone → env → install → dev → 打开窗口)
4. 能力矩阵(已具备的 25 项,按"读/写/Shell/联网/技能/记忆/UI/交互/工作流/Subagent"10 类简表)
5. 架构 + 文档索引(指 docs/README.md 的 5 维拆分,不复制内容)
+ 末尾:状态行(只一行日期 + 指向 git log)+ 约束
```

### 内容来源映射(避免重写,只引用)

| 段 | 内容来源 | 复用度 |
|---|---|---|
| 1. 项目是什么 | DESIGN.md §2.1 + §2.3(改写为短段) | 改写,不复制原文 |
| 2. 为什么存在 | README.md 现有"不是另一个 Claude Code 替代品"段,精简 | 改写 |
| 3. Quick Start | CLAUDE.md §Common Commands + HACKING-wsl env setup 5 步 | 重新组织为新手向,**happy path only**,WSL 坑指向 HACKING-wsl |
| 4. 能力矩阵 | DESIGN.md §3.1(精简为 10 类简表 + 25 项一句话),**不加近期里程碑时间线** | 浓缩 |
| 5. 架构 + 文档索引 | docs/README.md(引用) | **不复制,只指链接** |
| 末尾状态行 | git log + ROADMAP §1 | 只一行日期 + "详见 git log" |

### 样式

- 中文为主(对齐现有 docs/ 中文规范)
- 命令块 / code blocks 英文(对齐现有 README 风格)
- 不加 emoji 装饰 / 不加 GIF / 不加 badges(除已有 CI badge)
- 不写"贡献指南"(个人项目,DESIGN §2.2 已定)
- 不写"License"(无 LICENSE 文件)

### 不做(明确 out of scope)

- ❌ 不做 demo 段(已留 backlog,A6 拆分决策)
- ❌ 不写英文版(对外公开非目标)
- ❌ 不加 screenshots / GIFs(无素材 + 个人使用)
- ❌ 不动 `docs/README.md`(组织良好,本次范围外)
- ❌ 不动 `docs/DESIGN.md` / `ROADMAP.md` / 其他设计文档(README 是引用而非复制)
- ❌ 不引入新依赖、不改 build 配置、不改 CI 配置

## Acceptance Criteria

- [ ] README.md 重写后 **总行数 ≤ 200 行**(信息密度合理,避免膨胀)
- [ ] **5 段结构**齐:项目是什么 / 为什么存在 / Quick Start / 能力矩阵 / 文档索引
- [ ] **Quick Start 段**只写 happy path 5 步(clone / env vars / install / dev / 打开窗口),WSL 环境坑指向 [docs/HACKING-wsl.md](./docs/HACKING-wsl.md),不复制坑内容到 README
- [ ] **能力矩阵**列出 ≥ 20 项已具备能力,按 ≥ 8 类分组(从 DESIGN §3.1 浓缩),**不加近期里程碑时间线**
- [ ] **当前状态行**同步到 2026-07-17,**只更新日期 + 指向 ROADMAP / git log**,不在 README 列里程碑清单
- [ ] **文档索引**清晰指 `docs/README.md`,README 自身不重复 docs/ 内容
- [ ] **不引入新内容事实**(新写的每条事实都能在 DESIGN/ARCHITECTURE/CLAUDE.md 找到源)
- [ ] `pnpm build` 通过(vue-tsc 0 err,跑一遍验证 README 改动无副作用)
- [ ] 现有 CI badge 保留
- [ ] diff 合理:删除原 README 大段,新增结构化 5 段

## Implementation Notes(轻量 task 不强制 implement.md)

### 文件改动清单

- `README.md`(唯一改动的文件):full rewrite,~150-200 行

### 验证

- `cd app && pnpm build`(验证 vue-tsc + vite build)
- `git diff README.md` 手动 review:确保 5 段结构清晰 + 链接全部可达

### 风险 / 回滚

- 风险:极低(单文件文本改动,无代码逻辑)
- 回滚:`git checkout HEAD~ -- README.md`

## 已解决的决策(Brainstorm 结论,2026-07-17)

- **范围**:中等重写(方案 B),不是最小刷新也不是全面公开化
- **Quick Start 详略**:只写 happy path,WSL 坑指向 HACKING-wsl.md
- **能力矩阵**:不加近期里程碑时间线
- **顶层 README vs docs/README**:顶层 = 5 分钟第一印象,docs/README = 设计文档索引(分工清晰,不重复)

## Notes

- 用户决策(2026-07-17):中等重写,5 段结构,**不做 demo**
- 与 A5 错误处理独立,本 task 不涉及
- 与现有 docs/README.md 互补,不重复内容