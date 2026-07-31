# 群聊 group chat (主持人 LLM 主持 + 多 LLM 真对话)

> 状态:**Phase 1-3 已实现并提交**(2026-07-31,分支 `feat/group-chat`,commit `d2fca90` + `80ab4bd`)。Phase 4(人类抢占插话 + 配置 UI + utterance 渲染)待下一个 session 接手。D1-D11 全部拍板。详见文末「Phase 4 接手指南」。

## Goal

给 Everlasting 加一个**新 session 类型 `group_chat`**:一个**组织者(主持人)LLM** 主持,**多个不同 LLM** 围绕一个话题(需求/idea)进行**互可见的多轮对话** —— 参与者能互相看到对方发言、能反驳追问,人类可随时插话打断。

**架构核心:新建 session 类型,而非 workflow plugin。** 群聊没有 intake→…→收敛的状态机目标(D1),形态是一个自由 session 类型。引擎需要突破现有「worker 互不可见」硬限制:加 `session_type` 维度 + `speaker` 消息维度 + 双轨 turn-taking 编排(主持人 arbitrator + 人类抢占插话)。通用基建(per-dispatch model override、provider catalog、compaction、cancel)复用约 80%。

**与 review 的本质区别(不同构,非翻皮)**:review 是 fan-out 并行单评 → fan-in 主 LLM 综合,worker 无共享 transcript;群聊是多轮互相对话,参与者实时互见、能追问反驳。

## Background(代码事实)

### 现有架构对「一个 session 多 LLM 互见对话」原生支持度 = 零

是强「单主 LLM + 一次性 worker」二元结构。关键事实(每条带证据):

| 限制 | 证据 |
|---|---|
| session 绑定单一模型 | `SessionRow.model: String`(`db/types.rs:251`);无「session 类型」概念 |
| worker 看不到兄弟 worker | `build_worker_messages`(`subagent/mod.rs:696-735`)只装 memory+task;system prompt 明文「history NOT visible to you」(`mod.rs:602-603`) |
| worker 是一次性的 | `run_subagent` 跑到 terminal 即返回(`dispatch.rs:1222`);worker 不能调 dispatch_subagent(`STRUCTURALLY_DISABLED`,`mod.rs:763`) |
| worker 输出回流 = 一句话摘要 | `format_dispatch_result_with_model` 包成 `[status]\n[model]\n<summary>`(`truncate_summary.rs:292-326`),主 LLM 看不到完整推理 |
| messages 无 speaker 维度 | `messages.role` 只有 user/assistant(`migrations.rs:213-225`);前端 `ChatMessage.role` 二分(`chat.types.ts:208`);`Role` 枚举映射 wire 不可扩值(`llm/types.rs:34`) |

**关键判定**:主持人把历史摘要喂给新 worker = 退化成「带上下文的并行单评」,不是真群聊。要真群聊必须突破「worker 互不可见」。

### 可直接复用的通用基建(零耦合于 review)

| 能力 | 位置 |
|---|---|
| per-dispatch model override | `dispatch.rs:620-654` |
| 并发 dispatch (`DispatchBatch::Concurrent`) | `chat_loop.rs:4441-4500` + `3071-3149` |
| 拦截式 tool 模式(主持人调度用) | `ask_user_question`/`request_mode_change`/`request_task_state_transition`/`dispatch_subagent` 全是同款(`chat_loop.rs:3508/3600/3688/3781`) |
| cancel 中断机制(人类插话用) | `CancellationToken` + `commands/cancel.rs` |
| 上下文裁剪(多轮对话用) | `compact_messages`(`context.rs`,C3 压缩)+ `MAX_TURNS` |
| sessions.metadata 列(存 participants) | `migrations.rs:135`(已存在,零迁移);`SessionSummary.metadata` 也已就绪(`types.rs:362`) |
| subagent system_prompt 完全替换 | `mod.rs:494/507` |

### 落地方案选型(D2)

侦察原倾向方案 (c)(共享 transcript mini-loop,最轻真群聊路径),用户在知情后选 **方案 (b) 新建 session 类型**,优先架构干净度。(c) 的「加 speaker 列」「复用 messages 表」思路仍被 D6 采纳。

## Decisions(D1-D11,brainstorm 收敛)

### 形态与产物

