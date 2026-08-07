# 群聊参与者调研能力回归 + 结束总结入文本流

## Goal

修复 08-07 工具收敛 + prompt 加固(08-07-group-chat-toolset-and-identity)产生的**两个过度调整副作用**:

**(A) 参与者从不调用调研工具**——工具白名单已下发(`group_chat_tool_defs(.., false)` =
`read_file/grep/glob/list_dir/web_fetch`),但 participant prompt 的 identity-guard
禁令过重,弱模型把"禁止用工具夺权"过度泛化为"讨论轮不调任何工具"。参与者观点停留在
假设/推理层面,无法实证(用户问"是 bug 还是主动性",结论是**主动性,且是 prompt 压制**)。

**(B) 结束总结不进文本流**——moderator 调 `end_discussion` 时把完整总结全文放进
`input.summary` 工具参数,text 层只有一句"我来做个总结收尾"。前端无 end_discussion
专属渲染(全仓 grep 零命中),总结只能展开工具卡片看 output,transcript 消息流里没有。

> **好基线**:本次 session(6c00f286)没有再出现 8be4687f 的同模型串台——seq 1-9 是纯
> API 错误(用户确认忽略),seq 10-37 全程角色认知正常。说明工具收敛 + prompt 加固对
> 串台确有缓解。本任务的两个问题是在这个好基线上发现的**新一类**问题。

## Background / 事实(DB 取证,坐实)

取证 DB:`/root/.local/share/dev.everlasting.app/everlasting.db`
取证 session:`6c00f286-3c1e-4785-8187-783bd3611b81`(2026-08-07 08:34-08:38,最新一次群聊)
配置:moderator(deepseek b8d0abc2)+ M3(42274366)+ GLM5.2(fd926ed1)+ D4F(deepseek b8d0abc2)

### 问题 A — 参与者零工具调用(证据)

工具调用总览(全 session):**moderator 调了 9 轮工具**
(list_dir/read_file/grep/glob,seq 10-35),**3 个参与者(M3 seq 20 / GLM5.2 seq 23 /
D4F seq 36)全部零工具调用**(has_tool_calls=0)。

**决定性证据**:M3 的 thinking(`seq 20`)明确写:

> *"I don't need to use any tools - this is a discussion turn."*

且 M3 的 text 末尾问:*"要不要我先去看一下 `python-bridge.js` 里的具体实现,确认下
我说的 3、4 两条是不是真问题?"*——**它想调研,但自己判断"讨论轮不该调工具",主动放弃。**

### 问题 B — 总结不进文本流(证据)

`seq 37`(moderator 收尾)的消息结构:
- `thinking`:完整推理(正常)
- **`text`:"D4F 的收束很完整,三位的视角已经互相咬合…我来做个总结收尾。"**(仅此一句)
- **`tool_use: end_discussion`, `input.summary`:** 600 字四维度完整总结(架构/安全/工程/产品)

→ 总结全文在**工具参数**里(给系统/日志用),text 层是空的。前端 `end_discussion`
无专属渲染(grep `streamController.ts` + `components/chat/*.vue` 零命中),就是普通
工具卡片 → 用户只能展开卡片看 output。

### 根因(两层,都是 08-07 的过度调整)

1. **问题 A**:`participant_system_prompt` 的 identity-guard 块(`group_chat_loop.rs:403-413`,
   08-07 R3 为治 seq 9 夺权加的禁令):
   > "Do NOT invent or invoke system tools / skills to legitimize hosting — if you find
   > yourself wanting to 'run' the discussion, stop: you are a participant. Speak only
   > your own view, then end your turn."
   本意是禁止"用工具夺权"(use_skill/update_checklist 幻觉),但措辞让弱模型把**所有**
   工具调用都当"越权"。工具白名单下发了却成摆设。08-07 R2 决策"参与者保留调研类工具
   (取材实证)"与 prompt 的防御姿态**自相矛盾**。

