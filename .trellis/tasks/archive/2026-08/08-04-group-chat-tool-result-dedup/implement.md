# Implement: 根治群聊 tool_result 跨 speaker 重复持久化

## 前置

- 已读 `prd.md` + `design.md`。
- 采纳方案 D(在 `run_chat_loop` 用 `loaded_session.messages` 自判)。

## 执行步骤

### 1. 新增 `user_message_matches` 判据函数(`chat_loop.rs`)
位置:持久化块(894 行)之前,或 helpers 区。

```rust
/// 判断 in-memory 尾部 user 消息是否已落库于 `db_row`。
/// 保守判据:宁可返回 false(照常 persist),不可误判 true(丢消息)。
/// 见 design.md §判据设计。
fn user_message_matches(db_row: &crate::db::MessageRow, mem_msg: &ChatMessage) -> bool {
    // 仅 user-role
    if db_row.role != "user" { return false; }
    // tool_result:用 tool_use_id 精确匹配(全局唯一,最稳)
    let mem_tool_ids: Vec<&str> = mem_msg.content.blocks()
        .iter().rev()
        .filter_map(|b| if let ContentBlock::ToolResult { tool_use_id, .. } = b { Some(tool_use_id.as_str()) } else { None })
        .collect();
    if !mem_tool_ids.is_empty() {
        let db_content: MessageContent = serde_json::from_value(db_row.content.clone())
            .unwrap_or_default();
        let db_tool_ids: Vec<&str> = db_content.blocks().iter().rev()
            .filter_map(|b| if let ContentBlock::ToolResult { tool_use_id, .. } = b { Some(tool_use_id.as_str()) } else { None })
            .collect();
        return !mem_tool_ids.is_empty() && mem_tool_ids == db_tool_ids;
    }
    // 纯文本:字节相等(普通新消息不会等于 DB 旧消息)
    mem_msg.content.to_text() == db_row.text
}
```

(实际 API 以 `MessageContent::blocks()` 是否存在为准;若它是 enum,用对应方法。实现时核对。)

### 2. 修改持久化块(`chat_loop.rs:894-985`)
在 `if let Some(last_user) = messages.iter().rev().find(...)` 分支内,
persist 之前加判断:

```rust
// 找 DB 尾部 user 行,判断 in-memory 尾部 user 消息是否已落库。
// 群聊 reload 路径会把已落库的 tool_result 当尾部送进来,这里识别并跳过,
// 避免重复持久化产生孤立 tool_result(OpenAI 400)。
let db_last_user = loaded_session.messages.iter().rev().find(|m| m.role == "user");
let already_in_db = match (db_last_user, ) {
    (Some(db_row),) if user_message_matches(db_row, &msg) => {
        // 额外约束:仅当 DB 尾部 user 行是最后写入的行之一(reload 场景特征)
        // 且 in-memory messages 数量 == DB 行数量(纯 reload,无新消息 append)。
        // 普通重发同文本时 messages 比 DB 多一行 → 不满足 → 不误判。
        messages.len() == loaded_session.messages.len()
    }
    _ => false,
};

if already_in_db {
    // 跳过 persist;last_user_seq 指向已有行,不 bump seq。
    let user_seq = db_last_user.unwrap().seq;
    // (跳过 persist_turn / resend audit / seq += 1)
    (Some(msg.content), user_seq)
} else {
    // 原逻辑:persist + resend audit + seq += 1
    ...
}
```

**关键不变量**:
- `already_in_db == true` 时:**不 persist、不 bump seq**、`last_user_seq = db_row.seq`。
- `already_in_db == false` 时:字节级行为与改动前完全一致(回归保护)。

### 3. 单测(`chat_loop.rs` 测试模块 或 tests_agent_loop.rs)
- `user_message_matches`:tool_result 同 id 匹配 / 不同 id 不匹配 / 纯文本相等 / 不等。
- `run_chat_loop`:模拟 reload 场景(messages == DB rows,尾部是 tool_result)
  → 验证不产生新 user 行(seq 不变,DB 行数不变)。
- `run_chat_loop`:普通新消息 → 验证照常 persist(回归)。
- 重发同文本边界:`messages.len() > loaded len` → 不误判。

### 4. 验证命令
```bash
cd app/src-tauri
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib agent::chat_loop   # 新单测
PKG_CONFIG_PATH="..." cargo test --lib agent::tests_agent_loop  # 回归
PKG_CONFIG_PATH="..." cargo clippy --bin everlasting-daemon -- -D warnings
PKG_CONFIG_PATH="..." cargo check --bin everlasting-daemon
```

### 5. 端到端(AC1/AC2)
- `./scripts/daemon.sh restart`(用新二进制)
- 新建 group_chat,跑多轮对话。
- `sqlite3 ... "SELECT seq,role,speaker,substr(text,1,40) FROM messages WHERE session_id=... ORDER BY seq"`
  确认无重复 tool_result、无 `[生成出错中断]` 死循环。

## 回滚点

- 每步后 `cargo check` 通过再继续。
- 若回归测试红:整个改动是 `chat_loop.rs` 单文件,`git checkout -- app/src-tauri/src/agent/chat_loop.rs` 回滚。
- 若端到端仍 400:回到 design 重新评估方案 C(给 ChatMessage 加 seq)。

## Review Gates

- [ ] design.md 方案 D 判据被确认(tool_use_id 匹配 + 长度约束)。
- [ ] 单测全绿(含重发边界)。
- [ ] tests_agent_loop.rs 全套回归绿。
- [ ] clippy clean。
- [ ] 端到端群聊多轮跑通(AC1/AC2)。
