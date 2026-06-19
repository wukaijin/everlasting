# System Prompt 调研与方案设计

> 调研日期：2026-06-19
> 调研范围：Claude Code v2.0, OpenCode, Hermes Agent, Aider
> 评审：2026-06-19（见 §7，修订了 §5 推荐路径）

---

## 1. Everlasting 现状

### 1.1 数据流概览

```
LLM 收到的请求
┌─ system (top-level, per API schema) ──────────────────────┐
│  mode_system_prefix()           ← plan/edit/yolo 行为约束   │
│  + build_system_prompt()        ← session/project 元数据    │
│    "You are a coding agent. You have access to tools..."    │
│    Session ID: xxx                                         │
│    Project: xxx (/path)                                    │
│    Working directory: /path                                │
│    Worktree: ACTIVE on branch 'session/id' (HEAD abc1234)  │
│  ≈ 20 行，纯机械生成，无行为准则                              │
├───────────────────────────────────────────────────────────┤
│ messages[0]  role=user     ← build_instructions_blocks()   │
│   banner: "Instructions loaded from…" + cache_control       │
│   AGENTS.md → <primary instructions>                       │
│   CLAUDE.md → <reference>                                  │
│ messages[1]  role=assistant                                 │
│   "Understood. I will follow these instructions…"           │
│ messages[2]  role=user     ← skill listing block            │
│ messages[3..] role=user/assistant ← 真正的对话               │
└───────────────────────────────────────────────────────────┘
```

### 1.2 关键代码位置

| 文件 | 作用 |
|---|---|
| `agent/system_prompt.rs:56-96` | `build_system_prompt()` — 生成 "You are a coding agent…" + 元数据 |
| `agent/permissions/mod.rs:1321-1344` | `mode_system_prefix()` — 4 种模式的行为约束 |
| `agent/chat_loop.rs:327` | 拼接：`format!("{}\n\n{}", mode_prefix, base_prompt)` |
| `agent/chat_loop.rs:695-696` | 传给 provider：`provider.send(Some(system_prompt.clone()), …)` |
| `memory/loader.rs:325-365` | `build_instructions_blocks()` — 指令文件走 user message（非 system）|

### 1.3 当前缺失

| 维度 | 有？ | 说明 |
|---|---|---|
| 身份声明 | ✅ 简陋 | "You are a coding agent" — 一句话 |
| 语气/风格 | ❌ | 模型自行发挥 |
| 主动性策略 | ❌ | 何时主动做、何时该问？无指导 |
| 专业客观性 | ❌ | 可能盲目同意用户 |
| 任务管理 | ❌ | 有 TodoWrite tool，但没告诉模型要用 |
| 工具使用最佳实践 | ❌ | 何时并行调用？何时用专用工具而非 bash？ |
| 代码规范 | ❌ | 遵循已有约定？不随意加注释？不假设库可用？ |
| 完成标准 | ❌ | 描述 = 完成？还是必须执行+验证？ |
| Git 安全 | ❌ | 是否应禁止 force push 等危险操作？ |
| 模型族特化 | ❌ | Gemini/GPT/Claude 的行为指令都一样 |
| 分层缓存策略 | ❌ | 全部塞在 system 字段，无法做差异化缓存 |

---

## 2. 行业调研

### 2.1 Claude Code v2.0（~500 行）

**来源**：Anthropic 官方，社区逆向提取（`x1xhlol/system-prompts-and-models-of-ai-tools`）

**结构**：

```
1. 身份声明        "You are a Claude agent, built on Anthropic's Claude Agent SDK"
2. 语气和风格      简洁（<4 行），直接，不过度解释
3. 主动性          平衡：该做的时候做，不该让用户惊讶
4. 专业客观性      技术准确性 > 验证用户信念
5. 任务管理        TodoWrite 频繁使用，完成立刻标记
6. 工作流程        软件工程任务的标准步骤
7. 工具使用策略     并行化、用专用工具、WebFetch 重定向跟踪
8. 代码引用        使用 file_path:line_number 格式
9. Git 安全协议     禁止 force push、不改 git config、不跳过 hooks
10. Shell 最佳实践  目录验证、引号空格路径、不滥用 cd
```

**核心精华片段**：

