# TECH — 技术栈

> Everlasting 的"用什么库、为什么选这个库"。包括锁定项、候选但未定的、不用的、扩展功能新增的,以及选型决策。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 技术栈

### 1.1 锁定项(经过调研验证)

| 类别       | 选择                    | 版本 / 备注                              |
|------------|-------------------------|------------------------------------------|
| 桌面框架   | Tauri 2                 | WebView2 / WebKitGTK 共享                |
| 前端       | **Vue 3.4+** + Vite     | `<script setup>` 组合式 API + Pinia + reka-ui / shadcn-vue |
| 后端语言   | Rust 1.75+              | edition 2021                             |
| 异步运行时 | tokio                   | Tauri 已经用 tokio                       |
| LLM 框架   | **(未采用)** rig-core 0.38.1 | Step 3b-2 rig-core 迁移已废弃 (2026-06-09),自研 `Provider` trait + 手写 SSE 已完整支持 Anthropic / OpenAI 双 Provider,详见 §2 决策 + [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION.md#4-决策日志) |
| MCP        | **(已移除)** ~~rmcp 0.16.0~~ | A3 MCP 外暴露 2026-06-10 V2 重排移除,rmcp 从 Cargo.toml 删除(详见 §3) |
| Git 操作   | **git2-rs**             | libgit2 绑定,worktree / diff / commit   |
| 数据库     | **sqlx** + SQLite       | 编译期 SQL 检查,async 友好               |
| 序列化     | serde + serde_json      | 标准选择                                 |
| 错误处理   | anyhow + thiserror      | 边界用 anyhow,领域用 thiserror           |
| HTTP       | reqwest                 | 直接用,自研 Provider trait 内部也走 reqwest |
| 前端 diff  | `diff` (jsdiff) + 自渲染 | 框架无关,Vue 包装;`app/src/components/chat/DiffView.vue` 用 `parsePatch` |
| 终端       | xterm.js + portable-pty | 跑 shell 命令的实时终端                 |

### 1.2 候选但暂不锁定

- **Editor**:Monaco vs CodeMirror 6 — Monaco 体积大但功能强;CodeMirror 6 轻量可定制。先用 CodeMirror 6
- **State management (frontend)**:Pinia(已锁定,跟随 Vue 官方)
- **UI 组件库**:reka-ui vs shadcn-vue vs radix-vue — 三家都是 unstyled + accessible primitives。先 reka-ui
- **Schema validation**:schemars 跟着 rmcp 走就行

### 1.3 明确不用

- ❌ **PyO3 / 嵌入 Python** — 跨平台打包噩梦
- ❌ **Electron** — Tauri 是更优选
- ❌ **LangChain / LangGraph (Python)** — Rust 自己写,DAG 调度几百行代码就够
- ❌ **dspy-rs** — 还不成熟
- ❌ **Anthropic Agent SDK / Codex SDK** — 学习目标要求自研

### 1.4 扩展功能新增依赖(随候选功能引入)

下表依赖**仅在对应功能被选中实施时引入**,先调研评估,避免堆栈污染。**功能排期归 [ROADMAP.md §2 V2 路线图分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),本表只列技术细节**。

| 类别 | 库 | 用途 | 对应功能 |
|------|-----|------|---------|
| 图像处理 | `image` | 客户端 resize / 格式转换 | BACKLOG §1 输入层图片 |
| HEIC 支持 | `libheif-rs` | 苹果 HEIC/HEIF 格式 | BACKLOG §1 输入层图片 |
| 哈希 | `blake3` | 图片去重 / 缓存 key | BACKLOG §1 输入层图片 |
| 模糊搜索 | ~~`nucleo`~~(未采用) | @文件补全(B2,2026-06-17 落地)改用更简实现,未引入 nucleo | BACKLOG §1 输入层 @文件 |
| gitignore 解析 | `ignore` | 过滤项目扫描范围 | BACKLOG §1 输入层 @文件 |
| 文件监听 | ~~`notify`~~(已移除) | memory watcher 原用 notify,后改为 mtime fence freshness check(read_guard 防过期),notify 已从依赖删除 | BACKLOG §3 Memory |
| YAML 解析 | 手写 parser(B3);~~`serde_yml`~~(已废弃) | frontmatter 解析 | BACKLOG §2 Skill / §3 Memory / §4 Role / B3 /command |
| TOML 解析 | `toml` | role / config 解析 | BACKLOG §4 Role |
| 飞书 SDK | 用现有 `feishu-integration` skill | 消息收发 | BACKLOG §6 飞书 |
| 命令面板(前端) | reka-ui `command` (或自写 `<TriggerMenu>`) | 输入触发器 | BACKLOG §1 输入层 |
| 图表(前端) | `ECharts` + `vue-echarts` | 生成式 UI chart | BACKLOG §5 |
| 表格(前端) | `@tanstack/vue-table` | 生成式 UI table | BACKLOG §5 |
| 表单(前端) | `vee-validate` | 生成式 UI form | BACKLOG §5 |
| 工作流可视化 | `@vue-flow/core` | DAG 编辑器(后期再加) | BACKLOG §4 编排 |
| 云端 | Cloudflare Workers + D1 (SQLite) | REST API + 状态存储 | BACKLOG §7 |

**说明**:
- `image`、`libheif-rs`、`nucleo`、`ignore`、`notify` 都是轻量、跨平台、纯 Rust 实现(除了 `libheif-rs` 需要系统 libheif)
- **`serde_yml` 已废弃(2026-06-16 发现)**:`serde_yml` + 前代 `serde_yaml` 均在 crates.io 标 "Deprecated"(`0.0.13` 仅 compat shim)。B3 `/command` 的 frontmatter(`name` / `description` / `argument-hint` 单行标量)改用**手写 parser**(`app/src-tauri/src/resource_loader.rs::parse_frontmatter`,~40 行,split `---` + `key:value`),零依赖。未来 Skill / Memory / Role frontmatter 字段复杂化(多行 / 数组)时再上 maintained fork(候选 `serde_yaml_neo`)——§5 共享 loader 契约仍成立(parser 隔在 `parse_frontmatter` 函数后,替换局部)。
- 前端不引入 UI 框架(Element Plus / Vuetify 太重),自己攒 + 用 reka-ui / shadcn-vue primitives
- **ECharts 替代 recharts 的理由**:recharts 纯 React,跨框架方案 ECharts + vue-echarts 更成熟,中文文档全

---

## 2. 决策:rig-core 弃用(2026-06-09),改自研 Provider trait

**历史背景**:原计划步骤 3b-2 切到 rig-core 0.38.1,作为 LLM 抽象层(2026-06-04 决策,见 [IMPLEMENTATION §4 决策日志 2026-06-04 段](./IMPLEMENTATION.md#4-决策日志))。理由:
- 20+ provider 支持,后期切 OpenAI / 本地模型无痛
- 自带 `Agent<M>` 抽象,省掉"消息 → tool call → 循环"样板
- 自带 `MessageStore` trait,接 SQLite 顺

**弃用原因**(2026-06-09 决策,见 [IMPLEMENTATION §4 决策日志 2026-06-09 段](./IMPLEMENTATION.md#4-决策日志)):
- 学习价值:自研 Provider trait 比用 rig 学到更多 harness 细节
- 控制粒度:rig 帮你做了"消息流 → tool call → 循环",自研可以插自定义逻辑(权限、审计、统计)
- 风险:rig 预 1.0,breaking change 风险,锁版本治标不治本
- 已自实现等价能力:`Provider` trait + `AnthropicProvider` / `OpenAIProvider` + `provider::wire` WireMessage 跨协议中间层,2026-06-08/09 4 PR 落地

**当前架构**(2026-06-09 起):
- `llm/provider.rs` 定义 `Provider` trait:`async fn chat_stream(&self, req: ChatRequest) -> Stream<Item = ChatEvent>`
- `AnthropicProvider`:1:1 保留原 reqwest + 手写 SSE 路径(行为完全不变)
- `OpenAIProvider`:`provider::wire` WireMessage 中间层抽象协议差异,`strip_unsupported` 静默降级
- `llm/client.rs` 通过 `catalog` 调度到具体 provider(已存 DB),不直接绑 URL/model
- 详见 `.trellis/spec/backend/llm-contract.md` "Scenario: Multi-Provider Abstraction (PR1)" section

**未来考虑**:rig-core 仍可作为"快速接新 provider"的工具引入,但不在当前路线图。

---

## 3. 决策:不用 MCP 做内部通信,但用 MCP 做外部暴露

**MCP 不内用**:agent 调自己的工具,直接调 Rust 函数,不需要绕 MCP
**MCP 外暴露**:用 `rmcp` 起一个 MCP server,让 Claude Desktop / Cursor / 别的工具能调我们的工具

**意义**:你的工具集成了以后,不仅你的 app 能用,Claude Code 也能用。这是意外的杠杆点。

> ⚠️ **更新(2026-06-10 V2 重排)**:MCP 外暴露(A3)已移除——个人工具杠杆不足,Claude Desktop / Cursor 集成不在当前使用场景。`rmcp` 从 Cargo.toml 删除,`src/` 无残留引用。本节保留为历史 ADR;若未来需要外部集成,可重新引入 rmcp(0.16.0+ server + client)。

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
