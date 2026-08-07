# Implement — 群聊工具集收敛与身份认知加固

> 执行顺序：R1（工具收敛，核心 + 独立）→ R2（去 streak，依赖 R1 的工具集已稳）→
> R3（prompt 加固，叠加在 R1/R2 之上）→ R4（spec 沉淀）。每步后跑相关子集，全部后跑全量。

## 环境提示

- 后端（WSL 必须 PKG_CONFIG_PATH）：
  `cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`
  （多线程默认，**勿加** `--test-threads=1`）。冷编 ~1m37s，增量 ~11s。
- 前端：`cd app && pnpm test`；类型 `cd app && npx vue-tsc --noEmit`。
- scope 冒烟（一次调用）：`cargo test --manifest-path ... --lib "group_chat"`。

## Step 1 — R1 工具集收敛（白名单取代黑名单）

- [ ] `group_chat_loop.rs`：新增 `GROUP_CHAT_RESEARCH_TOOLS` + `MODERATOR_EXTRA_TOOLS`
      常量 + `group_chat_tool_defs(tool_defs, is_moderator)` 函数（见 design §R1 代码）。
- [ ] moderator `run_chat_loop` 调用：`tool_defs.clone()` → `group_chat_tool_defs(&tool_defs, true)`。
- [ ] participant `run_chat_loop` 调用：`participant_tool_defs(&tool_defs)` →
      `group_chat_tool_defs(&tool_defs, false)`。
- [ ] 删除 `participant_tool_defs` 函数 + 测试 `participant_tool_defs_strips_arbitration_tools`。
- [ ] 新增测试 `group_chat_tool_defs_moderator_has_research_plus_arbitration`：
      moderator 集合含 read_file/grep/glob/list_dir/web_fetch + nominate + end；
      **不含** use_skill/update_checklist/shell/write_file/edit_file/run_background_shell/
      shell_status/shell_kill/merge_worker/discard_worker/remember/ask_user_question/use_ui/
      request_mode_change/request_task_state_transition/create_task。sanity-check 白名单工具
      在 builtin_tools() 里。
- [ ] 新增测试 `group_chat_tool_defs_participant_has_research_only`：participant 集合 =
      调研类；**不含** nominate/end（仲裁隔离）+ 同上不含写/执行/交互类。
- [ ] 测试：`cargo test --manifest-path ... --lib "group_chat_tool_defs"`。

## Step 2 — R2 去掉 streak 计数（清理 08-07 引入的 streak 一族）

后端 `group_chat_loop.rs`：
- [ ] 删 `const MAX_NO_NOMINATE_STREAK`（:103）。
- [ ] 删 `STOP_REASON_MODERATOR_STUCK`（:125）+ 其在 stop_reason doc 注释里的条目（:115-116）。
- [ ] 删 `HaltReason::ModeratorStuck`（:140）+ match 臂（:865）。
- [ ] 删 `moderator_nudge` 函数（:182）。
- [ ] 删 `no_nominate_streak` 变量（:495）+ 所有引用：
      - :540-547 prompt 拼接（`if no_nominate_streak > 0 { base + nudge }` → 直接用 base）。
      - :622 `no_nominate_streak = 0`（提名重置，不再需要）。
      - :642-650 计数 + break 路径 → 改成**直接 continue**（不计数、不 break、不设 halt_reason）。
        即 `None => { tracing::warn!(...); continue; }`。
- [ ] 更新 streak 相关注释（:88-103 的常量 doc、:181 moderator_nudge 的 doc 引用、
      :492-499 的 no_nominate_streak 注释、:641 的"After MAX... give up"注释）。
- [ ] 删测试 `moderator_nudge_forces_tool_call`（:1004）、`max_no_nominate_streak_is_sane`（:1023）。
- [ ] `tests_group_chat.rs`：删 08-07 加的 `orchestrator_emits_terminal_done_when_moderator_stuck`。

前端：
- [ ] `streamController.ts`：`groupChatNotice` 删 `case "moderator_stuck"`（:855）。
- [ ] `streamController.ts`：finalize 白名单删 `event.stop_reason === "moderator_stuck"`（:1448）+
      注释更新（:1435）。
- [ ] `chat.types.ts`：`notice` 字段注释删 `moderator_stuck`（:243）。
- [ ] `streamController.test.ts`：删 `08-07 R2: 终端 moderator_stuck` 测试（:1981）+
      `groupChatNotice("moderator_stuck")` 断言（:2121，改 `groupChatNotice` 测试块的 case 列表）。
- [ ] 验证 finalize 触发集 = `{ group_chat_end, cancelled, max_rounds }`；非终态 =
      `{ nominee_unknown, participant_unresolved }`。
- [ ] 测试：`cargo test --manifest-path ... --lib "group_chat"` + `pnpm test streamController`。

## Step 3 — R3 prompt 加固身份边界

- [ ] `moderator_system_prompt`（`group_chat_loop.rs`）：补"research is a means / hand the floor /
      do not build your own flow"段（见 design §R3 文案）。
- [ ] `participant_system_prompt` 的 identity-guard 块：补"must NOT take over / do not build
      your own speaker-rotation / do not invent system tools or skills"段（见 design §R3 文案）。
- [ ] 新增测试 `moderator_prompt_guides_research_to_nominate`：assert moderator prompt 含
      "research is a means" + "hand the floor" + "do not build your own"。
- [ ] 新增测试 `participant_prompt_forbids_takeover`：assert participant prompt 含
      "must NOT take over" + "do not build your own speaker-rotation" +
      "do not invent system tools or skills"。
- [ ] 验证既有测试 `participant_prompt_forbids_self_label_but_allows_mentions` /
      `moderator_prompt_forbids_self_label`（08-06/08-07）仍绿（R3 是叠加，不替换）。
- [ ] 验证 08-07 的 `identity_contract_prompts_separate_roles_under_same_model` 仍绿
      （它断言 participant prompt 含 "The moderator's messages are NOT yours"，R3 叠加后仍成立）。
- [ ] 测试：`cargo test --manifest-path ... --lib "group_chat"`。

## Step 4 — R4 spec 沉淀

- [ ] `.trellis/spec/backend/agent-loop-architecture.md` §"Group-chat transcript view"：
      补两条新不变量（工具集白名单 / 无 streak 计数），引用本任务 diagnosis.md。
- [ ] 更新该 § 里提到 `participant_tool_defs` 的地方 → `group_chat_tool_defs`。
- [ ] 更新提到 `moderator max_turns=Some(1)` 的 key contract（不变，但确认 R3 prompt 加固
      不破坏 contract 3 的论证）。

## 全量验证

- [ ] `cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`
      （全绿，0 failed）。
- [ ] `cd app && pnpm test`（全绿）。
- [ ] `cd app && npx vue-tsc --noEmit`（零错误）。
- [ ] `cargo clippy --manifest-path ... --lib -- -D warnings`（零警告）。

## 风险点 / 回滚

- R1 白名单遗漏 → grep `GROUP_CHAT_RESEARCH_TOOLS` 即知要不要加；回滚 = 黑名单。
- R2 去 streak 烧钱 → MAX_ORCHESTRATION_ROUNDS=30 兜底；回滚 = 恢复 streak（但会和 D1 决策冲突）。
- R3 prompt 过度约束 → moderator prompt 同时强调"researching is good"，不是禁止调研。
- 跨任务：清理 08-07 streak 路径；08-07 其余 4 个修复（R1身份/R3max_turns/R4order/R5view）不受影响。