```text
## Professional objectivity
Prioritize technical accuracy and truthfulness over validating the user's beliefs.
It is best for the user if Claude honestly applies the same rigorous standards to
all ideas and disagrees when necessary, even if it may not be what the user wants
to hear.

## Tool usage policy
When doing file search, prefer to use the Task tool in order to reduce context usage.
You can call multiple tools in a single response. When multiple independent pieces
of information are requested, batch your tool calls together for optimal performance.
Use specialized tools instead of bash commands when possible.

## Task Management
Use the TodoWrite tool VERY frequently. It is critical that you mark todos as
completed as soon as you are done with a task. Do not batch up multiple tasks
before marking them as completed.
```

### 2.2 OpenCode（多套变体）

**来源**：`anomalyco/opencode` 源码 (`packages/opencode/src/session/prompt/`)

**多 Provider 策略**：`system.ts` 的 `provider()` 函数根据 model ID 选择不同提示词文件：

| 文件 | 适用模型 | 特点 |
|---|---|---|
| `anthropic.txt` | claude-* | 完整版，"best coding agent on the planet" |
| `default.txt` | 其他（fallback） | 通用版，更保守的主动性 |
| `gpt.txt` | gpt-* | GPT 专属 |
| `beast.txt` | gpt-4/o1/o3 | 高自主模式，"keep going until resolved" |
| `codex.txt` | codex-* | Codex 专属 |
| `gemini.txt` | gemini-* | Gemini 专属 |
| `plan.txt` / `plan-mode.txt` | plan agent | Read-only 5-phase 工作流 |

**default.txt 关键特色**：

```text
## Code style
IMPORTANT: DO NOT ADD ***ANY*** COMMENTS unless asked

## Following conventions
NEVER assume that a given library is available, even if it is well known.
When you create a new component, first look at existing components to see how
they're written.

## Proactiveness
You are allowed to be proactive, but only when the user asks you to do something.
Do not add additional code explanation summary unless requested by the user.
After working on a file, just stop.

## Response verbosity
You MUST answer concisely with fewer than 4 lines of text, unless user asks for detail.
```

**plan-mode.txt 的 5-phase 工作流**：
1. Phase 1 — 理解（explore agent 并行探索）
2. Phase 2 — 设计（general agent 设计实现方案）
3. Phase 3 — 审查（验证方案对齐用户意图）
4. Phase 4 — 最终计划（写入 plan file）
5. Phase 5 — 调用 plan_exit（申请审批）

### 2.3 Hermes Agent（~1000+ 行，3 层架构）

**来源**：`NousResearch/Hermes-Agent` 源码 (`agent/system_prompt.py`, `agent/prompt_builder.py`)

**3 层架构是本调研中最重要的发现**：

```
┌─ stable 层（缓存友好，session 内不变）─────────────────────┐
│  SOUL.md 或 DEFAULT_AGENT_IDENTITY                          │
│  + HERMES_AGENT_HELP_GUIDANCE                               │
│  + TASK_COMPLETION_GUIDANCE          ← 完成标准             │
│  + PARALLEL_TOOL_CALL_GUIDANCE       ← 并行调用             │
│  + 工具相关指导 (memory/skills/kanban/session_search)       │
│  + TOOL_USE_ENFORCEMENT_GUIDANCE     ← 必须调用工具          │
│  + 模型族特化 (Gemini/GPT/Codex 各不同)                     │
│  + 环境提示 + 平台提示 + 编码姿态                            │
├─ context 层（session 间可能变化）─────────────────────────│
│  + system_message (caller-supplied)                         │
│  + 上下文文件 (.hermes.md, HERMES.md, AGENTS.md)            │
├─ volatile 层（每次会话不同）────────────────────────────────│
│  + memory 快照 + USER.md                                    │
│  + 时间戳 / session ID / model / provider                   │
└──────────────────────────────────────────────────────────┘
```

**设计原则**（来自 `system_prompt.py` 注释）：
> The agent's system prompt is built once per session and reused across all turns — only context compression triggers a rebuild. This keeps the upstream prefix cache warm.

**最有价值的模块 — 模型族特化**：

Hermes 对 Gemini 和 GPT/Codex 提供了不同的操作指导。例如 `OPENAI_MODEL_EXECUTION_GUIDANCE`：

