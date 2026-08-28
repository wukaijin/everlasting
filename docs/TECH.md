# TECH — 技术栈

> Everlasting 的"用什么库、为什么选这个库"。包括锁定项、候选但未定的、不用的、扩展功能新增的,以及选型决策。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 技术栈

### 1.1 锁定项(经过调研验证)

| 类别       | 选择                    | 版本 / 备注                              |
|------------|-------------------------|------------------------------------------|
| 桌面框架   | Tauri 2                 | WebView2 / WebKitGTK 共享                |
| 前端       | **Vue 3.4+** + Vite     | `<script setup>` 组合式 API + Pinia + reka-ui / shadcn-vue + **Tailwind CSS 4**(`^4.3.0`,原子样式层,见 `app/package.json`)+ **marked 18** + **DOMPurify 3.4**(markdown 渲染 + XSS 白名单,见 [HACKING-markdown.md](./HACKING-markdown.md)) |
| 后端语言   | Rust 1.75+              | edition 2021                             |
| 异步运行时 | tokio                   | Tauri 已经用 tokio                       |
| LLM 框架   | **(未采用)** rig-core 0.38.1 | Step 3b-2 rig-core 迁移已废弃 (2026-06-09),自研 `Provider` trait + 手写 SSE 已完整支持 Anthropic / OpenAI 双 Provider,详见 §2 决策 + [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION/decisions-2026-06.md) |
| MCP        | **(已移除)** ~~rmcp 0.16.0~~ | A3 MCP 外暴露 2026-06-10 V2 重排移除,rmcp 从 Cargo.toml 删除(详见 §3) |
| Git 操作   | **git2-rs**             | libgit2 绑定,worktree / diff / commit   |
| 数据库     | **sqlx** + SQLite       | 编译期 SQL 检查,async 友好;**现为 daemon / remote 两套独立 SQLite**(remote 存 nodes / devices / pairing_codes) |
| 序列化     | serde + serde_json      | 标准选择                                 |
| 错误处理   | anyhow + thiserror      | 边界用 anyhow,领域用 thiserror           |
| HTTP       | reqwest                 | 直接用,自研 Provider trait 内部也走 reqwest |
| 前端 diff  | `diff` (jsdiff) + 自渲染 | 框架无关,Vue 包装;`app/src/components/chat/DiffView.vue` 用 `parsePatch`(B9 复用只读展示) |
| 后台 shell | `tokio::process::Child` | L1a(2026-06-19 落地,**不带 PTY**);`BackgroundShellRegistry` trait + 进程内 impl;session-scoped,默认 `max_runtime_ms` 24h。`portable-pty` / `xterm.js` 留 L1b follow-up |
| 模糊搜索(前端) | `fuzzysort` ^3.1.0 | @文件补全 B2 PR1(替代 `nucleo`,极轻量 TS 库) |
| 服务端框架(daemon / remote 复用) | **axum 0.7** | 含 ws feature;`everlasting-daemon` 与 remote daemon 共用(remote epic 08-11~13) |
| WSS 隧道 | **tokio-tungstenite 0.24**(PC 侧) | 自研 WSS 隧道协议(Frame/StreamEvent),不用 frp / rathole / yamux |
| 前端路由 | **vue-router 4** | `/pairing` / `/nodes` / `/chat`(remote epic 移动端 PWA) |
| PWA 壳 | **vite-plugin-pwa** | 移动端 PWA 壳(remote epic) |

### 1.2 候选但暂不锁定

- **State management (frontend)**:Pinia(已锁定,跟随 Vue 官方)
- **UI 组件库**:reka-ui vs shadcn-vue vs radix-vue — 三家都是 unstyled + accessible primitives。先 reka-ui

