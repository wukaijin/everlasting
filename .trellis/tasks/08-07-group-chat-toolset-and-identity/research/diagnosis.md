# 诊断 — session 8be4687f 群聊三层问题前因后果

> **本文档是交接材料**：记录从"moderator 报告 group-chat-director skill 不存在"这一现象，
> 一步步深挖到三层独立但叠加的缺陷的完整因果链。供后续单开 session 处理**同模型串台**
> （第三层）时直接接续，不必重查 DB。
>
> 取证 DB：`/root/.local/share/dev.everlasting.app/everlasting.db`
> 取证 session：`8be4687f-3e2d-4023-9d58-91af70abae43`（2026-08-07 02:50）
> 原始证据快照：同目录 `db-evidence-8be4687f.md`
> 时间线：2026-08-07，08-07-group-chat-review-fixes 任务完成后复盘时发现。

---

## 0. 起点：用户的疑问

> "moderator 发现 \`我需要使用 group-chat-director skill,但是它不存在\`，确认一下"

初看像个小 bug（"系统说分配了技能但不存在"）。取证后发现是冰山一角。

## 1. session 配置（决定了所有后续行为）

```
moderator = D4F = 同一个 deepseek 模型（model_id b8d0abc2）  ← 关键
M3        = 另一个模型（model_id 42274366）
标题      ："简单聊聊这个项目的不足"（讨论 fortune-telling 项目）
```

- moderator 模型 = session 自身的 `model_id`（`group_chat.rs:128`，`build_group_chat_ctx`）。
- D4F 的 model 也配置成 `b8d0abc2` → **moderator 和 D4F 是同一个模型实例**。
- 这是 PRD D2（08-06 任务）定为"硬约束必须支持"的同模型组合场景。

## 2. 完整时序（17 条消息）

| seq | role | speaker(DB) | 实际生成者 | 内容/动作 |
|----|------|------------|----------|----------|
| 0 | user | · | 人类 | "简单聊聊这个项目的不足" |
| 1 | assistant | moderator | moderator(deepseek) | `list_dir`+`glob` 调研 |
| 3 | assistant | moderator | moderator | `read_file`+`list_dir`×2 调研 |
| 5 | assistant | moderator | moderator | `read_file`×2 调研 |
| 7 | assistant | moderator | moderator | 第一次 `nominate_speaker(M3)` |
| **9** | assistant | **M3** | **M3** | **`@moderator:`主持人语气 + `update_checklist`自建调度 + `use_skill("group-chat-director")`幻觉** |
| 11 | assistant | M3 | M3 | 自我纠正"我自己就是 M3,不是主持人" + 正式发言 |
| **12** | assistant | **moderator** | **deepseek（但 thinking 自称 M3）** | `nominate_speaker(D4F)`（thinking 全是角色纠结） |
| 13 | user | · | 系统 | tool_result "Floor handed to D4F" |
| **14** | assistant | **D4F** | **deepseek（thinking 又自称 M3）** | **`[生成出错中断]`（无 tool_use，纯生成崩溃）** |
| 15 | assistant | moderator | deepseek | `end_discussion` 强行收尾 |
| 16 | user | · | 系统 | tool_result |

（seq 2/4/6/8/10 是各 tool_use 的 tool_result 行，省略）

## 3. 三层独立缺陷

深挖后发现这不是"一个 bug"，而是**三个独立但叠加**的缺陷。必须分开治，混在一起会治错。

### 第一层 — `use_skill` 幻觉（"group-chat-director 不存在"的表层）

**现象**：seq 9 的 M3 调了 `use_skill({skill_name: "group-chat-director"})`。全仓搜 `group-chat-director` 零命中——该技能不存在。

**根因（代码已坐实）**：
- 群聊 speaker（moderator + participant）走 `system_prompt_override = Some(prompt)`（`group_chat_loop.rs:583` moderator、`:767` participant）。
- `run_chat_loop` 在 override 时**直接用传入的 prompt，跳过 `build_system_prompt` + `assemble_system_prompt`**（`chat_loop.rs:923-924`）。
- `<available-skills>` 清单是 `assemble_system_prompt` 那条路径注入的（`skill/loader.rs:799`）。
- **但工具集里仍含 `use_skill`**（`builtin_tools()` 全集，`tools/mod.rs:141`；群聊只剥两个仲裁工具，没剥 `use_skill`）。
- 结果：模型看到一个"加载技能"工具 + 一个"调它要参考 `<available-skills>` 块"的描述，**但 system prompt 里没有任何技能清单**。弱模型在认知压力下虚构了一个符合角色语义的技能名（"主持/导演" → `group-chat-director`）。

