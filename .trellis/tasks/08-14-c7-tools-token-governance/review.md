# Review — tools[] 上下文 token 治理 (C7)

> 评审日期:2026-08-14。评审对象:`prd.md` / `design.md` / `implement.md`(status=planning,实施前评审)。
> 方法:对 PRD/design/implement 引用的代码事实(tools 数量与注册点、过滤链接入点与行号、ToolDef 字段、anthropic wire round-trip、body patch 先例、cache 率字段、migration 幂等模式、trace upsert/list 行号、群聊白名单与拦截逻辑)逐条 `sed` / `grep` / `awk` 核验;对 R2 的 cache 命中语义用 Anthropic 官方文档(platform.claude.com prompt-caching)交叉验证;对 R3 的 is_group_chat 信号可用性在 `drive_turn` 参数表内核对。

## 总体评价

三件套质量高——**事实基础扎实(全部行号精确命中)、收益诚实校准(A/B/C 各自省什么写得很清楚,C 明确标注 <7% 不制造错觉)、风险与回滚设计到位**。三条路径接入点均与现有代码惯例一致(body patch 有 `apply_deepseek_reasoning_fix` 先例、幂等 migration 有 `add_session_audit_events_column_if_missing` 先例、群聊白名单是先例本身)。

**结论:可批准进入实施**,但建议先处理 3 个 P1(1 处断点预算压线 + 1 处 relay 未 gate + 1 处 cache 率口径前提写反),其余 P2 顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| `builtin_tools()` 返回 23 工具(`tools/mod.rs:138`) | **精确**:read_file→end_discussion 逐项数 23 |
| 过滤链只有静态黑白名单,`drive.rs:504` `filter_tools_for_workflow(filter_tools_for_mode(...), ...)` | **精确**:L504 命中;`mode.rs:52`(Plan 砍 6 写工具)、`tools/mod.rs:244`(非 workflow 砍 create_task/request_task_state_transition)均命中 |
| `turn_tool_defs` freeze 于 `drive.rs:550`(dispatch_subagent append 之后) | **精确**:L550 `let turn_tool_defs = turn_tool_defs;` |
| `ToolDef` 只有 name/description/input_schema、derive Serialize(`chat.rs:44`) | **精确**:L44-49;wire round-trip `anthropic.rs:505-521` 重建只拷这 3 字段 → **body patch 方案成立** |
| `AnthropicProvider::send` 内 `apply_deepseek_reasoning_fix`(to_value + patch body)先例 `anthropic.rs:278/562` | **精确**:L562 `let mut body = apply_deepseek_reasoning_fix(&req);`;`send_request` 在 `chat_stream_with_tools` 内 L127;patch 落点在 `apply_speaker_prefix`(:568)后、`Box::pin` 前 |
| OpenAI 不认 cache_control、已自动缓存(`openai.rs:348`) | **精确**:裸 `body["tools"] = json!(tools)`,零改动成立 |
| `count_tokens`(`memory/tokens.rs:50`)— async + cl100k + `!Send` encoder | **精确**:`context.rs:43/194/367` 已有复用先例 |
| migration 幂等模式 `schema.rs:967` | **精确**:`add_session_audit_events_column_if_missing(pool, "turn_seq", "INTEGER")` 恰在 turn_trace 段 |
| `upsert_turn_trace_token`(`trace.rs:55`)+ `list_turn_traces`(`trace.rs:255`) | **精确**:唯一调用点 `drive.rs:801`,已包 `!skip_persist` 门 + non-fatal warn → 签名加参风险低 |
| 群聊白名单先例 `group_chat_prompts.rs:194/206`(5 研究 + 2 仲裁) | **精确**;且群聊走独立 `group_chat_loop.rs:291/488`,不进 `turn_tool_defs` 链 |
| 非群聊拦截 no-op(`tools/mod.rs:224` 注释) | **精确**:`chat_loop/tools.rs:924` 按 tool_name 拦截、`group_chat_state=None` 时返回 error tool_result,不依赖 tools[] 注册 → **R3 裁掉不影响拦截** |
| `build_group_chat_ctx` 判定 `session_type == GroupChat`(`group_chat.rs:80`) | **精确**:L80 起、gate 在 :91 |
| R2 cache 语义:多轮对话 tools 段命中 | **官方文档验证**:Anthropic 缓存为分层前缀模型,tools 是独立前缀段(不随 messages 增长失效);"末尾工具挂 cache_control → 第 2 轮起 cache_read 含 tools" 成立;代价是 tools 定义变更 invalidate 全部 cache(与 R2.4 稳定性前提自洽) |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 断点预算恰好压线:现有 3 + tools = 4 = Anthropic 上限