> **已锁定(2026-08-18 同步)**:~~**Editor**:Monaco vs CodeMirror 6 — 先用 CodeMirror 6~~ → **CodeMirror 6 已落地**(B2 PR1.5 2026-06-17,commits `1ed212c` / `8e7c975`,用于 `SearchModal` / 权限 Modal 等轻量编辑场景);Monaco 体积 + ~3MB bundle 仍偏大,走 `vue-codemirror` 包装;后续若需 LSP / 多光标再评估 Monaco

### 1.3 明确不用

- ❌ **PyO3 / 嵌入 Python** — 跨平台打包噩梦
- ❌ **Electron** — Tauri 是更优选
- ❌ **LangChain / LangGraph (Python)** — Rust 自己写,DAG 调度几百行代码就够
- ❌ **dspy-rs** — 还不成熟
- ❌ **Anthropic Agent SDK / Codex SDK** — 学习目标要求自研
- ❌ **frp / rathole / yamux** — 隧道自研 WSS(tokio-tungstenite 0.24),见 §1.1
- ❌ **Cloudflare Tunnel / Tailscale Funnel** — remote 中继落定为国内 2C2G 服务器 + 自研 Rust remote daemon(HTTPS 用户自理 nginx 反代,非 Tunnel)
- ❌ **主动推送** — 永久不做(PC daemon 一等公民,远程为 opt-in 附加层)

### 1.4 扩展功能新增依赖(随候选功能引入)

下表依赖**仅在对应功能被选中实施时引入**,先调研评估,避免堆栈污染。**功能排期归 [ROADMAP.md §2 V2 路线图分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),本表只列技术细节**。

| 类别 | 库 | 用途 | 对应功能 |
|------|-----|------|---------|
| 图像处理 | `image` | 客户端 resize / 格式转换 | BACKLOG §1 输入层图片 |
| HEIC 支持 | `libheif-rs` | 苹果 HEIC/HEIF 格式 | BACKLOG §1 输入层图片 |
| 哈希 | `blake3` | 图片去重 / 缓存 key | BACKLOG §1 输入层图片 |
| 模糊搜索 | `fuzzysort` ^3.1.0(B2 PR1 实际采用,前端 TS 库) | @文件补全 | BACKLOG §1 输入层 @文件 |
| gitignore 解析 | **未引入 `ignore` crate**(B2 改用更简实现) | 过滤项目扫描范围 | BACKLOG §1 输入层 @文件 |
| 文件监听 | ~~`notify`~~(已移除) | memory watcher 原用 notify,后改为 mtime fence freshness check(read_guard 防过期),notify 已从依赖删除 | BACKLOG §3 Memory |
| YAML 解析 | 手写 parser(B3 + L3d 2026-06-25/26 已支持数组 frontmatter) | frontmatter 解析 | BACKLOG §2 Skill / §3 Memory / §4 Role / B3 /command / **L3d subagent frontmatter(`SubagentDef.tools` 数组 + `arguments` 等)** |
| TOML 解析 | `toml` | role / config 解析 | BACKLOG §4 Role |
| 飞书 SDK | 用现有 `feishu-integration` skill | 消息收发 | BACKLOG §6 飞书 |
| 命令面板(前端) | reka-ui `command` (或自写 `<TriggerMenu>`) | 输入触发器 | BACKLOG §1 输入层 |
| 图表(前端) | `ECharts` + `vue-echarts` | 生成式 UI chart | BACKLOG §5(**B9 当前未引入,B9 落地范围:selector 复用 ask_user_question + code_block hljs + diff 复用 DiffView;chart/table/form 推后期**) |
| 表格(前端) | `@tanstack/vue-table` | 生成式 UI table | BACKLOG §5(**B9 当前未引入**) |
| 表单(前端) | `vee-validate` | 生成式 UI form | BACKLOG §5(**B9 当前未引入**) |
| 后台 shell 实现 | `tokio::process::Child` + `BackgroundShellRegistry` trait | L1a(进程内 impl,daemon 化换 impl 不动调用点) | L1a 2026-06-19 落地 |
| 工作流可视化 | `@vue-flow/core` | DAG 编辑器(后期再加) | BACKLOG §4 编排 |
| 云端(remote 中继) | ~~Cloudflare Workers + D1 (SQLite)~~ → **自研 Rust remote daemon**(`crates/everlasting-remote`:axum 0.7 含 ws feature + sqlx + dashmap + subtle,**零系统库依赖**) | 国内 2C2G 服务器,HTTPS 用户自理(nginx 反代,非 Cloudflare Tunnel);REST API + 状态存储 | remote epic 已落地(08-11~13),部署见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) |

