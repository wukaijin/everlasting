# tools=0 生产调用点清单(封闭池)+ 判别器

> R1a 产物,2026-09-01 代码核对(`app/src-tauri/src`)。前提取证:openai transport 的
> `→ LLM request (openai) … tools_count=N has_system=B` 日志行打在
> `llm/provider/openai.rs:679`,在 `provider.send()` 内部、不挑调用方 →
> **凡是走 OpenAI 兼容路径的请求必留此行,tools=0 嫌疑池封闭**。

## 1. 调用点全景

| # | 调用点 | tools | system | 输入规模 | 触发 | 落库指纹 |
|---|--------|-------|--------|----------|------|----------|
| 1 | 主 loop 本体 `agent/chat_loop/drive.rs:1189`(`retry_open`) | 会话工具集 | Some | 全量历史 | 每轮 | turn_trace 常规行 |
| 1a | worker 复用同路径(worker 自跑 run_chat_loop) | worker 白名单集 | Some | worker 历史 | dispatch 后每轮 | subagent_runs 关联 |
| 2 | **auto 压缩摘要** `agent/chat_loop/drive.rs:2611` → `send_summary_completion` | **0** | **None** | **大**(待压区整段嵌单条 user prompt) | loop 内水位触发 | messages 落 `kind=compaction_summary` 行;turn_trace.compaction_json 有事件 |
| 3 | 手动 /compact `agent/compaction.rs:949` + focus/retry 变体 `1220/1248`(同 `send_summary_completion`,`compaction.rs:775`,`retry_open(..., None, vec![req], vec![], ...)`) | **0** | **None** | **大**(同上) | 用户手动 | 同上(手动路径) |
| 4 | **auto_reflect** `agent/auto_reflect.rs:453`(`provider.send(Some(REFLECT_SYSTEM_PROMPT), msgs, vec![])`,fire-and-forget spawn) | **0** | **Some** | **小**(单条 user,截断的失败上下文) | 失败反思触发 | autonomous_memories 落 `kind=pitfall` 行 |

排除:`agent/subagent/truncate_summary.rs` 是纯格式化/落盘 helper(transcript cap /
final_text 格式化),**不发 LLM 请求** —— 父任务归因(08-31 R5)据此作废。

## 2. 判别器(远端日志/DB 归因用)

日志行直接判 `has_system`:

- `tools_count=0 has_system=false` → **压缩摘要族(#2/#3)**(唯一 system=None 的调用点)
- `tools_count=0 has_system=true` → **auto_reflect(#4)**(唯一 tools=0 且带 system)

进一步分 #2 vs #3、确认 #4:

- messages 表该 session 在时刻附近有无 `metadata.kind='compaction_summary'` 行
  (常量 `COMPACTION_SUMMARY_KIND="compaction_summary"`,`compaction.rs:75`);
  turn_trace.compaction_json 同轮有事件 → #2(auto);无 auto 痕迹但有手动 compact
  的 session_audit/请求 → #3。
- autonomous_memories 该 session/project 时刻附近有 `kind='pitfall'` 新行 → #4 佐证。
- tools=13 紧邻跟随者身份:subagent_runs 表 created_at 匹配 → worker 本体;否则查
  是否其他 session / C7D stub 态的主 loop。

## 3. 框架修正:机械性 miss vs 驱逐(本调查的核心分叉)

父任务把 seq 285 miss 当"缓存被挤占"的异常;但存在一个**无需驱逐假说的平凡解释**:

- 若 09:12:58 的 tools=0 请求**就是该 session 自己的 auto 压缩**(#2),则压缩把待压区
  折叠成 summary 行 → **seq 285 的请求前缀本来就变了** → cache_read=0(或部分回退)
  是**by design 的机械性 miss**,与上游缓存容量无关。
- 只有当 tools=0 请求**不属于该 session**(别 session 的压缩 / auto_reflect)而主 loop
  前缀未变却 miss 时,"大辅助请求插入条目 → LRU 驱逐主 loop 热条目"才成立。
- 佐证:证据里 "09:24:56 tools=0 → seq 401 **部分回退 154,112**" 的形态恰是
  "命中到保留区边界"的压缩折叠签名(摘要行插在保留区之后,前缀保留段仍命中)。

→ R1b 归因的第一判别问题:**seq 285 前该 session 有没有自己的压缩事件**。
→ R2 实验据此分臂:A 对照(纯连跑)/ B 自压缩(预期机械性 miss)/ C 跨 session
  大辅助调用(驱逐假说的唯一决定性检验)。