```text
<tool_persistence>
- Use tools whenever they improve correctness, completeness, or grounding.
- Do not stop early when another tool call would materially improve the result.
- Keep calling tools until: (1) the task is complete, AND (2) you have verified the result.
</tool_persistence>

<mandatory_tool_use>
NEVER answer these from memory — ALWAYS use a tool:
- Arithmetic, math → terminal or execute_code
- Hashes, encodings, checksums → terminal
- Current time, date → terminal
- System state: OS, CPU, memory, disk, ports → terminal
- File contents, sizes, line counts → read_file or terminal
- Git history, branches, diffs → terminal
</mandatory_tool_use>

<act_dont_ask>
When a question has an obvious default interpretation, act on it immediately.
Only ask for clarification when the ambiguity genuinely changes what tool you would call.
</act_dont_ask>
```

### 2.4 Aider（~10 行行为指令）

**来源**：`Aider-AI/aider` 源码 (`aider/coders/`)

**极简主义**。`main_system` 只有一段通用身份 + 编辑格式规则：

```python
main_system = """Act as an expert software developer.
Always use best practices when coding.
Respect and use existing conventions, libraries, etc that are already present in the code base.
Take requests for changes to the supplied code.
If the request is ambiguous, ask questions.
"""
```

绝大部分 system prompt 内容都是编辑格式规则（SEARCH/REPLACE block 格式、unified diff 格式等），而非行为准则。这是因为 Aider 的设计哲学是**通过编辑格式约束行为**，而非通过语言指令。

### 2.5 对比矩阵

| 维度 | Claude Code | OpenCode | Hermes | Aider | **Everlasting** |
|------|:---:|:---:|:---:|:---:|:---:|
| **身份声明** | ✅ 详细 | ✅ 详细 | ✅ 多层 | ✅ | ✅ 一句话 |
| **语气/风格** | ✅ 详细 | ✅ 详细 | ❌ | ❌ | ❌ |
| **主动性策略** | ✅ | ✅ | ❌ | ❌ | ❌ |
| **专业客观性** | ✅ | ✅ | ❌ | ❌ | ❌ |
| **任务管理** | ✅ | ✅ | ✅ kanban | ❌ | ❌ |
| **工具使用策略** | ✅ 详细 | ✅ 详细 | ✅ 极详细 | ❌ | ❌ |
| **代码规范** | ❌ | ✅ "NO comments" | ❌ | ✅ 弱 | ❌ |
| **完成标准** | ❌ | ❌ | ✅ 强 | ❌ | ❌ |
| **模式相关** | 子 agent | plan/build/beast | kanban | architect | ✅ 简陋 |
| **模型族特化** | ❌ | ✅ 多文件 | ✅ 详细 | ❌ | ❌ |
| **Git 安全** | ✅ 详细 | ❌ | ❌ | ❌ | ❌ |
| **分层缓存** | ❌ | ❌ | ✅ 3 层 | ❌ | ❌ |
| **可定制性** | CLAUDE.md | CLAUDE.md | SOUL.md | CONVENTIONS.md | AGENTS/CLAUDE.md |

---

## 3. 跨工具共性主题

从 4 个工具的 system prompt 中可以提取出 **6 个所有 coding agent 都需要的行为模块**：

### A. 身份与语气 (Identity & Tone)

```text
共识：简洁、直接、专业、不用 emoji（除非用户要求）
分歧：个性的强弱 — OpenCode anthropic.txt 最强（"best coding agent on the planet"），Aider 最弱
```

### B. 主动性边界 (Proactiveness)

```text
共识：该做的时候做，不该让用户惊讶
关键问题：什么时候主动做（如修 bug），什么时候先问（如改架构）？
Claude Code & OpenCode 的答案："当用户明确要求时"再主动，否则先问
```

### C. 任务管理 (Task Management)

```text
共识：复杂任务用 TodoWrite → 分解 → 逐步完成 → 立即标记
反例：Everlasting 有 TodoWrite tool 但 system prompt 没告诉模型要用
```

### D. 工具使用策略 (Tool Usage)

```text
核心原则（所有工具都有）：
1. 独立调用并行化（减少 round-trip）
2. 优先用专用工具而非 bash（Read > cat, Edit > sed, Grep > grep）
3. 需要前置信息时先收集再行动（prerequisite checks）
4. 完成后验证，不止于描述
```

### E. 代码规范 (Code Conventions)

```text
共识（OpenCode 表达最明确）：
1. 先看已有代码风格，模仿它
2. 不假设库/框架可用，先检查 package.json/Cargo.toml
3. 不确定时不要加注释（比乱加注释好）
4. 遵循安全最佳实践（不暴露 secrets）
```

