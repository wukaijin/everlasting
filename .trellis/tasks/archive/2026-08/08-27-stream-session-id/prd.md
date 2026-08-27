# chat 事件 payload 补 session_id:跨客户端实时认领

## Goal

修复 remote web 偶尔不能实时显示流式变化的问题:local 输入(或裸 HTTP 发 daemon)时,remote PWA 收不到实时流,只能刷新看。

## 根因(代码链路确认)

事件 payload 只带发起端生成的 `request_id`,不带 `session_id`。非发起端的 `activeRequests` 没有该 rid 映射,`streamEvents.ts` 的未知-request 守卫(旧 `if (!req) return`)把事件全部静默丢弃;刷新走 DB 快照重拉,所以一刷就有。事件分发本身(daemon `SseRegistry` 全局广播 → `/api/v1/stream` → remote 隧道桥)是全量透传的,断点在消费侧路由。

## Requirements

- 后端 3 个高频通道(`ChatEventPayload` / `ToolCallPayload` / `ToolResultPayload`)补必填 `session_id` 字段;permission/question/mode/task 四通道本就带,不动。
- 前端对未知 rid 且带 `session_id` 的事件按 session 认领(建外来 RequestState + 新 assistant 占位),后续 delta/tool/done 走既有路径。
- 旧 daemon wire 兼容:payload 无 `session_id` 时维持原「未知 request 即丢弃」语义。
- 已完结 rid 的迟到事件不复活(completedRequests 命中即丢弃)。

## Acceptance Criteria

- [x] 后端 `cargo test --lib` 全绿(2008 passed)。
- [x] 前端 `pnpm test` 全绿(1273 passed,含 5 个新增跨客户端认领用例:delta 认领+占位、旧 wire 不认领、完结后迟到事件丢弃、未加载 session 的 done 收尾、tool 通道认领)。
- [x] `scripts/turn-smoke.sh` 实跑一轮真实 LLM 冒烟通过。
- [x] 抓实际 SSE wire 确认 `start` / `thinking_delta` / `signature_delta` / `delta` / `turn_usage` / `turn_complete` / `done` 事件均携带 `session_id` 且与请求 session 一致。

## Notes

- 验证提交:`68f7cadc`(feat(stream): chat 事件 payload 补 session_id,支持跨客户端实时认领)。
- 兼容性:新 daemon + 旧前端 = 前端忽略多余字段;旧 daemon + 新前端 = 前端回退旧丢弃语义。同仓库两端同步发布无灰阶问题。
- 设计取舍:外来请求的 `projectId` 置空(streamingProjectIds 项目红点对 cross-client 流不亮);`groupChat` 置 false(群聊跨客户端认领不做 —— 事件不携带该标记,需后端另行补字段,超出本任务范围)。
