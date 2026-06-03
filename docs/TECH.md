# TECH — 技术栈

> Everlasting 的"用什么库、为什么选这个库"。包括锁定项、候选但未定的、不用的、扩展功能新增的,以及选型决策。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),实现路径见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 技术栈

### 1.1 锁定项(经过调研验证)

| 类别       | 选择                    | 版本 / 备注                              |
|------------|-------------------------|------------------------------------------|
| 桌面框架   | Tauri 2                 | WebView2 / WebKitGTK 共享                |
| 前端       | React 18 + Vite         | Tailwind CSS,shadcn/ui 或自写组件         |
| 后端语言   | Rust 1.75+              | edition 2021                             |
| 异步运行时 | tokio                   | Tauri 已经用 tokio                       |
| LLM 框架   | **rig-core** 0.38.1     | 20+ provider,Agent 抽象,MCP 桥接        |
| MCP        | **rmcp** 0.16.0         | 官方 Rust SDK,server + client            |
| Git 操作   | **git2-rs**             | libgit2 绑定,worktree / diff / commit   |
| 数据库     | **sqlx** + SQLite       | 编译期 SQL 检查,async 友好               |
| 序列化     | serde + serde_json      | 标准选择                                 |
| 错误处理   | anyhow + thiserror      | 边界用 anyhow,领域用 thiserror           |
| HTTP       | reqwest                 | rig 内部用,但我们可能直接用             |
| 前端 diff  | react-diff-viewer       | 成熟的 diff 组件                         |
| 终端       | xterm.js + portable-pty | 跑 shell 命令的实时终端                 |

### 1.2 候选但暂不锁定

- **Editor**:Monaco vs CodeMirror 6 — Monaco 体积大但功能强;CodeMirror 6 轻量可定制。先用 CodeMirror 6
- **State management (frontend)**:Zustand vs Redux — Zustand 简单,Redux 工具链好。Zustand
- **Schema validation**:schemars 跟着 rmcp 走就行

### 1.3 明确不用

- ❌ **PyO3 / 嵌入 Python** — 跨平台打包噩梦
- ❌ **Electron** — Tauri 是更优选
- ❌ **LangChain / LangGraph (Python)** — Rust 自己写,DAG 调度几百行代码就够
- ❌ **dspy-rs** — 还不成熟
- ❌ **Anthropic Agent SDK / Codex SDK** — 学习目标要求自研

### 1.4 扩展功能新增依赖(随候选功能引入)

下表依赖**仅在对应功能被选中实施时引入**,先调研评估,避免堆栈污染。

| 类别 | 库 | 用途 | 对应功能 |
|------|-----|------|---------|
| 图像处理 | `image` | 客户端 resize / 格式转换 | BACKLOG §1 输入层图片 |
| HEIC 支持 | `libheif-rs` | 苹果 HEIC/HEIF 格式 | BACKLOG §1 输入层图片 |
| 哈希 | `blake3` | 图片去重 / 缓存 key | BACKLOG §1 输入层图片 |
| 模糊搜索 | `nucleo` | @文件补全(fzf 算法 Rust 端口) | BACKLOG §1 输入层 @文件 |
| gitignore 解析 | `ignore` | 过滤项目扫描范围 | BACKLOG §1 输入层 @文件 |
| 文件监听 | `notify` | memory / config 文件变更检测 | BACKLOG §3 Memory |
| YAML 解析 | `serde_yml` | frontmatter 解析 | BACKLOG §2 Skill / §3 Memory / §4 Role |
| TOML 解析 | `toml` | role / config 解析 | BACKLOG §4 Role |
| 飞书 SDK | 用现有 `feishu-integration` skill | 消息收发 | BACKLOG §6 飞书 |
| 命令面板(前端) | shadcn `command` (基于 `cmdk`) | 输入触发器 | BACKLOG §1 输入层 |
| 图表(前端) | `recharts` | 生成式 UI chart | BACKLOG §5 |
| 表格(前端) | `@tanstack/react-table` | 生成式 UI table | BACKLOG §5 |
| 表单(前端) | `react-hook-form` | 生成式 UI form | BACKLOG §5 |
| 工作流可视化 | `@xyflow/react` | DAG 编辑器(留 v2) | BACKLOG §4 编排 |
| 云端 | Cloudflare Workers + D1 (SQLite) | REST API + 状态存储 | BACKLOG §7 |

**说明**:
- `image`、`libheif-rs`、`nucleo`、`ignore`、`notify`、`serde_yml` 都是轻量、跨平台、纯 Rust 实现(除了 `libheif-rs` 需要系统 libheif)
- 前端不引入 UI 框架(MUI / Ant Design 太重),自己攒 + 用 shadcn primitives

---

## 2. 决策:rig-core 作为 LLM 抽象层

**为什么选 rig**:
- 已经被多个 production coding agent 验证(VT Code、Con、Neon app.build)
- 支持 20+ provider,后期切 OpenAI / 本地模型无痛
- 自带 `Agent<M>` 抽象,省掉"消息 → tool call → 循环"样板
- 自带 `MessageStore` trait,接 SQLite 顺

**风险**:
- 预 1.0,有 breaking change
- 缓解:锁版本,每次升级读 changelog

**备选**:用 `reqwest` 直接打 API。理由:极致学习价值。**但只在早期用**,后续切 rig(详见 [IMPLEMENTATION.md §2 步骤 1-3](./IMPLEMENTATION.md#2-实施路线图))。

---

## 3. 决策:不用 MCP 做内部通信,但用 MCP 做外部暴露

**MCP 不内用**:agent 调自己的工具,直接调 Rust 函数,不需要绕 MCP
**MCP 外暴露**:用 `rmcp` 起一个 MCP server,让 Claude Desktop / Cursor / 别的工具能调我们的工具

**意义**:你的工具集成了以后,不仅你的 app 能用,Claude Code 也能用。这是意外的杠杆点。

---

## 4. 决策:SQLite 是唯一存储

**不存什么在文件系统**:
- ❌ session 消息存 JSON 文件 — 难查询、难聚合
- ❌ 配置存 YAML 散落各处 — 难管理

**全存 SQLite**:
- 项目元数据
- session 元数据
- 消息历史(含 tool calls / tool results)
- 用量统计(token 数、模型、provider)
- agent 配置(system prompt 模板、工具启用状态)

**理由**:SQLite 是单文件、零运维、有 FTS5 用于历史搜索,完美匹配个人工具的规模。

---

## 5. 决策:Skill / Memory / Role 共用 frontmatter loader

**核心洞察**:三者的"载体"都是 Markdown + YAML frontmatter,只是**加载时机**不同。

| 类型     | 加载时机        | 触发方式        |
|----------|-----------------|-----------------|
| Memory   | 每次 LLM 调用前 | 自动            |
| Skill    | LLM 显式调      | `use_skill` tool |
| /command | 用户显式调      | 键盘 `/`        |
| Role     | session 启动时  | UI 选           |

> 完整加载机制、token 预算、四层 Memory 边界见 [BACKLOG.md §3 多层 Memory](./BACKLOG.md#3-多层-memory-与约束) 和 [BACKLOG.md §2 Agent Skill 系统](./BACKLOG.md#2-agent-skill-系统)。

**实现影响**:
- 一个 `MarkdownResource` 通用数据结构
- 一个 `ResourceLoader` 通用加载器
- 不同的"加载策略"在调用层实现(自动 / 显式)
- 三类资源复用同一套解析代码,新增类型零成本
