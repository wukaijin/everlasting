# 工具上下文渐进式披露:启示与适配分析

> 调研日期:2026-08-11
> 来源:linux.do 帖子《关于渐进式披露工具上下文的几种方向讨论》(topic 2708576),原文整理见 [时歌的博客](https://www.lapis.cafe/posts/ai-and-deep-learning/agi/progressive-disclosure-of-tool-context/)
> 范围:评估"工具上下文渐进式披露"技术方向对本仓库(agent loop + tool calling)的适用性,为 `docs/ROADMAP.md` §2 第三档新增优化项提供依据。
> 方法:抓取原帖/转载全文 → 对照本仓库 `app/src-tauri/src/` 工具注册 / agent loop / cache 治理现状 → 逐方向判定适用度。
> 配套:本文不写实现代码,只产出帖子要点、本仓库现状对照、四方向适用度判定、落地候选锚点。

---

## 0. TL;DR

1. **帖子核心命题**:当 agent 注册几十上百个工具时,`tools[]` 数组本身成了上下文与成本的主要负担 —— Token / TTFT 成本飙升、Prompt Cache 前缀易失效、无关工具稀释有效信号诱发幻觉。解法方向是"渐进式披露"(progressive disclosure):平时只给工具元数据/存根,命中任务时才展开完整 schema。

2. **本仓库现状对照**:项目对 messages / instructions 层的 cache 治理已经做到 90 分(C3 压缩、`cache_control: Ephemeral` 断点红线、跨 provider cache 率度量、稳定前缀三层组装),但 **`tools[]` 是一个被忽略的、每 turn 全量重发的请求前缀段**。`builtin_tools()` 返回 ~21 个工具,每 turn 在 `chat_loop/drive.rs:504` 全量拼装下发,过滤链只有静态黑白名单(mode / workflow / worker-nesting),无任何按任务相关性 / turn 阶段的动态筛选。

3. **四方向适用度判定**:
   - **方向 ② Stub 注册** — 适用度最高。与项目现有 cache 策略天然兼容,且已有拦截式工具先例(`ask_user_question` / `request_mode_change`)可复用。
   - **方向 ④ Moonshot 思想(工具声明可后置)** — 适用度中。其"先侦察后展开"的思路契合多 turn ReAct loop,可落在 turn 边界。
   - **方向 ① Anthropic Tool Search** — 适用度低。强绑 provider 协议,破坏项目多 provider 抽象。
   - **方向 ③ `invoke_tool` 黑盒** — 适用度低。牺牲原生 schema 约束,对工具参数错即执行出错的场景风险过大。

4. **盲点揭示**:项目有漂亮的 cache 率指标(`cache_read / context_input`),但分子分母只算 messages + system,`tools[]` 的 token 从未被单独量化。**务实第一步是先量(给 `turn_tool_defs` 加 tools token 估算日志),拿到真实数字再决定是否上 Stub 架构。**

---

## 1. 帖子要点

帖子《关于渐进式披露工具上下文的几种方向讨论》把问题归结为三层,并梳理了四种解法方向。

### 1.1 三层问题

| 问题 | 说明 |
|------|------|
| **Token 与 TTFT 成本** | 200 个工具约占 80k Token,极速燃烧预算 + 极大首字延迟(TTFT) |
| **Prompt Cache 失效** | 原生 tool calling 要求工具契约在请求最前端的 `tools` 数组;任何中途增删工具改变请求前缀 → 缓存瞬间报废,历史消息重算 |
| **信息论噪音** | 无关工具稀释有效信号(互信息下降),长程任务诱发幻觉 |

### 1.2 四种解法方向

#### 方向 ① Anthropic Tool Search(服务端原生支持)

- **思路**:客户端发送完整契约,但非必要工具标 `defer_loading`;模型初始上下文只有一个内置 Tool Search 工具,通过 BM25 / 正则发现目标工具后,服务端返回 `tool_reference` 并在当前位置动态展开真实契约。
- **优点**:不破坏早期前缀缓存 + 严格服务端语法约束。
- **缺陷**:强依赖特定 Provider(Anthropic / OpenAI)底层协议,第三方生态(vLLM / OpenRouter)无法复刻。

#### 方向 ② Stub 注册(客户端压缩折中)

- **思路**:`tools` 数组保留 N 个真实工具名,但把参数契约极限压缩成"存根"(保留真名 + 万能宽松外壳);另注册一个常驻 `load_tool_schemas` 工具。
- **运行机制**:模型先调 `load_tool_schemas` 取得完整参数契约(作为普通文本返回),再按契约调用 Stub 工具。
- **优点**:可降约 80% 初始上下文 + 不破坏缓存。
- **缺陷**:工具数量仍 O(N) 占用注意力;本质半原生方案,真实参数合法性需客户端自行校验。

#### 方向 ③ `invoke_tool` 黑盒(O(1) 极致简化)

- **思路**:初始 `tools` 数组固定 O(1) 常数级 —— 只注册 `search_tools` + 一个全能 `invoke_tool`。
- **运行机制**:模型先 search 查工具名和简介,再把真实参数塞进 `invoke_tool` 的 `arguments` 黑盒;客户端拦截、提取、校验、执行。
- **优点**:无论背后挂载多少工具,初始上下文恒定;统一调用入口简化网关校验/限流。
- **缺陷**:Provider 侧无法对黑盒内参数做原生语法约束,校验失败率依赖模型指令遵循能力。

#### 方向 ④ Moonshot(Kimi)动态加载

- **思路**:打破"工具只能在请求头声明"的限制,允许在 `messages` 数组中插入携带 `tools` 字段的 System 消息 —— 工具声明视作普通对话内容。
- **运行机制**:模型先调搜索接口,客户端把匹配到的完整工具声明以 System Message 形式**追加到消息末尾**。
- **优点**:协议轻量 + 对前缀缓存极其友好 + 仍享 Provider 侧原生 Schema 约束。

### 1.3 关于 MCP 的观点

传统一次性全量注册架构下,每多挂一个 MCP server,工具定义总 Token 迅速逼近甚至突破模型上下文注意力上限;MCP 引入的海量工具进一步加剧上下文腐烂和 Token 消耗。

### 1.4 作者结论

一个优秀的工具系统核心目标是"让模型在当前决策中,只看到足以支持这一步行动的信息"。缺 Provider 协议配合时,"缓存稳定性 / 初始上下文 O(1) / 独立原生调用约束"三者只能取二。作者对 Moonshot 把工具声明当普通消息追加的设计评价最高("直觉且可控,且天然对缓存友好,十分之优雅")。

---

## 2. 本仓库现状对照

> 通过对本仓库 `app/src-tauri/src/` 的探查得出。结论:**项目受工具上下文膨胀困扰,且帖子高度适用。**

### 2.1 工具注册:静态全量

- **注册中心**:`app/src-tauri/src/tools/mod.rs:138` `pub fn builtin_tools() -> Vec<ToolDef>`,当前约 **21 个工具**(`read_file / write_file / edit_file / shell / grep / glob / list_dir / web_fetch / use_skill / update_checklist / run_background_shell / shell_status / shell_kill / merge_worker / discard_worker / remember / ask_user_question / use_ui / request_mode_change / request_task_state_transition / create_task / nominate_speaker / end_discussion`),外加动态的 `dispatch_subagent`(每 turn append,model enum 随用户配置增长)。
- **启动快照**:`AppState` 启动时一次性 `let tools = crate::tools::builtin_tools();`(`state.rs:266`)。
- **每 turn 拼装**:`app/src-tauri/src/agent/chat_loop/drive.rs:504` 组装 `turn_tool_defs`,**完整 JSON schema 全量下发**。

### 2.2 过滤:只有静态黑白名单

过滤链三层,**全是基于静态规则**:

1. `filter_tools_for_mode`(`agent/permissions/mode.rs:52`)— Plan 模式剔 write 类工具。
2. `filter_tools_for_workflow`(`tools/mod.rs:244`)— 非 workflow session 剔 `create_task / request_task_state_transition`。
3. `filter_tools_for_subagent`(`agent/subagent/tools_filter.rs:80`)— worker 剔 `STRUCTURALLY_DISABLED` 列表(防嵌套)。

**没有任何"按任务相关性 / turn 阶段 / 已调用前置工具"的动态筛选**。Edit 模式普通会话每 turn 稳定发 ~21 个工具的完整 schema。

### 2.3 MCP 集成:无

`grep -rni "mcp|model.context.protocol"` 在全仓(所有 .rs / Cargo.toml / package.json / 前端 src)**0 命中**。工具是硬编码 Rust 模块。帖子谈的"MCP server 工具数爆炸"这条路径不直接适用,但"全量 builtin tools + 动态 subagent enum"是同构问题。

### 2.4 System Prompt 构建:三层动态拼装

`app/src-tauri/src/agent/system_prompt.rs:145` `assemble_system_prompt(mode_prefix, base_prompt)`:

1. `DEFAULT_BEHAVIOR_PROMPT`(`agent/behavior_prompt.rs:29`)— 编译期常量,最稳定层放最前(prompt cache 前缀稳定)。
2. `mode_system_prefix(mode)`(`agent/permissions/mode.rs:12`)— 四档 `'static str` 前缀。
3. `base_prompt` = `build_system_prompt(...)` — **每轮重建**,含 session_id / project / cwd / worktree / HEAD SHA / today。

关键:`behavior_prompt.rs:24` RULE-E-013 明确**刻意不在 system prompt 里列工具名**,工具可见性"只"通过 `tools[]` 数组表达,避免 prompt 硬编码列表与 `builtin_tools()` 漂移。

### 2.5 上下文 / 缓存优化:messages 层 90 分,tools 层 0 分

项目在 messages / instructions 层投入很深,**但从未把 `tools[]` 当作 cache / 上下文膨胀治理对象**:

- **C3 上下文压缩**:`agent/context.rs:222` `compact_messages`,贪婪丢弃 droppable turn groups,token 估算用 cl100k_base。
- **Prompt cache 断点红线**:`memory/loader.rs:347` `build_instructions_blocks`,只在第一个 banner block 挂 `cache_control: Ephemeral`;recall block 必须 append 到 `messages[0]`(spec 反复强调,移位断点 = 每轮失效 = 5-10× 成本)。
- **System prompt cache 稳定性**:三层组装刻意把最稳定层放前;RULE-A-005 讨论 head_sha 每轮刷新 vs cache 前缀稳定的权衡,结论是二者解耦。
- **Cache 率追踪**:`llm/types/usage.rs` 跨 provider 归一化 `TokenUsage`,`cache_rate = cache_read_input_tokens / context_input_tokens`;群聊按 speaker 聚合(`08-10-group-chat-cache-rate` 任务)。

**`tools[]` 的 token 没有被单独量化** —— cache 率指标的分子分母只覆盖 messages + system。

### 2.6 Agent Loop:多 turn ReAct

`app/src-tauri/src/agent/chat_loop.rs:872` `for turn in 1..=turn_limit`(默认 `MAX_TURNS = 50`),每 turn 调 `drive_turn` 做 ⑤a 上下文注入 → C3 compaction → 组装 `turn_tool_defs` → `provider.send` → 处理 SSE → 收集 tool_calls → 回填 tool_result → 下一 turn。tool 派发有 L2 并行(只读 batch `FuturesUnordered`)和 serial(写工具 + Tier 4 权限 ask)两条路径。

---

## 3. 四方向对本仓库的适用度判定

### 3.1 方向 ② Stub 注册 —— 适用度最高

**为什么契合本仓库**:

- 每个 `ToolDef` 的 description + input_schema 相当厚,但绝大多数 turn 只用到 2-4 个工具。Stub 化能大幅瘦身初始上下文。
- `tools[]` 数组结构保持稳定 → **与现有 cache 策略天然兼容**,不破坏前缀缓存。
- 项目已有拦截式工具模式(`ask_user_question` / `request_mode_change` / `request_task_state_transition` / `dispatch_subagent` 在 chat_loop 里按名字拦截走 blocking 路径),再加一个 `load_tool_schemas` 是低改造成本。

**改造锚点**:`drive.rs:504-550` 的 `turn_tool_defs` 拼装是工具上下文的唯一咽喉;`tools/mod.rs:138` 的 `builtin_tools()` 是工具全集。

**风险**:半原生方案,真实参数合法性需客户端校验 —— 但本仓库本就有 Tier 4 权限层 + `dispatch_tool_calls` 的校验路径,补一层 schema 校验是自然延伸而非新负担。

### 3.2 方向 ④ "工具声明可后置"思想 —— 适用度中

帖子对 Moonshot 的推崇本质是"**工具定义不必钉死在请求最前端**"。本仓库走 Anthropic Messages API(`tools` 字段在前端),没法直接照搬,但这个思想可以体现为:**先发一个轻量侦察 turn**(模型用 stub 名决策),**下一 turn 才把命中的真实 schema 注入** —— 这恰好契合多 turn ReAct loop 的结构(`chat_loop.rs:872`),stub→full 的切换天然落在 turn 边界。

可视为方向 ② 的"两阶段变体":turn 0 用 stub 决策,turn 1 才展开。代价是多一个 round trip。

### 3.3 方向 ① Anthropic Tool Search —— 适用度低

依赖 provider 移植。本仓库是多 provider 抽象(`Provider` trait + Anthropic / OpenAI 两适配器),强绑 Anthropic 会破坏抽象。且 `dispatch_subagent` 的 model enum 走 OpenAI provider 时无法复刻该协议。

### 3.4 方向 ③ `invoke_tool` 黑盒 —— 适用度低

牺牲原生 schema 约束。本仓库工具参数错即执行出错(`edit_file` 的 old_string 不匹配、`shell` 的命令注入风险、`merge_worker` 的 conflict),把参数合法性完全交给客户端校验,收益不抵风险。

---

## 4. 落地候选路径(由低到高改造量)

> 不急于上架构。先量化,再决策。

### Path 0(必做前置):tools token 成本度量

给 `turn_tool_defs` 加一行 tools token 估算(复用 `memory::tokens::count_tokens`),落 `turn_trace`(已有 `token_usage_json` 列,E2 任务预留)。回答"我们到底在 tools 上烧多少 token / 多少 % 上下文"。这个数字做出来,后面要不要上 Stub、上侦察 turn,就有数据撑腰。

**改造点**:`drive.rs:801` 的 `upsert_turn_trace_token` 写入点旁边补 tools token 字段;`db/trace.rs` schema 加列。

### Path 1:静态分组裁剪(轻量,基于 Path 0 数据)

不引入 stub,只基于已有静态规则做更细的分组过滤。例如:已有 `nominate_speaker / end_discussion` 仅群聊用、`create_task / request_task_state_transition` 仅 workflow 用 —— 可扩展为"按 session_type / 当前 task 阶段 / 最近 N turn 工具使用历史"做更激进的裁剪。纯过滤,无架构变更,不破坏 cache。

### Path 2:Stub 注册(方向 ②,中等改造)

若 Path 0 数据证明 tools token 占比显著(如 >15% 上下文),引入 stub + `load_tool_schemas`。需设计:stub schema 压缩格式、`load_tool_schemas` 返回契约、客户端参数校验层、cache 断点是否需要调整(stub 数组稳定 → 应该不需要)。

### Path 3:两阶段侦察 turn(方向 ④ 变体,高改造)

若 Path 2 仍不够,引入"turn 0 stub 决策 → turn 1 full schema 注入"。需处理多 round trip 延迟、worker 继承、群聊多 speaker 各自的 tool schema 状态。复杂度高,列为远期。

---

## 5. 与现有架构的不变量/红线对照

落地任何路径前必须确认不违反这些既有约束:

| 约束 | 出处 | 影响 |
|------|------|------|
| recall block 必须 append 到 `messages[0]`,不能加 `cache_control` | `memory/loader.rs` + spec | 任何 tools 改造不能动 recall 注入路径 |
| system prompt 最稳定层放最前 | `system_prompt.rs:140-145` + behavior_prompt 注释 | stub 化若引入新的 system message 注入,必须评估前缀稳定性 |
| RULE-E-013 工具名不进 system prompt | `behavior_prompt.rs:24` | `load_tool_schemas` 返回的工具契约走 tool_result / system message,不能塞 system prompt |
| L2 并行只读 batch 不变 | `chat_loop/tools.rs` | stub 工具的并行 eligibility 判定需保持 |
| 跨 provider cache 率归一化 | `llm/types/usage.rs` | tools token 度量需纳入 `context_input_tokens` 分子,否则 cache 率失真 |

---

## 6. 结论

帖子把"工具上下文"独立出来当作与 messages 并列的治理对象。**本仓库在 messages 层做到了 90 分,在 tools 层是 0 分(全量静态注册)。** 最务实的第一步不是马上上 Stub 架构,而是先给 `tools[]` 的 token 成本加上度量(复用现有 `count_tokens`),拿到真实数字后再决定是否引入方向 ②。这与项目一贯的"先量化再优化"风格(C3 压缩、cache 率追踪)一脉相承。

ROADMAP 增补项见 `docs/ROADMAP.md` §2 第三档,编号 **C7**(C 簇 = context 治理;`tools[]` 是与 messages 并列的上下文治理对象;C5 已被移除项"Provider 限流"占用,不复用避免认知噪音)。

---

## 参考

- 原帖:linux.do/t/topic/2708576《关于渐进式披露工具上下文的几种方向讨论》
- 转载整理:[时歌的博客 — 渐进式披露工具上下文](https://www.lapis.cafe/posts/ai-and-deep-learning/agi/progressive-disclosure-of-tool-context/)
- 本仓库现状:
  - 工具注册:`app/src-tauri/src/tools/mod.rs:138`
  - 每 turn 拼装:`app/src-tauri/src/agent/chat_loop/drive.rs:504`
  - 过滤链:`agent/permissions/mode.rs:52` / `tools/mod.rs:244` / `agent/subagent/tools_filter.rs:80`
  - cache 治理:`memory/loader.rs:347` / `agent/context.rs:222` / `llm/types/usage.rs`
  - token 估算:`memory::tokens::count_tokens`(cl100k_base)
