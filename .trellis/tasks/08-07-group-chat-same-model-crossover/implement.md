# Implement — 群聊 per-role history 隔离(形态 B)

> 执行顺序:R1(组装器,核心)→ R2(调用点切换 + 删旧)→ R2.5(D-D 守卫扩展)→
> 测试(新增 + 迁移)→ 全量验证。**评审修订已合入**(review.md P0-1/P0-2/P1-1/P2)。
> 改动面 = 编排器(`group_chat_loop.rs`)+ D-D 守卫(`chat_loop.rs` 一处)。DB/前端/IPC/流式零改动。

## 环境提示

- 后端(WSL 必须 PKG_CONFIG_PATH):
  `cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`
  (多线程默认,**勿加** `--test-threads=1`)。冷编 ~1m37s,增量 ~11s。
- scope 冒烟(一次调用):`cargo test --manifest-path ... --lib "group_chat"`。
- 前端(本任务不动前端,但跑一次确保无回归):`cd app && pnpm test`。

## Step 1 — R1 新增 `role_history` 组装器

- [ ] `group_chat_loop.rs`:新增私有工具函数 `fn extract_text_blocks(c: &MessageContent) -> String`
      (从 Blocks 抽所有 Text 块拼接)。
- [ ] 新增 `fn extract_tool_use_ids(c: &MessageContent) -> Vec<String>`(从 Blocks 抽所有 ToolUse.id)。
- [ ] 新增 `fn row_carries_any_tool_result(m: &ChatMessage, ids: &[String]) -> bool`
      (判断 user 行是否携带 ids 中的任一 tool_result)。
- [ ] 新增 `fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage>`,
      按 design §R1 状态机实现。重点(含评审修订):
      - 当前角色 assistant(speaker == Some(current_role))→ 原样 clone(含 Thinking+signature)。
      - 他人 assistant → extract_text_blocks 改写为 role:user,**content 不带 `@` 前缀**,
        **speaker 字段保留原值**(P0-2 方案 a);收集 ToolUse id 进 pending。
      - 他人 ToolResult(匹配 pending)→ 整行跳过。
      - 人类原始 user(speaker == None)→ 原样保留。
      - **speaker None 的 assistant 行**(异常)→ `debug_assert!` 报警 + 原样保留,不静默
        改写成 `@?`(P2-2)。
      - 状态机用 `match (role)` + `match (speaker)`,避免重复分支(P2-1)。
- [ ] doc 注释引用 design §R1 的不变量 + 为何丢他人 Thinking(signature 约束)+ 归属策略。

## Step 2 — R2 调用点切换 + 旧逻辑删除

- [ ] moderator turn(`group_chat_loop.rs:536-540`):reload 后加
      `let history = role_history(&full, "moderator");`,run_chat_loop 传 `history`(取代 `full`)。
      **注意 round==0 分支**(caller 传入的 `messages`)也要过 role_history。
- [ ] participant turn(`:715-716`):`participant_view(&full)` →
      `role_history(&full, &participant.name)`。
- [ ] 删除 `participant_view`(`:231-264`)+ `participant_view_row`(`:270-313`)。
- [ ] grep 确认 `participant_view` 全仓零残留(除 git 历史)。

## Step 2.5 — R2.5 D-D 入口守卫扩展(P0-1 + P0-3)

- [ ] `chat_loop.rs:990-1000` 的 `already_in_db` 判定:在 `group_chat_state.is_some()` 作用域内,
      tail user 消息 `speaker.is_some()` 时**视同已落库、跳过 persist**(短路)。按 design §R1.5
      的安全依据实现(群聊人类消息恒 speaker None,speaker Some 的 user 行只能是改写产物)。
- [ ] **P0-3(自查发现)**:speaker Some 分支的 seq 取**尾部最后一个 user 行**
      (`iter().rev().find(role=="user")`),**不用** `find(|db_row| speaker_some || matches)`
      (常数短路会匹配到 DB 第一条 user 行,seq 错位)。
- [ ] **P0-3**:speaker Some 命中守卫后,`last_user_snapshot` 返回 **`None`**(非
      `Some(msg.content)`),使 `chat_loop.rs:1116` 的 at_file 注入条件为 false——改写行是
      他人发言转述,不触发 `@file` 注入(否则注入 manifest 写错误 seq 行 + FileInjections
      事件错位)。
- [ ] 不改守卫对非群聊路径的行为(speaker None 时走原判定)。

## Step 3 — 测试新增 + 迁移

