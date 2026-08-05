# Design: 系统重写群聊编排(transcript 管理 + 持久化去重 + 角色隔离)

## 1. 问题重述

> 群聊编排把"已落库消息"当新消息喂回 `run_chat_loop`,违反它"进入时尾部 user
> 消息一定是新的"这个隐式不变量;参与者的 transcript 里混着 moderator 的仲裁 tool
> 交互,造成身份混淆。

## 2. 根因确认(DB 取证,见 `research/db-evidence.md`)

**唯一重复写入点 = `run_chat_loop` 的入口 user 消息持久化点(`chat_loop.rs:964-1075`)**
- `messages` 表有 `UNIQUE(session_id, seq)`,但 `run_chat_loop` 每次进入重新算
  `seq = max+1` → "再写一遍已落库消息"不会撞约束,而是写一条**新 seq 的同内容行**。
- 群聊里,上一 speaker 持久化的 `tool_result`(role=user)成为下一 speaker 入口
  transcript 的尾部 user 消息 → 被无条件再写一条 → 同一 `tool_use_id` 两条 tool_result
  → 孤立 tool_result → OpenAI 400 / Anthropic 2013 → `[生成出错中断]` → 循环到
  MAX_ORCHESTRATION_ROUNDS。DB 证据:同一 `tool_use_id` 出现 30+ 条行。
- **`result_blocks` 持久化点(`chat_loop.rs:4380`)不是重复来源**:它在每个
  tool-执行迭代的**末尾**、用**本轮新构建**的 result_blocks 写入一次(文本轮在
  `should_continue=false` 处提前 return,到不了这里)。它写的永远是新鲜 tool_result。
  handoff-prompt 中"result_blocks 路径未覆盖"是静态阅读的假设,与 DB 证据不符。

**身份错乱根因**:参与者 transcript 含 moderator 的 `tool_use(nominate)` +
`tool_result("Floor handed to X.")`(DB 证据:M3 thinking = "respond as the moderator")。

**参与者拿仲裁工具**:`d2c7c32` 已修复(`participant_tool_defs`),保留。

## 3. 设计决策

| # | 决策 | 依据 |
|---|---|---|
| D-A | **参与者的 transcript 过滤掉仲裁工具交互(成对剔除)**;moderator 看到完整 transcript | 用户确认;身份混淆根因直接消除 |
| D-B | 编排层保持 reload(安全化):每个 speaker 调用前 reload 一次;reload 的安全性由入口 guard(R1 关键)保证 | run_chat_loop 返回 `()`,编排层无法知道它 append 了哪些行 → reload 是唯一的 resync 手段;reload 本身无害,有害的是"reload 后无条件重写尾部 user 消息" |
| D-C | 参与者视图剔除必须是**成对**的(assistant(tool_use) ↔ 紧随的 user(tool_result)),不破坏 llm-contract.md §469 Pair Atomicity | 协议契约 |
| D-D | `run_chat_loop` 只做**一处最小精确改动**:入口 user 持久化点加 guard(scope 到 `group_chat_state.is_some()`) | 这是唯一的重复来源;改动精确、不影响普通聊天;不加新参数(35 参已够多) |
| D-E | moderator 每轮 `max_turns=Some(1)`(单轮)、participant `Some(1)` | 08-04 follow-up(用户确认"moderator 单轮"):旧 `Some(3)` 让 moderator 在第二轮烧一次 LLM 调用产出一段第一人称仲裁叙述 filler("已把话筒交给 X，等待发言…"),DB 证据(b144cc2a seq 3-4)显示弱参与者把这段误认成自己的声音(身份混淆)。单轮 = 一轮一条消息(文本+仲裁 ToolCall 同流),turn 结束后直接回到编排循环;无 filler、无二次调用。 |
| D-F | 旧任务未提交的启发式 guard(`user_message_matches` + 长度判据)被**替换**为 D-D 的精确版本 | 旧版判据(内容比对 + 长度相等)在过滤后的视图上失效(视图行数 < DB 行数) |