### F. 专业客观性 (Professional Objectivity)

```text
共识：技术准确性 > 讨好用户
Claude Code 和 OpenCode 都有专门的 "Professional objectivity" 节
Hermes 和 Aider 没有（可能因为它们是工具而非产品）
```

---

## 4. 方案设计

### 4.1 方案 A：轻量注入（推荐起步方案）

**思路**：在现有 `build_system_prompt()` 基础上追加一个 `default_behavior_prompt()` 常量，不改架构。

**改动范围**：
- 新增 `agent/behavior_prompt.rs` — 一份 ~200 行的默认行为准则
- 修改 `agent/chat_loop.rs:327` — 拼接逻辑从 `mode_prefix + base_prompt` 变为 `mode_prefix + behavior_prompt + base_prompt`
- 可选：在 Settings 里加一个 "Custom System Prompt" 文本框

**优点**：改动最小、立刻生效、不破坏现有缓存策略
**缺点**：无法做 3 层缓存差异化；指令文件（AGENTS/CLAUDE.md）仍走 user message

**默认行为准则内容**（参考上述 6 个共性模块）：

```rust
pub const DEFAULT_BEHAVIOR_PROMPT: &str = "\
# Tone and style
- Be concise, direct, and to the point. A concise response is generally
  less than 4 lines, not including tool calls or code generation.
- Answer the user's question directly, without elaboration or
  explanation unless the user asks for detail.
- Only use emojis if the user explicitly requests it.
- Do not add code explanation summaries unless requested.

# Professional objectivity
- Prioritize technical accuracy and truthfulness over validating
  the user's beliefs.
- Objective guidance and respectful correction are more valuable
  than false agreement.

# Task management
- Use the TodoWrite tool to plan and track complex tasks (3+ steps).
- Mark todos as completed as soon as you are done — do not batch completions.

# Tool usage
- Batch independent tool calls into a single response to reduce round-trips.
- Prefer specialized tools over bash: Read > cat, Edit > sed,
  Write > cat/heredoc, Grep > grep.
- Never use bash echo or comments to communicate — output text directly.

# Code conventions
- Before making changes, understand the file's existing code conventions.
  Mimic code style, use existing libraries and utilities.
- NEVER assume a library is available without checking imports/dependencies first.
- Do not add comments unless asked.

# Finishing work
- When asked to build, run, or verify something, the deliverable is a working
  artifact backed by real tool output — not a description of one.
- Keep working until the task is actually complete, then verify.

# Git safety
- NEVER run destructive git commands (push --force, hard reset, etc.)
  unless the user explicitly requests them.
- NEVER commit changes unless the user explicitly asks you to.
";
```

### 4.2 方案 B：3 层架构（对标 Hermes）

**思路**：将 system prompt 拆成 stable/context/volatile 三层，利用 Anthropic prompt caching 的 breakpoint 机制。

**架构**：

```
┌─ stable 层（整个 session 不变，一次构建，缓存命中率高）────┐
│  DEFAULT_BEHAVIOR_PROMPT        ← 行为准则（新）            │
│  + mode_system_prefix()         ← 模式约束（现有）          │
│  + model_family_guidance()      ← 模型族特化（新）          │
│  + 工具使用指导（新）                                      │
├─ context 层（session 创建时确定，session 内不变）──────────┤
│  + build_system_prompt() 元数据（现有）                     │
│  + 指令文件内容（从 user message 提升到此）                 │
│    AGENTS.md → <primary instructions>                     │
│    CLAUDE.md → <reference>                                │
│  + skill listing（从 user message 提升到此）               │
├─ volatile 层（每次请求可能不同）────────────────────────────┤
│  + memory 快照 + 用户档案                                  │
│  + 时间戳 / session ID / model / provider                  │
└──────────────────────────────────────────────────────────┘
```

**关键改动**：

1. **指令文件从 user message 提升到 system**：
   - 当前：`messages[0]` 是 user role 的 `build_instructions_blocks()`
   - 改为：放在 system prompt 的 context 层
   - 代价：**失去 cache_control breakpoint**（Anthropic 的 cache_control 只能设在 messages 数组里）
   - 收益：语义更正确，指令文件的内容应该是 "system" 而非 "user"

