# Root cause: deepseek-flash 群聊「生成中断」= wire 拆条 × wukaijin deepseek 通道严格校验

调查时间:2026-09-11。全部证据来自生产 DB + daemon.log,可复按。

## 1. 事故现场

- 会话 `caa5020a-641a-4e68-b8b6-9f3a856c730f`(MCP 发起,议题「评审 apps/web 前端的架构与工程规范」,2026-09-10 18:36–18:45 UTC)。
- 参与者:架构师 = glm-5.3(`7df7b1fa`)、**前端专家 = deepseek-flash(`12a4ac4f`)**、产品经理 = glm-5.3-flash(`a92dee22`);全员走同一中继 `https://api.wukaijin.com/v1/messages`。
- 前端专家三轮全部 `[生成出错中断]`(messages 表 seq 19 / 22 / 40,内容仅 35 字节 ERROR_MARKER),全程零实质发言。会话本身正常收尾(18:44:58 `moderator ended discussion round=17`)。

## 2. 报错

```
status=400 invalid_request_error
messages.8: `tool_use` ids were found without `tool_result` blocks immediately after:
call_01_bRrwsJ0l2oDF2Zaa7F5k8145.
Each `tool_use` block must have a corresponding `tool_result` block in the next message.
```

daemon.log 出现 8 次(3 轮失败 + 重试),行号 11294 / 11310 / 11431 等。

## 3. 时序还原(round 7,前端专家第一次被点名)

| 时间 (UTC) | 事件 |
|---|---|
| 18:37:36.269 | 请求 1 发出(model=deepseek-flash) |
| 18:37:36.46–.93 | 流式正常:thinking + **同一条 assistant 消息里 2 个 tool_use**:`call_00_...`(read_file `docs/specs/web.md`)+ `call_01_...`(list_dir `apps/web`) |
| 18:37:36.98 | 两个工具都过权限(Tier 4 silent Allow)并执行 |
| 18:37:37.01 | seq 17(assistant:thinking+2×tool_use)、seq 18(user:**2×tool_result 同一条消息**)落库——**DB 状态完全正确** |
| 18:37:37.03 | 请求 2(带结果)发出 |
| 18:37:37.17 | **400**,即上述报错;`chat_loop::drive` 报 `category=InvalidRequest turn=2`;落 ERROR_MARKER(seq 19) |

关键对照(同会话、同中继、同代码路径):

- moderator(glm-5.3-flash)seq 13 单消息 **2 个 tool_use**(glob+grep),后续请求 18:37:06.884 → **流正常打开**;
- 架构师(glm-5.3)seq 25/27 单消息 **4 个 tool_use**,后续多轮全部通过;
- 产品经理(glm-5.3-flash)seq 49 单消息 3 个 tool_use,通过;
- **唯独 deepseek-flash 三次全 400**,且重试轮(round 8 / round 10)的**第一个**请求(18:37:45.809、18:39:39.814)发出 ~120ms 即 400,连流都没开——模型根本没机会说话。

## 4. 根因链

1. **deepseek-flash 单条 assistant 消息发 2 个 tool_use**(完全合法的 Anthropic 行为)。
2. 引擎正确执行工具并落库:一条 user 消息携带 2 个 tool_result(seq 18)。
3. **Anthropic adapter 出站前的 wire 往返把这条消息拆开了**:
   - `app/src-tauri/src/llm/provider/wire/to_wire.rs:371` — 每个 `ContentBlock::ToolResult` 被提升为独立 `WireMessage::Tool`;
   - `app/src-tauri/src/llm/provider/wire/from_wire.rs:119` — 每个 `WireMessage::Tool` 映回**独立的** `role:"user"` 消息(单 tool_result 块);
   - `app/src-tauri/src/llm/provider/anthropic.rs:521-528` 注释明确承认这是往返唯一的结构性变化,并默认无害(原生 Anthropic 会合并连续 user 消息)。
4. 出站 payload 尾部实际形态:

   ```
   messages.8   assistant [thinking, tool_use call_00, tool_use call_01]
   messages.9   user [tool_result call_00]   ← 拆出的第一条
   messages.10  user [tool_result call_01]   ← 拆出的第二条
   ```

5. wukaijin 中继的 **deepseek 通道逐消息严格校验**「tool_use 的 result 必须在紧邻的下一条消息」→ messages.9 只有 call_00 的 result,call_01 成孤儿 → 400。报错**只点名第二个 id(call_01)**与该形态精确吻合(glm 通道合并/宽容,故同样的拆分形态全部通过)。

## 5. 为什么重试救不回来

群聊 `role_history`(`app/src-tauri/src/agent/group_chat_prompts.rs:176`)的不变量:发言者**自己的 assistant 行原样保留**(thinking signature 回传需要)。毒源 seq 17+18 永久留在前端专家的自身历史里,后续每次重试的第一个请求就重放同样的拆分对 → 必然 400 → 三振落 ERROR_MARKER。这是「重试逻辑对该错误形态结构性无效」的实例。

## 6. 顺带记录

- 报错索引 `messages.8` 与重建后的消息序号精确对齐(role_history 产出 8 条前导消息后正是该 assistant 行),佐证以上形态还原。
- 会话尾部(18:44:29–58)moderator 收尾轮因 `max_turns=1` 在核实锚点后被截断,属已知设计(max turns),与本次事故无关。
- 同题更早两场(18:23 `b87de4c5`、18:32 `66b0cc6e`)前端专家分别为其他模型,未触发;18:36 这场是换入 deepseek-flash 后首发即中。

## 7. 取证附记(与本次 bug 无关,防止误读)

- **DB 里 tool_result 块存在两种 key 顺序**(`type,tool_use_id,content` 与 `content,...,tool_use_id,type` 混排):来源是 `db/sessions/messages.rs` 的 `record_tool_duration` 补丁——它把 `messages.content` 整体解析成 `serde_json::Value` 再写回,serde_json 的 Map 按 key 字母序(BTreeMap 语义)重排。反序列化按 key 取值,顺序无关紧要;这**不是**第二条序列化路径,与本次 400 无关。列为附记是免得后人取证时误判。
- **引擎 tool-result 行只有两种形态**:`[ToolResult×N]` 与 `[ToolResult×N, Text(loop hint)]`——chat_loop/tools.rs ⑬ 刻意把 loop 提示追加在末尾(放开头会在 wire fan-out 后把 `user(text)` 插进 assistant 与 tool 消息之间,OpenAI 严格序 400;tests_wire.rs `orphan_tool_call_order_flags_user_text_between_assistant_and_tool` 是该历史 bug 的回归锁)。即 result 块恒在行首,这是「融合相邻纯-tool_result 消息即可满足紧随其后」前提的代码级依据。