- **D1. 产物 = 纯讨论纪要 / 思路记录,无下游消费者**(2026-07-29)。讨论目的是过程本身(头脑风暴、思路记录、不同观点对比),**不是**产出可流转给 dev 的需求/prd,也**不是**收敛一个决策结论。
  - 推论:群聊**不需要 workflow state machine**,不复用 review 的产物流转。形态上是自由 session 类型。
- **D11. 产物 = utterance 序列本身即纪要,不做二次整理**(2026-07-30)。session 的 utterance 序列就是纪要,前端按 speaker + 时间轴渲染(复用现有消息渲染基建 + speaker 维度)。二次整理会增加 MVP 复杂度且偏离过程导向。

### 落地与建模

- **D2. 落地方案 = (b) 新建 session 类型**(2026-07-30)。引入 `session_type` 新抽象层,与 D1「自由 session 类型」一致。代价是动单模型假设 + messages speaker + 前端 role 二分;收益是架构最干净、可扩展。
- **D4. session 类型建模 = `session_type` 列 + participants 外置 metadata**(2026-07-30)。
  - `sessions` 表加 `session_type` 列(`chat` / `group_chat`,默认 `chat` 兼容存量)。群聊 session 的 `model` 字段语义 = **主持人模型**(保持单模型假设不变,零侵入存量读取)。
  - 参与者配置存 `sessions.metadata` JSON 列(**已存在**,零迁移)。结构初稿 `{participants: [{name, model, persona_md?, order}]}`。注意:参与者是列表,JSON 并发更新需单写者/读改写串行。
- **D6. speaker 维度建模**(2026-07-30,基于业界调研):
  - **业界共识**(AutoGen `source` / OpenAI `name`):speaker 应是消息的**一等公民字段,与 role 正交**;不推荐焙进 content 文本(不可靠)。
  - **D6a 存储层 = 加 speaker 字段**:DB `messages` 加 `speaker` 列(参与者 name/id);`ChatMessage` 加 `speaker` 字段(**不入 wire**,仅内部追踪)。role 仍按协议走 user/assistant。
  - **D6b wire 层 = 各 provider 最优**:OpenAI provider 用 `name` 字段传 speaker(协议原生);Anthropic provider 退化为**前缀注入**(`@参与者名: 内容`)。两套逻辑需维护,但各 provider 都用最优方式。Anthropic 协议无 `name` 字段是关键约束。
  - **role 映射**:人类发言 = `user`,所有 LLM(主持人+参与者)发言 = `assistant`。
- **D8. 参与者 persona = inline metadata**(2026-07-30)。persona 全文 inline 存 sessions.metadata 的 participant 项(`{name, model, persona_md}`),与 D4 一致,零额外加载机制。否决复用 agent.md(编译期/磁盘静态,与群聊动态配置本性冲突)。

### 调度与插话

- **D3. 调度 = 主持人 LLM 调度 + 人类可随时插话**(2026-07-30)。需**双轨 turn-taking**:主持人驱动的 arbitrator + 人类插话通道。
- **D7. 主持人调度指令 = 拦截式 tool**(2026-07-30)。复用代码库成熟的「拦截式 tool」模式 —— 新增 `nominate_speaker`(input_schema: 下一个发言者)和 `end_discussion`(结束)两个拦截 tool,chat_loop 拦截不走 execute_tool。`input_schema` 强约束,可靠性最高。否决 JSON in text(易错格式)、纯文本调度(NLU 不可靠)。
  - arbitrator 逻辑:`nominate_speaker` → 取目标发言者 → 派发该参与者模型 → 发言落库 → 回主持人;`end_discussion` → 结束循环。
- **D9. 人类插话 = 抢占语义**(2026-07-30)。插话 = 先 cancel 当前发言(复用 `CancellationToken`)→ 人类发言落库 → 重新进入 turn-taking(主持人基于新上下文重新调度)。完全复用现有 cancel 基建,零新增复杂度。否决排队语义(需新建消息队列,与现有「cancel + 重发」模型冲突)。主持人重新调度时会看到人类插话内容(互见性满足)。

### 资源管理

- **D10. 上下文管理 = 复用现有 compaction 基建**(2026-07-30)。群聊多轮对话直接复用 `compact_messages`(C3 压缩),MVP 不做额外摘要。轮次上限沿用 `MAX_TURNS` 思路,群聊设独立上限(如 20-30 轮 utterance)避免无限成本。

### MVP 边界

