<!-- Moved from token-usage-tracking.md 2026-09-13 (doc-split) -->

## Scenario:tools=0 辅助请求归因判别器 + cache miss 归因次序(09-01-aux-call-cache-interference,2026-09-01)

OpenAI 兼容路径的请求行日志(`llm/provider/openai.rs` transport 层,`→ LLM request
(openai) … tools_count=N has_system=B`)不挑调用方,**tools=0 嫌疑池封闭为三族**,
`has_system` 一刀两断:

| 调用点 | tools | system(has_system) | 输入 | 落库指纹 |
|---|---|---|---|---|
| auto 压缩摘要 `chat_loop/drive.rs` → `send_summary_completion` | 0 | **None(false)** | 大(待压区整段嵌单条 user) | messages `kind=compaction_summary` 行 + turn_trace.compaction_json |
| 手动 /compact `compaction.rs`(含 focus/retry 变体) | 0 | **None(false)** | 大 | 同上(trigger=manual) |
| auto_reflect `agent/auto_reflect.rs` `reflect_to_pitfall` | 0 | **Some(true)** | 小(单条 user) | autonomous_memories `kind=pitfall` 行 |

(`agent/subagent/truncate_summary.rs` 是纯落盘 helper,不发请求 —— 勿被名字误导。)

**cache miss 归因次序**(再见 cache_read 回退/清零时按此排序,别先立"缓存被挤占"
调查任务):①本 session 自己的压缩折叠(待压区折成摘要行 → 前缀机械性变化 → miss
by design;签名 = miss 轮邻近 compaction 事件,**部分回退到固定值** = 命中到保留区
边界);②头部易变注入(breadcrumb/instruction/head_sha,08-31-cache-head-volatility
已修);③**跨请求驱逐 —— 已实验排除**(2026-09-01:同栈 deepseek-v4-flash,S 条目
124k + 旁路 266k(对齐事故 280k 量级)插入后 S 下轮 cache_read 逐字节不变;31k/161k
档同),勿再循此假设消耗调查成本。实验方法与数字见任务
`09-01-aux-call-cache-interference/research/r2-experiment-results.md`。

**附注(脚本化 daemon API 的坑)**:`create_session` 的 `model` 参数是 legacy 标签,
**不决定实际模型**;每轮解析链 = `sessions.model_id`(优先)→ `app_config
.default_model_id`(`agent/chat.rs` lookup_provider_for_session)。按 session 指定模型
走 `POST /api/v1/providers/update_session_model_id`。验证请求实际走了哪条路径用
daemon.log 请求行(transport 名 + model + has_system)。
