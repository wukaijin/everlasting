# Design — tools[] 上下文 token 治理 (C7)

> 配套 `prd.md`。MVP = A(度量) + C(静态裁剪)。R2(B)实测降级 Phase 2(prd decision 5)。本文给出三路径技术设计、接入点、红线对照、风险回滚。

## 0. 收益的诚实评估(先校准预期)

| 路径 | 直接省什么 | 预期收益 | 备注 |
|---|---|---|---|
| **A 度量** | 不直接省 | 0(数据基础) | 解锁 D/R2(Phase 2)决策 + 给 C 验证手段 |
| **C 静态裁剪** | 窗口(provider 无关) | **有限**:~465 tok/轮(<7%) | 多数大工具通用,静态只能裁群聊专属 2 个小工具 |
| ~~**B cache 断点**~~ | ~~Anthropic input 钱 + TTFT~~ | **Phase 2(实测降级)** | 实测 relay `cache_creation=0` 零收益;原生 Claude 未测。详见 §R2 |

**结论**:MVP 核心交付是 **A(度量基础设施)**;**C 是低风险小幅"卫生"补充**。B/R2 实测降级 Phase 2(等原生 Claude),D(Stub)本就 Phase 2。实测铁证(session 50b91178):一句"早上好" input=12838,其中 tools ~7k 是每轮重发的大头——这正是 A 要量化、C/D 要治理的对象。

> 这个校准很重要:不要给评审"C 能砍掉大头"的错觉。大头是通用工具(use_ui/ask_user_question/remember/update_checklist/shell),静态裁不动。

## R1. tools[] token 度量

### 接入点
- **估算点**:`drive.rs:550` `let turn_tool_defs = turn_tool_defs;`(freeze)之后。此时 turn_tool_defs 已过完整过滤链(mode/workflow/dispatch 注入),是真实下发的集合。
- **序列化估算**:复用 `serde_json::to_string(&turn_tool_defs)`(`ToolDef` derive `Serialize`,`chat.rs:43`)→ `count_tokens`(`memory/tokens.rs:50`,`async fn(&str) -> u32`,cl100k)。llm `ToolDef` 序列化与 provider wire 序列化略有差异,但 token 估算够用(1-2% 漂移可接受)。
- **落盘**:扩 `db/trace.rs` 的 upsert。当前 `upsert_turn_trace_token(db, session_id, seq, token_usage_json)`(trace.rs:55)只写 `token_usage_json` 列。
  - 方案:新增列 `tools_token INTEGER` + 扩 `upsert_turn_trace_token` 签名加 `tools_token: Option<u32>` 参数(或新增独立 upsert)。`list_turn_traces`(trace.rs:255)返回行加 `tools_token`。
- **加列(migration)**:`schema.rs` 是**幂等模式**(无版本号,`CREATE TABLE IF NOT EXISTS` + `add_*_column_if_missing`)。参照 `add_session_audit_events_column_if_missing(pool, "turn_seq", "INTEGER")`(`schema.rs:967`)写 `add_turn_trace_column_if_missing(pool, "tools_token", "INTEGER")`,在 `run_migrations` 内 turn_trace 段(`schema.rs:967` 后)调用。

### cache 率口径(评审 P1-3 纠正)
- **`context_input_tokens` 已含 tools**(`llm/types/usage.rs:44-51`:Anthropic `input_tokens + cache_creation + cache_read`;OpenAI `prompt_tokens`)。research §2.5 / 我原稿"分母只覆盖 messages+system"是错的。
- 现状:分子 cache_read **不含** tools(tools 无断点)、分母 context_input **含** tools → cache 率被稀释**偏低**;R2 后 cache_read 也含 tools → 口径自然一致。
- **tools_token 单列** `turn_trace.tools_token`,不混入 cache 率。TracePanel 占比公式 = `tools_token / context_input`(估算 vs provider 实测 ~1-2% 漂移)。⚠️ **不要** `tools_token / (context_input + tools_token)` — context_input 已含 tools,那是 double-count。

### 前端
- 复用 E2 `<TracePanel>`(research 已确认结构)。`TurnCard` 加 `tools_token` 维度展示。后端 `list_turn_traces` IPC 已返回 turn_trace 行,加字段即可;live 走现有 ChatEvent(若需 live tools_token,可复用 `TurnComplete` 或加字段,design 阶段不细化)。

## R2. Anthropic tools cache 断点 — Phase 2(实测降级,见 prd decision 5)

> **实测结论(2026-08-14)**:session 50b91178(MiniMax-M3 / wukaijin relay)吃 `cache_control` 不 400 但 `cache_creation=0` → relay 静默忽略、不缓存 → R2 在 relay 环境零收益。原生 Claude 未测(无 provider)。以下设计保留供 Phase 2(配原生 Anthropic provider 后)复用。

### 方案说明
**不在 `ToolDef`(`chat.rs:44`)/ `WireTool`(`wire/types.rs:174`)加 `cache_control` 字段。** 理由:
1. **wire round-trip 会丢字段**:anthropic 适配器走 wire round-trip(`anthropic.rs:505-521` 从 `wire.tools` 重建 `ToolDef` 只拷贝 `name/description/input_schema`)。加在 llm `ToolDef` 要同步改 `WireTool` 双份,污染跨 provider wire 抽象。
2. **cache_control 是 Anthropic-specific**,藏在 anthropic 适配器内最合适,不该进通用 wire 层。

**改为:body 序列化后 patch**(文件内已有先例 `apply_deepseek_reasoning_fix`,`anthropic.rs:279-280` `to_value(req)` 后 patch body):
- `AnthropicProvider::send` 构造 `body: serde_json::Value`(从 ChatRequest `to_value`)后、`send_request(&config, &url, &body)`(`anthropic.rs:127`)前。
- patch:`tools` 数组非空时,给 `body["tools"].last_mut()` 插入 `"cache_control": {"type":"ephemeral"}`。
- 纯 Value 操作,不动任何 struct。

