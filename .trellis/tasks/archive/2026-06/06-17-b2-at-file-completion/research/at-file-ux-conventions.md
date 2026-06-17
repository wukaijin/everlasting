# Research: At-Mention File Completion UX Conventions

- **Query**: 主流 AI 编码工具的 @-mention 文件补全 UX 约定与选中后注入格式 — 为 Everlasting B2 功能对齐 Claude Code 提供决策依据
- **Scope**: 外部调研（web docs + GitHub 源 + Wayback Machine 归档 + Claude Code changelog）
- **Date**: 2026-06-17
- **对比对象**: Claude Code、Cursor、GitHub Copilot Chat、Continue、Aider、Cline（原 Claude Dev）

> 注：本次调研使用 curl 直接抓取官方文档/源码 + web.archive.org CDX API。Exa MCP 在本 agent 不可用；Google/Bing/DuckDuckGo 全部上 captcha 拦了 curl，故未走搜索引擎，而是直接命中各工具的一手文档与 changelog。

---

## TL;DR 核心结论（针对本项目对齐 Claude Code）

**Claude Code 的 @ 行为（要复制的那一套）：**

1. **触发条件**：`@` 必须在 **输入起始位置（at start）** —— Claude Code interactive-mode 文档明确把 `@` 列为「at start」快捷键，和 `/`（command）与 `!`（shell mode）同列。换言之 **行首/输入框开头触发**，而不是任意位置触发（PRD 的「行首 `@`」假设是对的，不是任意位置 `@`）。
   - 来源：`docs.claude.com/en/docs/claude-code/interactive-mode` —— "Quick commands: `/` at start = Command or skill · `!` at start = Shell mode · `@` at start = File path mention, **Trigger file path autocomplete**"
2. **选中后注入到 LLM 的内容**：**直接展开为文件内容**注入到 context，而不是仅注入路径让 LLM 自己 read。
   - "Updated @-mention file truncation from 100 lines to **2000 lines**"（CC changelog 1.0.53, 2025-07-18）—— 如果只是路径文本，根本不会存在「行数截断」这件事。
   - "@-mention files to **directly add them to context**"（0.2.80, 2025-04-21）
   - "Use @ to reference files ... Claude reads the file before responding."（best-practices 文档）
3. **大文件降级策略**：超过 50K 字符的文件**不直接注入内容**，而是保存到磁盘并把"文件路径 + preview"作为 context 项（CC changelog 措辞）。
4. **截断上限**：2000 行（截至 1.0.53；早期是 100 行）。超 2000 行的文件**被截断**注入（行数截断 = 截断后注入，不是 path-only）。
5. **路径基准**：相对或绝对路径都支持（common-workflows 文档：`@src/utils/auth.js` 与绝对路径都可）。
6. **目录 @**：支持 `@src/components` —— "Directory references show **file listings, not contents**"（即 @ 目录注入的是文件列表，@ 文件注入的是文件内容）。
7. **行号 `@path:10-20`**：CC **不支持**行号区间语法。锚点语法存在但是 Markdown heading fragment：`@README.md#installation`（changelog v1.x 修了 "anchor fragments e.g. @README.md#installation" 的解析 bug）。本项目若要加 `:line-range`，属于**自创语法**，无 CC 先例。
8. **路径含空格**：CC 1.x 起支持（2025-08-18 changelog "Support files with spaces in path"）—— 实现上意味着选中后注入的 `@path` 需要某种 quoting/边界处理。
9. **匹配算法**：fuzzy + filename matching（changelog 0.2.80 "Improved performance for filename auto-complete"；2025-06-24 "Improved file path autocomplete with **filename matching**"；2025-07 "pre-warming the index on startup + session-based caching + background refresh"）。
10. **gitignore / 隐藏文件**：早期版本被 gitignore 静默排除引发 bug（2026-01-18 修），后续加入了隐藏文件（changelog "Added hidden files to file search and @-mention suggestions"）。
11. **键盘交互**：Slash command 和 @-mention 的 suggestion list 在 fullscreen 模式下支持鼠标 hover/click（2026-Q1 changelog）—— 即默认是 ↑↓ 键盘 + Enter 选中。
12. **多文件选择**：单消息内可 `@file1.js and @file2.js`（common-workflows 文档明确支持，每个 @ 独立展开为各自内容）。

---

## Findings

### Files / Sources Found

