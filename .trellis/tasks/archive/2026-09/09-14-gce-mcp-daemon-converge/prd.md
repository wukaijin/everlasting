# MCP 能力收敛到 daemon:HTTP transport 原生 MCP endpoint

## Goal

把 group-chat MCP server 从「每宿主会话 spawn 一个 98MB bun standalone bin
(RSS 58MB/实例)」收敛为 daemon(:7456)原生 MCP streamable-HTTP endpoint
(`/mcp`),宿主以 `{"type":"http","url":...}` 挂载,MCP 子进程归零。
roadmap 既定方向(docs/GROUP-CHAT-API-ROADMAP.md §5 path③)。

## Requirements

- daemon `POST /mcp` 实现极简兼容 profile:initialize(版本 echo)/ping/
  tools/list(8 工具,wire schema 逐字段平移 JS 版)/tools/call/通知 202/
  未知方法 -32601;GET→405、DELETE→200、无 session、纯 JSON 响应、
  lenient 协议头。契约:research/mcp-wire-protocol.md。
- 八工具语义 1:1 平移(start/status+wait/detail/result/cancel/interrupt/
  inject/list_models/list_presets),行为红线(不阻塞/错误两级翻译/三处
  降级)不降级;JS→Rust 原语映射:research/daemon-converge.md §2。
- 内置四档预设编译期嵌入(include_str! 单源保持);XDG ledger 退役
  (session 行派生);转录复用 Rust 渲染器、落点参数化(D2)。
- 宿主挂载切换保 server 名 `everlasting-group-chat`(工具前缀稳定),
  双挂载并行实测后才切,bin 留回滚观察期。

## Acceptance Criteria

- [ ] AC1 协议层:Router oneshot 测试覆盖 initialize(版本 echo 五值)/
      通知 202/ping/未知方法/406/415 路径。
- [ ] AC2 tools/list wire schema 与 JS 版逐字段一致,序列化合计 ≤4200 字符
      (AC4 预算锁 Rust 化,断言锁上限)。
- [ ] AC3 SDK 客户端冒烟:StreamableHTTPClientTransport 直连 /mcp 走通
      initialize→tools/list→tools/call 错误链(非 live 零成本);--live 全链。
- [ ] AC4 宿主实测:双挂载并行期内 zcode 会话可见可用同名工具。
- [ ] AC5 切换后常驻内存:MCP 子进程数 0(daemon RSS 增量 <10MB)。

## Notes

- 调研已完成:research/mcp-wire-protocol.md(SDK 1.30.0 wire 契约反向提取)+
  research/daemon-converge.md(架构映射/决策点 D1-D7/风险/四阶段迁移)。
- 复杂任务:实现前按 workflow 补 design.md + implement.md 再 task.py start。
- 本期不做:remote tunnel 远程暴露 /mcp(须先过安全评审,roadmap §5 前置)。