- **D5. MVP 边界**(2026-07-30):
  - **含人类插话**:MVP 就做双轨 turn-taking(一步到位,不分期)。
  - **带最简参与者配置 UI**:加/删/重排/选模型/选人设。
  - **限定 2-3 个参与者**(不含主持人):控制上下文增长和成本,足够验证互见/反驳语义。

## Requirements

### 功能需求

1. **session 类型维度**:`sessions` 表加 `session_type` 列(`chat`/`group_chat`,默认 `chat`)。`SessionRow`/`SessionSummary` 透传该字段。`create_session`(`db/sessions.rs:28`)INSERT + bind 加 `session_type`。
2. **participants 配置**:存 `sessions.metadata` JSON(`{participants: [{name, model, persona_md?, order}]}`)。群聊 session 创建时写入;支持运行时增删改。
3. **speaker 消息维度**:DB `messages` 加 `speaker` 列;`ChatMessage`(`llm/types.rs:185`)加 `speaker` 字段;落库点(`db/sessions.rs:695` append)写入 speaker。
4. **wire 层 speaker 注入**:OpenAI provider(`provider/openai.rs`)emit `name` 字段;Anthropic provider(`provider/anthropic.rs`)退化为 `@参与者名: 内容` 前缀注入。
5. **turn-taking 编排**:群聊 session 走专用编排(在 `run_chat_loop` 入口或调用层加 `session_type` 分支)。编排循环:主持人发言 → 拦截 `nominate_speaker`/`end_discussion` → 派发参与者 → 发言落库(带 speaker)→ 回主持人。
6. **拦截式调度 tool**:`nominate_speaker` + `end_discussion` 两个拦截 tool(注册进 `builtin_tools` `tools/mod.rs:129`,拦截点加在 `chat_loop.rs` 同款位置)。主持人 system prompt 引导其使用。
7. **人类抢占插话**:群聊 session 的 send_message 路径 → cancel 当前发言(复用 `CancellationToken`)+ 人类消息落库 + 重新进入 turn-taking。
8. **主持人 persona / 参与者 persona**:主持人 persona 注入 system prompt;参与者 persona 在派发时作为 system_prompt 完全替换(`mod.rs:494` 模式)。
9. **参与者配置 UI**:加/删/重排/选模型/选人设(inline persona 文本框)。MVP 最简形态。
10. **utterance 渲染**:前端消息列表按 speaker 维度渲染(区分不同参与者,复用现有 MessageItem 基建)。

### 非功能需求

- **opt-in**:`session_type = chat` 的存量 session 行为零改动;群聊逻辑只在 `group_chat` 分支生效。
- **零迁移 metadata**:不新建表存 participants(用已存在的 `sessions.metadata` 列)。仅 `sessions` 加 `session_type` 列 + `messages` 加 `speaker` 列(均为非破坏性 ALTER)。
- **复用优先**:cancel / compaction / per-dispatch model override / provider catalog 全部复用,不重写。
- **provider 兼容**:speaker 注入分 provider 走最优路径,不因 Anthropic 无 `name` 而降级全体。

## Acceptance Criteria

> Phase 划分以**实际实现**为准(下方标注)。每项标注完成状态(commit)。

### Phase 1 数据层 ✅ (commit `d2fca90`)
- [x] `sessions` 表加 `session_type` 列 + migration(默认 `chat`,存量兼容);`messages` 表加 `speaker` 列 + migration(可空)。
- [x] `SessionRow`/`SessionSummary` 透传 `session_type` + `metadata`;`create_session` 支持 `group_chat`(INSERT 已含 session_type,但**前端创建入口待 Phase 4** —— 当前只能手工改 DB 建 group_chat session)。
- [x] 群聊 session 的 `metadata` 能写入/读出 participants JSON。
- [x] `ChatMessage` 加 `speaker` 字段;落库/读取往返正确(含回归:普通 chat session speaker 字段为 null 不破坏现有逻辑)。

### Phase 2 wire 层 ✅ (commit `d2fca90`)
- [x] OpenAI provider emit `name` 字段;Anthropic provider 前缀注入 `@参与者名:` + 移除 speaker 字段。
- [x] `WireMessage::User/Assistant` 携带 speaker,forward/reverse/strip_unsupported 全透传。
- [ ] 两 provider 均能让模型正确识别发言者 —— **真模型验证待 Phase 4 集成测试**(单测只验 wire 形态,未跑真实 LLM)。