| Source | URL | Key takeaway |
|---|---|---|
| Claude Code interactive-mode | https://docs.claude.com/en/docs/claude-code/interactive-mode | `@` = **at start** trigger, "File path mention, Trigger file path autocomplete" |
| Claude Code common-workflows | https://docs.claude.com/en/docs/claude-code/common-workflows | `@src/utils/auth.js` 注入**完整文件内容**；`@src/components` 注入目录 listing；路径相对或绝对；多文件支持 |
| Claude Code best-practices | https://docs.claude.com/en/docs/claude-code/best-practices | "Use @ to reference files ... Claude reads the file before responding." |
| Claude Code changelog | https://docs.claude.com/en/docs/claude-code/changelog | 截断 100→2000 lines；>50K chars 降级为 path+preview；filename matching + pre-warm cache；支持路径含空格；支持 markdown anchor `@file#heading` |
| Cursor @-Files (archived) | https://web.archive.org/web/20241217200556id_/https://docs.cursor.com/context/@-symbols/@-files | `@Files` 触发文件搜索，显示**路径 preview** 用于消歧；长文件分块 + rerank |
| Cursor @-Code | https://web.archive.org/web/20240614000644id_/https://docs.cursor.com/context/@-symbols/@-code | `@Code` 引用具体代码段（symbol/function），显示 code preview |
| Cursor @-Folders | https://web.archive.org/web/20240613234059id_/https://docs.cursor.com/context/@-symbols/@-folders | `@Folders` 仅 Chat 支持，引入整个目录为 context |
| Cursor @-Docs | https://web.archive.org/web/20240613224807id_/https://docs.cursor.com/context/@-symbols/@-docs | `@Docs > Add new doc` 抓取第三方文档，索引后作为 context |
| Cursor @-Codebase | https://web.archive.org/web/20240614002116id_/https://docs.cursor.com/context/@-symbols/@-codebase | 4 步 RAG：Gathering → Reranking → Reasoning → Generating |
| Cursor @-Git | https://web.archive.org/web/20240613235441id_/https://docs.cursor.com/context/@-symbols/@-git | `@Git` 加 commit/diff/PR 为 context（仅 Chat） |
| Cursor docs/rules (现行) | https://cursor.com/docs/rules | "When @-mentioned in chat"、"Manual — only via @-mention"、`@filename.ts` 在 rule 文件里做引用 |
| Copilot Chat (VS Code) | https://code.visualstudio.com/docs/copilot/copilot-chat | 用 **`#` 而非 `@`** 做 file mention：`#file / #codebase / #terminalSelection / #fetch`；`@` 是 chat participant（`@workspace`） |
| Continue custom-providers | https://raw.githubusercontent.com/continuedev/continue/main/docs/customize/deep-dives/custom-providers.mdx | `@` 是 **ContextProvider 抽象** 的入口：`@File / @Code / @Git Diff / @Current File / @Terminal / @Open / @Clipboard / @Tree / @Problems / @Debugger / @Repository Map / @OS / @HTTP / MCP`；每种都注入对应内容 |
| Continue chat quick-start | https://raw.githubusercontent.com/continuedev/continue/main/docs/ide-extensions/chat/quick-start.mdx | "Type `@` to include specific context"；`@Files` `@Terminal` |
| Aider commands | https://aider.chat/docs/usage/commands.html | Aider **不用 `@`**——而是 `/add <path>` 显式加入 chat（让 aider 可编辑）；`/read-only <path>` 加只读引用；其他文件靠 **repo-map 自动注入** |
| Aider repo-map blog | https://aider.chat/2023/10/22/repomap.html | tree-sitter AST 提取 symbol 签名 + 图排序算法（PageRank 变种）按重要性排序、按 token budget 截断；**自动注入**每次请求 |
| Cline working-with-files | https://raw.githubusercontent.com/cline/cline/main/docs/core-workflows/working-with-files.mdx | `@/path/to/file`（**前导 `/`**）= 完整文件内容；`@/path/to/folder/`（**结尾 `/`**）= 目录结构 + 所有文件内容；多根 workspace 用 `@workspace-name:/path` |

---

### Pattern 1 — 触发条件（trigger conditions）