- [ ] 删除 `participant_view_*` 一族测试(`group_chat_loop.rs:1373+`)+ `tests_group_chat.rs` 相关。
- [ ] 新增 `role_history_*` 测试(design §测试设计)。重点:
      - `role_history_other_speaker_rewritten_as_user`:断言 **content 无 `@` 前缀 + speaker 保留** + 无 Thinking(P0-2)。
      - `role_history_other_thinking_dropped`:断言结果里无 Thinking/RedactedThinking(他人)。
      - `role_history_signature_roundtrip_contract`:契约级守当前角色 signature 完整。
      - `role_history_other_tool_pair_dropped` / `role_history_own_tool_pair_preserved`:tool 对剥离/保留。
      - `role_history_wire_no_double_prefix`:改写行经 wire 序列化后 Anthropic `@name:` 只一次(P0-2)。
- [ ] 新增 D-D 守卫测试(chat_loop 守卫测试处):
      `dd_guard_skips_persist_for_speaker_user_in_group_chat`(speaker Some 不 persist)+
      `dd_guard_unchanged_for_classic_chat_speaker_none`(非群聊行为不变)+
      `dd_guard_rewrite_row_skips_at_file_injection`(P0-3:改写行 last_user_snapshot=None,
      注入条件 false,无 manifest 写入/无 FileInjections 事件)+
      `dd_guard_rewrite_row_seq_not_first_user_row`(P0-3:改写行 seq 非 DB 第一条 user 行)。
- [ ] **P1-1 语义重写**:`identity_contract_view_holds_under_same_model_and_mislabel`(`:1537`)
      **不是断言对象替换**——旧前提是"view 不消毒,mislabeled 行原样透传";role_history
      按 speaker 强制归因。迁移做法:断言 mislabeled 行(`speaker="M3"` 内容 `@D4F: …`)在
      role_history 输出里变为 `role:user + speaker=M3`,content 文本不变;不再断言"原样透传
      assistant"。仲裁剥离 + 无孤儿不变量仍成立。
- [ ] 验证既有 `identity_contract_prompts_separate_roles_under_same_model`(`:1614`,只测 prompt)
      仍绿;prompt 回归测试(08-07 R3)仍绿(prompt 没动)。

## Step 4 — 全量验证

- [ ] scope 冒烟:`cargo test --manifest-path ... --lib "group_chat"`(先确保群聊全绿)。
- [ ] 后端全量:`cargo test --manifest-path /usr/local/code/github/everlasting/app/src-tauri/Cargo.toml --lib`(0 failed)。
- [ ] 前端(确认无回归):`cd app && pnpm test`。
- [ ] 类型:`cd app && npx vue-tsc --noEmit`(零错误,理论上本任务不动前端)。
- [ ] clippy:`cargo clippy --manifest-path ... --lib -- -D warnings`(零警告)。
- [ ] **人工核验(AC6/AC9)**:群聊跑一轮后查 DB,确认无 `@<speaker>: <text>` 重复 user 行
      (SQL: `SELECT role, speaker, substr(text,1,50) FROM messages WHERE session_id=? AND role='user' ORDER BY seq`),
      且前端无重复 speaker chip 行。

## Step 5 — spec 沉淀

- [ ] `.trellis/spec/backend/agent-loop-architecture.md`:更新 §"Group-chat transcript view",
      把 `participant_view` 的描述改为 `role_history`(per-role 隔离);补两条新不变量:
      1. **per-role history 隔离**:每角色只看到自己的 assistant 历史(含 thinking signature),
         他人发言改写为 user(不带前缀、保留 speaker),他人 thinking/tool 对不进上下文
         (治多身份 assistant 串台 + 绕 Anthropic signature 400 约束)。
      2. **D-D 守卫 speaker 短路**:群聊作用域内 tail user 行 speaker Some 视同已落库
         (role_history 改写产物不重复落库)。
- [ ] 引用本任务 diagnosis.md + design.md。

## 风险点 / 回滚

- R1 重写规则边界 bug → 测试覆盖每条重写规则;signature 契约 + identity_contract 兜底。
- P0-1 守卫扩展误伤 → `speaker.is_some()` 信号零误伤(群聊人类消息恒 None);两测试守边界。
- P0-2 归属策略 → `role_history_wire_no_double_prefix` 守无双重前缀。
- 误把自己当他人剥离 → `role_history_multiturn_same_role_preserved` 守。
- 跨 provider 行为分叉 → role_history 无 provider 分叉(design §风险);OpenAI 测试用例确认。
- 回滚成本:纯逻辑层(组装器 + 守卫一处短路),无 DB 迁移,git revert 即可。

## 注意

- 本任务**不动前端/DB/IPC/流式**。若 diff 审查发现触及这些层,说明偏离形态 B 设计,
  回到 design.md §R4 核对。
- 真模型验证(AC9)在落地后人工重跑同模型组合 session,不在 implement 自动化范围。
