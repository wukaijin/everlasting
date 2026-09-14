# GCE MCP stdio 壳退役(P4)

## Goal

P3(2026-09-15)已把用户级挂载 `~/.zcode/cli/config.json` 的 `everlasting-group-chat` 原位换成
HTTP 条目(`http://127.0.0.1:7456/mcp`)并删除 `-http` 别名(备份 `config.json.p3-dual.bak`,
工具前缀 `mcp__everlasting-group-chat__` 保持)。本任务收尾:退役 stdio 部署面的全部产物——
stdio 壳 JS、standalone bun bin、deploy 脚本及其测试/冒烟,并同步文档 / spec / 代码注释,
让 daemon `/mcp`(09-14 路径③)成为唯一 MCP 实现。

## Requirements

- R1 删除 6 个仓库文件:`scripts/group-chat-mcp.mjs`、`group-chat-mcp.test.mjs`、
  `group-chat-mcp-smoke.mjs`、`group-chat-mcp-deploy.mjs`、`group-chat-mcp-deploy.test.mjs`、
  `group-chat-mcp-standalone-entry.mjs`。
- R2 `scripts/package.json` 依赖收缩(修订:`@modelcontextprotocol/sdk` **保留**——
  `group-chat-mcp-http-smoke.mjs` 以 SDK 客户端验 daemon wire,是设计目的;`zod` 删除——
  确认零消费方)。lockfile 收敛 pnpm 单源(npm 残留 package-lock.json 删,pnpm-lock 重装再生)。
- R3 机器面清理:`~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp` +
  `.build-info` sidecar。
- R4 文档同步:AGENTS.md GCE-M2 / 收敛端点两块;DAEMON-API.md §6.1 挂载段换 HTTP +
  部署面段删 + §6.5 P3 状态翻转;GROUP-CHAT-API-ROADMAP.md §5 记 P3/P4 落地。
- R5 `app/src-tauri/src/daemon/routes/mcp.rs` 两处指向 JS 源(含行号)的注释改为语义自足,
  不留悬空引用。
- R6 spec 同步:删 `.trellis/spec/scripts/group-chat-mcp-deploy.md` + `scripts/index.md` 行;
  `group-chat-presets.md` / `pattern-group-chat-structured-summary.md` 中
  `group-chat-mcp.test.mjs` 的测试覆盖引用改指现存覆盖(run.test.mjs + mcp.rs 单测 + HTTP 冒烟)。

## Acceptance Criteria

- [ ] AC1 残留检查:`git grep -n group-chat-mcp` 在非归档区只剩「已退役」表述与 roadmap
      历史记录(含本次补记),无活引用指向被删文件。
- [ ] AC2 `node --test scripts/group-chat-run.test.mjs` 全绿(引擎 20 用例不受影响)。
- [ ] AC3 daemon 在跑时 `node scripts/group-chat-mcp-http-smoke.mjs`(非 live)全绿。
- [ ] AC4 `scripts/package.json` 依赖仅 `@modelcontextprotocol/sdk`(http-smoke 消费);
      `zod` 删除;lockfile 仅 pnpm-lock.yaml(版本与 packageManager 字段一致)。
- [ ] AC5 机器 bin 与 sidecar 已删;`~/.zcode/cli/config.json` 仅 HTTP 条目(P3 已达成,复核)。
- [ ] AC6 引擎 `group-chat-run.mjs` / `group-chat-presets.json` / `mcp.rs` 行为零改动
      (mcp.rs 仅注释);工作树无未预期脏文件。

## 边界(不做)

- 不动引擎与 Rust 行为;不做 `/mcp` 远程暴露 / 跨平台 / 其他宿主配置写入。
- 用户侧备份(`config.json.p3-dual.bak` / `config.json.mcp-deploy.bak`)保留,不清理。
- `.trellis/tasks/archive/` 历史任务文档不改(历史快照语义)。