### 不动
- OpenAI 适配器:不认 `cache_control`,已自动缓存(`openai.rs:348`),零改动。

### 前缀稳定性(决定 cache 命中率)
- 同一 session 同一 mode 下 `turn_tool_defs` 稳定(`builtin_tools()` 静态 + 过滤链按 session 固定);mode/workflow/session_type 切换才变。cache 在一个 mode 段内有效,切换后重建(5min TTL 内)。

### 断点预算 + relay gate(评审 P1-1/P1-2 + 独立补充)
- **断点预算**:现有 3 个 Ephemeral 断点(`loader.rs:362` / `skill/loader.rs:647` / `memory_recall.rs:287`)+ tools = 4 = Anthropic 上限(多源确认)。
- ⚠️ **独立发现(评审 P1-1 未覆盖)**:Anthropic automatic caching 可能占 4 槽中的 1 个(官方 cookbook 原文 "Automatic caching uses one slot")。若属实 → 4 槽已满 → tools 断点直接超限 400(不止评审说的"零余量")。无法静态确认项目模型是否开 automatic,必须实测。
- **relay gate**:`AnthropicProvider` 靠 `base_url`(`anthropic.rs:63/74`)同时服务原生 Claude + wukaijin relay;relay 对未知字段有 400 前科(`apply_deepseek_reasoning_fix` 即为此而生)。cache_control 对原生有益、对 relay 可能有害 → patch 按 `base_url` gate(非 wukaijin 才挂),或实测 relay 行为后决定。
- **R2 首步硬验证**:原生 Claude + relay 两路径都加 tools 断点跑一轮,确认不 400;再谈 cache 命中。

### 验证
- Anthropic 路径:同 session 连跑 ≥2 轮(**间隔 <5min TTL**),第 2 轮起 `cache_read_input_tokens` 上升(幅度对齐 R1 的 tools_token)。OpenAI 路径行为不变。

## R3. 静态分组裁剪

### 接入点
- 过滤链:`drive.rs:504` `filter_tools_for_workflow(filter_tools_for_mode(tool_defs, mode), workflow_ctx)`。新增 session_type 过滤环节,与现有两环同级。
- session_type 判定:直接用 `drive_turn` 参数表的 `loaded_session.session.session_type == SessionType::GroupChat`(`drive.rs:87`,零成本)。**不**碰 `build_group_chat_ctx`(async DB 加载 participants,过度)— 评审 P2-1。

### 裁剪规则(MVP)
- **非群聊 session 砍 `nominate_speaker` + `end_discussion`**。落实 `tools/mod.rs:224` 注释"MVP: always registered; Phase 4 may filter by session_type for a cleaner classic-chat tool list"。
- **workflow 专属**(`create_task`/`request_task_state_transition`):已有 `filter_tools_for_workflow`(`tools/mod.rs:244`),**复核覆盖,不重复实现**。
- **交互专属**(`ask_user_question`/`request_mode_change`/`use_ui`):**不裁** — 任何 session 都可能用,裁了丢能力。

### 预期收益(诚实量化)
- 仅省 `nominate_speaker`(~280 tok)+ `end_discussion`(~185 tok)≈ **~465 tok/轮**,相对 ~7k tools 总量 **<7%**。
- 大头(`use_ui`/`ask_user_question`/`remember`/`update_checklist`/`shell`)是通用工具,静态裁不动。省窗口的大动作 = D(Stub, Phase 2)。
- **R3 在 MVP 的价值主要是"卫生"**(非群聊不暴露无意义工具,减少弱模型误用),不是省 token。这点要在评审时明确,避免错觉。

## 红线对照(落地前必须确认不违反)

| 约束 | 出处 | 对本任务的影响 |
|---|---|---|
| recall block append `messages[0]`,不加 cache_control | `loader.rs` + spec | R1/R3 不动 recall 注入路径 ✓ |
| system prompt 最稳定层放最前 | `system_prompt.rs:140` | R2 只 patch tools,不动 system ✓ |
| RULE-E-013 工具名不进 system prompt | `behavior_prompt.rs:24` | R3 裁剪只动 `tools[]`,不进 system ✓ |
| 跨 provider cache 率归一化 | `llm/types/usage.rs` | R1 tools_token 单列,不混 cache 率 ✓ |
| L2 并行只读 batch eligibility | `chat_loop/tools.rs` | R3 裁剪不改并行判定 ✓ |
| `PROTECTED_HEAD` memory 对不被 compaction 丢 | `context.rs:55` | 本任务不动 compaction ✓ |

## 风险 + 回滚

| 路径 | 风险 | 回滚 |
|---|---|---|
| R2 | provider/relay 不支持 `cache_control` → 400;5min TTL 首次 1.25× 写入成本 | 删除 patch 一行,零副作用 |
| R1 | `count_tokens` async + `!Send` encoder;best-effort 不阻塞主 loop | 包在 `tokio::spawn` 或非关键 await,失败 `tracing::warn` 跳过 |
| R3 | 非群聊看不到 nominate/end(本就无意义);确认 chat_loop 拦截 no-op(`tools/mod.rs:220`)不依赖工具注册 | 删除过滤环节 |

## 不做(Phase 2 / OOS,见 prd.md)
- D Stub 注册:等 R1 数据 tools 占比 >15% 窗口。
- memory 指令块治理:`docs/BACKLOG.md`。
- 方向① Anthropic Tool Search / ③ invoke_tool 黑盒:research 判定不适用。