**已落地但不引入新 crate 的基础设施模块**(07-02~07-10):

| 模块 | 路径 | 备注 |
|------|------|------|
| LLM 网络健壮性(A5+)| `app/src-tauri/src/llm/retry.rs` | `retry_open` wrapper(Full Jitter + 首字节前重试 + retry-after 解析);07-05 落地,**零新增依赖**(用既有 `tokio` + `reqwest` + `rand`) |
| 后台 shell(L1a)| `app/src-tauri/src/background_shell/` | `BackgroundShellRegistry` trait + `InMemoryBackgroundShellRegistry` 进程内 impl;tokio `Child` + `tokio::select!`,**零新增依赖** |
| Workflow 系统(B8)| `app/src-tauri/src/agent/workflow/` | workflow.json 外置 + builtin dev plugin + task 状态机 + breadcrumb + delegation;`create_task` / `request_task_state_transition` LLM tool;07-08~10 落地,**零新增依赖**(用既有 `serde` + `tokio::fs`;notify 已移除,freshness 走 mtime fence) |
| 生成式 UI(B9)| `app/src-tauri/src/tools/use_ui.rs` | non-blocking execute + UiPrimitive registry;07-02 部分落地,推后期(button+action 白名单 / diff 应用 / session 开关) |
| 自主记忆(V2 2 期)| `app/src-tauri/src/agent/{auto_reflect,memory_recall,memory_hygiene,remember}.rs` + `db::autonomous_memories` 表 | FTS5 + pitfall trigger_key + 状态机(candidate→active→verified)+ 异步卫生 job;06-29 落地,**零新增依赖**(用既有 `sqlx` FTS5) |
| E2 turn-level harness trace(07-14)| `app/src-tauri/src/db/turn_trace.rs` + `db/migrations/schema.rs:222` | `turn_trace` 表 + 4 个 ChatEvent(`TokenUsage` / `Compaction` / `LoopHint` / `Breadcrumb`)+ `session_audit_events.turn_seq` 列(nullable,审计按 turn 落表);`scripts/turn-smoke.sh` 单轮烟测,**零新增依赖** |
| C7 tools token 治理(08-14)| `app/src-tauri/src/tools/{mod.rs,filter_tools_for_session_type}.rs` + `agent/drive.rs` 第 3 环 | `STUB_CANDIDATES` 静态裁剪 + `turn_trace.tools_token INTEGER` 列(`add_turn_trace_column_if_missing` backfill,`db/migrations/schema.rs:994-999`);session 起步 -38.5%,**零新增依赖** |
| C7D tools stub 注册(08-14)| `app/src-tauri/src/tools/stub.rs` + `agent/drive.rs` 第 4 环 | `StubRegistry`(session 粘性 loaded-set)+ `load_tool_schemas` 元工具 + `tools_stub_enabled` gate;C7+C7D 联合 -62%,**零新增依赖** |
| memory-gov 指令块治理(08-15)| `app/src-tauri/src/memory/digest.rs` + `agent/drive.rs` + `tools/load_memory_sections.rs` | `MemoryDigestRegistry` OnceLock + fence-aware 切节注入(标题+首句;`AGENTS.md` primary 永不 digest / `CLAUDE.md` 且 tokens>600 才 digest)+ `load_memory_sections` 元工具 + `turn_trace.memory_token INTEGER`;**零新增依赖** |
| B1 image multimodal(08-16/17)| `app/src-tauri/src/attachments/` + `app/src-tauri/src/llm/wire.rs::ContentBlock` + `daemon/routes/attachments.rs` + `db/migrations/schema.rs:1012` + `:1006` | `ContentBlock::Image` / `ImageRef` 双形态(Anthropic 优先发 `Image`,OpenAI / 不支持 vision 走 `ImageRef` 占位降级)+ `models.supports_images` 配置 + `messages.metadata.attachments[]` 引用 + **首个二进制 GET 路由** `/api/v1/attachments/<id>`(`tower-http::set_header(Content-Type, mime_guess)`)+ `turn_trace.images_token INTEGER`;**新增直接依赖 `imagesize = "0.13"`**(B1 PR4 08-16,读图文件头拿 w/h;`mime_guess` 已借 `axum` / `tower-http` 间接带入) |
| D2 跨 session 全文搜索(08-17)| `app/src-tauri/src/db/{search,messages_fts}.rs` + `db/migrations/schema.rs:1051` + `commands/search_messages.rs` + `app/src/components/SearchModal.vue` | `messages_fts` FTS5 虚拟表(external-content + trigram + `UPDATE OF text` 防写放大 + `messages_fts_docsize` 影子表守卫回填)+ `db/search.rs` 双路分派(FTS rowid→主表;0 命中 LIKE 兜底)+ `search_messages` POST IPC + `SearchModal` 两态 + Cmd/Ctrl+K 接管;**零新增依赖**(`sqlx` FTS5 已带) |
| D2② agent search_history(08-17)| `app/src-tauri/src/tools/search_history.rs` + `app/src/components/SearchHistoryCard.vue` | 薄封装 `db::search::search_messages` + `READONLY_TOOL_ALLOWLIST` 第 6 员 + 4 态机(loading/empty/results/error);**零新增依赖** |
| F4 web_search 工具(08-25)| `app/src-tauri/src/tools/web_search/` + `db/config.rs` + `commands/` | enum dispatch 双后端(Tavily keyed / DDG 兜底)+ 30s 预算重试环;key 三态 AEAD 配置(`app_config`,aad=web_search)+ Settings 第 7 tab;`READONLY_TOOL_ALLOWLIST` 第 7 员 + C7D `STUB_CANDIDATES` 第 11 员;**零新增依赖**(用既有 reqwest + 自研 retry) |
| F5 PDF/docx 原生提取(08-26)| `app/src-tauri/src/agent/doc_extract.rs` + `db/config.rs` | **3 个新增依赖**:`pdf-extract = "0.12"`(vendored path `vendor/pdf-extract`,纯 Rust 读 PDF 文本)+ `quick-xml = "0.42"`(docx `w:t` 提取)+ `calamine = "0.36"`(xlsx/xlsm,chrono feature);`zip` 收紧 `default-features=false, features=["deflate"]`(防 zstd-sys C 编译依赖);三级 cap + `at_files_token` 度量 |
| F6/F3 异步编排(08-27)| `app/src-tauri/src/commands/sessions.rs` + `chat.rs` + `sidecar.rs` | `SessionSummary.busy` enrich + 轮次终结 toast + `max_concurrent_loops` 信号量 + Tauri 壳关闭确认;**零新增依赖** |
| F2/F2b 定时任务(08-28)| `app/src-tauri/src/scheduler/` + `db/scheduled_tasks.rs` + `commands/scheduled_tasks.rs` | daemon 常驻 30s tick 调度器(单一扫描 + `due` 落账 + catch-up)+ `scheduled_tasks` 表(双 FK 级联)+ `ScheduledTaskFired` 审计;6 档 preset + `max_runs`/`ends_at` 结束条件;**零新增依赖**(用既有 tokio timer + sqlx) |
| C3+ LLM 摘要式压缩(08-18)| `app/src-tauri/src/agent/context.rs::compact_messages` + `messages` 表 metadata kind | `context_window * 0.85` 触发 → LLM 9 段模板结构化摘要(`task/progress/facts/decisions/open/files/next`)+ `prior-summary` 增量合并 + 保留区存活(`clamp(15k, 10%窗, 25k)`)+ `cutoff_seq` 水位精确折叠;连续 3 次 LLM 摘要失败熔断回退 C3 机械丢组(兜底);`messages.metadata.kind = "compaction_summary"` 元数据;**零新增依赖** |
| B11 远程遥控通道(08-11~13)| `crates/everlasting-remote/`(独立二进制)+ `crates/everlasting-remote-protocol/` + `app/src-tauri/src/daemon/tunnel/{client,config,dispatcher,manager,node_id,sse_bridge}.rs` + `app/src/components/{settings/RemoteTab,PairingView,NodeListView,RemoteTab}.vue` + `app/src/transport/{auth,http}.ts` | axum 0.7 + `tower-http::ServeDir` + `tokio-tungstenite` 0.24 WSS + `sqlx` + `dashmap` + `subtle`(constant-time compare);**Cargo workspace 翻转**(08-11,根 `Cargo.toml` members 3 个,default-members 只含 remote 两 crate,`Cargo.lock` / `target` 在根);**新引入依赖** `everlasting-remote-protocol` 内部 crate + `dashmap` + `subtle`,其余借已有 |