## 4. transcript 视图

记 `full` = reload 得到的 `Vec<ChatMessage>`(DB 行、seq 升序)。每个 assistant 行与
紧随的 user 行形成 tool_use↔tool_result 原子对(同 tool_use_id)。

**View-1 moderator(每轮进入)** = `full` 的克隆(moderator 是唯一调用仲裁工具的实体,
它的仲裁对是"自己的历史",保留)。尾部 = 上一参与者最后一行 assistant 文本。

- **View-2 participant(每轮进入)** = `participant_view(&full)`:
- 扫描 `full`,遇到含仲裁工具 `ToolUse` 块的 assistant 行:
  - 保留该行**非工具块**(thinking / text);
  - 记录被剔除的 `tool_use_id` 集合,并**跳过紧随的 user 行**(该行含对应 tool_result);
  - 若该行只剩工具块(无 think/text)→ 整行丢弃(仍跳过紧随 user 行)。
- 其余行(人类消息、moderator/participant 文本)原样通过。
- 不变量:剔除后任何残存的 assistant(tool_use) 与其 tool_result 仍相邻配对;
  不产生孤儿 tool_use / 孤儿 tool_result。
- 群聊参与者不会收到 dispatch 等工具,所以实际效果 = 仅剔除仲裁对。

> 实现提示:`full` 由 DB 轮询重建;仲裁对在 `full` 中始终相邻(由 turn 内执行顺序
> 保证),状态机一行一行处理即可,不需要回溯。

**08-04 follow-up(参与者身份护栏,用户确认"Prompt 强化")**:wire 层 speaker 标注
(OpenAI `name` / Anthropic `@name:` 前缀)压不住弱模型 —— DB 证据(b144cc2a seq 4)
参与者 M3 thinking = "I (as the system) am prompting the conversation. The first turn
was my moderator opening",回复以 `@moderator:` 开头。因此 `participant_system_prompt`
在 persona 与默认模板**之后追加**显式角色边界块:

```
## Group-chat roles (read carefully)
- You are <name> — one of the PARTICIPANTS. A separate moderator runs
  the discussion and assigns turns; you never do.
- The moderator's messages are NOT yours — never reply in the
  moderator's voice and never act as the moderator (no summing up,
  no handing the floor, no nominating speakers, no opening/closing).
- Only ever reply as <name>. Do not start your reply with another
  speaker's label (e.g. never prefix with `@moderator:`), and do not
  refer to yourself in the third person.
- Just say your own piece on the topic and respond to what others said.
```

persona 只描述人设,不防御角色混淆 → 护栏块对**两种来源**都追加。

## 5. 入口 user 持久化点 guard(R1 关键,chat_loop.rs)

**规则**:`run_chat_loop` 入口的 user 消息持久化(chat_loop.rs:964-1075),
当且仅当满足以下**全部**条件时跳过 persist(并把 `last_user_seq` 指向已存在的行、
不 bump seq、跳过 resend audit):

1. `group_chat_state.is_some()`(本次调用是群聊 speaker);
2. `messages` 的尾部 user-role 消息与 `loaded_session.messages` 中**任一** user-role
   行的内容匹配(`user_message_matches` 复用:tool_result 按 tool_use_id 精确匹配,
   纯文本按字节相等)。

**为什么可靠**:
- 群聊编排的每个 speaker 入口 transcript 都是从 DB 行构建的视图 → 其尾部 user 消息
  必是某条已落库行 → 内容匹配 → 跳过 ✓。
- round 1 / 人类插话重入:入口 `messages` = 前端 history + **新的**人类文本消息
  (未落库)→ 内容不匹配任何行 → 照常 persist ✓。
- 普通聊天 `group_chat_state=None` → 不进入 guard,行为完全不变 ✓。
- 过滤后的 participant 视图(行数 < DB 行数)不再误判:判据是"内容匹配任一 DB 行",
  不是旧版的行数/尾部行对齐。