**位置**:design R2(无提及)。

**问题**:每请求现有 3 个 Ephemeral 断点——memory 指令块(`memory/loader.rs:362`)+ skills 块(`skill/loader.rs:647`)+ recall banner(`memory_recall.rs:287`)。R2 加 tools 断点后**恰满 Anthropic 4 断点上限,零余量**。R2 本身可行,但未来任何新断点(如 messages 断点)会直接 400。

**建议**:implement 时在 patch 处加注释声明"已占用第 4 个断点槽位";PRD/design 补一行断点预算说明。

### 🔴 P1-2 — R2 patch 未 gate,会打到 DeepSeek relay 路径

**位置**:implement R2 步骤 1(无 gate)。

**问题**:`AnthropicProvider::send` 同时服务原生 Claude 和 wukaijin relay(即 `apply_deepseek_reasoning_fix` 存在的那个有 400 前科的 relay)。patch 不加判定会跟着打到 relay 上,relay 对未知字段有 400 历史。风险表已提及"relay 不支持 → 400",但实现没给 gate。

**建议**:按 `config.base_url` / `config.model` 判定只对原生 Claude 生效,或至少在 relay 上先手测一轮;把该验证列为 R2 第一步验收项。回滚虽零副作用,但踩 400 会连带 DeepSeek 用户全挂。

### 🔴 P1-3 — cache 率口径前提写反(结论不变,论证必须改)

**位置**:prd R1.3 + design §R1"cache 率口径" + research §2.5(源头)。

**问题**:"现状 cache 率分子分母只覆盖 messages + system"**不准确**。Anthropic `input_tokens` 统计全请求(含 tools),OpenAI `prompt_tokens` 同理 → `context_input_tokens` 分母**已含 tools**(`anthropic.rs:437`、`streaming.rs:156`)。真实情况:
- 现状(Anthropic):分母含 tools、分子 `cache_read` 不含 → cache 率被 tools **稀释(偏低)**;
- R2 后:分子分母都含 → 口径自然一致,并非"混算失真"。

两个派生问题:
1. **prd R1.3 的"或纳入 context_input_tokens 分母"备选方案是错的**——tools 已在分母里,再纳入即 double-count,应删;
2. **design 的 TracePanel 占比公式 `tools_token / (context_input + tools_token)` double-count 分母**,应直接用 `tools_token / context_input`(估算 vs provider 实测 ~1-2% 漂移,design 已自认)。

design 的"单列、不混入"结论依然正确,只需改论证文字与公式。

### 🟡 P2-1 — R3 的 is_group_chat 信号来源写错

**位置**:design §R3"接入点"。

**问题**:"复用 `build_group_chat_ctx`(`group_chat.rs:80`)判定"是 async DB 加载,过度。`drive_turn` 参数表里**已有 `loaded_session.session.session_type`**(`drive.rs:89`),直接比较 `== SessionType::GroupChat` 即可,零成本(另 `current_speaker: &Option<String>` 也是可用信号)。实现时按此修正,不碰 `build_group_chat_ctx`。

### 🟡 P2-2 — 小问题清单

- **R1 估算对 worker turn 白算**:估算点在 `:550`(worker 也经过),落盘在 `!skip_persist` 门内。建议 `skip_persist` 短路估算(ms 级,不修也行)。
- **AC2 手测需写 5 分钟 TTL 前提**:"第 2 轮起命中"仅当两轮间隔 <5min;implement.md:80 手测清单应补,否则隔久误判失败。
- **引用漂移**:`cache_rate = cache_read / context_input` 公式实际算在前端(`GroupChatConfigModal.vue`),research 是概念定义;`usage.rs` 实为 `llm/types/usage.rs`;ROADMAP "~21 工具" 为旧数(prd 已写 23 ✅)。
- **design R2 标题"修正 prd R2.1"冗余**:prd R2.1 本就写 body patch,两者一致,标题易误导。
- **流程提醒**:`check.jsonl` / `implement.jsonl` 仍为模板占位,check 阶段前按模板补 spec/research 条目。

## 放行建议

1. 采纳 P1-3 / P2-1 的表述与公式修正(改文档即可,不影响架构);
2. implement R2 时把 P1-2 的 relay 验证/gate 列为 R2 第一步验收项;
3. P1-1 断点预算在 patch 处留注释声明。

修正后按 implement.md 执行顺序(R1→R2→R3)开工,无需重新 brainstorm。
