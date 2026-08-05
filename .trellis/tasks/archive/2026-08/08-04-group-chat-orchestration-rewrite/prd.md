# 系统重写群聊(group_chat)编排

## Goal

系统重写 `run_group_chat_loop`(`app/src-tauri/src/agent/group_chat_loop.rs`,447 行)的
编排逻辑,根治三个交织的根本性缺陷:跨 speaker 重复持久化 tool_result(→ API 400)、
参与者身份错乱(误认自己是 moderator)、参与者拿到仲裁工具(已修复,保留)。
群聊多轮(moderator → participant → moderator)必须能完整流畅跑通,
每轮 DB 只落一份正确的 tool_result,每个 speaker 的角色视角正确。

这是**系统性重写**(非继续打补丁):现有"reload 全部消息 + 依赖 `run_chat_loop`
不变量"的策略已被证明无法收敛(旧任务 `08-04-group-chat-tool-result-dedup` 的
`user_message_matches` 守卫只覆盖一个持久化点,DB 仍出现重复)。

## Background / 已确认事实

完整取证见 `research/db-evidence.md`。核心事实:

- **缺陷 1 — 重复 tool_result 的写入路径**:`messages` 表 `UNIQUE(session_id, seq)`
  但 `run_chat_loop` 每次进入重新算 `seq = max+1`,所以"再写一遍已落库消息"不会撞约束,
  而是写一条**新 seq 的同内容行**。写重复行的点有三个(全在 `run_chat_loop` 内):
  用户消息持久化点(`chat_loop.rs:1001`)、result_blocks 持久化点(`chat_loop.rs:4393`)、
  合成 tool_result 点(cancel/error 路径)。群聊 4 个 turn 都用 `max_turns`(=3 或 1)
  退出 → result_blocks 点**每轮必命中**。DB 证据:同一 `tool_use_id` 出现 30+ 条
  user-role 行,每条后跟一个 `[生成出错中断]`。
- **缺陷 2 — 身份错乱**:参与者 transcript 含 moderator 的 `tool_use(nominate)` +
  `tool_result("Floor handed to X.")`;DB 证据 M3 的 thinking = "I need to respond as the
  moderator",text 自称 @moderator。根因是参与者视角可见仲裁 tool 交互。
- **缺陷 3 — 参与者拿仲裁工具**:`d2c7c32` 已修复(`participant_tool_defs` 过滤 +
  单测),重写必须保留。
- **wire 契约**:`assistant(tool_use)` + 紧邻的 `user(tool_result)` 是原子对
  (llm-contract.md §469)。OpenAI 400 / Anthropic 2013 都是"转录配对错乱"的表现。
- **`user_message_matches` 守卫**(工作区未提交):只覆盖用户消息持久化点,
  判据是启发式(内容比对 + 长度相等),治标不治本。**本次重写不保留,整体由新方案替代。**
- 主持人 turn 当前传 `Some(3)`(max_turns=3)→ 每次两次 LLM 调用,加重重复。

## Requirements

### R1 — 群聊编排不再把已落库消息当新消息(持久化去重)
- R1.1 每次 speaker turn 传入 `run_chat_loop` 的 transcript 尾部**不得是已落库消息**;
  编排层负责保证"进入 run_chat_loop 的尾部 user 消息一定是本轮新的"。
- R1.2 单 agent 聊天行为不变(它的尾部 user 消息天然是新的,照常持久化)。
- R1.3 DB 中同一 `tool_use_id` 只有一条 user-role tool_result 行(回归断言点)。

### R2 — 参与者视角过滤仲裁工具交互(transcript 隔离)
- R2.1 参与者看到的 transcript 剔除 moderator 的仲裁 tool_use / tool_result 对
  (nominate_speaker / end_discussion),同时保持
  assistant(tool_use)↔user(tool_result) 配对的原子性(剔除必须是成对的)。