**说明**:
- `image`、`libheif-rs`、`nucleo`、`ignore` 都是轻量、跨平台、纯 Rust 实现(除了 `libheif-rs` 需要系统 libheif;`notify` 已移除,见上)
- **`serde_yml` 已废弃(2026-06-16 发现)**:`serde_yml` + 前代 `serde_yaml` 均在 crates.io 标 "Deprecated"(`0.0.13` 仅 compat shim)。B3 `/command` 的 frontmatter(`name` / `description` / `argument-hint` 单行标量)+ L3d subagent frontmatter(`SubagentDef.tools` 数组 + `arguments` 等,2026-06-25/26 落地)都用**手写 parser**(`app/src-tauri/src/resource_loader.rs::parse_frontmatter`,~60 行,split `---` + `key:value` + 数组 `- item` 解析),零依赖。未来 Skill / Memory / Role frontmatter 字段再复杂化(嵌套 map / 多行字符串)时再上 maintained fork(候选 `serde_yaml_neo`)——§5 共享 loader 契约仍成立(parser 隔在 `parse_frontmatter` 函数后,替换局部)。
- 前端不引入 UI 框架(Element Plus / Vuetify 太重),自己攒 + 用 reka-ui / shadcn-vue primitives
- **ECharts 替代 recharts 的理由**:recharts 纯 React,跨框架方案 ECharts + vue-echarts 更成熟,中文文档全

