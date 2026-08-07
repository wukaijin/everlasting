# Implement — 群聊评审问题修复

> 执行顺序按依赖与风险排：R4（独立、低风险）→ R5（注释+测试）→ R3（核心能力）
> → R2（编排器事件，依赖 R3 已稳）→ R1（契约测试，最后补，验证整体）。
> 每步后跑相关子集测试，全部完成后跑全量。

## 环境提示

- 后端测试（WSL 必须 PKG_CONFIG_PATH）：
  `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
  （多线程默认，**勿加** `--test-threads=1`，单线程慢 ~3×）。
  冷编 ~1m37s，增量 ~11s。
- 前端测试：`cd app && pnpm test`。
- scope 冒烟：`cargo test --lib "tests_group_chat::"`（一次调用，避免逐模块循环
  每次付 ~11s relink 税）。

## Step 1 — R4 删 order + UI 按钮（独立、低风险）

- [ ] 后端 `app/src-tauri/src/agent/group_chat.rs`：删 `ParticipantConfig.order`
      字段 + `#[serde(default)]` / `#[allow(dead_code)]` 注解（`:40-42`）。
- [ ] 验证 serde 默认 ignore unknown fields：加一条单测，反序列化含 `order`
      的 metadata JSON，断言不报错 + participants 解析正确。
      （若 ParticipantConfig 未设 `deny_unknown_fields`，serde 默认忽略多余 key。）
- [ ] `group_chat_loop.rs:848` 测试 fixture 删 `order: None`。
- [ ] 前端 `chat.types.ts`：`ParticipantConfig` 删 `order?`。
- [ ] 前端 `GroupChatConfigModal.vue`：删 `moveUp`/`moveDown`、↑/↓ 两个 button、
      submit 的 `order: i`（两处）、re-seed 的 `order: p.order`。
- [ ] 前端 `GroupChatConfigModal.test.ts`：若有 order/重排断言则更新。
- [ ] 测试：`cargo test --lib "tests_group_chat::"` + `pnpm test GroupChatConfigModal`。

## Step 2 — R5 participant_view 不变量锁死（注释 + 测试，不改逻辑）

- [ ] `group_chat_loop.rs:181-185` `participant_view` doc 补显式不变量注释：
      仲裁对相邻性只依赖 moderator 单轮落库；参与者非仲裁对 passthrough 不进
      strip 状态机。
- [ ] 加一条 view 单测 `participant_view_participant_multiturn_non_arbitration_mixed`：
      构造 [user, mod(仲裁对), participant(read_file 对 ×2 多轮) + text, mod(仲裁对),
      participant(text)]，断言：所有非仲裁对 passthrough + 无 orphan +
      仲裁对全剥 + 顺序保留。
- [ ] 测试：`cargo test --lib "tests::participant_view"`。

## Step 3 — R3 参与者 max_turns 1 → 20（核心）

- [ ] `group_chat_loop.rs:665` 参与者分支 `Some(1)` → `Some(20)`。
- [ ] 更新该处注释：从 "single turn" 改为多轮取材说明 + moderator 保持单轮的
      理由（保留 08-04 follow-up 的论证）。
- [ ] 新增集成测试 `participant_can_use_tools_then_speak_next_turn`：
      - mock 参与者第 1 个 response = tool_use(read_file)，stop_reason=tool_use。
      - mock 参与者第 2 个 response = text 发言，end_turn。
      - 断言：参与者 `sent_messages` ≥ 2（多轮闭环）；DB 参与者 assistant 行 +
        user(tool_result) 行相邻；无 `ChatEvent::Error`。
- [ ] 复跑既有 `group_chat_full_multi_round_flow...`（参与者用单轮 text mock）
      仍绿（确保 max_turns 提高不破坏既有 flow）。
- [ ] 测试：`cargo test --lib "tests_group_chat::"`。

## Step 4 — R2 编排器静默路径变事件（D2：复用 Done.stop_reason）

- [ ] `group_chat_loop.rs`：循环退出原因提成 `enum HaltReason { ModeratorStuck,
      MaxRounds, DiscussionEnded }`。`break` 时携带（或循环后用一个
      `Option<HaltReason>` 变量）。