2. **新增 `model_family_guidance()`**：
   ```rust
   fn model_family_guidance(model_id: &str) -> &'static str {
       if model_id.contains("gemini") { GEMINI_GUIDANCE }
       else if model_id.contains("gpt") { GPT_GUIDANCE }
       else if model_id.contains("claude") { "" }  // Claude 不需要额外指导
       else { "" }
   }
   ```

3. **拆分 system prompt 构建**：
   ```rust
   pub struct SystemPrompt {
       pub stable: String,    // 缓存友好
       pub context: String,   // session 级
       pub volatile: String,  // 每次请求
   }
   ```

**优点**：结构清晰、缓存友好、可扩展性强
**缺点**：改动较大，需要仔细处理 Anthropic cache_control 的迁移（可能需要在 messages 数组里保留一个轻量 breakpoint marker）

### 4.3 方案 C：可配置 + 默认（长期目标）

在方案 A 或 B 的基础上，增加：
- Settings UI 中的 "Custom System Prompt" 字段（追加到默认提示词后）
- Per-project 覆盖（`.everlasting/system.md` 类似 Hermes 的 `.hermes.md`）
- 模板变量支持（`{project_name}`, `{cwd}`, `{mode}` 等）

---

## 5. 推荐路径

### 第一阶段（立即）：方案 A — 轻量注入

1. 创建 `agent/behavior_prompt.rs`，写入 `DEFAULT_BEHAVIOR_PROMPT` 常量
2. 修改 `chat_loop.rs:327` 的拼接逻辑
3. 验证：与 LLM 对话，确认模型行为是否有可感知的改善

**预计改动量**：~50 行新增 + 3 行修改

### 第二阶段（后续）：方案 B — 3 层架构

1. 设计 stable/context/volatile 三层数据结构
2. 将指令文件从 user message 迁移到 system context 层
3. 处理 Anthropic cache_control 的迁移（在 messages 数组里保留 breakpoint）
4. 实现 `model_family_guidance()`
5. 前端 Settings 增加 custom system prompt 字段

**预计改动量**：~300 行新增 + ~50 行修改

### 不推荐

- **完全模仿某一家**：每家都是为自己的产品定制的，直接复制会引入无关内容
- **Aider 风格极简**：Everlasting 的目标是 Claude Code 同级能力，需要更丰富的行为指导
- **一次性大重构**：system prompt 的质量需要在实践中迭代，先出 MVP 再优化

---

## 6. 参考资料

| 来源 | 路径 / 仓库 |
|---|---|
| Claude Code v2.0 system prompt | `x1xhlol/system-prompts-and-models-of-ai-tools` / `Anthropic/Claude Code 2.0.txt` |
| OpenCode 多套提示词 | `anomalyco/opencode` / `packages/opencode/src/session/prompt/` |
| OpenCode prompt 选择逻辑 | `anomalyco/opencode` / `packages/opencode/src/session/system.ts` |
| Hermes Agent 3 层架构 | `NousResearch/Hermes-Agent` / `agent/system_prompt.py` |
| Hermes Agent 提示词常量 | `NousResearch/Hermes-Agent` / `agent/prompt_builder.py` |
| Aider edit block prompts | `Aider-AI/aider` / `aider/coders/editblock_prompts.py` |
| Aider base prompts | `Aider-AI/aider` / `aider/coders/base_prompts.py` |
| Everlasting 当前实现 | `app/src-tauri/src/agent/system_prompt.rs` |
| Everlasting 指令文件注入 | `app/src-tauri/src/memory/loader.rs` |
| Everlasting chat loop 拼接 | `app/src-tauri/src/agent/chat_loop.rs:254-327` |

---

## 7. 评审与修订（2026-06-19）

> 本节为对 §4–§5 方案的评审。结论一句话：**方案 A 该做（需 4 处修正）、方案 B 不整体做（只取 `model_family_guidance` 子项）、§1.3 的 TodoWrite 断言有事实错误**。

### 7.1 现状描述核实（逐行对过代码）

§1.1 / §1.2 的现状描述**全部准确**：`build_system_prompt()` 简陋、拼接在 `chat_loop.rs:327`、指令文件走 user message + banner `cache_control: ephemeral`（`loader.rs:325`）、4 种 mode（`permissions/mod.rs:1321`）。地基稳。

### 7.2 事实纠正：没有 TodoWrite tool（照抄方案 A 会出 bug）

