# 群聊工具集收敛与身份认知加固

## Goal

基于 session `8be4687f`（08-07 02:50）的 DB 取证，修复群聊两个**叠加且独立**的缺陷：
(A) moderator 拿到 `builtin_tools` 全集后用 `update_checklist` 自建调度流程、长时间不 nominate；
(B) 参与者 M3 身份认知紊乱——在被正确提名后，用 `@moderator:` 主持人语气发言、自建发言清单、
调用幻觉技能 `use_skill("group-chat-director")`。

修复手段：**收敛群聊工具集**（剥掉群聊无关/危险工具，只留调研类 + 仲裁类）+
**去掉 `MAX_NO_NOMINATE_STREAK` 计数限制**（改为纯 prompt 引导 + 外层硬上限兜底）+
**prompt 加固身份边界**。

核心代码：`app/src-tauri/src/agent/group_chat_loop.rs`（编排器 + prompt + 工具过滤）、
`app/src-tauri/src/tools/mod.rs`（`builtin_tools` 清单）。

> **背景与三层问题完整诊断**：见 `research/diagnosis.md`（从"group-chat-director skill
> 不存在"现象 → 深挖到三层独立缺陷的完整因果链 + 代码位置备查）。本任务治第一/二层
> （`use_skill` 幻觉 + M3 夺权），第三层（同模型串台，seq 12/14）单列后续 session。
> 原始 DB 证据快照：`research/db-evidence-8be4687f.md`。

## Background / 事实（DB 取证）

session `8be4687f`，配置：moderator = D4F = deepseek（`b8d0abc2`，**同模型**）；M3 = `42274366`。
17 条消息的关键时序：

- seq 1/3/5：moderator 连续 3 轮纯调研（`list_dir`/`glob`/`read_file`），0 次 nominate。
- seq 7：moderator 第一次 `nominate_speaker(M3)`。
- **seq 9（speaker=M3，身份紊乱 + skill 幻觉的关键行）**：M3 生成 `@moderator: 提醒一下——
  我上一轮没有调用工具,现在请 M3 先发言`（主持人第一人称），同一条里调了
  `update_checklist`（自建发言清单：M3发言/D4F发言/第二轮/结束）+ `use_skill("group-chat-director")`
  （幻觉技能）。**这四个动作叠在一起 = M3 认为自己是来主持的导演。**
- seq 11：M3 自我纠正（`"抱歉,刚才搞错了——我自己就是 M3,不是主持人"`）——模型察觉到错乱，
  但发生在已污染 history 之后。
- seq 12：moderator 终于 `nominate_speaker(D4F)` → seq 14 D4F `[生成出错中断]`（孤儿 tool_use
  导致下一轮 400）。

### 工具集事实

- moderator 拿 `builtin_tools()` **全集**（23 个工具），含 `use_skill`/`update_checklist`/
  `shell`/`write_file` 等。参与者拿全集减两个仲裁工具（`participant_tool_defs`）。
- 全仓搜 `group-chat-director` / `group_chat_director` **零命中**——该技能不存在，是模型在
  "看到 `use_skill` 工具但 system prompt 无 `<available-skills>` 清单"压力下的幻觉
  （`system_prompt_override = Some` 全替换 prompt，没复制 `<available-skills>` 块）。

### 两层限制事实

- `MAX_ORCHESTRATION_ROUNDS=30`（外层硬上限，防烧钱兜底）——独立于判 stuck，**保留**。
- `MAX_NO_NOMINATE_STREAK=3`（内层，连续 3 轮不 nominate 判 stuck 停讨论）——**无法区分**
  "moderator 在认真调研"（seq 1/3/5，合理）和"moderator 卡住"。抬高数值只推迟问题。
- 08-07 任务（`08-07-group-chat-review-fixes`）刚为 streak 路径引入了 `HaltReason::ModeratorStuck` /
  `STOP_REASON_MODERATOR_STUCK` / `moderator_nudge` —— 本任务去掉 streak 后**一并清理**这些。

## Requirements

### R1 — 收敛群聊工具集（治问题 A 的 update_checklist 自建 + 问题 B 的 use_skill 幻觉）
新增 `group_chat_tool_defs`，按角色过滤：

- **调研类（moderator + participant 共享）**：`read_file` / `grep` / `glob` / `list_dir` /
  `web_fetch`。
- **moderator 专有仲裁**：`nominate_speaker` / `end_discussion`（叠加在调研类上）。
- **剥掉**：`use_skill`（幻觉诱因）、`update_checklist`（自建调度诱因）、`shell` /
  `write_file` / `edit_file` / `run_background_shell` / `shell_status` / `shell_kill`
  （写/执行类，群聊不需要 + 危险）、`merge_worker` / `discard_worker`（worker 专用）、
  `remember` / `ask_user_question` / `use_ui` / `request_mode_change` /
  `request_task_state_transition` / `create_task`（其他交互类）。

落地：moderator 用 `group_chat_tool_defs(tool_defs, /* is_moderator */ true)`，
participant 用 `group_chat_tool_defs(tool_defs, /* is_moderator */ false)`。现有
`participant_tool_defs` 被 `group_chat_tool_defs` 取代（更严格——从"只剥仲裁"升级到
"只留调研白名单"）。**这修订了 08-07 任务的 D3 决策**（当时拍"沿用全工具集"是
在没看到这条 DB 证据时的判断；收敛后参与者有调研能力但无写/执行能力，边界更干净）。

