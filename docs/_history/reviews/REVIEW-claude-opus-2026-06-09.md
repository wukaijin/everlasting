# Everlasting 代码审计与重构计划

> **审计人**: Claude (Opus 4.8, Trellis session) & carlos
> **日期**: 2026-06-09
> **范围**: 全仓库源代码结构、文档体系、技术路线
> **基线**: commit `54246dd` (main, clean)

---

## 目录

1. [大文件审计与重构计划](#1-大文件审计与重构计划)
2. [Markdown 文档审阅](#2-markdown-文档审阅)
3. [技术路线决策记录](#3-技术路线决策记录)
4. [项目结构全景图](#4-项目结构全景图)

---

## 1. 大文件审计与重构计划

### 1.1 概览

**总代码量**: Rust 16,502 行 / 前端 10,909 行 = **27,411 行**

超过 800 行的非 markdown 文件共 **6 个**：

| 文件 | 行数 | 优先级 | 结论 |
|------|------|--------|------|
| `src-tauri/src/lib.rs` | 3195 | 🔴 高 | **必须拆分** — 全部 Tauri 命令 + Agent Loop 挤在单文件 |
| `src-tauri/src/db.rs` | 2862 | 🔴 高 | **必须拆分** — 类型 + 迁移 + 6 域 CRUD + 925 行测试 |
| `src-tauri/src/llm/provider/openai.rs` | 1150 | 🟡 中 | **建议拆分** — send() 284 行过长，但整体尚可 |
| `src-tauri/src/llm/provider/wire.rs` | 1109 | 🟢 低 | **暂不动** — 高内聚的协议转换层，测试占 47% |
| `app/src/components/chat/ChatPanel.vue` | 957 | 🟡 中 | **建议拆分** — worktree chip + diff modal 可提取 |
| `app/src/components/settings/ModelsTab.vue` | 954 | 🟡 中 | **建议拆分** — 典型 CRUD 巨组件 |

### 1.2 lib.rs (3195 行) — 重构方案

**现状**: 单文件包含全部 26 个 Tauri 命令、AppState、Agent Loop (chat 函数 680 行)、事件类型、测试。

**目标结构**:
```
src-tauri/src/
├── lib.rs              # ~120 行: mod 声明 + run() 入口 + init_tracing()
├── state.rs            # ~160 行: AppState, CancellationGuard, 事件 Payload
├── commands/
│   ├── mod.rs          # ~30 行: pub re-exports
│   ├── config.rs       # ~100 行: get_llm_config, get_home_dir
│   ├── providers.rs    # ~260 行: Provider/Model CRUD + test_model
│   ├── sessions.rs     # ~200 行: Session CRUD + diff_worktree
│   ├── worktree.rs     # ~360 行: attach/detach/delete + cancel_inflight
│   ├── projects.rs     # ~120 行: Project CRUD + pick_project_dir
│   └── cancel.rs       # ~30 行: cancel_chat
└── agent/
    ├── mod.rs          # ~20 行: pub re-exports
    ├── chat.rs         # ~700 行: chat 命令 (Agent Loop 主函数)
    ├── provider.rs     # ~130 行: resolve_chat_provider, ResolvedChatProvider, PreFlightError
    ├── system_prompt.rs # ~100 行: build_system_prompt, lookup_head_sha
    ├── thinking.rs     # ~30 行: PendingThinking, flush_pending_thinking
    ├── helpers.rs      # ~80 行: tool_result_envelope, build_synthetic_tool_result_message, persist_turn_cwd, emit_chat_event
    └── tests.rs        # ~600 行: 全部 inline 测试迁移至此
```

**拆分步骤**（建议按顺序执行）：

| 步骤 | 内容 | 风险 | 预计耗时 |
|------|------|------|----------|
| L-1 | 提取 `state.rs`（AppState + CancellationGuard + 事件 Payload） | 低 | 30min |
| L-2 | 提取 `agent/` 模块（chat + helpers + provider + system_prompt + thinking） | 中 | 1h |
| L-3 | 提取 `commands/` 模块（6 个命令文件） | 低 | 1h |
| L-4 | 迁移测试到 `agent/tests.rs` | 低 | 30min |
| L-5 | 清理 `lib.rs` 为纯入口 | 低 | 15min |

**关键原则**:
- 每步后 `cargo check && cargo test --lib` 确认编译通过
- `commands/` 各文件只做 IPC 命令分发，不含业务逻辑
- `agent/` 是核心 Agent Loop，保持高内聚
- `state.rs` 是跨模块共享状态，避免循环依赖

### 1.3 db.rs (2862 行) — 重构方案

**现状**: 类型定义 (227 行) + 迁移 (406 行) + 6 域 CRUD + seed + 925 行测试。

**目标结构**:
```
src-tauri/src/db/
├── mod.rs          # ~30 行: pub re-exports + init_pool + run_migrations
├── types.rs        # ~230 行: 所有 row 类型、WorktreeState、ProviderProtocol
├── migrations.rs   # ~410 行: Schema 创建、ALTER TABLE 迁移、列探测
├── projects.rs     # ~290 行: Project CRUD
├── sessions.rs     # ~310 行: Session CRUD + worktree state + persist_turn + system events
├── providers.rs    # ~160 行: Provider CRUD
├── models.rs       # ~190 行: Model CRUD
├── config.rs       # ~200 行: app_config KV + seed
└── tests.rs        # ~930 行: 全部测试（或按域分 test 模块）
```

**拆分步骤**：

| 步骤 | 内容 | 风险 | 预计耗时 |
|------|------|------|----------|
| D-1 | `mkdir db/`，创建 `mod.rs` + `types.rs`，迁移类型定义 | 低 | 30min |
| D-2 | 提取 `migrations.rs`（init_pool + run_migrations + ALTER helpers） | 低 | 30min |
| D-3 | 按 CRUD 域提取 projects/sessions/providers/models/config | 低 | 1h |
| D-4 | 迁移测试 | 中 | 30min |
| D-5 | 删除旧 `db.rs`，确认 `mod db` 指向 `db/mod.rs` | 低 | 15min |

**关键原则**:
- `types.rs` 零 async 依赖，纯数据结构
- 每个域 CRUD 文件只依赖 `types.rs` 和 `sqlx::SqlitePool`
- 测试可按域分文件或集中管理

### 1.4 openai.rs (1150 行) — 轻量优化

**现状**: `send()` 方法 284 行，混合 HTTP 构建 + SSE 消费 + Delta 解析 + Tool Call 累积。

**建议**（非紧急，优先级低于 lib.rs/db.rs）：

| 步骤 | 内容 | 效果 |
|------|------|------|
| O-1 | 提取 `ToolCallBuf` + `build_tool_call_event()` 到 `tool_call_accumulator.rs` | 减少 ~50 行 |
| O-2 | 考虑将 SSE 消费循环提取为独立方法 `consume_stream()` | send() 降至 ~150 行 |

**不建议进一步拆分**: `send()` 内在需要协调 HTTP/SSE/ToolCall 三者，强行拆分会引入过多参数传递。

### 1.5 wire.rs (1109 行) — 保持不动

**理由**: 高内聚的双向协议转换层。470 行测试（43%）锁定 1:1 线格式契约。拆分反而增加同步验证的认知负担。

### 1.6 前端大文件

#### ChatPanel.vue (957 行)

| 提取目标 | 行数 | 收益 |
|----------|------|------|
| `WorktreeChip.vue` | ~350 行 (template + script + style) | 三态 chip + dropdown + clipboard 逻辑独立 |
| `DiffModal.vue` | ~100 行 | diff overlay 包装层独立 |
| 剩余 ChatPanel | ~500 行 | 纯消息列表 + 输入框集成 |

**优先级**: 中。Worktree 逻辑复杂度足以支撑独立组件。

#### ModelsTab.vue (954 行)

| 提取目标 | 行数 | 收益 |
|----------|------|------|
| `ModelRow.vue` | ~200 行 | 行显示 + 测试按钮 + 内联测试结果 |
| `ModelForm.vue` | ~350 行 | Add/Edit 表单（6 字段 + 验证） |
| `DeleteModelConfirm.vue` | ~40 行 | 删除确认 overlay |
| 剩余 ModelsTab | ~360 行 | 薄编排层 |

**优先级**: 中。典型 CRUD 巨组件拆分，收益明确。

### 1.7 重构总优先级

```
Phase 1 (立即):   lib.rs 拆分 → db.rs 拆分
Phase 2 (近期):   ChatPanel.vue 拆分 → ModelsTab.vue 拆分
Phase 3 (可选):   openai.rs send() 优化
不动的文件:       wire.rs, streamController.ts
```

---

## 2. Markdown 文档审阅

### 2.1 docs/ 文件逐项评审

| 文件 | 行数 | 状态 | 建议 |
|------|------|------|------|
| **CLAUDE.md** | — | ✅ 准确 | **保留** — 核心入口文档，维护良好 |
| **README.md** | — | ⚠️ 过时 | **更新** — "设计完备,实施前夜" 和步骤状态表需更新 |
| **ARCHITECTURE.md** | 694 | ⚠️ 小修 | **保留+修** — 第4行 "14 关卡"→"16 关卡" |
| **BACKLOG.md** | 701 | ⚠️ 冗长 | **压缩** — v3+ 内容移至附录，目标从 700→300 行 |
| **IMPLEMENTATION.md** | 347 | ✅ 准确 | **保留** — 维护最好的文档 |
| **HACKING-wsl.md** | 613 | ✅ 准确 | **保留** — "10 个坑"→实际 12 个，小修 |
| **HACKING-llm.md** | 402 | ✅ 准确 | **保留** — 考虑拆为 protocol + traps 两文件 |
| **HACKING-markdown.md** | — | ✅ 准确 | **保留** |
| **TECH.md** | — | ⚠️ 不准确 | **更新** — rig-core 标为已锁定但实际未迁移 |
| **DESIGN.md** | — | ⚠️ 小修 | **更新** — MVP checklist 中 edit_file/grep/glob 已实现需勾选 |
| **HANDOFF.md** | — | ⚠️ 过时 | **更新** — 缺少 06-08/06-09 多 provider PR 记录 |
| **docs/prompt.md** | — | 需确认 | 检查是否过时 |
| **docs/_archive/** | — | ✅ 归档 | **保留** — 历史决策记录 |
| **docs/_reviews/** | — | ✅ 只读 | **保留** — 外部评审快照 |
| **docs/spikes/001-004** | — | ✅ 验证 | **保留** — 技术决策验证记录 |
| **docs/spikes/2026-06-06-***| — | ⚠️ 已完成 | **归档** — bug/feature 已在后续 PR 中修复 |

### 2.2 .trellis/spec/ 逐项评审

| 文件 | 行数 | 状态 | 建议 |
|------|------|------|------|
| **llm-contract.md** | 3149 | ⚠️ 过大 | **拆分** → 5 个聚焦子文件（见下文） |
| **git-diff.md** | 321 | ✅ | 保留 |
| **database-guidelines.md** | 321 | ✅ | 保留 |
| **project-cwd-boundary.md** | 112 | ✅ | 保留 |
| **error-handling.md** | 86 | 🟡 部分 | 低优先级补全 |
| **backend/directory-structure.md** | 55 | ❌ 空 | **删除** |
| **backend/quality-guidelines.md** | 52 | ❌ 空 | **删除** |
| **backend/logging-guidelines.md** | 52 | ❌ 空 | **删除** |
| **popover-pattern.md** | 522 | ✅ | 保留 |
| **reka-ui-usage.md** | 460 | ✅ | 保留 |
| **cjk-fonts.md** | 259 | ✅ | 保留 |
| **state-management.md** | 249 | ✅ | 保留 |
| **design-tokens.md** | 228 | ✅ | 保留 |
| **frontend/component-guidelines.md** | 60 | ❌ 空 | **删除** |
| **frontend/directory-structure.md** | 55 | ❌ 空 | **删除** |
| **frontend/quality-guidelines.md** | 52 | ❌ 空 | **删除** |
| **frontend/hook-guidelines.md** | 52 | ❌ 空 | **删除** |
| **frontend/type-safety.md** | 52 | ❌ 空 | **删除** |
| **guides/cross-layer-thinking-guide.md** | 269 | ✅ | 保留 |
| **guides/code-reuse-thinking-guide.md** | 105 | ✅ | 保留 |

#### llm-contract.md 拆分方案

```
.trellis/spec/backend/
├── llm-contract.md          # ~400 行: 核心类型 + thinking 契约 + 反模式汇总
├── tool-contract.md         # ~300 行: 工具定义、ReadGuard、shell spill
├── worktree-contract.md     # ~400 行: attach/detach/delete + cancel + system prompt
├── multi-provider-contract.md # ~400 行: Provider trait + catalog + Anthropic/OpenAI 分发
├── test-model-contract.md   # ~100 行: test_model IPC
└── (其余文件不变)
```

### 2.3 需删除的空骨架文件（9 个）

```
.trellis/spec/backend/directory-structure.md
.trellis/spec/backend/quality-guidelines.md
.trellis/spec/backend/logging-guidelines.md
.trellis/spec/frontend/component-guidelines.md
.trellis/spec/frontend/directory-structure.md
.trellis/spec/frontend/quality-guidelines.md
.trellis/spec/frontend/hook-guidelines.md
.trellis/spec/frontend/type-safety.md
```

> 这些文件全是未填写的骨架文本，仅有标题和 "TODO" 占位符。删除后如需要可以从 git 历史恢复。同时需更新对应的 `index.md` 移除引用。

---

## 3. 技术路线决策记录

> 以下议题已于 2026-06-09 与 carlos 确认。

### 3.1 rig-core 迁移 — ✅ 决定放弃

**决策**: rig-core 迁移从路线图中移除，标记为「不采用」。

**原因**: 自研 provider trait + wire layer 已完整支持 Anthropic/OpenAI 双协议（含 thinking、tool call 累积、SSE 解析），rig-core 的增量价值不足以 justify 迁移成本。

**需更新文件**:
- `TECH.md`: rig-core 从「已锁定」改为「不采用」，记录决策原因
- `IMPLEMENTATION.md`: 步骤 3b-2 从「暂缓」改为「废弃」
- `CLAUDE.md`: 移除 rig-core 迁移相关描述

### 3.2 步骤路线图 — ✅ 新增 Step 8（代码重构），作为当前最高优先级

**决策**: 新增 Step 8「代码质量与文档清理」，在 lib.rs (3195L) 和 db.rs (2862L) 上堆新功能之前先完成重构。

**新路线图**:
```
已完成:  1 ✅  2 ✅  3a ✅  3b-1 ✅  4 ✅  6 ✅
当前:    → Step 8: 代码重构与文档清理
暂缓:    Step 5 (WSL 体验优化，降为可选)
远期:    Step 7 (daemon 化)
废弃:    3b-2 (rig-core 迁移)
```

**Step 8 子步骤**:
| 子步骤 | 内容 | 预计 |
|--------|------|------|
| 8-PR1 | lib.rs 拆分 → `state.rs` + `agent/` + `commands/` | 2-3h |
| 8-PR2 | db.rs 拆分 → `db/` 目录 | 1-2h |
| 8-PR3 | 前端组件拆分 (ChatPanel + ModelsTab) | 2h |
| 8-PR4 | 文档更新 (README/TECH/DESIGN/HANDOFF/BACKLOG) + 删除空骨架 | 2h |
| 8-PR5 | 创建 STRUCTURE.md 全景图 + 拆分 llm-contract.md | 1h |

**原则**: 每个 PR 都是纯重构/文档更新，不改变任何运行时行为。

### 3.3 BACKLOG 功能优先级 — ✅ MCP 保持 v2

| 功能 | 最终优先级 | 备注 |
|------|-----------|------|
| 多会话并发 | v1 | 保留 |
| 对话分支 | v2 | 保留 v2 |
| Memory/上下文压缩 | v1 | 保留 |
| MCP 协议支持 | v2 | carlos 确认不急，保持 v2 |
| 代码审查模式 | v2 | 保留 |
| 远期功能（v3+） | 归档 | 移至 BACKLOG 附录 |

---

## 4. 项目结构全景图

> 以下为计划在根目录创建的 `STRUCTURE.md` 内容，同时更新 CLAUDE.md 和 README.md 的引用。

```
everlasting/
├── CLAUDE.md                          # Claude Code 入口指南
├── README.md                          # 项目简介与状态
├── AGENTS.md                          # 多 agent 配置
├── STRUCTURE.md                       # ← 本文件（代码结构全景图）
│
├── app/                               # Tauri 2 应用
│   ├── src/                           # Vue 3 前端
│   │   ├── App.vue                    # 根组件
│   │   ├── main.ts                    # 入口
│   │   ├── style.css                  # 全局样式
│   │   ├── components/
│   │   │   ├── ChatWindow.vue         # (遗留入口，待清理)
│   │   │   ├── Icon.vue               # SVG 图标组件
│   │   │   ├── ProjectTabs.vue        # 项目 Tab 栏
│   │   │   ├── SessionList.vue        # 会话列表（侧边栏）
│   │   │   ├── chat/                  # 聊天区组件
│   │   │   │   ├── ChatPanel.vue      # 聊天主面板 (957L)
│   │   │   │   ├── ChatInput.vue      # IME 安全输入框
│   │   │   │   ├── MessageList.vue    # 消息列表
│   │   │   │   ├── MessageItem.vue    # 单条消息
│   │   │   │   ├── ToolCallCard.vue   # 工具调用卡片
│   │   │   │   ├── DiffView.vue       # Git diff 视图
│   │   │   │   ├── ModelSelect.vue    # 模型选择器
│   │   │   │   ├── ThinkingBlock.vue  # 扩展思考块
│   │   │   │   ├── EmptyProjectState.vue
│   │   │   │   └── DeleteWorktreeConfirm.vue
│   │   │   ├── layout/                # 布局组件
│   │   │   │   ├── AppShell.vue       # 三栏壳
│   │   │   │   ├── AppHeader.vue      # 顶栏
│   │   │   │   ├── AppLogo.vue        # Logo
│   │   │   │   ├── TitleBar.vue       # 自定义标题栏
│   │   │   │   └── Sidebar.vue        # 侧边栏
│   │   │   └── settings/              # 设置组件
│   │   │       ├── SettingsModal.vue   # 设置弹窗
│   │   │       ├── ProvidersTab.vue    # Provider 管理
│   │   │       ├── ModelsTab.vue       # Model 管理 (954L)
│   │   │       └── DefaultTab.vue      # 默认模型设置
│   │   ├── stores/                    # Pinia stores
│   │   │   ├── chat.ts                # 消息/会话/发送
│   │   │   ├── config.ts              # LLM 配置
│   │   │   ├── projects.ts            # 项目管理
│   │   │   ├── models.ts              # 模型目录
│   │   │   ├── providers.ts           # Provider 目录
│   │   │   ├── streamController.ts    # 流式事件路由 (796L)
│   │   │   └── streamController.test.ts
│   │   └── utils/
│   │       ├── lru.ts / lru.test.ts   # LRU 缓存
│   │       ├── markdown.ts / .test.ts # Markdown 渲染
│   │       ├── messageFormat.ts / .test.ts
│   │       └── path.ts / .test.ts
│   │
│   └── src-tauri/                     # Rust 后端
│       ├── main.rs                    # Windows 子系统入口
│       ├── lib.rs                     # Tauri 入口 + 全部命令 (3195L ⚠️)
│       ├── db.rs                      # SQLite 持久化 (2862L ⚠️)
│       ├── llm/                       # LLM 客户端模块
│       │   ├── mod.rs
│       │   ├── sse.rs                 # SSE 状态机解析器
│       │   ├── error.rs               # LlmError 5 类分类
│       │   ├── types.rs               # ContentBlock, ChatMessage, ToolDef, ChatEvent
│       │   └── provider/              # 多协议 Provider
│       │       ├── mod.rs             # Provider trait + build_provider 工厂
│       │       ├── anthropic.rs        # Anthropic 适配器 (772L)
│       │       ├── openai.rs           # OpenAI 适配器 (1150L)
│       │       └── wire.rs            # 协议无关线格式 (1109L)
│       ├── tools/                     # 工具定义与执行
│       │   ├── mod.rs                 # builtin_tools() + execute_tool 分发
│       │   ├── read_file.rs           # 读文件 (>50KB 截断)
│       │   ├── write_file.rs          # 写文件 (自动建目录)
│       │   ├── edit_file.rs           # 编辑文件 (644L)
│       │   ├── shell.rs               # Shell 命令 (5min 超时)
│       │   ├── grep.rs                # 内容搜索
│       │   ├── glob.rs                # 文件搜索
│       │   ├── list_dir.rs            # 目录列表
│       │   └── read_guard.rs          # 读取守卫 (沙箱校验)
│       ├── git/                       # Git 操作模块
│       │   ├── mod.rs
│       │   ├── diff.rs                # diff 统计 (git --numstat)
│       │   ├── worktree.rs            # worktree 创建/删除 (745L)
│       │   └── error.rs               # Git 错误类型
│       └── projects/                  # 项目管理模块
│           ├── mod.rs
│           ├── types.rs               # Project 类型
│           ├── detector.rs            # 自动检测 Git 项目
│           ├── store.rs               # 项目持久化
│           └── boundary.rs            # CWD 边界校验
│
├── docs/                              # 设计文档（全中文）
│   ├── ARCHITECTURE.md                # 系统架构、16 阶段生命周期
│   ├── DESIGN.md                      # 项目范围、约束
│   ├── TECH.md                        # 技术选型决策
│   ├── IMPLEMENTATION.md              # 7 步路线图、决策日志
│   ├── BACKLOG.md                     # 候选功能评估
│   ├── HANDOFF.md                     # Session 交接指南
│   ├── HACKING-wsl.md                 # WSL 环境踩坑记录
│   ├── HACKING-llm.md                 # LLM API 兼容层笔记
│   ├── HACKING-markdown.md            # Markdown 渲染笔记
│   ├── prompt.md                      # 系统 prompt 模板
│   ├── _archive/                      # 归档文档（一次性任务产物）
│   ├── _reviews/                      # 外部评审快照（只读）
│   └── spikes/                        # 技术验证记录
│
└── .trellis/                          # Trellis 任务管理
    ├── spec/                          # 编码规范与契约
    │   ├── backend/                   # 后端 spec
    │   ├── frontend/                  # 前端 spec
    │   └── guides/                    # 跨层思考指南
    ├── tasks/                         # 任务目录
    └── workspace/                     # 工作区
```

### 代码量统计

| 模块 | 文件数 | 代码行数 | 占比 |
|------|--------|----------|------|
| Rust 后端 (`src-tauri/src/`) | 25 | 16,502 | 60% |
| Vue 前端 (`src/`) | 33 | 10,909 | 40% |
| **合计** | **58** | **27,411** | 100% |

### Rust 后端各模块代码量

| 模块 | 行数 | 占 Rust 比 |
|------|------|-----------|
| `lib.rs` (入口 + 命令 + Agent Loop) | 3,195 | 19% |
| `db.rs` (SQLite) | 2,862 | 17% |
| `llm/provider/` (多协议) | 3,391 | 21% |
| `llm/` 其余 (types + sse + error) | 990 | 6% |
| `tools/` (工具集) | 2,729 | 17% |
| `git/` (Git 操作) | 1,458 | 9% |
| `projects/` (项目管理) | 1,022 | 6% |

---

## 附录：执行检查清单

### Phase 1: 代码重构（预计 4-5h）

- [ ] L-1: 提取 `state.rs`
- [ ] L-2: 提取 `agent/` 模块
- [ ] L-3: 提取 `commands/` 模块
- [ ] L-4: 迁移测试到 `agent/tests.rs`
- [ ] L-5: 清理 `lib.rs` 为纯入口
- [ ] D-1~D-5: `db.rs` → `db/` 目录拆分
- [ ] `cargo check && cargo test --lib` 全部通过

### Phase 2: 前端重构（预计 2-3h）

- [ ] 提取 `WorktreeChip.vue` from ChatPanel
- [ ] 提取 `DiffModal.vue` from ChatPanel
- [ ] 提取 `ModelRow.vue` from ModelsTab
- [ ] 提取 `ModelForm.vue` from ModelsTab
- [ ] `pnpm build` (vue-tsc + vite) 通过

### Phase 3: 文档更新（预计 2h）

- [ ] 更新 README.md 状态和路线图
- [ ] 修复 ARCHITECTURE.md "14"→"16"
- [ ] 压缩 BACKLOG.md (700→300 行)
- [ ] 更新 TECH.md rig-core 状态
- [ ] 更新 DESIGN.md MVP checklist
- [ ] 更新 HANDOFF.md 至 06-09
- [ ] 拆分 llm-contract.md → 5 子文件
- [ ] 删除 9 个空骨架 spec 文件
- [ ] 创建 STRUCTURE.md，更新 CLAUDE.md/README.md 引用

---

*审计完成于 2026-06-09，基于 main 分支 commit 54246dd。*
*审计工具: Claude Opus 4.8 (Trellis session)*