### Phase 3 turn-taking 编排引擎 ✅ (commit `80ab4bd`)
- [x] `nominate_speaker` / `end_discussion` 拦截 tool 注册 + 拦截点就位;主持人 system prompt 引导使用。
- [x] 群聊 turn-taking 编排跑通:`run_group_chat_loop` 外层调度器(moderator → nominate_speaker → participant → reload → 回 moderator → end_discussion)。
- [x] 兜底机制:moderator 不 nominate → round-robin;MAX 30 轮硬上限。
- [x] 参与者发言时能看到前序所有 utterance(互见性 = 共享 messages Vec + DB reload)。
- [ ] **⚠️ participant 发言尚未带 speaker 落库** —— 见「Phase 4 接手指南」TODO-A。
- [ ] 端到端跑通验证 —— 待 Phase 4 集成(需真实 LLM + 配好的 session)。

### Phase 4 人类抢占插话 + UI ✅ (2026-07-31,commits `e065a12` + `a75aa37` + `35e631c` + `2b6ab8a` + `49ea28f`)
- [x] 人类抢占插话跑通:cancel 当前发言 + 人类消息落库 + 主持人基于新上下文重新调度(Phase 3 的 `run_group_chat_loop` cancel 路径已就绪,Phase 4 验证 round-trip `speaker` + reload,fail-closed by ask_user_question oneshot 监听 cancel token)。
- [x] 参与者配置 UI(加/删/重排/选模型/选人设)—— `GroupChatConfigModal.vue` (Phase 4 TODO-E6),2-3 参与者上限,name 唯一性 + model 必填校验。
- [x] 前端按 speaker 渲染 utterance 流(区分不同参与者)—— `MessageItem.vue` speaker chip,主持人 neutral + 参与者 djb2-hash 8-palette 配色(Phase 4 TODO-F1-F4)。
- [x] 前端创建群聊 session 入口(选 session_type=group_chat + 配 participants)—— `Sidebar.vue` 加 "新建群聊" 按钮,打开 `GroupChatConfigModal` create 模式。
- [x] **集成**:完整群聊跑通(代码路径完整)—— speaker round-trip 单测覆盖(2 new,1618 全绿);E2E 真实 LLM 端到端验证待 daemon + 真实模型环境。
- [x] 回归:普通 chat session + 现有 subagent/review 机制全绿;`cargo test --lib` 1618 + `cargo clippy --lib --tests` 0 新 warning + `pnpm vitest` 1002 + `pnpm vue-tsc --noEmit` 0 error 全部通过。

## Out of Scope

- ❌ workflow 状态机 / 产物流转 —— 群聊是自由 session 类型,无 intake→收敛目标(D1)。
- ❌ 结构化讨论纪要二次整理 —— utterance 序列本身即纪要(D11)。
- ❌ 上下文摘要/裁剪新机制 —— 复用现有 compaction(D10)。
- ❌ 排队式插话 / 消息队列 —— 选抢占语义(D9)。
- ❌ persona 复用 agent.md 机制 —— inline metadata(D8)。
- ❌ speaker 焙进 content 文本 —— 业界明确不推荐,选一等公民字段(D6)。
- ❌ 不限轮次 / 不限参与者 —— MVP 限 2-3 参与者 + 独立轮次上限(D5/D10)。
- ❌ 跨 session / 外部 LLM / 纯人类参与者 —— MVP 聚焦 AI 内部闭环(主持人 + LLM 参与者 + 人类插话)。

## Notes

- 本任务前置依赖:无硬前置,所有基建(cancel/compaction/per-dispatch model/拦截 tool 模式)均已就绪。
- 风险点(实现后复盘):
  1. **D6b 两套 wire 逻辑** —— ✅ 已实现(Phase 2)。Anthropic 前缀注入可能被模型混淆 → 待 Phase 4 真模型验证。
  2. **主持人 arbitrator 可靠性** —— ✅ 已兜底(Phase 3 round-robin fallback + MAX 30 轮)。
  3. **turn-taking 编排侵入 chat_loop** —— ✅ 用路径 A(外层包装器)规避,不改 run_chat_loop,零回归(1616 测试)。
- 与 review epic Phase 2 的交集:review 的「跨 session / 人机混合评审」与本任务的「人类插话」有概念交集,但本任务聚焦 AI 内部闭环多 LLM 对话,混合评审留 review Phase 2。

---

## Phase 4 接手指南(下一个 session 必读)

> 本指南为下个 session 无缝接手 Phase 4 而写。包含:实际定型的数据结构、精确待改文件清单、已知 TODO、验证方法。