### R2 — 去掉 MAX_NO_NOMINATE_STREAK 计数限制（治问题 A 的节奏误杀）
删除 `MAX_NO_NOMINATE_STREAK` 常量、`no_nominate_streak` 计数、`moderator_nudge`、
`HaltReason::ModeratorStuck`、`STOP_REASON_MODERATOR_STUCK`，以及 08-07 引入的对应
`moderator_stuck` 测试。moderator 不提名时**继续下一轮**（reload 历史 + 重跑 moderator
turn），直到 `nominate_speaker` / `end_discussion` 或撞 `MAX_ORCHESTRATION_ROUNDS`
（R2 的 `max_rounds` stop_reason 保留，作为唯一的非正常终态）。

防烧钱兜底：`MAX_ORCHESTRATION_ROUNDS=30` 不变（最坏情况有界）。前端 `streamController.ts`
的 `moderator_stuck` finalize 白名单条目随之删除（`max_rounds` 保留）。

### R3 — prompt 加固身份边界（治问题 B 的身份紊乱）
- **moderator prompt**：明确"调研是手段、`nominate_speaker`/`end_discussion` 是目的；
  调研几轮后必须把话筒交给参与者；不要用其他工具自建发言流程"。
- **participant prompt（identity-guard 强化）**：新增明确禁令——"即使讨论停滞、即使你
  看到主持人长时间不提名，你也不能接管主持、不能自建发言清单、不能冒充主持人第一人称；
  你只在被 `nominate_speaker` 提名后发言一次"。针对 seq 9 的 `@moderator:` + 第一人称
  + 自建 checklist 三种具体失范模式。

### R4 — DB 证据归档 + spec 沉淀
`research/db-evidence-8be4687f.md` 已归档（seq 全表 + seq 9 完整 content）。本任务完成后
更新 `agent-loop-architecture.md` §"Group-chat transcript view"，补充"工具集收敛 +
去掉 streak"两条新不变量。

## 已定决策

- **D1（用户拍板）**：moderator 允许调研，但靠 prompt 引导节奏，**不用计数/MAX_TURNS 硬限制**
  （外层 `MAX_ORCHESTRATION_ROUNDS=30` 兜底）。
- **D2（用户拍板）**：参与者保留调研类工具（R3 max_turns=20 的"取材实证"初衷不变），
  但剥掉写/执行/交互类。
- **D3（本任务修订 08-07 的 D3）**：参与者工具集从"沿用全工具集"收敛到"调研白名单"。
- 串行时序不变；5 层身份防御不推翻，只补强 prompt 层；D-D 入口护栏 / wire 孤儿自愈不动。

## Out of Scope

- 真模型端到端验证（仍人工；08-07 R1 契约测试 + 本任务 R3 prompt 测试守结构）。
- **同模型串台（单列任务 `08-07-group-chat-same-model-crossover`）**：session `8be4687f`
  seq 12/14 取证发现——当 moderator 和某 participant 是同一个模型（本例 moderator = D4F
  = deepseek `b8d0abc2`），该模型在自己的 moderator turn（seq 12）和 participant turn
  （seq 14）之间**记忆串台**：两个 turn 的 thinking 都是 "我是 M3" 第一人称，模型在
  seq 14 反复自我怀疑角色后**生成中断**（`[生成出错中断]`，非孤儿/400——seq 14 content
  无 tool_use）。这是同模型组合的结构性后果，prompt 层扛不住。本任务（工具收敛 + 去 streak
  + prompt 加固）对 seq 9 的 M3 夺权有效，但对 seq 12/14 的同模型串台**效果有限**。
  证据见 `research/db-evidence-8be4687f.md` §补充证据。
- `MAX_ORCHESTRATION_ROUNDS=30` 调值（保持现状）。
- 人类抢占插话改 live 介入（不变）。

## Acceptance Criteria

- [ ] AC1（R1）：`group_chat_tool_defs` 按角色过滤；moderator = 调研类 + 仲裁；
      participant = 调研类。`use_skill`/`update_checklist`/`shell`/`write_file` 等对两者
      均不可见。单测覆盖（白名单 + moderator/participant 差异）。
- [ ] AC2（R2）：`MAX_NO_NOMINATE_STREAK` / `moderator_nudge` / `HaltReason::ModeratorStuck` /
      `STOP_REASON_MODERATOR_STUCK` 删除；moderator 不提名时继续下一轮；`MAX_ORCHESTRATION_ROUNDS`
      仍生效（`max_rounds` stop_reason 保留）。前端 `moderator_stuck` finalize 条目删除。
- [ ] AC3（R3）：moderator prompt 含"调研→nominate 节奏"引导；participant prompt 含
      "禁止夺权/自建流程/冒充主持人"禁令；prompt 回归测试覆盖。
- [ ] AC4：既有群聊测试（08-07 的 + 08-04/08-06 的）调整后全绿；`cargo test --lib`
      + `pnpm test` + clippy 零警告 + vue-tsc 零错误。
- [ ] AC5：08-07 引入的 `moderator_stuck` 测试（`orchestrator_emits_terminal_done_when_moderator_stuck`）
      删除或改写（streak 机制不存在了）。

## Notes

- 复杂任务，`design.md` + `implement.md` 在 `task.py start` 前补齐。
- 跨任务影响：本任务清理 08-07 刚加的 streak 路径（`moderator_stuck` 一族）。
  08-07 的其余 4 个修复（R1 身份契约测试 / R3 参与者 max_turns / R4 删 order /
  R5 view 不变量）**不受影响**，保留。
- 验证命令：后端 `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
  （多线程默认）；前端 `cd app && pnpm test`。
