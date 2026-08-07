# Design — 群聊工具集收敛与身份认知加固

> PRD: `prd.md`。诊断与 DB 证据: `research/diagnosis.md` + `research/db-evidence-8be4687f.md`。
> 本设计治第一层（`use_skill` 幻觉）+ 第二层（M3 夺权）。第三层（同模型串台）单列。

## 全局约束（不改）

- 串行时序（`for round { moderator.await → participant.await }`）不变。
- 5 层身份防御（wire speaker 标注 / `participant_view` 转录隔离 / 工具隔离 / identity-guard
  prompt / moderator 单轮）不推翻，只补强工具层 + prompt 层。
- D-D 入口护栏（`chat_loop.rs:987-1007`）/ wire 孤儿自愈（`wire.rs:240-305`）不动。
- `MAX_ORCHESTRATION_ROUNDS=30` 不变（唯一外层硬上限 / 烧钱兜底）。
- `max_turns`：moderator 保持 `Some(1)`、participant 保持 `Some(20)`（08-07 R3，不变）。

---

## R1 — 收敛群聊工具集

### 现状
- moderator 拿 `tool_defs.clone()` = `builtin_tools()` **全集**（23 个工具）。
- participant 拿 `participant_tool_defs(&tool_defs)` = 全集减 2 个仲裁工具。
- 问题：moderator 能用 `update_checklist` 自建调度（seq 12 前的失范）；participant 能用
  `use_skill`（无 `<available-skills>` 清单 → 幻觉）+ `update_checklist`（自建调度）。

### 设计：白名单取代黑名单
现状是**黑名单**（"剥掉仲裁工具"）。改成**白名单**（"只留群聊需要的"），更严格也更稳：
未来 `builtin_tools` 新增工具默认不进群聊，避免再次出现"群聊误用新工具"。

新增 `group_chat_tool_defs(tool_defs, is_moderator)`（取代 `participant_tool_defs`）：

```rust
/// 群聊调研类工具白名单（moderator + participant 共享）。只读 + 取材，
/// 无写/执行/交互/调度类——群聊讨论只读代码库，不改它，也不自建流程。
const GROUP_CHAT_RESEARCH_TOOLS: &[&str] = &[
    "read_file", "grep", "glob", "list_dir", "web_fetch",
];

/// moderator 在调研白名单之上额外拥有仲裁类工具。
const MODERATOR_EXTRA_TOOLS: &[&str] = &[
    NOMINATE_SPEAKER_TOOL_NAME, END_DISCUSSION_TOOL_NAME,
];

fn group_chat_tool_defs(tool_defs: &[ToolDef], is_moderator: bool) -> Vec<ToolDef> {
    let allow: Vec<&str> = if is_moderator {
        GROUP_CHAT_RESEARCH_TOOLS.iter().chain(MODERATOR_EXTRA_TOOLS.iter()).copied().collect()
    } else {
        GROUP_CHAT_RESEARCH_TOOLS.to_vec()
    };
    tool_defs.iter()
        .filter(|t| allow.contains(&t.name.as_str()))
        .cloned()
        .collect()
}
```

### 调用点改动
- moderator `run_chat_loop` 调用（`group_chat_loop.rs:550` 附近）：`tool_defs.clone()` →
  `group_chat_tool_defs(&tool_defs, true)`。
- participant `run_chat_loop` 调用（`:761`）：`participant_tool_defs(&tool_defs)` →
  `group_chat_tool_defs(&tool_defs, false)`。
- 删除 `participant_tool_defs` 函数（:341）+ 它的测试 `participant_tool_defs_strips_arbitration_tools`（:914）。
  被新的 `group_chat_tool_defs` 测试取代。

### 关键不变量
- **白名单是穷尽的**：新增 builtin_tools 不会自动进群聊（必须显式加进 `GROUP_CHAT_RESEARCH_TOOLS`
  或 `MODERATOR_EXTRA_TOOLS`）。这是对 seq 9/12 失范的根因修复——`update_checklist`/`use_skill`
  当初正是因为黑名单没拦住才泄漏进群聊。
- **moderator 仍能调研**（D1）：`read_file`/`grep`/`glob`/`list_dir`/`web_fetch` 都在白名单。
  seq 1/3/5 的合理调研不受影响。