**和 `###TASK_COMPLETED###`（08-06 任务发现的）同源**：都是模型在角色认知受压时凭空虚构系统信号。

**归属任务**：`08-07-group-chat-toolset-and-identity`（工具集收敛，剥 `use_skill`）。

### 第二层 — M3 夺权（seq 9，参与者身份认知紊乱）

**现象**：seq 9 是 M3 这个模型（42274366）生成的，单条 assistant 消息里叠加四个动作：
1. `@moderator:` 开头（把自己当成跟主持人对话的元层）
2. 主持人第一人称"我上一轮没有调用工具"（adopting moderator 视角）
3. `update_checklist` 自建发言清单（M3发言/D4F发言/第二轮/结束）——**冒充主持人职能**
4. `use_skill("group-chat-director")`（给自己找合法性）

= M3 认为自己是来主持的导演。seq 11 它自己也察觉了（"抱歉,刚才搞错了——我自己就是 M3,不是主持人"），但错乱已污染 history。

**根因（叠加）**：
- moderator 连续 3 轮纯调研（seq 1/3/5）不 nominate → 编排器走"重试 moderator"路径，期间无人发言，讨论看似僵住。
- M3 被提名后看到 history 里"moderator 调研好几轮没让我说话"，推断"主持权空缺，我来补"。
- 同模型（moderator=D4F=deepseek）让 history 里"deepseek 的话"同时挂在 moderator 和 D4F 名下，边界更糊。
- identity-guard prompt（`group_chat_loop.rs:participant_system_prompt`）说"never act as moderator（no nominating speakers）"，但**没明确禁止 `update_checklist` 这类自建调度**，留了灰色地带。

**注意**：这层 M3 是**另一个模型**（42274366），它主动接管。和第三层的"同一个模型串台"是两回事。

**归属任务**：`08-07-group-chat-toolset-and-identity`（工具收敛剥 `update_checklist` + prompt 加固"禁止夺权/自建流程"）。

### 第三层 — 同模型串台（seq 12/14，D4F 生成中断）★ 单列任务

**现象**：seq 14（DB 标 speaker=D4F）只有 thinking + `[生成出错中断]` 文本，**无 tool_use**。

**纠正一个误判**：之前曾随口说"D4F 出错是孤儿 tool_use 导致 400"——**错了**。孤儿/400 会有 tool_use 块；seq 14 没有。真相见下。

**根因（DB thinking 字段坐实）**：
- seq 12（moderator turn）和 seq 14（D4F turn）的 thinking **开头都是"我是 M3,参与者之一"**——但 DB 分别标 `speaker=moderator` 和 `speaker=D4F`。
- 这两条都是**同一个 deepseek 模型**生成的（moderator = D4F = `b8d0abc2`）。
- 该模型在 seq 9 看到 M3 发言后，自己的角色认知也崩了。**崩溃状态在它的两个角色 turn 之间串台**：
  - seq 12 它以 moderator 身份被调用，thinking 自称 M3，纠结一番后还是 `nominate_speaker(D4F)`。
  - seq 14 它以 D4F 身份被调用，thinking 又是"等等,我又犯了同样的错误——我是 M3...不应该提名 D4F"，反复自我怀疑 → **生成中断**（thinking 极长，耗到 max_tokens 或触发截断）。
- 对模型来说，seq 12 和 seq 14 的 history 是连续的——**它分不清"现在我该是 moderator 还是 D4F"**。

**为什么 `[生成出错中断]`**：模型在 thinking 里反复纠结角色，无法收敛到一个稳定的输出生成，最终被截断/中断。不是 API 错误、不是孤儿、不是 400。

**本质**：同模型组合（PRD D2 硬约束）的结构性后果——**同一个模型扮演多个角色时，会在自己的 turn 之间记忆串台**。这不是 prompt 能完全解决的：prompt 在，但模型在自己的连续 history 压力下不认（seq 12/14 的 thinking 明确引用了 system prompt 的角色定义，说明它读了，只是输入太矛盾）。