### 实际定型的数据结构(Phase 1-3 已落地)

**participants 配置 JSON(存 `sessions.metadata`)** —— 见 `agent/group_chat.rs`:
```rust
// sessions.metadata 列(已存在,Phase 1 启用)
GroupChatConfig { participants: Vec<ParticipantConfig> }
ParticipantConfig {
    name: String,           // 显示名 + speaker 身份,session 内唯一
    model: String,          // model_id —— ProviderCatalog 的 key(catalog 按 model_id 解析 provider)
    persona_md: Option<String>,  // inline persona markdown(D8)
    order: Option<i32>,     // UI 排序 + 未来 order-aware fallback(当前 round-robin 未用)
}
```

**关键事实(实现中确认/修正)**:
- `sessions.model` 字段语义 = **主持人模型**;`moderator_model_id` 优先取 `session.model_id`,fallback `session.model`(见 `group_chat.rs::build_group_chat_ctx`)。
- `SessionRow`/`SessionSummary` 已有 `session_type: SessionType` + `metadata: Option<serde_json::Value>` 字段(Phase 1)。
- `ChatMessage.speaker: Option<String>` 已加(`llm/types.rs:201`,`#[serde(default, skip_serializing_if = Option::is_none)]` → None 不序列化,Some 序列化到前端)。
- `messages.speaker` 列已加(Phase 1 migration,可空)。

### ⚠️ 已知 TODO(Phase 3 遗留,Phase 4 必须处理)

**TODO-A:participant 发言未带 speaker 落库(高优先级)**
`run_group_chat_loop`(`agent/group_chat_loop.rs`)派发 participant turn 时,调用了 `run_chat_loop`,但 `run_chat_loop` 内部的 assistant 落库点(`chat_loop.rs:2115-2118`)写的是 `speaker: None`(Phase 1.4 机械补的)。群聊里 participant 的 speaker 需要在这里被设置成参与者名。
- **修复方式(已裁决,见下方「设计裁决 Q1」)**:给 run_chat_loop 加 `current_speaker: Option<String>` 参数(尾部追加,紧邻 `group_chat_state`),assistant 落库点(`chat_loop.rs:2118`)改成 `speaker: current_speaker.clone()`。所有现有调用点(普通 chat / subagent / review)填 `None`(机械改动)。
- **注意**:reload_messages(`group_chat_loop.rs`)当前把 speaker 硬编码为 None,也要改成读 DB 的 speaker 列(但 MessageRow 当前没有 speaker 字段 —— 见 TODO-B)。

**TODO-B:MessageRow 未读 speaker 列**
`db/types.rs` 的 `MessageRow` + `load_session` 的 SELECT(`db/sessions.rs:222`)还没有 `speaker` 字段。Phase 1 加了 DB 列 + persist_turn 写入,但读取侧没接通。Phase 4 要:MessageRow 加 `speaker: Option<String>` + SELECT 加列 + 映射。

### 设计裁决(接手答疑,2026-07-31)

> 以下 4 条裁决回应接手 LLM 的疑问,与实现者(上一手)确认一致。Phase 4 按此执行。

**Q1. TODO-A 实现方式 = 加显式 `current_speaker: Option<String>` 参数**
否决隐式传递(thread-local / 读 system_prompt_override 关联)—— run_chat_loop 落库点离 prompt 解析点很远,隐式关联易漂移。显式参数数据流可见、自文档化。签名变粗代价可控(已有 34 参,多 1 个在尾部,现有调用点填 None)。参数加在 `group_chat_state` 旁边(语义相关)。

**Q2. 配置 UI 时机 = 创建入口 + 简版改 modal(非全套 CRUD)**
澄清:D4「支持运行时增删改」是**数据建模能力声明**(metadata 可写),非 MVP 功能要求;D5「最简配置 UI」才是功能边界 —— 两者不冲突。MVP 形态:创建时配好(写 metadata)+ 运行中允许重新打开配置编辑(简版 modal,非 inline 实时增删)。需新建 `update_session_metadata` IPC(当前无写 metadata 的 IPC)。排序(order)MVP 用数组顺序,不做拖拽。