| Tool | 触发字符 | 位置约束 | 是否支持目录逐级筛选 |
|---|---|---|---|
| **Claude Code** | `@` | **必须 at start**（行首/输入框开头，与 `/` `!` 同列）| 是，`@src/components` 注入目录 listing |
| **Cursor** | `@` | **任意位置**触发下拉（@Files / @Code / @Folders / @Docs / @Git / @Web / @Codebase 等多类型选择）| 是，`@Folders`（仅 Chat）|
| **GitHub Copilot Chat** | `#`（**不是 `@`**）| 任意位置（type `#` in chat input）；`@` 留给 chat participant `@workspace` | 是，`#file` `#codebase` |
| **Continue** | `@` | 任意位置；触发 **ContextProvider 下拉**（每个 provider 一个类型）| 是，`@File` 子菜单可继续选文件 |
| **Aider** | 无 `@`；用 `/add` `/read-only` `/drop` 命令 | `/` at start（命令模式）| 无下拉；命令式 |
| **Cline** | `@` | 任意位置（"Type `@` in the chat input and select a file or folder"）| 是，文件夹 `@/folder/` |

**Everlasting 对齐 Claude Code 的取值**：触发条件 = `@` 在 **行首/输入框开头**（和 B3 的 `/` 同列）。这与 PRD 的现有假设一致，是干净的设计选择，不与现有 B3 `/command` 冲突。

> 注：cursor.com/docs/rules 页面的措辞 "When @-mentioned in chat (e.g., ...)" 暗示 Cursor 早期也是行首触发，后来放宽到任意位置。本项目若取行首触发，符合 Claude Code 当前实测行为。

---

### Pattern 2 — 补全面板交互（panel UX）

| Tool | 模糊匹配 | 排序 | 目录层级展开 | 键盘导航 | 结果上限 |
|---|---|---|---|---|---|
| **Claude Code** | filename matching + fuzzy（2025-06-24 changelog）| 最近修改 + 文件名匹配（"pre-warming index on startup + session-based caching + background refresh"）| 是（@dir 显示子目录文件）| ↑↓ + Enter；fullscreen 模式额外支持鼠标 hover/click | 早期有 bug "directories with more than 100 entries"；实际无硬上限，靠 fuzzy 过滤 |
| **Cursor** | fuzzy（输入 `@` 后继续输入即触发文件搜索）| 路径 preview 消歧（同名文件不同目录）| `@Folders` 整个目录 | 上下键 + Enter | 未文档化 |
| **Copilot Chat** | fuzzy（`#file:` 后弹 workspace 文件列表）| open/recent 文件优先 + workspace 文件 | 是 | Tab/Enter | 未文档化 |
| **Continue** | fuzzy | 取决于 provider；`@File` 默认按 workspace 文件 | `@File` 子菜单选文件 | ↑↓ + Tab/Enter | 无文档化上限 |
| **Aider** | n/a | n/a | n/a | n/a（命令式）| n/a |
| **Cline** | 模糊（未明示算法）| 同 workspace 文件 | 是（`@folder/`）| ↑↓ + Enter | 未文档化 |

**通用约定**：↑↓ 选中、Enter 确认、Esc 关闭。Claude Code 的 fuzzy 实现走自研 + 自有 file index cache，性能优化方向是 **pre-warm + session cache + background refresh** —— 这正是 PRD 提到的"mtime fence 缓存"思路。

---

### Pattern 3 — 选中后注入到输入框的格式

| Tool | 注入到输入框的可见形态 | 路径形式 |
|---|---|---|
| **Claude Code** | `@src/utils/auth.js`（结构化 token，用户可继续看到 `@path`）| 相对或绝对路径 |
| **Cursor** | `@filename.ts`（结构化 token，下方带 chip/pill 显示文件名 + 路径 preview）| 相对路径 |
| **Copilot Chat** | `#file:src/index.ts`（结构化 token，显示为 chip）| 相对路径 |
| **Continue** | `@File: src/index.ts`（结构化 chip）| 相对路径 |
| **Aider** | n/a（用 `/add src/index.ts` 命令；显式加到 in-chat file 列表，前端不可编辑）| 任意路径 |
| **Cline** | `@/src/index.ts`（**前导 `/`** —— 显式区分绝对 vs 相对，多根 workspace 用 `@ws-name:/path`）| 绝对路径风格（以 workspace root 为基准）|

**关键差异**：Cline 用 `@/...`（前导 `/`）来无歧义表达"workspace 根开始的绝对路径"；Claude Code 用纯 `@path`（相对路径，无前导 `/`）。**本项目若严格对齐 Claude Code，应取 `@<相对路径>`，不加前导 `/`。**

