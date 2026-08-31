# 取证:提示词头部易变内容 → OpenAI 路径缓存全量失效(2026-08-31)

## 触发场景

用户在路由计费页发现两条请求(deepseek-v4-flash,`280,368/678` 与 `281,165/257`,first token 均 14.5s)全价计费,疑似缓存失效。与 turn_trace 精确对上:session `d6728b3a` **seq 435、seq 437 两轮 cache_read=0**。

## 关键数据

- 该 session ~110 轮 cache_read ≈ 上轮 input(正常递增命中);全零仅 3 轮:285(09:13:03)、435(09:41:18)、437(09:42:00)。
- 另有 3 轮**部分命中恰好 154,112**(09:12/09:28/09:40,均发生在 3.5~7 分钟空闲后)——头部内容与邻轮相同,属上游缓存保留问题,非 harness(见"排除")。
- 计费两条与 daemon.log 对应:请求 09:41:18.7(流打开延迟 16.1s = 全量 prefill)、09:42:00.4(13.3s);两轮 `tools_count=29 has_system=true` 不变,`build_http_body` 无 seed/user/时间戳等易变字段。

## 根因链(harness 侧,单供应商假设下成立)

**前提**:OpenAI 兼容路径前缀缓存从字节 0 严格匹配;`to_wire` 对 OpenAI 丢弃 `cache_control`(openai.rs B5 注释:"OpenAI Chat Completions has no prompt-cache marker")。头部任何字节变化 = 全量重付。

头部易变内容清单(按实证强弱):

1. **workflow breadcrumb**(`inject.rs:376 append_workflow_breadcrumb` → push 进 **messages[0]** 块列表;drive.rs:939 每轮调用):
   - turn_trace `breadcrumb_json` 快照直接对出:seq 433 = `task_id: b3fa4fb5… status: in_progress`,seq 435 起 = `plugin: dev / state: planning (no active task)`——内容在 435 翻转(09:41:11 `request_task_state_transition` 触发),**435 cache_read=0**;
   - 代码注释自证:"the breadcrumb changes per-turn (status can flip mid-loop)"。
2. **instruction 文件合成块**(`init.rs:662-675`,AGENTS.md/CLAUDE.md × User/Project 插 messages[0..1],每次**新 chat 请求 init 从磁盘重读**):
   - session 仅两条用户消息(seq 0 / seq 436 "commit" @09:41:59);
   - agent 在 loop 中期成功编辑过 AGENTS.md(09:12:56)与 CLAUDE.md ×2(09:13:31/09:13:38),tool_result "Successfully edited" 确认;09:42:34 的 `git add AGENTS.md CLAUDE.md …` 同证;
   - 新请求 init 读到新内容 → 头部与已缓存前缀不同 → **437 cache_read=0**;
   - 旁证(loop 内不重读):09:13:31 CLAUDE.md 编辑后,seq 287(09:13:34)照常命中 210,176 ≈ seq 285 前缀。
3. **loop-hint(已排除)**:初判曾疑其注入头部,复核代码后排除——drive.rs ⑬ 注释与 openai.rs wire order-guard 注释证实 hint 是 prepend 到 **result 消息(user(tool_results))的块头部**,即对话尾部最新消息,不破坏前缀。turn_trace seq 284/286 的 loop_hint 记录与 seq 285 全零的时间相关性另有候选解释(见下条)。
4. **并发辅助调用(tools_count=0,新发现,待查)**:daemon.log 当天有 4 次 `→ LLM request … tools_count=0`(08:50:58 / 09:00:35 / 09:12:58 / 09:24:56),以及紧随其后的 `tools_count=13` 请求(09:00:35.509 tools=0 → 09:00:35.550 tools=13,间隔 40ms)——形态吻合 **worker dispatch 前的 `truncate_summary` 摘要调用(无 tools)+ worker 本体(13 tools)**。时间相关性:09:12:58 tools=0 → seq 285(09:13:03)全零;09:24:56 tools=0 → seq 401(09:28:20)部分回退 154,112;09:00:35 tools=0 → 154,112 冻结快照纪元起点附近。疑似此类共享大段历史的无 tools 大请求在单供应商侧挤占/驱逐主 loop 的缓存条目——未定证,列为 R5 调查项。
5. **head_sha**(system prompt 内,每轮刷新,RULE-A-005):mid-session commit 会让下轮全量 miss(本 session 未触发;commit 0f9aa993 发生在 session 后)。

恢复模式佐证:seq 439 命中 281,344 ≈ 恰为 437 刚预填充的前缀——头部稳定后缓存链即刻重建。

## 排除

- 多上游负载均衡解释:曾作为备选提出(154,112 陈旧快照反复命中 ⇒ 疑似多节点),但单供应商假设下,435/437 有更直接的头部分叉证据,且 154,112 三次命中均 correlates 空闲间隔(3.5/3.5/7 min)——归为上游缓存保留行为,与 harness 无关。
- DeepSeek 官方缓存 TTL 为小时级,37s 间隔排除过期。
- 前缀中段改写(如 compaction)会掉到分叉点而非 0;实测精确 0 ⇒ 分叉在字节 0(头部),与上述清单一致。

## 影响量化

- 该 session 三次全 miss ≈ 77 万 input token 全价重付 + 三次部分 miss 短付 ~29 万 token;DeepSeek cache hit 计价约为 miss 的 1/10。
- 症状触发条件常见:任何 workflow 状态迁移、agent 编辑 instruction 文件后的下一条用户消息、loop 检测触发、mid-session commit。

## 相关代码坐标

- `app/src-tauri/src/agent/workflow/inject.rs:376` `append_workflow_breadcrumb`(messages[0] 头部;`append_delegation_template` 同构)
- `app/src-tauri/src/agent/chat_loop/drive.rs:939` 每轮调用点
- `app/src-tauri/src/agent/chat_loop/init.rs:630-684` instruction/memory/skill listing 合成头插入
- `app/src-tauri/src/llm/provider/wire/to_wire.rs` cache_control 在 OpenAI 侧被丢弃
- turn_trace 佐证列:`breadcrumb_json` / `loop_hint_json` / `token_usage_json.cache_read_input_tokens`