**为什么不用长度判据(旧版缺陷)**:旧版要求 `messages.len() == db.len()`,
在过滤视图上恒假 → 误判为新消息 → 重复 persist。内容扫描没有这个限制。

**已知边界(cosmetic,可接受)**:群聊中人类**重发完全相同的文本**(需先 cancel 再发,
罕见)会被判为已落库而跳过 → 该行不落库。后果只是转录缺一行,不破坏 tool 配对、
不触发 400。文档化 + 注释说明。

**result_blocks 持久化点(4380)/ 合成 tool_result 点(2322/2381)**:不改。
DB 证据 + 代码走查确认它们只写本轮新鲜构造的 tool_result。

## 6. 新编排流(group_chat_loop.rs 重写)

```
run_group_chat_loop(...) {
    turn_state = SharedTurnState::new();
    for round in 0..MAX_ORCHESTRATION_ROUNDS {
        if token.is_cancelled() { break; }

        // ---- 1. moderator turn(进入 = View-1)----
        let full = if round == 0 { messages.clone() }   // chat_inner 传入(尾部 = 新人类消息)
                   else { reload_messages(&db, &session_id).await };
        if let Some(provider) = &moderator_provider {
            run_chat_loop(tool_defs.clone(), provider, ..., full, ...,
                Some(3) /* max_turns */, ..., Some(moderator_prompt), ...,
                Some(turn_state), Some("moderator")).await;
        }

        // ---- 2/3. 读 turn state ----
        (next_speaker, ended) = turn_state.take();
        if ended { break; }
        nominee = next_speaker.or(round-robin fallback);
        participant = gc_ctx.participant_by_name(&nominee) else { continue; };

        // ---- 5/6. participant turn(进入 = View-2)----
        let full = reload_messages(&db, &session_id).await;
        let view = participant_view(&full);              // D-A 过滤
        if let Some(provider) = participant_provider {
            run_chat_loop(participant_tool_defs(&tool_defs), provider, ..., view, ...,
                Some(1) /* max_turns */, ..., Some(participant_prompt), ...,
                None /* group_chat_state */, Some(participant.name)).await;
        }
        // ---- 下一轮:reload 由第 2 次循环的 round>0 分支完成 ----
    }
}
```

**为什么这修复缺陷 1**:每个 speaker 入口 transcript 的尾部 user 消息要么是
(round 0) 新的 `loaded_session` 之后的人类消息 → guard 照常持久化;要么是已落库行
(reload / 过滤视图)→ guard 跳过 → **不再产生重复行**。`result_blocks` 点只写
新鲜结果,天然不重复。

**为什么修复缺陷 2**:participant 视角不再包含仲裁工具交互(D-A),身份混淆的
主要诱因被移除;`participant_system_prompt` 的"你是参与者"声明保留。

**为什么修复缺陷 3**:`participant_tool_defs` 保留(d2c7c32)。

**人类插话(D9)**:cancel 中断编排 → 前端重新 send → `chat_inner` 重新进入
`run_group_chat_loop`,`messages` = 前端最新 history(尾部 = 新人类消息)→
round 0 分支 → 照常持久化人类消息。行为与旧实现一致。

## 7. 兼容性 / 回归

- 普通 chat:`chat_inner` 不走 `run_group_chat_loop`;`group_chat_state=None` →
  guard 不生效。零影响。
- `reload_messages` 保留(编排层 resync 用);`participant_view` 新增纯函数。
- 旧启发式 guard 的代码 + 其 6 个单测被替换为 D-D 精确版(保留 `user_message_matches`
  辅助函数,去掉长度判据,加 scope)。
- 无 DB schema 变更、无迁移、wire 层不变、前端不变。
- `run_chat_loop` 行为:群聊 speaker 调用在 guard 生效时跳过入口 user 重写;普通调用
  逐字节不变(guard 第一条件短路)。

## 8. 测试设计(集成,`#[cfg(test)]`)

新文件 `app/src-tauri/src/agent/tests_group_chat.rs`(仿 `tests_agent_loop`)。

