# N3 项目冷启动 /init:GlobalBuiltin skill + AGENTS.md(repo map 内嵌)

## Goal

新项目一条 `/init` 一站式完成 agent 首印象建立:LLM 扫 repo 归纳结构,生成 `<project>/AGENTS.md`(repo map 作为内嵌章节),经 B5 loader 自动注入后续所有 session——解「4 个指令文件手写」与「每 session 靠 grep 摸地形」两个痛点(BACKLOG 附录 B.1 候选 N3,群聊共识 P1)。

## Background

- 现状(2026-09-18 勘察):`create_project`(`db/projects.rs:17`)只插 DB 行,磁盘零初始化;memory Project 层 Missing 是预期态,新项目天然零注入。repo map 代码库零现成实现。
- 项目级路径约定全景(memory 双槽 + `.everlasting/{commands,skills,agents,workflow}`)均「缺目录静默空」,不需要预建骨架。
- 用户 2026-09-18 立项并定范围:唯一磁盘交付物 = `<project>/AGENTS.md`(内嵌 repo map);不预建 `.everlasting/` 骨架、不自动建项目级 `EVERLASTING.md`、不动 `.gitignore`(入库与否由用户自行 commit,daemon 遵循不自动 commit 原则)。

## Requirements

- **R1 交付物**:`/init` 产出 `<project>/AGENTS.md`,含模板章节(项目简介/构建与测试命令/关键约定)+ marker 包裹的「项目结构」(repo map)章节。不产出其他任何文件。
- **R2 执行者**:GlobalBuiltin skill,命名 `init`(N1 层,`/` 面板即字面 `/init` 体验)。执行 = 当前 session 的 LLM 经 `use_skill` L1 渐进披露加载 SKILL.md 指引,用既有工具面(glob/grep/list_dir/read_file + write_file/edit_file)完成扫描与落盘。零机制新增(除单行注册)、不做内置命令模板扩展、不做 GUI/daemon 机械生成。
- **R3 幂等(区块 marker 增量更新)**:
  - AGENTS.md 不存在 → 生成全模板;
  - 存在且含 marker 区块 → 只重写 marker 区块内内容,**区块外字节不动**(指引规定用 `edit_file` 做区块替换,结构性保障);
  - 存在但无 marker(纯手写)→ 不改文件,报告并退出。
- **R4 repo map 质量与体量**:内容 = LLM 归纳(模块职责 / 入口点 / 构建测试命令 / 关键约定),不是机械文件列表(LLM 自己 glob 即得,无价值);体量走指引软目标(repo map 章节 ≤150 行),硬闸继承既有机制(B5 单文件 100 KiB 上限 / memory-gov digest / unified-context-budget)。

## Technical Notes

- 落点:默认 session cwd = 主 repo(worktree 为显式 attach,非默认);SKILL.md 指引须规定 AGENTS.md 落**主 repo 根**——attach worktree 的 session 中需写主 repo 绝对路径(权限层 5-tier 把关),此场景为指引级要求,不新开机制。
- marker 格式:HTML 注释对(`<!-- everlasting:repo-map:start -->` / `<!-- everlasting:repo-map:end -->`),入库共享无害。
- 注册面:`resources/builtin-skills/init/SKILL.md` 新文件 + `skill/loader.rs:340-348` 单行 `include_str!`;继承 GlobalBuiltin 的 daemon-only 可用域与用户同名覆盖语义(N1 既有行为)。
- 指引资产(不写死在 PRD,design/实现期起草):扫 repo 的策略(glob 深度/忽略 node_modules 等噪音目录/读关键入口文件)、AGENTS.md 章节骨架、增量更新流程。

## Acceptance Criteria

- [ ] **AC1 挂载**:daemon 模式 session 的 `/` 面板出现 `init` skill;选中后编辑器插入 `/init `,发送后 agent 经 use_skill 加载指引(复用 N1 管线,无新机制)。
- [ ] **AC2 首跑生成**:无 AGENTS.md 的 repo 跑 `/init` → `<project>/AGENTS.md` 落盘,含 marker 区块 + 模板章节;memory 预览面板 Project AGENTS.md 层变 Loaded。
- [ ] **AC3 增量更新**:已有含 marker 的 AGENTS.md(含手写章节)重跑 `/init` → 仅 marker 区块内内容变化,区块外字节零改动;repo map 反映当前结构。
- [ ] **AC4 纯手写保护**:无 marker 的手写 AGENTS.md 重跑 → 文件零改动,session 中报告原因与建议。
- [ ] **AC5 live 质量**:真实 repo live 一场,repo map 章节含模块职责/入口/构建测试命令,人审无编造(不存在目录/命令);体量 ≤ 软目标。
- [ ] **AC6 回归**:cargo `--lib` + pnpm test + vue-tsc 全绿(skill 层注册新增单测;loader 既有用例不回归);ROADMAP §1.2 落账 + BACKLOG N3 销项。

## Out of Scope

- repo map 的持续自动刷新(结构演进后的重刷靠手动重跑 `/init`;若需定时/钩子触发记 follow-up)。
- `.everlasting/` 目录骨架、项目级 EVERLASTING.md、.gitignore 建议(2026-09-18 用户裁定不做)。
- 跨机器同步(BACKLOG §4);GUI 按钮/daemon 机械生成路径;内置命令模板机制扩展。