**多文件选择**：所有工具都支持在单条消息内多次 `@`。Claude Code 文档原文："You can reference multiple files in a single message (for example, `@file1.js and @file2.js`)"。

**行号 `@path:10-20`**：**所有调研的工具都不支持**这种行区间语法。Claude Code 唯一相关的 anchor 语法是 markdown heading：`@README.md#installation`（changelog v1.x bug fix）。**这是空地，本项目要加等于自创语法**。

---

### Pattern 4 — LLM 实际收到什么（**两种哲学的分歧**）

这是 PRD Open Question 2 的核心。**两种哲学**：

#### 哲学 A：注入时**展开为文件内容**（路径在用户侧是「快捷方式」）

LLM 不需要自己 read，文件内容直接出现在 context 里。

- **Claude Code**：A 派 —— "Updated @-mention file truncation from 100 lines to 2000 lines"（行数截断证明是注入内容），"@-mention files to directly add them to context"。大文件（>50K chars）降级为"file path + preview"。
- **Cursor**：A 派 —— Chat 长文件自动 chunk + rerank；Cmd K 用 `auto / full file / outline / chunks` 四种 reading strategy。
- **Continue**：A 派 —— 每个 ContextProvider 都把自己抓的内容塞进 context（`@File` 注入文件全文，`@Code` 注入 symbol 全文，`@Git Diff` 注入 diff，`@Repository Map` 注入 tree-sitter 签名）。
- **Cline**：A 派 —— "Cline sees the complete file content, including imports, related functions, and surrounding context"（明确说**完整内容**，不是路径）。

#### 哲学 B：只注入**路径文本/索引**，LLM 自己用 read tool 按需读

- **Aider**：B 派的极端形态 —— **不用 `@`**，用 `/add` 显式管理"in-chat files"列表（这些文件全文发送给 LLM，可编辑）；同时 **repo-map 自动注入**所有文件的 symbol 签名（按重要性 + token budget 截断的 tree-sitter 索引），让 LLM 在不读全文的情况下也能看到全仓结构、再主动 `/add` 需要的文件。

> 严格说 Aider 既不属于纯 A 也不属于纯 B：它有两个层次——（1）`/add` 过的文件 = 全文注入（A）；（2）未 `/add` 的文件 = repo-map 索引注入（B 的索引变种）。

**项目决策依据**：**Claude Code 是哲学 A（注入内容）**。PRD 的"Open Question 2"答案是 —— **选中后注入展开为文件内容，不是路径提示**。但为了控制 token：
1. 文件 ≥ 某阈值（CC 用 50K chars / 2000 lines）→ 降级为"路径 + 文件头 preview"。
2. 文件 < 阈值 → 全文注入到 user message（CC 是把 @ 文件读出来塞进 context，前端仍展示 `@path` token）。

> **重要副作用**：如果走哲学 A，则前端只需注入 `@path` 结构化 token，**真正的文件读取 + 注入 context 这件事必须在后端 agent loop 完成**（前端不知道 token 预算、不该读文件内容）。Claude Code 的实现路径：用户输入 `@src/foo.rs` → 提交时后端解析 user message 中的 `@path` token → 后端读文件 → 把内容替换/拼接进 user message 的 context blocks → 再发给 LLM。**这和 Everlasting 现有 memory/指令文件系统的 `build_instructions_blocks()` 注入 cache_control 的模式同构**，可以参考。

---

### Pattern 5 — 相对路径 vs 绝对路径基准

| Tool | 默认基准 |
|---|---|
| **Claude Code** | 相对（`@src/utils/auth.js`），文档明示"File paths can be relative or absolute" |
| **Cursor** | workspace root 为基准的相对路径 |
| **Copilot Chat** | workspace root 为基准 |
| **Continue** | workspace root（`@File` 引用 workspace 内文件）|
| **Aider** | 命令参数路径相对当前目录 |
| **Cline** | workspace root；多根 workspace 用 `@ws-name:/path` 前缀消歧 |

**Everlasting 项目对齐**：基准 = `currentCwd`（chat store 的 project.path，PRD 已指出），相对路径注入。无多根 workspace 需求，不必学 Cline 的 `@ws-name:/` 前缀。

---

### Related Specs

