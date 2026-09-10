# Design: wire 往返后融合相邻 tool_result 消息(from_wire 层)

## 决策

**在 `from_wire.rs::wire_messages_to_chat_messages` 出口处融合(fuse)相邻的「纯 tool_result user 消息」**,把 wire 往返产生的 `N 条连续 user 消息、每条单 tool_result` 合并回 `1 条 user 消息、N 个 tool_result 块`,恢复 PR3 wire 重构前的出站形态。

## 背景与动机

- 拆分点(事故链,详见 `research/root-cause.md`):`to_wire.rs:371` 把 user 消息里每个 `ToolResult` 提升为独立 `WireMessage::Tool`;`from_wire.rs:119` 把每个 `Tool` 映回独立 user 消息。原生 Anthropic 合并连续 user 消息故无害;wukaijin 的 deepseek 通道逐消息严格校验 → 单消息多 tool_use 的后续请求必 400。
- `scenario-provider-trait-anthropic.md` 规定 Anthropic 路径行为须与 **pre-PR2** legacy **1:1**(措辞与 spec 原文一致;PR2 引入 Provider trait、PR3 引入 wire 层,同一行为冻结边界);legacy 从不拆条,拆分本身就是 wire 往返引入的形态偏移,本设计即还原。

## 方案对比

| 方案 | 描述 | 取舍 |
|---|---|---|
| **A(采纳)** | `wire_messages_to_chat_messages` 出口处融合相邻纯-tool_result user 消息 | 在拆分artifact的源头还原;已验证该函数**仅** anthropic.rs:532 一个消费方,影响面精确;纯函数易测 |
| B(否决) | 出站 body 后处理(仿 `apply_deepseek_reasoning_fix` 再加一个 body patch) | 治标:先制造结构偏移再下游缝合;每加一个 Anthropic-wire 中继都要评估同一 body 形态,body 补丁会累积 |
| C(否决) | 改 `to_wire.rs` 让 `WireMessage::Tool` 承载多 result | 动 `WireMessage` 语义会波及 OpenAI adapter(`Tool` → role:"tool" 单条是其原生形态)与既有 strip/orphan 逻辑,风险面大 |

## 实现要点

1. **融合函数**(from_wire.rs,私有纯函数):

   ```rust
   fn fuse_adjacent_tool_results(msgs: Vec<ChatMessage>) -> Vec<ChatMessage>
   ```

   规则:扫描输出序列,把**连续**的「role=User 且 content=Blocks 且全部块为 ToolResult」的消息合并为一条(user 消息,块按原顺序拼接);遇任何非该形态消息(assistant、纯文本 user、UserBlocks 含 text/image)即断开当前融合段。`speaker`(tool_result 行恒为 None)、`is_error`、`resolved` 等逐块字段原样保留。

2. **挂接点**:`wire_messages_to_chat_messages` 的 `flat_map(...).collect()` 之后、返回之前。函数签名与调用方不变。

3. **边界与不变量**:
   - **引擎 tool-result 行的真实形态只有两种**:`[ToolResult×N]` 与 `[ToolResult×N, Text(loop hint)]`——tools.rs ⑬(chat_loop/tools.rs)刻意把 loop 提示 Text 块追加在**末尾**:若放开头,fan-out 会在 assistant 与 tool 消息之间插入 `user(text)`,OpenAI 严格序直接 400(该历史 bug 的回归锁见 tests_wire.rs `orphan_tool_call_order_flags_user_text_between_assistant_and_tool`)。即 result 块恒在 user 行最前,不存在 text 前置或交错的现行产生路径。
   - 尾部 hint 经 to_wire 后成为**独立的尾部 `User(text)`**。融合规则只融合纯 tool_result run,出站形态为 `assistant → user[TR×N](配对完整) → user(hint)`——紧邻消息含全部 result,尾部 user 是正常后续消息,两协议皆收(tools.rs ⑬ 注释对 `tool×N → user(text)` 形态的同一结论)。
   - **明确不把尾部 text 折进融合后的结果消息**:wire 层已丢失原始行边界,`Tool×N, User(text)` 无法区分「同行的 loop hint」与「下一行的独立 user 文本」——群聊 role_history 把其他发言者改写为 user 行,折入会污染 speaker 归属与语义。连续 user 消息两协议均合法,事故 400 只针对配对缺失,不针对连续 user 本身。
   - 既有防线互补且不受影响:`orphan_tool_use_ids`(to_wire.rs,兜「整个历史无 result」)、`orphan_tool_call_order`(wire 序向,user-text 插队检测——fuse 只合并不重排,输出序列不触发)、`llm-contract.md §Pair Atomicity`(C3,压缩/截断原子性)。本次修的是「result 存在但被 wire 拆开」这一此前无防线覆盖的形态。
   - OpenAI 路径零改动:openai.rs 消费 `WireRequest`(经 openai-wire-converter),不走 `wire_messages_to_chat_messages`。
   - `apply_deepseek_reasoning_fix` / `apply_speaker_prefix` 在融合后的 body 上运行,逻辑互不干扰(speaker 前缀只碰带 speaker 的消息,tool_result 行无 speaker)。

4. **测试设计**:
   - from_wire 单测:① 2 个相邻 `WireMessage::Tool` → 1 条 user 消息 2 块;② `Tool, User(text), Tool` 不跨文本合并;③ 单 `Tool` 原样;④ `[TR×2, Text(loop hint)]` 形态(即 `Tool, Tool, User(hint)`)→ 融合为 `user[TR×2]` + 独立 `user(hint)`,断言 hint **不被折入**结果消息。
   - anthropic 出站回归测(AC1):构造事故形态历史(assistant[thinking, tool_use×2] + user[tool_result×2]),断言出站 body 中紧邻 assistant 的**下一条** user 消息同时含两个 tool_use_id 的 tool_result——直接复刻 daemon.log 2026-09-10 18:37:37.166 的 400 场景。

## 风险与回滚

- 改动为纯函数 + 单挂接点,回滚 = 移除一次函数调用。
- 主要回归面是既有 wire/anthropic 测试组(`tests_wire.rs` / `tests_anthropic.rs` / anthropic.rs 内嵌 `deepseek_reasoning_fix_tests`),AC3 全量跑守门。