- [ ] 循环退出后的终态 Done emit：match HaltReason 决定 stop_reason
      （`DiscussionEnded`→`group_chat_end`，`ModeratorStuck`→`moderator_stuck`，
      `MaxRounds`→`max_rounds`）。非 cancel 才 emit（既有逻辑，保留）。
- [ ] nominee 未知 / participant provider 解析失败的 `continue` 两条：在
      `continue` 前 emit `Done { stop_reason: "nominee_unknown" /
      "participant_unresolved" }`（非 group_chat_end，不 finalize）。注意这是
      非终态 Done——前端靠 stop_reason 区分。
- [ ] 前端 `streamController.ts` `done` handler：
      finalize 白名单加 `moderator_stuck`/`max_rounds`
      （`streamController.ts:1397`）。continue 两条不加。
- [ ] 前端 `streamController.ts`：非 finalize Done 且 stop_reason ∈
      `{moderator_stuck, max_rounds, nominee_unknown, participant_unresolved}` →
      在当前 placeholder 挂 `notice` 字段（类比 `retrying`，transient 不进 DB）。
      ChatEventPayload 已有 stop_reason 字段，无需扩 payload 类型。
- [ ] 前端 `MessageItem.vue`：渲染 `message.notice` 灰色行（4 条文案）。
      speaker chip 仍正常显示（notice 是叠加，不替代发言）。
- [ ] 前端 `chat.types.ts`：`ChatMessage` 加可选 `notice?: string`。
- [ ] 后端测试：
      - `moderator_stuck_terminal_done`：moderator 连续 3 次不调工具 → 最后一个
        Done = `stop_reason: "moderator_stuck"` + 是终态。
      - `nominee_unknown_non_terminal_done`：nominate 未知名 → emit
        `Done{stop_reason:"nominee_unknown"}` 但后续仍有事件（非 finalize）。
- [ ] 前端测试：`streamController.test.ts` 加"moderator_stuck finalize +
      notice"、"nominee_unknown 不 finalize + notice"两条。
- [ ] 测试：`cargo test --lib "tests_group_chat::"` + `pnpm test streamController`。

## Step 5 — R1 身份正确性契约底线（最后补，验证整体）

- [ ] 在 `group_chat_loop.rs` `#[cfg(test)] mod tests`（或新文件，倾向前者）
      加 `identity_contract_same_model_combination`：
      - 构造 GroupChatCtx，moderator_model_id == participant.model（同模型）。
      - 构造最坏 `full`：moderator 仲裁对 + 一个"弱模型输出他名前缀"的
        assistant 行（speaker=M3 但内容"@D4F: ..."）。
      - 断言 `participant_view(&full)`：
        (a) 0 个仲裁块；(b) `no_orphan_pairs` 成立；
        (c) 该"他名前缀"行作为非仲裁行 **passthrough**（view 层不改内容——
        内容净化靠 prompt，view 只保证结构不变量；注释说明此边界）。
      - 断言 `participant_system_prompt("M3", None)` + `moderator_system_prompt`
        在同模型组合下仍含角色区分文本（既有 `participant_prompt_forbids...` /
        `moderator_prompt_forbids...` 断言复用，加同模型 ctx 构造）。
- [ ] 测试注释写明：这是契约级底线，不保证真模型行为。
- [ ] 测试：`cargo test --lib "tests::identity_contract"`。

## 全量验证（task.py start 前不跑；落地后跑）

- [ ] `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
- [ ] `cd app && pnpm test`
- [ ] clippy：`cd app/src-tauri && PKG_CONFIG_PATH="..." cargo clippy --lib -- -D warnings`
      （零警告是既有基线）。

## 风险点 / 回滚

- R2 终态 Done finalize 白名单漏加 → 前端挂死；Step 4 测试必须覆盖"终态 Done
  后 active request 已清"。
- R3 max_turns=20 成本上升；回滚 = `Some(1)`。
- R4 反序列化既有 metadata；Step 1 加 ignore-unknown 测试锁死。
- 全程不碰 D-D 入口护栏 / wire 孤儿自愈 / 5 层身份防御的核心逻辑——只补边界。