- **participant 仍能取材**（D2）：同样的调研白名单。R3 的 max_turns=20 取材闭环不变。
- **仲裁工具隔离不变**：participant 拿不到 `nominate_speaker`/`end_discussion`
  （白名单不含），比旧黑名单更稳。

### 测试（取代旧 `participant_tool_defs_strips_arbitration_tools`）
- `group_chat_tool_defs_moderator_has_research_plus_arbitration`：moderator 集合 = 调研类
  + nominate + end；且**不含** `use_skill`/`update_checklist`/`shell`/`write_file`。
- `group_chat_tool_defs_participant_has_research_only`：participant 集合 = 调研类；
  **不含** nominate/end（仲裁隔离）+ 不含 `use_skill`/`update_checklist`/`shell`/`write_file`。
- 两个断言都要 sanity-check 白名单工具确实在 `builtin_tools()` 里（否则 filter 是空操作）。

---

## R2 — 去掉 MAX_NO_NOMINATE_STREAK 计数限制

### 现状
moderator 不提名时：`no_nominate_streak += 1`，超过 `MAX_NO_NOMINATE_STREAK=3` 就
`halt_reason = ModeratorStuck; break`。08-07 又为它加了 `moderator_nudge`（streak>0 时
追加到 system prompt 强制工具调用）+ `STOP_REASON_MODERATOR_STUCK`。

问题（PRD R2）：无法区分"moderator 在认真调研"（seq 1/3/5，合理）和"卡住"。streak=3
会误杀合理调研。

### 设计：删 streak，不提名就继续下一轮
- moderator turn 结束、`next_speaker == None && !ended` 时：**不再计数、不再 break**，
  直接 `continue`（回到 `for round` 顶部，reload 历史 + 重跑 moderator turn）。
- 兜底：`MAX_ORCHESTRATION_ROUNDS=30` 仍在——moderator 连续 29 轮不提名，第 30 轮落出
  循环 → `halt_reason = MaxRounds` → 终态 `Done { stop_reason: "max_rounds" }`（R2 保留）。

### 删除清单（08-07 引入的 streak 一族，全部清理）
后端 `group_chat_loop.rs`：
- `const MAX_NO_NOMINATE_STREAK`（:103）
- `STOP_REASON_MODERATOR_STUCK`（:125）
- `HaltReason::ModeratorStuck`（:140）+ match 臂（:865）
- `moderator_nudge` 函数（:182）
- `no_nominate_streak` 变量（:495）+ 所有引用（:540 prompt 拼接、:622 重置、:642-650 计数/break）
- streak 路径的 `halt_reason = Some(HaltReason::ModeratorStuck)` —— 不提名路径不再设 halt
- 测试：`moderator_nudge_forces_tool_call`（:1004）、`max_no_nominate_streak_is_sane`（:1023）、
  `orchestrator_emits_terminal_done_when_moderator_stuck`（`tests_group_chat.rs`，08-07 加的）

前端：
- `streamController.ts`：`groupChatNotice` 的 `moderator_stuck` case（:855）、
  finalize 白名单的 `moderator_stuck`（:1448）、注释引用（:1435）。
- `chat.types.ts`：`notice` 字段注释里的 `moderator_stuck`（:243）。
- `streamController.test.ts`：`08-07 R2: 终端 moderator_stuck` 测试（:1981）+
  `groupChatNotice("moderator_stuck")` 断言（:2121）。

### 关键不变量
- **finalize 触发集**：去掉 `moderator_stuck` 后，群聊终态 finalize 的 stop_reason =
  `{ group_chat_end, cancelled, max_rounds }`。非终态（不 finalize）= `{ nominee_unknown,
  participant_unresolved }`（R2 保留）。前端白名单同步收紧。
- **`max_rounds` 是唯一的非正常终态**：讨论非正常结束只剩"撞 30 轮上限"一种。语义更干净。
- **moderator 不再被"强制工具调用"**：去掉 `moderator_nudge` 后，moderator 的 system prompt
  稳定（不随轮次变化）。节奏引导改靠 R3 的 prompt 常驻说明（见下）。

### 保留并调整
- `HaltReason` 枚举保留（只剩 `DiscussionEnded` + `MaxRounds`）——post-loop 终态 Done 仍需它
  决定 stop_reason。
- `nominee_unknown` / `participant_unresolved` 两条 continue 路径**完全保留**（R2，不变）——
  它们是单轮可恢复的，和 streak 无关。

---

## R3 — prompt 加固身份边界