2. **问题 B**:moderator 的 `moderator_system_prompt`(`group_chat_loop.rs:137-175`)教
   moderator 用 `end_discussion({summary})` 收尾,但没说**总结正文要写进文本**。模型
   自然地把内容塞进工具参数(那是它唯一"确定会被记录"的地方)。前端又没有
   end_discussion 专属渲染。

## Requirements

### R1(问题 A)— participant prompt 区分"调研工具"与"夺权工具"

修订 `participant_system_prompt` 的 identity-guard 块,把禁令从"别碰任何工具"改为
"**禁止用仲裁/调度类工具夺权,但鼓励用调研类工具取材实证**":

- 保留:禁止 nominate/end/自建调度/冒充主持人。
- 修订:"Do NOT invent or invoke system tools" → 明确为"don't use scheduling tools
  (nominate_speaker / end_discussion / checklists / skills) to take over",同时新增
  **鼓励句**:"You MAY use the research tools (read_file / grep / glob / list_dir /
  web_fetch) to verify your points — grounded claims beat speculation."
- 动机:08-07 R2 决策(参与者保留调研工具取材实证)本意如此,只是 prompt 没跟上。

### R2(问题 B)— end_discussion 总结入文本流

方案空间(需 brainstorm 收敛,倾向 a):
- **(a) prompt 引导 + 前端兜底**:moderator prompt 明确"把总结写在你的文本回复里,
  `end_discussion.summary` 只放简短摘要";前端为 end_discussion 卡片增加专属渲染
  (总结区/折叠卡片),或把 summary 提取为 transcript 尾部一条可见的总结消息。
- (b) 落库时提取:end_discussion 拦截器(`chat_loop.rs:3746-3790`)把 `input.summary`
  提取为一条单独的消息/区块。
- (c) 仅前端:不改 prompt,前端把工具卡片的 summary 渲染成更显眼的总结样式。

### R3 — 回归:不破坏 08-07 已落地修复

- 工具白名单 / 无 streak / prompt 加固 / 串台缓解(本 session 已验证)不破坏。
- identity-guard 的"禁止夺权"核心不丢(R1 只改措辞,不改语义)。

## Out of Scope

- 同模型串台治理(单列 `08-07-group-chat-same-model-crossover`,未 start)。
- API 错误重试(seq 1-9 的 `[生成出错中断]`,用户确认忽略)。
- 参与者调工具后的工具结果共享语义(那是串台任务 R3 的范围)。

## Acceptance Criteria

- [ ] AC1(R1):participant prompt 含"research tools 可用 + scheduling tools 禁用"的
      区分;既有 prompt 回归测试(identity-guard 核心断言)仍绿。
- [ ] AC2(R1):契约级测试——participant prompt 鼓励调研工具(read_file 等),
      禁止仲裁工具(nominate/end),不鼓励技能/清单类。
- [ ] AC3(R2):选定方案落地——end_discussion 总结在 transcript/前端可见
      (文本流或专属渲染,按 brainstorm 收敛)。
- [ ] AC4:`cargo test --lib` + `pnpm test` + clippy 零警告 + vue-tsc 零错误。
- [ ] AC5(人工):真模型重跑群聊,参与者至少出现一次调研工具调用;
      讨论结束的总结在文本流可见,不必展开卡片。

## Constraints / 约束

- 不改 DB schema / 不改群聊时序 / 不改 wire 层。
- 不破坏 08-07 全部已落地修复(工具白名单 / 无 streak / prompt 加固 / 5 层防御)。
- 串台任务(`08-07-group-chat-same-model-crossover`)未 start,本任务与其无依赖冲突
  (都改 `group_chat_loop.rs` 的 prompt,但改的是不同段落;若并行需注意合并)。

## Notes

- 轻量任务:可能只需改 prompt 文案 + 前端渲染,PRD-only 可能够。若 R2 选 (b)
  (落库提取)则偏复杂,需 design.md。
- 跨任务:与 `08-07-group-chat-same-model-crossover`(per-role history)在
  `group_chat_loop.rs` 有重叠文件,串行执行更稳。
