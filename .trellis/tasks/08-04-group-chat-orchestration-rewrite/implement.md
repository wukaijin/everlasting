# Implement: 系统重写群聊编排(transcript 管理 + 持久化去重 + 角色隔离)

## 前置

- 已读 `prd.md` + `design.md` + `research/db-evidence.md`。
- 已确认关键结论(design §2):唯一重复写入点 = `run_chat_loop` 入口 user 持久化点
  (`chat_loop.rs:964-1075`);`result_blocks` 持久化点只写新鲜结果,不是重复来源。
- 决策:D-A 参与者过滤仲裁工具交互(用户确认);D-B reload 保持;D-D 入口 guard(scope
  到 group_chat);D-F 旧启发式 guard 被替换。

## 执行步骤(每步后 `cargo check` 通过再继续)

### Step 1 — 复现缺陷(先写失败测试,红)

1. `app/src-tauri/src/agent/mod.rs` 加 `pub mod tests_group_chat;`(模块头 `#![cfg(test)]`)。
2. 新建 `app/src-tauri/src/agent/tests_group_chat.rs`:
   - `make_group_chat_harness()`:仿 `tests_common::make_harness`,session 用
     `db::create_session(..., Some("group_chat"), Some(metadata_json))`(metadata =
     `{"participants":[{"name":"M1","model":"m1","persona_md":"<M1 persona>"},{"name":"M2","model":"m2"}]}`)。
   - 三个 `MockProvider`(moderator / m1 / m2)+ `ProviderCatalog`
     (`HashMap<model_id, Arc<dyn Provider>>`)注入 `worker_catalog`。
   - 直接调 `run_group_chat_loop(...)`(参数对齐 group_chat_loop.rs 现有签名)。
   - mock 脚本(moderator 单轮 = **1 次 send**,`mod_tool_turn` 一轮流 = Start → Delta
     (发言) → ToolCall(仲裁) → Done{tool_use};08-04 follow-up moderator max_turns=1):
     - moderator round0:`mod_tool_turn("c1", nominate_speaker, {name:"M1"}, "主持人发言")`
     - m1:`text_turn("我是 M1")`
     - m2:`text_turn("我是 M2")`
     - moderator round1:`mod_tool_turn("c2", nominate_speaker, {name:"M2"}, "主持人:请 M2")`
     - moderator round2:`mod_tool_turn("c3", end_discussion, {}, "主持人:结束")`
   - 断言(此时应红):无 `ChatEvent::Error` / DB 每 tool_use_id 恰 1 条 tool_result /
     无重复文本行。
3. 跑:`cargo test --lib agent::tests_group_chat` → 预期因重复 tool_result 而红
   (400 未 mock,但重复行断言直接红)。

### Step 2 — 群聊编排重写(`group_chat_loop.rs`)

1. 新增纯函数 `participant_view(full: &[ChatMessage]) -> Vec<ChatMessage>`:
   状态机剔除含仲裁工具 ToolUse 的 assistant 行(保留 think/text 块) + 紧随的
   user(tool_result) 行;其余原样。模块头注释引用 design §4。
2. `run_group_chat_loop` 重写:
   - 移除 `messages = reload_messages(...)` 的两处赋值;改为 round==0 用入参
     `messages`、否则 reload 一次作为 `full`。
   - moderator 轮:入参 = `full`(View-1),`max_turns = Some(1)`(08-04 follow-up
     单轮;旧 Some(3) 的第二轮 filler 是身份混淆诱因,已确认删除)。
   - participant 轮:入参 = `participant_view(&full)`(View-2),其余参数不变。
   - 保留 `SharedTurnState` 拦截、round-robin fallback、MAX_ORCHESTRATION_ROUNDS、
     `resolve_provider`、`moderator_system_prompt`、`participant_system_prompt`、
     `participant_tool_defs`(d2c7c32 保留)。
3. `reload_messages` 保留(编排 resync 用,不删)。
4. `cargo check` → 修复签名/借用问题。

### Step 3 — 入口 user 持久化点 guard(`chat_loop.rs`)

