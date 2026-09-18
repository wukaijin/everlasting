---
name: init
description: Bootstrap or refresh a project's AGENTS.md — the per-project instruction file auto-loaded by every future session. Scans the repo (structure, entry points, build/test commands, conventions), distills a 项目结构 section wrapped in everlasting markers, and writes to the project root. Use when the user says "/init / 初始化项目 / 生成或刷新 AGENTS.md / repo map / 项目首印象", or when working in a project that has no AGENTS.md yet.
allowed-tools: [glob, grep, list_dir, read_file, write_file, edit_file]
---

# 项目冷启动（init）

目标：为当前项目生成或增量刷新根目录 `AGENTS.md`——它是每个后续会话自动加载的项目指令文件。这一步建立「首印象」：未来 agent 开局就知道模块职责、入口和常用命令，不必每场靠 grep 摸地形。

## 落点铁律

- 目标文件 = **项目根的 `AGENTS.md`**（与 README.md 同级），不是任何子目录。
- 若本会话 attach 在 worktree（系统提示会说明，或 cwd 路径含 `/worktrees/`）：先用 `git worktree list --porcelain` 找主仓库路径，把文件写到主仓库根；越界写会被权限审批拦住，如实说明用途即可。
- 只产出一个文件：不建目录骨架、不动 .gitignore、不建 EVERLASTING.md。
- 完成后**不要自动 commit**；建议用户提交入库（团队共享），由用户决定。

## 第一步：判定路径（幂等）

用 `read_file` 读项目根 `AGENTS.md`：

- **不存在** → **路径 A 全新生成**：按「模板」写整个文件。
- **存在且含 marker 对**（`<!-- everlasting:repo-map:start -->` 与 `<!-- everlasting:repo-map:end -->`）→ **路径 B 增量刷新**：重新扫描后，用 `edit_file` 把旧 marker 区块（含两行 marker）整体替换为新 marker 区块——用旧区块全文做 old_string，**区块外的每一个字节原样保留**，不要图省事重写整文件。
- **存在但无 marker**（纯手写）→ **路径 C 不写文件**：明确报告「文件已存在且非 /init 生成，为保护手写内容不自动改动」；如用户想接管，建议其自行把结构章节用 marker 对包裹后再重跑。

## 第二步：扫描（先广后深，控制轮数）

1. **广**：`list_dir` 根目录 + `glob` 摸 2-3 层目录结构，识别语言生态与构建系统（package.json / Cargo.toml / pyproject.toml / go.mod / Makefile / …）。
2. **深（选择性）**：读 README、构建/CI 配置、主入口文件，通常 3-6 个文件足够；构建/测试命令优先从配置文件（package.json scripts、Makefile、CI yml）拿实证，不要猜。
3. **忽略噪音**：node_modules / target / dist / build / out / vendor / .git / 各类缓存与生成物 / lockfile——不进 map、不逐个读。
4. **归纳，不是罗列**：map 写「模块职责 / 入口点 / 构建测试命令 / 模块间关系」；文件名清单未来随时 glob 可得，占行数是浪费。每一条都必须有扫描实证——不确定的宁缺毋滥，**禁止编造不存在的目录或命令**。
5. **不做机制性断言**：目录与文件「存在」你能看见，但它们「是否被某个系统加载/注入/消费」你**看不见**——扫描只见文件，不见运行机制。禁止写「X 会被自动加载 / X 是 Y 的注入槽位 / 共 N 个工具」这类判断（哪怕 X 真实存在），除非本次扫描中拿到了直接证据（如配置文件显式引用）。仓库里常有多工具并存的遗留文件（为其他 agent 写的指令文件、过时文档），如实写「存在」，不推断其地位。

## 模板（路径 A 全新生成；marker 区块内 = 你的归纳）

```markdown
<!-- 由 everlasting /init 生成；「项目结构」区块可随时重跑 /init 增量刷新。建议提交入库与团队共享。 -->
# <项目名>

<一句话定位：这是什么项目、核心技术栈>

## 项目结构

<!-- everlasting:repo-map:start -->
<顶层目录职责表（目录 → 职责一行）；入口点与启动路径；模块间关键依赖方向。全部来自实证，≤150 行。>
<!-- everlasting:repo-map:end -->

## 构建与测试

- 构建：<实证命令>
- 测试：<实证命令>
- lint / 格式化：<实证命令；没有就不写这行>

## 关键约定

<代码风格 / 分支与提交约定 / 目录纪律等；没有把握就整节约掉>
```

## 体量与质量

- repo map 章节（marker 区块内）≤ 150 行；整文件 ≤ 300 行。
- 每行经得起拷问：「未来会话真的会用到这条信息吗？」
- 路径 B 刷新时若发现「构建与测试」等 marker 外章节已过时，不要顺手改——报告差异即可（marker 外的改动必须出自人）。

## 完成动作

报告三件事：走了哪条路径（A/B/C）、生成或刷新了什么（区块行数与要点）、下一步建议（commit 与否）。