- §1.3 表格写「有 TodoWrite tool，但没告诉模型要用」——**错误**。实际工具是 `update_checklist`（`tools/update_checklist.rs:80`）。
- 后果：方案 A 的 `DEFAULT_BEHAVIOR_PROMPT` 里 `Use the TodoWrite tool to plan and track complex tasks` 若照抄，模型会去调用不存在的 `todo_write` 工具而报错。
- 附带发现的现状问题：`build_system_prompt()` 的工具列表（read_file/write_file/edit_file/shell/grep/glob/list_dir）**漏列** `update_checklist` / `web_fetch` / `use_skill` / `background_shell`。工具清单不全这件事，比 prompt 文案更值得先修。

### 7.3 来源可信度

§2.1 Claude Code「~500 行」来自 `x1xhlol` 社区逆向仓库（二手、易过时）。作方向参考可以，勿当权威逐字依据。

### 7.4 方案 A：支持，4 个修正点

1. **工具名**：`TodoWrite` → `update_checklist`（见 7.2）。
2. **拼接顺序倒过来**：文档写 `mode_prefix + behavior_prompt + base_prompt`，但 `base_prompt` 含 session_id / cwd / head_sha（每次会变）。正确顺序是最稳定的在前：`behavior_prompt + mode_prefix + base_prompt`，利于未来上 cache。
3. **与 `mode_prefix` 明确分层**：mode 管「权限边界」（Plan 不能写 / Yolo 跳确认），behavior 管「做事风格」。两者都涉及「是否 commit」，须在 prompt 里明确边界避免重复。
4. **token 成本**：~200 行每轮发送，对 token-tight（C3 压缩 + MAX_TURNS 50）的项目要意识到，配合 cache_control 缓解。

### 7.5 方案 B：核心卖点站不住，拆开才有价值（与 §5 的主要分歧）

- **「3 层 struct = 缓存收益」是概念混淆。** 缓存收益只取决于 `cache_control` breakpoint 放哪，跟「代码分 3 个 String 字段」无关。Hermes 的 3 层是**代码组织**，不是缓存策略。
- **「指令文件移到 system 会失去 breakpoint」——前提错误。** Anthropic 的 `cache_control` 可放在 **system / tools / messages 三处**（共 4 个 breakpoint 上限），并非「只能设在 messages 数组里」。指令文件可移到 system 字段并保留缓存。
- **stable 复用你们已天然达成。** `system_prompt` 在 `chat_loop.rs:327`（loop 外）构建一次，turn 循环内每轮 `clone()`（line 696）——Hermes 的 "build once per session" 已经有了。
- **唯一真有价值的子项：`model_family_guidance()`。** GLM / GPT / Gemini 的 tool-use 坚持性确实需要差异化指导。**应拆出独立做，作为方案 A 增量**，不等整个 3 层重构。

**结论：方案 B 整体暂缓。** 除非做真·缓存重构（把稳定 system 内容纳入 breakpoint），否则 3 层 struct 是过度设计；指令文件留在 user message 不动（当前设计本就是已优化态）。

### 7.6 方案 C：合理，优先级最低

Custom system prompt / `.everlasting/system.md` / 模板变量方向对，但默认行为准则未稳定前加，只增加维护面。先调好默认，再开放覆盖。

### 7.7 文档遗漏的三点

1. **可测量性**：§5.1「与 LLM 对话确认改善」太主观。先定 4–6 场景 mini-eval（歧义请求 / 危险 git / 该并行没并行 / 客观性被打压），改前改后对比。
2. **prompt injection**：AGENTS.md 已是用户可控注入面，custom system prompt 扩大攻击面，文档通篇未提防护。
3. **语言**：默认 prompt 英文（行业惯例 OK），但中文项目应考虑显式约束「按用户语言回复」。

### 7.8 修订后的推荐路径（覆盖 §5）

| 优先级 | 动作 | 依据 |
|---|---|---|
| **P0 现在就修** | `build_system_prompt()` 补全工具清单（含 update_checklist） | 纯现状修复，不依赖任何方案 |
| **P1 做方案 A** | 加 `behavior_prompt.rs`，修正工具名 + 调换拼接顺序 | 务实、立竿见影 |
| **P2 拆 model_family_guidance** | 作为方案 A 增量 | 方案 B 唯一有价值子项 |
| **暂缓** | 方案 B 整体 | 概念混淆 + 已天然达成 + 反优化 |
| **先于一切** | 定 4–6 场景 mini-eval | 让 prompt 迭代可量化 |