**Harness**:复用 `tests_common::make_harness` + 新 helper `make_group_chat_harness`
(把 session 标记为 group_chat + 写 `{participants:[{name,model,persona_md?},…]}` metadata,
见 implement.md)。

**Mock 编排**:三个模型各一个 `MockProvider`,构造 `ProviderCatalog` 注入
`worker_catalog: Some(Arc::new(RwLock::new(catalog)))`。`resolve_provider` 只读 catalog。

**Mock 脚本**(moderator 单轮 = **1 次 send**:`max_turns=Some(1)` 下,一轮消息流 =
Start → Delta(发言) → ToolCall(仲裁) → Done{tool_use};执行完工具后
`max_turns` 已达 → max_turns 退出 → 回到编排循环,无第二轮):
- moderator round0:`mod_tool_turn("c1", nominate_speaker, {name:"M1"}, "主持人发言")`
- M1:`text_turn("我是 M1")`
- M2:`text_turn("我是 M2")`
- moderator round1:`mod_tool_turn("c2", nominate_speaker, {name:"M2"}, "主持人:请 M2")`
- moderator round2:`mod_tool_turn("c3", end_discussion, {}, "主持人:结束")`
- 调用数断言:moderator=3、M1=1、M2=1。

**断言点**:
- 无 `ChatEvent::Error`(400 落地为 Error 事件 → 断言列表为空)。
- DB 每 tool_use_id 恰 1 条 tool_result(`c1`/`c2`/`c3` 各 1)。
- M1 的 `sent_messages()` 各条**不含** nominate/end 的 ToolUse/ToolResult 块(AC3)。
- M1 `sent_systems()[0]` **以 persona 开头 + 含身份护栏块**;M2 以默认模板开头 +
  含身份护栏块;moderator 含模板(AC4)。
- moderator 的 `sent_messages` 含自身 nominate tool_use(AC5)。
- 人类消息 "hello" 在 DB 中恰 1 条(round-0 持久化 + guard 不重复)。
- participant 视角的人类消息可见(透传未丢失)。

**回归**:`cargo test --lib agent::tests_agent_loop`(40 个)全绿(AC6)。

## 9. 风险

| 风险 | 等级 | 缓解 |
|---|---|---|
| 群聊内人类重发相同文本被 guard 跳过 | 低 | 罕见(cancel+重发同文本)+ 后果 cosmetic(缺一行文本,不触发 400);注释 + 文档说明 |
| moderator 单轮后每轮一次 LLM 调用,编排节奏变化 | 低 | 用户确认"moderator 单轮";moderator 每轮只发一条(文本+仲裁),点名/结束语义不变(由 SharedTurnState 驱动);比旧 2-3 次调用更快、更稳 |
| `participant_view` 剔除破坏配对 | 中 | 纯函数 + 状态机 + 单测覆盖四类输入(含仲裁对 / 无仲裁对 / 纯工具行 / 连续两个 moderator 轮) |
| mock 脚本与真实调用数不符 | 低 | mock 耗尽 = InvalidRequest → 测试失败,暴露偏差(设计特性) |
| guard 误判新消息为已落库 | 低 | 群聊内"新消息"只有两类:round0/插话的人类文本(不匹配任何 DB 行)+ 新 tool_result(由本 turn 构造,入口时不存在) |
| 参与者仍误认自己是 moderator(prompt 护栏无效) | 低(残留,已尽力) | wire 标注 + 身份护栏块 + moderator 单轮三重防线;弱模型仍可能犯错 → 后续可加内容级作者前缀(方案 3,见 §4 follow-up) |

## 10. 回滚

- 核心改动集中在 `group_chat_loop.rs`(编排)+ `chat_loop.rs`(入口 guard)+ 新测试文件。
  `git revert` 对应 commit 即回滚;`run_chat_loop` 其余逻辑未动 → 单 agent 零风险。
- 若集成测试暴露 guard 判据问题,回退到"编排层完全内存 transcript(不 reload)"
  变体(D-B 的极端形态),guard 可随之简化;但需先确认 resync 手段(见 §3 D-B)。