### moderator prompt（`moderator_system_prompt`）
现状已说"Facilitate / nominate / end"。补一段**节奏 + 边界**说明（常驻，不随轮次变）：

> Researching the codebase (read_file / grep / etc.) to ground the discussion is good —
> but research is a means, not the goal. After a brief investigation, hand the floor to a
> participant with `nominate_speaker`. Do NOT use other tools (checklists, skills, etc.) to
> build your own speaker-rotation flow — `nominate_speaker` is the only scheduling mechanism.

针对 seq 1/3/5 的"调研过头" + seq 12 前的"想用 update_checklist 自建流程"。

### participant prompt（`participant_system_prompt` 的 identity-guard 块）
现状已说"never act as moderator (no nominating speakers, no opening/closing)"。补**禁止夺权**
的具体禁令（针对 seq 9 的三种失范）：

> Even if the discussion seems stalled, or the moderator takes a long time to nominate,
> you must NOT take over the moderator's job. Specifically: do not build your own
> speaker-rotation checklist, do not address the room as the moderator (no "let's hear from
> X next"), and do not invent system tools or skills to legitimize hosting. You speak only
> when nominated, and only your own view.

针对 seq 9 的 `@moderator:` 第一人称 + `update_checklist` 自建清单 + `use_skill` 幻觉三种模式。

### 关键不变量
- **prompt 是常驻的**（不随轮次/streak 变）——和去 streak（R2）一致：节奏靠常驻引导，不靠
  动态 nudge。
- **不展示 `@`-前缀自指示例**（08-06 既有不变量，保留）——新增禁令用描述性语言，不给"错误示范"。
- **不针对同模型串台**（第三层）——prompt 扛不住那层，不在本任务范围。

### 测试
- `moderator_prompt_guides_research_to_nominate`：moderator prompt 含"research is a means"/
  "hand the floor"/"do not build your own flow"。
- `participant_prompt_forbids_takeover`：participant prompt 含"must NOT take over"/
  "do not build your own speaker-rotation"/"do not invent system tools or skills"。
- 既有 `participant_prompt_forbids_self_label_but_allows_mentions` / `moderator_prompt_forbids_self_label`
  保留（08-06/08-07 既有，不冲突）。

---

## R4 — 归档 + spec 沉淀

- `research/diagnosis.md` + `research/db-evidence-8be4687f.md` 已就位。
- 完成后更新 `.trellis/spec/backend/agent-loop-architecture.md` §"Group-chat transcript view"，
  补两条新不变量：
  1. **工具集白名单**（R1）：群聊 speaker 只拿调研类（+ moderator 仲裁类），非白名单工具
     不进群聊。`group_chat_tool_defs` 取代旧的 `participant_tool_defs` 黑名单。
  2. **无 streak 计数**（R2）：moderator 不提名→继续下一轮，靠 `MAX_ORCHESTRATION_ROUNDS`
     兜底。`moderator_stuck`/`moderator_nudge`/`MAX_NO_NOMINATE_STREAK` 已移除。

---

## 风险与回滚

- **R1 白名单遗漏**：若白名单漏了某个群聊需要的工具（如未来加的），moderator/participant
  会"看不到"该工具。缓解：白名单是显式常量，新增工具时 grep `GROUP_CHAT_RESEARCH_TOOLS`
  即知要不要加。回滚 = 改回黑名单。
- **R2 去 streak 后烧钱**：理论上坏 moderator 可连续 29 轮不提名。缓解：工具收敛（R1）+
  prompt（R3）后正常 moderator 不会瞎调研；`MAX_ORCHESTRATION_ROUNDS=30` 最坏有界。这是
  PRD 已接受的权衡。
- **R3 prompt 过度约束**：新增禁令可能让 moderator 过于保守（不敢调研）。缓解：moderator
  prompt 同时强调"researching is good"，不是禁止调研，是禁止"只调研不提名"。
- **跨任务影响**：本任务清理 08-07 刚加的 streak 路径。08-07 其余 4 个修复（R1 身份契约 /
  R3 max_turns / R4 删 order / R5 view 不变量）不受影响。**注意**：08-07 的 R1 身份契约测试
  里有一个 `identity_contract_prompts_separate_roles_under_same_model` 断言 participant prompt
  含"The moderator's messages are NOT yours"——R3 新增禁令后该断言仍成立（新增是叠加，不替换）。