**Q3. 主持人 persona = MVP 不配,只配 model;内置固定 system prompt**
当前实现:`moderator_system_prompt`(`group_chat_loop.rs:55`)是内置固定模板(硬编码「你是 MODERATOR...」),不可配;只有 participants 有 `persona_md`。MVP 维持现状。主持人 speaker 名用固定标识 `"moderator"`(结构化角色,不需自定义)。未来扩展点:`sessions.metadata` 加 `moderator_persona_md`,模板优先用它 —— MVP 不做。

**Q4. 工具调用权限弹窗时的人类插话 = A(抢占,吃掉权限弹窗)**
与 D9 抢占语义一致。代码事实:权限弹窗(ask_user_question / Tier 4 ask)走 `QuestionStore` oneshot 阻塞,它**监听 cancel token**,所以 cancel 天然唤醒它(返回 cancelled)+ turn 终止 —— A 语义已被现有基建支持,无需特殊处理。前端:cancel 按钮直接 `cancel_chat` IPC + send 新消息;权限弹窗被 cancel 吃掉。否决 B(插话视为对工具调用的回应)—— 与 ask_user_question 结构化回答机制冲突。验证时确认 cancelled synthetic tool_result 路径在群聊编排下正常。

### Phase 4 待改文件清单(精确)

**后端**:
- `db/types.rs`:`MessageRow` 加 `speaker: Option<String>` 字段(TODO-B)。
- `db/sessions.rs`:`load_session` 的 messages SELECT(~222)+ MessageRow 映射(~235)加 speaker(TODO-B)。
- `agent/chat_loop.rs`:assistant 落库点(~2129)接通 speaker(TODO-A);可能需给 run_chat_loop 加 current_speaker 参数。
- `agent/group_chat_loop.rs`:`reload_messages` 读 speaker 列(替换硬编码 None)。

**前端**:
- `stores/chat.types.ts`:`ChatMessage`(~206)加 `speaker?: string`;`SessionSummary`(~310)加 `session_type` + `metadata` 字段。
- `stores/streamController.ts`:`rehydrateMessages`(~443)读 speaker(从 LoadedMessage)。
- `stores/chat.ts`:`createNewSession`(~549)支持 session_type=group_chat + 写 metadata;新增「配置 participants」IPC 或复用现有 session 更新。
- `components/chat/MessageItem.vue`:`msg--${message.role}`(~1121)加 speaker 维度 —— 按 speaker 渲染名字 chip + accent 色(仿 color_tag palette);区分不同参与者。
- 新建:参与者配置组件(加/删/重排/选模型/选人设 inline persona)。

**IPC / 命令**:
- 可能需新增 `create_group_chat_session` 或给 `create_session`(`commands/sessions.rs:106`)加 session_type + metadata 参数。
- 参与者运行时增删改 → 需 `update_session_metadata` IPC(写 sessions.metadata)。

### 人类抢占插话(D9)—— 后端现状
Phase 3 的 `run_group_chat_loop` 已持有 `token: CancellationToken`(每轮 `token.is_cancelled()` 检查 break)。**人类抢占的 cancel 路径已就绪**(复用现有 `commands/cancel.rs`)。Phase 4 主要工作是:
- 前端群聊 session 的 send_message → 触发 cancel 当前发言(cancel_chat IPC)→ 人类消息落库 → 重新 send(chat_inner 会因 session_type=group_chat 重新进入 run_group_chat_loop,主持人 reload 看到人类插话)。
- 验证:cancel → run_group_chat_loop 的 token 检查 break → 外层退出 → 新 send 进入新编排循环(看到人类消息)。

### 验证方法
- **端到端**:手工建 group_chat session(DB 改 session_type + 写 metadata participants)→ 前端发首条消息 → 观察 moderator 点名 → participant 发言 → 人类插话 → end_discussion。
- **回归**:`cargo test --lib`(当前 1616 全绿)+ `cargo clippy --lib --tests`(注意 WSL 用 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check/test`)。
- **前置**:WSL 环境编译需 `PKG_CONFIG_PATH`(见 CLAUDE.md「WSL 环境」节);完整 build 需 GTK 系统库(用 `pnpm tauri dev/build`)。

### 建议实施顺序(Phase 4)
1. TODO-A + TODO-B(speaker 落库/读取接通)—— 后端闭环,可单测验证。
2. 前端类型 + rehydrate(chat.types.ts + streamController.ts)—— speaker 透传到 UI。
3. 创建群聊 session 入口 + 参与者配置 UI(chat.ts + 新组件)。
4. utterance 渲染(MessageItem.vue speaker chip)。
5. 人类抢占插话验证。
6. 端到端集成测试。
