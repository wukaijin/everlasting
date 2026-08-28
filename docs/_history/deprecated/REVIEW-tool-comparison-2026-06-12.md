# Tool 横向对比分析 — Everlasting vs 主流 AI Coding Agent

> **日期**: 2026-06-12
> **类型**: 竞品调研 / Tool 差距分析
> **对标产品**: Claude Code / Open Code / Codex CLI / Cursor Agent / Cline

---

## 1. 背景

Everlasting 当前有 7 个内置 tool：`read_file` / `write_file` / `edit_file` / `shell` / `grep` / `glob` / `list_dir`。本文档对比主流 AI coding agent 的 tool 设计，识别差距并给出优先级建议。

## 2. 总览对比

| Tool | Everlasting | Claude Code | Open Code | Codex CLI | Cursor | Cline |
|------|:-----------:|:-----------:|:---------:|:---------:|:------:|:-----:|
| read_file | ✅ | ✅ | ✅ | ❌ (shell) | ✅ | ✅ |
| write_file | ✅ | ✅ | ✅ | ❌ (patch) | ❌ | ✅ |
| edit_file | ✅ | ✅ | ✅ | ❌ (patch) | ✅ | ✅ |
| apply_patch | ❌ | ❌ | ✅ | ✅ | ❌ | ✅(新版) |
| shell/bash | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| grep | ✅ | ✅ | ✅ | ❌ (shell) | ✅ | ✅ |
| glob | ✅ | ✅ | ✅ | ❌ (shell) | ❌ | ❌ |
| list_dir | ✅ | ❌ | ✅ | ❌ (shell) | ✅ | ✅ |
| web_fetch | ❌ | ✅ | ✅ | ❌ | ❌ | ✅ |
| web_search | ❌ | ✅ | ✅ | ❌ | ✅ | ❌ |
| notebook_edit | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |
| agent/task | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| ask_user | ❌ | ✅ | ✅ | ❌ | ❌ | ✅ |
| todo 管理 | ❌ | ✅ | ✅ | ❌ | ❌ | ❌ |
| LSP | ❌ | ✅ | ✅(实验) | ❌ | ❌ | ❌ |
| delete_file | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| skill 加载 | ❌ | ✅ | ✅ | ✅ | ❌ | ❌ |

## 3. 各家 Tool 详细参数

### 3.1 Claude Code（25+ 工具）

核心高频 7 工具：

| Tool | 关键参数 |
|------|---------|
| **Read** | `file_path`(必填) + `offset`(起始行) + `limit`(行数,默认 2000) |
| **Edit** | `file_path` + `old_string` + `new_string` + `replace_all`(可选) |
| **Write** | `file_path` + `content` |
| **Bash** | `command` + `timeout`(默认 120s,最大 600s) + `description` + `run_in_background` |
| **Glob** | `pattern` + `path`(可选) |
| **Grep** | `pattern` + `path` + `glob` + `output_mode`(files_with_matches/content/count) + `-A`/`-B`/`-C` + `-i` + `head_limit` + `multiline` + `type`(语言过滤) |
| **Agent/Task** | `prompt` + `subagent_type` + `description` + `model`(可选) |

扩展工具：NotebookEdit / WebFetch / WebSearch / BashOutput / KillShell / AskUserQuestion / Skill / EnterPlanMode / ExitPlanMode / Monitor / LSP / EnterWorktree/ExitWorktree / CronCreate/CronDelete/CronList / PowerShell / TaskCreate/TaskUpdate/TaskList

关键设计模式：
- **必须先 Read 才能 Edit/Write** — 防止基于过时内容编辑
- **Edit 是精确字符串替换** — 不做正则或模糊匹配
- **Read 有 offset/limit 分页** — 大文件分块读
- **系统优先用专用工具而非 Bash** — Read > cat, Grep > grep, Glob > find

### 3.2 Open Code / anomalyco/opencode（15 工具，TypeScript）

