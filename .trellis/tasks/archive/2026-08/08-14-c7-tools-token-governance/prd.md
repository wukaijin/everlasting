# tools[] 上下文 token 治理 (C7)

## Goal

把 `tools[]` 数组当作与 messages 并列的**上下文治理对象**。核心目标是**省上下文窗口预算**(provider 无关),附带优化 Anthropic 侧的 input 费用 + TTFT。对应 ROADMAP §2 第三档 C7(`docs/ROADMAP.md:144`)。

用户价值:多 turn 任务里 tools[] 每 turn 占用 ~7-8k 窗口预算(单对话首回合 ~8k 固定层的大头),挤占有效历史、提前触发 C3 压缩。治理后长任务保留更多真历史、减少压缩抖动。

## Background

调研文档已存在:`docs/research/tool-context-progressive-disclosure.md`(四方向适用度判定 + Path 0-3 + 红线对照)。本 PRD 在其基础上补了三轮代码核实的关键事实(provider cache 差异 + 一个被 research 漏掉的零成本动作),并收敛 MVP。

ROADMAP C7 原文:"每 turn 全量下发 ~21 个 builtin tool schema...messages 层 cache 治理已 90 分,tools[] 是 0 分的请求前缀段。先量后优化"。

## Confirmed Facts(代码核实,2026-08-14)

### tools[] 规模与下发
- **23 工具,~7-8k token/轮**:`builtin_tools()`(`tools/mod.rs:138`)返回 23 个 ToolDef;静态测量原始 definition 函数体合计 ~45k 字符,序列化 JSON ×~0.7 ≈ ~31k 字符 ≈ ~7-8k token。最重:`use_ui`(5261)/`ask_user_question`(3615)/`remember`(3377)。
- **每 turn 全量拼装**:`turn_tool_defs`(`chat_loop/drive.rs:504`),过滤链只有静态黑白名单 — `filter_tools_for_mode`(`mode.rs:52`,Plan 砍 6 写工具)/ `filter_tools_for_workflow`(`tools/mod.rs:244`,非 workflow 砍 create_task + request_task_state_transition)/ worker-nesting gate。无按任务相关性/turn 阶段的动态筛选。

### Provider cache 差异(决定优化手段的 provider 边界)
- **Anthropic 无 cache 断点**:`ToolDef`(`llm/types/chat.rs:48`)只有 `name/description/input_schema`,**无 cache_control 字段**;anthropic 适配器下发是裸 `collect()`(`anthropic.rs:511-519`)→ tools[] 不进 prompt cache → **每轮全价 input**。
- **OpenAI 自动缓存**:`openai.rs:348` 裸 `body["tools"]`,OpenAI 自动缓存静态最长前缀 → tools[] **已 0.5× 折扣**;OpenAI 不认 cache_control 标记。
- **归一化把两家塞同一字段**:`llm/types/usage.rs:31/37/38/53` — Anthropic cache_read 0.1×、OpenAI cached_tokens 0.5×,都映射到 `cache_read_input_tokens`;Anthropic 有 cache_creation、OpenAI 无(→0)。两家用同一字段但单价不同,跨 provider 混算 cache 率会失真。
- **关键:cache 不省窗口预算**。无论哪家,prompt cache 只省 input 费用 + TTFT,**不省上下文窗口预算**(缓存 token 仍占窗口)。→ 省 window 只有裁剪/Stub 能做,provider 无关。

### 度量盲点
- **tools[] token 从未单独量化**:`cache_rate = cache_read_input_tokens / context_input_tokens`(`llm/types/usage.rs`;公式实算在前端 `GroupChatConfigModal.vue`,research §2.5 是概念定义)。`context_input_tokens` **已含 tools**(`:44-51`),但 tools 占多少从未单独计数 — R1 要补。research §2.5 原说"分母只覆盖 messages+system"不准确(评审 P1-3 纠正)。

### 现成落点 + 先例
- **群聊已有白名单先例**:`group_chat_tool_defs`(`group_chat_prompts.rs:194/206`),参与者只 5 研究、主持人 +2 仲裁 → 证明"按 session_type 裁剪 tools"可行且在用。
- **度量落点现成**:E2 任务已建 `turn_trace` 表 + `token_usage_json` 列(预留);`count_tokens`(`memory/tokens.rs:50`,cl100k)可直接复用估算 tools token。

## Decisions(2026-08-14 brainstorm)

