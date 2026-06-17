# Everlasting Skill 系统调研

> 调研日期：2026-06-18
> 范围：第三档 **B4 Skill 系统** 的前置设计调研
> 目标：理清三件事 —— ① `use_skill` 虚拟 tool 怎么设计；② 注入位置在哪；③ 与已落地的 B3 `/command` 的复用边界。
> 方法：一手抓取 Claude Code / Hermes Agent / opencode 官方文档 + `agentskills.io` 开放标准规范，叠加本仓库现有代码现状（B3 ResourceLoader / memory loader / agent loop）。
> 配套：本文不写实现代码，只产出设计结论与待决策项；落地走 Trellis 任务（待建）。

---

## 0. TL;DR

1. **`use_skill` 虚拟 tool 是当前主流**。Claude Code 暴露一个名为 `Skill` 的虚拟工具，Hermes 暴露 `skills_list` / `skill_view` 一组虚拟工具。模型通过**工具列表**（而非 system prompt 全量注入）感知 skill 的存在，主动决定何时调用。opencode **不用**这个模式 —— 它的"自动按需加载"挂在 subagent 委托上，命令纯手动触发。

2. **业界共识：渐进式披露（progressive disclosure）三层**。① 启动时只注入所有 skill 的 `{name, description}` 清单（受 token 预算约束）；② 模型命中后调用虚拟 tool，返回 SKILL.md 正文；③ 正文里引用的 reference 文件由模型用 `read_file` 按需拉取。"挂很多 skill 但 footprint 很小"是这套设计的核心收益。

3. **本仓库已有早期构思，但需修正两处**：BACKLOG §2 写于 MVP 阶段，方向对（虚拟 tool + 双入口），但有两点过时：
   - 选型写的 `serde_yml` —— **B3 落地时已废弃**（`resource_loader.rs:9-16` 手写 parser，理由是 serde_yml/serde_yaml 均已 deprecated）。
   - 写"use_skill → 注入 **system prompt**" —— 业界实际是"注入**消息流**"（作为一条 user/system message 常驻上下文），原因见 §3.4。

4. **注入位置已精确锁定三处**（§4 给行号）：L0 清单复用 `build_instructions_blocks()`（`memory/loader.rs:325`，带 `cache_control: Ephemeral`）；虚拟 tool 注册进 `AppState.tools`（`state.rs:177`）；`use_skill` 执行进 `execute_tool_inner` 的 match 分发（`tools/mod.rs:140`）。

5. **与 `/command` 的复用边界 = skill 是 command 的 superset**（对齐 Claude Code "commands have been merged into skills"）。两者共用同一套 `ResourceLoader`（`resource_loader.rs` 的 `CommandCache` mtime fence + `parse_frontmatter`），只是**加载时机不同**：command 是用户敲 `/` 强制触发、正文当 user message；skill 是 LLM 按需触发、正文注入消息流常驻。

6. **MVP 建议**（§6）：先 ship 最小集 —— SKILL.md frontmatter 只认 `name` + `description`（对齐 agentskills.io 标准最小集），`allowed-tools` 等留作 stretch；L0 清单预算复用 `memory/tokens.rs` 估算；路径与 B3 对齐 `~/.config/everlasting/skills/<name>/SKILL.md` + `<project>/.everlasting/skills/<name>/SKILL.md`。

---

## 1. 背景与三个待解问题

第三档 B4 Skill 系统（`docs/ROADMAP.md` §2 第三档，备注"指令层扩展"）要解决的核心问题：

> 把"做某件事的方法"打包成可复用单元，既能被用户显式调（`/skill-name`），也能被 LLM 按需调（虚拟 tool）。—— BACKLOG §2

本调研要回答用户提出的三个具体设计问题：

| # | 问题 | 本文章节 |
|---|------|----------|
| Q1 | `use_skill` 虚拟 tool 怎么设计？签名、触发、与 system prompt 全量注入的取舍 | §3 |
| Q2 | 注入位置在哪？skill 清单 / 虚拟 tool 注册 / 执行分别落在哪段代码 | §4 |
| Q3 | 与已落地的 B3 `/command` 的复用边界？共享什么、分歧在哪 | §5 |