| Tool | 关键参数 |
|------|---------|
| **read** | `filePath` + `offset`(1-indexed) + `limit`(默认 2000) |
| **write** | `filePath` + `content`（必须先 read） |
| **edit** | `filePath` + `oldString` + `newString` + `replaceAll`（内置模糊匹配策略） |
| **apply_patch** | `patchText`（批量多文件编辑，GPT 系模型专用） |
| **bash** | `command` + `description` + `timeout`(默认 120s) + `workdir` |
| **glob** | `pattern` + `path` |
| **grep** | `pattern` + `path` + `include` |
| **webfetch** | `url` + `format`(text/markdown/html) + `timeout` |
| **websearch** | `query` + `numResults`(默认 8) + `type`(auto/fast/deep) |
| **lsp** | `operation`(9 种: goToDefinition/findReferences/hover 等) + `filePath` + `line` + `character` |
| **task** | `prompt` + `description` + `subagent_type` + `background`(实验) |
| **question** | `questions` 数组（向用户提问） |
| **todowrite** | `todos` 数组 |
| **skill** | `name` |
| **plan_exit** | 无参数（退出计划模式） |

### 3.3 Codex CLI / openai/codex（2 工具，极简）

| Tool | 关键参数 |
|------|---------|
| **shell** | `cmd: string[]`（命令数组，在沙箱中执行） |
| **apply_patch** | `patch`（结构化补丁: Add File / Update File / Delete File / Move） |

设计哲学：极简 tool 集，文件操作全部通过 shell 命令。编辑走 diff/patch 而非 string replacement。沙箱化执行（Docker / sandbox-exec）。

### 3.4 Cursor Agent Mode（12 工具）

| Tool | 关键参数 |
|------|---------|
| **codebase_search** | `query` + `target_directories`（语义搜索,embedding + re-ranker） |
| **read_file** | `target_file` + `start_line_one_indexed` + `end_line_one_indexed_inclusive`(每次最多 250 行) |
| **edit_file** | `target_file` + `instructions` + `code_edit`（语义 diff,`// ... existing code ...` 标记） |
| **run_terminal_cmd** | `command` + `is_background` + `require_user_approval` |
| **list_dir** | `relative_workspace_path` |
| **grep_search** | `query`(regex) + `include_pattern` + `exclude_pattern` |
| **file_search** | `query`（文件名模糊匹配） |
| **delete_file** | `target_file` |
| **reapply** | `target_file`（调用更强模型重新应用上次编辑） |
| **web_search** | `search_term` |
| **diff_history** | 无参数（查看工作区近期修改） |
| **fetch_rules** | `rule_names`（按需加载项目规则） |

独特：语义搜索 codebase_search、reapply（动态升级 apply model）、3 种互补搜索模式。

### 3.5 Cline（7 工具，VS Code Extension）

| Tool | 关键参数 |
|------|---------|
| **read_file** | `path`（v3.77+ chunked 读取） |
| **write_to_file** | `path` + `content` |
| **replace_in_file** | `path` + `diff`(SEARCH/REPLACE 块) |
| **execute_command** | `command` |
| **search_files** | `path` + `regex` |
| **list_files** | `path` + `recursive`(bool) |
| **list_code_definition_names** | `path`（AST 级代码结构概览） |

独特：`list_code_definition_names`（轻量版 repo map）、`fetch_web`、`ask_question`。

## 4. 现有 Tool 差距分析

### 4.1 `read_file` — 缺少 offset/limit ⚠️ P0

**现状**：只有 `path` 参数。大文件读 50KB head+tail 截断，中间行完全不可见。

**各家标配**：
- Claude Code: `offset` + `limit`(默认 2000)
- Open Code: `offset` + `limit`(默认 2000)
- Cursor: `start_line_one_indexed` + `end_line_one_indexed_inclusive`(每次最多 250 行)
- Cline: v3.77+ chunked 读取

**建议**：加 `offset`(起始行号,1-indexed) + `limit`(最大行数,默认 2000)。实现改动小，收益大。