**归属任务**：**单列 `08-07-group-chat-same-model-crossover`**。`08-07-group-chat-toolset-and-identity` 的工具收敛 + prompt 加固对这层**效果有限**——它治的是 seq 9 的 M3 夺权（另一个模型主动接管），不是 seq 12/14 的同模型串台（同一模型记忆混乱）。

## 4. 三层的关系与归属

```
session 8be4687f
├─ 第一层 use_skill 幻觉 ─────────→ 08-07-group-chat-toolset-and-identity（工具收敛）
├─ 第二层 M3 夺权(seq9) ──────────→ 08-07-group-chat-toolset-and-identity（工具收敛 + prompt）
└─ 第三层 同模型串台(seq12/14) ───→ 08-07-group-chat-same-model-crossover（单列，更深）
```

- 第一层和第二层**叠加**：幻觉技能是 M3 夺权的"合法性道具"。工具收敛同时治两者。
- 第二层和第三层**症状相似（都是身份紊乱）但根因不同**：
  - 第二层：M3 是**另一个模型**，主动接管（prompt + 工具能治）。
  - 第三层：moderator/D4F 是**同一个模型**，记忆串台（prompt 扛不住，需更深设计）。

## 5. 为什么第三层要单列（给后续 session 的判断依据）

1. **方案空间不同**：第一/二层靠"工具集收敛 + prompt 加固"能治（已规划）。第三层需要动到 wire 层角色锚定 / 同模型组合约束 / turn 间隔离等更深的设计——和工具收敛是两套思路，混在一个任务里会互相牵制。
2. **验证条件不同**：第一/二层的修复效果可以靠"工具收敛后 history 更干净、诱因更少"在现有 mock 测试里部分验证。第三层的修复效果**必须靠真模型重跑同模型组合 session** 才能验证（mock 无法复现模型的记忆串台）。
3. **优先级**：第三层是同模型组合（D2 硬约束）下才会触发的 corner case，第一/二层是所有群聊都会触发的普遍问题。先把普遍问题修了（当前任务），再啃 corner case（后续 session）。

## 6. 给后续 session（同模型串台）的交接要点

- **起点**：读本文档 §3 第三层 + `db-evidence-8be4687f.md` §补充证据。
- **取证 DB/session**：见本文档顶部。
- **不要重新调研的**：根因已坐实（同模型、thinking 自称 M3、生成中断非 400）。直接进方案空间。
- **方案空间（待 brainstorm）**：
  1. wire 层角色锚定：同模型 turn 之间在 wire 请求里强化"当前你是 X"的提示（OpenAI name 字段 / Anthropic @name 前缀之外再加？）。
  2. 同模型组合约束：是否在 UI/配置层限制 moderator 不得与任何 participant 同模型（但这违反 D2 硬约束，需重新和用户确认）。
  3. turn 间隔离：同模型的不同角色 turn 是否需要某种状态重置（而非共享连续 history）。
  4. 其他。
- **约束**：D2（08-06）定同模型组合为硬约束；若方案 2 要改它，必须和用户重新拍板。
- **前置依赖**：`08-07-group-chat-toolset-and-identity` 落地后再做（工具收敛让 history 更干净，便于单独观察同模型串台的效果，排除第一/二层干扰）。

## 7. 涉及的代码位置（备查）

| 关注点 | 位置 |
|---|---|
| moderator/participant system_prompt 全替换 | `group_chat_loop.rs:583`(mod)、`:767`(par)；消费于 `chat_loop.rs:923-924` |
| `<available-skills>` 注入（override 路径跳过它） | `skill/loader.rs:799`（注入）；`chat_loop.rs:923-924`（override 跳过） |
| 工具集（含 use_skill/update_checklist） | `tools/mod.rs:131` `builtin_tools()`；群聊过滤 `group_chat_loop.rs:341` `participant_tool_defs` |
| 同模型组合来源 | `group_chat.rs:128`（moderator = session model_id）；participant 各自 model |
| MAX_NO_NOMINATE_STREAK / moderator_stuck（08-07 引入，工具收敛任务会清理） | `group_chat_loop.rs:103` 常量 + streak 路径 |
| identity-guard prompt | `group_chat_loop.rs:participant_system_prompt` / `moderator_system_prompt` |
