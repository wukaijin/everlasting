# Design: 根治群聊 tool_result 跨 speaker 重复持久化

## 问题回顾

`run_chat_loop`(`chat_loop.rs:894-985`)无条件 persist `messages` 尾部
user 消息。它依赖隐式不变量"尾部 user 消息一定未落库":

- 普通聊天:前端 append 新 user 文本到 history 尾部(`chat.ts:1206`)→ 不变量成立。
- 群聊:`reload_messages` 把已落库 tool_result(role=user)reload 回来当尾部
  → 不变量被违反 → 重复 persist → 孤立 tool_result → OpenAI 400。

## 关键事实(代码已确认)

1. `chat_loop.rs:463` 已 `load_session` 拿到 `loaded_session.messages`
   (`Vec<MessageRow>`,每行有 `.seq` / `.role` / `.content`),目前**只用来算
   `next_seq`**(481-487 行取 max seq + 1),内容被丢弃。这正是"消息是否已落库"
   的现成判据,无需新参数或新查询。
2. `ChatMessage`(`llm/types.rs`)**没有 seq 字段**,只有 `role` / `content` /
   `speaker`。in-memory 消息无法直接说"我是 DB 的第 N 行"。
3. tool_result 的 content 形态:`MessageContent::Blocks(vec![ContentBlock::ToolResult{ tool_use_id, content, is_error }])`。
   `tool_use_id` 是稳定唯一键。
4. `reload_messages`(`group_chat_loop.rs:171-195`)用
   `serde_json::from_value(m.content)` 反序列化,与前端 JSON 反序列化路径产出
   同型 `MessageContent`。

## 方案对比

### 方案 A(否决):group_chat 层 strip 尾部已落库消息
在 `reload_messages` 后、每个 speaker `run_chat_loop` 调用前,剥掉尾部已落库的
user 消息。
- 优点:不碰核心 `chat_loop`。
- 缺点:**治标不治本**。隐式不变量依然脆弱(任何新调用方违反它就再爆)。每个
  speaker 调用点都要处理,易漏。剥掉尾部 tool_result 还可能让 moderator 看不到
  上一轮 tool 结果,破坏编排语义。否决。

### 方案 B(否决):加 `skip_user_persist: bool` 参数
`run_chat_loop` 已 35 个参数。再加一个:群聊调用点设 `true`。
- 缺点:签名臃肿;只是绕过不是修对;调用方要正确判断"何时 skip"(把不变量的
  责任转嫁给每个调用方,没消除根因)。否决。

### 方案 C(否决):给 `ChatMessage` 加 `db_seq: Option<i64>`
reload 时带上原 seq,`run_chat_loop` 见 `Some(seq)` 即跳过。
- 缺点:改 `ChatMessage` 类型 → 波及 wire 层序列化、所有构造点、前端 payload。
  改动面过大,风险/收益不匹配。否决(留作未来若方案 D 不够用的备选)。

### 方案 D(采纳):`run_chat_loop` 用 `loaded_session.messages` 自判已落库
在 `chat_loop.rs:894` 持久化块,用已在手的 `loaded_session.messages` 判断尾部
user 消息是否已落库;若是,跳过 persist 并把 `last_user_seq` 指向已有行 seq。

**判据设计**(核心,必须稳定):

不用整段 content 字节比对(thinking signature / 空白等不稳定)。用**尾部行
对齐 + tool_use_id 精确匹配**:

```
let db_last_user = loaded_session.messages.iter().rev()
    .find(|m| m.role == "user");
let already_in_db = match (db_last_user, messages.iter().rev().find(|m| m.role == Role::User)) {
    (Some(db_row), Some(mem_msg)) => user_message_matches(db_row, mem_msg),
    _ => false,
};
```

`user_message_matches` 判据(保守,宁可漏判=照常 persist,不可误判=丢消息):
1. **tool_result 消息**:若 `mem_msg.content` 的尾部 block 是 `ToolResult{ tool_use_id }`,
   且 `db_row` 反序列化后尾部 block 也是同 `tool_use_id` 的 ToolResult → 匹配。
   (tool_use_id 全局唯一,最稳。)
2. **纯文本 user 消息**:比对 `mem_msg.content.to_text()` 与 `db_row.text`
   完全相等(普通聊天的尾部新消息不会等于 DB 尾部行,因为 DB 尾部是更早的消息)。
3. 其余情况 → 不匹配(照常 persist)。

匹配时:
- 跳过 `persist_turn`。
- `last_user_seq = db_row.seq`(保证 FileInjections metadata update
  `update_message_metadata(&db, &session_id, last_user_seq, ...)` 仍指向正确行)。
- 不 bump `seq`(`seq` 仍是 `next_seq`,因为没写新行)。

不匹配时:走原逻辑(persist + `seq += 1`)。

## 为什么方案 D 安全(普通聊天回归)

普通聊天进入 `run_chat_loop` 时:
- `loaded_session.messages` 尾部 user 行 = 上一轮的用户消息(已落库)。
- `messages`(前端 history)尾部 user = 这一轮**新**消息。
- 二者内容不同(新消息文本 ≠ 旧消息文本)→ `user_message_matches` 返回 false
  → 照常 persist。**行为不变。**

唯一需要警惕的边界:用户重新发送(resend)**完全相同**的上一条消息文本。
此时 `mem_msg.to_text()` 可能等于 `db_row.text` → 误判为已落库 → 跳过 persist
→ 丢消息。`resend_seq` 路径(`chat_loop.rs:943`)正是处理"重发"的,但它在
persist 之后。**缓解**:判据额外要求 `db_row.seq == next_seq - 1`(即 DB 尾部
user 行就是最后一行)且该行是 tool_result(has_tool_results)或当前 `messages`
长度 == `loaded_session.messages` 长度(reload 场景的特征)。普通重发时
`messages` 比 DB 多一行(新消息),长度不等 → 不误判。见 implement.md 细化。

## 改动面

仅 `app/src-tauri/src/agent/chat_loop.rs`:
1. 新增私有函数 `user_message_matches(db_row: &MessageRow, mem_msg: &ChatMessage) -> bool`。
2. 修改 `chat_loop.rs:894-985` 持久化块:加 `already_in_db` 判断分支。
3. 新增单测覆盖:已落库 tool_result(跳过)、新文本消息(persist)、重发同文本(不误判)。

不改:`ChatMessage` 类型、wire 层、group_chat_loop、前端。

## 风险

- **R1 误判丢消息**(最高风险):判据过宽导致普通消息被跳过。缓解:判据保守
  (tool_use_id 精确匹配 + 长度/尾部行约束)+ 单测覆盖重发边界。
- **R2 FileInjections 指向错行**:`last_user_seq` 必须正确指向已有行。
  缓解:跳过时显式设 `last_user_seq = db_row.seq`。
- **R3 seq 漂移**:跳过 persist 时不能 bump seq。缓解:仅在 persist 分支 `seq += 1`。

## 验证策略

1. 单测:`user_message_matches` 各分支 + `run_chat_loop` 已落库 tool_result 跳过。
2. 回归:现有 `tests_agent_loop.rs` 全套(普通多轮工具对话)必须全绿。
3. 端到端:用 daemon 新建群聊,跑 moderator→participant→moderator 多轮,确认
   无 400、DB 无重复 tool_result(AC1/AC2)。