- `.trellis/spec/frontend/popover-pattern.md` — B3 沉淀的 popover/External Trigger Element pattern，B2 复用 `<TriggerMenu>` 骨架
- `.trellis/tasks/archive/2026-06/06-16-b3-command-palette/prd.md` — B3（command palette）作为 B2 的兄弟实现参考
- `docs/TECH.md` §输入层 — `nucleo`（fzf Rust 端口）模糊匹配候选 + CodeMirror 6 候选评估
- `.trellis/spec/backend/llm-contract.md` — LLM 请求契约（@-mention 注入文件内容需要参照此处 user message 构造规则）

---

## 针对本项目（Everlasting B2）的硬约束清单

1. **触发**：`@` 在 **行首/输入框开头**（与 Claude Code "at start" 一致；与 B3 `/` 同列，复用 `<TriggerMenu>` 的 trigger-char 扩展位）。
2. **选中注入（用户可见形态）**：`@<相对路径>` 结构化 token（不加前导 `/`，对齐 Claude Code；不像 Cline 那样用 `@/`）。
3. **LLM 收到的内容（后端行为）**：**展开为文件内容**注入 user message（哲学 A），不是路径提示。需要在 agent loop / commands 层加路径解析 + 文件读取 + 注入逻辑。
4. **截断策略**：建议对齐 Claude Code 的两段式：
   - 文件 < 阈值（建议先用 2000 行 / 50K chars 之一）：全文注入。
   - 文件 ≥ 阈值：降级为"路径 + 文件头 N 行 preview"，让 LLM 自行决定是否 read 完整。
5. **行号 `@path:10-20`**：**先不做**（无 CC 先例，自创语法）。如要做，借鉴的应是 markdown anchor `@file#heading` 形态而不是行号。
6. **多文件**：支持，单消息内多次 `@`，每个独立展开。
7. **路径基准**：`currentCwd`（project.path）；相对路径为主，绝对路径兼容。
8. **模糊匹配**：nucleo（TECH.md 已候选）—— Claude Code 自研 + cache，性能手段是 pre-warm + session cache + background refresh。本项目可先 nucleo + mtime fence（PRD 已规划），不必一开始就上 background refresh。
9. **键盘**：↑↓ + Enter + Esc；Enter 不能与发送冲突（B3 已有 IME-safe keydown 处理模式）。
10. **目录 @ 支持**（可选 v2）：`@src/components/` 注入目录 listing（不展开递归文件内容），与 Claude Code 行为一致。

---

## Caveats / Not Found

- **Cursor 新版（cursor.com/docs，2026 起）的 @ 行为**：站点全 BAILOUT_TO_CLIENT_SIDE_RENDERING，curl 拿不到 RSC 内容；只能依赖 **旧 docs.cursor.com 的 Wayback 归档**（截至 2025-07 前）。新版是否调整了触发位置（行首 vs 任意）、是否引入了新的 @ 类型（除 Files/Code/Folders/Docs/Git/Web/Codebase 外），**本次调研未能验证**。
- **GitHub Copilot Chat 的 `#file` 注入语义**（path vs content）：MS Learn 文档措辞模糊（"explicitly reference files"），未明示是路径文本还是全文注入。从体验和"manage context for AI"的措辞推断是注入内容，但**无一手明文**。GitHub Copilot Chat 是闭源，无法看源码。
- **Continue 的 `@File` 行号区间**：custom-providers.mdx 文档未提行号语法；推断不支持 `@File:line-range`。
- **Aider 的 `/add` 是否展开 vs repo-map 注入策略细节**：repo-map blog 给了算法（tree-sitter + 图排序 + token budget 截断），但 `/add` 注入的精确格式（system prompt 还是 user message？是否带 fence？）需要看源码 `aider/coders/base_coder.py`，本次未深入。
- **Claude Code 的 `@` 是否真的强制 at start**：interactive-mode 文档措辞 "at start"，但 changelog 里有 "type `@` to add context" 的泛化措辞。社区反馈（未一手验证）说新版 CC 也允许任意位置触发 `@`。**建议实现时按 at start 起步**（更简单、与 B3 一致），后续视用户反馈放开到任意位置。
- **Wayback Machine 对 `docs.cursor.com/context/@-symbols/@-files` 的 2024 早期快照**：多个 snapshot 是压缩损坏的二进制乱码，仅有 2024-08 之后的快照可读。

