# Research: DB 取证 — 群聊缺陷机制确认(2026-08-04)

> 本文件记录对线上 DB(`~/.local/share/dev.everlasting.app/everlasting.db`)的取证,
> 用于确认 handoff-prompt 中 3 个缺陷的**确切发生机制**,作为重写设计的依据。
> 全部会话都在 uncommitted `user_message_matches` 守卫之前产生(2026-08-04 早于 15:05 的工作区改动)。

## 0. 三个 group_chat 会话

| session | 首条消息时间 | 结果 |
|---|---|---|
| `01e7d154-…` | 04:01 | 复现(在守卫之前) |
| `905b3bf1-…` | 04:19 | 复现(在守卫之前) |
| `c40bb478-…` | 06:45 | 复现(在守卫之前) |

## 1. 失败级联(逐行证据,c40bb478)

```
seq 0  user  "开始第3次群聊测试,请随意发言"
seq 1  assistant moderator  [think][text "先请 M3…"][tool_use zvrh9bga1zkj_1 nominate M3]
seq 2  user  [tool_result zvrh9bga1zkj_1 "Floor handed to M3."]  ← 主持人 turn 内正常落库
seq 3  assistant moderator  [think][text "等待 M3 发言…"]          ← max_turns=3 的第二次 LLM 调用
seq 4  user  [tool_result zvrh9bga1zkj_1 …]                        ← 主持人 turn 再写一条同名 tool_result
       ↑ 这里就出现第一个"重复 tool_result"(同一 tool_use_id 两条 user-role 行)
seq 5  assistant M3  [think][text][tool_use phgvwv5oj4v8_1 nominate]  ← 缺陷3(修复前):M3 拿到 nominate 工具并调用
seq 6  user  [tool_result phgvwv5oj4v8_1 "nominate_speaker: only available in group chat" is_error]
       ↑ M3 turn 内拦截返回错误 tool_result(此时 M3 的 send 已成功——第1轮正常)
seq 7  user  [tool_result phgvwv5oj4v8_1 …]                        ← 用户消息持久化点把 M3 的 tool_result 当新消息再写一条(缺陷1)
seq 8  assistant moderator  [生成出错中断]                            ← 下一 LLM 请求带重复 tool_result → 400
seq 9  user  [tool_result phgvwv5oj4v8_1 …]                        ← 下一个 speaker turn 再写一条
seq 10…  assistant M3/D4F/moderator 交替 [生成出错中断] + 每个 assistant turn 后必跟一条同 id tool_result
       → 一直到 MAX_ORCHESTRATION_ROUNDS=30 停
```

- `01e7d154` 同构:seq 2 vs 4(`call_019fcaef8025…` 两条)、seq 6 vs 7(`call_019fcaef97e2…` 两条)。
- 每个 assistant `[生成出错中断]` 之后都**紧跟着一条新的同 id tool_result**(时间戳差 ~10ms,
  是下一 speaker 的 `run_chat_loop` 用户持久化点写的)。

## 2. 机制确认

### 缺陷 1 — 重复 tool_result 的确切写入路径

- `messages` 表 `UNIQUE(session_id, seq)`;`run_chat_loop` 每次进入**重新算**
  `seq = max(seq)+1`(chat_loop.rs 内部)。所以"再写一遍已落库消息"**不会撞唯一约束**,
  而是写一条**新 seq 的同内容行** —— 这是之前 dedup 任务没走 UNIQUE 约束路径的原因。
- 写重复行的三个点(全部在 `run_chat_loop` 内、全部把 `messages` 尾部 user 消息当新消息):
  1. **用户消息持久化点** chat_loop.rs:1001(`persist_turn`):reload 进来的尾部 tool_result 被当新消息。
  2. **result_blocks 持久化点** chat_loop.rs:4393:max_turns 退出时,把本次 LLM 轮
     `result_blocks`(含 reload 进来的 tool_use 的 tool_result)再写一次。
     - 该点 `speaker: None` 且 `seq` 自增 —— 群聊 4 个 turn 参数都是 `max_turns`(=3 或 1),
       每次都是 max_turns 退出 → **每轮必命中**。
  3. 合成 tool_result 点(cancel/error 路径,chat_loop.rs:2322/2381):群聊非 cancel/error,不常触发。
- **错误循环的动力**:主 loop 下一轮 `provider.send` 带重复 tool_result → provider 400
  (OpenAI 400 / Anthropic 2013)→ `[生成出错中断]` 落库 → 编排 reload → 下一 speaker
  再重复上面 2 个点 → 400 → … 直到 MAX_ORCHESTRATION_ROUNDS。

### 缺陷 2 — 身份错乱(M3 以为自己是 moderator)

- 参与者 turn 拿到的 transcript = reload 全部消息,含 moderator 的
  `tool_use(nominate)` + `tool_result("Floor handed to X.")`。DB 证据
  (01e7d154 seq 5, speaker=M3):thinking = "M3 has spoken. Now I need to respond as the moderator",
  text = "@moderator: …"(speaker 前缀注入的是 M3,但模型内容把自己当 moderator)。
- 根因 = 参与者视角可见 moderator 仲裁 tool 交互(与 D-A 决策一致:过滤)。
- 次要:参与者的 `participant_system_prompt`(persona)当前是**完全替换**系统提示,
  文本里有明确的"你是参与者"模板;但 `01e7d154` 的 M3 没有 persona(走默认模板),
  仍然搞错 —— 说明光是 prompt 不够,transcript 视角才是主因。

### 缺陷 3 — 参与者拿到 nominate_speaker(已修复,保留)

- 已在 `d2c7c32` 修复:`participant_tool_defs` 过滤两个仲裁工具 + 单测。
- DB 证据(修复前):M3 调用 nominate → `group_chat_state=None` 拦截返回错误 tool_result。

## 3. 其它事实

- seq 2 的 tool_result 块带 `duration_ms` 字段(`db::sessions.rs::record_tool_duration`
  IPC 补丁),重写后的 reload 路径要兼容(字段不影响判重 —— 判重基于 tool_use_id)。
- 主持人 turn 传 `Some(3)`(max_turns=3)导致两次 LLM 调用(seq 1 + seq 3)且每次
  max_turns 退出 → result_blocks 持久化点每轮都触发。这是重复的高频来源。
- 每条 tool_result 行的 `speaker` 列都是 `NULL`(user-role 不带 speaker;前端按 role 渲染)。
- `messages.speaker` 列已存在 + `load_session` 已读;`reload_messages` 已透传
  (group_chat_loop.rs:192)。