1. 现有 `user_message_matches` 辅助函数保留(纯判据:tool_use_id 精确匹配 / 文本字节相等)。
2. 替换旧 guard(chat_loop.rs:967-980 `already_in_db` 计算块):
   - **去掉** `messages.len() == loaded_session.messages.len()` 长度判据;
   - **加上** `group_chat_state.is_some()` 前置条件;
   - 匹配范围改为 `loaded_session.messages` 中**任一** user-role 行(不再是"尾部 user 行")。
3. 旧 guard 的 6 个单测更新:`user_message_matches` 纯判据测试保留(可能微调),
   删除依赖长度判据的测试;新增 `group_chat_state=None → 不跳过` 回归测试。
4. `cargo test --lib agent::chat_loop` → 绿。

### Step 4 — 集成测试转绿 + 断言补全

1. 跑 `cargo test --lib agent::tests_group_chat`:
   - 断言无 `ChatEvent::Error`;DB 每 tool_use_id 恰 1 条 tool_result;
   - `M1.sent_messages()` 各条不含 nominate/end 的 ToolUse/ToolResult 块(AC3);
   - `M1.sent_systems()[0]` 以 M1 persona 开头 + 含身份护栏块(AC4);moderator
     `sent_systems()[0]` 含模板(AC4);
   - moderator `sent_messages` 含自身 nominate tool_use(AC5);
   - `call_count` 断言(moderator=3、m1=1、m2=1);
   - round-0 人类消息在 DB 恰 1 条。
2. 单测覆盖 `participant_view` 四类输入(含仲裁对 / 无仲裁对 / 纯工具行 / 连续两个
   moderator 轮)→ 断言剔除后无孤儿 tool_use / 无孤儿 tool_result(§469)。

### Step 5 — 回归

```bash
cd app/src-tauri
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib agent::tests_agent_loop   # 40 个回归
PKG_CONFIG_PATH="..." cargo test --lib agent::tests_group_chat                          # 新集成
PKG_CONFIG_PATH="..." cargo clippy --bin everlasting-daemon -- -D warnings
PKG_CONFIG_PATH="..." cargo check --bin everlasting-daemon
```

### Step 6 — 清理

- 删除旧任务 `08-04-group-chat-tool-result-dedup` 已提交部分?不 —— 旧任务未提交,
  工作区只有 `chat_loop.rs` diff。Step 3 重写该 diff 区域即可(同文件,覆盖性修改)。
- 确认 `git diff` 仅涉及:`chat_loop.rs`(guard 区域)、`group_chat_loop.rs`(重写)、
  `agent/mod.rs`(一行)、新文件 `tests_group_chat.rs`。
- 更新 handoff 相关注释(`group_chat_loop.rs` 模块头、guard 处)。

## 验证命令(汇总)

```bash
cd app/src-tauri
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib agent::tests_group_chat agent::tests_agent_loop agent::chat_loop
PKG_CONFIG_PATH="..." cargo clippy --bin everlasting-daemon -- -D warnings
PKG_CONFIG_PATH="..." cargo check --bin everlasting-daemon
```

## 回滚点

- Step 1 后失败测试即回滚点:保留测试文件,git checkout 其余。
- Step 3 后若 `tests_agent_loop` 红:guard 判据过宽 → 收紧(加回部分条件)。
- 集成测试暴露编排 resync 问题 → design §10 变体(内存 transcript 不 reload)。
- 整体:`git checkout -- app/src-tauri/src/agent/group_chat_loop.rs app/src-tauri/src/agent/chat_loop.rs`。

## Review Gates

- [ ] 失败测试先红(Step 1),重写后转绿(Step 4)。
- [ ] 无 400 / 无 `[生成出错中断]` / DB 无重复 tool_result(AC1/AC2)。
- [ ] participant 视角过滤断言(AC3/AC5);system prompt 断言(AC4)。
- [ ] `tests_agent_loop` 40 个回归全绿(AC6)。
- [ ] clippy clean + cargo check 通过(AC7)。
- [ ] 旧任务 `08-04-group-chat-tool-result-dedup` 由用户在验证后归档。
