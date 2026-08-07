# Implement — 群聊 per-role history 隔离(形态 B)

> 执行顺序:R1(组装器,核心)→ R2(调用点切换 + 删旧)→ 测试(新增 + 迁移)→ 全量验证。
> 冲击面只在编排器一层;DB/前端/IPC/流式零改动(diff 审查确认)。

## 环境提示

- 后端(WSL 必须 PKG_CONFIG_PATH):
  `cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`
  (多线程默认,**勿加** `--test-threads=1`)。冷编 ~1m37s,增量 ~11s。
- scope 冒烟(一次调用):`cargo test --manifest-path ... --lib "group_chat"`。
- 前端(本任务不动前端,但跑一次确保无回归):`cd app && pnpm test`。

## Step 1 — R1 新增 `role_history` 组装器

- [ ] `group_chat_loop.rs`:新增私有工具函数
      `fn extract_text_blocks(c: &MessageContent) -> String`(从 Blocks 抽所有 Text 块拼接)。
- [ ] 新增 `fn extract_tool_use_ids(c: &MessageContent) -> Vec<String>`
      (从 Blocks 抽所有 ToolUse.id)。
- [ ] 新增 `fn row_carries_any_tool_result(m: &ChatMessage, ids: &[String]) -> bool`
      (判断 user 行是否携带 ids 中的任一 tool_result)。
- [ ] 新增 `fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage>`,
      按 design §R1 的状态机实现。重点:
      - 当前角色 assistant(speaker == Some(current_role))→ 原样 clone。
      - 他人 assistant → extract_text_blocks 改写为 role:user + `"@{speaker}: {text}"`,
        speaker 字段保留原值;收集 ToolUse id 进 pending。
      - 他人 ToolResult(匹配 pending)→ 整行跳过。
      - 人类原始 user(speaker == None)→ 原样保留。
      - 他人 user 行(非 tool_result)→ 保守保留。
- [ ] doc 注释引用 design §R1 的不变量 + 为何丢他人 Thinking(signature 约束)。

## Step 2 — R2 调用点切换 + 旧逻辑删除

- [ ] moderator turn(`group_chat_loop.rs:536-540`):reload 后加
      `let history = role_history(&full, "moderator");`,run_chat_loop 传 `history`(取代 `full`)。
      注意 round==0 分支用的是 caller 传入的 `messages`,也要过 role_history。
- [ ] participant turn(`:715-716`):`participant_view(&full)` →
      `role_history(&full, &participant.name)`。
- [ ] 删除 `participant_view`(`:231-264`)+ `participant_view_row`(`:270-313`)。
- [ ] grep 确认 `participant_view` 全仓零残留(除 git 历史)。

## Step 3 — 测试新增 + 迁移

- [ ] 删除 `participant_view_*` 一族测试(`group_chat_loop.rs:1373+`)+
      `tests_group_chat.rs` 相关。
- [ ] 新增 `role_history_*` 10 个测试(见 design §测试设计)。重点:
      - `role_history_current_role_assistant_verbatim`:断言 Thinking 块 + signature 原样。
      - `role_history_other_thinking_dropped`:断言结果里无 Thinking/RedactedThinking(他人)。
      - `role_history_signature_roundtrip_contract`:契约级守 signature 完整。
      - `role_history_other_tool_pair_dropped`:他人 ToolUse+ToolResult 整对消失。
      - `role_history_own_tool_pair_preserved`:自己 tool 对保留(防 OpenAI 400)。
- [ ] 验证既有 `identity_contract_*` 测试仍绿(08-07 R1,断言不依赖 participant_view
      具体输出;若依赖,调整断言对象为 role_history 输出,不变量不变)。
- [ ] 验证既有 prompt 回归测试(08-07 R3)仍绿(prompt 没动)。

## Step 4 — 全量验证

- [ ] scope 冒烟:`cargo test --manifest-path ... --lib "group_chat"`(先确保群聊全绿)。
- [ ] 后端全量:`cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`(0 failed)。
- [ ] 前端(确认无回归):`cd app && pnpm test`。
- [ ] 类型:`cd app && npx vue-tsc --noEmit`(零错误,理论上本任务不动前端)。
- [ ] clippy:`cargo clippy --manifest-path ... --lib -- -D warnings`(零警告)。

## Step 5 — spec 沉淀

- [ ] `.trellis/spec/backend/agent-loop-architecture.md`:更新 §"Group-chat transcript view",
      把 `participant_view` 的描述改为 `role_history`(per-role 隔离);补一条新不变量:
      "per-role history 隔离——每个角色只看到自己的 assistant 历史(含 thinking signature),
      他人发言改写为 user,他人 thinking/tool 对不进上下文(治多身份 assistant 串台 +
      绕 Anthropic signature 400 约束)"。
- [ ] 引用本任务 diagnosis.md + design.md。

## 风险点 / 回滚

- R1 重写规则边界 bug → 10 个测试 + signature 契约 + identity_contract 兜底;回滚 = 恢复 participant_view。
- 误把自己当他人剥离 → `role_history_multiturn_same_role_preserved` 守。
- 跨 provider 行为分叉 → role_history 无 provider 分叉(design §风险);OpenAI 测试用例确认。
- 回滚成本:纯逻辑层,git revert 即可,无 DB 迁移。

## 注意

- 本任务**不动前端/DB/IPC/流式**。若 diff 审查发现触及这些层,说明偏离了形态 B 设计,
  回到 design.md §"不动的"核对。
- 真模型验证(AC8)在落地后人工重跑同模型组合 session,不在 implement 自动化范围。