1. **MVP = A + C**。度量(数据基础)+ 静态分组裁剪(省窗口,provider 无关,群聊白名单已是先例)。R2(B)实测后降级 Phase 2,见 decision 5。
2. **D(Stub 注册)留 Phase 2**,触发条件 = A 的度量数据显示 tools[] 占比 >15% 上下文窗口时再启动;避免无数据时的过早架构。
3. **C 的裁剪 provider 无关**:发生在 provider 之前的 `turn_tool_defs` 拼装层。
4. **memory 指令块治理暂不纳入**,记 BACKLOG(见 `docs/BACKLOG.md`)。理由:不同治理对象(`loader.rs` 路径 vs tools 路径)、不同手段(裁剪文档 vs 过滤工具);等 C7 度量数据评估其窗口占比后再排优先级。
5. **R2(Anthropic tools cache 断点)实测降级 Phase 2**(2026-08-14 实测 session 50b91178,MiniMax-M3 / anthropic 协议 / wukaijin relay):relay 吃 `cache_control` **不 400**(`{"input_tokens":12838,"cache_creation_input_tokens":0,"cache_read_input_tokens":128,"context_input_tokens":12966}`),但 `cache_creation=0` → **relay 静默忽略 cache_control、不执行缓存** → R2 在 relay 环境**零收益**(无害无益)。R2 真正收益(省钱+TTFT)仅在原生 Claude,**未测**(无原生 provider)。降级 Phase 2,等配了原生 Anthropic provider 再评估;设计保留在 `design.md` §R2 供复用。

## Requirements

### R1 tools[] token 度量(Path A)
- R1.1 per-turn tools token 估算:`turn_tool_defs` 拼装后(`drive.rs:504` 附近),用 `count_tokens`(`memory/tokens.rs:50`)对序列化后的 tools JSON 估算 token。
- R1.2 落盘:`turn_trace` 表加 `tools_token` 字段(migration v8),`upsert_turn_trace_token` 写入点(`drive.rs:801` 旁)补该字段。
- R1.3 cache 率口径(`context_input_tokens` **已含 tools**,见 `llm/types/usage.rs:44-51`):tools token **单列** `turn_trace.tools_token`,不混入 cache 率。现状:分子 cache_read 不含 tools、分母 context_input 含 → cache 率被 tools 稀释偏低(relay 环境 `cache_creation=0` 无缓存,见 decision 5)。**禁止**再把 tools_token 加进 context_input 分母(double-count)。
- R1.4 前端可见:复用 E2 `<TracePanel>` 展示 per-turn tools token + 占比。

### R2 Anthropic tools cache 断点 — Phase 2(实测降级,见 decision 5)
relay 环境 `cache_creation=0` 零收益 + 无原生 Claude provider 未验证。设计保留在 `design.md` §R2(含 body-patch 方案、断点预算、relay gate、automatic caching 隐患),等配原生 Anthropic provider 后重启。MVP 不实施。

### R3 静态分组裁剪(Path C)
- R3.1 扩展过滤链,按 session_type/场景裁剪"专属"工具(具体规则见 `design.md`):
  - 交互专属(`ask_user_question`/`request_mode_change`/`request_task_state_transition`/`use_ui`)— **不裁**(任何 session 都可能用,裁了丢能力)。
  - 群聊专属(`nominate_speaker`/`end_discussion`)— 非群聊 session 裁(MVP 注释已说"Phase 4 may filter",本任务落实)。
  - workflow 专属(`create_task`/`request_task_state_transition`)— 已有 `filter_tools_for_workflow`,复核覆盖。
- R3.2 不破坏 cache:裁剪结果在同一 session 连续 turn 内稳定。
- R3.3 用 R1 度量验证典型 session(非群聊/非 workflow)tools token 占比下降。

## Acceptance Criteria
- [ ] AC1 per-turn tools token 数落 `turn_trace.tools_token`,`<TracePanel>` 可见,典型单对话首回合约 ~7-8k(实测 session 50b91178 首轮 input 12838 印证 tools 是大头)。
- [ ] AC2 静态裁剪后,典型非群聊/非 workflow session 下发工具数下降,R1 度量的 tools token 占比下降。
- [ ] AC3 不回归:23 工具在各自适用场景仍可用;群聊白名单 `group_chat_tool_defs` 行为不变;mode/workflow 过滤不变;现有测试全绿。
- [ ] AC4 cache 率指标不失真(tools token 口径在 trace/usage 明确区分)。

## Out of Scope
- **方向① Anthropic Tool Search**:强绑 provider 协议,破坏多 provider 抽象。
- **方向③ invoke_tool 黑盒**:牺牲原生 schema 约束,对"参数错即执行出错"工具风险过大。
- **D Stub 注册**:Phase 2,触发 = A 度量数据 tools[] 占比 >15% 上下文窗口。
- **R2 Anthropic tools cache 断点**:Phase 2,实测 relay 零收益 + 无原生 Claude 未验证(decision 5)。等原生 Anthropic provider 配置后重启。
- **memory 指令块治理**:记 `docs/BACKLOG.md`,等 C7 数据评估。

## Phase 2 触发条件
A 的度量数据产出后:① 若 tools[] 占上下文窗口 >15%,启动 D(Stub 注册,research Path 2/方向②);② 配了原生 Anthropic provider 后,重启 R2(cache 断点,design §R2)。否则维持 A+C。