### 4.2 `shell` — 缺少 timeout ⚠️ P0

**现状**：无超时机制，靠 C1 cancel 手动中断。LLM 无法控制命令执行时长。

**各家标配**：
- Claude Code: `timeout`(默认 120s,最大 600s)
- Open Code: `timeout`(默认 120s)

**建议**：加 `timeout`(默认 120s,最大 600s)。超时自动 kill 子进程。

### 4.3 `grep` — 可选小改进 🟡 P1

**现状**：与 Claude Code 基本对齐，但缺少：
- `multiline`(多行正则匹配,如跨行函数签名搜索)
- `type`(按语言过滤如 `rust`/`ts`，底层转 `--type` 参数传给 rg)
- `-A`/`-B` 分离参数（我们只有合并的 `-C`）

**建议**：加 `multiline` + `type`。改动小，提升搜索能力。

### 4.4 `edit_file` / `write_file` / `glob` / `list_dir` — ✅ 无需改动

| Tool | 评估 |
|------|------|
| `edit_file` | old_string/new_string/replace_all 完全对齐 Claude Code |
| `write_file` | 对齐，auto-create dirs 是加分项 |
| `glob` | 对齐，100 条上限一致，mtime 排序一致 |
| `list_dir` | Claude Code 没有但我们有，没问题 |

## 5. 完全缺失的 Tool（按优先级）

### P1 — 中期建议

| Tool | 谁有 | 价值 | 备注 |
|------|------|------|------|
| **web_fetch** | Claude Code / Open Code / Cline | agent 可自主获取文档/API 参考/错误信息 | 显著提升自主解决能力 |
| **agent/task** | Claude Code / Open Code | 路线图 B6，已规划 | harness 学习价值最高 |
| **notebook_edit** | Claude Code | Jupyter 场景 | 取决于目标用户群体 |

### P2 — 远期考虑

| Tool | 谁有 | 价值 | 备注 |
|------|------|------|------|
| **ask_user** | Claude Code / Open Code / Cline | agent 主动提问澄清歧义 | 增强交互质量 |
| **apply_patch** | Codex / Open Code | 批量多文件编辑 | 对 GPT 模型更友好 |
| **LSP** | Claude Code(新) / Open Code(实验) | 跳转定义/引用 | 高价值但实现复杂 |
| **todo 管理** | Claude Code / Open Code | 结构化任务追踪 | 前端已有概念，tool 层可后补 |
| **skill 加载** | Claude Code / Open Code / Codex | 可复用指令包 | 路线图 B4，已规划 |
| **delete_file** | Cursor | 显式文件删除 | 当前 `shell rm` 替代足够 |

## 6. 实施建议

```
当前 7 个 tool 的覆盖面 ≈ Claude Code 核心高频工具的 70%

  P0（立刻做）:
  ├── read_file 加 offset + limit（改动小、各家标配）
  └── shell 加 timeout（改动小、防挂死）

  P1（中期）:
  ├── grep 加 multiline + type
  ├── web_fetch（提升 agent 自主能力）
  └── agent/task（B6，已规划）

  P2（远期）:
  ├── ask_user
  ├── apply_patch
  ├── LSP
  └── notebook_edit / skill / todo
```

## 7. 各家设计哲学对比

| Agent | 工具数 | 哲学 | 编辑范式 |
|-------|--------|------|----------|
| **Codex CLI** | 2 | 极简，shell-first | Diff/Patch |
| **Aider** | 0 (纯文本) | 非 tool calling | SEARCH/REPLACE 块 |
| **Cline** | 7 | 经典 VS Code agent | String replacement → Unified diff |
| **Everlasting** | 7 | 自研 agent core | String replacement (对齐 Claude Code) |
| **Cursor** | 12 | 工具丰富，专业分工 | 语义 diff (apply model) |
| **Claude Code** | 25+ | 最完整工具集 | String replacement |
| **Open Code** | 15 | 均衡，TypeScript 生态 | String replacement + Patch |