---

## 2. 决策:rig-core 弃用(2026-06-09),改自研 Provider trait

**历史背景**:原计划步骤 3b-2 切到 rig-core 0.38.1,作为 LLM 抽象层(2026-06-04 决策,见 [IMPLEMENTATION §4 决策日志 2026-06-04 段](./IMPLEMENTATION/decisions-2026-06.md))。理由:
- 20+ provider 支持,后期切 OpenAI / 本地模型无痛
- 自带 `Agent<M>` 抽象,省掉"消息 → tool call → 循环"样板
- 自带 `MessageStore` trait,接 SQLite 顺

**弃用原因**(2026-06-09 决策,见 [IMPLEMENTATION §4 决策日志 2026-06-09 段](./IMPLEMENTATION/decisions-2026-06.md)):
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

> 完整加载机制、token 预算、四层 Memory 边界见 [memory spec](../.trellis/spec/backend/memory.md) 和 [BACKLOG.md §2 Agent Skill 系统](./BACKLOG.md#2-agent-skill-系统)。

**实现影响**:
- 一个 `MarkdownResource` 通用数据结构
- 一个 `ResourceLoader` 通用加载器
- 不同的"加载策略"在调用层实现(自动 / 显式)
- 三类资源复用同一套解析代码,新增类型零成本