- R2.2 参与者 system prompt(persona)保留"你是参与者、不是主持人"的显式声明(现有
  `participant_system_prompt` 逻辑保留,可增强措辞)。
- R2.3 moderator 看到的 transcript = 完整消息流(含自己的工具历史),跨轮连贯。

### R3 — 多轮编排跑通 + 不变量
- R3.1 moderator → participant → moderator 多轮完整跑通,无 `[生成出错中断]` 死循环。
- R3.2 moderator 的 `nominate_speaker` / `end_discussion` 信号继续通过
  `SharedTurnState` 拦截(保留现有拦截模式)。
- R3.3 每轮一个 speaker 只产生一次 LLM 调用(消除 max_turns=3 的双调用;若保留
  moderator 多调用语义则必须保证第二次调用不再重写 tool_result)。

### R4 — 集成测试(必须,不靠手动 UI 撞 bug)
- R4.1 用 `MockProvider`(脚本化流)写完整多轮群聊集成测试:
  moderator(mock 脚本:文本 + 调用 nominate_speaker + end_discussion)
  → participant(mock 脚本:发言)→ 断言:
  无 400 错误事件、DB 无重复 tool_result、每个 speaker 收到的 transcript 正确、
  participant 看不到仲裁 tool 块、system prompt 正确(参与者 = persona / moderator = 模板)。

### R5 — 回归
- R5.1 现有 `agent::tests_agent_loop` 全套(40 个)回归全绿 —— 单 agent 聊天行为不变。
- R5.2 `participant_tool_defs` 过滤测试保留。

## Non-Goals / 范围外

- 不改 wire 层 / provider 层 / `ChatMessage` 类型(不引入 seq 字段方案 C)。
- 不改 `run_chat_loop` 的签名(不新增 `skip_user_persist` 之类参数;它 35 个参数已够多,
  也不接受把不变量责任转嫁给调用方)。
- 不改前端(渲染、入口、modal 已在前序任务完成)。
- 不改 DB schema(不加迁移)。
- 不重写 `run_chat_loop` 本体 —— 只改编排层让它不再违反调用不变量;
  若实现中必须给 `run_chat_loop` 加小改动(如判定已落库以跳过重写),须设计.md 明确论证。
- 不做群聊产物二次整理 / 纪要(D1 维持 utterance 即纪要)。

## Acceptance Criteria

- [ ] **AC1**(集成测试,`#[cfg(test)]`):mock provider 跑通完整群聊多轮
  (moderator 提名 → participant 发言 → moderator 继续 → end_discussion 结束),
  断言无 `ChatEvent::Error`、无 `[生成出错中断]`。
- [ ] **AC2**:上述测试 session 的 DB 里,每个 `tool_use_id` 恰好一条 user-role
  tool_result 行;`messages` 行数 = 预期轮次(无重写、无多写)。
- [ ] **AC3**:participant 的 `sent_messages`(MockProvider 快照)不包含任何
  nominate_speaker / end_discussion 的 tool_use 或 tool_result 块;包含 moderator 的
  text 发言与 participant 彼此的发言。
- [ ] **AC4**:participant 的 `sent_systems` 首条 = 该 participant 的 persona
  (或默认参与者模板),不是 moderator 模板;moderator 的 = `moderator_system_prompt`。
- [ ] **AC5**:moderator 视角 `sent_messages` 包含自己的 nominate 工具块(跨轮连贯)。
- [ ] **AC6**:`agent::tests_agent_loop` 40 个回归全绿;`participant_tool_defs` 单测保留且绿。
- [ ] **AC7**:`cargo check` / `cargo clippy`(daemon bin)通过;`cargo test --lib agent::` 全绿。

## Notes

- 复杂任务:需要 design.md + implement.md 才能 `task.py start`。
- 本任务接管旧任务 `08-04-group-chat-tool-result-dedup`(in_progress,方案已被否决);
  提交后由用户归档旧任务。
- 提交风格沿用 `fix(group-chat): …`。
