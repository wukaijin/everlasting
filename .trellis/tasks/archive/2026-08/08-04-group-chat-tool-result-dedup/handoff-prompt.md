# 群聊编排系统重写 — 新 session 启动 prompt

把下面整段贴给新 session 的 AI。

---

## 任务:系统重写群聊(group_chat)编排

群聊功能(task `07-29-group-chat`)上线后无法正常工作,经多轮调试确认是
`run_group_chat_loop` 的编排逻辑存在**多个交织的根本性缺陷**,逐个补丁无法收敛。
需要系统性地重新设计与重写群聊编排。这是一个复杂任务,请先走 Trellis Phase 1
规划(读 prd/design、研究、写新任务的 prd+design+implement),再动手。

### 背景:群聊是什么

`group_chat` 是一种多 LLM 群聊会话:一个 **moderator**(用 session 自己的 model)
编排讨论,多个 **participants**(各自独立的 model + persona)轮流发言。moderator 用
`nominate_speaker` 工具点名 / `end_discussion` 结束。设计文档在:
- `.trellis/tasks/archive/2026-07/07-29-group-chat/design.md`(原始设计)
- `.trellis/tasks/archive/2026-07/07-29-group-chat/prd.md`

### 已确认的缺陷清单(本次调试结论,务必先验证再采信)

**核心入口**:`app/src-tauri/src/agent/group_chat_loop.rs`(`run_group_chat_loop`,
447 行)。每个 speaker 轮流调 `run_chat_loop`(`app/src-tauri/src/agent/chat_loop.rs`),
轮之间用 `reload_messages` 从 DB 重载全部消息作为下一 speaker 的 transcript。

**缺陷 1 — tool_result 跨 speaker 重复持久化 → API 400**
`run_chat_loop` 的 user 消息持久化点(`chat_loop.rs:894`)和 `result_blocks` 持久化点
(`chat_loop.rs:4300`)会把 reload 进来的、已落库的 tool_result(role=user)当新消息再
写一遍。结果:同一 `tool_use_id` 出现多条 tool_result → 一条孤立(无匹配 tool_calls)
→ OpenAI 400 "Messages with role 'tool' must be a response to a preceding message with
'tool_calls'" / Anthropic 2013 "invalid params"。这是**转录配对错乱**在两个协议上的
不同表现,根因相同。已有部分修复(见下"已提交的修复")但只覆盖了 user 消息持久化点,
`result_blocks` 路径未覆盖。

**缺陷 2 — 参与者身份/system prompt 错乱**
DB 证据:participants speaker=M3 的发言,其 thinking 是 "I need to respond as the
moderator",还在 text 里自称"我是主持人"。说明 `run_group_chat_loop` 给参与者传的
transcript 视角或 system prompt 让它错认为自己是 moderator。怀疑点:参与者的
`system_prompt_override`(`participant_system_prompt`)是否真的覆盖了,以及参与者看到的
transcript 里 moderator 的 tool 交互是否让模型混淆角色。

**缺陷 3 — 参与者拿到了 nominate_speaker 工具**
参与者 turn 此前传入完整 `builtin_tools()`(含 nominate_speaker/end_discussion),
参与者会自行调用 nominate_speaker,因其 `group_chat_state=None` 拦截返回错误。
**已在 commit `d2c7c32` 修复**(提取 `participant_tool_defs` 过滤 + 单测),新设计要保留
这个隔离。

### 关键文件

- `app/src-tauri/src/agent/group_chat_loop.rs` — 编排主循环(重写核心)
- `app/src-tauri/src/agent/group_chat.rs` — `build_group_chat_ctx`(门禁 + 解析 participants)
- `app/src-tauri/src/agent/chat_loop.rs` — `run_chat_loop`(单 speaker turn 引擎,**不要大改**,
  理解其 user 消息持久化点 `:894` 和 result_blocks 持久化点 `:4300` 如何依赖"尾部 user 消息
  一定是新的"这个隐式不变量)
- `app/src-tauri/src/agent/chat.rs:395-420` — `chat_inner` 里 group_chat 分支入口
- `app/src-tauri/src/tools/nominate_speaker.rs` / `end_discussion.rs` — 信号工具
- `app/src-tauri/src/llm/provider/wire.rs:239-259` — tool_use/tool_result 配对的诊断
  (`orphan_tool_use_ids`,纯 log 不修复)
- DB 路径:`~/.local/share/dev.everlasting.app/everlasting.db`,表 `sessions`(`session_type`
  列 + `metadata` JSON 含 participants)/ `messages`(`speaker` 列)

### 已提交的修复(新设计要保留 / 基于其上)

- `4e2fe99` 前端:UI 图标缺失 / 下拉 option 不可见 / modal 滚动(已验证,与编排无关)
- `d2c7c32` 参与者工具过滤 `participant_tool_defs`(已验证,保留)

### 已实现但未 commit 的修复(缺陷 1 的部分覆盖)

`08-04-group-chat-tool-result-dedup` 任务:
- `chat_loop.rs` 加了 `user_message_matches` 判据 + `already_in_db` 守卫(在 user 消息持久化点)
- 6 个单测 + 40 个 `tests_agent_loop` 回归全绿
- **但 DB 证明重复 tool_result 仍出现** —— 守卫只覆盖 user 消息持久化点,`result_blocks`
  持久化点未覆盖。这部分代码在工作区未提交。**新设计若重写编排,可决定取舍这部分。**

### 核心设计难点(重写时必须解决)

1. **transcript 隔离**:每个 speaker 应该看到什么?moderator 的 tool 交互
   (nominate_speaker)是否该对参与者可见?跨 speaker 的 tool_use/tool_result 配对如何
   在 DB 单线性消息流里保持 API 合法?当前"reload 全部消息"策略是 400 的根源。
2. **持久化去重**:`run_chat_loop` 假设"进入时尾部 user 消息是新的",reload 违反它。
   要么编排层不把已落库消息当新消息传,要么 `run_chat_loop` 自己能识别已落库消息。
3. **角色/prompt 隔离**:参与者的 system prompt + transcript 不能让它误认自己是 moderator。
4. **充分测试**:必须用 mock provider 写**集成测试**(跑完整 moderator→participant→moderator
   多轮),验证:无 400、DB 无重复 tool_result、每个 speaker 角色正确。**不要靠手动 UI 测试撞 bug。**

### 第一步建议

1. 读原始 design(`07-29-group-chat/design.md`)+ 本任务的 prd/design/implement
   (`08-04-group-chat-tool-result-dedup/`)。
2. 用 mock provider 复现缺陷(写一个失败测试:多轮群聊 → 断言无 400 / 无重复 tool_result)。
3. 重新设计 transcript 管理(每个 speaker 的可见消息集 + tool 配对如何保持)。
4. 走 Trellis Phase 1 规划后再实现。