---

## 2. 业界方案调研

> 全部一手文档抓取（WebSearch/WebFetch MCP 配额本月耗尽，改 `curl` 直抓）。每条结论带 source URL；未拿到的如实标注。

### 2.1 Claude Code（发起者，渐进披露的教科书）

**来源**：[docs.claude.com/en/docs/claude-code/skills](https://docs.claude.com/en/docs/claude-code/skills) · [docs.claude.com/en/docs/claude-code/sub-agents](https://docs.claude.com/en/docs/claude-code/sub-agents)

- **双轨触发**：① 用户 `/skill-name`（手动）；② **模型自动调用** —— skill description 注入模型可见的 skill 清单，模型判断相关时通过名为 **`Skill` 的虚拟工具**主动调用。文档原话："Claude adds it to its toolkit. Claude uses skills when relevant, or you can invoke one directly with /skill-name."
- description 匹配是**模型自主判断**（非规则引擎）；每条 skill 的 `(description + when_to_use)` 合并文本被**截断到 1024 字符**塞进 skill 清单。description 末尾要写 "Use when..." 触发条件。
- 控制开关：`disable-model-invocation: true`（只允许用户触发，description 不进上下文）、`user-invocable: false`（只允许模型触发，不进 `/` 菜单）。
- **注入手段是工具清单里多一个 `Skill` 工具**，不是 system prompt 全量塞 —— 这是关键设计。

**渐进披露的精确实现**：
- **L0（常驻）**：所有 skill 的 `name` + `description` 在 session 启动时注入模型上下文，受 **1% 上下文窗口**字符预算约束，超预算时"最少被调用的优先丢 description"。
- **L1（按需）**：skill 被触发后，渲染后的 SKILL.md 正文**作为单条消息注入对话**，之后整 session 留在上下文里（不会每轮重读文件）。auto-compaction 时每 skill 留最近一次调用、合计 25000 token 预算。
- **L2（支撑文件）**：目录里的 `reference.md` / `examples.md` / `scripts/` 默认不加载，需 SKILL.md 引用、模型用 Read 按需拉取。建议 SKILL.md < 500 行。

**作用域（4 层路径）**：

| 层 | 路径 | 优先级 |
|---|---|---|
| Enterprise | managed settings | 最高 |
| Personal | `~/.claude/skills/<name>/SKILL.md` | 高 |
| Project | `.claude/skills/<name>/SKILL.md` | 中 |
| Plugin | `<plugin>/skills/<name>/SKILL.md` | 命名空间隔离（`plugin:name`） |

同名覆盖：enterprise > personal > project。旧 `.claude/commands/*.md` 仍兼容（"已合并进 skill"），但 skill 与 command 同名时 **skill 优先**。

### 2.2 Hermes Agent（兼容标准 + 大幅扩展）

**来源**：[hermes-agent.nousresearch.com/docs](https://hermes-agent.nousresearch.com/docs/) · [skills 技术参考](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills) · [creating-skills](https://hermes-agent.nousresearch.com/docs/developer-guide/creating-skills)

> Disambiguation："Hermes" 有歧义 —— ① Nous Research 的 Hermes 系列 LLM（模型）；② Hermes Agent（开源自主 agent harness）。本节指后者。它**也兼容 `agentskills.io`** 开放标准。

- **三轨触发**：① 用户 `/skill-name [task]`（每个 skill 自动注册成 slash command）；② 自然语言（"ask Hermes to use a skill, it loads via `skill_view`"）；③ **模型自主** —— agent 看 `skills_list()` 清单后自己决定 `skill_view(name)`。
- **注入位置（三层 + 工具名，文档给得最清楚）**：
  ```
  Level 0: skills_list()           → [{name, description, category}, ...]  (~3k tokens)  会话启动加载
  Level 1: skill_view(name)        → 完整 SKILL.md 内容 + metadata
  Level 2: skill_view(name, path)  → skill 内某个 reference 文件
  ```
  原话："skills don't cost tokens until they're actually used."
- **独有亮点**：
  - `skill_manage` 工具 —— agent **自己创建/改进 skill**，解决问题后主动存为 skill，形成自我改进闭环。
  - 条件激活：`requires_toolsets`（仅当某 tool 可用才显示）/ `fallback_for_toolsets`（仅当某 tool 不可用才显示——免费替代方案）。
  - `blueprint`：skill 可直接挂 cron 调度。

### 2.3 opencode（不做 skill，agent + command 二分）

**来源**：[opencode.ai/docs/agents](https://opencode.ai/docs/agents/) · [opencode.ai/docs/commands](https://opencode.ai/docs/commands/)

- **没有 "skill" 概念**。能力拆成两条线：
  - **commands（slash command）**：**只支持用户手动 `/name` 触发**，frontmatter `description` 仅在 TUI 自动补全里给用户看，**模型不会自动按 description 调用**。
  - **agents（primary/subagent）**：subagent 支持双轨 —— primary agent 自动委托（按 description）或用户 `@name` 提及。
- 所以 opencode 的"自动按需加载"**全部挂在 subagent 上**，命令纯手动。这比 Claude/Hermes 的 skill 模型更简单但能力更弱（命令无法被模型自发触发）。

### 2.4 agentskills.io 开放标准

**来源**：[agentskills.io](https://agentskills.io/) · [agentskills.io/specification](https://agentskills.io/specification)

- Anthropic 发起的开放标准：**skill = 一个含 `SKILL.md` 的目录**，frontmatter 最小字段 `name`（必，1-64，小写+连字符，须匹配父目录）+ `description`（必，1-1024）。
- 三阶段渐进披露：**Discovery → Activation → Execution**。
- Claude Code 和 Hermes 都声称兼容此标准，并在其上各自扩展（Claude 的 `disable-model-invocation` / `paths` / `hooks`；Hermes 的 `metadata.hermes.*`）都是私有扩展。

### 2.5 横向对比表

| 维度 | Claude Code | Hermes Agent | opencode |
|---|---|---|---|
| 有 "skill" 概念 | ✅（标准发起者） | ✅（兼容标准 + 扩展） | ❌（拆成 agent + command） |
| 触发：用户手动 `/` | ✅ | ✅（skill 自动=slash cmd） | ✅（command） |
| 触发：模型自动按 description | ✅（虚拟工具 `Skill`） | ✅（虚拟工具 `skill_view`） | ⚠️ 只 subagent 支持；command 不支持 |
| 虚拟工具名 | `Skill(name)` | `skills_list` / `skill_view` / `skill_manage` | 无 |
| 渐进披露层数 | 2 层（清单常驻 + 正文按需）+ 支撑文件 | 3 层（清单/正文/reference） | 1 层（subagent 描述委托） |
| description 清单预算 | 1% 窗口 + 每条 1024 字符封顶 | ~3k token 清单 | 未公开 |
| 正文去留 | 触发后常驻（compaction 留预算） | `skill_view` 按次返回 | command 一次性 prompt |
| 作用域层级 | enterprise/personal/project/plugin/嵌套 5 层 | 主目录 + 外部目录 + plugin 命名空间 | global/project 2 层 |
| command 独立性 | **已并入 skill**（旧 commands 兼容） | 无独立 command（skill 即命令） | 独立（frontmatter 简化版） |
| subagent 关系 | skill 可 `context:fork` 驱动 subagent | 正交（delegates 并行） | command 可 `agent:+subtask` 跑进 subagent |
| 权限控制 | `Skill(name)` deny/allow + `allowed-tools` | toolset 级条件激活 | `permission: edit/bash` allow/ask/deny |
| 自我创建 skill | ❌ | ✅ `skill_manage` | ❌ |
| 兼容开放标准 | 发起 agentskills.io | 兼容 | 不参与 |

---

## 3. 问题一：`use_skill` 虚拟 tool 设计

### 3.1 为什么是虚拟 tool，而非 system prompt 全量注入

业界（Claude Code、Hermes）一致选虚拟 tool，理由：

1. **上下文成本可控**。全量注入会把 skill 数量限死在上下文窗口内；虚拟 tool 让"挂很多 skill 但 footprint 很小"成为可能 —— 清单常驻（只 name+description），正文按需。
2. **复用模型已有的 tool-use 能力**。description 就是 tool description，模型用固有的 function-calling 决策何时调用，无需自研调度器/规则引擎。
3. **权限/审计统一走 tool 通道**。本项目已有 ⑨ 关 5-tier 权限层 + ⑩ `tool_executed` 审计行（`docs/ARCHITECTURE.md` §2.3-§2.4），把 skill 当普通 tool 管，`Skill(name)` 可自然接入 deny/allow + 审计，零额外架构。
4. **与 `use_memory` / `use_ui` 一致**。`ARCHITECTURE.md:480-482` 已为 `use_skill` / `use_memory` / `use_ui` 三者预留了虚拟 tool 占位，是一组同构设计。

opencode 的反例印证了这一点：它**不做**虚拟 tool，代价是 command 无法被模型自发触发，自动按需只能靠 subagent 委托 —— 能力弱一档。

### 3.2 三层渐进披露（推荐采纳 Hermes 的最清晰模型）

| 层 | 内容 | 何时加载 | Everlasting 落地 |
|---|---|---|---|
| **L0 Discovery** | 所有 skill 的 `{name, description}` 清单 | session 启动 / 首轮 | 注入到 synthetic user message（复用 `build_instructions_blocks`，§4.1） |
| **L1 Activation** | 某个 skill 的 SKILL.md 正文 | 模型调 `use_skill(name)` 时 | `use_skill` 执行分支返回正文（§4.3） |
| **L2 Execution** | skill 目录里的 reference 文件 | 正文里引用、模型按需 | 模型用现有 `read_file` tool 拉，**无需新代码** |

> 关键："skills don't cost tokens until they're actually used"（Hermes 原话）。L0 是唯一常驻成本。

### 3.3 工具签名与 description 写法

**虚拟 tool 定义**（对齐 Claude Code 的 `Skill` / Hermes 的 `skill_view`）：

```text
name:        use_skill
description: 当用户的任务可能匹配某个已注册 skill 的 description 时调用，
             以加载该 skill 的完整指令正文。可用 skill 清单见上方注入。
             Use when the task matches a skill's description.
input_schema: { skill_name: string (required), args?: string }
```

要点：
- `description` 必须含 "Use when..." 触发句（Claude Code 文档强调），且单条封顶（建议 1024 字符，对齐标准）。
- L0 清单里**每个 skill 的 description 单独截断**，超长丢弃并 `warn!`（对齐 Claude Code 的 `/doctor` 诊断思路）。
- `args` 选填，留给将来 skill 接参数（如 `/review-pr <branch>`）；MVP 可不实现插值。

### 3.4 与早期 BACKLOG 构思的关键差异

BACKLOG §2（MVP 阶段写）说：

> 我们的 runtime：看到 use_skill 调用 → 展开 skill 内容 → **注入到 system prompt**

**业界实际不是注入 system prompt，而是注入消息流**（作为一条消息常驻上下文）。原因：

1. **system prompt 是 cache 边界之前的内容**。本项目 `build_instructions_blocks()` 把指令放在 synthetic **user** message 里（`memory/loader.rs:325`），而非 system prompt —— 这正是为了让 `cache_control: Ephemeral` 命中（system prompt 段若频繁变会破坏缓存）。把 skill 正文塞 system prompt 会破坏现有 cache 结构。
2. **Anthropic API 的 system 是稳定段**。skill 是按需动态加入的，塞 system 会让 system prompt 在 session 中途变化，破坏 prompt caching（5 分钟 TTL）。
3. **Claude Code 原话**：触发后"渲染后的 SKILL.md 正文**作为单条消息注入对话**，之后整 session 留在上下文里"。

**修正建议**：`use_skill` 执行后，把 skill 正文作为一条 **user 消息**（或 system-reminder 风格的标记消息）追加进 `messages`，**不动 system prompt**。这与本项目 memory 指令的注入方式（synthetic user message）完全一致，可复用同一套 block 构造逻辑。

---

## 4. 问题二：注入位置（精确代码定位）

三个接入点，全部已在本仓库定位到行号：

### 4.1 L0 清单注入 —— 复用 `build_instructions_blocks()`

**文件**：`app/src-tauri/src/memory/loader.rs:325` + `app/src-tauri/src/agent/chat_loop.rs:233-248`

现状（每轮开头注入 4 个指令文件）：

```rust
// chat_loop.rs:233
let instructions_blocks =
    crate::memory::loader::build_instructions_blocks(&memory_layers);
if !instructions_blocks.is_empty() {
    // 构造 synthetic user message + assistant ack（:240 / :248）
}
```

**Skill L0 清单的复用方式**：新增一个 `build_skill_listing_block()`（与 `build_instructions_blocks` 同构），返回单个 `ContentBlock::Text`，内容是所有 skill 的 `{name, description}` 渲染成的清单文本。两种合并方案：

- **方案 A（推荐 MVP）**：作为 `instructions_blocks` 的**附加 block**，跟在 memory block 之后，共享同一条 synthetic user message + 同一个 `cache_control` 断点。零新消息槽，cache 友好。
- **方案 B**：独立的第二条 synthetic message（skill 清单变更频率与 memory 不同时更干净，但多一条消息）。

清单内容示例：
```text
<available-skills>
- review-pr: review 一个 PR / diff，给出结构化反馈。Use when 用户要求 review。
- commit: 用约定格式提交当前变更。Use when 用户说 commit。
</available-skills>
```

### 4.2 虚拟 tool 注册 —— `AppState.tools`

**文件**：`app/src-tauri/src/state.rs:76`（`pub tools: Vec<ToolDef>`）+ `state.rs:177`（`let tools = crate::tools::builtin_tools();`）

现状：tool list 在 `AppState::load` 时**一次性**构建，存进 `AppState.tools`，每 chat 请求 `chat.rs:79` `let tool_defs = state.tools.clone();` 克隆传入 `run_chat_loop`。

**接入方式**：在 `builtin_tools()`（`tools/mod.rs:37`）里追加 `skill::definition()`，把 `use_skill` 虚拟 tool 加进 Vec。

> 注意：这是**静态注册**（所有 session 都有 `use_skill`）。若将来要支持 `disable-model-invocation`（只用户触发）或 `allowed-tools` 限 tool，需改成 per-session 动态 tool list（`tool_defs` 在 `run_chat_loop` 内按 session mode/skill 配置过滤）。MVP 先静态。

### 4.3 `use_skill` 执行 —— `execute_tool_inner` 分发

**文件**：`app/src-tauri/src/tools/mod.rs:140`（`execute_tool_inner` 的 match 分发）+ `ARCHITECTURE.md:480`（⑩ 表已预留占位）

现状 match 分发（`read_file` / `write_file` / ... / `web_fetch`），`_ => Unknown tool`。

**接入方式**：加一个分支：

```rust
"use_skill" => {
    // 从 input 取 skill_name → 从 SkillCache（复用 CommandCache 模式）查正文
    // → 返回 (正文, is_error=false, default_ctx, None)
    // agent loop 的 ⑫ tool_result 回填 会把正文作为 tool_result 注入 messages
}
```

`use_skill` 的返回值 = skill 正文。这里有个**设计抉择**（§7 开放问题 Q3）：正文是作为 `tool_result` 回填（走标准 ⑫ 路径），还是作为独立的 system/user message 注入？两种都对，影响 compaction 行为。

### 4.4 整体数据流（skill 模式）

```
session 启动
  └─ SkillCache.list_all()（mtime fence，复用 B3 模式）
      → [{name, description}, ...]
  └─ build_skill_listing_block()（§4.1）
      → 作为 instructions 附加 block，进 synthetic user message（cache_control 命中）

每轮 agent loop
  └─ LLM 看 tool list 里有 use_skill + 上方清单
      → 命中时 emit tool_use("use_skill", {skill_name})
  └─ execute_tool_inner "use_skill" 分支（§4.3）
      → 从 SkillCache 取 SKILL.md 正文
      → 作为 tool_result 回填（⑫ 路径，正文进 messages 常驻）
  └─ LLM 拿到正文，按其指令执行（必要时 read_file 拉 reference，L2）
```

---

## 5. 问题三：与 `/command` 的复用边界

### 5.1 共享 `ResourceLoader`（核心复用点）

B3 已落地的 `resource_loader.rs` 提供了一套现成的、经过测试的能力，skill 可**直接复用**：

| B3 能力（`resource_loader.rs`） | skill 复用方式 |
|---|---|
| `parse_frontmatter()`（手写 40 行）| 解析 SKILL.md 的 `name`/`description`（+ 将来 `allowed-tools`） |
| `CommandCache`（`RwLock` + mtime fence）| 改名 `ResourceCache` 或新建 `SkillCache`，同构 |
| `read_through()`（mtime 比对，无 background watcher）| skill 文件变更检测，同构 |
| `scan_dir()` / `load_command_file()`（64KiB cap + 非 UTF-8 容错 + bad-file skip）| skill 目录扫描，同构 |
| `list_all()`（builtin > project > user precedence）| skill 优先级合并，同构 |
| `user_dir()`（`~/.config/everlasting/`）| skill 路径同根 |

`resource_loader.rs:9-16` 的注释**已经预留了这个口子**：

> Future Skill / Memory / Role loaders with complex frontmatter can graduate to a maintained YAML crate (`serde_yaml_neo`) — the parser lives behind `parse_frontmatter` so the swap is local.

**唯一分歧：frontmatter 复杂度**。command 的 frontmatter 是纯标量（`name`/`description`/`argument-hint`），手写 parser 够用。skill 若要支持 `allowed-tools: [a, b]`（数组）+ 多行 `description: |`（块标量），手写 parser 会吃力。

### 5.2 加载时机矩阵（复用边界 = 时机不同）

BACKLOG §3 已点明核心论断 —— **Skill + Memory + Role 都走同一个 loader，只是触发时机不同**。本仓库现状对照：

| 类型 | 加载时机 | 触发方式 | 注入去向 | 现状 |
|---|---|---|---|---|
| Memory | 每次 LLM 调用前 | 自动 | synthetic user message（`cache_control`） | ✅ 已落地（B5） |
| Skill | LLM 显式调（+ 用户可手动） | `use_skill` tool + `/skill` | L0 清单常驻；L1 正文注入消息流 | 🟠 B4 本任务 |
| /command | 用户显式调 | 键盘 `/` | 正文当 user message 走 `send()` | ✅ 已落地（B3） |
| Role | session 启动时 | UI 选 | system prompt base + suffix | 🔴 未做（第四档 B8 周边） |

**复用边界结论**：skill 和 command 共享**加载层**（ResourceLoader + Cache + parse），分叉在**注入层**：
- command：前端 `ChatInput` `/` 触发 → `get_command_body` 拿 body → 前端把它当 **user message** 走 `send()`（用户消息路径，一次性）。
- skill：LLM `use_skill` → 后端 `execute_tool` 返回正文 → 走 **tool_result 路径**注入（常驻）。

### 5.3 skill 是 command 的 superset（对齐 Claude Code）

Claude Code 官方明确："Custom commands have been merged into skills"。命令是 skill 的子集（单文件、无支撑目录、无 frontmatter 控制）。

**对本项目的含义**：
- 长期看，`.everlasting/commands/*.md`（单文件）应是 `.everlasting/skills/<name>/SKILL.md`（目录）的简化形态。
- 但 **B3 已 ship 且独立工作良好**，MVP 阶段**不必强行合并**（避免回归风险）。建议：B4 先独立实现 `SkillCache`，复用 B3 的 parser/cache 代码模式（甚至抽公共 trait），等稳定后再考虑统一为一个 `ResourceLoader<Kind>` 泛型层。
- 一个 skill **可以**额外暴露成 `/skill-name`（复用 B3 的 TriggerMenu + `/` 前缀检测）—— 这就是 BACKLOG §2 说的"一个 skill 可以暴露成 /command，但反之不必然"。

### 5.4 路径约定（与 B3 对齐）

```
~/.config/everlasting/skills/<name>/SKILL.md          # user 层
<project>/.everlasting/skills/<name>/SKILL.md          # project 层（覆盖 user）
```

- 复用 B3 的 `user_dir()`（`~/.config/everlasting/`）+ `PROJECT_NAMESPACE`（`.everlasting/`）。
- 优先级与 B3 一致：project > user（builtin 目前只有 command 有，skill 无 builtin 或仅占位）。
- 命名约束对齐 agentskills.io：`name` 小写+连字符、须匹配父目录名。

### 5.5 frontmatter parser 升级决策

| 方案 | 适用 | 代价 |
|---|---|---|
| **A. 复用手写 parser**（MVP）| frontmatter 只要 `name` + `description`（标量） | 0 依赖，但 `allowed-tools` 数组/多行 description 不支持 |
| **B. 升级 `serde_yaml_neo`** | 要 `allowed-tools` 数组、块标量 description | +1 依赖；swap 局部（`parse_frontmatter` 背后） |
| **C. 手写扩展**（支持数组）| 折中 | parser 复杂度上升，维护成本 |

**建议**：MVP 走 **A**（对齐 agentskills.io 标准最小集 `name`+`description`），`allowed-tools` 留 stretch。若 stretch 要做，再走 **B**（`resource_loader.rs` 注释已点名 `serde_yaml_neo`）。理由：先用最小 frontmatter 验证 L0/L1/L2 数据流跑通，frontmatter 富化是正交的后续。

---

## 6. 对 Everlasting MVP 的设计建议

### 6.1 MVP 范围（做什么 / 不做什么）

**做（B4 PR1）**：
- [ ] `SkillCache`（复用 `CommandCache` 模式）+ skill 目录扫描（user + project 两层）
- [ ] `build_skill_listing_block()`（L0 清单，§4.1 方案 A）
- [ ] `use_skill` 虚拟 tool：定义（§4.2）+ 执行分支（§4.3）
- [ ] SKILL.md frontmatter 最小集 `name` + `description`（方案 A parser）
- [ ] 用户 `/skill-name` 手动触发（复用 B3 TriggerMenu + `get_command_body` 同构的 `get_skill_body`）

**不做（stretch / 后续 PR）**：
- `allowed-tools` / `disallowed-tools`（tool 限制）
- `disable-model-invocation` / `user-invocable` 开关
- L2 reference 文件显式支持（已隐式由 `read_file` 覆盖）
- 预算制（1% 窗口）—— skill 数量少时先不做，超阈值再上
- 条件激活（Hermes 的 `requires_toolsets`）
- 自我创建 skill（Hermes 的 `skill_manage`）
- skill 与 command 合并成一个泛型 loader

### 6.2 选型修正（覆盖 BACKLOG §2 过时项）

| BACKLOG §2 原写 | 修正为 | 依据 |
|---|---|---|
| `serde_yml` 解析 frontmatter | 手写 parser（复用 B3）或 `serde_yaml_neo` | `serde_yml`/`serde_yaml` 均已 deprecated（`resource_loader.rs:9`） |
| use_skill → 注入 **system prompt** | use_skill → 正文注入**消息流**（tool_result / synthetic message） | Claude Code 原话 + 本项目 cache_control 结构（§3.4） |
| 风险"限制单 session 可见 skill 数" | 同意，MVP 先不设硬限，靠 description 质量 | Claude Code 的 1% 预算是 stretch |

### 6.3 权限 / 审计接入

- `use_skill` 作为普通 tool，自动进 ⑨ 关 5-tier 权限层（Tier 4 走 `match_kind='tool'`，与 `web_fetch` 同）。
- `use_skill` 执行进 ⑩ `tool_executed` 审计行（`AuditKind`），前端 ToolCallCard 可复用展示。
- **恶意指令风险**（BACKLOG §2 已列）：skill 文件位置隔离（user vs project），project skill 首次加载时可考虑显式 approve（stretch；MVP 先信任，单用户桌面场景）。

### 6.4 风险与缓解

| 风险 | 缓解 |
|---|---|
| skill 数量爆炸，模型不知道用哪个 | description 写 "Use when..."；L0 清单封顶；将来上 1% 预算 |
| skill 注入恶意指令 | 文件位置隔离；project skill approve（stretch） |
| skill 正文常驻吃 context | 触发后才进；compaction 时按预算裁剪（对齐 Claude Code 25k token/skill） |
| skill 与 command 同名冲突 | 优先级：skill > command（对齐 Claude Code）或 builtin > project > user 统一裁决 |
| 正文进 system prompt 破坏 cache | **注入消息流，不动 system prompt**（§3.4） |

---

## 7. 待决策的开放问题

> 这些是落地前需要 grill / 定夺的设计点，建议进 Trellis brainstorm。

- **Q1（L0 注入合并方式）**：skill 清单作为 memory instructions 的附加 block（方案 A，cache 友好），还是独立 synthetic message（方案 B，隔离干净）？ → **倾向 A**。
- **Q2（正文注入路径）**：`use_skill` 正文走标准 `tool_result` 回填（⑫，复用现有路径），还是独立的 system-reminder 风格消息？ → **倾向 tool_result**（零新路径，审计统一）。
- **Q3（tool list 动态化）**：MVP 是否需要 per-session 动态 tool list（支持 `disable-model-invocation`）？ → **倾向否**，先静态注册 `use_skill`。
- **Q4（与 command 合并时机）**：B4 先独立 `SkillCache`，还是直接抽 `ResourceLoader<Kind>` 泛型层把 command 一起重构？ → **倾向先独立**（避免 B3 回归），合并留稳定后。
- **Q5（frontmatter 富化）**：MVP 是否含 `allowed-tools`？含则需升级 parser（方案 B/C）。 → **倾向不含**，最小集先跑通。
- **Q6（用户手动 `/skill` 入口）**：复用 B3 TriggerMenu 的第二 caller（像 B2 @文件 那样），还是单独 UI？ → **倾向复用 TriggerMenu**（B3 设计时已预留为共享组件）。

---

## 8. 来源

**一手（均实际抓取）**：
- Claude Code Skills 官方文档：https://docs.claude.com/en/docs/claude-code/skills
- Claude Code Sub-agents 官方文档：https://docs.claude.com/en/docs/claude-code/sub-agents
- Agent Skills 开放标准：https://agentskills.io/ · https://agentskills.io/specification
- Hermes Agent Skills 技术参考：https://hermes-agent.nousresearch.com/docs/user-guide/features/skills
- Hermes Creating Skills：https://hermes-agent.nousresearch.com/docs/developer-guide/creating-skills
- opencode Agents：https://opencode.ai/docs/agents/ · opencode Commands：https://opencode.ai/docs/commands/

**未取得可靠一手资料（如实说明）**：
- Cursor rules（`.cursor/rules/*.mdc`）：docs.cursor.com 为客户端渲染，`curl` 只拿到 JS shell。通用认知是 "Agent Requested / Always / Manual" 三种触发，Agent Requested 按 description 自动注入 —— **建议二次核实**。
- Cline / Goose (block) / Aider：文档站结构/JS 渲染问题未拿下，不在结论引用。

**本仓库内部依据**：
- `app/src-tauri/src/resource_loader.rs`（B3 ResourceLoader / CommandCache / parse_frontmatter）
- `app/src-tauri/src/memory/loader.rs:325`（`build_instructions_blocks`，L0 复用点）
- `app/src-tauri/src/agent/chat_loop.rs:233`（指令注入点）
- `app/src-tauri/src/state.rs:76/177`（`AppState.tools` 注册点）
- `app/src-tauri/src/tools/mod.rs:140`（`execute_tool_inner` 分发点）
- `docs/ARCHITECTURE.md:480`（⑩ 表 `use_skill` 占位）
- `docs/BACKLOG.md` §2（Agent Skill 系统早期构思）/ §3（共同关注点 + 加载时机矩阵）
