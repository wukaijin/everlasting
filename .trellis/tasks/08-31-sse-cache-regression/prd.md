# 流式与缓存回归修复:SSE 丢分片 + 提示词头部易变内容

## 背景(2026-08-31 两轮 DB/日志取证结论)

08-31 对 `everlasting.db` 全部 275 条 `is_error` tool_result 与 daemon.log 做了归因,发现两类 harness 侧缺陷:

1. **SSE 流式丢分片**(子任务 `08-31-sse-halfline-fix`):`SseParser::feed` 把 TCP chunk 尾部的不完整行当完整行处理,`data:` 行被 chunk 边界切断时后半段按 malformed 静默丢弃 → event JSON 截断 → 整个 chunk(含 `function.arguments` 分片)被 `continue` 丢弃。表现为 `Missing required parameter`(input={},全库 30 例)与**静默参数缺段**(shell 引号不闭合 ×4、old_string 中段缺失 ×12,同一编辑重试两次丢不同位置)。当天 daemon.log 有 255 次 `failed to parse SSE data JSON`。
2. **提示词头部易变内容致缓存全量失效**(子任务 `08-31-cache-head-volatility`):OpenAI 兼容路径无 cache_control 断点,前缀缓存从字节 0 严格匹配;而 breadcrumb(状态迁移即变)位于 messages[0] 头部、instruction 文件(AGENTS.md/CLAUDE.md)每次新请求 init 重读后插 messages[0..1]、head_sha 每轮刷新在 system prompt。实证:session `d6728b3a` seq 435(状态迁移改 breadcrumb)与 seq 437(新请求 init 重读被 agent 自己改过的 AGENTS/CLAUDE)两轮 cache_read=0,28 万 token 全量重付。另有并发辅助调用(tools=0,疑似 worker truncate_summary)与缓存回退的相关性待查(loop-hint 已排除:其注入在对话尾部 result 消息,不破坏前缀)。

## 任务地图

| 子任务 | 交付物 | 独立验收 |
|---|---|---|
| `08-31-sse-halfline-fix` | SseParser 半行缓冲 + 回归测试 | 单测:任意位置切 chunk 数据不丢 |
| `08-31-cache-head-volatility` | 易变注入下沉到尾部的设计与落地 | turn_trace cache_read 不再因状态迁移/新请求掉 0 |

依赖关系:两者无实现依赖,可并行;cache-head 子任务的设计评审不阻塞 sse 子任务。

## 跨子任务验收

- [ ] 两个子任务各自验收通过
- [ ] `scripts/turn-smoke.sh` 单轮烟测通过(实跑一轮 LLM 验证流式链路)
- [ ] 无新增 `failed to parse SSE data JSON` / `failed to parse tool_call arguments JSON` warn(烟测日志)

## 非目标

- 08-04 群聊 nominate_speaker 事故(已由白名单 + filter_tools_for_session_type 修复,无残留)
- 上游(聚合路由)空闲后部分缓存命中回退问题(harness 不可控)
- Anthropic 路径的缓存策略(已有 cache_control 断点,不受影响)
